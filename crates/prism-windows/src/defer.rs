//! Off-screen content gets its XAML elements after the window shows what
//! is in view.
//!
//! A long screen (the counter's components: 700 nodes, a window of them
//! in view) costs most of its first frame in what nobody sees yet: XAML
//! elements to create and style, control templates, layout passes,
//! visuals. Prism's own layout (prism-native's) needs no element: every
//! node has its frame and size from the scene (`scene.rs`, `text.rs`).
//!
//! So nodes of the simple kinds ([`lazy_kind`]: views, texts, buttons,
//! fields, most controls) get no elements when created. After the layout,
//! the new nodes in view get theirs, made from what the scene holds
//! (`Win::materialize`); the subtrees in a scroll view whose frames lie
//! wholly outside the window wait. Once a frame has shown the rest, they
//! are made a few at a time, nearest first, one batch per frame; at once
//! when scrolling brings them in view, the focus moves into them, a popover
//! opens on them or a smoke step looks for them. A waiting subtree whose
//! root has its elements already (a kind made at once: a scroll view, a
//! stack, a 3D scene…) is `Collapsed` meanwhile.
//!
//! A11y: a made node gets its custom automation peer (`peers::canvas`,
//! `peers::button`) as any other; the UI Automation tree is whole once the
//! batches are done.
//!
//! `PRISM_DEFER=0` turns it off: every node gets its elements at once and
//! none waits.

use std::cell::{Cell, RefCell};

use prism_core::{IdMap, IdSet, NodeId, NodeKind};

use crate::controls;
use crate::perf::{self, Cost};
use crate::scene::{Rect, Scene};
use crate::widgets::Win;
use crate::win::log;
use crate::winui::xaml::Media::CompositionTarget;

/// Nodes made per frame while catching up (each costs its elements, their
/// templates and layout).
const PER_FRAME: usize = 48;

/// What was set on a lazy node beyond what making it always applies (its
/// style, text, config, icon, drawing, menu, frame, children, input).
pub const VALUE: u8 = 1;
pub const A11Y: u8 = 2;
pub const POINTER: u8 = 4;

thread_local! {
    /// Nodes created (or moved while lazy) since the last layout.
    static FRESH: RefCell<Vec<NodeId>> = const { RefCell::new(Vec::new()) };
    /// The roots of off-screen subtrees waiting to be shown.
    static DEFERRED: RefCell<IdSet<NodeId>> = RefCell::new(IdSet::default());
    /// Nodes without elements yet, and what was set on them ([`VALUE`]…).
    static LAZY: RefCell<IdMap<NodeId, u8>> = RefCell::new(IdMap::default());
    /// The per-frame handler while some wait (`Rendering`).
    static TICKING: Cell<Option<i64>> = const { Cell::new(None) };
}

/// On unless `PRISM_DEFER=0`.
fn enabled() -> bool {
    thread_local! {
        static ON: bool = crate::win::env("PRISM_DEFER").is_none_or(|v| v != "0");
    }
    ON.with(|on| *on)
}

/// Whether a node of `kind` can wait for its elements: everything it
/// shows is in the scene. The others are made at once: they hold state of
/// their own (a scroll offset, a page shown, a web page, a video, a 3D
/// scene, rich text) or stand for the window (overlays, stacks, tabs).
pub fn lazy_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::View
            | NodeKind::Text
            | NodeKind::Button
            | NodeKind::TextInput
            | NodeKind::TextArea
            | NodeKind::Toggle
            | NodeKind::Checkbox
            | NodeKind::RadioGroup
            | NodeKind::Slider
            | NodeKind::Picker
            | NodeKind::DatePicker
            | NodeKind::Progress
            | NodeKind::Spinner
            | NodeKind::Timer
            | NodeKind::Image
            | NodeKind::Picture
            | NodeKind::Shape
    )
}

/// Node `id` was just created: whether it waits for its elements.
pub fn created(id: NodeId, kind: NodeKind) -> bool {
    if !enabled() {
        return false;
    }
    FRESH.with(|f| f.borrow_mut().push(id));
    let lazy = lazy_kind(kind);
    if lazy {
        LAZY.with(|l| l.borrow_mut().insert(id, 0));
    }
    lazy
}

