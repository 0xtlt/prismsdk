//! Layout beyond flexbox: grids, per-axis gaps, baselines, aspect ratios,
//! horizontal scroll views, sticky and fixed nodes, and rules on a node's
//! own width. Kept apart from `lib.rs` so the flexbox core stays readable.

use std::collections::{HashMap, HashSet};

use prism_core::{
    Align, GridTrack, Length, NodeId, NodeKind, Position, Repetition, ScrollAxis, Style,
};
use taffy::prelude::{LengthPercentage, Size};
use taffy::style::{
    GridPlacement, GridTemplateComponent, GridTemplateRepetition, MaxTrackSizingFunction,
    MinTrackSizingFunction, RepetitionCount, TrackSizingFunction,
};

/// Where a sticky view sits in its scroll view, in the scroll content's
/// coordinates (view units: points × zoom). Backends move the view down by
/// [`offset`](Self::offset) as the content scrolls, and draw it above its
/// siblings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sticky {
    /// Its top edge in the content, unscrolled.
    pub top: f32,
    /// How far below the top of the visible area it stops.
    pub inset: f32,
    /// The largest offset: its parent's bottom edge pushes it away.
    pub max_offset: f32,
}

impl Sticky {
    /// How far to move the view down when the content coordinate `scroll`
    /// is at the top of the visible area.
    pub fn offset(&self, scroll: f32) -> f32 {
        (scroll + self.inset - self.top).clamp(0., self.max_offset.max(0.))
    }
}

/// Bookkeeping for the features in this module.
#[derive(Default)]
pub(crate) struct Extras {
    /// Children as the app declared them (before fixed nodes are hoisted).
    pub children: HashMap<NodeId, Vec<NodeId>>,
    pub parents: HashMap<NodeId, NodeId>,
    /// `.fixed()` nodes: shown as children of their screen (or the root).
    pub fixed: HashSet<NodeId>,
    /// Fixed nodes hoisted into each anchor, last time.
    pub hoisted: HashMap<NodeId, Vec<NodeId>>,
    /// Children lists to set again at the end of the batch.
    pub rehoist: bool,
    /// `.sticky()` nodes and what their backend was last told.
    pub sticky: HashMap<NodeId, Option<Sticky>>,
    /// Nodes with rules on their own width: the unresolved style and the
    /// number of thresholds the node's width passed last time.
    pub own_width: HashMap<NodeId, (Style, usize)>,
    /// Restyling from `own_width` (keeps the unresolved styles).
    pub in_width_pass: bool,
    /// Some node aligns on baselines: leaves report theirs.
    pub baselines: bool,
}

/// Containers that fixed nodes are positioned against.
pub(crate) fn is_anchor(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::Screen | NodeKind::Modal | NodeKind::Tab)
}

pub(crate) fn is_horizontal_scroll(kind: NodeKind, s: &Style) -> bool {
    kind == NodeKind::Scroll && s.scroll_axis == ScrollAxis::Horizontal
}

fn min_track(t: &GridTrack) -> MinTrackSizingFunction {
    match t {
        GridTrack::Px(v) => MinTrackSizingFunction::length(*v),
        GridTrack::Percent(f) => MinTrackSizingFunction::percent(*f),
        GridTrack::MinContent => MinTrackSizingFunction::min_content(),
        GridTrack::MaxContent => MinTrackSizingFunction::max_content(),
        // `fr` is not a valid minimum (CSS treats it as auto).
        _ => MinTrackSizingFunction::auto(),
    }
}

fn max_track(t: &GridTrack) -> MaxTrackSizingFunction {
    match t {
        GridTrack::Px(v) => MaxTrackSizingFunction::length(*v),
        GridTrack::Percent(f) => MaxTrackSizingFunction::percent(*f),
        GridTrack::Fr(f) => MaxTrackSizingFunction::fr(*f),
        GridTrack::MinContent => MaxTrackSizingFunction::min_content(),
        GridTrack::MaxContent => MaxTrackSizingFunction::max_content(),
        _ => MaxTrackSizingFunction::auto(),
    }
}

