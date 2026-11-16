//! Transitions, enter/exit poses, loops and shape redraws.

use prism_core::{
    Backend, Context, Environment, FluentBuilder, IntoElement, Locale, NodeKind, ParentElement,
    Path, Pose, Render, Router, Segment, Styled, Transition, UiEvent,
    animation::{DEFAULT_ENTER, DEFAULT_EXIT},
    button, div, rgb, screen, shape,
};

use crate::common::{Call, Harness};

/// A box whose width, color and presence flip with a button.
struct Flip {
    on: bool,
}

impl Render for Flip {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.on;
        div()
            .child(button("flip").on_click(cx.listener(|this, _, _| this.on ^= true)))
            .child(
                div()
                    .key("box")
                    .h(10.)
                    .w(if on { 200. } else { 100. })
                    .bg(if on { rgb(0xff0000) } else { rgb(0x0000ff) })
                    .animate(250),
            )
            .when(!on, |d| {
                d.child(
                    div()
                        .key("leaving")
                        .h(10.)
                        .exit(Pose::fade().offset(20., 0.)),
                )
            })
            .when(!on, |d| d.child(div().key("plain").h(10.)))
    }
}

fn flip(h: &mut Harness) {
    let b = h.w().by_text("flip");
    let patches = h.rt.dispatch(UiEvent::Click(b));
    h.r.apply(&patches);
}

fn the_box(h: &Harness) -> u64 {
    h.w().of_kind(NodeKind::View)[1]
}

#[test]
fn first_placement_is_instant_then_changes_animate() {
    let mut h = Harness::root(|| Flip { on: false });
    h.layout(400., 300.);
    let b = the_box(&h);
    let calls = h.r.widgets.take_calls();
    assert!(calls.contains(&Call::Frame(b, (0., 20., 100., 10.))));
    assert!(
        !calls
            .iter()
            .any(|c| matches!(c, Call::FrameAnimated(..) | Call::StyleAnimated(..))),
        "nothing animates before it is on screen: {calls:?}"
    );

    flip(&mut h);
    h.layout(400., 300.);
    let calls = h.r.widgets.take_calls();
    let t = Transition::ease(250);
    assert!(
        calls.contains(&Call::FrameAnimated(b, (0., 20., 200., 10.), t)),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, Call::StyleAnimated(id, s, tr) if *id == b && *tr == t && s.background == Some(rgb(0xff0000)))),
        "the color change animates: {calls:?}"
    );
}

/// A screen with a card that fills a row beside a box the button widens.
struct Card {
    wide: bool,
}

impl Render for Card {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        screen("Cards").child(
            div()
                .child(button("widen").on_click(cx.listener(|this, _, _| this.wide ^= true)))
                .child(
                    div()
                        .flex_row()
                        .child(div().h(10.).w(if self.wide { 100. } else { 50. }))
                        .child(div().key("card").flex_1().h(10.).animate(250)),
                ),
        )
    }
}

fn animated_frames(calls: &[Call]) -> Vec<&Call> {
    calls
        .iter()
        .filter(|c| matches!(c, Call::FrameAnimated(..)))
        .collect()
}

#[test]
fn frames_the_platform_resizes_are_placed_not_animated() {
    let mut h = Harness::new(|cx| {
        let r = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| Card { wide: false }))
            .build(cx);
        cx.set_root(r);
    });
    h.layout(400., 800.);
    let card = *h.w().of_kind(NodeKind::View).last().unwrap();
    let before = h.w().frame(card);
    h.r.widgets.take_calls();

    // The screen's safe area becomes known (#41: a screen shown for the
    // first time, a side bar).
    h.r.widgets.screen_insets.left = 40.;
    h.layout(400., 800.);
    let calls = h.r.widgets.take_calls();
    assert_ne!(h.w().frame(card), before);
    assert_eq!(animated_frames(&calls), Vec::<&Call>::new());

    // The window resizes (rotation, a foldable unfolding).
    h.layout(700., 800.);
    let calls = h.r.widgets.take_calls();
    assert_eq!(h.w().frame(card).2, 700. - 40. - 50.);
    assert_eq!(animated_frames(&calls), Vec::<&Call>::new());

    // The same pass again: nothing moves, nothing animates.
    h.layout(700., 800.);
    assert!(h.r.widgets.take_calls().is_empty());

    // The app's state changes the card's width: animated.
    let b = h.w().by_text("widen");
    let patches = h.rt.dispatch(UiEvent::Click(b));
    h.r.apply(&patches);
    h.layout(700., 800.);
    let calls = h.r.widgets.take_calls();
    assert_eq!(
        animated_frames(&calls),
        vec![&Call::FrameAnimated(
            card,
            (100., 0., 700. - 40. - 100., 10.),
            Transition::ease(250)
        )]
    );
}

