//! Interaction states (`.hover(|s| …)`…) and box decorations as native
//! backends receive them: the platform reports states, the renderer
//! restyles the node (and its ancestors for focus-within, its descendants
//! for groups); `Color::current()` and leading/trailing borders are resolved.

use prism_core::prelude::*;
use prism_core::{Backend, Interaction, Interactions, NodeId, NodeKind, Style, rgba};

use crate::common::Harness;

const RED: u32 = 0xFF0000;
const BLUE: u32 = 0x0000FF;

fn tree(f: fn() -> prism_core::Div) -> Harness {
    struct Root(fn() -> prism_core::Div);
    impl Render for Root {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            (self.0)()
        }
    }
    let mut h = Harness::root(move || Root(f));
    h.layout(400., 300.);
    h
}

fn views(h: &Harness) -> Vec<NodeId> {
    h.w().of_kind(NodeKind::View)
}

#[test]
fn hover_and_press_restyle_the_node_and_combine_in_order() {
    let mut h = tree(|| {
        div().child(
            div()
                .bg(rgb(0xFFFFFF))
                .hover(|s| s.bg(rgb(RED)).opacity(0.9))
                .active(|s| s.bg(rgb(BLUE))),
        )
    });
    let row = views(&h)[1];
    let w = h.w();
    let used: Interactions = Interactions::from(Interaction::Hover) | Interaction::Active.into();
    assert_eq!(w.interactions[&row], used, "the platform reports both");
    assert_eq!(w.styles[&row].background, Some(rgb(0xFFFFFF)));

    assert!(h.r.set_interaction(row, Interaction::Hover, true));
    assert_eq!(h.w().styles[&row].background, Some(rgb(RED)));
    assert_eq!(h.w().styles[&row].opacity, 0.9);
    // Pressed while hovered: active applies last, hover's opacity stays.
    assert!(h.r.set_interaction(row, Interaction::Active, true));
    assert_eq!(h.w().styles[&row].background, Some(rgb(BLUE)));
    assert_eq!(h.w().styles[&row].opacity, 0.9);
    // Same state again: nothing to do.
    assert!(!h.r.set_interaction(row, Interaction::Active, true));
    h.r.set_interaction(row, Interaction::Active, false);
    h.r.set_interaction(row, Interaction::Hover, false);
    assert_eq!(h.w().styles[&row].background, Some(rgb(0xFFFFFF)));
    assert_eq!(h.w().styles[&row].opacity, 1.);
    assert!(h.r.interactions(row).is_empty());
}

#[test]
fn a_rerender_keeps_the_current_state() {
    let mut h = tree(|| div().child(div().w(10.).hover(|s| s.bg(rgb(RED)))));
    let row = views(&h)[1];
    h.r.set_interaction(row, Interaction::Hover, true);
    // A new style from the app (e.g. after a click) is shown hovered.
    h.r.apply(&[prism_core::Patch::SetStyle {
        id: row,
        style: Style::default()
            .w(20.)
            .hover(|s| s.bg(rgb(BLUE)))
            .resolve(Default::default()),
    }]);
    assert_eq!(h.w().styles[&row].background, Some(rgb(BLUE)));
    assert_eq!(h.w().styles[&row].width, prism_core::Length::Px(20.));
}

#[test]
fn focus_marks_ancestors_focus_within_and_moves() {
    let mut h = tree(|| {
        div()
            .focus_within(|s| s.border(2.).border_color(rgb(BLUE)))
            .child(text_input().placeholder("a"))
            .child(text_input().placeholder("b"))
    });
    let root = views(&h)[0];
    let inputs = h.w().of_kind(NodeKind::TextInput);
    assert_eq!(h.w().styles[&root].border_width, 0.);
    h.r.set_interaction(inputs[0], Interaction::Focus, true);
    assert_eq!(h.w().styles[&root].border_width, 2.);
    assert!(h.r.interactions(inputs[0]).contains(Interaction::Focus));
    // Focus moves to the other field: still within.
    h.r.set_interaction(inputs[1], Interaction::Focus, true);
    assert!(!h.r.interactions(inputs[0]).contains(Interaction::Focus));
    assert_eq!(h.w().styles[&root].border_width, 2.);
    h.r.set_interaction(inputs[1], Interaction::Focus, false);
    assert_eq!(h.w().styles[&root].border_width, 0.);
}

#[test]
fn groups_drive_their_descendants_and_can_show_hidden_nodes() {
    let mut h = tree(|| {
        div().group().child(text_input().placeholder("q")).child(
            div()
                .w(30.)
                .h(30.)
                .with_style(|s| s.display_none = true)
                .group_focus_within(|s| s.with_style(|s| s.display_none = false))
                .group_hover(|s| s.bg(rgb(RED))),
        )
    });
    let [group, clear] = views(&h)[..] else {
        panic!("two views")
    };
    let input = h.w().of_kind(NodeKind::TextInput)[0];
    // A group reports hover, press and focus-within for its descendants.
    assert!(h.w().interactions[&group].contains(Interaction::Hover));
    assert_eq!(h.w().hidden.get(&clear), Some(&true));

    assert!(h.r.set_interaction(input, Interaction::Focus, true));
    assert_eq!(h.w().hidden.get(&clear), Some(&false));
    let f = h.layout(400., 300.);
    assert_eq!(f.frame(clear).2, 30., "laid out once shown");

    h.r.set_interaction(group, Interaction::Hover, true);
    assert_eq!(h.w().styles[&clear].background, Some(rgb(RED)));
    h.r.set_interaction(input, Interaction::Focus, false);
    assert_eq!(h.w().hidden.get(&clear), Some(&true));
}

