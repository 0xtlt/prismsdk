//! Pull to refresh (`Scroll::on_refresh`) on macOS, which has no refresh
//! control: a refreshable scroll view refreshes when a trackpad (or Magic
//! Mouse) gesture pulls its content past the top by [`PULL`] points, which
//! AppKit's elastic scrolling allows, and on ⌘R (the Mac's Reload) while
//! it is shown in the key window, so a mouse wheel and the keyboard reach
//! it too. While the app says it refreshes (`Scroll::refreshing`), a small
//! spinner sits in a gap above the content, like iOS's refresh control.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use objc2::MainThreadMarker;
use objc2::rc::{Retained, Weak};
use objc2_app_kit::{
    NSAccessibility, NSAutoresizingMaskOptions, NSEvent, NSEventModifierFlags, NSEventPhase,
    NSEventType, NSProgressIndicator, NSProgressIndicatorStyle, NSScrollView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use prism_core::{NodeId, UiEvent};

/// How far past the top a pull goes before it refreshes, like iOS's.
pub(crate) const PULL: f64 = 60.;

/// The room the spinner takes above the content while refreshing, and the
/// spinner's side.
const GAP: f64 = 32.;
const SPINNER: f64 = 16.;

/// A refresh shown: its spinner, and how the scroll view's insets were.
struct Shown {
    spinner: Retained<NSProgressIndicator>,
    automatic_insets: bool,
}

struct Refreshable {
    scroll: Weak<NSScrollView>,
    /// While the app says it refreshes.
    shown: Option<Shown>,
    /// The current gesture already refreshed (once per pull).
    pulled: Cell<bool>,
}

thread_local! {
    /// Refreshable scroll views, by node.
    static REFRESHABLE: RefCell<HashMap<NodeId, Refreshable>> = RefCell::new(HashMap::new());
    static MONITOR: Cell<bool> = const { Cell::new(false) };
}

/// Whether scroll view `id` refreshes (`Widgets::set_clickable`).
pub(crate) fn set_refreshable(id: NodeId, scroll: &NSScrollView, on: bool) {
    if !on {
        forget(id);
        return;
    }
    REFRESHABLE.with(|r| {
        r.borrow_mut().entry(id).or_insert_with(|| Refreshable {
            scroll: Weak::from(scroll),
            shown: None,
            pulled: Cell::new(false),
        });
    });
    install_key_monitor();
}

/// Node `id` is gone, or not refreshable anymore.
pub(crate) fn forget(id: NodeId) {
    if let Some(r) = REFRESHABLE.with(|r| r.borrow_mut().remove(&id))
        && let (Some(shown), Some(scroll)) = (r.shown, r.scroll.load())
    {
        hide(&scroll, shown);
    }
}

/// Whether scroll view `id` shows that it refreshes (`Scroll::refreshing`):
/// a spinning indicator in a gap above its content, like iOS's refresh
/// control.
pub(crate) fn set_refreshing(id: NodeId, scroll: &NSScrollView, on: bool) {
    REFRESHABLE.with(|r| {
        let mut r = r.borrow_mut();
        let Some(entry) = r.get_mut(&id) else {
            return;
        };
        match (on, entry.shown.take()) {
            (true, None) => entry.shown = Some(show(scroll)),
            (false, Some(shown)) => hide(scroll, shown),
            (_, shown) => entry.shown = shown,
        }
    });
}

/// Makes room above `scroll`'s content (its top inset grows by [`GAP`];
/// content at the top moves down) and spins an indicator there, kept at the
/// top center as the scroll view resizes.
fn show(scroll: &NSScrollView) -> Shown {
    let mtm = MainThreadMarker::from(scroll);
    let clip = scroll.contentView();
    let mut insets = scroll.contentInsets();
    let top = insets.top;
    // Resting at the top: the content moves down with the gap (a pull
    // still under way settles there by itself).
    let at_top = (clip.bounds().origin.y + top).abs() < 0.5;
    let automatic_insets = scroll.automaticallyAdjustsContentInsets();
    scroll.setAutomaticallyAdjustsContentInsets(false);
    insets.top += GAP;
    scroll.setContentInsets(insets);
    if at_top {
        clip.scrollToPoint(NSPoint::new(clip.bounds().origin.x, -insets.top));
        scroll.reflectScrolledClipView(&clip);
    }
    let s = NSProgressIndicator::new(mtm);
    s.setStyle(NSProgressIndicatorStyle::Spinning);
    s.setControlSize(objc2_app_kit::NSControlSize::Small);
    s.setIndeterminate(true);
    s.setDisplayedWhenStopped(false);
    let b = scroll.bounds();
    let from_top = top + ((GAP - SPINNER) / 2.).round();
    let flipped = scroll.isFlipped();
    let y = if flipped {
        from_top
    } else {
        b.size.height - from_top - SPINNER
    };
    s.setFrame(NSRect::new(
        NSPoint::new(((b.size.width - SPINNER) / 2.).round(), y),
        NSSize::new(SPINNER, SPINNER),
    ));
    s.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewMinXMargin
            | NSAutoresizingMaskOptions::ViewMaxXMargin
            | if flipped {
                NSAutoresizingMaskOptions::ViewMaxYMargin
            } else {
                NSAutoresizingMaskOptions::ViewMinYMargin
            },
    );
    s.setAccessibilityLabel(Some(&NSString::from_str("Refreshing")));
    scroll.addSubview(&s);
    unsafe { s.startAnimation(None) };
    Shown {
        spinner: s,
        automatic_insets,
    }
}

