//! # prism-native
//!
//! Everything native backends have in common. A platform only implements the
//! small [`Widgets`] trait (create a view, set its text, measure it, place it);
//! [`NativeRenderer`] turns reconciler patches into those calls and runs the
//! flexbox layout with taffy, so AppKit, UIKit (and later Android, Win32, GTK)
//! all lay out exactly like the web backend's CSS flexbox.

use std::collections::{HashMap, HashSet};

#[cfg(target_vendor = "apple")]
pub mod apple;
pub mod images;
pub mod remote;

use prism_core::{
    Accessibility, Align, Appearance, Backend, Direction, FlexDirection, Justify, Length, NodeId,
    NodeKind, Patch, Pose, RenderEnvironment, Style, TextAlign, Transition,
    animation::{DEFAULT_ENTER, DEFAULT_EXIT},
};
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
    /// Narrowest width a leaf can take (CSS min-content: the longest word of
    /// a text). Default: its natural single-line width.
    fn measure_min_content(&mut self, view: &Self::View) -> f32 {
        self.measure(view, None).0
    }
    /// Place a view in its parent's coordinate space (top-left origin).
    fn set_frame(&mut self, view: &Self::View, x: f32, y: f32, w: f32, h: f32);
    /// For platform-managed containers (stacks, tab bars): where `container`
    /// shows `child` (in the coordinates of the child's superview) and what its
    /// content must avoid (navigation bar, tab bar, safe area). Called after
    /// the container got its frame.
    fn screen_viewport(&mut self, container: &Self::View, child: &Self::View) -> Viewport;
    /// Apply the resolved accessibility description (role, label, state…).
    /// Required: every backend must expose its views to assistive technologies.
    fn set_accessibility(&mut self, view: &Self::View, a11y: &Accessibility);
    /// Toggle state (0/1) or selected tab index.
    fn set_value(&mut self, view: &Self::View, value: f64);
    /// What a shape draws, in points for its current size (arcs already
    /// turned into curves; colors may adapt to dark mode). Called when the
    /// drawing or the shape's size changes.
    fn set_drawing(&mut self, _view: &Self::View, _paths: &[prism_core::Path]) {}
    /// Control configuration (see `prism_core::controls`), before the value:
    /// `SliderConfig` (slider), `PickerConfig` (picker), `DateConfig` (date
    /// picker), `InputConfig` (text input, text area). Decode it with the
    /// matching `decode`.
    fn set_config(&mut self, _view: &Self::View, _kind: NodeKind, _config: &str) {}
    /// Tab icon (SF Symbol name on Apple platforms).
    fn set_icon(&mut self, _view: &Self::View, _icon: &str) {}
    /// Context menu of a view (`None` removes it). Items report
    /// `UiEvent::MenuSelect(id)`.
    fn set_menu(&mut self, _view: &Self::View, _menu: Option<&prism_core::menu::MenuSpec>) {}
    /// Which horizontal swipes a view reports (`UiEvent::Swipe`).
    fn set_swipe(&mut self, _view: &Self::View, _left: bool, _right: bool) {}
    /// Whether a scroll view reports `UiEvent::Scroll`.
    fn set_scroll_events(&mut self, _view: &Self::View, _on: bool) {}
    /// Light/dark native chrome.
    fn set_appearance(&mut self, _appearance: Appearance) {}
    /// `display: none` (also hides it from assistive technologies).
    fn set_hidden(&mut self, _view: &Self::View, _hidden: bool) {}
    /// Scrollable extent of a scroll view's content.
    fn set_content_size(&mut self, _view: &Self::View, _width: f32, _height: f32) {}
    /// Like `set_frame`, animated (only called when the frame changes).
    fn set_frame_animated(
        &mut self,
        view: &Self::View,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        _transition: Transition,
    ) {
        self.set_frame(view, x, y, w, h);
    }
    /// Like `set_style`, animating opacity and colors.
    fn set_style_animated(&mut self, view: &Self::View, style: &Style, _transition: Transition) {
        self.set_style(view, style);
    }
    /// Language and layout direction of native chrome (navigation bars…).
    fn set_environment(&mut self, _env: &RenderEnvironment) {}
    /// `view` is about to leave `parent` (and be destroyed): show it animating
    /// to `to` (its style at the exit pose), e.g. with a snapshot. Default:
    /// it disappears at once.
    fn animate_exit(
        &mut self,
        _parent: &Self::View,
        _view: &Self::View,
        _to: &Style,
        _t: Transition,
    ) {
    }
    /// Loops `view` between `style` (its opacity and transform) and `pose`,
    /// back and forth, until called with `None`.
    fn set_repeat(
        &mut self,
        _view: &Self::View,
        _style: &Style,
        _repeat: Option<(Pose, Transition)>,
    ) {
    }
    /// The interaction states `view` has style refinements for (called when
    /// that set changes): report them with
    /// [`NativeRenderer::set_interaction`] (pointer over it, pressed,
    /// keyboard focus; focus-within is derived). Backends that resolve
    /// states themselves (GTK CSS) read `style.states` instead.
    fn set_interactions(&mut self, _view: &Self::View, _states: prism_core::Interactions) {}
}