/// One track (`1fr` is `minmax(auto, 1fr)`, like CSS).
fn track(t: &GridTrack) -> TrackSizingFunction {
    match t {
        GridTrack::MinMax(min, max) => TrackSizingFunction {
            min: min_track(min),
            max: max_track(max),
        },
        GridTrack::Repeat(_, tracks) => tracks.first().map(track).unwrap_or(TrackSizingFunction {
            min: MinTrackSizingFunction::auto(),
            max: MaxTrackSizingFunction::auto(),
        }),
        other => TrackSizingFunction {
            min: min_track(other),
            max: max_track(other),
        },
    }
}

fn template<S: taffy::CheapCloneStr>(tracks: &[GridTrack]) -> Vec<GridTemplateComponent<S>> {
    tracks
        .iter()
        .map(|t| match t {
            GridTrack::Repeat(r, inner) => GridTemplateComponent::Repeat(GridTemplateRepetition {
                count: match r {
                    Repetition::Count(n) => RepetitionCount::Count(*n),
                    Repetition::AutoFill => RepetitionCount::AutoFill,
                    Repetition::AutoFit => RepetitionCount::AutoFit,
                },
                tracks: inner.iter().map(track).collect(),
                line_names: Vec::new(),
            }),
            other => GridTemplateComponent::Single(track(other)),
        })
        .collect()
}

fn placement<S: taffy::CheapCloneStr>(
    start: Option<i16>,
    span: u16,
) -> taffy::Line<GridPlacement<S>> {
    use taffy::style_helpers::{TaffyGridLine, TaffyGridSpan};
    let span = span.max(1);
    match start.filter(|s| *s != 0) {
        Some(line) => taffy::Line {
            start: GridPlacement::from_line_index(line),
            end: GridPlacement::from_span(span),
        },
        None if span > 1 => taffy::Line {
            start: GridPlacement::from_span(span),
            end: GridPlacement::Auto,
        },
        None => taffy::Line {
            start: GridPlacement::Auto,
            end: GridPlacement::Auto,
        },
    }
}

/// Grid, gaps, baseline, aspect ratio, sticky/fixed and scroll axis on top
/// of the flexbox mapping of `to_taffy`.
pub(crate) fn apply(t: &mut taffy::Style, s: &Style, kind: NodeKind) {
    if s.grid && !s.display_none {
        t.display = taffy::Display::Grid;
        t.grid_template_columns = template(&s.grid_columns);
        t.grid_template_rows = template(&s.grid_rows);
        if let Some(rows) = &s.grid_auto_rows {
            t.grid_auto_rows = vec![track(rows)];
        }
        // Like CSS, `align-items: stretch` makes every cell of a row as tall
        // as the row; `justify-items` stretches them across their columns.
        t.justify_items = Some(taffy::AlignItems::STRETCH);
    }
    t.grid_column = placement(s.col_start, s.col_span);
    t.grid_row = placement(s.row_start, s.row_span);
    if s.gap_x.is_some() || s.gap_y.is_some() {
        t.gap = Size {
            width: LengthPercentage::length(s.gap_x.unwrap_or(s.gap)),
            height: LengthPercentage::length(s.gap_y.unwrap_or(s.gap)),
        };
    }
    if s.align_items == Align::Baseline {
        t.align_items = Some(taffy::AlignItems::BASELINE);
    }
    if s.align_self == Some(Align::Baseline) {
        t.align_self = Some(taffy::AlignItems::BASELINE);
    }
    t.aspect_ratio = s.aspect_ratio;
    if let Some(r) = s.aspect_ratio {
        // A cap on one side caps the other (taffy sizes the parent with the
        // uncapped side otherwise, e.g. `w_full().max_w(360.)`).
        match (s.max_width, s.max_height) {
            (Length::Px(w), Length::Auto) => {
                t.max_size.height = taffy::LengthPercentageAuto::length(w / r)
            }
            (Length::Auto, Length::Px(h)) => {
                t.max_size.width = taffy::LengthPercentageAuto::length(h * r)
            }
            _ => {}
        }
    }
    match s.position {
        // Placed against its anchor (it is hoisted there), out of the flow.
        Position::Fixed => t.position = taffy::Position::Absolute,
        // In the flow; backends move it as the content scrolls.
        Position::Sticky => {
            t.position = taffy::Position::Relative;
            t.inset = taffy::Rect::auto();
        }
        _ => {}
    }
    if is_horizontal_scroll(kind, s) {
        t.overflow = taffy::Point {
            x: taffy::Overflow::Scroll,
            y: taffy::Overflow::Hidden,
        };
    }
}

