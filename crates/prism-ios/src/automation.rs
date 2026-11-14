//! Development hooks, like the macOS backend's, for simulators that cannot
//! be driven from outside (tvOS) and for scripted checks on iOS:
//!
//! - `PRISM_CLICK="+1|Save"` presses buttons by title, or clickable views
//!   by accessibility label, in order;
//! - `PRISM_SNAPSHOT=/abs/out.png` then writes the window to a PNG and quits;
//! - `PRISM_DELAY` (seconds, default 1) before clicking, `PRISM_AFTER`
//!   (default 0.5) between the clicks and the snapshot.

use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, class, msg_send};
use objc2_foundation::{NSData, NSString};
use objc2_ui_kit::{UIButton, UIControlEvents, UIControlState, UIView};
use prism_core::{NodeId, UiEvent};

fn seconds(var: &str, default: f64) -> Duration {
    Duration::from_secs_f64(
        std::env::var(var)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default),
    )
}

fn after(delay: Duration, f: impl FnOnce() + Send + 'static) {
    let when = dispatch2::DispatchTime::try_from(delay).unwrap_or(dispatch2::DispatchTime::NOW);
    let _ = dispatch2::DispatchQueue::main().after(when, f);
}

/// Schedules the hooks if any is set.
pub(crate) fn schedule() {
    let snapshot = std::env::var("PRISM_SNAPSHOT").ok();
    let clicks = std::env::var("PRISM_CLICK").ok();
    if snapshot.is_none() && clicks.is_none() {
        return;
    }
    after(seconds("PRISM_DELAY", 1.), move || {
        let mtm = MainThreadMarker::new().expect("main thread");
        for title in clicks
            .iter()
            .flat_map(|c| c.split('|'))
            .filter(|t| !t.is_empty())
        {
            if !press(mtm, title) {
                eprintln!("prism-ios: PRISM_CLICK: nothing labelled {title:?}");
            }
        }
        if let Some(path) = snapshot {
            after(seconds("PRISM_AFTER", 0.5), move || {
                let mtm = MainThreadMarker::new().expect("main thread");
                match write_snapshot(mtm, &path) {
                    true => eprintln!("prism-ios: snapshot written to {path}"),
                    false => eprintln!("prism-ios: snapshot to {path} failed"),
                }
                std::process::exit(0);
            });
        }
    });
}

fn key_window(mtm: MainThreadMarker) -> Option<Retained<UIView>> {
    let app = objc2_ui_kit::UIApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    let windows = app.windows();
    let window = windows
        .iter()
        .find(|w| w.isKeyWindow())
        .or_else(|| windows.firstObject())?;
    Some(Retained::into_super(window))
}

/// Presses the first visible button titled `title`, or clickable view
/// labelled `title`.
fn press(mtm: MainThreadMarker, title: &str) -> bool {
    let Some(window) = key_window(mtm) else {
        return false;
    };
    let Some(view) = find(&window, title) else {
        return false;
    };
    if let Ok(button) = view.clone().downcast::<UIButton>() {
        button.sendActionsForControlEvents(
            UIControlEvents::PrimaryActionTriggered | UIControlEvents::TouchUpInside,
        );
    } else {
        crate::imp::dispatch_event(UiEvent::Click(view.tag() as NodeId));
    }
    true
}

fn find(view: &UIView, title: &str) -> Option<Retained<UIView>> {
    for sub in view.subviews().iter() {
        if sub.isHidden() || sub.alpha() < 0.01 {
            continue;
        }
        if let Ok(b) = sub.clone().downcast::<UIButton>()
            && b.titleForState(UIControlState::Normal)
                .is_some_and(|t| t.to_string() == title)
        {
            return Some(sub.clone());
        }
        let label: Option<Retained<NSString>> = unsafe { msg_send![&*sub, accessibilityLabel] };
        if sub.tag() > 0
            && !sub.gestureRecognizers().is_none_or(|g| g.is_empty())
            && label.is_some_and(|l| l.to_string() == title)
        {
            return Some(sub.clone());
        }
        if let Some(found) = find(&sub, title) {
            return Some(found);
        }
    }
    None
}

/// Draws the key window into a PNG at `path`.
fn write_snapshot(mtm: MainThreadMarker, path: &str) -> bool {
    let Some(window) = key_window(mtm) else {
        return false;
    };
    let bounds = window.bounds();
    let target = window.clone();
    let draw = RcBlock::new(move |_context: *mut AnyObject| {
        let _: bool = unsafe {
            msg_send![&*target, drawViewHierarchyInRect: bounds, afterScreenUpdates: true]
        };
    });
    unsafe {
        let renderer: Retained<AnyObject> =
            msg_send![msg_send![class!(UIGraphicsImageRenderer), alloc], initWithBounds: bounds];
        let png: Retained<NSData> = msg_send![&*renderer, PNGDataWithActions: &*draw];
        png.writeToFile_atomically(&NSString::from_str(path), true)
    }
}