/// `style` at `pose` (its opacity and transform replaced).
pub fn posed(style: &Style, pose: Pose) -> Style {
    let (opacity, transform) = pose.apply(style.opacity, style.transform);
    Style {
        opacity,
        transform,
        ..style.clone()
    }
}

/// What [`prism_core::Color::current`] resolves to on a node without a text
/// color: black, white in dark mode (the platforms' label colors).
pub fn label_color() -> prism_core::Color {
    prism_core::Color::adaptive(prism_core::Color::BLACK, prism_core::Color::WHITE)
}

/// The outline of a box `w` × `h` at `(x, y)` (top-left origin, y down),
/// rounding `corners` (leading = left) by `radius` (clamped to half the
/// smaller side), clockwise from the top-left: quarter circles as cubics.
/// Backends draw per-corner backgrounds, borders, clips and shadows with it.
pub fn box_path(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    corners: prism_core::Corners,
) -> Vec<prism_core::Segment> {
    use prism_core::{Corners as C, Segment as S};
    let max = (w.min(h) / 2.).max(0.);
    let r = |c: C| {
        if corners.contains(c) {
            radius.clamp(0., max)
        } else {
            0.
        }
    };
    let (tl, tr, br, bl) = (
        r(C::TOP_LEADING),
        r(C::TOP_TRAILING),
        r(C::BOTTOM_TRAILING),
        r(C::BOTTOM_LEADING),
    );
    let k = 0.5523;
    let (x1, y1) = (x + w, y + h);
    let mut p = vec![S::Move(x + tl, y), S::Line(x1 - tr, y)];
    if tr > 0. {
        p.push(S::Cubic(
            x1 - tr + k * tr,
            y,
            x1,
            y + tr - k * tr,
            x1,
            y + tr,
        ));
    }
    p.push(S::Line(x1, y1 - br));
    if br > 0. {
        p.push(S::Cubic(
            x1,
            y1 - br + k * br,
            x1 - br + k * br,
            y1,
            x1 - br,
            y1,
        ));
    }
    p.push(S::Line(x + bl, y1));
    if bl > 0. {
        p.push(S::Cubic(
            x + bl - k * bl,
            y1,
            x,
            y1 - bl + k * bl,
            x,
            y1 - bl,
        ));
    }
    p.push(S::Line(x, y + tl));
    if tl > 0. {
        p.push(S::Cubic(x, y + tl - k * tl, x + tl - k * tl, y, x + tl, y));
    }
    p.push(S::Close);
    p
}

/// Containers whose children are positioned by the platform, not by flexbox.
fn is_managed(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::Stack | NodeKind::Tabs)
}

fn is_container(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::View
            | NodeKind::Stack
            | NodeKind::Screen
            | NodeKind::Scroll
            | NodeKind::Tabs
            | NodeKind::Tab
            | NodeKind::Modal
    )
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
    /// Last frame given to the platform (animations only run on changes).
    frame: Option<(f32, f32, f32, f32)>,
    /// What a shape draws (resolved again when its size changes).
    drawing: Option<prism_core::Drawing>,
    /// Interaction states: the style as rendered (`style` is it in the
    /// current states), the states the node is in, and those it refines.
    states: EntryStates,
}