/// The estimated distance from the top of a leaf to its first baseline
/// when the platform does not say: text sits at the top (ascent ≈ 0.95 em),
/// controls center one line of text.
pub(crate) fn estimated_baseline(kind: NodeKind, font_size: f32, height: f32) -> Option<f32> {
    let ascent = 0.95 * font_size;
    match kind {
        NodeKind::Text | NodeKind::Timer => Some(ascent),
        NodeKind::Button | NodeKind::TextInput | NodeKind::Picker | NodeKind::DatePicker => {
            Some((height - 1.2 * font_size) / 2. + ascent)
        }
        _ => None,
    }
}

/// The inset a sticky node stops at, in view units.
pub(crate) fn sticky_inset(s: &Style) -> f32 {
    match s.inset.top {
        Length::Px(v) => v,
        _ => 0.,
    }
}

impl<W: crate::Widgets> crate::NativeRenderer<W> {
    /// A style with rules on the node's own width, resolved at the width it
    /// had last time (narrowest first). `None`: nothing to resolve.
    pub(crate) fn own_width_style(&mut self, id: NodeId, style: &Style) -> Option<Style> {
        if self.extras.in_width_pass {
            return None;
        }
        if style.responsive.is_empty() {
            self.extras.own_width.remove(&id);
            return None;
        }
        let width = self
            .nodes
            .get(&id)
            .and_then(|e| e.frame)
            .map_or(0., |f| f.2 / self.zoom);
        let band = passed(&style.own_width_thresholds(), width);
        self.extras.own_width.insert(id, (style.clone(), band));
        Some(style.resolve_own_width(width))
    }

    /// Restyles nodes whose width crossed one of their own-width rules.
    /// Returns whether any did (the layout must run again).
    fn refresh_own_width(&mut self) -> bool {
        if self.extras.own_width.is_empty() {
            return false;
        }
        let mut patches = Vec::new();
        for (id, (base, band)) in self.extras.own_width.iter_mut() {
            let Some(e) = self.nodes.get(id) else {
                continue;
            };
            let Ok(l) = self.taffy.layout(e.layout) else {
                continue;
            };
            let width = l.size.width / self.zoom;
            let now = passed(&base.own_width_thresholds(), width);
            if now != *band {
                *band = now;
                patches.push(prism_core::Patch::SetStyle {
                    id: *id,
                    style: base.resolve_own_width(width),
                });
            }
        }
        if patches.is_empty() {
            return false;
        }
        self.extras.in_width_pass = true;
        prism_core::Backend::apply(self, &patches);
        self.extras.in_width_pass = false;
        true
    }

    /// Lays out, then again (at most twice) while own-width rules change.
    pub(crate) fn compute_with_rules(
        &mut self,
        node: taffy::NodeId,
        width: f32,
        height: f32,
    ) -> bool {
        for _ in 0..3 {
            if !self.compute(node, width, height) {
                return false;
            }
            if !self.refresh_own_width() {
                break;
            }
        }
        true
    }

    /// Notes what a new style asks of this module.
    pub(crate) fn note_style(&mut self, id: NodeId, style: &Style) {
        if style.align_items == Align::Baseline || style.align_self == Some(Align::Baseline) {
            self.extras.baselines = true;
        }
        let fixed = style.position == Position::Fixed && !style.display_none;
        if fixed != self.extras.fixed.contains(&id) {
            if fixed {
                self.extras.fixed.insert(id);
            } else {
                self.extras.fixed.remove(&id);
            }
            self.extras.rehoist = true;
        }
        if style.position == Position::Sticky {
            self.extras.sticky.entry(id).or_insert(None);
        } else if let Some(Some(_)) = self.extras.sticky.remove(&id)
            && let Some(e) = self.nodes.get(&id)
        {
            self.widgets.set_sticky(&e.view, None);
        }
    }

    /// A scroll view's axis may have changed: its children shrink again, or not.
    pub(crate) fn style_changed(&mut self, id: NodeId) {
        if self
            .nodes
            .get(&id)
            .is_some_and(|e| e.kind == NodeKind::Scroll)
        {
            for c in self.extras.children.get(&id).cloned().unwrap_or_default() {
                self.sync_layout_style(c);
            }
        }
    }

