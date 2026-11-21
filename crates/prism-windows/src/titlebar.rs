//! Custom window chrome (`WindowChrome::Custom`; `chrome.rs` has the
//! rules): the window's `AppWindow` extends the content into its title bar
//! (`AppWindowTitleBar::ExtendsContentIntoTitleBar`); the system still
//! draws the caption buttons, over the content's top right corner, on a
//! transparent background and in the app's light or dark theme. The
//! `.window_drag()` views are the window's caption regions and what keeps
//! its presses inside them passthrough regions (`InputNonClientPointerSource`),
//! set again after each layout and scroll; `cx.window_controls()` is the
//! buttons' corner (`RightInset` or `LeftInset`, `Height`).

use core::ffi::c_void;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};

use prism_core::WindowControls;
use windows::Foundation::IReference;
use windows::Graphics::RectInt32;
use windows::UI::Color;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::IAgileObject;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows_core::{
    GUID, HRESULT, IInspectable, IInspectable_Vtbl, IUnknown, IUnknown_Vtbl, Interface, Result,
};

use crate::chrome;
use crate::imp::State;
use crate::win::log;
use crate::winui::bindings::Microsoft::UI::Input::{
    InputNonClientPointerSource, NonClientRegionKind,
};
use crate::winui::bindings::Microsoft::UI::Windowing::{
    AppWindow, AppWindowTitleBar, TitleBarTheme,
};

/// Regions in pixels: captions, passthroughs.
type Pixels = (Vec<[i32; 4]>, Vec<[i32; 4]>);

struct Bar {
    hwnd: HWND,
    _window: AppWindow,
    title: AppWindowTitleBar,
    input: InputNonClientPointerSource,
    /// The regions set last (`None`: set them again).
    regions: Option<Pixels>,
    /// The caption buttons' frame reported last.
    reported: Option<Option<WindowControls>>,
}

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

/// Takes window `hwnd`'s title bar away (once its XAML island is there:
/// the caption buttons go above it). Returns whether it could.
pub fn install(hwnd: HWND, dark: bool) -> bool {
    match make(hwnd, dark) {
        Ok(Some(bar)) => {
            BAR.with(|b| *b.borrow_mut() = Some(bar));
            true
        }
        Ok(None) => {
            log!("custom window chrome: the system cannot customize the title bar");
            false
        }
        Err(e) => {
            log!("custom window chrome: {e}");
            false
        }
    }
}

fn make(hwnd: HWND, dark: bool) -> Result<Option<Bar>> {
    if !AppWindowTitleBar::IsCustomizationSupported()? {
        return Ok(None);
    }
    let id = crate::host::window_id(hwnd)?;
    let window = AppWindow::GetFromWindowId(id)?;
    let title = window.TitleBar()?;
    title.SetExtendsContentIntoTitleBar(true)?;
    // The content shows behind the buttons (only their hover and press
    // backgrounds are drawn).
    let clear = boxed_color(Color {
        A: 0,
        R: 0,
        G: 0,
        B: 0,
    });
    if let Err(e) = title
        .SetButtonBackgroundColor(&clear)
        .and_then(|()| title.SetButtonInactiveBackgroundColor(&clear))
    {
        log!("caption buttons' background: {e}");
    }
    title.SetPreferredTheme(theme(dark))?;
    let input = InputNonClientPointerSource::GetForWindowId(id)?;
    Ok(Some(Bar {
        hwnd,
        _window: window,
        title,
        input,
        regions: None,
        reported: None,
    }))
}

fn theme(dark: bool) -> TitleBarTheme {
    if dark {
        TitleBarTheme::Dark
    } else {
        TitleBarTheme::Light
    }
}

/// The window, its title bar and its pointer source, out of [`BAR`]: the
/// system calls on them may send the window messages that use it.
fn handles() -> Option<(HWND, AppWindowTitleBar, InputNonClientPointerSource)> {
    BAR.with(|b| {
        b.borrow()
            .as_ref()
            .map(|bar| (bar.hwnd, bar.title.clone(), bar.input.clone()))
    })
}

/// The app turned light or dark: so do the caption buttons.
pub fn set_dark(dark: bool) {
    if let Some((_, title, _)) = handles()
        && let Err(e) = title.SetPreferredTheme(theme(dark))
    {
        log!("caption buttons' theme: {e}");
    }
}

/// The window changed size: the regions are set again after the next
/// layout (the system lays out its own on a resize).
pub fn invalidate() {
    BAR.with(|b| {
        if let Some(bar) = &mut *b.borrow_mut() {
            bar.regions = None;
        }
    });
}

/// The drag regions follow the views (after a layout or a scroll).
pub fn update_regions(s: &State) {
    let Some((_, _, input)) = handles() else {
        return;
    };
    let w = &s.renderer.widgets;
    let k = w.scale();
    let r = chrome::regions(&w.scene, |id, _| w.keeps_presses(id));
    // The views start under the menu bar's row.
    let top = crate::menubar::height();
    let px = |r: &crate::scene::Rect, grow| {
        chrome::px(&crate::scene::Rect::new(r.x, r.y + top, r.w, r.h), k, grow)
    };
    let pixels: Pixels = (
        r.caption.iter().map(|r| px(r, false)).collect(),
        r.passthrough.iter().map(|r| px(r, true)).collect(),
    );
    let same = BAR.with(|b| {
        b.borrow()
            .as_ref()
            .is_some_and(|bar| bar.regions.as_ref() == Some(&pixels))
    });
    if same {
        return;
    }
    match set_regions(&input, &pixels) {
        Ok(()) => BAR.with(|b| {
            if let Some(bar) = &mut *b.borrow_mut() {
                bar.regions = Some(pixels);
            }
        }),
        Err(e) => log!("window drag regions: {e}"),
    }
}

