//! Development hooks, the same contract as the other backends'
//! (`scripts/linux-smoke.sh`, `scripts/windows-smoke.sh`):
//!
//! - `PRISM_CLICK="+1|Save"` presses buttons by label (a control's title,
//!   an accessibility label, a tab, `Back`, a bar button, or a clickable
//!   view showing that text); buttons are pressed through their UI
//!   Automation peer, like a screen reader would. `fill:Placeholder=text`
//!   types into the text field with that placeholder; `submit:Placeholder`
//!   presses Enter in it; `menu:Title` opens the context menu of the view
//!   showing Title; `scroll:600` scrolls the visible scroll views before
//!   the next steps; `key:Escape` (`Tab`, `Enter`) presses a key;
//!   `contrast:night-sky` switches the contrast theme (`PRISM_HIGH_CONTRAST`'s
//!   values) through the system's settings message; `hover:`,
//!   `drag:`, `swipe:` and `wheel:` move the real mouse over a view
//!   (`inject.rs`).
//! - `PRISM_SCROLL=600` scrolls the visible scroll views.
//! - `PRISM_SNAPSHOT=out.png` then saves the window's XAML, rendered by
//!   `RenderTargetBitmap`, and what DWM shows of the window
//!   (`PrintWindow`: Mica, shadows, open flyouts) as `out-window.png`, and
//!   quits. `PRISM_DELAY` (seconds, default 1) before the clicks,
//!   `PRISM_AFTER` (default 0.7) between them and the snapshot.
//!
//! WinUI renders only in a desktop session: in session 0 (SSH) the clicks
//! run, but there is nothing to snapshot.

use prism_core::{NodeKind, UiEvent};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, KillTimer, PostMessageW, SetTimer};
use windows_core::Interface;

use crate::imp::{State, dispatch, drain, with_state};
use crate::scene::Target;
use crate::win::{
    TIMER_AUTOMATION, TIMER_SNAPSHOT, TIMER_WATCHDOG, WM_PRISM_SNAPSHOT, env, env_secs, log,
};
use crate::winui::xaml::Automation::Peers::ButtonAutomationPeer;
use crate::winui::xaml::Automation::Provider::IInvokeProvider;
use crate::winui::xaml::Media::Imaging::RenderTargetBitmap;

pub fn start(main: HWND) {
    if env("PRISM_SNAPSHOT").is_none() && env("PRISM_CLICK").is_none() {
        return;
    }
    let delay = env_secs("PRISM_DELAY", 1.);
    unsafe {
        SetTimer(Some(main), TIMER_AUTOMATION, (delay * 1000.) as u32, None);
        if env("PRISM_SNAPSHOT").is_some() {
            // Whatever happens, the run ends.
            let limit = delay + env_secs("PRISM_AFTER", 0.7) + 20.;
            SetTimer(Some(main), TIMER_WATCHDOG, (limit * 1000.) as u32, None);
        }
    }
}

/// `TIMER_WATCHDOG`: the snapshot never came.
pub fn watchdog(main: HWND) {
    log!("snapshot failed: timed out");
    unsafe {
        let _ = DestroyWindow(main);
    }
}

/// `TIMER_AUTOMATION`: the clicks, the scroll, then the snapshot's timer.
pub fn automation(main: HWND) {
    unsafe {
        let _ = KillTimer(Some(main), TIMER_AUTOMATION);
    }
    let clicks = env("PRISM_CLICK").unwrap_or_default();
    let steps: Vec<String> = clicks
        .split('|')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    steps_then_snapshot(main, steps);
}