/// Removes the spinner and its gap, the content moving back up when the
/// gap was shown.
fn hide(scroll: &NSScrollView, shown: Shown) {
    unsafe { shown.spinner.stopAnimation(None) };
    shown.spinner.removeFromSuperview();
    let mut insets = scroll.contentInsets();
    insets.top = (insets.top - GAP).max(0.);
    scroll.setContentInsets(insets);
    scroll.setAutomaticallyAdjustsContentInsets(shown.automatic_insets);
    let clip = scroll.contentView();
    let origin = clip.bounds().origin;
    if origin.y < -insets.top {
        clip.scrollToPoint(NSPoint::new(origin.x, -insets.top));
        scroll.reflectScrolledClipView(&clip);
    }
}

/// How far `scroll`'s content is pulled past its top (elastic scrolling),
/// in points; 0 when it is not.
fn pulled_past_top(scroll: &NSScrollView) -> f64 {
    let clip = scroll.contentView();
    let top = clip.bounds().origin.y + clip.contentInsets().top;
    (-top).max(0.)
}

/// A scroll wheel event `scroll` (node `id`) handled: the gesture pulled
/// its content past the top far enough, once per gesture.
pub(crate) fn scroll_wheel(id: NodeId, scroll: &NSScrollView, event: &NSEvent) {
    let refresh = REFRESHABLE.with(|r| {
        let r = r.borrow();
        let Some(entry) = r.get(&id) else {
            return false;
        };
        let phase = event.phase();
        if phase.intersects(NSEventPhase::Began | NSEventPhase::MayBegin) {
            entry.pulled.set(false);
        }
        let pull = pulled_past_top(scroll);
        let refresh = pull_refreshes(
            phase,
            event.momentumPhase(),
            entry.pulled.get() || entry.shown.is_some(),
            pull,
        );
        if refresh {
            entry.pulled.set(true);
        }
        refresh
    });
    if refresh {
        crate::imp::dispatch_or_defer(UiEvent::Click(id));
    }
}

/// Whether a scroll event refreshes: the fingers' own scrolling (not the
/// momentum after they lifted) pulled the content `pull` points past the
/// top, at least [`PULL`], unless this gesture already refreshed or a
/// refresh is shown (`done`).
fn pull_refreshes(phase: NSEventPhase, momentum: NSEventPhase, done: bool, pull: f64) -> bool {
    let touching = phase.intersects(NSEventPhase::Began | NSEventPhase::Changed)
        && momentum == NSEventPhase::None;
    touching && !done && pull >= PULL
}

