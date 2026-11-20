//! The app's window and UI thread: `run`, the window procedure, the
//! message loop, and the dispatch / commit cycle shared with the other
//! native backends (prism-linux's).
//!
//! One UI thread, an OLE single-threaded apartment (clipboard, drag and
//! drop, UI Automation). UI events are queued and dispatched from the
//! message loop (`WM_PRISM_DISPATCH`): Win32 calls back while Prism
//! changes the window (`SetWindowPos`, `SetWindowTextW` on fields), and
//! nothing raised then is reported (`UPDATING`).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;

use prism_core::{AppConfig, AppInit, NodeId, NodeKind, Patch, Runtime, UiEvent};
use prism_native::{Insets, NativeRenderer};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, PAINTSTRUCT, ScreenToClient, UpdateWindow,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows::Win32::UI::Controls::RichEdit::EN_SELCHANGE;
use windows::Win32::UI::Controls::{NMHDR, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardState, MAPVK_VK_TO_VSC, MapVirtualKeyW, TME_LEAVE, TRACKMOUSEEVENT, ToUnicode,
    TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, w};

use crate::gfx;
use crate::images::Images;
use crate::input;
use crate::scene::Target;
use crate::services::WinServices;
use crate::system;
use crate::widgets::Win;
use crate::win::*;

pub struct State {
    pub runtime: Runtime,
    pub renderer: NativeRenderer<Win>,
    pub services: WinServices,
}

type Deferred = Box<dyn FnOnce(&mut State)>;

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// Set while Prism changes the window: what Win32 reports meanwhile
    /// (field edits, focus) is Prism's own doing.
    static UPDATING: Cell<bool> = const { Cell::new(false) };
    static QUEUE: RefCell<VecDeque<UiEvent>> = const { RefCell::new(VecDeque::new()) };
    static DEFERRED: RefCell<Vec<Deferred>> = const { RefCell::new(Vec::new()) };
    static MAIN: Cell<isize> = const { Cell::new(0) };
    /// Closing asks first (#158).
    static UNSAVED: Cell<bool> = const { Cell::new(false) };
    /// Clicks in a quick series (double clicks).
    static CLICKS: Cell<u32> = const { Cell::new(1) };
    static TRACKING: Cell<bool> = const { Cell::new(false) };
}

pub fn main_hwnd() -> HWND {
    hwnd(MAIN.get())
}

/// Runs `f` with the state, unless it is already in use (Win32 called
/// back while Prism changes the window).
pub fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

fn post(msg: u32) {
    unsafe {
        let _ = PostMessageW(Some(main_hwnd()), msg, WPARAM(0), LPARAM(0));
    }
}

/// A UI event, dispatched from the message loop.
pub fn dispatch(event: UiEvent) {
    if UPDATING.get() {
        return;
    }
    QUEUE.with(|q| q.borrow_mut().push_back(event));
    post(WM_PRISM_DISPATCH);
}

/// Work that needs the state, run from the message loop.
pub fn defer(f: Deferred) {
    DEFERRED.with(|d| d.borrow_mut().push(f));
    post(WM_PRISM_DISPATCH);
}

/// Runs the queued work and events now.
pub fn drain() {
    loop {
        let work: Vec<Deferred> = DEFERRED.with(|d| std::mem::take(&mut *d.borrow_mut()));
        let events: Vec<UiEvent> = QUEUE.with(|q| q.borrow_mut().drain(..).collect());
        if work.is_empty() && events.is_empty() {
            return;
        }
        let ran = with_state(|s| {
            for f in work {
                f(s);
            }
            for e in events {
                deliver(s, e);
            }
        });
        if ran.is_none() {
            return;
        }
    }
}

fn deliver(s: &mut State, event: UiEvent) {
    let (event, restyled) = s.renderer.locate_drag(event);
    if restyled {
        relayout(s);
    }
    let patches = s.runtime.dispatch(event);
    commit(s, patches);
}

