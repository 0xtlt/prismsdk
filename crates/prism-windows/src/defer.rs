//! Off-screen content joins XAML's layout after the window shows what is
//! in view.
//!
//! A long screen (the counter's components: 650 nodes, a window of them
//! in view) costs XAML most of its first frame in what nobody sees yet:
//! control templates, layout passes, visuals. New nodes in a scroll view
//! whose frames lie wholly outside the window are made `Collapsed` (XAML
//! skips collapsed elements' templates, layout and rendering) and shown
//! again once a frame has shown the rest: a few at a time, nearest first,
//! one batch per frame, and at once when scrolling brings them in view.
//! Prism's own layout (prism-native's) is not affected: every node has
//! its frame and size from the start.

use std::cell::{Cell, RefCell};

use prism_core::{IdSet, NodeId, NodeKind};

use crate::scene::{Rect, Scene};
use crate::widgets::Win;
use crate::winui::xaml::Media::CompositionTarget;

/// Nodes shown per frame while catching up (each costs XAML its
/// elements' templates and layout).
const PER_FRAME: usize = 48;

thread_local! {
    /// Nodes created since the last layout.
    static FRESH: RefCell<Vec<NodeId>> = const { RefCell::new(Vec::new()) };
    /// Collapsed until shown: the roots of off-screen subtrees.
    static DEFERRED: RefCell<IdSet<NodeId>> = RefCell::new(IdSet::default());
    /// The per-frame handler while some are left (`Rendering`).
    static TICKING: Cell<Option<i64>> = const { Cell::new(None) };
}

/// Off unless `PRISM_DEFER=0`.
fn enabled() -> bool {
    thread_local! {
        static ON: bool = crate::win::env("PRISM_DEFER").is_none_or(|v| v != "0");
    }
    ON.with(|on| *on)
}

/// Node `id` was just created.
pub fn created(id: NodeId) {
    if enabled() {
        FRESH.with(|f| f.borrow_mut().push(id));
    }
}

/// Node `id` is gone.
pub fn forget(id: NodeId) {
    DEFERRED.with(|d| d.borrow_mut().remove(&id));
}

/// Whether node `id` waits to be shown.
pub fn deferred(id: NodeId) -> bool {
    DEFERRED.with(|d| !d.borrow().is_empty() && d.borrow().contains(&id))
}

