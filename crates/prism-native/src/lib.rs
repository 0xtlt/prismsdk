//! # prism-native
//!
//! Everything native backends have in common. A platform only implements the
//! small [`Widgets`] trait (create a view, set its text, measure it, place it);
//! [`NativeRenderer`] turns reconciler patches into those calls and runs the
//! flexbox layout with taffy, so AppKit, UIKit (and later Android, Win32, GTK)
//! all lay out exactly like the web backend's CSS flexbox.

use std::collections::HashMap;

use prism_core::{Align, Backend, FlexDirection, Justify, Length, NodeId, NodeKind, Patch, Style};
use taffy::prelude::{
    AvailableSpace, Dimension, LengthPercentage, LengthPercentageAuto, Rect, Size, TaffyTree,
};

/// The platform-specific half of a native backend.
pub trait Widgets {
    /// A cheap, clonable handle to a native view.
    type View: Clone;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> Self::View;
    fn set_style(&mut self, view: &Self::View, style: &Style);
    fn set_text(&mut self, view: &Self::View, text: &str);
    fn set_placeholder(&mut self, view: &Self::View, text: &str);
    fn set_clickable(&mut self, view: &Self::View, clickable: bool);
    /// Make `children` the subviews of `parent`, in order.
    fn set_children(&mut self, parent: &Self::View, children: &[Self::View]);
    /// Mount `view` as the root, replacing the previous one.
    fn set_root(&mut self, view: &Self::View);
    fn destroy(&mut self, view: &Self::View);
    /// Natural size of a leaf (text, button, input) given an optional width limit.
    fn measure(&mut self, view: &Self::View, max_width: Option<f32>) -> (f32, f32);
    /// Place a view in its parent's coordinate space (top-left origin).
    fn set_frame(&mut self, view: &Self::View, x: f32, y: f32, w: f32, h: f32);
    /// Where `stack` shows `screen` (in the coordinates of the screen's
    /// superview) and what its content must avoid (navigation bar, safe area).
    /// Called after the stack got its frame.
    fn screen_viewport(&mut self, stack: &Self::View, screen: &Self::View) -> Viewport;
}

/// The area a navigation stack gives to one of its screens.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub insets: Insets,
}

/// Safe-area or window insets applied around the root.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

struct Entry<V> {
    view: V,
    kind: NodeKind,
    style: Style,
    layout: taffy::NodeId,
}

/// Applies patches to native widgets and lays them out.
pub struct NativeRenderer<W: Widgets> {
    pub widgets: W,
    taffy: TaffyTree<W::View>,
    nodes: HashMap<NodeId, Entry<W::View>>,
    by_layout: HashMap<taffy::NodeId, NodeId>,
    viewport: taffy::NodeId,
    root: Option<NodeId>,
    insets: Insets,
    /// Screens of each stack. Screens are separate layout roots: their size
    /// comes from the platform's navigation container, not from flexbox.
    stack_screens: HashMap<NodeId, Vec<NodeId>>,
}

impl<W: Widgets> NativeRenderer<W> {
    pub fn new(widgets: W) -> Self {
        let mut taffy = TaffyTree::new();
        let viewport = taffy
            .new_leaf(taffy::Style::default())
            .expect("taffy viewport");
        NativeRenderer {
            widgets,
            taffy,
            nodes: HashMap::new(),
            by_layout: HashMap::new(),
            viewport,
            root: None,
            insets: Insets::default(),
            stack_screens: HashMap::new(),
        }
    }

    /// Lays the tree out in a `width` × `height` viewport and moves every view.
    ///
    /// `insets` (e.g. the iOS safe area) are added to the root's padding, so the
    /// root's background still fills the screen edge to edge.
    pub fn layout(&mut self, width: f32, height: f32, insets: Insets) {
        if insets != self.insets {
            self.insets = insets;
            if let Some(root) = self.root {
                self.sync_layout_style(root);
            }
        }
        let viewport_style = taffy::Style {
            size: Size {
                width: Dimension::length(width),
                height: Dimension::length(height),
            },
            flex_direction: taffy::FlexDirection::Column,
            ..Default::default()
        };
        let _ = self.taffy.set_style(self.viewport, viewport_style);
        if self.compute(self.viewport, width, height)
            && let Some(root) = self.root
        {
            self.place(root);
        }
    }