/// Applies patches, lays out, repaints what changed, carries out the
/// app's requests, schedules the runtime's next timer.
pub fn commit(s: &mut State, patches: Vec<Patch>) {
    let was = UPDATING.replace(true);
    if !patches.is_empty() {
        prism_core::Backend::apply(&mut s.renderer, &patches);
    }
    relayout(s);
    UPDATING.set(was);
    // `cx.focus(…)`, `dismiss_keyboard()`.
    let requests = std::mem::take(&mut s.renderer.widgets.focus_requests);
    for r in requests {
        let keyboard = s.renderer.widgets.focus_visible;
        input::set_focus(s, r, keyboard);
    }
    for request in s.runtime.take_requests() {
        match request {
            prism_core::PlatformRequest::SetTitle(title) => {
                let w = &s.renderer.widgets;
                let t = wide(title.as_deref().unwrap_or(&w.app_title));
                unsafe {
                    let _ = SetWindowTextW(w.hwnd, PCWSTR(t.as_ptr()));
                }
            }
            prism_core::PlatformRequest::SetUnsavedChanges(on) => UNSAVED.set(on),
            prism_core::PlatformRequest::LoadFont(face) => {
                // Phase 3: DirectWrite in-memory font sets.
                log!(
                    "load_font({:?}): run-time fonts are not loaded on Windows yet",
                    face.family
                );
            }
            other => prism_core::PlatformServices::handle(&mut s.services, other),
        }
    }
    if let Some(at) = s.runtime.next_timer() {
        let ms = at
            .saturating_duration_since(std::time::Instant::now())
            .as_millis()
            .clamp(1, u32::MAX as u128) as u32;
        unsafe {
            SetTimer(Some(main_hwnd()), TIMER_RUNTIME, ms, None);
        }
    }
}

/// Lays the tree out in the window again, then repaints what moved.
pub fn relayout(s: &mut State) {
    let (w, h) = s.renderer.widgets.size;
    if w > 0. {
        if (w, h) != s.runtime.app.viewport() {
            let patches = s.runtime.set_viewport(w, h);
            if !patches.is_empty() {
                prism_core::Backend::apply(&mut s.renderer, &patches);
            }
        }
        s.renderer.layout(w, h, Insets::default());
        let more = prism_native::report_layout(&mut s.runtime, &mut s.renderer);
        if !more.is_empty() {
            commit(s, more);
        }
    }
    s.renderer.widgets.flush();
}

fn poll() {
    with_state(|s| {
        let patches = s.runtime.poll();
        commit(s, patches);
    });
}

/// A field took or lost the Win32 focus.
pub fn field_focus(id: NodeId, on: bool) {
    defer(Box::new(move |s| {
        let w = &mut s.renderer.widgets;
        let previous = w.focus;
        if on {
            w.focus = Some(id);
        } else if w.focus == Some(id) {
            w.focus = None;
        } else {
            return;
        }
        if let Some(p) = previous.filter(|p| *p != id)
            && let Some(n) = w.scene.get_mut(p)
        {
            n.focused = false;
            w.mark(p);
        }
        if let Some(n) = w.scene.get_mut(id) {
            n.focused = on;
        }
        w.mark(id);
        w.style_field(id);
        let mut restyled = false;
        if let Some(p) = previous.filter(|p| *p != id && on) {
            restyled |= s
                .renderer
                .set_interaction(p, prism_core::Interaction::Focus, false);
        }
        restyled |= s
            .renderer
            .set_interaction(id, prism_core::Interaction::Focus, on);
        if restyled {
            relayout(s);
        }
        for e in s.renderer.take_focus_changes() {
            dispatch(e);
        }
        crate::uia::focus_changed(s.renderer.widgets.hwnd, on.then_some(id));
        s.renderer.widgets.flush();
    }));
}

/// Tab in a field: the focus moves on, from the message loop.
pub fn post_focus_step(back: bool) {
    unsafe {
        let _ = PostMessageW(
            Some(main_hwnd()),
            WM_PRISM_FOCUS,
            WPARAM(back as usize),
            LPARAM(0),
        );
    }
}

