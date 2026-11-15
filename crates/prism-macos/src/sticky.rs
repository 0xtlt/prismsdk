//! `.sticky()` views: they follow their scroll view's clip view, moved down
//! by `Sticky::offset` from their unscrolled frame, drawn above their
//! siblings. One bounds observer per clip view serves both sticky views and
//! scroll events (`NSNotificationCenter` removes observers by object, so the
//! two must share it).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use objc2::rc::Retained;
use objc2::sel;
use objc2_app_kit::{NSClipView, NSView};
use objc2_foundation::{NSNotificationCenter, NSRect};
use prism_core::NodeId;
use prism_native::Sticky;

struct Follower {
    view: Retained<NSView>,
    clip: Retained<NSClipView>,
    sticky: Sticky,
    /// The frame the layout gave it (unscrolled).
    frame: NSRect,
}

thread_local! {
    static FOLLOWERS: RefCell<HashMap<NodeId, Follower>> = RefCell::new(HashMap::new());
    /// Scroll views (by document id) whose app listens to `on_scroll`.
    static SCROLL_EVENTS: RefCell<HashSet<NodeId>> = RefCell::new(HashSet::new());
}

fn shifted(f: &Follower) -> NSRect {
    let b = f.clip.bounds();
    let top = b.origin.y + f.clip.contentInsets().top;
    let mut frame = f.frame;
    frame.origin.y += f.sticky.offset(top as f32) as f64;
    frame
}

fn doc_id(clip: &NSClipView) -> Option<NodeId> {
    let doc = clip
        .documentView()?
        .downcast::<crate::imp::PrismView>()
        .ok()?;
    Some(crate::imp::node_id(&doc))
}

/// Observes `clip`'s bounds while something needs them.
fn observe(target: &crate::imp::Target, clip: &NSClipView) {
    let center = NSNotificationCenter::defaultCenter();
    let name = unsafe { objc2_app_kit::NSViewBoundsDidChangeNotification };
    unsafe { center.removeObserver_name_object(target, Some(name), Some(clip)) };
    let events = doc_id(clip).is_some_and(|id| SCROLL_EVENTS.with(|s| s.borrow().contains(&id)));
    let sticky = FOLLOWERS.with(|f| f.borrow().values().any(|f| *f.clip == *clip));
    if events || sticky {
        clip.setPostsBoundsChangedNotifications(true);
        unsafe {
            center.addObserver_selector_name_object(
                target,
                sel!(onScrollBounds:),
                Some(name),
                Some(clip),
            )
        };
    }
}

/// `on_scroll` handlers on or off for the scroll view whose clip is `clip`.
pub(crate) fn set_scroll_events(target: &crate::imp::Target, clip: &NSClipView, on: bool) {
    if let Some(id) = doc_id(clip) {
        SCROLL_EVENTS.with(|s| {
            let mut s = s.borrow_mut();
            if on {
                s.insert(id);
            } else {
                s.remove(&id);
            }
        });
    }
    observe(target, clip);
}

/// Whether the app listens to this scroll view's scrolling.
pub(crate) fn wants_scroll_events(clip: &NSClipView) -> bool {
    doc_id(clip).is_some_and(|id| SCROLL_EVENTS.with(|s| s.borrow().contains(&id)))
}

/// The clip view scrolled: move its sticky views.
pub(crate) fn scrolled(clip: &NSClipView) {
    let moves: Vec<(Retained<NSView>, NSRect)> = FOLLOWERS.with(|f| {
        let Ok(f) = f.try_borrow() else {
            return Vec::new();
        };
        f.values()
            .filter(|f| *f.clip == *clip)
            .map(|f| (f.view.clone(), shifted(f)))
            .collect()
    });
    for (view, frame) in moves {
        view.setFrame(frame);
    }
}

/// The frame to give `id` for its layout frame `frame` (shifted if sticky).
pub(crate) fn frame_for(id: NodeId, frame: NSRect) -> NSRect {
    FOLLOWERS.with(|f| {
        let Ok(mut f) = f.try_borrow_mut() else {
            return frame;
        };
        match f.get_mut(&id) {
            Some(follower) => {
                follower.frame = frame;
                shifted(follower)
            }
            None => frame,
        }
    })
}

/// Starts or stops `view` following `clip`.
pub(crate) fn set(
    target: &crate::imp::Target,
    id: NodeId,
    view: &NSView,
    sticky: Option<(Retained<NSClipView>, Sticky)>,
) {
    let old = FOLLOWERS.with(|f| f.borrow_mut().remove(&id));
    let base = old.as_ref().map_or_else(|| view.frame(), |o| o.frame);
    match sticky {
        Some((clip, sticky)) => {
            let follower = Follower {
                view: objc2::Message::retain(view),
                clip: clip.clone(),
                sticky,
                frame: base,
            };
            let frame = shifted(&follower);
            FOLLOWERS.with(|f| f.borrow_mut().insert(id, follower));
            view.setFrame(frame);
            if let Some(parent) = unsafe { view.superview() } {
                raise(&parent);
            }
            observe(target, &clip);
        }
        None => view.setFrame(base),
    }
    if let Some(old) = old {
        observe(target, &old.clip);
    }
}

/// `id` is gone.
pub(crate) fn forget(id: NodeId) {
    FOLLOWERS.with(|f| f.borrow_mut().remove(&id));
    SCROLL_EVENTS.with(|s| s.borrow_mut().remove(&id));
}

/// Draws the sticky children of `parent` last, above the content that
/// scrolls under them (their frames, not their order, place them).
pub(crate) fn raise(parent: &NSView) {
    let sticky: Vec<Retained<NSView>> = FOLLOWERS.with(|f| {
        f.borrow()
            .values()
            .filter(|f| unsafe { f.view.superview() }.is_some_and(|s| *s == *parent))
            .map(|f| f.view.clone())
            .collect()
    });
    for view in sticky {
        parent.addSubview_positioned_relativeTo(
            &view,
            objc2_app_kit::NSWindowOrderingMode::Above,
            None,
        );
    }
}
