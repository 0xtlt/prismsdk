//! The app's window and UI thread: `run`, the window procedure, the
//! message loop, and the dispatch / commit cycle shared with the other
//! native backends (prism-linux's).
//!
//! One UI thread, an OLE single-threaded apartment (clipboard, drag and
//! drop) with WinUI's dispatcher queue. The window is a plain Win32 one;
//! its client area is a XAML island (`host.rs`). UI events are queued and
//! dispatched from the message loop (`WM_PRISM_DISPATCH`): XAML raises
//! events while Prism changes it (a control's value, a text box's text),
//! and nothing raised then is reported (`UPDATING`).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;

use prism_core::{AppConfig, AppInit, NodeId, NodeKind, Patch, Runtime, UiEvent};
use prism_native::{Insets, NativeRenderer};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, w};

use crate::host::{self, Island};
use crate::images::Images;
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
    /// Set while Prism changes the window: what XAML reports meanwhile
    /// (a control's new value, a field's text) is Prism's own doing.
    static UPDATING: Cell<bool> = const { Cell::new(false) };
    static QUEUE: RefCell<VecDeque<UiEvent>> = const { RefCell::new(VecDeque::new()) };
    static DEFERRED: RefCell<Vec<Deferred>> = const { RefCell::new(Vec::new()) };
    static MAIN: Cell<isize> = const { Cell::new(0) };
    /// Closing asks first (#158).
    static UNSAVED: Cell<bool> = const { Cell::new(false) };
    /// The text each field was last given or reported: `TextChanged`
    /// comes later than the change, and must not report the app's own.
    static FIELD_TEXT: RefCell<HashMap<NodeId, String>> = RefCell::new(HashMap::new());
    static START: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
    /// The first frame was rendered; the handler that saw it.
    static FRAMED: Cell<bool> = const { Cell::new(false) };
    static RENDERED: Cell<Option<i64>> = const { Cell::new(None) };
}

pub fn main_hwnd() -> HWND {
    hwnd(MAIN.get())
}

/// Runs `f` with the state, unless it is already in use (XAML called
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
    let control = match event {
        UiEvent::Change(id, _) => Some(id),
        _ => None,
    };
    let started = std::time::Instant::now();
    let (event, restyled) = s.renderer.locate_drag(event);
    if restyled {
        relayout(s);
    }
    let name = crate::perf::on().then(|| format!("{event:?}"));
    let patches = s.runtime.dispatch(event);
    let app = crate::perf::ms(started);
    commit(s, patches);
    if let Some(name) = name {
        // What took time: a click, a tab switch (not pointer moves).
        let total = crate::perf::ms(started);
        let costs = crate::perf::take();
        if total >= 2. {
            log!("{name}: {total:.1} ms (app {app:.1} ms; {costs})");
            crate::perf::committed(&name, started);
        }
    }
    // A dragged payload near a scroll view's edge scrolls it (#251).
    crate::pointer::autoscroll(s);
    // A control the app did not follow shows the app's value again.
    if let Some(id) = control {
        let was = UPDATING.replace(true);
        s.renderer.widgets.sync_value(id);
        s.renderer.widgets.flush();
        UPDATING.set(was);
    }
}

