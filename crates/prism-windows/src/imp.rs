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
    AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
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
    let r = STATE.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    });
    // A drain that ran meanwhile (a modal loop inside `f`) kept its work:
    // it runs now that the state is free, not at the next event.
    if r.is_some() && queued() {
        post(WM_PRISM_DISPATCH);
    }
    r
}

/// Whether work or events wait for a drain.
fn queued() -> bool {
    DEFERRED.with(|d| !d.borrow().is_empty()) || QUEUE.with(|q| !q.borrow().is_empty())
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

/// [`dispatch`], logged with `PRISM_TRACE_INPUT=1` (what reached the app:
/// clicks with their position, keys and count; keys with the caret's
/// edges; key releases).
pub fn dispatch_traced(event: UiEvent) {
    if env("PRISM_TRACE_INPUT").is_some() {
        log!("input: {event:?}");
    }
    dispatch(event);
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
        let mut queued = Some((work, events));
        let ran = with_state(|s| {
            let (work, events) = queued.take().unwrap_or_default();
            for f in work {
                f(s);
            }
            for e in events {
                deliver(s, e);
            }
        });
        if ran.is_none() {
            // No state yet (startup), or it is in use: kept, in order, for
            // the next drain (posted by the state's creation, or by
            // `with_state` once the state is free).
            if let Some((work, events)) = queued {
                DEFERRED.with(|d| {
                    let mut d = d.borrow_mut();
                    let later = std::mem::replace(&mut *d, work);
                    d.extend(later);
                });
                QUEUE.with(|q| {
                    let mut q = q.borrow_mut();
                    let later: Vec<UiEvent> = q.drain(..).collect();
                    q.extend(events);
                    q.extend(later);
                });
            }
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
            other => {
                if matches!(other, prism_core::PlatformRequest::Device { .. }) {
                    s.services.filter_names = crate::pickers::FilterNames::of(&s.runtime.app);
                }
                prism_core::PlatformServices::handle(&mut s.services, other)
            }
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
        screen_changed(s, false);
        crate::perf::layout(|| s.renderer.layout(w, h, Insets::default()));
        let more = prism_native::report_layout(&mut s.runtime, &mut s.renderer);
        if !more.is_empty() {
            commit(s, more);
        }
    }
    crate::perf::layout(|| s.renderer.widgets.flush());
    // A custom title bar's drag regions, and whether the caption buttons
    // are over the app's views (`titlebar.rs`).
    crate::titlebar::update_regions(s);
    crate::titlebar::follow(s);
    UPDATING.set(was);
}

/// The window on another monitor, at another scale, or the monitor
/// changed (`force`): `cx.device_info().screen` follows.
fn screen_changed(s: &mut State, force: bool) {
    let k = s.renderer.widgets.scale();
    if let Some(info) = system::device_info_changed(main_hwnd(), k, force) {
        let patches = s.runtime.set_device_info(info);
        if !patches.is_empty() {
            prism_core::Backend::apply(&mut s.renderer, &patches);
        }
    }
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

thread_local! {
    /// The lifecycle phase last reported to the app.
    static LIFECYCLE: Cell<prism_core::Lifecycle> = const { Cell::new(prism_core::Lifecycle::Active) };
    /// A close was confirmed: background was the window's last phase.
    static CLOSING: Cell<bool> = const { Cell::new(false) };
}

/// The window's phase (`cx.on_lifecycle`): reported when it changes, now,
/// or once the state is free. After a confirmed close (`closing`) no
/// phase follows: the window's deactivation as it goes (`DestroyWindow`)
/// is not reported.
fn report_lifecycle(phase: prism_core::Lifecycle, closing: bool) {
    let apply = move |s: &mut State| {
        if CLOSING.get() {
            return;
        }
        CLOSING.set(closing);
        if LIFECYCLE.get() == phase {
            return;
        }
        LIFECYCLE.set(phase);
        let patches = s.runtime.set_lifecycle(phase);
        commit(s, patches);
    };
    if with_state(apply).is_none() {
        defer(Box::new(apply));
    }
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
        crate::titlebar::update_regions(s);
        if report {
            dispatch(UiEvent::Scroll(id, y, h));
        }
    }));
}

/// Now, Unix seconds.
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0., |d| d.as_secs_f64())
}