/// Runs `steps`, then `PRISM_SCROLL` and the snapshot's timer. After a
/// `scroll:` or a `key:` step the next ones wait for XAML to scroll, or
/// for the key to arrive.
fn steps_then_snapshot(main: HWND, mut steps: Vec<String>) {
    while !steps.is_empty() {
        let step = steps.remove(0);
        let found = run_step(&step);
        if !found {
            eprintln!("prism-windows: PRISM_CLICK: nothing labelled {step:?}");
        }
        // Each step sees what the previous one did.
        drain();
        // Real input plays over a few hundred milliseconds: the next steps
        // (a key after a click) and the snapshot's delay come after it.
        if crate::inject::playing() {
            after_input(main, steps);
            return;
        }
        if (step.starts_with("scroll:") || step.starts_with("key:")) && !steps.is_empty() {
            crate::motion::after(
                std::time::Duration::from_millis(400),
                Box::new(move || steps_then_snapshot(main, steps)),
            );
            return;
        }
    }
    if let Some(y) = env("PRISM_SCROLL").and_then(|y| y.parse::<f32>().ok()) {
        scroll(y);
    }
    if env("PRISM_SNAPSHOT").is_some() {
        let after = env_secs("PRISM_AFTER", 0.7);
        unsafe {
            SetTimer(Some(main), TIMER_SNAPSHOT, (after * 1000.) as u32, None);
        }
    }
}

/// Goes on with `steps` once the real input queued so far has played.
fn after_input(main: HWND, steps: Vec<String>) {
    if crate::inject::playing() {
        crate::motion::after(
            std::time::Duration::from_millis(100),
            Box::new(move || after_input(main, steps)),
        );
        return;
    }
    steps_then_snapshot(main, steps);
}

fn run_step(step: &str) -> bool {
    // `contrast:night-sky` (`0`, `1`, a theme): the contrast theme changes
    // as the app runs, through the system's own message.
    if let Some(value) = step.strip_prefix("contrast:") {
        crate::system::override_contrast(value);
        log!("PRISM_CLICK: contrast theme {value}");
        return with_state(|s| s.renderer.widgets.hwnd)
            .map(crate::imp::send_contrast_changed)
            .is_some();
    }
    // `key:Escape`, `key:Tab`, `key:Enter`: the key, as the keyboard sends
    // it (to the window with the keyboard focus, through the message loop).
    if let Some(name) = step.strip_prefix("key:") {
        return press(name);
    }
    // `scroll:600`: scrolls first (a popover then opens on its anchor in view).
    if let Some(y) = step
        .strip_prefix("scroll:")
        .and_then(|y| y.parse::<f32>().ok())
    {
        scroll(y);
        return true;
    }
    // `hover:`, `drag:`, `swipe:`, `wheel:`: real mouse input.
    if let Some(found) = crate::inject::step(step) {
        return found;
    }
    if let Some(title) = step.strip_prefix("menu:") {
        let found = with_state(|s| {
            let w = &s.renderer.widgets;
            let scene = &w.scene;
            let Some(Target::Node(id)) = scene.find_by_label(title).or_else(|| {
                scene
                    .shown_in_order()
                    .into_iter()
                    .find(|&id| scene.own_text(id).as_deref() == Some(title))
                    .map(Target::Node)
            }) else {
                return false;
            };
            let Some(m) = scene.find_up(id, |n| n.menu.is_some()) else {
                return false;
            };
            let spec = scene.get(m).and_then(|n| n.menu.clone());
            let r = scene.get(m).map(|n| n.frame).unwrap_or_default();
            match (spec, w.outer(m)) {
                (Some(spec), Some(target)) => {
                    crate::events::show_menu(&target, &spec, r.w / 2., r.h / 2.).is_ok()
                }
                _ => false,
            }
        });
        return found == Some(true);
    }
    if let Some((placeholder, text)) = step.strip_prefix("fill:").and_then(|t| t.split_once('=')) {
        let id = with_state(|s| s.renderer.widgets.scene.find_field(placeholder)).flatten();
        let Some(id) = id else { return false };
        crate::imp::fill(id, &text.replace("\\n", "\n"));
        return true;
    }
    if let Some(placeholder) = step.strip_prefix("submit:") {
        let id = with_state(|s| s.renderer.widgets.scene.find_field(placeholder)).flatten();
        let Some(id) = id else { return false };
        dispatch(UiEvent::Submit(id));
        return true;
    }
    with_state(|s| {
        let target = s.renderer.widgets.scene.find_by_label(step);
        match target {
            Some(t) => {
                activate(s, t);
                true
            }
            None => false,
        }
    }) == Some(true)
}