/// ⌘R refreshes the refreshable scroll views shown in the key window.
fn install_key_monitor() {
    if MONITOR.replace(true) {
        return;
    }
    let block = block2::RcBlock::new(|event: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a valid event.
        let e = unsafe { event.as_ref() };
        if !is_reload(e) {
            return event.as_ptr();
        }
        let mtm = MainThreadMarker::new().expect("main thread");
        let window = e.window(mtm);
        let ids: Vec<NodeId> = REFRESHABLE.with(|r| {
            r.borrow()
                .iter()
                .filter_map(|(id, entry)| {
                    let scroll = entry.scroll.load()?;
                    let shown = scroll.window().is_some_and(|w| Some(&w) == window.as_ref())
                        && !scroll.isHiddenOrHasHiddenAncestor();
                    shown.then_some(*id)
                })
                .collect()
        });
        if ids.is_empty() {
            return event.as_ptr();
        }
        for id in ids {
            crate::imp::dispatch_or_defer(UiEvent::Click(id));
        }
        std::ptr::null_mut()
    });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            objc2_app_kit::NSEventMask::KeyDown,
            &block,
        )
    };
    // Lives as long as the app.
    std::mem::forget(monitor);
}

/// ⌘R alone (no ⇧, ⌥ or ⌃), not repeated.
fn is_reload(e: &NSEvent) -> bool {
    if e.r#type() != NSEventType::KeyDown || e.isARepeat() {
        return false;
    }
    let mods = e.modifierFlags() & NSEventModifierFlags::DeviceIndependentFlagsMask;
    let others =
        NSEventModifierFlags::Shift | NSEventModifierFlags::Option | NSEventModifierFlags::Control;
    mods.contains(NSEventModifierFlags::Command)
        && !mods.intersects(others)
        && e.charactersIgnoringModifiers()
            .is_some_and(|c| c.to_string().eq_ignore_ascii_case("r"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_past_the_top_refreshes_once_while_the_fingers_scroll() {
        let none = NSEventPhase::None;
        let changed = NSEventPhase::Changed;
        assert!(pull_refreshes(changed, none, false, PULL));
        assert!(pull_refreshes(NSEventPhase::Began, none, false, PULL + 20.));
        assert!(
            !pull_refreshes(changed, none, false, PULL - 1.),
            "not far enough"
        );
        assert!(
            !pull_refreshes(changed, none, true, PULL * 2.),
            "once per gesture"
        );
        // Momentum after the fingers lifted, a wheel without phases, the
        // gesture's end: never.
        assert!(!pull_refreshes(none, changed, false, PULL * 2.));
        assert!(!pull_refreshes(none, none, false, PULL * 2.));
        assert!(!pull_refreshes(NSEventPhase::Ended, none, false, PULL * 2.));
    }

    #[test]
    fn command_r_alone_reloads() {
        use objc2_app_kit::NSEventType;
        use objc2_foundation::NSPoint;
        let key = |mods: NSEventModifierFlags, c: &str, repeat: bool| {
            let c = NSString::from_str(c);
            NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                NSEventType::KeyDown,
                NSPoint::ZERO,
                mods,
                0.,
                0,
                None,
                &c,
                &c,
                repeat,
                15,
            )
            .unwrap()
        };
        let cmd = NSEventModifierFlags::Command;
        assert!(is_reload(&key(cmd, "r", false)));
        assert!(is_reload(&key(
            cmd | NSEventModifierFlags::CapsLock,
            "R",
            false
        )));
        assert!(!is_reload(&key(cmd, "r", true)), "held down");
        assert!(!is_reload(&key(
            cmd | NSEventModifierFlags::Shift,
            "r",
            false
        )));
        assert!(!is_reload(&key(
            cmd | NSEventModifierFlags::Option,
            "r",
            false
        )));
        assert!(!is_reload(&key(NSEventModifierFlags::empty(), "r", false)));
        assert!(!is_reload(&key(cmd, "t", false)));
    }
}