/// Applies patches, lays out, updates what changed, carries out the app's
/// requests, schedules the runtime's next timer.
pub fn commit(s: &mut State, patches: Vec<Patch>) {
    let was = UPDATING.replace(true);
    if !patches.is_empty() {
        crate::perf::patches(|| prism_core::Backend::apply(&mut s.renderer, &patches));
    }
    relayout(s);
    // `cx.focus(…)`, `dismiss_keyboard()`.
    let requests = std::mem::take(&mut s.renderer.widgets.focus_requests);
    for r in requests {
        s.renderer.widgets.focus_node(r);
    }
    UPDATING.set(was);
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
            prism_core::PlatformRequest::LoadFont(face) => crate::fonts::load(s, face),
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

/// Lays the tree out in the window again.
pub fn relayout(s: &mut State) {
    let was = UPDATING.replace(true);
    let (w, h) = s.renderer.widgets.size;
    if w > 0. {
        if (w, h) != s.runtime.app.viewport() {
            let patches = s.runtime.set_viewport(w, h);
            if !patches.is_empty() {
                prism_core::Backend::apply(&mut s.renderer, &patches);
            }
        }
        crate::perf::layout(|| s.renderer.layout(w, h, Insets::default()));
        let more = prism_native::report_layout(&mut s.runtime, &mut s.renderer);
        if !more.is_empty() {
            commit(s, more);
        }
    }
    crate::perf::layout(|| s.renderer.widgets.flush());
    UPDATING.set(was);
}

fn poll() {
    with_state(|s| {
        let patches = s.runtime.poll();
        commit(s, patches);
    });
}

/// A control's value changed by the user: reported unless it is Prism's
/// own (the node already has it).
pub fn changed(id: NodeId, value: f64) {
    if UPDATING.get() {
        return;
    }
    defer(Box::new(move |s| {
        let current = s.renderer.widgets.scene.get(id).map(|n| n.value);
        if current.is_some_and(|c| (c - value).abs() > 1e-9 || c.is_nan() != value.is_nan()) {
            dispatch(UiEvent::Change(id, value));
        }
    }));
}

/// The app set a field's text (its `TextChanged` comes later).
pub fn programmatic_text(id: NodeId, text: &str) {
    FIELD_TEXT.with(|f| f.borrow_mut().insert(id, text.to_string()));
}

/// A field's text changed: the user's typing is reported.
pub fn typed(id: NodeId, text: String) {
    if UPDATING.get() {
        return;
    }
    let same = FIELD_TEXT.with(|f| f.borrow().get(&id) == Some(&text));
    if same {
        return;
    }
    report_typed(id, text);
}

/// Sends what the user typed, remembering it (`Typed`, #73).
pub fn report_typed(id: NodeId, text: String) {
    FIELD_TEXT.with(|f| f.borrow_mut().insert(id, text.clone()));
    defer(Box::new({
        let text = text.clone();
        move |s| s.renderer.widgets.typed.entry(id).or_default().sent(&text)
    }));
    dispatch(UiEvent::Input(id, text));
}

/// A field's selection moved (UTF-16 offsets): reported to listeners.
pub fn selected(id: NodeId, text: &str, a: usize, b: usize) {
    if UPDATING.get() {
        return;
    }
    let a = prism_native::fields::utf16_to_chars(text, a);
    let b = prism_native::fields::utf16_to_chars(text, b);
    defer(Box::new(move |s| {
        if s.renderer.widgets.select_events.contains(&id) {
            dispatch(UiEvent::Select(id, a, b));
        }
    }));
}

/// Types `text` into a field like the user would (smoke hooks).
pub fn fill(id: NodeId, text: &str) {
    with_state(|s| {
        if let Some(v) = s.renderer.widgets.views.get(&id) {
            programmatic_text(id, text);
            let _ = crate::controls::set_field_text(&v.content, text);
        }
    });
    report_typed(id, text.to_string());
}

/// A control took or lost the keyboard focus.
pub fn focus_changed(id: NodeId, on: bool) {
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
        if let Some(n) = w.scene.get_mut(id) {
            n.focused = on;
        }
        let mut restyled = false;
        if let Some(p) = previous.filter(|p| *p != id && on) {
            if let Some(n) = s.renderer.widgets.scene.get_mut(p) {
                n.focused = false;
            }
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
    }));
}

/// A scroll view moved (by the user, or by Prism).
pub fn scrolled(id: NodeId, x: f32, y: f32) {
    defer(Box::new(move |s| {
        let w = &mut s.renderer.widgets;
        let Some(n) = w.scene.get_mut(id) else { return };
        if n.scroll == (x, y) {
            return;
        }
        n.scroll = (x, y);
        let report = n.scroll_events;
        let h = n.frame.h;
        w.flush();
        if report {
            dispatch(UiEvent::Scroll(id, y, h));
        }
    }));
}

/// Unix seconds for `secs` past midnight (local) on the day the date
/// picker shows (`TimePicker`).
pub fn time_of_day(id: NodeId, secs: f64) -> f64 {
    let day = with_state(|s| s.renderer.widgets.scene.get(id).map(|n| n.value))
        .flatten()
        .filter(|v| v.is_finite())
        .unwrap_or_else(now);
    let offset = local_offset();
    let midnight = ((day + offset) / 86_400.).floor() * 86_400. - offset;
    midnight + secs
}

/// Seconds past local midnight of a Unix time.
pub fn seconds_of_day(unix: f64) -> f64 {
    (unix + local_offset()).rem_euclid(86_400.)
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0., |d| d.as_secs_f64())
}

/// The local time zone's offset from UTC, seconds.
fn local_offset() -> f64 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut tz = TIME_ZONE_INFORMATION::default();
    let r = unsafe { GetTimeZoneInformation(&mut tz) };
    let bias = tz.Bias
        + if r == 2 {
            tz.DaylightBias
        } else {
            tz.StandardBias
        };
    -(bias as f64) * 60.
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
                w.measurer.forget(id);
                w.restyle(id);
                changed = true;
            }
        }
        if changed {
            s.renderer.mark_all_dirty();
            relayout(s);
        }
    });
}

