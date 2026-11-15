//! Layout beyond flexbox on UIKit: text baselines for `items_baseline()`,
//! horizontal scroll views, and sticky views that follow their scroll view.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use objc2::Message;
use objc2::rc::Retained;
use objc2_core_foundation::{CGPoint, CGSize};
use objc2_ui_kit::{UIFont, UIScrollView, UIScrollViewContentInsetAdjustmentBehavior, UIView};
use prism_core::{NodeId, ScrollAxis};
use prism_native::Sticky;

use crate::imp::View;

struct StickyView {
    view: Retained<UIView>,
    scroll: Retained<UIScrollView>,
    sticky: Sticky,
    /// How far it is moved down now.
    offset: f64,
}

thread_local! {
    /// The font each text leaf draws with (view units), for baselines.
    static FONTS: RefCell<HashMap<NodeId, Retained<UIFont>>> = RefCell::new(HashMap::new());
    /// Sticky views, by node id.
    static STICKY: RefCell<HashMap<NodeId, StickyView>> = RefCell::new(HashMap::new());
    /// Scroll views whose app wants scroll events.
    static SCROLL_EVENTS: RefCell<HashSet<NodeId>> = RefCell::new(HashSet::new());
}

/// Remembers the font of a text leaf (its style is already zoomed and scaled).
pub fn remember_font(view: &View, font: impl FnOnce() -> Retained<UIFont>) {
    if matches!(
        view,
        View::Label(_)
            | View::Timer(_)
            | View::Button(_)
            | View::Input(_)
            | View::Picker(_)
            | View::DatePicker(_)
    ) {
        let font = font();
        FONTS.with(|f| f.borrow_mut().insert(view.id(), font));
    }
}

pub fn forget(id: NodeId) {
    FONTS.with(|f| f.borrow_mut().remove(&id));
    STICKY.with(|s| s.borrow_mut().remove(&id));
    SCROLL_EVENTS.with(|s| s.borrow_mut().remove(&id));
}

/// Distance from the top of a `width` × `height` leaf to its first baseline.
/// Labels draw their text centered vertically in their bounds (at the top
/// when the bounds fit the text); controls center one line of text.
pub fn baseline(view: &View, width: f32, height: f32) -> Option<f32> {
    let font = FONTS.with(|f| f.borrow().get(&view.id()).cloned())?;
    let (ascent, line) = unsafe { (font.ascender(), font.lineHeight()) };
    let h = height as f64;
    let b = match view {
        View::Label(l) | View::Timer(l) => {
            let text = l
                .sizeThatFits(CGSize::new(width as f64, f64::MAX / 4.))
                .height
                .ceil();
            ((h - text) / 2.).max(0.) + ascent
        }
        View::Button(_) | View::Input(_) | View::Picker(_) | View::DatePicker(_) => {
            (h - line) / 2. + ascent
        }
        _ => return None,
    };
    Some(b as f32)
}

/// Vertical (the default) or horizontal scrolling.
pub fn set_axis(s: &UIScrollView, axis: ScrollAxis) {
    let horizontal = axis == ScrollAxis::Horizontal;
    s.setAlwaysBounceVertical(!horizontal);
    s.setAlwaysBounceHorizontal(horizontal);
    s.setShowsVerticalScrollIndicator(!horizontal);
    s.setShowsHorizontalScrollIndicator(true);
    // A row of chips never runs under bars: no automatic insets.
    s.setContentInsetAdjustmentBehavior(if horizontal {
        UIScrollViewContentInsetAdjustmentBehavior::Never
    } else {
        UIScrollViewContentInsetAdjustmentBehavior::Automatic
    });
}

/// Whether the scroll view needs a delegate (events or sticky views).
pub fn wants_delegate(scroll: NodeId) -> bool {
    SCROLL_EVENTS.with(|s| s.borrow().contains(&scroll))
        || STICKY.with(|s| {
            s.borrow()
                .values()
                .any(|v| v.scroll.tag() as NodeId == scroll)
        })
}

pub fn set_scroll_events(scroll: NodeId, on: bool) {
    SCROLL_EVENTS.with(|s| {
        let mut s = s.borrow_mut();
        if on {
            s.insert(scroll);
        } else {
            s.remove(&scroll);
        }
    });
}

pub fn scroll_events(scroll: NodeId) -> bool {
    SCROLL_EVENTS.with(|s| s.borrow().contains(&scroll))
}

fn visible_top(s: &UIScrollView) -> f64 {
    s.contentOffset().y + s.adjustedContentInset().top
}

fn shift(v: &UIView, by: f64) {
    if by != 0. {
        let c = v.center();
        v.setCenter(CGPoint::new(c.x, c.y + by));
    }
}

/// `view` is sticky in `scroll` (or not anymore).
pub fn set_sticky(view: &UIView, id: NodeId, sticky: Option<(&UIScrollView, Sticky)>) {
    let old = STICKY.with(|s| s.borrow_mut().remove(&id));
    let previous = old.as_ref().map_or(0., |o| o.offset);
    match sticky {
        Some((scroll, sticky)) => {
            let offset = sticky.offset(visible_top(scroll) as f32) as f64;
            shift(view, offset - previous);
            // Above the rows that scroll under it.
            view.layer().setZPosition(1.);
            STICKY.with(|s| {
                s.borrow_mut().insert(
                    id,
                    StickyView {
                        view: view.retain(),
                        scroll: scroll.retain(),
                        sticky,
                        offset,
                    },
                )
            });
        }
        None => {
            shift(view, -previous);
            view.layer().setZPosition(0.);
        }
    }
}

/// A new (unscrolled) frame was set: move it to where it sticks again.
pub fn framed(id: NodeId) {
    STICKY.with(|s| {
        if let Some(v) = s.borrow_mut().get_mut(&id) {
            v.offset = v.sticky.offset(visible_top(&v.scroll) as f32) as f64;
            shift(&v.view, v.offset);
        }
    });
}

/// The scroll view moved: its sticky views follow.
pub fn scrolled(scroll: &UIScrollView) {
    let top = visible_top(scroll) as f32;
    STICKY.with(|s| {
        for v in s.borrow_mut().values_mut() {
            if !std::ptr::eq(&*v.scroll, scroll) {
                continue;
            }
            let offset = v.sticky.offset(top) as f64;
            shift(&v.view, offset - v.offset);
            v.offset = offset;
        }
    });
}