/// Types `text` into a field like the user would (replacing its text):
/// the edit is reported as usual.
pub fn fill(field: HWND, text: &str) {
    let t = wide(&text.replace('\n', "\r\n"));
    unsafe {
        SendMessageW(
            field,
            0x00B1, /* EM_SETSEL */
            Some(WPARAM(0)),
            Some(LPARAM(-1)),
        );
        SendMessageW(
            field,
            0x00C2, /* EM_REPLACESEL */
            Some(WPARAM(1)),
            Some(LPARAM(t.as_ptr() as isize)),
        );
    }
    field_changed(field);
}

/// A field's text changed (`EN_CHANGE`).
fn field_changed(field: HWND) {
    if UPDATING.get() {
        return;
    }
    let Some((id, text)) = crate::fields::changed(field) else {
        return;
    };
    with_state(|s| s.renderer.widgets.typed.entry(id).or_default().sent(&text));
    dispatch(UiEvent::Input(id, text));
}

pub fn set_dark_title_bar(h: HWND, dark: bool) {
    let on = BOOL::from(dark);
    unsafe {
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &on as *const BOOL as *const c_void,
            size_of::<BOOL>() as u32,
        );
    }
}

/// The character a key types with the current layout, without touching
/// dead-key state.
fn typed_char(vk: u32) -> Option<char> {
    let mut state = [0u8; 256];
    unsafe {
        GetKeyboardState(&mut state).ok()?;
        let scan = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC);
        let mut buf = [0u16; 8];
        let n = ToUnicode(vk, scan, Some(&state), &mut buf, 4);
        if n <= 0 {
            return None;
        }
        char::decode_utf16(buf[..n as usize].iter().copied())
            .next()
            .and_then(Result::ok)
    }
}

fn dips(lparam: LPARAM) -> (f32, f32) {
    let (x, y) = point_of(lparam);
    let k = with_state(|s| s.renderer.widgets.scale()).unwrap_or(1.);
    (x as f32 / k, y as f32 / k)
}

