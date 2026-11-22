//! Tray icons (`cx.set_tray`): `Shell_NotifyIcon` in the notification
//! area. The icon is the SF Symbol's Segoe Fluent Icons glyph in the
//! taskbar's color (else the app's icon), the tooltip (else the title) is
//! its tip, and a click, a right click or Enter opens its menu: a Win32
//! popup menu, the notification area's own idiom, dark when the system's
//! apps are. Choices come back as `UiEvent::MenuSelect`.
//!
//! The icons are the services window's (`shell.rs`): they come back when
//! Explorer restarts (`TaskbarCreated`) and go when the app quits.

use std::cell::RefCell;

use prism_core::UiEvent;
use prism_core::menu::{MenuItemSpec, MenuSpec, TraySpec};
use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER,
    Shell_NotifyIconGetRect, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCSTR, PCWSTR, w};

use crate::shell;
use crate::win::{env, hiword, log, loword, wide};

/// `NIN_SELECT | NINF_KEY`: Enter or Space on the icon.
const NIN_KEYSELECT: u32 = NIN_SELECT | 0x1;

struct Icon {
    uid: u32,
    spec: TraySpec,
    hicon: HICON,
    /// Drawn here (destroyed with the icon), not a shared system icon.
    owned: bool,
}

thread_local! {
    static ICONS: RefCell<Vec<Icon>> = const { RefCell::new(Vec::new()) };
    static NEXT_UID: std::cell::Cell<u32> = const { std::cell::Cell::new(1) };
    static THEMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the taskbar is light (`SystemUsesLightTheme`).
fn light_taskbar() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let mut v = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok() && v == 1
}

/// The icon for a spec: its glyph, else the app's icon.
fn icon_for(spec: &TraySpec) -> (HICON, bool) {
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) };
    let color = if light_taskbar() {
        prism_core::rgb(0x1A1A1A)
    } else {
        prism_core::Color::WHITE
    };
    if let Some(g) = spec.icon.as_deref().and_then(crate::keys::fluent_icon)
        && let Some(icon) = crate::icons::glyph(g, size, color)
    {
        return (icon, true);
    }
    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        // The executable's first icon (MAKEINTRESOURCE(1)), else Windows'
        // application icon.
        let icon = LoadIconW(Some(module.into()), PCWSTR(std::ptr::without_provenance(1)))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default();
        (icon, false)
    }
}

fn data(icon: &Icon) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: shell::window(),
        uID: icon.uid,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: shell::WM_TRAY,
        hIcon: icon.hicon,
        ..Default::default()
    };
    let s = &icon.spec;
    let tip = s
        .tooltip
        .clone()
        .or_else(|| s.title.clone())
        .unwrap_or_default();
    // Copied whole: the struct is packed on x86 (no reference to a field).
    let mut text = [0u16; 128];
    for (d, c) in text.iter_mut().zip(tip.encode_utf16().take(127)) {
        *d = c;
    }
    nid.szTip = text;
    nid
}

fn add(icon: &Icon) {
    let mut nid = data(icon);
    unsafe {
        if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            log!("tray {:?}: Shell_NotifyIcon failed", icon.spec.id);
            return;
        }
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
}

fn destroy(icon: &Icon) {
    if icon.owned {
        unsafe {
            let _ = DestroyIcon(icon.hicon);
        }
    }
}

/// Adds the tray item, or updates the one with the same id.
pub fn set(spec: TraySpec) {
    theme_menus();
    let (hicon, owned) = icon_for(&spec);
    ICONS.with(|icons| {
        let mut icons = icons.borrow_mut();
        if let Some(i) = icons.iter_mut().find(|i| i.spec.id == spec.id) {
            destroy(i);
            i.spec = spec;
            i.hicon = hicon;
            i.owned = owned;
            let nid = data(i);
            unsafe {
                if !Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() {
                    add(i);
                }
            }
            return;
        }
        let uid = NEXT_UID.get();
        NEXT_UID.set(uid + 1);
        let icon = Icon {
            uid,
            spec,
            hicon,
            owned,
        };
        add(&icon);
        // `PRISM_TRAY_MENU=<id>` (development, smoke tests): the item's menu
        // opens by itself, as after a click.
        if env("PRISM_TRAY_MENU").as_deref() == Some(icon.spec.id.as_str()) {
            let (menu, at) = (icon.spec.menu.clone(), place(uid));
            shell::timer(1000, move || show(&menu, at));
        }
        icons.push(icon);
    });
}

/// Where an icon is on screen (its top-left corner), logged; the cursor
/// when the shell does not say (the icon is in the overflow).
fn place(uid: u32) -> POINT {
    let id = NOTIFYICONIDENTIFIER {
        cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
        hWnd: shell::window(),
        uID: uid,
        ..Default::default()
    };
    unsafe {
        match Shell_NotifyIconGetRect(&id) {
            Ok(r) => {
                log!(
                    "tray icon at {},{} {}x{}",
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top
                );
                POINT {
                    x: r.left,
                    y: r.top,
                }
            }
            Err(e) => {
                log!("tray icon: not shown in the taskbar ({e})");
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                p
            }
        }
    }
}