#[derive(Default)]
struct EntryStates {
    declared: Style,
    current: prism_core::Interactions,
    used: Option<prism_core::Interactions>,
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
    env: RenderEnvironment,
    /// Every length is multiplied by this (TVs: bigger than phone sizes).
    zoom: f32,
    /// The node with the keyboard focus (its ancestors are focus-within).
    focused: Option<NodeId>,
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
            env: RenderEnvironment {
                text_scale: 1.,
                ..Default::default()
            },
            zoom: 1.,
            focused: None,
        }
    }

    /// Zooms the whole UI: styles are in Prism points, views get
    /// `points × zoom`. The app sees a viewport of `window / zoom` (see
    /// [`zoom`](Self::zoom)).
    pub fn set_zoom(&mut self, zoom: f32) {
        if zoom == self.zoom || zoom <= 0. {
            return;
        }
        self.zoom = zoom;
        let ids: Vec<NodeId> = self.nodes.keys().copied().collect();
        for id in ids {
            let e = &self.nodes[&id];
            let s = self.effective_style(&e.style, e.kind);
            self.widgets.set_style(&e.view, &s);
            self.sync_layout_style(id);
        }
        self.mark_all_dirty();
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// The style handed to the platform: Dynamic Type scaling and
    /// right-to-left text alignment applied, [`prism_core::Color::current`]
    /// resolved; the same for each interaction state's style.
    fn effective_style(&self, style: &Style, kind: NodeKind) -> Style {
        let mut s = self.effective_one(style, kind);
        if !style.states.is_empty() {
            s.states = style.states.map_resolved(|st| self.effective_one(st, kind));
        }
        s
    }

    fn effective_one(&self, style: &Style, kind: NodeKind) -> Style {
        let mut s = style.scaled(self.zoom).with_current_color(label_color());
        if self.env.direction == Direction::Rtl {
            // Leading/trailing borders, like margins.
            s.border_sides = s.border_sides.mirrored();
        }
        if kind != NodeKind::View && self.env.text_scale != 1. {
            s.font_size = (s.font_size * self.env.text_scale).round();
        }
        if self.env.direction == Direction::Rtl {
            s.text_align = match s.text_align {
                TextAlign::Left => TextAlign::Right,
                TextAlign::Right => TextAlign::Left,
                TextAlign::Center => TextAlign::Center,
            };
        }
        if self.env.direction == Direction::Rtl {
            // Offsets are leading/trailing, like margins.
            s.transform.x = -s.transform.x;
            // So are corners: backends get leading = left.
            s.rounded_corners = s.rounded_corners.mirrored();
            s.animations = s.animations.scaled(1., true);
        }
        if self.env.reduce_motion {
            s.transition = None;
            s.animations = Default::default();
        }
        s
    }

    /// Places a view, animating if its style asks for it. The first time, an
    /// element with an enter pose starts there and animates to its style.
    fn set_frame(&mut self, id: NodeId, x: f32, y: f32, w: f32, h: f32) {
        let Some(e) = self.nodes.get_mut(&id) else {
            return;
        };
        let frame = (x, y, w, h);
        let previous = e.frame.replace(frame);
        // Unchanged: no native call (big trees relayout on every change).
        if previous == Some(frame) {
            return;
        }
        let transition = e.style.transition.filter(|_| !self.env.reduce_motion);
        let enter = e
            .style
            .animations
            .enter
            .filter(|_| previous.is_none() && !self.env.reduce_motion && !e.style.display_none);
        if let Some(pose) = enter {
            let (view, kind, style) = (e.view.clone(), e.kind, e.style.clone());
            self.play_enter(&view, kind, &style, pose);
            self.widgets.set_frame(&view, x, y, w, h);
            self.redraw(id);
            return;
        }
        let e = &self.nodes[&id];
        match (transition, previous) {
            (Some(t), Some(p)) if p != frame && p.2 > 0. && p.3 > 0. => {
                self.widgets.set_frame_animated(&e.view, x, y, w, h, t)
            }
            _ => self.widgets.set_frame(&e.view, x, y, w, h),
        }
        if previous.map(|p| (p.2, p.3)) != Some((w, h)) {
            self.redraw(id);
        }
    }

    fn is_shape(&self, id: NodeId) -> bool {
        self.nodes
            .get(&id)
            .is_some_and(|e| e.kind == NodeKind::Shape)
    }

    /// Hands a shape its drawing at its current size.
    fn redraw(&mut self, id: NodeId) {
        let Some(e) = self.nodes.get(&id) else {
            return;
        };
        let (Some(d), Some((_, _, w, h))) = (&e.drawing, e.frame) else {
            return;
        };
        let mut paths = d.resolve(w, h, self.zoom);
        if self.env.direction == Direction::Rtl {
            // Like the layout around it: charts read right to left.
            for p in &mut paths {
                p.mirror_x(w);
            }
        }
        self.widgets.set_drawing(&e.view, &paths);
    }

    /// Shows `view` at its enter pose and animates it to its style.
    fn play_enter(&mut self, view: &W::View, kind: NodeKind, style: &Style, pose: Pose) {
        let s = self.effective_style(style, kind);
        let pose = pose.scaled(self.zoom, self.env.direction == Direction::Rtl);
        self.widgets.set_style(view, &posed(&s, pose));
        let t = s.transition.unwrap_or(DEFAULT_ENTER);
        self.widgets.set_style_animated(view, &s, t);
    }

    /// Children of `parent` removed by `children` and destroyed in this
    /// batch, with an exit pose: they animate out before going away.
    fn play_exits(&mut self, parent: NodeId, children: &[NodeId], destroyed: &HashSet<NodeId>) {
        if self.env.reduce_motion {
            return;
        }
        let Some(p) = self.nodes.get(&parent) else {
            return;
        };
        if is_managed(p.kind) {
            return;
        }
        let old: Vec<NodeId> = self
            .taffy
            .children(p.layout)
            .unwrap_or_default()
            .iter()
            .filter_map(|c| self.by_layout.get(c).copied())
            .collect();
        let parent_view = p.view.clone();
        for id in old {
            if children.contains(&id) || !destroyed.contains(&id) {
                continue;
            }
            let Some(e) = self.nodes.get(&id) else {
                continue;
            };
            let Some(pose) = e.style.animations.exit else {
                continue;
            };
            if e.frame.is_none() || e.style.display_none {
                continue;
            }
            let s = self.effective_style(&e.style, e.kind);
            let pose = pose.scaled(self.zoom, self.env.direction == Direction::Rtl);
            let t = s
                .transition
                .map_or(DEFAULT_EXIT, |t| Transition { delay: 0., ..t });
            let view = e.view.clone();
            self.widgets
                .animate_exit(&parent_view, &view, &posed(&s, pose), t);
        }
    }

    /// The current layout environment.
    pub fn environment(&self) -> &RenderEnvironment {
        &self.env
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
            self.place(root, width);
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
                            AvailableSpace::MinContent => Some(widgets.measure_min_content(view)),
                            AvailableSpace::MaxContent => None,
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
        let zoomed = sc.style.scaled(self.zoom);
        let mut style = to_taffy(&zoomed, NodeKind::View);
        let (p, mut i) = (zoomed.padding, vp.insets);
        // A screen that starts with a scroll view lets it run under the bars,
        // like native apps: the platform insets its content (and collapses
        // large titles) itself.
        let first_child = self
            .taffy
            .children(sc.layout)
            .ok()
            .and_then(|c| c.first().copied())
            .and_then(|c| self.by_layout.get(&c))
            .and_then(|c| self.nodes.get(c));
        if first_child.is_some_and(|c| c.kind == NodeKind::Scroll) {
            i.top = 0.;
            i.bottom = 0.;
        }
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
        // Absolutely positioned children (toasts, floating buttons) are placed
        // against the screen's edges, not its padding: keep them inside the
        // safe area so bars and the keyboard don't cover them.
        let safe = vp.insets;
        let floating: Vec<(taffy::NodeId, prism_core::Inset)> = self
            .taffy
            .children(sc.layout)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| {
                let n = self.by_layout.get(&c).and_then(|id| self.nodes.get(id))?;
                (n.style.position == prism_core::Position::Absolute)
                    .then(|| (c, n.style.scaled(self.zoom).inset))
            })
            .collect();
        let shift = |l: Length, by: f32| match l {
            Length::Px(v) => LengthPercentageAuto::length(v + by),
            other => lpa(other),
        };
        for (child, inset) in floating {
            if let Ok(mut st) = self.taffy.style(child).cloned() {
                st.inset = Rect {
                    top: shift(inset.top, safe.top),
                    right: shift(inset.right, safe.right),
                    bottom: shift(inset.bottom, safe.bottom),
                    left: shift(inset.left, safe.left),
                };
                let _ = self.taffy.set_style(child, st);
            }
        }
        let layout = sc.layout;
        let _ = self.taffy.set_style(layout, style);
        if self.compute(layout, vp.width, vp.height) {
            self.set_frame(screen, vp.x, vp.y, vp.width, vp.height);
            self.place_children(screen);
        }
    }

    /// Forces leaves to be measured again (e.g. an image finished loading).
    pub fn mark_all_dirty(&mut self) {
        for e in self.nodes.values() {
            let _ = self.taffy.mark_dirty(e.layout);
        }
    }

    fn sync_layout_style(&mut self, id: NodeId) {
        let Some(e) = self.nodes.get(&id) else { return };
        let zoomed = e.style.scaled(self.zoom);
        let mut style = to_taffy(&zoomed, e.kind);
        if self.root == Some(id) {
            // Like `#prism-root > *` on the web: the root fills the viewport.
            style.flex_grow = 1.;
        }
        // A root stack handles the safe area itself (navigation bar, screens).
        if self.root == Some(id) && !is_managed(e.kind) {
            let (p, i) = (zoomed.padding, self.insets);
            style.padding = Rect {
                top: LengthPercentage::length(p.top + i.top),
                right: LengthPercentage::length(p.right + i.right),
                bottom: LengthPercentage::length(p.bottom + i.bottom),
                left: LengthPercentage::length(p.left + i.left),
            };
        }
        let _ = self.taffy.set_style(e.layout, style);
    }

    /// Places `id` inside a parent `parent_width` wide (mirrored in RTL).
    fn place(&mut self, id: NodeId, parent_width: f32) {
        let Some(entry) = self.nodes.get(&id) else {
            return;
        };
        let Ok(l) = self.taffy.layout(entry.layout) else {
            return;
        };
        let (mut x, y, w, h) = (l.location.x, l.location.y, l.size.width, l.size.height);
        if self.env.direction == Direction::Rtl {
            x = parent_width - x - w;
        }
        self.set_frame(id, x, y, w, h);
        match self.stack_screens.get(&id).cloned() {
            Some(screens) => {
                for screen in screens {
                    self.layout_screen(id, screen);
                }
            }
            None => {
                self.place_children(id);
                self.update_content_size(id);
            }
        }
    }

    /// Tells a scroll view how far its content extends.
    fn update_content_size(&mut self, id: NodeId) {
        let Some(entry) = self.nodes.get(&id) else {
            return;
        };
        if entry.kind != NodeKind::Scroll {
            return;
        }
        let Ok(frame) = self.taffy.layout(entry.layout).map(|l| l.size) else {
            return;
        };
        let mut bottom: f32 = 0.;
        for child in self.taffy.children(entry.layout).unwrap_or_default() {
            if let Ok(l) = self.taffy.layout(child) {
                bottom = bottom.max(l.location.y + l.size.height);
            }
        }
        let height = (bottom + entry.style.padding.bottom).max(frame.height);
        self.widgets
            .set_content_size(&entry.view, frame.width, height);
    }

    fn place_children(&mut self, id: NodeId) {
        let Some(entry) = self.nodes.get(&id) else {
            return;
        };
        let width = self.taffy.layout(entry.layout).map_or(0., |l| l.size.width);
        let children = self.taffy.children(entry.layout).unwrap_or_default();
        let ids: Vec<NodeId> = children
            .iter()
            .filter_map(|c| self.by_layout.get(c).copied())
            .collect();
        for child in ids {
            self.place(child, width);
        }
    }
}