/// #212: a refinement follows the group it names, through other groups,
/// and its size changes are laid out.
#[test]
fn named_groups_drive_only_their_refinements() {
    let mut h = tree(|| {
        div().group_named("block").child(
            div().group().child(
                div()
                    .w(8.)
                    .h(8.)
                    .group_hover_named("block", |s| s.w(16.).bg(rgb(RED)))
                    .group_hover(|s| s.opacity(0.5)),
            ),
        )
    });
    let [block, row, grip] = views(&h)[..] else {
        panic!("three views")
    };
    // A named group reports its states too.
    assert!(h.w().interactions[&block].contains(Interaction::Hover));
    assert_eq!(h.layout(400., 300.).frame(grip).2, 8.);

    // The inner (unnamed) group: only the unnamed refinement.
    h.r.set_interaction(row, Interaction::Hover, true);
    assert_eq!(h.w().styles[&grip].opacity, 0.5);
    assert_eq!(h.w().styles[&grip].background, None);
    h.r.set_interaction(row, Interaction::Hover, false);

    // The named block: its refinement, width included.
    assert!(h.r.set_interaction(block, Interaction::Hover, true));
    assert_eq!(h.w().styles[&grip].background, Some(rgb(RED)));
    assert_eq!(h.w().styles[&grip].opacity, 1.);
    assert_eq!(h.layout(400., 300.).frame(grip).2, 16.);
    h.r.set_interaction(block, Interaction::Hover, false);
    assert_eq!(h.w().styles[&grip].background, None);
    assert_eq!(h.layout(400., 300.).frame(grip).2, 8.);
}

#[test]
fn disabled_nodes_get_their_refinement_and_no_others() {
    let mut h = tree(|| {
        div().child(
            div()
                .accessibility_disabled(true)
                .hover(|s| s.bg(rgb(RED)))
                .disabled(|s| s.opacity(0.4)),
        )
    });
    let node = views(&h)[1];
    assert_eq!(h.w().styles[&node].opacity, 0.4);
    assert!(!h.r.set_interaction(node, Interaction::Hover, true));
    assert_eq!(h.w().styles[&node].background, None);
}

#[test]
fn current_color_and_side_borders_are_resolved_for_the_platform() {
    let mut h = tree(|| {
        div()
            .child(
                div()
                    .text_color(rgb(BLUE))
                    .bg(Color::current().with_alpha(0.5))
                    .border_left(2., Color::current())
                    .shadow(Shadow::new(0., 1., 2., Color::current().with_alpha(0.2))),
            )
            .child(div().bg(Color::current().with_alpha(0.1)))
    });
    let [_, tinted, plain] = views(&h)[..] else {
        panic!("three views")
    };
    let s = &h.w().styles[&tinted];
    assert_eq!(s.background, Some(rgba(BLUE, 0.5)));
    assert_eq!(s.shadows[0].color, rgba(BLUE, 0.2));
    assert_eq!(s.borders()[3].color, Some(rgb(BLUE)));
    assert!(s.has_side_borders());
    // No text color: the label color, black or white.
    let bg = h.w().styles[&plain].background.unwrap();
    assert_eq!((bg.r, bg.a), (0., 0.1));
    assert_eq!(bg.dark, Some([1., 1., 1., 0.1]));

    // Right to left: the leading border is on the right.
    h.rt.app.set_locale(Some(prism_core::Locale::new("ar")));
    h.rerender();
    let s = &h.w().styles[&tinted];
    assert_eq!(s.borders()[1].width, 2.);
    assert_eq!(s.borders()[3].width, 0.);
}

#[test]
fn zoom_scales_shadows_borders_and_blur() {
    let mut h = tree(|| {
        div().child(
            div()
                .shadow(Shadow::new(1., 2., 3., rgb(0)).spread(4.))
                .border_bottom(1., rgb(0))
                .backdrop_blur(10.),
        )
    });
    let node = views(&h)[1];
    h.r.set_zoom(2.);
    let s = &h.w().styles[&node];
    let sh = s.shadows[0];
    assert_eq!((sh.x, sh.y, sh.blur, sh.spread), (2., 4., 6., 8.));
    assert_eq!(s.borders()[2].width, 2.);
    assert_eq!(s.backdrop_blur, 20.);
}

/// #224: focus moves become events for the app, in order, once each.
#[test]
fn focus_changes_are_queued_for_the_app() {
    use prism_core::UiEvent;
    let mut h = tree(|| div().child(text_input()).child(text_input()));
    let fields = h.w().of_kind(NodeKind::TextInput);
    let (a, b) = (fields[0], fields[1]);
    h.r.set_interaction(a, Interaction::Focus, true);
    assert_eq!(h.r.take_focus_changes(), [UiEvent::FocusChange(a, true)]);
    h.r.set_interaction(a, Interaction::Focus, true);
    assert!(h.r.take_focus_changes().is_empty(), "no change, no event");
    h.r.set_interaction(b, Interaction::Focus, true);
    assert_eq!(
        h.r.take_focus_changes(),
        [
            UiEvent::FocusChange(a, false),
            UiEvent::FocusChange(b, true)
        ]
    );
    h.r.set_interaction(b, Interaction::Focus, false);
    assert_eq!(h.r.take_focus_changes(), [UiEvent::FocusChange(b, false)]);
}