/// Posts a key press and release to the window with the keyboard focus.
fn press(name: &str) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, VK_ESCAPE, VK_RETURN, VK_TAB};
    use windows::Win32::UI::WindowsAndMessaging::{WM_KEYDOWN, WM_KEYUP};
    let vk = match name {
        "Escape" => VK_ESCAPE,
        "Tab" => VK_TAB,
        "Enter" => VK_RETURN,
        _ => return false,
    };
    // The window with the keyboard focus, else the island's (the app is
    // not in the foreground).
    let mut target = unsafe { GetFocus() };
    if target.is_invalid() {
        use windows::Win32::UI::WindowsAndMessaging::{GW_CHILD, GetWindow};
        target = unsafe { GetWindow(crate::imp::main_hwnd(), GW_CHILD) }.unwrap_or_default();
    }
    if target.is_invalid() {
        return false;
    }
    let scan = unsafe {
        windows::Win32::UI::Input::KeyboardAndMouse::MapVirtualKeyW(
            vk.0 as u32,
            windows::Win32::UI::Input::KeyboardAndMouse::MAPVK_VK_TO_VSC,
        )
    } as isize;
    unsafe {
        let _ = PostMessageW(
            Some(target),
            WM_KEYDOWN,
            WPARAM(vk.0 as usize),
            LPARAM(1 | (scan << 16)),
        );
        let up = 1 | (scan << 16) | (1 << 30) | (1 << 31);
        let _ = PostMessageW(Some(target), WM_KEYUP, WPARAM(vk.0 as usize), LPARAM(up));
    }
    true
}

/// Scrolls the visible scroll views to `y`.
fn scroll(y: f32) {
    with_state(|s| {
        let w = &mut s.renderer.widgets;
        let ids: Vec<_> = w
            .scene
            .shown_in_order()
            .into_iter()
            .filter(|&id| w.scene.get(id).is_some_and(|n| n.scrolls(true)))
            .collect();
        for id in ids {
            s.renderer.widgets.scroll_view_to(id, 0., y, false);
        }
        s.renderer.widgets.flush();
    });
}

/// Presses what `PRISM_CLICK` found.
fn activate(s: &mut State, target: Target) {
    let w = &s.renderer.widgets;
    match target {
        Target::Node(id) => {
            let Some(n) = w.scene.get(id) else { return };
            match n.kind {
                NodeKind::Button => {
                    // Through UI Automation, like a screen reader.
                    let invoked = match w.views.get(&id).map(|v| &v.content) {
                        Some(crate::controls::Content::Button(b, _)) => {
                            ButtonAutomationPeer::CreateInstanceWithOwner(b)
                                .and_then(|p| p.cast::<IInvokeProvider>())
                                .and_then(|p| p.Invoke())
                                .is_ok()
                        }
                        _ => false,
                    };
                    if !invoked {
                        dispatch(UiEvent::Click(id));
                    }
                }
                NodeKind::Toggle | NodeKind::Checkbox => {
                    dispatch(UiEvent::Change(id, if n.value >= 0.5 { 0. } else { 1. }))
                }
                _ => dispatch(UiEvent::Click(id)),
            }
        }
        Target::Tab(tabs, i) => dispatch(UiEvent::Change(tabs, i as f64)),
        Target::Back(stack) => {
            let depth = w.scene.pages(stack).len();
            dispatch(UiEvent::Back(stack, depth.saturating_sub(1)));
        }
        Target::BarItem(screen, i) => dispatch(UiEvent::Change(screen, i as f64)),
    }
}

thread_local! {
    static RTB: std::cell::RefCell<Option<RenderTargetBitmap>> = const { std::cell::RefCell::new(None) };
}