// Interaction states (`.hover(|s| …)`…): the platform reports them, the
// node is restyled in its new states.
impl<W: Widgets> NativeRenderer<W> {
    /// Stores the style `id` was given and returns it in the node's current
    /// interaction states; tells the platform which states to report.
    fn declare(&mut self, id: NodeId, style: &Style) -> Style {
        let Some(e) = self.nodes.get_mut(&id) else {
            return style.clone();
        };
        e.states.declared = style.clone();
        let mut used = style.states.used();
        if style.group {
            // A group reports its states for its descendants' refinements.
            use prism_core::Interaction as I;
            for i in [I::Hover, I::Active, I::FocusWithin] {
                used = used.with(i, true);
            }
        }
        if e.states.used != Some(used) && (e.states.used.is_some() || !used.is_empty()) {
            e.states.used = Some(used);
            let view = e.view.clone();
            self.widgets.set_interactions(&view, used);
        }
        let e = &self.nodes[&id];
        style.interacted(e.states.current)
    }

    /// The platform saw `id` enter or leave `state` (pointer over it,
    /// pressed, keyboard focus): restyles it, and its ancestors for
    /// focus-within. Returns whether a style changed (the backend lays out
    /// again: a refinement may change sizes).
    pub fn set_interaction(
        &mut self,
        id: NodeId,
        state: prism_core::Interaction,
        on: bool,
    ) -> bool {
        use prism_core::Interaction;
        if state != Interaction::Focus {
            return self.set_state_bit(id, state, on);
        }
        let mut changed = false;
        if on {
            if let Some(previous) = self.focused.replace(id)
                && previous != id
            {
                changed |= self.set_state_bit(previous, Interaction::Focus, false);
                changed |= self.set_focus_within(previous, false);
            }
        } else if self.focused == Some(id) {
            self.focused = None;
        }
        changed |= self.set_state_bit(id, Interaction::Focus, on);
        changed |= self.set_focus_within(id, on);
        changed
    }