#[test]
fn removed_children_with_an_exit_pose_animate_out_before_being_destroyed() {
    let mut h = Harness::root(|| Flip { on: false });
    h.layout(400., 300.);
    let views = h.w().of_kind(NodeKind::View);
    let (root, leaving, plain) = (views[0], views[2], views[3]);
    h.r.widgets.take_calls();
    flip(&mut h);
    let calls = h.r.widgets.take_calls();
    let exits: Vec<&Call> = calls
        .iter()
        .filter(|c| matches!(c, Call::Exit { .. }))
        .collect();
    assert_eq!(
        exits.len(),
        1,
        "only the element with an exit pose: {calls:?}"
    );
    let Call::Exit {
        parent,
        view,
        to,
        t,
    } = exits[0]
    else {
        unreachable!()
    };
    assert_eq!((*parent, *view), (root, leaving));
    assert_eq!(to.opacity, 0.);
    assert_eq!(to.transform.x, 20.);
    assert_eq!(*t, DEFAULT_EXIT);
    let exit_at = calls.iter().position(|c| matches!(c, Call::Exit { .. }));
    let destroy_at = calls.iter().position(|c| *c == Call::Destroy(leaving));
    assert!(exit_at < destroy_at, "exit before destroy: {calls:?}");
    assert!(calls.contains(&Call::Destroy(plain)));
}

#[test]
fn exits_are_mirrored_right_to_left_and_skipped_with_reduce_motion() {
    let mut h = Harness::root(|| Flip { on: false });
    h.rt.app.set_locale(Some(Locale::new("ar")));
    h.rerender();
    h.layout(400., 300.);
    h.r.widgets.take_calls();
    flip(&mut h);
    let to_x = h.r.widgets.take_calls().iter().find_map(|c| match c {
        Call::Exit { to, .. } => Some(to.transform.x),
        _ => None,
    });
    assert_eq!(to_x, Some(-20.));

    let mut h = Harness::root(|| Flip { on: false });
    h.rt.app.set_environment(Environment {
        reduce_motion: true,
        ..Default::default()
    });
    h.rerender();
    h.layout(400., 300.);
    flip(&mut h);
    h.layout(400., 300.);
    let calls = h.r.widgets.take_calls();
    assert!(
        !calls.iter().any(|c| matches!(
            c,
            Call::Exit { .. } | Call::FrameAnimated(..) | Call::StyleAnimated(..)
        )),
        "reduce motion: {calls:?}"
    );
}

struct Enter;
impl Render for Enter {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(div().h(10.).opacity(0.8).enter(Pose::slide(4., 16.)))
    }
}

#[test]
fn enter_poses_start_posed_then_animate_to_the_style() {
    let mut h = Harness::root(|| Enter);
    h.r.set_zoom(2.);
    h.r.widgets.take_calls();
    h.layout(400., 300.);
    let b = h.w().of_kind(NodeKind::View)[1];
    let calls: Vec<Call> =
        h.r.widgets
            .take_calls()
            .into_iter()
            .filter(|c| match c {
                Call::Style(id, _) | Call::StyleAnimated(id, ..) | Call::Frame(id, _) => *id == b,
                _ => false,
            })
            .collect();
    let [
        Call::Style(_, from),
        Call::StyleAnimated(_, to, t),
        Call::Frame(..),
    ] = calls.as_slice()
    else {
        panic!("posed style, animation, frame: {calls:?}");
    };
    assert_eq!(from.opacity, 0.);
    assert_eq!(
        (from.transform.x, from.transform.y),
        (8., 32.),
        "zoomed offsets"
    );
    assert_eq!(to.opacity, 0.8);
    assert!(to.transform.is_identity());
    assert_eq!(*t, DEFAULT_ENTER);
    // Laid out again: no second entrance.
    h.layout(500., 300.);
    assert!(
        !h.r.widgets
            .take_calls()
            .iter()
            .any(|c| matches!(c, Call::StyleAnimated(..)))
    );
}