/// `TIMER_SNAPSHOT`: renders the window's XAML, then `snapshot_done`.
pub fn snapshot(main: HWND) {
    unsafe {
        let _ = KillTimer(Some(main), TIMER_SNAPSHOT);
    }
    let Some(path) = env("PRISM_SNAPSHOT") else {
        return;
    };
    let started = with_state(|s| -> Result<(), String> {
        let w = &s.renderer.widgets;
        if w.island.is_none() {
            return Err("no desktop session: WinUI renders nothing in session 0".into());
        }
        let window = path.replace(".png", "-window.png");
        match crate::capture::print_window(main, &window, &w.wic) {
            Ok(()) => log!("window as composed written to {window}"),
            Err(e) => log!("PrintWindow: {e}"),
        }
        let rtb = RenderTargetBitmap::new().map_err(|e| e.to_string())?;
        let action = rtb.RenderAsync(&w.root).map_err(|e| e.to_string())?;
        RTB.with(|r| *r.borrow_mut() = Some(rtb));
        let target = main.0 as isize;
        action
            .SetCompleted(&windows_future::AsyncActionCompletedHandler::new(
                move |_, status| {
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(target as *mut _)),
                            WM_PRISM_SNAPSHOT,
                            WPARAM(status.0 as usize),
                            LPARAM(0),
                        );
                    }
                    Ok(())
                },
            ))
            .map_err(|e| e.to_string())
    });
    match started {
        Some(Ok(())) => {}
        Some(Err(e)) => {
            log!("snapshot failed: {e}");
            unsafe {
                let _ = DestroyWindow(main);
            }
        }
        None => log!("snapshot failed: the window is busy"),
    }
}

/// The XAML's pixels, read off the UI thread (`GetPixelsAsync`).
static PIXELS: std::sync::Mutex<Option<Vec<u8>>> = std::sync::Mutex::new(None);
/// `WM_PRISM_SNAPSHOT` statuses: rendered (`AsyncStatus::Completed`), and
/// pixels read.
const RENDERED: i32 = 1;
const READ: i32 = 100;

/// `WM_PRISM_SNAPSHOT`: the XAML is rendered, then its pixels are read,
/// then written as a PNG.
pub fn snapshot_done(main: HWND, status: i32) {
    let quit = || unsafe {
        let _ = DestroyWindow(main);
    };
    let rtb = RTB.with(|r| r.borrow().clone());
    let Some(rtb) = rtb.filter(|_| status == RENDERED || status == READ) else {
        log!("snapshot failed: RenderTargetBitmap did not complete ({status})");
        quit();
        return;
    };
    if status == RENDERED {
        let target = main.0 as isize;
        let started = rtb.GetPixelsAsync().and_then(|op| {
            op.SetCompleted(&windows_future::AsyncOperationCompletedHandler::new(
                move |op, _| {
                    let bytes = op.as_ref().and_then(|op| op.GetResults().ok()).and_then(
                        |b: windows::Storage::Streams::IBuffer| unsafe {
                            let access: windows::Win32::System::WinRT::IBufferByteAccess =
                                b.cast().ok()?;
                            let len = b.Length().ok()? as usize;
                            Some(std::slice::from_raw_parts(access.Buffer().ok()?, len).to_vec())
                        },
                    );
                    *PIXELS.lock().unwrap_or_else(|e| e.into_inner()) = bytes;
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(target as *mut _)),
                            WM_PRISM_SNAPSHOT,
                            WPARAM(READ as usize),
                            LPARAM(0),
                        );
                    }
                    Ok(())
                },
            ))
        });
        if let Err(e) = started {
            log!("snapshot failed: {e}");
            quit();
        }
        return;
    }
    RTB.with(|r| r.borrow_mut().take());
    let path = env("PRISM_SNAPSHOT").unwrap_or_default();
    let px = PIXELS.lock().unwrap_or_else(|e| e.into_inner()).take();
    let under =
        with_state(|s| s.renderer.widgets.palette.window).unwrap_or(prism_core::Color::WHITE);
    let wic = with_state(|s| s.renderer.widgets.wic.clone());
    let result = (|| -> windows_core::Result<(u32, u32)> {
        let (w, h) = (rtb.PixelWidth()? as u32, rtb.PixelHeight()? as u32);
        let mut px = px.ok_or_else(windows_core::Error::empty)?;
        let c = crate::winui::win_color(under);
        crate::wic::unpremultiply(&mut px, Some([c.R, c.G, c.B]));
        if let Some(wic) = &wic {
            crate::wic::save_png(wic, &path, w, h, &px)?;
        }
        Ok((w, h))
    })();
    match result {
        Ok((w, h)) => log!(
            "snapshot written to {path} ({w}x{h}); {}",
            crate::imp::memory()
        ),
        Err(e) => log!("snapshot failed: {e}"),
    }
    quit();
}
