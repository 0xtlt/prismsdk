//! Drag and drop between elements (#251), shared by the native backends.
//!
//! A backend reports the drags of a [`draggable`](prism_core::Div::draggable)
//! view as it reports any drag (`UiEvent::Drag`), and passes them through
//! [`NativeRenderer::locate_drag`]: from where the pointer is in the window,
//! the platform's own hit-testing ([`Widgets::node_at`], which knows about
//! scrolling, clipping and what is drawn above what) gives the view under it,
//! and the renderer walks up Prism's tree to the innermost node that takes
//! the payload's kind. The event becomes `UiEvent::PayloadDrag` with the
//! pointer in the target's coordinates (its content's for a scroll view),
//! the target gets the `DropHover` state, the snapshot preview follows the
//! pointer, and a scroll view whose edge the pointer stays near scrolls
//! ([`NativeRenderer::autoscroll`], called on a timer while
//! [`NativeRenderer::autoscroll_due`]).

use std::collections::HashMap;

use prism_core::{DragEvent, DragPhase, DragPreview, DropPoint, NodeId, NodeKind, Point, UiEvent};

use crate::{NativeRenderer, Widgets};

/// How near a scroll view's edge (in points) the pointer makes it scroll.
pub const AUTOSCROLL_EDGE: f32 = 44.;
/// The fastest autoscroll step, in points per tick (at the very edge).
pub const AUTOSCROLL_STEP: f32 = 14.;
/// How often backends call [`NativeRenderer::autoscroll`] while it is due.
pub const AUTOSCROLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

/// What the renderer knows about drag and drop.
#[derive(Default)]
pub(crate) struct Dnd {
    /// What draggable nodes carry: the kind, and the preview.
    sources: HashMap<NodeId, (String, DragPreview)>,
    /// The kinds drop targets take.
    targets: HashMap<NodeId, Vec<String>>,
    /// The payload being dragged.
    active: Option<Active>,
}

/// A payload being dragged, in view units (points × zoom).
#[derive(Clone)]
struct Active {
    source: NodeId,
    kind: String,
    preview: DragPreview,
    /// Where the pointer went down, in the window.
    origin: (f32, f32),
    /// Where it went down in the source (the preview is held there).
    grab: (f32, f32),
    /// The last report, to report again after an autoscroll step.
    last: DragEvent,
    /// The target under the pointer.
    over: Option<NodeId>,
    /// The scroll view to scroll, and by how much per tick.
    autoscroll: Option<(NodeId, f32, f32)>,
}

impl Dnd {
    pub(crate) fn forget(&mut self, id: NodeId) {
        self.sources.remove(&id);
        self.targets.remove(&id);
    }
}

impl<W: Widgets> NativeRenderer<W> {
    /// `Patch::SetDragAndDrop`.
    pub(crate) fn set_drag_and_drop(
        &mut self,
        id: NodeId,
        kind: Option<&str>,
        preview: DragPreview,
        accepts: &[String],
    ) {
        match kind {
            Some(k) => self.dnd.sources.insert(id, (k.to_string(), preview)),
            None => self.dnd.sources.remove(&id),
        };
        if accepts.is_empty() {
            self.dnd.targets.remove(&id);
        } else {
            self.dnd.targets.insert(id, accepts.to_vec());
        }
        if let Some(e) = self.nodes.get(&id) {
            self.widgets
                .set_drag_and_drop(&e.view, kind, preview, accepts);
        }
    }

    /// A drag a backend reports, located (#251): a drag of a draggable node
    /// becomes `UiEvent::PayloadDrag` with the drop target under the
    /// pointer, which gets the `DropHover` state; the preview follows it.
    /// Other events come back as they are, and so does everything when the
    /// backend cannot locate views ([`Widgets::window_origin`] is `None`).
    /// The `bool` says the hover state changed sizes: lay out again.
    pub fn locate_drag(&mut self, event: UiEvent) -> (UiEvent, bool) {
        let UiEvent::Drag(id, e) = event else {
            return (event, false);
        };
        let Some((kind, preview)) = self.dnd.sources.get(&id).cloned() else {
            return (event, false);
        };
        let fresh =
            e.phase == DragPhase::Start || self.dnd.active.as_ref().is_none_or(|a| a.source != id);
        if fresh {
            let mut relayout = self.end_payload_drag();
            let Some(view) = self.nodes.get(&id).map(|n| n.view.clone()) else {
                return (event, relayout);
            };
            let Some((x, y)) = self.widgets.window_origin(&view) else {
                return (event, relayout);
            };
            let z = self.zoom;
            let grab = (e.start.x * z, e.start.y * z);
            self.dnd.active = Some(Active {
                source: id,
                kind,
                preview,
                origin: (x + grab.0, y + grab.1),
                grab,
                last: e,
                over: None,
                autoscroll: None,
            });
            let (event, more) = self.drag_at(id, e);
            relayout |= more;
            return (event, relayout);
        }
        self.drag_at(id, e)
    }

    /// Whether a dragged payload stays near a scroll view's edge: call
    /// [`autoscroll`](Self::autoscroll) every
    /// [`AUTOSCROLL_INTERVAL`] meanwhile.
    pub fn autoscroll_due(&self) -> bool {
        self.dnd
            .active
            .as_ref()
            .is_some_and(|a| a.autoscroll.is_some())
    }