    /// Runs flexbox on the subtree at `node`, measuring leaves natively.
    fn compute(&mut self, node: taffy::NodeId, width: f32, height: f32) -> bool {
        let widgets = &mut self.widgets;
        let result = self.taffy.compute_layout_with_measure(
            node,
            Size {
                width: AvailableSpace::Definite(width),
                height: AvailableSpace::Definite(height),
            },
            |input, _node, view, style| {
                // Leaf layout (style size, min/max, padding) is taffy's; we only
                // provide the content size of native leaves.
                taffy::compute_leaf_layout(
                    input,
                    style,
                    |_, _| 0.0,
                    |known, available| {
                        let Some(view) = view else {
                            return Size::ZERO;
                        };
                        let max_width = known.width.or(match available.width {
                            AvailableSpace::Definite(w) => Some(w),
                            _ => None,
                        });
                        let (w, h) = widgets.measure(view, max_width);
                        Size {
                            width: known.width.unwrap_or(w),
                            height: known.height.unwrap_or(h),
                        }
                    },
                )
            },
        );
        match result {
            Ok(()) => true,
            Err(err) => {
                eprintln!("prism: layout failed: {err}");
                false
            }
        }
    }

    /// Lays out one screen of a stack in the area the platform gives it.
    fn layout_screen(&mut self, stack: NodeId, screen: NodeId) {
        let (Some(st), Some(sc)) = (self.nodes.get(&stack), self.nodes.get(&screen)) else {
            return;
        };
        let vp = self.widgets.screen_viewport(&st.view, &sc.view);
        let mut style = to_taffy(&sc.style, NodeKind::View);
        let (p, i) = (sc.style.padding, vp.insets);
        style.padding = Rect {
            top: LengthPercentage::length(p.top + i.top),
            right: LengthPercentage::length(p.right + i.right),
            bottom: LengthPercentage::length(p.bottom + i.bottom),
            left: LengthPercentage::length(p.left + i.left),
        };
        style.size = Size {
            width: Dimension::length(vp.width),
            height: Dimension::length(vp.height),
        };
        let (layout, view) = (sc.layout, sc.view.clone());
        let _ = self.taffy.set_style(layout, style);
        if self.compute(layout, vp.width, vp.height) {
            self.widgets
                .set_frame(&view, vp.x, vp.y, vp.width, vp.height);
            self.place_children(screen);
        }
    }

    fn sync_layout_style(&mut self, id: NodeId) {
        let Some(e) = self.nodes.get(&id) else { return };
        let mut style = to_taffy(&e.style, e.kind);
        if self.root == Some(id) {
            // Like `#prism-root > *` on the web: the root fills the viewport.
            style.flex_grow = 1.;
        }
        // A root stack handles the safe area itself (navigation bar, screens).
        if self.root == Some(id) && e.kind != NodeKind::Stack {
            let (p, i) = (e.style.padding, self.insets);
            style.padding = Rect {
                top: LengthPercentage::length(p.top + i.top),
                right: LengthPercentage::length(p.right + i.right),
                bottom: LengthPercentage::length(p.bottom + i.bottom),
                left: LengthPercentage::length(p.left + i.left),
            };
        }
        let _ = self.taffy.set_style(e.layout, style);
    }

    fn place(&mut self, id: NodeId) {
        let Some(entry) = self.nodes.get(&id) else {
            return;
        };
        let Ok(l) = self.taffy.layout(entry.layout) else {
            return;
        };
        self.widgets.set_frame(
            &entry.view,
            l.location.x,
            l.location.y,
            l.size.width,
            l.size.height,
        );
        match self.stack_screens.get(&id).cloned() {
            Some(screens) => {
                for screen in screens {
                    self.layout_screen(id, screen);
                }
            }
            None => self.place_children(id),
        }
    }

    fn place_children(&mut self, id: NodeId) {
        let Some(entry) = self.nodes.get(&id) else {
            return;
        };
        let children = self.taffy.children(entry.layout).unwrap_or_default();
        let ids: Vec<NodeId> = children
            .iter()
            .filter_map(|c| self.by_layout.get(c).copied())
            .collect();
        for child in ids {
            self.place(child);
        }
    }
}