    pub(crate) fn children_changed(&mut self, id: NodeId, children: &[NodeId]) {
        let old = self.extras.children.insert(id, children.to_vec());
        for c in old.unwrap_or_default() {
            if self.extras.parents.get(&c) == Some(&id) {
                self.extras.parents.remove(&c);
            }
        }
        for c in children {
            self.extras.parents.insert(*c, id);
        }
        if self
            .nodes
            .get(&id)
            .is_some_and(|e| is_horizontal_scroll(e.kind, &e.style))
        {
            for c in children {
                self.sync_layout_style(*c);
            }
        }
        if !self.extras.fixed.is_empty() || !self.extras.hoisted.is_empty() {
            self.extras.rehoist = true;
        }
    }

    pub(crate) fn forget_extras(&mut self, id: NodeId) {
        self.extras.children.remove(&id);
        self.extras.parents.remove(&id);
        self.extras.own_width.remove(&id);
        self.extras.sticky.remove(&id);
        self.extras.hoisted.remove(&id);
        if self.extras.fixed.remove(&id) {
            self.extras.rehoist = true;
        }
    }

    /// Whether `id` is a direct child of a horizontal scroll view.
    pub(crate) fn in_horizontal_scroll(&self, id: NodeId) -> bool {
        self.extras
            .parents
            .get(&id)
            .and_then(|p| self.nodes.get(p))
            .is_some_and(|p| is_horizontal_scroll(p.kind, &p.style))
    }

    /// How wide a horizontal scroll view's content is (view units).
    pub(crate) fn content_width(&self, id: NodeId) -> Option<f32> {
        let e = self.nodes.get(&id)?;
        if !is_horizontal_scroll(e.kind, &e.style) {
            return None;
        }
        let frame = self.taffy.layout(e.layout).ok()?.size.width;
        let mut right: f32 = 0.;
        for child in self.taffy.children(e.layout).unwrap_or_default() {
            if let Ok(l) = self.taffy.layout(child) {
                right = right.max(l.location.x + l.size.width);
            }
        }
        Some((right + e.style.padding.right * self.zoom).max(frame))
    }

    /// The screen, sheet or tab (else the root) a fixed node is placed in.
    fn anchor_of(&self, id: NodeId) -> Option<NodeId> {
        let mut cur = *self.extras.parents.get(&id)?;
        loop {
            let e = self.nodes.get(&cur)?;
            if is_anchor(e.kind) {
                return Some(cur);
            }
            match self.extras.parents.get(&cur) {
                Some(p) => cur = *p,
                None => return (self.root == Some(cur)).then_some(cur),
            }
        }
    }

    /// Moves fixed nodes to their anchors (and back to their parents when
    /// they stop being fixed), in the views and in the layout.
    pub(crate) fn rehoist(&mut self) {
        self.extras.rehoist = false;
        let mut fixed: Vec<NodeId> = self.extras.fixed.iter().copied().collect();
        fixed.sort_unstable();
        let mut hoisted: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for f in fixed {
            if let Some(anchor) = self.anchor_of(f) {
                hoisted.entry(anchor).or_default().push(f);
            }
        }
        let old = std::mem::replace(&mut self.extras.hoisted, hoisted);
        let mut touched: HashSet<NodeId> = HashSet::new();
        for (anchor, nodes) in old.iter().chain(self.extras.hoisted.iter()) {
            touched.insert(*anchor);
            touched.extend(nodes.iter().filter_map(|n| self.extras.parents.get(n)));
        }
        // Fixed nodes that never were hoisted (no anchor yet) leave their parent too.
        touched.extend(
            self.extras
                .fixed
                .iter()
                .filter_map(|n| self.extras.parents.get(n)),
        );
        for id in touched {
            self.set_effective_children(id);
        }
    }

    /// Children of `id` minus its fixed ones, plus the fixed nodes hoisted
    /// into it (drawn last, above the rest).
    fn set_effective_children(&mut self, id: NodeId) {
        let Some(parent) = self.nodes.get(&id) else {
            return;
        };
        if crate::is_managed(parent.kind) {
            return;
        }
        let mut ids: Vec<NodeId> = self
            .extras
            .children
            .get(&id)
            .map(|c| {
                c.iter()
                    .copied()
                    .filter(|c| !self.extras.fixed.contains(c))
                    .collect()
            })
            .unwrap_or_default();
        ids.extend(self.extras.hoisted.get(&id).into_iter().flatten().copied());
        let kids: Vec<_> = ids.iter().filter_map(|c| self.nodes.get(c)).collect();
        let views: Vec<W::View> = kids.iter().map(|e| e.view.clone()).collect();
        let layouts: Vec<taffy::NodeId> = kids.iter().map(|e| e.layout).collect();
        let (view, layout) = (parent.view.clone(), parent.layout);
        self.widgets.set_children(&view, &views);
        let _ = self.taffy.set_children(layout, &layouts);
    }

