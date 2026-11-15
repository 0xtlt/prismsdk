//! Easing curves, transforms, poses and animated updates.

use prism_core::prelude::*;
use prism_core::{Animations, Curve, Patch, Transform};

use prism_testing::{Harness, mount};

fn samples(t: Transition) -> Vec<f32> {
    (0..=100).map(|i| t.ease_at(i as f32 / 100.)).collect()
}

#[test]
fn every_curve_starts_at_0_ends_at_1_and_clamps() {
    for t in [
        Transition::ease(200),
        Transition::ease_in(200),
        Transition::ease_out(200),
        Transition::linear(200),
        Transition::spring(200),
    ] {
        assert_eq!(t.ease_at(0.), 0., "{:?}", t.curve);
        assert_eq!(t.ease_at(1.), 1., "{:?}", t.curve);
        assert_eq!(t.ease_at(-3.), 0., "{:?}", t.curve);
        assert_eq!(t.ease_at(7.), 1., "{:?}", t.curve);
    }
}

#[test]
fn non_spring_curves_are_monotonic_and_bounded() {
    for t in [
        Transition::ease(200),
        Transition::ease_in(200),
        Transition::ease_out(200),
        Transition::linear(200),
    ] {
        let s = samples(t);
        assert!(s.windows(2).all(|w| w[1] >= w[0] - 1e-5), "{:?}", t.curve);
        assert!(s.iter().all(|v| (0.0..=1.0).contains(v)), "{:?}", t.curve);
    }
}

#[test]
fn curves_have_their_shape() {
    let at = |t: Transition| t.ease_at(0.3);
    // Ease-in starts slow, ease-out starts fast.
    assert!(at(Transition::ease_in(1)) < at(Transition::linear(1)));
    assert!(at(Transition::ease_out(1)) > at(Transition::linear(1)));
    // Ease-in-out is symmetric around the middle.
    let e = Transition::ease(1);
    for p in [0.1f32, 0.25, 0.4] {
        assert!((e.ease_at(p) + e.ease_at(1. - p) - 1.).abs() < 1e-3, "{p}");
    }
    // Springs overshoot a little, then settle.
    let s = samples(Transition::spring(1));
    let peak = s.iter().cloned().fold(0f32, f32::max);
    assert!(peak > 1. && peak < 1.2, "peak {peak}");
}

#[test]
fn css_easing_matches_the_bezier_points() {
    for (curve, css) in [
        (Transition::ease(1), "ease-in-out"),
        (Transition::linear(1), "linear"),
        (Transition::ease_out(1), "cubic-bezier(.2,.8,.2,1)"),
        (Transition::ease_in(1), "cubic-bezier(.4,0,1,1)"),
        (Transition::spring(1), "cubic-bezier(.34,1.56,.64,1)"),
    ] {
        assert_eq!(curve.css_easing(), css);
        if let Some(points) = css.strip_prefix("cubic-bezier(") {
            let p: Vec<f32> = points
                .trim_end_matches(')')
                .split(',')
                .map(|v| v.parse().unwrap())
                .collect();
            let (a, b, c, d) = curve.bezier();
            assert_eq!(p, [a, b, c, d]);
        }
    }
    assert_eq!(Transition::ease(1).bezier(), (0.42, 0., 0.58, 1.));
}

#[test]
fn transitions_convert_milliseconds() {
    let t = Transition::spring(450).delay(120);
    assert_eq!(t.curve, Curve::Spring);
    assert!((t.duration - 0.45).abs() < 1e-6);
    assert!((t.delay - 0.12).abs() < 1e-6);
}

#[test]
fn transforms_print_css_and_matrices() {
    assert_eq!(Transform::IDENTITY.css(), "none");
    let t = Transform {
        x: 4.,
        y: -2.,
        scale: 1.5,
        rotate: 45.,
        ..Transform::IDENTITY
    };
    assert_eq!(t.css(), "translate(4px,-2px) rotate(45deg) scale(1.5)");
    let squashed = Transform {
        scale: 2.,
        scale_y: 0.25,
        ..Transform::IDENTITY
    };
    assert_eq!(
        squashed.css(),
        "translate(0px,0px) rotate(0deg) scale(2,0.5)"
    );
    // Non-uniform scale around the center of a 10 × 10 box: x doubles, y halves.
    let [a, b, c, d, tx, ty] = squashed.matrix_around_center(10., 10.);
    assert_eq!((a, b, c, d), (2., 0., -0., 0.5));
    assert_eq!((a * 5. + tx, d * 5. + ty), (5., 5.));
    // A pure translation moves every point by the offset.
    let m = Transform {
        x: 3.,
        y: 7.,
        ..Transform::IDENTITY
    }
    .matrix_around_center(20., 10.);
    assert_eq!(m, [1., 0., -0., 1., 3., 7.]);
}

