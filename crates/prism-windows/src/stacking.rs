//! Paint order apart from reading order. XAML draws a panel's children in
//! their order, then by `Canvas.ZIndex`; UI Automation and the Tab key
//! follow their order alone. prism-native gives `set_children` the paint
//! order (`z_index`, #232) and, when it differs, the tree's order right
//! after it (`set_reading_order`): the children go back to the tree's
//! order and each gets its rank in the paint order as its `Canvas.ZIndex`.
//! A sticky child is drawn above its siblings the same way (as
//! `Scene::hit_test` finds it first), whatever its place.
//!
//! `Scene::children` stays in paint order (hit testing); `View::children`
//! is the elements' order.

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::NodeId;

use crate::widgets::Win;
use crate::winui::xaml::Controls::Canvas;

thread_local! {
    /// The elements given a `Canvas.ZIndex` other than 0, and its value.
    static Z: RefCell<HashMap<NodeId, i32>> = RefCell::default();
}

/// Each child of `parent` drawn in its paint order, a sticky child above
/// the others, whatever their elements' order.
pub fn restack(w: &Win, parent: NodeId) {
    let (Some(n), Some(v)) = (w.scene.get(parent), w.views.get(&parent)) else {
        return;
    };
    let paint = &n.children;
    let sticky = |c: NodeId| w.scene.get(c).is_some_and(|n| n.sticky.is_some());
    let in_order = *paint == v.children;
    let lifted = paint.iter().any(|c| sticky(*c));
    // Nothing to draw out of order, and no element keeps an older index.
    if in_order && !lifted && Z.with(|z| z.borrow().is_empty()) {
        return;
    }
    let len = paint.len() as i32;
    for (rank, &c) in paint.iter().enumerate() {
        let lift = if sticky(c) { len } else { 0 };
        let z = if in_order { lift } else { rank as i32 + lift };
        let old = Z.with(|m| m.borrow().get(&c).copied().unwrap_or(0));
        if z == old {
            continue;
        }
        let Some(e) = w.outer(c) else { continue };
        if let Err(err) = Canvas::SetZIndex(&e, z) {
            crate::win::log!("paint order of node {c}: {err}");
            continue;
        }
        Z.with(|m| {
            let mut m = m.borrow_mut();
            if z == 0 {
                m.remove(&c);
            } else {
                m.insert(c, z);
            }
        });
    }
}

/// Node `id` is gone.
pub fn forget(id: NodeId) {
    Z.with(|m| m.borrow_mut().remove(&id));
}