/// Startup phases, with `PRISM_TRACE=1`.
pub fn trace(what: &str) {
    crate::profile::phase(what);
    if crate::perf::on() {
        log!(
            "{what} after {:.0} ms (UI thread CPU {:.0} ms)",
            since_start(),
            crate::perf::cpu_ms()
        );
    }
}

/// Whether the window showed its first frame.
pub fn framed() -> bool {
    FRAMED.get()
}

/// Logs the first frame (`CompositionTarget::Rendered`) with the memory
/// then; with `PRISM_TRACE`, the XAML elements in the tree, and when each
/// update reaches the screen.
fn first_frame() {
    use crate::winui::xaml::Media::{CompositionTarget, RenderedEventArgs};
    use windows::Foundation::EventHandler;
    let handler = EventHandler::<RenderedEventArgs>::new(|_, _| {
        if !FRAMED.replace(true) {
            crate::profile::first_frame(since_start());
            log!(
                "first frame after {:.0} ms (UI thread CPU {:.0} ms); {}",
                since_start(),
                crate::perf::cpu_ms(),
                memory()
            );
            if crate::perf::on() {
                let root = with_state(|s| s.renderer.widgets.root.clone());
                if let Some(Ok(root)) = root.map(|r| windows_core::Interface::cast(&r)) {
                    log!("{} XAML elements", crate::perf::count_elements(&root));
                }
            } else if let Some(t) = RENDERED.take() {
                let _ = CompositionTarget::RemoveRendered(t);
            }
            crate::scene3d::first_frame();
        }
        crate::perf::rendered();
        Ok(())
    });
    match CompositionTarget::Rendered(&handler) {
        Ok(t) => RENDERED.set(Some(t)),
        // No frame will say so: what waits for it goes now.
        Err(_) => FRAMED.set(true),
    }
}

/// Milliseconds since the process was created (startup logs).
pub fn since_start() -> f64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    unsafe {
        let (mut created, mut exited, mut kernel, mut user) = Default::default();
        if GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
        .is_err()
        {
            return START
                .get()
                .map_or(0., |s| s.elapsed().as_secs_f64() * 1000.);
        }
        let now = GetSystemTimePreciseAsFileTime();
        t(now).saturating_sub(t(created)) as f64 / 10_000.
    }
}