/// Whether node `id` has no elements yet.
pub fn lazy(id: NodeId) -> bool {
    LAZY.with(|l| {
        let l = l.borrow();
        !l.is_empty() && l.contains_key(&id)
    })
}

/// `what` was set on node `id` (replayed once made, if it is lazy).
pub fn mark(id: NodeId, what: u8) {
    LAZY.with(|l| {
        if let Some(set) = l.borrow_mut().get_mut(&id) {
            *set |= what;
        }
    });
}

/// Node `id` joined a parent: if lazy, the next layout decides again
/// whether it waits.
pub fn joined(id: NodeId) {
    if lazy(id) {
        FRESH.with(|f| f.borrow_mut().push(id));
    }
}

/// Node `id` is gone.
pub fn forget(id: NodeId) {
    DEFERRED.with(|d| d.borrow_mut().remove(&id));
    LAZY.with(|l| l.borrow_mut().remove(&id));
}

/// Whether node `id` waits to be shown.
pub fn deferred(id: NodeId) -> bool {
    DEFERRED.with(|d| !d.borrow().is_empty() && d.borrow().contains(&id))
}

fn waiting() -> bool {
    DEFERRED.with(|d| !d.borrow().is_empty()) || LAZY.with(|l| !l.borrow().is_empty())
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

/// After a layout: the new nodes in view get their elements; those
/// outside wait, the biggest subtrees possible (their roots).
pub fn settle(w: &mut Win) {
    let mut fresh: Vec<NodeId> = FRESH.with(|f| std::mem::take(&mut *f.borrow_mut()));
    if fresh.is_empty() {
        return;
    }
    let set: IdSet<NodeId> = fresh.iter().copied().collect();
    fresh.retain(|id| {
        // A new subtree's root; one under a waiting node waits with it.
        w.scene
            .get(*id)
            .is_some_and(|n| n.parent.is_none_or(|p| !set.contains(&p) && !lazy(p)))
    });
    let mut seen = IdSet::default();
    fresh.retain(|id| seen.insert(*id));
    let area = view(w.size);
    let mut out = Walk::default();
    for id in fresh {
        walk(w, id, area, &set, &mut out);
    }
    w.relink(&out.made);
    for id in out.shown {
        w.update_visibility(id);
    }
    if out.waiting.is_empty() {
        return;
    }
    DEFERRED.with(|d| d.borrow_mut().extend(out.waiting.iter().copied()));
    for id in out.waiting {
        if let Some(v) = w.views.get(&id) {
            let _ = v.set_visible(false);
        }
    }
    tick_on();
}

#[derive(Default)]
struct Walk {
    /// Nodes given their elements.
    made: Vec<NodeId>,
    /// Waiting roots in view again.
    shown: Vec<NodeId>,
    /// New roots that wait.
    waiting: Vec<NodeId>,
}

fn walk(w: &mut Win, id: NodeId, area: Rect, fresh: &IdSet<NodeId>, out: &mut Walk) {
    if !intersects(w.scene.window_rect(id), area) && can_wait(&w.scene, id) {
        out.waiting.push(id);
        return;
    }
    if DEFERRED.with(|d| d.borrow_mut().remove(&id)) {
        out.shown.push(id);
    }
    if w.materialize(id) {
        out.made.push(id);
    }
    let children = w
        .scene
        .get(id)
        .map(|n| n.children.clone())
        .unwrap_or_default();
    for c in children {
        if fresh.contains(&c) || lazy(c) || deferred(c) {
            walk(w, c, area, fresh, out);
        }
    }
}

/// Makes every lazy node of the subtree at `root`, parents first.
fn make_tree(w: &mut Win, root: NodeId, made: &mut Vec<NodeId>) {
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        DEFERRED.with(|d| d.borrow_mut().remove(&id));
        if w.materialize(id) {
            made.push(id);
        }
        if let Some(n) = w.scene.get(id) {
            stack.extend(n.children.iter().rev().copied());
        }
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

/// The waiting root nearest to the window.
fn nearest(scene: &Scene, area: Rect) -> Option<NodeId> {
    let distance = |r: Rect| {
        let dy = (r.y - area.y).max(area.y - (r.y + r.h)).max(0.);
        let dx = (r.x - area.x).max(area.x - (r.x + r.w)).max(0.);
        dx + dy
    };
    DEFERRED.with(|d| {
        d.borrow()
            .iter()
            .map(|id| (*id, distance(scene.window_rect(*id))))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    })
}

/// Shows the waiting subtrees now in view (the user scrolled), or about
/// `budget` nodes, nearest first, when `budget` is some.
fn show(w: &mut Win, budget: Option<usize>) {
    let area = view(w.size);
    let mut made = Vec::new();
    let mut shown = Vec::new();
    match budget {
        None => {
            let ids: Vec<NodeId> = DEFERRED.with(|d| {
                d.borrow()
                    .iter()
                    .copied()
                    .filter(|id| intersects(w.scene.window_rect(*id), area))
                    .collect()
            });
            for id in ids {
                make_tree(w, id, &mut made);
                shown.push(id);
            }
        }
        Some(mut left) => {
            while left > 0 {
                let Some(id) = nearest(&w.scene, area) else {
                    break;
                };
                let n = size(&w.scene, id);
                if n <= left {
                    make_tree(w, id, &mut made);
                    left -= n;
                } else {
                    // Too big for this frame: the root now, its children
                    // wait as roots of their own.
                    DEFERRED.with(|d| d.borrow_mut().remove(&id));
                    if w.materialize(id) {
                        made.push(id);
                    }
                    let children = w.scene.get(id).map(|n| n.children.clone());
                    for c in children.unwrap_or_default() {
                        DEFERRED.with(|d| d.borrow_mut().insert(c));
                        if let Some(v) = w.views.get(&c) {
                            let _ = v.set_visible(false);
                        }
                    }
                    left -= 1;
                }
                shown.push(id);
            }
        }
    }
    w.relink(&made);
    for id in shown {
        w.update_visibility(id);
    }
}

/// Node `id` is needed now (focused, scrolled to, a popover's anchor): it
/// and its ancestors get their elements and stop waiting.
pub fn reveal(w: &mut Win, id: NodeId) {
    if !waiting() {
        return;
    }
    let mut chain: Vec<NodeId> = std::iter::once(id).chain(w.scene.ancestors(id)).collect();
    chain.reverse();
    let mut made = Vec::new();
    let mut shown = Vec::new();
    for i in chain {
        if deferred(i) {
            make_tree(w, i, &mut made);
            shown.push(i);
        } else if w.materialize(i) {
            made.push(i);
        }
    }
    w.relink(&made);
    for i in shown {
        w.update_visibility(i);
    }
}

/// Everything waiting is made and shown now (a smoke step looks for a
/// node).
pub fn show_all(w: &mut Win) {
    if !DEFERRED.with(|d| !d.borrow().is_empty()) {
        return;
    }
    let ids: Vec<NodeId> = DEFERRED.with(|d| d.borrow().iter().copied().collect());
    let mut made = Vec::new();
    for id in &ids {
        make_tree(w, *id, &mut made);
    }
    w.relink(&made);
    for id in ids {
        w.update_visibility(id);
    }
}

/// A scroll view moved: what it brings in view shows now.
pub fn scrolled(w: &mut Win) {
    if DEFERRED.with(|d| !d.borrow().is_empty()) {
        show(w, None);
    }
}

/// An open popover's anchor has its elements (the flyout is placed on
/// them).
pub fn anchors(w: &mut Win) {
    if !waiting() {
        return;
    }
    for o in w.scene.open_overlays() {
        if let Some(p) = w.scene.get(o).and_then(|n| n.parent) {
            reveal(w, p);
        }
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
        crate::imp::with_state(|s| show(&mut s.renderer.widgets, Some(PER_FRAME)));
        if DEFERRED.with(|d| d.borrow().is_empty())
            && let Some(t) = TICKING.take()
        {
            let _ = CompositionTarget::RemoveRendering(t);
            if perf::on() {
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
            crate::imp::defer(Box::new(|s| show_all(&mut s.renderer.widgets)));
        }
    }
}

impl Win {
    /// Node `id` has its elements now (a setter needs them), if it was
    /// lazy; it stays out of the window while its parent waits.
    pub fn ensure_view(&mut self, id: NodeId) {
        if self.materialize(id) {
            self.relink(&[id]);
        }
    }

    /// Makes lazy node `id`'s elements from what the scene holds; its
    /// children join them in [`Win::relink`]. Whether it was lazy.
    pub fn materialize(&mut self, id: NodeId) -> bool {
        let Some(set) = LAZY.with(|l| l.borrow_mut().remove(&id)) else {
            return false;
        };
        let Some(kind) = self.scene.get(id).map(|n| n.kind) else {
            return false;
        };
        {
            let _t = perf::span(Cost::Create);
            if !self.make_view(id, kind) {
                return false;
            }
        }
        let _t = perf::span(Cost::Style);
        let Some(n) = self.scene.get(id) else {
            return true;
        };
        let configured = !n.config.is_empty();
        let field_text = (n.is_text_field() && !n.text.is_empty()).then(|| n.text.clone());
        let picture = kind == NodeKind::Picture && !n.text.is_empty();
        if configured {
            self.config_view(id, kind);
        }
        if let Some(text) = field_text {
            self.set_field_text(id, &text);
        }
        if picture {
            self.show_picture(id);
        }
        let pal = self.palette_for(id);
        let (Some(n), Some(v)) = (self.scene.get(id), self.views.get_mut(&id)) else {
            return true;
        };
        let r = (|| -> windows_core::Result<()> {
            if !n.icon.is_empty() {
                controls::set_icon(&v.content, &n.icon)?;
            }
            if let Some(spec) = &n.menu {
                v.outer.SetContextFlyout(&crate::events::menu(spec)?)?;
            }
            if set & VALUE != 0 {
                controls::set_value(&v.content, n, n.value)?;
            }
            let sticky = self.scene.sticky_offset(n);
            {
                let _p = perf::span(Cost::Place);
                // A new size restyles (`View::place`).
                v.place(n, &pal, sticky)?;
            }
            if n.frame.w == 0. && n.frame.h == 0. {
                v.apply_style(n, &pal, sticky)?;
            }
            v.content_size(n)?;
            if set & POINTER != 0 {
                crate::pointer::wire_swipes(&v.outer(), id)?;
            }
            Ok(())
        })();
        if let Err(e) = r {
            log!("node {id} made late: {e}");
        }
        if kind == NodeKind::Shape {
            self.draw_shape(id);
        }
        // Clicks, hover, press: a clickable view's button and its peer.
        self.update_hit(id);
        if set & A11Y != 0
            && let (Some(n), Some(v)) = (self.scene.get(id), self.views.get(&id))
            && let Err(e) = crate::a11y::apply(v, id, &n.a11y, &n.text)
        {
            log!("accessibility of node {id}: {e}");
        }
        // A disclosure and its region, whichever was made last.
        crate::a11y::link_controls(&self.views, id);
        true
    }

    /// The elements of `made` join their parents, and their children (made
    /// or not) join them, in order.
    pub fn relink(&mut self, made: &[NodeId]) {
        if made.is_empty() {
            return;
        }
        let _t = perf::span(Cost::Children);
        let mut done = IdSet::default();
        for id in made {
            let parent = self.scene.get(*id).and_then(|n| n.parent);
            for p in std::iter::once(*id).chain(parent) {
                if !done.insert(p) {
                    continue;
                }
                // In the tree's order where `z_index` paints them in another
                // (`stacking.rs`).
                let children = crate::stacking::reading_order(p)
                    .or_else(|| self.scene.get(p).map(|n| n.children.clone()))
                    .unwrap_or_default();
                if !children.is_empty() {
                    self.link_children(p, &children);
                    crate::stacking::restack(self, p);
                }
            }
        }
    }
}