    /// The interaction states `id` is in.
    pub fn interactions(&self, id: NodeId) -> prism_core::Interactions {
        self.nodes
            .get(&id)
            .map_or_else(Default::default, |e| e.states.current)
    }

    fn set_focus_within(&mut self, id: NodeId, on: bool) -> bool {
        let mut changed = false;
        let mut node = Some(id);
        while let Some(n) = node {
            changed |= self.set_state_bit(n, prism_core::Interaction::FocusWithin, on);
            node = self.parent_of(n);
        }
        changed
    }

    fn parent_of(&self, id: NodeId) -> Option<NodeId> {
        let e = self.nodes.get(&id)?;
        match self.taffy.parent(e.layout) {
            Some(p) => self.by_layout.get(&p).copied(),
            // Screens are laid out on their own: their stack is the parent.
            None => self
                .stack_screens
                .iter()
                .find(|(_, screens)| screens.contains(&id))
                .map(|(stack, _)| *stack),
        }
    }

    fn set_state_bit(&mut self, id: NodeId, state: prism_core::Interaction, on: bool) -> bool {
        let Some(e) = self.nodes.get_mut(&id) else {
            return false;
        };
        let current = e.states.current.with(state, on);
        if current == e.states.current {
            return false;
        }
        e.states.current = current;
        let mut changed = false;
        // A group's hover, press and focus-within reach its descendants.
        if e.states.declared.group
            && let Some(group_state) = state.for_group()
        {
            for d in self.descendants(id) {
                changed |= self.set_state_bit(d, group_state, on);
            }
        }
        let Some(e) = self.nodes.get_mut(&id) else {
            return changed;
        };
        if !e.states.declared.states.used().contains(state) {
            return changed;
        }
        let style = e.states.declared.interacted(current);
        if style == e.style {
            return changed;
        }
        let was_hidden = e.style.display_none;
        e.style = style;
        let e = &self.nodes[&id];
        let s = self.effective_style(&e.style, e.kind);
        let view = e.view.clone();
        if was_hidden != s.display_none {
            self.widgets.set_hidden(&view, s.display_none);
        }
        match s.transition {
            Some(t) if e.frame.is_some() => self.widgets.set_style_animated(&view, &s, t),
            _ => self.widgets.set_style(&view, &s),
        }
        self.sync_layout_style(id);
        true
    }