/// The process's memory: working set and private bytes (MB).
pub fn memory() -> String {
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut m = PROCESS_MEMORY_COUNTERS_EX::default();
    unsafe {
        let _ = GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut m as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        );
    }
    format!(
        "working set {:.1} MB, private {:.1} MB, peak working set {:.1} MB",
        m.WorkingSetSize as f64 / 1048576.,
        m.PrivateUsage as f64 / 1048576.,
        m.PeakWorkingSetSize as f64 / 1048576.
    )
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE => {
                let (pw, ph) = (loword(lp.0 as usize) as i32, hiword(lp.0 as usize) as i32);
                with_state(|s| {
                    let w = &mut s.renderer.widgets;
                    let k = w.scale();
                    if let Some(i) = &w.island {
                        i.resize(pw, ph);
                    }
                    w.resize(pw as f32 / k, ph as f32 / k);
                    relayout(s);
                });
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let dpi = loword(wp.0) as u32;
                let r = &*(lp.0 as *const RECT);
                system::dpi_changed(dpi);
                with_state(|s| {
                    let w = &mut s.renderer.widgets;
                    w.dpi = dpi;
                    if let (Some(k), Some(i)) = (system::scale_override(), &w.island) {
                        i.set_scale(k);
                    }
                    // Its pixels change: 3D scenes draw again.
                    crate::scene3d::redraw_all();
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
                let dpi = system::window_dpi(h);
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
            WM_SETFOCUS => {
                // The island takes the keyboard focus.
                with_state(|s| {
                    if let Some(i) = &s.renderer.widgets.island {
                        i.focus();
                    }
                });
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
                    TIMER_WATCHDOG => crate::smoke::watchdog(h),
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
            WM_PRISM_IMAGE => {
                with_state(|s| {
                    let w = &mut s.renderer.widgets;
                    let wic = w.wic.clone();
                    let loaded = w.images.take_loaded(&wic);
                    let ids: Vec<NodeId> = w
                        .scene
                        .nodes
                        .iter()
                        .filter(|(_, n)| n.kind == NodeKind::Picture && loaded.contains(&n.text))
                        .map(|(id, _)| *id)
                        .collect();
                    for id in ids {
                        w.measurer.forget(id);
                        w.show_picture(id);
                    }
                    // Natural sizes changed.
                    s.renderer.mark_all_dirty();
                    relayout(s);
                });
                LRESULT(0)
            }
            WM_PRISM_SNAPSHOT => {
                crate::smoke::snapshot_done(h, wp.0 as i32);
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
    START.set(Some(std::time::Instant::now()));
    crate::profile::start();
    // Windows Hello answers slowly: asked while WinUI starts.
    crate::security::prefetch_hello();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if let Err(e) = OleInitialize(None) {
            log!("OleInitialize: {e}");
        }
        host::log_fail_fasts();
        let xaml = match host::start() {
            Ok(x) => x,
            Err(e) => {
                log!("cannot start WinUI: {e}");
                std::process::exit(3);
            }
        };
        log!("WinUI ready after {:.0} ms", since_start());
        let instance = GetModuleHandleW(None).expect("module handle");
        let class = w!("PrismWindow");
        let (icon, icon_small) = crate::package::class_icons(instance.into());
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hIcon: icon,
            hIconSm: icon_small,
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
        trace("window");
        // The client area at the window's DPI (or the app's own scale).
        let dpi = system::window_dpi(main);
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

        let assets = crate::package::assets_dir(config.assets.as_deref());
        let data = system::data_dir(&config.title);
        let _ = std::fs::create_dir_all(&data);
        let images = Images::new(prism_native::images::ImageSources {
            embedded: config.embedded_assets.clone(),
            asset_dirs: assets.into_iter().collect(),
            cache_dir: Some(data.join("images")),
        });
        let services = WinServices::new(main, &data);
        trace("services");
        let target = hwnd_value(main);
        let environment = system::environment();
        let device_info = system::device_info(width, height, k);
        trace("environment");
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
                environment,
                data_dir: Some(data),
                viewport: Some((width, height)),
                device_info: Some(device_info),
                ..Default::default()
            },
        );
        trace("app runtime");
        let mut win = match Win::new(main, images, config.title.clone(), dpi) {
            Ok(w) => w,
            Err(e) => {
                log!("cannot create the window's XAML: {e}");
                std::process::exit(3);
            }
        };
        set_dark_title_bar(main, win.dark());
        // WinUI renders through DWM: no island in session 0 (SSH).
        if host::has_desktop() {
            match Island::new(main, &win.root, &xaml.app) {
                Ok(i) => win.island = Some(i),
                Err(e) => log!("XAML island: {e}"),
            }
            // XAML laid the tree out, then the first frame.
            let _ = win
                .root
                .Loaded(&crate::winui::xaml::RoutedEventHandler::new(|_, _| {
                    trace("XAML layout (Loaded)");
                    crate::profile::phase("render");
                    Ok(())
                }));
            first_frame();
        } else {
            FRAMED.set(true);
            log!(
                "no desktop session (session 0, e.g. SSH): WinUI cannot render here. The \
                 tree is built and laid out, nothing is shown; run the app in a signed-in \
                 session (scripts/windows/run-in-session.ps1)"
            );
        }
        trace("XAML island");
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
            let w = &mut s.renderer.widgets;
            if let Some(i) = &w.island {
                i.resize(client.right, client.bottom);
            }
            w.resize(client.right as f32 / k, client.bottom as f32 / k);
            let t = std::time::Instant::now();
            crate::profile::phase("tree");
            let first = s.runtime.render();
            trace(&format!(
                "the app's tree ({:.0} ms, {} patches)",
                crate::perf::ms(t),
                first.len()
            ));
            commit(s, first);
            crate::profile::phase("XAML layout");
            if crate::perf::on() {
                log!("XAML elements: {}", crate::perf::take());
            }
        });
        log!(
            "first layout after {:.0} ms ({} nodes)",
            since_start(),
            with_state(|s| s.renderer.widgets.scene.nodes.len()).unwrap_or(0)
        );
        SetTimer(Some(main), TIMER_SECOND, 1000, None);
        // The first ShowWindow takes the launcher's STARTUPINFO (hidden
        // under sshd): show it again.
        let _ = ShowWindow(main, SW_SHOWDEFAULT);
        if !IsWindowVisible(main).as_bool() {
            let _ = ShowWindow(main, SW_SHOW);
        }
        with_state(|s| {
            if let Some(i) = &s.renderer.widgets.island {
                i.show();
            }
        });
        crate::smoke::start(main);

        let pre = host::pre_translate();
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
                let typed = if up {
                    None
                } else {
                    crate::keyboard::typed_char(vk)
                };
                let consumed =
                    with_state(|s| crate::keyboard::key_event(s, vk as u16, typed, repeat, up))
                        .unwrap_or(false);
                if consumed {
                    continue;
                }
            }
            if let Some(pre) = pre
                && pre(&msg).as_bool()
            {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // XAML objects go before XAML and COM do.
        let state = STATE.with(|s| s.borrow_mut().take());
        drop(state);
        drop(xaml);
        OleUninitialize();
    }
}