/// The local time zone's offset from UTC, seconds.
pub fn local_offset() -> f64 {
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

/// Tells window `h` the contrast theme changed, as the system does
/// (`WM_SETTINGCHANGE`, `SPI_SETHIGHCONTRAST`): its colors are read again.
pub fn post_contrast_changed(h: HWND) {
    unsafe {
        let _ = PostMessageW(
            Some(h),
            WM_SETTINGCHANGE,
            WPARAM(SPI_SETHIGHCONTRAST.0 as usize),
            LPARAM(0),
        );
    }
}

/// The same, at once (the `contrast:` test step, outside any work).
pub fn send_contrast_changed(h: HWND) {
    unsafe {
        SendMessageW(
            h,
            WM_SETTINGCHANGE,
            Some(WPARAM(SPI_SETHIGHCONTRAST.0 as usize)),
            Some(LPARAM(0)),
        );
    }
}

pub fn set_dark_title_bar(h: HWND, dark: bool) {
    crate::titlebar::set_dark(dark);
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
    let m = memory_counters();
    format!(
        "working set {:.1} MB, private {:.1} MB, peak working set {:.1} MB",
        m.WorkingSetSize as f64 / 1048576.,
        m.PrivateUsage as f64 / 1048576.,
        m.PeakWorkingSetSize as f64 / 1048576.
    )
}

/// The process's memory counters.
pub fn memory_counters() -> windows::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS_EX {
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
    m
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            // Moved to another monitor; a resolution or refresh rate change.
            WM_EXITSIZEMOVE | WM_DISPLAYCHANGE => {
                with_state(|s| screen_changed(s, msg == WM_DISPLAYCHANGE));
                DefWindowProcW(h, msg, wp, lp)
            }
            WM_SIZE => {
                // Minimized, or back from it: the lifecycle follows. (Sizes
                // of a shown window leave it: the first comes before the
                // window is shown, hence active.)
                let minimized = wp.0 == SIZE_MINIMIZED as usize;
                let back = matches!(wp.0 as u32, SIZE_RESTORED | SIZE_MAXIMIZED)
                    && LIFECYCLE.get() == prism_core::Lifecycle::Background;
                if minimized || back {
                    let active = GetActiveWindow() == h;
                    report_lifecycle(crate::lifecycle::phase(minimized, active, false), false);
                }
                if minimized {
                    // A minimized window is 0 × 0: the layout stays.
                    return LRESULT(0);
                }
                let (pw, ph) = (loword(lp.0 as usize) as i32, hiword(lp.0 as usize) as i32);
                crate::titlebar::invalidate();
                with_state(|s| {
                    let w = &mut s.renderer.widgets;
                    let k = w.scale();
                    if let Some(i) = &w.island {
                        i.resize(pw, ph);
                    }
                    w.resize(pw as f32 / k, ph as f32 / k);
                    relayout(s);
                    crate::titlebar::report_controls(s);
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
                if crate::perf::on() {
                    let mut c = RECT::default();
                    let _ = GetClientRect(h, &mut c);
                    let k = dpi as f32 / 96.;
                    log!(
                        "DPI {dpi} ({:.0} %): client area {}x{} pixels, {:.0}x{:.0} DIPs",
                        k * 100.,
                        c.right,
                        c.bottom,
                        c.right as f32 / k,
                        c.bottom as f32 / k
                    );
                }
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
                    GetDpiForWindow(h).max(96),
                );
                info.ptMinTrackSize = POINT {
                    x: r.right - r.left,
                    y: r.bottom - r.top,
                };
                LRESULT(0)
            }
            WM_ACTIVATE => {
                let active = u32::from(loword(wp.0)) != WA_INACTIVE;
                if !active {
                    // The keys still down get their release in the window
                    // active now: released here (#159).
                    crate::keyboard::release_all();
                }
                // The high word: minimized.
                let minimized = hiword(wp.0) != 0 || IsIconic(h).as_bool();
                report_lifecycle(crate::lifecycle::phase(minimized, active, false), false);
                DefWindowProcW(h, msg, wp, lp)
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
                // Light or dark, the accent; a contrast theme on or off.
                let contrast =
                    wp.0 == SPI_SETHIGHCONTRAST.0 as usize || what == "WindowsThemeElement";
                if contrast {
                    let on = system::high_contrast().is_some();
                    log!("contrast theme {}", if on { "on" } else { "off" });
                }
                if what == "ImmersiveColorSet" || what.is_empty() || contrast {
                    let done = with_state(|s| {
                        let w = &mut s.renderer.widgets;
                        w.system_dark = system::dark_mode();
                        w.accent = system::accent();
                        w.retheme();
                        s.runtime.app.set_environment(system::environment());
                        let patches = s.runtime.poll();
                        commit(s, patches);
                    });
                    if done.is_none() {
                        // Busy: once the current work is done.
                        post_contrast_changed(h);
                    }
                }
                LRESULT(0)
            }
            // The system colors changed (a contrast theme's).
            WM_SYSCOLORCHANGE => {
                post_contrast_changed(h);
                LRESULT(0)
            }
            WM_CLOSE => {
                // With unsaved changes, asks first (#158): discarding
                // closes again.
                if UNSAVED.get() {
                    crate::alert::ask_leave(hwnd_value(h));
                    return LRESULT(0);
                }
                // The app's last chance to save (`on_lifecycle`).
                report_lifecycle(crate::lifecycle::phase(false, false, true), true);
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

/// The user discarded the unsaved changes (`alert::ask_leave`): window
/// `main` closes.
pub fn discard_and_close(main: isize) {
    UNSAVED.set(false);
    unsafe {
        let _ = PostMessageW(Some(hwnd(main)), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

/// Opens the app's window and runs its message loop until it closes.
pub fn run(config: AppConfig, init: AppInit) {
    START.set(Some(std::time::Instant::now()));
    crate::profile::start();
    // What the PC has (Windows Hello, cameras, location, passkeys…),
    // asked in the background: published as each check answers, never
    // waited for (`probe.rs`).
    crate::probe::start();
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
        // The frame is the monitor's, at its own DPI.
        let frame_dpi = GetDpiForWindow(main).max(96);
        let _ = AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE(0), frame_dpi);
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
        crate::fonts::set_data_dir(&data);
        crate::fonts::register_bundled(assets.as_slice(), &config.embedded_assets);
        crate::drops::set_folder(data.join("picked"));
        let images = Images::new(prism_native::images::ImageSources {
            embedded: config.embedded_assets.clone(),
            asset_dirs: assets.into_iter().collect(),
            cache_dir: Some(data.join("images")),
        });
        let services = WinServices::new(main, &data);
        trace("services");
        let target = hwnd_value(main);
        let environment = system::environment();
        let device_info = system::device_info(main, k);
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
        set_dark_title_bar(main, win.palette.dark);
        // WinUI renders through DWM: no island in session 0 (SSH).
        if host::has_desktop() {
            match Island::new(main, &win.root, &xaml.app) {
                Ok(i) => win.island = Some(i),
                Err(e) => log!("XAML island: {e}"),
            }
            // No title bar (`WindowChrome::Custom`): the client area
            // grows to the window's top edge before the first layout.
            if config.window_chrome.is_custom() {
                crate::titlebar::install(main, win.palette.dark);
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
        // Work deferred before the state existed (a check that answered
        // meanwhile) runs from the message loop.
        post(WM_PRISM_DISPATCH);
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
                log!("text layouts: {}", crate::text::take_counts());
            }
        });
        log!(
            "first layout after {:.0} ms ({} nodes, {:.0} %: {}x{} pixels)",
            since_start(),
            with_state(|s| s.renderer.widgets.scene.nodes.len()).unwrap_or(0),
            k * 100.,
            client.right,
            client.bottom
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
            // The caption buttons are laid out once shown.
            crate::titlebar::report_controls(s);
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
                // The key itself: its virtual key, scan code and extended
                // bit (a release matches its press, #159).
                let code = (vk & 0xFFFF) << 16 | ((msg.lParam.0 >> 16) & 0x1FF) as u32;
                let typed = if up {
                    None
                } else {
                    crate::keyboard::typed_char(vk)
                };
                let consumed = with_state(|s| {
                    crate::keyboard::key_event(s, code, vk as u16, typed, repeat, up)
                })
                .unwrap_or(false);
                if consumed {
                    continue;
                }
            } else if matches!(
                msg.message,
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_POINTERDOWN
            ) {
                // A press: focus moved now shows no focus visual.
                with_state(|s| s.renderer.widgets.focus_visible = false);
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