#[test]
fn poses_build_and_mirror() {
    let p = Pose::new().x(5.).y(6.);
    assert_eq!(p.offset, Some((5., 6.)));
    assert_eq!(Pose::new().y(2.).x(1.).offset, Some((1., 2.)));
    assert_eq!(Pose::new().opacity(3.).opacity, Some(1.));
    assert_eq!(Pose::new().opacity(-1.).opacity, Some(0.));
    assert_eq!(Pose::pop().scale, Some(0.85));
    assert_eq!(Pose::fade().opacity, Some(0.));
    let a = Animations {
        enter: Some(Pose::slide(10., 4.)),
        exit: Some(Pose::new().x(-3.)),
        repeat: Some((Pose::new().offset(1., 1.).rotate(10.), Transition::ease(1))),
        restart: true,
    };
    let m = a.scaled(2., true);
    assert_eq!(m.enter.unwrap().offset, Some((-20., 8.)));
    assert_eq!(m.exit.unwrap().offset, Some((6., 0.)));
    let (rp, _) = m.repeat.unwrap();
    assert_eq!((rp.offset, rp.rotate), (Some((-2., 2.)), Some(10.)));
    assert!(m.restart, "scaling keeps the loop mode");
    assert!(Animations::default().is_empty() && !a.is_empty());
}

#[test]
fn poses_scale_each_axis_and_turn_fully() {
    let rest = Transform {
        scale: 2.,
        ..Transform::IDENTITY
    };
    let (_, t) = Pose::new().scale_y(0.1).apply(1., rest);
    assert_eq!((t.sx(), t.sy()), (2., 0.2));
    // Halfway through a full turn is 180°, not the matrix average (identity).
    let to = Transform {
        rotate: 360.,
        ..Transform::IDENTITY
    };
    assert_eq!(Transform::IDENTITY.lerp(to, 0.5).rotate, 180.);
    assert_eq!(Transform::IDENTITY.lerp(to, 1.), to);
}

#[test]
fn repeat_from_start_restarts_and_scale_builders_set_each_axis() {
    use prism_core::{Styled, div};
    let mut d = div()
        .scale_x(1.5)
        .scale_y(0.5)
        .translate_percent(-50., -100.)
        .repeat_from_start(Pose::new().rotate(360.), Transition::linear(800));
    let s = d.style_mut().clone();
    assert_eq!((s.transform.scale_x, s.transform.scale_y), (1.5, 0.5));
    assert_eq!(s.translate_percent, (-50., -100.));
    assert!(s.animations.restart);
    assert_eq!(s.animations.repeat.unwrap().0.rotate, Some(360.));
    let mut back_and_forth = div().repeat(Pose::new().scale(1.1), Transition::ease(500));
    assert!(!back_and_forth.style_mut().animations.restart);
}

// --- with_animation and Reduce Motion ---------------------------------------

#[derive(Default)]
struct Bar {
    wide: bool,
}

impl Render for Bar {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(if self.wide { 200. } else { 100. }))
    }
}

fn bar(reduce_motion: bool) -> Harness {
    Harness::with_runtime(
        |init| {
            prism_core::Runtime::with_options(
                init,
                prism_core::RuntimeOptions {
                    environment: prism_core::Environment {
                        reduce_motion,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
        },
        |cx| {
            let root = cx.new_entity(|_| Bar::default());
            mount(cx, root);
        },
    )
}

fn transitions(p: &[Patch]) -> Vec<Option<Transition>> {
    p.iter()
        .filter_map(|p| match p {
            Patch::SetStyle { style, .. } => Some(style.transition),
            _ => None,
        })
        .collect()
}

#[test]
fn with_animation_is_skipped_with_reduce_motion() {
    for reduce in [false, true] {
        let mut h = bar(reduce);
        let root = h.root::<Bar>();
        h.rt.app.with_animation(Transition::ease(300), |app| {
            root.update(app, |b, cx| {
                b.wide = true;
                cx.notify();
            })
        });
        let t = transitions(h.render());
        let expected = (!reduce).then(|| Transition::ease(300));
        assert_eq!(t, vec![expected], "reduce motion: {reduce}");
        assert_eq!(h.host.env.as_ref().unwrap().reduce_motion, reduce);
    }
}