impl<W: Widgets> Backend for NativeRenderer<W> {
    fn apply(&mut self, patches: &[Patch]) {
        for patch in patches {
            match patch {
                Patch::Create { id, kind } => {
                    let view = self.widgets.create(*id, *kind);
                    let container =
                        matches!(kind, NodeKind::View | NodeKind::Stack | NodeKind::Screen);
                    let layout = if container {
                        self.taffy.new_leaf(taffy::Style::default())
                    } else {
                        self.taffy
                            .new_leaf_with_context(taffy::Style::default(), view.clone())
                    }
                    .expect("taffy node");
                    self.by_layout.insert(layout, *id);
                    self.nodes.insert(
                        *id,
                        Entry {
                            view,
                            kind: *kind,
                            style: Style::default(),
                            layout,
                        },
                    );
                }
                Patch::SetStyle { id, style } => {
                    if let Some(e) = self.nodes.get_mut(id) {
                        self.widgets.set_style(&e.view, style);
                        e.style = style.clone();
                    }
                    self.sync_layout_style(*id);
                }
                Patch::SetText { id, text } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_text(&e.view, text);
                        let _ = self.taffy.mark_dirty(e.layout);
                    }
                }
                Patch::SetPlaceholder { id, text } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_placeholder(&e.view, text);
                        let _ = self.taffy.mark_dirty(e.layout);
                    }
                }
                Patch::SetClickable { id, clickable } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_clickable(&e.view, *clickable);
                    }
                }
                Patch::SetChildren { id, children } => {
                    let Some(parent) = self.nodes.get(id) else {
                        continue;
                    };
                    let kids: Vec<&Entry<W::View>> =
                        children.iter().filter_map(|c| self.nodes.get(c)).collect();
                    let views: Vec<W::View> = kids.iter().map(|e| e.view.clone()).collect();
                    let layouts: Vec<taffy::NodeId> = kids.iter().map(|e| e.layout).collect();
                    self.widgets.set_children(&parent.view, &views);
                    if parent.kind == NodeKind::Stack {
                        let _ = self.taffy.set_children(parent.layout, &[]);
                        self.stack_screens.insert(*id, children.clone());
                    } else {
                        let _ = self.taffy.set_children(parent.layout, &layouts);
                    }
                }
                Patch::SetRoot { id } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_root(&e.view);
                        let _ = self.taffy.set_children(self.viewport, &[e.layout]);
                        let previous = self.root.replace(*id);
                        if let Some(previous) = previous {
                            self.sync_layout_style(previous);
                        }
                        self.sync_layout_style(*id);
                    }
                }
                Patch::Destroy { id } => {
                    if let Some(e) = self.nodes.remove(id) {
                        self.widgets.destroy(&e.view);
                        self.by_layout.remove(&e.layout);
                        self.stack_screens.remove(id);
                        let _ = self.taffy.remove(e.layout);
                    }
                }
            }
        }
    }
}

fn dim(l: Length) -> Dimension {
    match l {
        Length::Auto => Dimension::auto(),
        Length::Px(v) => Dimension::length(v),
        Length::Fraction(f) => Dimension::percent(f),
    }
}

fn lpa(l: Length) -> LengthPercentageAuto {
    match l {
        Length::Auto => LengthPercentageAuto::auto(),
        Length::Px(v) => LengthPercentageAuto::length(v),
        Length::Fraction(f) => LengthPercentageAuto::percent(f),
    }
}

fn align(a: Align) -> taffy::AlignItems {
    match a {
        Align::Start => taffy::AlignItems::FLEX_START,
        Align::Center => taffy::AlignItems::CENTER,
        Align::End => taffy::AlignItems::FLEX_END,
        Align::Stretch => taffy::AlignItems::STRETCH,
    }
}