struct Pulse {
    on: bool,
}
impl Render for Pulse {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.on;
        div()
            .child(button("stop").on_click(cx.listener(|this, _, _| this.on = false)))
            .child(div().h(10.).when(on, |d| {
                d.repeat(Pose::new().scale(1.15), Transition::ease(700))
            }))
    }
}

#[test]
fn loops_start_and_stop() {
    let mut h = Harness::root(|| Pulse { on: true });
    let b = h.w().of_kind(NodeKind::View)[1];
    let calls = h.r.widgets.take_calls();
    let pose = Pose::new().scale(1.15);
    assert!(calls.contains(&Call::Repeat(b, Some((pose, Transition::ease(700))))));
    let stop = h.w().by_text("stop");
    let patches = h.rt.dispatch(UiEvent::Click(stop));
    h.r.apply(&patches);
    assert!(h.r.widgets.take_calls().contains(&Call::Repeat(b, None)));
}

struct Chart {
    wide: bool,
}
impl Render for Chart {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let peak = if self.wide { 0. } else { 0.5 };
        div()
            .child(button("change").on_click(cx.listener(|this, _, _| this.wide ^= true)))
            .child(
                shape()
                    .path(Path::line([(0., 1.), (1., peak)]))
                    .w_full()
                    .h(100.),
            )
    }
}

#[test]
fn shapes_redraw_on_resize_and_new_data_only() {
    let mut h = Harness::root(|| Chart { wide: false });
    h.layout(400., 300.);
    let s = h.w().of_kind(NodeKind::Shape)[0];
    assert_eq!(
        h.w().drawings[&s][0].segments,
        [Segment::Move(0., 100.), Segment::Line(400., 50.)]
    );
    h.r.widgets.take_calls();
    h.layout(400., 500.);
    assert!(
        !h.r.widgets.take_calls().contains(&Call::Drawing(s)),
        "same size: no redraw"
    );
    let change = h.w().by_text("change");
    let patches = h.rt.dispatch(UiEvent::Click(change));
    h.r.apply(&patches);
    assert_eq!(h.w().drawings[&s][0].segments[1], Segment::Line(400., 0.));
    h.r.set_zoom(2.);
    h.layout(400., 300.);
    assert_eq!(
        h.w().drawings[&s][0].segments,
        [Segment::Move(0., 200.), Segment::Line(400., 0.)],
        "zoomed height, same width"
    );
}

/// A card with `.animate` that leaves a row for its own line, and a tint
/// changed later (#50).
struct Moving {
    wide: bool,
    tint: bool,
}

impl Render for Moving {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let card = div()
            .p(24.)
            .bg(if self.tint {
                rgb(0xff0000)
            } else {
                rgb(0xf28b6b)
            })
            .animate(420)
            .child(button("tint").on_click(cx.listener(|this, _, _| this.tint ^= true)));
        let place = if self.wide {
            div().child(card)
        } else {
            div().child(div().flex_row().child(card).child(div().w(50.).h(10.)))
        };
        div()
            .child(button("move").on_click(cx.listener(|this, _, _| this.wide ^= true)))
            .child(place)
    }
}

fn click(h: &mut Harness, label: &str) {
    let b = h.w().by_text(label);
    let patches = h.rt.dispatch(UiEvent::Click(b));
    h.r.apply(&patches);
    h.layout(400., 300.);
}

#[test]
fn an_element_that_starts_animating_where_another_was_appears_at_once() {
    let mut h = Harness::root(|| Moving {
        wide: false,
        tint: false,
    });
    h.layout(400., 300.);
    h.r.widgets.take_calls();

    // Its first frame and style are set, not animated from the row's.
    click(&mut h, "move");
    let calls = h.r.widgets.take_calls();
    assert!(
        !calls
            .iter()
            .any(|c| matches!(c, Call::FrameAnimated(..) | Call::StyleAnimated(..))),
        "{calls:?}"
    );

    // A change made to it afterwards animates.
    click(&mut h, "tint");
    let calls = h.r.widgets.take_calls();
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, Call::StyleAnimated(_, s, t)
            if s.background == Some(rgb(0xff0000)) && *t == Transition::ease(420))),
        "{calls:?}"
    );
}
