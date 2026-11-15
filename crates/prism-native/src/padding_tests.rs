//! Padding on leaves (#14): taffy adds it around the measured content, the
//! backend insets what it draws (or the view is placed inside it).

use std::collections::HashMap;

use prism_core::{
    Accessibility, Backend, Direction, Edges, NodeId, NodeKind, Patch, RenderEnvironment, Style,
};

use crate::{Insets, NativeRenderer, Viewport, Widgets};

/// Text is 10 points per character on one 20-point line, wrapping at the
/// width it is given; controls are 30 × 20.
#[derive(Default)]
struct Fake {
    texts: HashMap<NodeId, String>,
    styles: HashMap<NodeId, Style>,
    frames: HashMap<NodeId, (f32, f32, f32, f32)>,
    widths: Vec<Option<f32>>,
}

impl Widgets for Fake {
    type View = NodeId;
    fn create(&mut self, id: NodeId, _: NodeKind) -> NodeId {
        id
    }
    fn set_style(&mut self, v: &NodeId, s: &Style) {
        self.styles.insert(*v, s.clone());
    }
    fn set_text(&mut self, v: &NodeId, t: &str) {
        self.texts.insert(*v, t.into());
    }
    fn set_placeholder(&mut self, _: &NodeId, _: &str) {}
    fn set_clickable(&mut self, _: &NodeId, _: bool) {}
    fn set_children(&mut self, _: &NodeId, _: &[NodeId]) {}
    fn set_root(&mut self, _: &NodeId) {}
    fn destroy(&mut self, _: &NodeId) {}
    fn measure(&mut self, v: &NodeId, max_width: Option<f32>) -> (f32, f32) {
        self.widths.push(max_width);
        // Controls (no text): 30 points.
        let natural = match self.texts.get(v) {
            Some(t) if !t.is_empty() => t.len() as f32 * 10.,
            _ => 30.,
        };
        match max_width {
            Some(w) if w < natural => (w, 20. * (natural / w).ceil()),
            _ => (natural, 20.),
        }
    }
    fn measure_min_content(&mut self, _: &NodeId) -> f32 {
        10.
    }
    fn set_frame(&mut self, v: &NodeId, x: f32, y: f32, w: f32, h: f32) {
        self.frames.insert(*v, (x, y, w, h));
    }
    fn set_accessibility(&mut self, _: &NodeId, _: &Accessibility) {}
    fn set_value(&mut self, _: &NodeId, _: f64) {}
    fn screen_viewport(&mut self, _: &NodeId, _: &NodeId) -> Viewport {
        Viewport::default()
    }
}

/// A root column (items at the start) holding `leaf` (id 2).
fn render(kind: NodeKind, text: &str, leaf: Style, rtl: bool) -> NativeRenderer<Fake> {
    let mut r = NativeRenderer::new(Fake::default());
    let mut patches = Vec::new();
    if rtl {
        patches.push(Patch::SetEnvironment {
            env: RenderEnvironment {
                direction: Direction::Rtl,
                text_scale: 1.,
                ..Default::default()
            },
        });
    }
    let root = Style {
        align_items: prism_core::Align::Start,
        ..Default::default()
    };
    patches.extend([
        Patch::Create {
            id: 1,
            kind: NodeKind::View,
        },
        Patch::Create { id: 2, kind },
        Patch::SetText {
            id: 2,
            text: text.into(),
        },
        Patch::SetStyle { id: 1, style: root },
        Patch::SetStyle { id: 2, style: leaf },
        Patch::SetChildren {
            id: 1,
            children: vec![2],
        },
        Patch::SetRoot { id: 1 },
    ]);
    r.apply(&patches);
    r.layout(400., 800., Insets::default());
    r
}

fn padded(top: f32, right: f32, bottom: f32, left: f32) -> Style {
    Style {
        padding: Edges {
            top,
            right,
            bottom,
            left,
        },
        ..Default::default()
    }
}

#[test]
fn a_padded_text_grows_by_its_padding_and_gets_it_as_its_inset() {
    // `text("Chip").px(12.).py(4.)`: 40 × 20 of text.
    let r = render(NodeKind::Text, "Chip", padded(4., 12., 4., 12.), false);
    assert_eq!(r.widgets.frames[&2], (0., 0., 64., 28.));
    assert_eq!(
        r.widgets.styles[&2].padding,
        padded(4., 12., 4., 12.).padding
    );
}

#[test]
fn a_padded_text_wraps_inside_its_content_box() {
    // 200 points of text in a 100-point box with 10 on each side: it wraps
    // at 80 (three lines).
    let style = Style {
        width: prism_core::Length::Px(100.),
        ..padded(5., 10., 5., 10.)
    };
    let r = render(NodeKind::Text, "twenty characters...", style, false);
    assert!(
        r.widgets.widths.contains(&Some(80.)),
        "{:?}",
        r.widgets.widths
    );
    assert!(!r.widgets.widths.contains(&Some(100.)));
    assert_eq!(r.widgets.frames[&2], (0., 0., 100., 70.));
}

#[test]
fn leading_padding_is_on_the_right_right_to_left() {
    // `.pl(10.)`: leading. Backends get leading = left, so it is mirrored.
    let r = render(NodeKind::TextInput, "", padded(0., 2., 0., 10.), true);
    let p = r.widgets.styles[&2].padding;
    assert_eq!((p.left, p.right), (2., 10.));
    // The box is at the leading (right) edge.
    let (x, _, w, _) = r.widgets.frames[&2];
    assert_eq!((x + w, w), (400., 42.));
}

#[test]
fn controls_without_text_are_placed_inside_their_padding() {
    let r = render(NodeKind::Toggle, "", padded(4., 6., 8., 10.), false);
    // 30 × 20 of control, its box 46 × 32: the view gets the content box.
    assert_eq!(r.widgets.frames[&2], (10., 4., 30., 20.));
    // And no padding to draw.
    assert!(r.widgets.styles[&2].padding.is_zero());
    // Right to left, leading (10) is on the right.
    let r = render(NodeKind::Toggle, "", padded(4., 6., 8., 10.), true);
    assert_eq!(r.widgets.frames[&2], (400. - 46. + 6., 4., 30., 20.));
}

#[test]
fn containers_keep_their_padding_to_the_layout() {
    let r = render(NodeKind::Text, "Chip", Style::default(), false);
    assert!(r.widgets.styles[&1].padding.is_zero());
    assert_eq!(r.widgets.frames[&2], (0., 0., 40., 20.));
}