fn delete(icon: &Icon) {
    let nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: shell::window(),
        uID: icon.uid,
        ..Default::default()
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
    destroy(icon);
}

pub fn remove(id: &str) {
    ICONS.with(|icons| {
        icons.borrow_mut().retain(|i| {
            if i.spec.id == id {
                delete(i);
                false
            } else {
                true
            }
        })
    });
}

/// Every icon goes (the app quits).
pub fn remove_all() {
    ICONS.with(|icons| {
        for i in icons.borrow_mut().drain(..) {
            delete(&i);
        }
    });
}

/// Explorer started again: the icons are added back.
pub fn taskbar_created() {
    ICONS.with(|icons| {
        for i in icons.borrow().iter() {
            add(i);
        }
    });
}

/// The taskbar's colors changed: glyphs are drawn again.
pub fn retheme() {
    let specs: Vec<TraySpec> = ICONS.with(|i| i.borrow().iter().map(|i| i.spec.clone()).collect());
    for s in specs {
        set(s);
    }
}

/// A message from an icon (`NOTIFYICON_VERSION_4`: the event in
/// `LOWORD(lParam)`, the icon in `HIWORD(lParam)`, where in `wParam`).
pub fn message(wp: WPARAM, lp: LPARAM) {
    let event = loword(lp.0 as usize) as u32;
    let uid = hiword(lp.0 as usize) as u32;
    if !matches!(event, NIN_SELECT | NIN_KEYSELECT | WM_CONTEXTMENU) {
        return;
    }
    let x = loword(wp.0) as i16 as i32;
    let y = hiword(wp.0) as i16 as i32;
    let menu = ICONS.with(|icons| {
        icons
            .borrow()
            .iter()
            .find(|i| i.uid == uid)
            .map(|i| i.spec.menu.clone())
    });
    if let Some(menu) = menu {
        // A message's handler: the menu's loop runs after it returns.
        shell::later(move || show(&menu, POINT { x, y }));
    }
}

/// Shows a menu at `at` (screen pixels) and reports the choice.
pub fn show(spec: &MenuSpec, at: POINT) {
    let mut ids = Vec::new();
    let Some(menu) = build(spec, &mut ids) else {
        return;
    };
    let owner = shell::window();
    let chosen = unsafe {
        // So the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(owner);
        let align = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 {
            TPM_RIGHTALIGN
        } else {
            TPM_LEFTALIGN
        };
        let r = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | align).0,
            at.x,
            at.y,
            owner,
            None,
        );
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        r.0
    };
    if chosen > 0
        && let Some(id) = ids.get(chosen as usize - 1)
    {
        crate::imp::dispatch(UiEvent::MenuSelect(*id));
    }
}

/// A popup menu; `ids[n - 1]` is the Prism id of command `n`.
fn build(spec: &MenuSpec, ids: &mut Vec<u64>) -> Option<HMENU> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for item in &spec.items {
            match item {
                MenuItemSpec::Action {
                    id,
                    title,
                    checked,
                    enabled,
                    ..
                } => {
                    ids.push(*id);
                    let mut flags = MF_STRING;
                    if !enabled {
                        flags |= MF_GRAYED;
                    }
                    if *checked == Some(true) {
                        flags |= MF_CHECKED;
                    }
                    let t = wide(&title.replace('&', "&&"));
                    let _ = AppendMenuW(menu, flags, ids.len(), PCWSTR(t.as_ptr()));
                }
                MenuItemSpec::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
                MenuItemSpec::Submenu { title, menu: m, .. } => {
                    if let Some(sub) = build(m, ids) {
                        let t = wide(&title.replace('&', "&&"));
                        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, PCWSTR(t.as_ptr()));
                    }
                }
                // Platform actions (#161) belong to a window's menus.
                MenuItemSpec::System { .. } => {}
            }
        }
        Some(menu)
    }
}

/// Popup menus follow the apps' light or dark mode (`PRISM_APPEARANCE`
/// too): uxtheme's `SetPreferredAppMode` (ordinal 135, Windows 10 1903 and
/// later, what Explorer and browsers use), then `FlushMenuThemes` (136).
fn theme_menus() {
    if THEMED.replace(true) {
        return;
    }
    unsafe {
        let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) else {
            return;
        };
        if let Some(f) = GetProcAddress(ux, PCSTR(135 as *const u8)) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            // AllowDark 1, ForceDark 2, ForceLight 3.
            let mode = match std::env::var("PRISM_APPEARANCE").as_deref() {
                Ok("dark") => 2,
                Ok("light") => 3,
                _ => 1,
            };
            set_mode(mode);
        }
        if let Some(f) = GetProcAddress(ux, PCSTR(136 as *const u8)) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}