    /// Fixed nodes in the root (no screen around them) stay inside the
    /// safe area, like those of a screen.
    pub(crate) fn shift_root_fixed(&mut self) {
        let Some(root) = self.root else {
            return;
        };
        let safe = self.insets;
        for f in self.extras.hoisted.get(&root).cloned().unwrap_or_default() {
            let Some(e) = self.nodes.get(&f) else {
                continue;
            };
            let inset = e.style.scaled(self.zoom).inset;
            let shift = |l: Length, by: f32| match l {
                Length::Px(v) => taffy::LengthPercentageAuto::length(v + by),
                Length::Fraction(f) => taffy::LengthPercentageAuto::percent(f),
                Length::Auto => taffy::LengthPercentageAuto::auto(),
            };
            if let Ok(mut st) = self.taffy.style(e.layout).cloned() {
                st.inset = taffy::Rect {
                    top: shift(inset.top, safe.top),
                    right: shift(inset.right, safe.right),
                    bottom: shift(inset.bottom, safe.bottom),
                    left: shift(inset.left, safe.left),
                };
                let _ = self.taffy.set_style(e.layout, st);
            }
        }
    }

    /// Tells backends where each sticky node sits in its scroll view.
    pub(crate) fn update_sticky(&mut self) {
        if self.extras.sticky.is_empty() {
            return;
        }
        let ids: Vec<NodeId> = self.extras.sticky.keys().copied().collect();
        for id in ids {
            let info = self.sticky_info(id);
            let known = self.extras.sticky.get(&id).copied().flatten();
            if known == info.map(|i| i.1) {
                continue;
            }
            self.extras.sticky.insert(id, info.map(|i| i.1));
            let Some(view) = self.nodes.get(&id).map(|e| e.view.clone()) else {
                continue;
            };
            let scroll = info.and_then(|(s, st)| Some((self.nodes.get(&s)?.view.clone(), st)));
            self.widgets
                .set_sticky(&view, scroll.as_ref().map(|(v, st)| (v, *st)));
        }
    }

    /// The vertical scroll view around a sticky node and its range there.
    fn sticky_info(&self, id: NodeId) -> Option<(NodeId, Sticky)> {
        let node = self.nodes.get(&id)?;
        if node.style.display_none {
            return None;
        }
        let own = *self.taffy.layout(node.layout).ok()?;
        let parent = *self.extras.parents.get(&id)?;
        let mut top = 0.;
        let mut cur = id;
        let scroll = loop {
            let e = self.nodes.get(&cur)?;
            top += self.taffy.layout(e.layout).ok()?.location.y;
            let p = *self.extras.parents.get(&cur)?;
            let pe = self.nodes.get(&p)?;
            if pe.kind == NodeKind::Scroll && pe.style.scroll_axis == ScrollAxis::Vertical {
                break p;
            }
            if is_anchor(pe.kind) || crate::is_managed(pe.kind) {
                return None;
            }
            cur = p;
        };
        let bottom = top + own.size.height;
        let limit = if parent == scroll {
            // Its parent is the content: it sticks until the end.
            let s = self.nodes.get(&scroll)?;
            let mut end: f32 = 0.;
            for child in self.taffy.children(s.layout).unwrap_or_default() {
                if let Ok(l) = self.taffy.layout(child) {
                    end = end.max(l.location.y + l.size.height);
                }
            }
            end + s.style.padding.bottom * self.zoom
        } else {
            let p = self.nodes.get(&parent)?;
            let pl = self.taffy.layout(p.layout).ok()?;
            let parent_top = top - own.location.y;
            parent_top + pl.size.height - p.style.padding.bottom * self.zoom
        };
        Some((
            scroll,
            Sticky {
                top,
                inset: sticky_inset(&node.style) * self.zoom,
                max_offset: (limit - bottom).max(0.),
            },
        ))
    }
}

/// How many of `thresholds` a `width` reaches.
fn passed(thresholds: &[f32], width: f32) -> usize {
    thresholds.iter().filter(|t| width >= **t).count()
}