    /// Every node under `id` (screens of stacks included).
    fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut todo = vec![id];
        while let Some(n) = todo.pop() {
            let Some(e) = self.nodes.get(&n) else {
                continue;
            };
            let mut kids: Vec<NodeId> = self
                .taffy
                .children(e.layout)
                .unwrap_or_default()
                .iter()
                .filter_map(|c| self.by_layout.get(c).copied())
                .collect();
            if let Some(screens) = self.stack_screens.get(&n) {
                kids.extend(screens.iter().copied());
            }
            out.extend(kids.iter().copied());
            todo.extend(kids);
        }
        out
    }
}

impl<W: Widgets> Backend for NativeRenderer<W> {
    fn apply(&mut self, patches: &[Patch]) {
        let destroyed: HashSet<NodeId> = patches
            .iter()
            .filter_map(|p| match p {
                Patch::Destroy { id } => Some(*id),
                _ => None,
            })
            .collect();
        for patch in patches {
            match patch {
                Patch::Create { id, kind } => {
                    let view = self.widgets.create(*id, *kind);
                    // Shapes have no content size: only their style sizes them.
                    let container = is_container(*kind) || *kind == NodeKind::Shape;
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
                            frame: None,
                            drawing: None,
                            states: EntryStates::default(),
                        },
                    );
                }
                Patch::SetStyle { id, style } => {
                    // The style in the node's current interaction states.
                    let style = &self.declare(*id, style);
                    if let Some(e) = self.nodes.get(id) {
                        let s = self.effective_style(style, e.kind);
                        let was_hidden = e.style.display_none;
                        if e.style.display_none != s.display_none || s.display_none {
                            self.widgets.set_hidden(&e.view, s.display_none);
                        }
                        let (view, kind) = (e.view.clone(), e.kind);
                        let enter = s
                            .animations
                            .enter
                            .filter(|_| was_hidden && !s.display_none && e.frame.is_some());
                        let old = self.effective_style(&e.style, e.kind);
                        match (enter, s.transition) {
                            // Shown again: enters like a new element.
                            (Some(pose), _) => self.play_enter(&view, kind, style, pose),
                            (None, Some(t)) if e.frame.is_some() => {
                                self.widgets.set_style_animated(&view, &s, t)
                            }
                            _ => self.widgets.set_style(&view, &s),
                        }
                        // A loop restarts when it, or what it loops from, changes.
                        if old.animations.repeat != s.animations.repeat
                            || (s.animations.repeat.is_some()
                                && (old.opacity != s.opacity || old.transform != s.transform))
                        {
                            self.widgets.set_repeat(&view, &s, s.animations.repeat);
                        }
                    }
                    if let Some(e) = self.nodes.get_mut(id) {
                        e.style = style.clone();
                    }
                    self.sync_layout_style(*id);
                }
                Patch::SetEnvironment { env } => {
                    let changed_text = env.text_scale != self.env.text_scale
                        || env.direction != self.env.direction;
                    self.env = env.clone();
                    self.widgets.set_environment(env);
                    if changed_text {
                        // Restyle every node at the new scale/direction.
                        let ids: Vec<NodeId> = self.nodes.keys().copied().collect();
                        for id in ids {
                            let e = &self.nodes[&id];
                            let s = self.effective_style(&e.style, e.kind);
                            self.widgets.set_style(&e.view, &s);
                            if e.kind == NodeKind::Shape {
                                self.redraw(id);
                            }
                        }
                        self.mark_all_dirty();
                    }
                }
                Patch::SetText { id, text } if self.is_shape(*id) => {
                    let drawing = prism_core::Drawing::decode(text);
                    if let Some(e) = self.nodes.get_mut(id) {
                        e.drawing = Some(drawing);
                    }
                    self.redraw(*id);
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
                    self.play_exits(*id, children, &destroyed);
                    let Some(parent) = self.nodes.get(id) else {
                        continue;
                    };
                    let kids: Vec<&Entry<W::View>> =
                        children.iter().filter_map(|c| self.nodes.get(c)).collect();
                    let views: Vec<W::View> = kids.iter().map(|e| e.view.clone()).collect();
                    let layouts: Vec<taffy::NodeId> = kids.iter().map(|e| e.layout).collect();
                    self.widgets.set_children(&parent.view, &views);
                    if is_managed(parent.kind) {
                        let _ = self.taffy.set_children(parent.layout, &[]);
                        self.stack_screens.insert(*id, children.clone());
                    } else {
                        let _ = self.taffy.set_children(parent.layout, &layouts);
                    }
                }
                Patch::SetAccessibility { id, a11y } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_accessibility(&e.view, a11y);
                    }
                }
                Patch::SetAppearance { appearance } => self.widgets.set_appearance(*appearance),
                Patch::SetSwipe { id, left, right } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_swipe(&e.view, *left, *right);
                    }
                }
                Patch::SetScrollEvents { id, on } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_scroll_events(&e.view, *on);
                    }
                }
                Patch::SetMenu { id, menu } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_menu(&e.view, menu.as_ref());
                    }
                }
                Patch::SetValue { id, value } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_value(&e.view, *value);
                    }
                }
                Patch::SetConfig { id, config } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_config(&e.view, e.kind, config);
                        let _ = self.taffy.mark_dirty(e.layout);
                    }
                }
                Patch::SetIcon { id, icon } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_icon(&e.view, icon);
                        let _ = self.taffy.mark_dirty(e.layout);
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
                    if self.focused == Some(*id) {
                        self.focused = None;
                        self.set_focus_within(*id, false);
                    }
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
    let container = is_container(kind);
    taffy::Style {
        display: if s.display_none {
            taffy::Display::None
        } else {
            taffy::Display::Flex
        },
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
        position: match s.position {
            prism_core::Position::Relative => taffy::Position::Relative,
            prism_core::Position::Absolute => taffy::Position::Absolute,
        },
        inset: Rect {
            top: lpa(s.inset.top),
            right: lpa(s.inset.right),
            bottom: lpa(s.inset.bottom),
            left: lpa(s.inset.left),
        },
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
        // A scroll view's content may be taller than the view itself.
        overflow: if kind == NodeKind::Scroll {
            taffy::Point {
                x: taffy::Overflow::Hidden,
                y: taffy::Overflow::Scroll,
            }
        } else {
            taffy::Point {
                x: taffy::Overflow::Visible,
                y: taffy::Overflow::Visible,
            }
        },
        scrollbar_width: 0.,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::{
        AppInit, IntoElement, ParentElement, Render, Router, Runtime, Styled, div, screen, scroll,
        text,
    };
    use std::sync::Arc;

    /// Records frames; screens get the stack's area minus a 44pt header.
    #[derive(Default)]
    struct Fake {
        frames: HashMap<NodeId, (f32, f32, f32, f32)>,
        content: HashMap<NodeId, (f32, f32)>,
        drawings: HashMap<NodeId, Vec<prism_core::Path>>,
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
        fn set_accessibility(&mut self, _: &NodeId, _: &Accessibility) {}
        fn set_value(&mut self, _: &NodeId, _: f64) {}
        fn set_content_size(&mut self, v: &NodeId, w: f32, h: f32) {
            self.content.insert(*v, (w, h));
        }
        fn set_drawing(&mut self, v: &NodeId, paths: &[prism_core::Path]) {
            self.drawings.insert(*v, paths.to_vec());
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

    struct Long;
    impl Render for Long {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            div().child(
                scroll()
                    .p(10.)
                    .children((0..50).map(|i| text(format!("row {i}")))),
            )
        }
    }

    #[test]
    fn scroll_views_report_their_content_height() {
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = cx.new_entity(|_| Long);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 300., Insets::default());
        let w = &renderer.widgets;
        assert_eq!(
            w.frames[&2].3, 300.,
            "scroll view keeps the viewport height"
        );
        // 50 rows × 20pt + 10pt padding top and bottom.
        assert_eq!(w.content[&2], (400., 1020.));
    }

    struct Row;
    impl Render for Row {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            div().flex_row().child(text("a")).child(text("b"))
        }
    }

    #[test]
    fn right_to_left_mirrors_rows_and_text_scale_restyles() {
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = cx.new_entity(|_| Row);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 300., Insets::default());
        assert_eq!(
            renderer.widgets.frames[&2].0, 0.,
            "LTR: first item on the left"
        );

        rt.app.set_locale(Some(prism_core::Locale::new("ar")));
        rt.app.set_environment(prism_core::Environment {
            text_scale: 1.5,
            ..Default::default()
        });
        renderer.apply(&rt.render());
        assert_eq!(renderer.environment().direction, prism_core::Direction::Rtl);
        assert_eq!(renderer.environment().text_scale, 1.5);
        renderer.layout(400., 300., Insets::default());
        let f = &renderer.widgets.frames;
        assert_eq!(f[&2].0, 350., "RTL: first item on the right");
        assert_eq!(f[&3].0, 300.);
    }

    struct Overlay;
    impl Render for Overlay {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            div()
                .flex_1()
                .child(text("in flow"))
                .child(div().absolute().left(10.).bottom(20.).w(100.).h(30.))
        }
    }

    #[test]
    fn absolute_nodes_sit_at_their_insets_mirrored_right_to_left() {
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = cx.new_entity(|_| Overlay);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 300., Insets::default());
        assert_eq!(renderer.widgets.frames[&3], (10., 250., 100., 30.));
        assert_eq!(renderer.widgets.frames[&2].1, 0., "out of the flow");
        rt.app.set_locale(Some(prism_core::Locale::new("ar")));
        renderer.apply(&rt.render());
        renderer.layout(400., 300., Insets::default());
        assert_eq!(renderer.widgets.frames[&3], (290., 250., 100., 30.));
    }

    struct Chart;
    impl Render for Chart {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            div().child(
                prism_core::shape()
                    .path(prism_core::Path::line([(0., 1.), (1., 0.)]))
                    .w_full()
                    .h(100.),
            )
        }
    }

    #[test]
    fn shapes_are_drawn_at_their_size_and_mirrored_right_to_left() {
        use prism_core::Segment;
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = cx.new_entity(|_| Chart);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 300., Insets::default());
        let first = |r: &NativeRenderer<Fake>| r.widgets.drawings[&2][0].segments.clone();
        assert_eq!(
            first(&renderer),
            [Segment::Move(0., 100.), Segment::Line(400., 0.)]
        );
        // Resized: drawn again at the new width.
        renderer.layout(200., 300., Insets::default());
        assert_eq!(first(&renderer)[1], Segment::Line(200., 0.));
        rt.app.set_locale(Some(prism_core::Locale::new("ar")));
        renderer.apply(&rt.render());
        renderer.layout(200., 300., Insets::default());
        assert_eq!(
            first(&renderer),
            [Segment::Move(200., 100.), Segment::Line(0., 0.)]
        );
    }

    #[test]
    fn tab_contents_are_laid_out_in_their_viewport() {
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = Router::new()
                .tab("A", "a", "/")
                .tab("B", "b", "/b")
                .route("/", |cx, _| cx.new_entity(|_| Page))
                .route("/b", |cx, _| cx.new_entity(|_| Page))
                .build(cx);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 800., Insets::default());
        let f = &renderer.widgets.frames;
        assert_eq!(f[&1], (0., 0., 400., 800.), "tabs");
        // tab (viewport below a fake 44pt bar) → stack fills it → screen.
        assert_eq!(f[&2], (0., 44., 400., 756.), "tab");
        assert_eq!(f[&3], (0., 0., 400., 756.), "stack inside tab");
    }
}