    /// Scrolls the scroll view whose edge the dragged payload is near by a
    /// step and reports the drag again where it is now (the content moved
    /// under the pointer): dispatch the event, lay out again on `true`.
    /// `None` when nothing scrolled (the end of the content).
    pub fn autoscroll(&mut self) -> Option<(UiEvent, bool)> {
        let a = self.dnd.active.as_mut()?;
        let (node, dx, dy) = a.autoscroll?;
        let (source, last) = (a.source, a.last);
        let view = self.nodes.get(&node)?.view.clone();
        let before = self.widgets.scroll_offset(&view);
        self.widgets.scroll_to(&view, dx, dy, true, false);
        if self.widgets.scroll_offset(&view) == before {
            if let Some(a) = self.dnd.active.as_mut() {
                a.autoscroll = None;
            }
            return None;
        }
        Some(self.drag_at(
            source,
            DragEvent {
                phase: DragPhase::Move,
                ..last
            },
        ))
    }

    /// The drag `e` of the active payload (dragged from `id`), located.
    fn drag_at(&mut self, id: NodeId, e: DragEvent) -> (UiEvent, bool) {
        let Some(mut a) = self.dnd.active.clone().filter(|a| a.source == id) else {
            return (UiEvent::Drag(id, e), false);
        };
        let z = self.zoom;
        let p = (
            a.origin.0 + e.translation.x * z,
            a.origin.1 + e.translation.y * z,
        );
        a.last = e;
        let ending = matches!(e.phase, DragPhase::End | DragPhase::Cancel);
        let hit = self.widgets.node_at(p.0, p.1);
        let target = hit.and_then(|h| self.drop_target(h, a.source, &a.kind));
        let mut relayout = false;
        let hovered = if ending { None } else { target };
        if hovered != a.over {
            if let Some(old) = a.over {
                relayout |= self.drop_hover(old, false);
            }
            if let Some(new) = hovered {
                relayout |= self.drop_hover(new, true);
            }
            a.over = hovered;
        }
        if a.preview == DragPreview::Snapshot
            && let Some(view) = self.nodes.get(&a.source).map(|n| n.view.clone())
        {
            let at = (!ending).then_some((p.0 - a.grab.0, p.1 - a.grab.1));
            self.widgets.set_drag_preview(&view, at);
        }
        a.autoscroll = match hit {
            Some(h) if !ending => self.autoscroll_at(h, p),
            _ => None,
        };
        let over = target.and_then(|t| {
            let entry = self.nodes.get(&t)?;
            let view = entry.view.clone();
            let scrolls = entry.kind == NodeKind::Scroll;
            let (x, y) = self.widgets.window_origin(&view)?;
            let (sx, sy) = if scrolls {
                self.widgets.scroll_offset(&view)
            } else {
                (0., 0.)
            };
            Some(DropPoint {
                target: t,
                position: Point::new((p.0 - x + sx) / z, (p.1 - y + sy) / z),
            })
        });
        let source = a.source;
        self.dnd.active = (!ending).then_some(a);
        (UiEvent::PayloadDrag(source, e, over), relayout)
    }

    /// Ends the payload drag in progress, if any (a new one started):
    /// clears the hover and the preview.
    fn end_payload_drag(&mut self) -> bool {
        let Some(a) = self.dnd.active.take() else {
            return false;
        };
        if let Some(view) = self.nodes.get(&a.source).map(|n| n.view.clone()) {
            self.widgets.set_drag_preview(&view, None);
        }
        a.over.is_some_and(|old| self.drop_hover(old, false))
    }

    /// The innermost node around `hit` that takes `kind`, leaving out
    /// `source` and what is inside it.
    fn drop_target(&self, hit: NodeId, source: NodeId, kind: &str) -> Option<NodeId> {
        let mut chain = Vec::new();
        let mut node = Some(hit);
        while let Some(n) = node {
            chain.push(n);
            node = self.parent_of(n);
        }
        let from = chain.iter().position(|n| *n == source).map_or(0, |i| i + 1);
        chain[from..].iter().copied().find(|n| {
            self.dnd
                .targets
                .get(n)
                .is_some_and(|kinds| kinds.iter().any(|k| k == kind))
        })
    }

    /// The innermost scroll view around `hit` whose edge the window point
    /// `p` is near, along its axis, and the step to scroll it by.
    fn autoscroll_at(&mut self, hit: NodeId, p: (f32, f32)) -> Option<(NodeId, f32, f32)> {
        let z = self.zoom;
        let (edge, step) = (AUTOSCROLL_EDGE * z, AUTOSCROLL_STEP * z);
        // Faster the nearer the edge: a step at it, none `edge` away.
        let speed = |from_start: f32, length: f32| -> f32 {
            let edge = edge.min(length / 3.);
            if edge <= 0. {
                0.
            } else if from_start < edge {
                -step * (1. - from_start.max(0.) / edge)
            } else if from_start > length - edge {
                step * (1. - (length - from_start).max(0.) / edge)
            } else {
                0.
            }
        };
        let mut node = Some(hit);
        while let Some(n) = node {
            node = self.parent_of(n);
            let Some(entry) = self.nodes.get(&n) else {
                continue;
            };
            if entry.kind != NodeKind::Scroll {
                continue;
            }
            let Some((_, _, w, h)) = entry.frame else {
                continue;
            };
            let horizontal = entry.style.scroll_axis == prism_core::ScrollAxis::Horizontal;
            let view = entry.view.clone();
            let Some((x, y)) = self.widgets.window_origin(&view) else {
                continue;
            };
            let (dx, dy) = if horizontal {
                (speed(p.0 - x, w), 0.)
            } else {
                (0., speed(p.1 - y, h))
            };
            if dx != 0. || dy != 0. {
                return Some((n, dx, dy));
            }
        }
        None
    }

    /// Node `id` takes the dragged payload (or not anymore).
    fn drop_hover(&mut self, id: NodeId, on: bool) -> bool {
        if let Some(e) = self.nodes.get(&id) {
            let view = e.view.clone();
            self.widgets.set_drop_hover(&view, on);
        }
        self.set_interaction(id, prism_core::Interaction::DropHover, on)
    }
}