/// The visible part of the window, with half a window of margin around
/// it: what scrolling a little shows at once.
fn view(size: (f32, f32)) -> Rect {
    let (w, h) = size;
    Rect::new(-w / 2., -h / 2., w * 2., h * 2.)
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// Whether node `id` can wait: inside a scroll view, not in an overlay,
/// not sticky.
fn can_wait(scene: &Scene, id: NodeId) -> bool {
    let Some(n) = scene.get(id) else { return false };
    if n.sticky.is_some() || n.kind == NodeKind::Overlay {
        return false;
    }
    let mut scrolled = false;
    for a in scene.ancestors(id) {
        match scene.get(a).map(|a| a.kind) {
            Some(NodeKind::Overlay) => return false,
            Some(NodeKind::Scroll) => scrolled = true,
            _ => {}
        }
    }
    scrolled
}

/// After a layout: the new nodes outside the window are collapsed, the
/// biggest subtrees possible (their roots).
pub fn collapse_fresh(w: &Win) {
    let fresh: Vec<NodeId> = FRESH.with(|f| std::mem::take(&mut *f.borrow_mut()));
    if fresh.is_empty() {
        return;
    }
    let set: IdSet<NodeId> = fresh.iter().copied().collect();
    let area = view(w.size);
    let mut collapsed = Vec::new();
    for id in fresh {
        let Some(n) = w.scene.get(id) else { continue };
        // A new subtree's root (its parent is not new), or a child of a
        // new node that is partly in view (the walk below).
        if n.parent.is_some_and(|p| set.contains(&p)) {
            continue;
        }
        walk(w, id, area, &set, &mut collapsed);
    }
    if collapsed.is_empty() {
        return;
    }
    DEFERRED.with(|d| d.borrow_mut().extend(collapsed.iter().copied()));
    for id in collapsed {
        if let Some(v) = w.views.get(&id) {
            let _ = v.set_visible(false);
        }
    }
    tick_on();
}

fn walk(w: &Win, id: NodeId, area: Rect, fresh: &IdSet<NodeId>, out: &mut Vec<NodeId>) {
    let r = w.scene.window_rect(id);
    if !intersects(r, area) && can_wait(&w.scene, id) {
        out.push(id);
        return;
    }
    let Some(n) = w.scene.get(id) else { return };
    for c in &n.children {
        if fresh.contains(c) {
            walk(w, *c, area, fresh, out);
        }
    }
}

/// Shows the waiting nodes now in view (the user scrolled), or the
/// `budget` nearest when `budget` is some.
fn show(w: &Win, budget: Option<usize>) {
    let area = view(w.size);
    let mut waiting: Vec<(NodeId, Rect)> = DEFERRED.with(|d| {
        d.borrow()
            .iter()
            .map(|id| (*id, w.scene.window_rect(*id)))
            .collect()
    });
    let mut shown = Vec::new();
    match budget {
        None => {
            shown.extend(
                waiting
                    .iter()
                    .filter(|(_, r)| intersects(*r, area))
                    .map(|(id, _)| *id),
            );
        }
        Some(budget) => {
            // Nearest to the window first.
            let distance = |r: &Rect| {
                let dy = (r.y - area.y).max(area.y - (r.y + r.h)).max(0.);
                let dx = (r.x - area.x).max(area.x - (r.x + r.w)).max(0.);
                dx + dy
            };
            waiting.sort_by(|a, b| distance(&a.1).total_cmp(&distance(&b.1)));
            let mut nodes = 0;
            for (id, _) in waiting {
                if nodes >= budget {
                    break;
                }
                nodes += size(&w.scene, id);
                shown.push(id);
            }
        }
    }
    if shown.is_empty() {
        return;
    }
    DEFERRED.with(|d| {
        let mut d = d.borrow_mut();
        for id in &shown {
            d.remove(id);
        }
    });
    for id in shown {
        w.update_visibility(id);
    }
}

/// How many nodes the subtree of `id` holds.
fn size(scene: &Scene, id: NodeId) -> usize {
    let mut n = 0;
    let mut stack = vec![id];
    while let Some(i) = stack.pop() {
        n += 1;
        if let Some(node) = scene.get(i) {
            stack.extend(node.children.iter().copied());
        }
    }
    n
}

/// A scroll view moved: what it brings in view shows now.
pub fn scrolled(w: &Win) {
    if DEFERRED.with(|d| !d.borrow().is_empty()) {
        show(w, None);
    }
}

/// Every frame while nodes wait: once the window has shown its first
/// frame, the next batch.
fn tick_on() {
    if TICKING.get().is_some() {
        return;
    }
    let handler = windows::Foundation::EventHandler::<windows_core::IInspectable>::new(|_, _| {
        if !crate::imp::framed() {
            return Ok(());
        }
        crate::imp::with_state(|s| show(&s.renderer.widgets, Some(PER_FRAME)));
        if DEFERRED.with(|d| d.borrow().is_empty())
            && let Some(t) = TICKING.take()
        {
            let _ = CompositionTarget::RemoveRendering(t);
            if crate::perf::on() {
                crate::imp::trace("off-screen content shown");
            }
        }
        Ok(())
    });
    match CompositionTarget::Rendering(&handler) {
        Ok(t) => TICKING.set(Some(t)),
        Err(_) => {
            // No frames to wait for: everything shows (once the window's
            // state is free).
            crate::imp::defer(Box::new(|s| {
                let ids: Vec<NodeId> = DEFERRED.with(|d| d.borrow_mut().drain().collect());
                for id in ids {
                    s.renderer.widgets.update_visibility(id);
                }
            }));
        }
    }
}