/// Maps a Prism style to taffy. Kept in sync with `prism-web`'s CSS output.
pub fn to_taffy(s: &Style, kind: NodeKind) -> taffy::Style {
    let edges = |e: prism_core::Edges| Rect {
        top: LengthPercentage::length(e.top),
        right: LengthPercentage::length(e.right),
        bottom: LengthPercentage::length(e.bottom),
        left: LengthPercentage::length(e.left),
    };
    let margin = Rect {
        top: LengthPercentageAuto::length(s.margin.top),
        right: LengthPercentageAuto::length(s.margin.right),
        bottom: LengthPercentageAuto::length(s.margin.bottom),
        left: LengthPercentageAuto::length(s.margin.left),
    };
    let container = matches!(kind, NodeKind::View | NodeKind::Stack | NodeKind::Screen);
    taffy::Style {
        display: taffy::Display::Flex,
        flex_direction: match s.direction {
            FlexDirection::Row => taffy::FlexDirection::Row,
            FlexDirection::Column => taffy::FlexDirection::Column,
        },
        flex_wrap: if s.wrap {
            taffy::FlexWrap::Wrap
        } else {
            taffy::FlexWrap::NoWrap
        },
        align_items: Some(align(s.align_items)),
        align_self: s.align_self.map(align),
        justify_content: Some(match s.justify_content {
            Justify::Start => taffy::JustifyContent::FLEX_START,
            Justify::Center => taffy::JustifyContent::CENTER,
            Justify::End => taffy::JustifyContent::FLEX_END,
            Justify::SpaceBetween => taffy::JustifyContent::SPACE_BETWEEN,
            Justify::SpaceAround => taffy::JustifyContent::SPACE_AROUND,
            Justify::SpaceEvenly => taffy::JustifyContent::SPACE_EVENLY,
        }),
        gap: Size {
            width: LengthPercentage::length(s.gap),
            height: LengthPercentage::length(s.gap),
        },
        // Leaves (text, buttons, inputs) draw their own insets natively.
        padding: if container {
            edges(s.padding)
        } else {
            Rect::zero()
        },
        border: Rect::zero(),
        margin,
        size: Size {
            width: dim(s.width),
            height: dim(s.height),
        },
        min_size: Size {
            width: lpa(s.min_width),
            height: lpa(s.min_height),
        },
        max_size: Size {
            width: lpa(s.max_width),
            height: lpa(s.max_height),
        },
        flex_grow: s.flex_grow,
        flex_shrink: s.flex_shrink,
        flex_basis: dim(s.flex_basis),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::{AppInit, IntoElement, ParentElement, Render, Router, Runtime, screen, text};
    use std::sync::Arc;

    /// Records frames; screens get the stack's area minus a 44pt header.
    #[derive(Default)]
    struct Fake {
        frames: HashMap<NodeId, (f32, f32, f32, f32)>,
    }

    impl Widgets for Fake {
        type View = NodeId;
        fn create(&mut self, id: NodeId, _: NodeKind) -> NodeId {
            id
        }
        fn set_style(&mut self, _: &NodeId, _: &Style) {}
        fn set_text(&mut self, _: &NodeId, _: &str) {}
        fn set_placeholder(&mut self, _: &NodeId, _: &str) {}
        fn set_clickable(&mut self, _: &NodeId, _: bool) {}
        fn set_children(&mut self, _: &NodeId, _: &[NodeId]) {}
        fn set_root(&mut self, _: &NodeId) {}
        fn destroy(&mut self, _: &NodeId) {}
        fn measure(&mut self, _: &NodeId, _: Option<f32>) -> (f32, f32) {
            (50., 20.)
        }
        fn set_frame(&mut self, v: &NodeId, x: f32, y: f32, w: f32, h: f32) {
            self.frames.insert(*v, (x, y, w, h));
        }
        fn screen_viewport(&mut self, stack: &NodeId, _: &NodeId) -> Viewport {
            let (_, _, w, h) = self.frames[stack];
            Viewport {
                x: 0.,
                y: 44.,
                width: w,
                height: h - 44.,
                insets: Insets::default(),
            }
        }
    }

    struct Page;
    impl Render for Page {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            screen("Page").child(text("hello"))
        }
    }

    #[test]
    fn root_stack_fills_viewport_and_screens_get_their_area() {
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = Router::new()
                .route("/", |cx, _| cx.new_entity(|_| Page))
                .build(cx);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 800., Insets::default());
        let f = &renderer.widgets.frames;
        assert_eq!(f[&1], (0., 0., 400., 800.), "stack");
        assert_eq!(f[&2], (0., 44., 400., 756.), "screen");
        assert_eq!(f[&3], (0., 0., 400., 20.), "text in screen");
    }
}