fn set_regions(input: &InputNonClientPointerSource, (caption, pass): &Pixels) -> Result<()> {
    let rects = |v: &[[i32; 4]]| -> Vec<RectInt32> {
        v.iter()
            .map(|&[x, y, w, h]| RectInt32 {
                X: x,
                Y: y,
                Width: w,
                Height: h,
            })
            .collect()
    };
    for (kind, v) in [
        (NonClientRegionKind::Caption, caption),
        (NonClientRegionKind::Passthrough, pass),
    ] {
        if v.is_empty() {
            // No drag view: no caption region at all, not the system's
            // default strip.
            input.ClearRegionRects(kind)?;
        } else {
            input.SetRegionRects(kind, &rects(v))?;
        }
    }
    Ok(())
}

/// Reports where the caption buttons are (`cx.window_controls()`) when
/// that changed: re-renders the app.
pub fn report_controls(s: &mut State) {
    let Some((hwnd, title, _)) = handles() else {
        return;
    };
    let mut r = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut r) }.is_err() || r.right <= 0 {
        // Minimized: they stay where they were.
        return;
    }
    let controls = chrome::controls(
        r.right,
        title.LeftInset().unwrap_or(0),
        title.RightInset().unwrap_or(0),
        title.Height().unwrap_or(0),
        s.renderer.widgets.scale(),
    );
    let changed = BAR.with(|b| {
        let mut b = b.borrow_mut();
        let bar = b.as_mut()?;
        (bar.reported != Some(controls)).then(|| bar.reported = Some(controls))
    });
    if changed.is_some() {
        let patches = s.runtime.set_window_chrome(false, controls);
        crate::imp::commit(s, patches);
    }
}

// --- A boxed color ---------------------------------------------------------

/// `IReference<Color>`'s vtable: `IInspectable`'s, then `get_Value`.
#[repr(C)]
struct ColorVtbl {
    base: IInspectable_Vtbl,
    value: unsafe extern "system" fn(*mut c_void, *mut Color) -> HRESULT,
}

/// A `Color` as the `IReference<Color>` the title bar's colors take
/// (windows-rs boxes no structs): written by hand, like `boxed.rs`.
#[repr(C)]
struct ColorBox {
    vtable: *const ColorVtbl,
    refs: AtomicU32,
    color: Color,
}

static COLOR_VTABLE: ColorVtbl = ColorVtbl {
    base: IInspectable_Vtbl {
        base: IUnknown_Vtbl {
            QueryInterface: color_qi,
            AddRef: color_add_ref,
            Release: color_release,
        },
        GetIids: color_iids,
        GetRuntimeClassName: color_class_name,
        GetTrustLevel: color_trust_level,
    },
    value: color_value,
};

fn boxed_color(color: Color) -> IReference<Color> {
    let b = Box::new(ColorBox {
        vtable: &COLOR_VTABLE,
        refs: AtomicU32::new(1),
        color,
    });
    // SAFETY: a COM object laid out with its vtable first, one reference.
    unsafe { IReference::from_raw(Box::into_raw(b) as *mut c_void) }
}

unsafe fn color_box<'a>(p: *mut c_void) -> &'a ColorBox {
    unsafe { &*(p as *const ColorBox) }
}

unsafe extern "system" fn color_qi(
    p: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe {
        let iid = &*iid;
        if *iid == IUnknown::IID
            || *iid == IInspectable::IID
            || *iid == IAgileObject::IID
            || *iid == <IReference<Color> as Interface>::IID
        {
            color_box(p).refs.fetch_add(1, Ordering::Relaxed);
            *out = p;
            return HRESULT(0);
        }
        *out = core::ptr::null_mut();
        HRESULT(0x8000_4002_u32 as i32) // E_NOINTERFACE
    }
}

unsafe extern "system" fn color_add_ref(p: *mut c_void) -> u32 {
    unsafe { color_box(p).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn color_release(p: *mut c_void) -> u32 {
    unsafe {
        let left = color_box(p).refs.fetch_sub(1, Ordering::Release) - 1;
        if left == 0 {
            std::sync::atomic::fence(Ordering::Acquire);
            drop(Box::from_raw(p as *mut ColorBox));
        }
        left
    }
}

unsafe extern "system" fn color_value(p: *mut c_void, out: *mut Color) -> HRESULT {
    unsafe { *out = color_box(p).color };
    HRESULT(0)
}

unsafe extern "system" fn color_iids(
    _: *mut c_void,
    count: *mut u32,
    iids: *mut *mut GUID,
) -> HRESULT {
    unsafe {
        *count = 0;
        *iids = core::ptr::null_mut();
    }
    HRESULT(0)
}

unsafe extern "system" fn color_class_name(_: *mut c_void, name: *mut *mut c_void) -> HRESULT {
    unsafe { *name = core::ptr::null_mut() };
    HRESULT(0)
}

unsafe extern "system" fn color_trust_level(_: *mut c_void, level: *mut i32) -> HRESULT {
    unsafe { *level = 0 };
    HRESULT(0)
}