/// Every second: timers count down.
fn tick() {
    with_state(|s| {
        let w = &mut s.renderer.widgets;
        let timers: Vec<NodeId> = w
            .scene
            .nodes
            .iter()
            .filter(|(_, n)| n.kind == NodeKind::Timer)
            .map(|(id, _)| *id)
            .collect();
        let mut changed = false;
        for id in timers {
            let Some(n) = w.scene.get_mut(id) else {
                continue;
            };
            let text = prism_core::timer_text(n.value, &n.placeholder);
            if text != n.text {
                n.text = text;
                w.texts.forget(id);
                w.mark(id);
                changed = true;
            }
        }
        if changed {
            s.renderer.mark_all_dirty();
            relayout(s);
        }
    });
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(h, &mut ps);
                let r = ps.rcPaint;
                let update = (r.right > r.left && r.bottom > r.top).then_some(r);
                with_state(|s| s.renderer.widgets.paint(hdc, update));
                let _ = EndPaint(h, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE => {
                let (pw, ph) = (loword(lp.0 as usize) as f32, hiword(lp.0 as usize) as f32);
                with_state(|s| {
                    let w = &mut s.renderer.widgets;
                    let k = w.scale();
                    w.size = (pw / k, ph / k);
                    w.repaint_all();
                    w.moved = true;
                    let was = UPDATING.replace(true);
                    relayout(s);
                    UPDATING.set(was);
                });
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let dpi = loword(wp.0) as u32;
                let r = &*(lp.0 as *const RECT);
                with_state(|s| {
                    let w = &mut s.renderer.widgets;
                    w.dpi = dpi;
                    let ids: Vec<NodeId> = w
                        .scene
                        .nodes
                        .iter()
                        .filter(|(_, n)| n.field.is_some())
                        .map(|(id, _)| *id)
                        .collect();
                    for id in ids {
                        w.style_field(id);
                    }
                    w.repaint_all();
                    w.moved = true;
                });
                let _ = SetWindowPos(
                    h,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lp.0 as *mut MINMAXINFO);
                let dpi = GetDpiForWindow(h).max(96);
                let mut r = RECT {
                    left: 0,
                    top: 0,
                    right: (320. * dpi as f32 / 96.) as i32,
                    bottom: (240. * dpi as f32 / 96.) as i32,
                };
                let _ = AdjustWindowRectExForDpi(
                    &mut r,
                    WS_OVERLAPPEDWINDOW,
                    false,
                    WINDOW_EX_STYLE(0),
                    dpi,
                );
                info.ptMinTrackSize = POINT {
                    x: r.right - r.left,
                    y: r.bottom - r.top,
                };
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if !TRACKING.replace(true) {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: h,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                }
                let (x, y) = dips(lp);
                with_state(|s| input::pointer_move(s, x, y));
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                TRACKING.set(false);
                with_state(input::pointer_leave);
                LRESULT(0)
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                CLICKS.set(if msg == WM_LBUTTONDBLCLK { 2 } else { 1 });
                let (x, y) = dips(lp);
                with_state(|s| input::press(s, h, x, y));
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = dips(lp);
                let count = CLICKS.get();
                with_state(|s| input::release(s, x, y, count));
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                let (x, y) = dips(lp);
                with_state(|s| input::context_menu(s, x, y));
                LRESULT(0)
            }
            WM_CONTEXTMENU => {
                // The menu key / Shift+F10: the focused node's menu.
                with_state(|s| {
                    let w = &s.renderer.widgets;
                    if let Some(f) = w.focus {
                        let r = w.scene.window_rect(f);
                        input::context_menu(s, r.x + r.w / 2., r.y + r.h / 2.);
                    }
                });
                LRESULT(0)
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let delta = hiword(wp.0) as i16 as f32;
                let (sx, sy) = point_of(lp);
                let mut p = POINT { x: sx, y: sy };
                let _ = ScreenToClient(h, &mut p);
                let k = with_state(|s| s.renderer.widgets.scale()).unwrap_or(1.);
                // A notch scrolls 48 DIPs (WinUI's ScrollViewer); smooth
                // wheels and precision touchpads send fractions of a notch.
                let precise = delta % 120. != 0.;
                let d = delta / 120. * 48.;
                let (dx, dy) = if msg == WM_MOUSEWHEEL {
                    (0., -d)
                } else {
                    (d, 0.)
                };
                with_state(|s| input::wheel(s, p.x as f32 / k, p.y as f32 / k, dx, dy, precise));
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let name = crate::keys::key_name(wp.0 as u16, None);
                let used = name
                    .and_then(|n| with_state(|s| input::key(s, &n)))
                    .unwrap_or(false);
                if used {
                    LRESULT(0)
                } else {
                    DefWindowProcW(h, msg, wp, lp)
                }
            }
            WM_COMMAND => {
                let code = hiword(wp.0) as u32;
                if code == EN_CHANGE && lp.0 != 0 {
                    field_changed(HWND(lp.0 as *mut c_void));
                }
                LRESULT(0)
            }
            WM_NOTIFY => {
                let hdr = &*(lp.0 as *const NMHDR);
                if hdr.code == EN_SELCHANGE && !UPDATING.get() {
                    let field = hdr.hwndFrom;
                    if let Some(id) = crate::fields::node_of(field) {
                        let wants = with_state(|s| s.renderer.widgets.select_events.contains(&id));
                        if wants == Some(true) {
                            let text = crate::fields::text(field);
                            let (a, b) = crate::fields::selection(field);
                            let a = prism_native::fields::utf16_to_chars(&text, a);
                            let b = prism_native::fields::utf16_to_chars(&text, b);
                            dispatch(UiEvent::Select(id, a, b));
                        }
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => {
                match wp.0 {
                    TIMER_RUNTIME => {
                        let _ = KillTimer(Some(h), TIMER_RUNTIME);
                        poll();
                    }
                    TIMER_SECOND => tick(),
                    TIMER_AUTOMATION => crate::smoke::automation(h),
                    TIMER_SNAPSHOT => crate::smoke::snapshot(h),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_PRISM_WAKE => {
                poll();
                LRESULT(0)
            }
            WM_PRISM_DISPATCH => {
                drain();
                LRESULT(0)
            }
            WM_PRISM_INVOKE => {
                let id = lp.0 as NodeId;
                with_state(|s| {
                    let scene = &s.renderer.widgets.scene;
                    let target = match scene.get(id).map(|n| (n.kind, n.parent)) {
                        Some((NodeKind::Tab, Some(tabs))) => scene
                            .pages(tabs)
                            .iter()
                            .position(|&t| t == id)
                            .map(|i| Target::Tab(tabs, i)),
                        Some(_) => Some(Target::Node(id)),
                        None => None,
                    };
                    if let Some(t) = target {
                        input::activate(s, t, None);
                    }
                });
                LRESULT(0)
            }
            WM_PRISM_IMAGE => {
                with_state(|s| {
                    let w = &mut s.renderer.widgets;
                    let wic = w.gfx.wic.clone();
                    let loaded = w.images.take_loaded(&wic);
                    let ids: Vec<NodeId> = w
                        .scene
                        .nodes
                        .iter()
                        .filter(|(_, n)| n.kind == NodeKind::Picture && loaded.contains(&n.text))
                        .map(|(id, _)| *id)
                        .collect();
                    for id in ids {
                        w.cache.forget(id);
                        w.mark(id);
                    }
                    // Natural sizes changed.
                    s.renderer.mark_all_dirty();
                    relayout(s);
                });
                LRESULT(0)
            }
            WM_PRISM_MENU => {
                let k = with_state(|s| s.renderer.widgets.scale()).unwrap_or(1.);
                input::show_pending_menu(h, k);
                LRESULT(0)
            }
            WM_PRISM_FOCUS => {
                with_state(|s| match wp.0 {
                    0 | 1 => input::focus_step(s, wp.0 == 1),
                    _ => input::set_focus(s, Some(lp.0 as NodeId), true),
                });
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                let what = if lp.0 != 0 {
                    PCWSTR(lp.0 as *const u16).to_string().unwrap_or_default()
                } else {
                    String::new()
                };
                if what == "ImmersiveColorSet" || what.is_empty() {
                    with_state(|s| {
                        let w = &mut s.renderer.widgets;
                        w.system_dark = system::dark_mode();
                        w.accent = system::accent();
                        w.retheme();
                        s.runtime.app.set_environment(system::environment());
                        let patches = s.runtime.poll();
                        commit(s, patches);
                    });
                }
                LRESULT(0)
            }
            WM_GETOBJECT => match crate::uia::get_object(h, wp, lp) {
                Some(r) => r,
                None => DefWindowProcW(h, msg, wp, lp),
            },
            WM_CLOSE => {
                if UNSAVED.get() {
                    let r = MessageBoxW(
                        Some(h),
                        w!("Quit without saving your changes?"),
                        w!("Unsaved changes"),
                        MB_OKCANCEL | MB_ICONWARNING,
                    );
                    if r != IDOK {
                        return LRESULT(0);
                    }
                }
                let _ = DestroyWindow(h);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

/// Opens the app's window and runs its message loop until it closes.
pub fn run(config: AppConfig, init: AppInit) {
    unsafe {
        // Per-monitor DPI v2 (a manifest says it too once apps are packaged).
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if let Err(e) = OleInitialize(None) {
            log!("OleInitialize: {e}");
        }
        // RICHEDIT50W.
        if let Err(e) = LoadLibraryW(w!("Msftedit.dll")) {
            log!("Msftedit.dll: {e} (text fields will be missing)");
        }
        let instance = GetModuleHandleW(None).expect("module handle");
        let class = w!("PrismWindow");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let (width, height) = env("PRISM_WINDOW")
            .and_then(|s| {
                let (w, h) = s.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?))
            })
            .unwrap_or((config.width, config.height));
        let title = wide(&config.title);
        let style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
        let main = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR(title.as_ptr()),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            width as i32,
            height as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .expect("prism-windows: CreateWindowExW");
        MAIN.set(hwnd_value(main));
        // The client area at the window's DPI.
        let dpi = GetDpiForWindow(main).max(96);
        let k = dpi as f32 / 96.;
        let mut r = RECT {
            left: 0,
            top: 0,
            right: (width * k).round() as i32,
            bottom: (height * k).round() as i32,
        };
        let _ = AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE(0), dpi);
        let _ = SetWindowPos(
            main,
            None,
            0,
            0,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );

        let gfx = gfx::shared();
        let assets: Option<std::path::PathBuf> = env("PRISM_ASSETS_DIR")
            .or_else(|| config.assets.clone())
            .map(Into::into);
        let data = system::data_dir(&config.title);
        let _ = std::fs::create_dir_all(&data);
        let images = Images::new(prism_native::images::ImageSources {
            embedded: config.embedded_assets.clone(),
            asset_dirs: assets.into_iter().collect(),
            cache_dir: Some(data.join("images")),
        });
        let services = WinServices::new(main);
        let target = hwnd_value(main);
        let runtime = Runtime::with_options(
            &init,
            prism_core::RuntimeOptions {
                path: std::env::var("PRISM_PATH").unwrap_or_else(|_| "/".into()),
                storage: Some(Box::new(prism_core::FileStorage::new(data.clone()))),
                // Posted, not run: `invoke` on the UI thread would poll in
                // the middle of the update that woke it (#57).
                wake_ui: Some(std::sync::Arc::new(move || {
                    let _ = PostMessageW(Some(hwnd(target)), WM_PRISM_WAKE, WPARAM(0), LPARAM(0));
                })),
                capabilities: prism_core::PlatformServices::capabilities(&services),
                environment: system::environment(),
                data_dir: Some(data),
                viewport: Some((width, height)),
                device_info: Some(system::device_info(width, height, k)),
                ..Default::default()
            },
        );
        let win = Win::new(main, gfx, images, config.title.clone(), dpi);
        set_dark_title_bar(main, win.dark());
        let renderer = NativeRenderer::new(win);
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                runtime,
                renderer,
                services,
            })
        });
        let mut client = RECT::default();
        let _ = GetClientRect(main, &mut client);
        with_state(|s| {
            s.renderer.widgets.size = (client.right as f32 / k, client.bottom as f32 / k);
            let first = s.runtime.render();
            commit(s, first);
        });
        SetTimer(Some(main), TIMER_SECOND, 1000, None);
        // The first ShowWindow takes the launcher's STARTUPINFO (hidden
        // under sshd): show it again.
        let _ = ShowWindow(main, SW_SHOWDEFAULT);
        if !IsWindowVisible(main).as_bool() {
            let _ = ShowWindow(main, SW_SHOW);
        }
        let _ = UpdateWindow(main);
        crate::smoke::start(main);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let key = matches!(
                msg.message,
                WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP
            );
            if key {
                let vk = msg.wParam.0 as u32;
                let up = matches!(msg.message, WM_KEYUP | WM_SYSKEYUP);
                let repeat = !up && (msg.lParam.0 >> 30) & 1 == 1;
                let typed = if up { None } else { typed_char(vk) };
                let consumed = with_state(|s| input::key_event(s, vk as u16, typed, repeat, up))
                    .unwrap_or(false);
                if consumed {
                    continue;
                }
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // COM objects go before COM does.
        let state = STATE.with(|s| s.borrow_mut().take());
        drop(state);
        OleUninitialize();
    }
}
