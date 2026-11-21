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
//! is the elements' order. A lazy child (`defer.rs`) joins its parent in
//! the tree's order when it gets its elements, kept here meanwhile.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use prism_core::NodeId;

use crate::widgets::Win;
use crate::winui::xaml::Controls::Canvas;

thread_local! {
    /// The elements given a `Canvas.ZIndex` other than 0, and its value.
    static Z: RefCell<HashMap<NodeId, i32>> = RefCell::default();
    /// A parent's children in the tree's order, where it is not their
    /// paint order.
    static READING: RefCell<HashMap<NodeId, Vec<NodeId>>> = RefCell::default();
}

/// `parent`'s children in the tree's order (`set_reading_order`), or
/// `None`: in their paint order (`set_children`).
pub fn read_in(parent: NodeId, order: Option<&[NodeId]>) {
    READING.with(|r| {
        let mut r = r.borrow_mut();
        match order {
            Some(o) => r.insert(parent, o.to_vec()),
            None => r.remove(&parent),
        };
    });
}

/// `parent`'s children in the tree's order, if it is not their paint order.
pub fn reading_order(parent: NodeId) -> Option<Vec<NodeId>> {
    READING.with(|r| r.borrow().get(&parent).cloned())
}

/// Each child of `parent` drawn in its paint order, a sticky child above
/// the others, whatever their elements' order.
pub fn restack(w: &Win, parent: NodeId) {
    let (Some(n), Some(v)) = (w.scene.get(parent), w.views.get(&parent)) else {
        return;
    };
    let paint = &n.children;
    let sticky = |c: NodeId| w.scene.get(c).is_some_and(|n| n.sticky.is_some());
    // The children that have elements (lazy ones may not yet).
    let in_order = *paint == v.children || {
        let linked: HashSet<NodeId> = v.children.iter().copied().collect();
        paint
            .iter()
            .filter(|c| linked.contains(c))
            .eq(v.children.iter())
    };
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
    READING.with(|r| r.borrow_mut().remove(&id));
}