/// `cargo test -p prism-native --release -- --ignored --nocapture bench`
#[cfg(test)]
mod bench {
    use super::*;
    use prism_core::{
        App, AppInit, Context, Entity, IntoElement, ParentElement, Render, Runtime, Styled, div,
        scroll, text,
    };
    use std::sync::Arc;
    use std::time::Instant;

    #[derive(Default)]
    struct Null {
        frames: usize,
    }
    impl Widgets for Null {
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
        fn measure(&mut self, _: &NodeId, w: Option<f32>) -> (f32, f32) {
            (w.unwrap_or(200.).min(200.), 20.)
        }
        fn set_frame(&mut self, _: &NodeId, _: f32, _: f32, _: f32, _: f32) {
            self.frames += 1;
        }
        fn set_accessibility(&mut self, _: &NodeId, _: &Accessibility) {}
        fn set_value(&mut self, _: &NodeId, _: f64) {}
        fn screen_viewport(&mut self, _: &NodeId, _: &NodeId) -> Viewport {
            Viewport::default()
        }
    }

    struct Row {
        n: usize,
        hits: u32,
    }
    impl Render for Row {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex_row()
                .gap(8.)
                .p(8.)
                .child(text(format!("Row {}", self.n)))
                .child(text(format!("{} hits", self.hits)))
        }
    }
    struct List {
        rows: Vec<Entity<Row>>,
    }
    impl Render for List {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            scroll().p(16.).gap(4.).children(self.rows.iter().copied())
        }
    }

    fn run(n: usize) {
        let rows: Arc<std::sync::Mutex<Vec<Entity<Row>>>> = Default::default();
        let out = rows.clone();
        let init: AppInit = Arc::new(move |cx: &mut App| {
            let list = cx.new_entity(|cx: &mut Context<List>| List {
                rows: (0..n).map(|n| cx.new(|_| Row { n, hits: 0 })).collect(),
            });
            *out.lock().unwrap() = list.read(cx).rows.clone();
            cx.set_root(list);
        });
        let t = Instant::now();
        let mut rt = Runtime::new(&init);
        let patches = rt.render();
        let render = t.elapsed();
        let mut r = NativeRenderer::new(Null::default());
        let t = Instant::now();
        r.apply(&patches);
        let apply = t.elapsed();
        let t = Instant::now();
        r.layout(400., 800., Insets::default());
        let layout = t.elapsed();
        // One row changes.
        let row = rows.lock().unwrap()[n / 2];
        let t = Instant::now();
        row.update(&mut rt.app, |r, cx| {
            r.hits += 1;
            cx.notify();
        });
        let patches = rt.poll();
        r.apply(&patches);
        let t_update = t.elapsed();
        let before = r.widgets.frames;
        let t = Instant::now();
        r.layout(400., 800., Insets::default());
        let relayout = t.elapsed();
        let moved = r.widgets.frames - before;
        println!(
            "{n:>5} rows ({} nodes): render {render:?} · apply {apply:?} · layout {layout:?} | 1 row changed: render+apply {t_update:?} ({} patches) · relayout {relayout:?} ({moved} native frame updates)",
            r.nodes.len(),
            patches.len()
        );
    }

    #[test]
    #[ignore]
    fn bench() {
        for n in [100, 1000, 5000] {
            run(n);
        }
    }
}
