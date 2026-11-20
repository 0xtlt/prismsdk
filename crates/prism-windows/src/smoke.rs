//! Development hooks, the same contract as the other backends'
//! (`scripts/linux-smoke.sh`, `scripts/windows-smoke.sh`):
//!
//! - `PRISM_CLICK="+1|Save"` presses buttons by label (a control's title,
//!   an accessibility label, a tab, `Back`, a bar button, or a clickable
//!   view showing that text); `fill:Placeholder=text` types into the text
//!   field with that placeholder; `submit:Placeholder` presses Enter in it;
//!   `menu:Title` opens the context menu of the view showing Title.
//! - `PRISM_SCROLL=600` scrolls the visible scroll views.
//! - `PRISM_SNAPSHOT=out.png` then saves the window, drawn offscreen with
//!   its text fields, and quits. `PRISM_DELAY` (seconds, default 1)
//!   before the clicks, `PRISM_AFTER` (default 0.7) between them and the
//!   snapshot.
//!
//! Everything works over SSH (session 0, no desktop compositor).

use prism_core::UiEvent;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, KillTimer, SetTimer};

use crate::imp::{dispatch, drain, with_state};
use crate::scene::Target;
use crate::win::{TIMER_AUTOMATION, TIMER_SNAPSHOT, env, env_secs, log};

pub fn start(main: HWND) {
    if env("PRISM_SNAPSHOT").is_none() && env("PRISM_CLICK").is_none() {
        return;
    }
    let delay = env_secs("PRISM_DELAY", 1.);
    unsafe {
        SetTimer(Some(main), TIMER_AUTOMATION, (delay * 1000.) as u32, None);
    }
}

/// `TIMER_AUTOMATION`: the clicks, the scroll, then the snapshot's timer.
pub fn automation(main: HWND) {
    unsafe {
        let _ = KillTimer(Some(main), TIMER_AUTOMATION);
    }
    let clicks = env("PRISM_CLICK").unwrap_or_default();
    for step in clicks.split('|').filter(|s| !s.is_empty()) {
        let found = run_step(step);
        if !found {
            eprintln!("prism-windows: PRISM_CLICK: nothing labelled {step:?}");
        }
        // Each step sees what the previous one did.
        drain();
    }
    if let Some(y) = env("PRISM_SCROLL").and_then(|y| y.parse::<f32>().ok()) {
        with_state(|s| {
            let w = &mut s.renderer.widgets;
            let ids: Vec<_> = w
                .scene
                .shown_in_order()
                .into_iter()
                .filter(|&id| w.scene.get(id).is_some_and(|n| n.scrolls(true)))
                .collect();
            for id in ids {
                let w = &mut s.renderer.widgets;
                let cur = w.scene.get(id).map_or(0., |n| n.scroll.1);
                if w.scroll_by(id, 0., y - cur) {
                    crate::input::scrolled(s, id);
                }
            }
            s.renderer.widgets.flush();
        });
    }
    if env("PRISM_SNAPSHOT").is_some() {
        let after = env_secs("PRISM_AFTER", 0.7);
        unsafe {
            SetTimer(Some(main), TIMER_SNAPSHOT, (after * 1000.) as u32, None);
        }
    }
}

fn run_step(step: &str) -> bool {
    if let Some(title) = step.strip_prefix("menu:") {
        let found = with_state(|s| {
            let scene = &s.renderer.widgets.scene;
            let Some(crate::scene::Target::Node(id)) = scene.find_by_label(title).or_else(|| {
                // Any view showing the title with a menu around it.
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
            let r = scene.window_rect(m);
            crate::input::context_menu(s, r.x + r.w / 2., r.y + r.h / 2.);
            true
        });
        return found == Some(true);
    }
    if let Some((placeholder, text)) = step.strip_prefix("fill:").and_then(|t| t.split_once('=')) {
        let field = with_state(|s| {
            let w = &s.renderer.widgets;
            w.scene
                .find_field(placeholder)
                .and_then(|id| w.field_of(id))
        })
        .flatten();
        let Some(f) = field else { return false };
        crate::imp::fill(f, &text.replace("\\n", "\n"));
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
                crate::input::activate(s, t, None);
                true
            }
            None => false,
        }
    }) == Some(true)
}

/// `TIMER_SNAPSHOT`: saves the window and quits.
pub fn snapshot(main: HWND) {
    unsafe {
        let _ = KillTimer(Some(main), TIMER_SNAPSHOT);
    }
    let Some(path) = env("PRISM_SNAPSHOT") else {
        return;
    };
    let result = with_state(|s| {
        let w = &mut s.renderer.widgets;
        if !w.surface.presented() {
            // Never painted (session 0 shows no window): exercise the
            // on-screen path once, so its fallbacks are tested too.
            w.paint_now();
        }
        log!("presenting through {}", w.surface.describe());
        w.snapshot(&path)
    });
    match result {
        Some(Ok(())) => log!("snapshot written to {path}"),
        Some(Err(e)) => log!("snapshot failed: {e}"),
        None => log!("snapshot failed: the window is busy"),
    }
    unsafe {
        let _ = DestroyWindow(main);
    }
}
