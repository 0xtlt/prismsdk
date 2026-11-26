//! Box decorations and interaction states: builders, resolution, the
//! text-relative color, and how they reach the patches.

use prism_core::prelude::*;
use prism_core::{
    BorderStyle, Fill, GradientKind, Interaction, Interactions, Patch, Style, StyleContext, rgba,
};

use prism_testing::{Harness, mount};

fn resolved(s: Style) -> Style {
    s.resolve(StyleContext::default())
}

#[test]
fn current_color_is_css_current_color_and_resolves_against_the_text() {
    let tint = Color::current().with_alpha(0.12);
    assert!(tint.is_current());
    assert_eq!(
        tint.to_css(),
        "color-mix(in srgb, currentColor 12%, transparent)"
    );
    assert_eq!(
        tint.resolve_current(rgba(0x336699, 0.5)),
        rgba(0x336699, 0.06)
    );
    let adaptive = Color::adaptive(rgb(0x000000), rgb(0xFFFFFF));
    assert_eq!(
        tint.resolve_current(adaptive).dark,
        Some([1., 1., 1., 0.12])
    );
    // Ordinary colors are left alone.
    assert_eq!(rgb(0x123456).resolve_current(adaptive), rgb(0x123456));

    let s = Style::default()
        .text_color(rgb(0x0000FF))
        .bg(tint)
        .border_bottom(1., Color::current())
        .shadow(Shadow::new(0., 1., 2., tint))
        .bg_gradient(Gradient::linear(
            90.,
            [Color::current(), Color::TRANSPARENT],
        ))
        .with_current_color(Color::BLACK);
    assert_eq!(s.background, Some(rgba(0x0000FF, 0.12)));
    assert_eq!(s.borders()[2].color, Some(rgb(0x0000FF)));
    assert_eq!(s.shadows[0].color, rgba(0x0000FF, 0.12));
    assert_eq!(s.gradient.unwrap().stops[0].color, rgb(0x0000FF));
}

#[test]
fn shadows_gradients_and_borders_build_and_print_as_css() {
    let sh = Shadow::new(0., 4., 12., rgba(0x000000, 0.25))
        .spread(-2.)
        .inset();
    assert_eq!(sh.to_css(), "inset 0px 4px 12px -2px rgba(0,0,0,0.25)");
    assert_eq!(Shadow::ring(3., rgb(0xFF0000)).spread, 3.);

    let g = Gradient::linear(180., [rgb(0xFF0000), rgb(0x00FF00), rgb(0x0000FF)]);
    assert_eq!(
        g.stops.iter().map(|s| s.at).collect::<Vec<_>>(),
        [0., 0.5, 1.]
    );
    assert_eq!(
        g.to_css(),
        "linear-gradient(180deg, rgba(255,0,0,1) 0%, rgba(0,255,0,1) 50%, rgba(0,0,255,1) 100%)"
    );
    // CSS's gradient line: top to bottom, through the center.
    let close = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
    assert!(close(g.geometry(100., 50.), [50., 0., 50., 50.]));
    // 90°: left to right; 45° on a square: corner to corner.
    let right = Gradient::linear(90., [Color::BLACK, Color::WHITE]);
    assert!(close(right.geometry(100., 50.), [0., 25., 100., 25.]));
    let diag = Gradient::linear(45., [Color::BLACK, Color::WHITE]);
    assert!(close(diag.geometry(100., 100.), [0., 100., 100., 0.]));
    let radial = Gradient::radial([Color::BLACK, Color::WHITE])
        .with_stops([(Color::BLACK, 0.2), (Color::WHITE, 0.9)]);
    assert_eq!(radial.kind, GradientKind::Radial);
    assert_eq!(radial.stops[1].at, 0.9);
    assert!(
        radial
            .to_css()
            .starts_with("radial-gradient(rgba(0,0,0,1) 20%")
    );
    let [cx, cy, rx, ry] = radial.geometry(100., 50.);
    assert_eq!((cx, cy), (50., 25.));
    assert!((rx - 70.71).abs() < 0.01 && (ry - 35.36).abs() < 0.01);

    // Conic: CSS `conic-gradient(from …)`, the stops over a full turn.
    let wheel = Gradient::conic(90., [rgb(0xFF0000), rgb(0x00FF00), rgb(0xFF0000)]);
    assert_eq!(wheel.kind, GradientKind::Conic { from: 90. });
    assert_eq!(
        wheel.stops.iter().map(|s| s.at).collect::<Vec<_>>(),
        [0., 0.5, 1.]
    );
    assert_eq!(
        wheel.to_css(),
        "conic-gradient(from 90deg, rgba(255,0,0,1) 0%, rgba(0,255,0,1) 50%, rgba(255,0,0,1) 100%)"
    );
    // The center, and where the first color starts: to the right at 90°,
    // on the largest circle inside the box; up at 0°.
    assert!(close(wheel.geometry(100., 50.), [50., 25., 75., 25.]));
    let up = Gradient::conic(0., [Color::BLACK, Color::WHITE]);
    assert!(close(up.geometry(40., 40.), [20., 20., 20., 0.]));
    // Colors resolve like the other kinds' and keep the angle.
    let current = Gradient::conic(45., [Color::current(), Color::WHITE])
        .map_colors(|c| c.resolve_current(rgb(0x0000FF)));
    assert_eq!(current.kind, GradientKind::Conic { from: 45. });
    assert_eq!(current.stops[0].color, rgb(0x0000FF));

    assert_eq!(BorderStyle::Solid.dash(2.), None);
    assert_eq!(BorderStyle::Dashed.dash(2.), Some((6., 4.)));
    assert_eq!(BorderStyle::Dotted.dash(2.), Some((0., 4.)));

    // Per-side borders override the uniform one on their side only.
    let s = Style::default()
        .border(1.)
        .border_color(rgb(0x00FF00))
        .border_top(3., rgb(0xFF0000));
    let [t, r, b, l] = s.borders();
    assert_eq!((t.width, t.color), (3., Some(rgb(0xFF0000))));
    assert_eq!(r, b);
    assert_eq!((l.width, l.color), (1., Some(rgb(0x00FF00))));
    assert!(s.has_side_borders());
    assert!(!Style::default().border(1.).has_side_borders());
    let s = Style::default()
        .border_left(1., rgb(0))
        .border_right(2., rgb(0));
    assert_eq!(s.border_sides.mirrored().left.unwrap().width, 2.);
    assert_eq!(
        Cursor::from_css(Cursor::NotAllowed.css()),
        Some(Cursor::NotAllowed)
    );
}

#[test]
fn states_resolve_per_state_and_combine_in_a_fixed_order() {
    let s = resolved(
        Style::default()
            .bg(rgb(0xFFFFFF))
            .active(|s| s.bg(rgb(0x0000FF)))
            .hover(|s| s.bg(rgb(0xFF0000)).opacity(0.5))
            .md(|s| s.focus(|s| s.border(2.))),
    );
    let used = s.states.used();
    assert!(used.contains(Interaction::Hover) && used.contains(Interaction::Active));
    // Width rules come first: the `md` focus refinement is not there yet.
    assert!(!used.contains(Interaction::Focus));
    let order: Vec<Interaction> = s.states.resolved().map(|(i, _)| i).collect();
    assert_eq!(order, [Interaction::Hover, Interaction::Active]);
    let (_, hover) = s.states.resolved().next().unwrap();
    assert_eq!(hover.background, Some(rgb(0xFF0000)));

    // Active wins over hover, whatever the order they were written in.
    let both = s.interacted(Interactions::from(Interaction::Hover) | Interaction::Active.into());
    assert_eq!(both.background, Some(rgb(0x0000FF)));
    assert_eq!(both.opacity, 0.5);
    assert!(!both.states.is_empty(), "the refinements stay for backends");
    assert_eq!(s.interacted(Interactions::NONE), s);

    // Wide windows: the focus refinement from the `md` rule.
    let wide = Style::default()
        .md(|s| s.focus(|s| s.border(2.)))
        .resolve(StyleContext {
            breakpoint: Breakpoint::Medium,
            ..Default::default()
        });
    assert_eq!(wide.interacted(Interaction::Focus.into()).border_width, 2.);

    // Resolved styles compare by what each state gives (no re-patching),
    // unresolved ones with rules never do.
    let make = || Style::default().hover(|s| s.bg(rgb(0xFF0000)));
    assert_eq!(resolved(make()), resolved(make()));
    assert_ne!(make(), make());
    let other = resolved(Style::default().hover(|s| s.bg(rgb(0x00FF00))));
    assert_ne!(resolved(make()), other);

    // Disabled: its refinement, and the others are dropped.
    let d = resolved(make().disabled(|s| s.opacity(0.4))).disabled_applied();
    assert_eq!(d.opacity, 0.4);
    assert!(d.states.is_empty());

    // Zoom scales what each state gives too.
    let z = resolved(Style::default().hover(|s| s.border(1.))).scaled(2.);
    assert_eq!(z.states.resolved().next().unwrap().1.border_width, 2.);
}

#[test]
fn linear_gradients_take_css_angles_and_placed_stops() {
    // #330: `.bg_linear_gradient(angle, [(color, stop), …])`.
    let clear = rgba(0xFFFFFF, 0.);
    let s = Style::default().bg_linear_gradient(180., [(clear, 0.), (rgb(0xFFFFFF), 0.8)]);
    let g = s.gradient.clone().expect("a gradient");
    assert_eq!(g.kind, GradientKind::Linear { angle: 180. });
    assert_eq!(
        g.stops.iter().map(|s| (s.color, s.at)).collect::<Vec<_>>(),
        [(clear, 0.), (rgb(0xFFFFFF), 0.8)]
    );
    assert_eq!(
        g,
        Gradient::linear_stops(180., [(clear, 0.), (rgb(0xFFFFFF), 0.8)])
    );
    assert_eq!(
        g.to_css(),
        "linear-gradient(180deg, rgba(255,255,255,0) 0%, rgba(255,255,255,1) 80%)"
    );

    // CSS's angles: 0 to the top, 90 to the right, 180 to the bottom, 270
    // to the left (`geometry`: from the first color to the last).
    let close = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
    let line = |angle: f32| Gradient::linear(angle, [Color::BLACK, Color::WHITE]);
    assert!(close(line(0.).geometry(100., 50.), [50., 50., 50., 0.]));
    assert!(close(line(90.).geometry(100., 50.), [0., 25., 100., 25.]));
    assert!(close(line(180.).geometry(100., 50.), [50., 0., 50., 50.]));
    assert!(close(line(270.).geometry(100., 50.), [100., 25., 0., 25.]));

    // Stops placed like CSS: clamped, never going back (a hard edge), a NaN
    // where the one before is; one stop is a solid fill, none no gradient.
    let placed = Gradient::linear_stops(
        90.,
        [
            (rgb(1), -0.5),
            (rgb(2), 0.6),
            (rgb(3), 0.4),
            (rgb(4), f32::NAN),
            (rgb(5), 2.),
        ],
    )
    .normalized()
    .unwrap();
    assert_eq!(
        placed.stops.iter().map(|s| s.at).collect::<Vec<_>>(),
        [0., 0.6, 0.6, 0.6, 1.]
    );
    let one = Style::default().bg_linear_gradient(90., [(rgb(7), 0.3)]);
    assert_eq!(
        one.gradient
            .unwrap()
            .stops
            .iter()
            .map(|s| (s.color, s.at))
            .collect::<Vec<_>>(),
        [(rgb(7), 0.), (rgb(7), 1.)]
    );
    let none = Style::default()
        .bg_gradient(Gradient::linear(90., [rgb(0), rgb(1)]))
        .bg_linear_gradient(90., []);
    assert_eq!(none.gradient, None);

    // Core Animation's unit line: the CSS line in a square box, rotated in
    // a wide one so the stretched isobars stay perpendicular to it.
    let diag = line(135.);
    assert!(close(diag.unit_points(100., 100.), [0., 0., 1., 1.]));
    let [x0, y0, x1, y1] = diag.unit_points(200., 100.);
    let (ux, uy) = (x1 - x0, y1 - y0);
    // An isobar of CSS's line, in points, is along (-1, 1) for 135°: in
    // unit space (-1/200, 1/100), perpendicular to the unit line.
    assert!((ux * (-1. / 200.) + uy * (1. / 100.)).abs() < 1e-6);
    // Axis-aligned lines are the same in both spaces.
    assert!(close(
        line(180.).unit_points(200., 100.),
        [0.5, 0., 0.5, 1.]
    ));
}

#[test]
fn fills_replace_the_whole_background() {
    let g = Gradient::linear(90., [rgb(0xFF0000), rgb(0x0000FF)]);
    // `bg` and `bg_gradient` layer: the gradient over the color.
    let layered = Style::default().bg(rgb(0)).bg_gradient(g.clone());
    assert_eq!(layered.background, Some(rgb(0)));
    assert!(layered.gradient.is_some());
    // `bg_fill` replaces both, either way.
    let solid = layered.clone().bg_fill(rgb(0x00FF00));
    assert_eq!(
        (solid.background, solid.gradient.clone()),
        (Some(rgb(0x00FF00)), None)
    );
    let back = solid.bg_fill(g.clone());
    assert_eq!((back.background, back.gradient), (None, Some(g.clone())));
    assert_eq!(Fill::from(rgb(1)), Fill::Solid(rgb(1)));

    // In a state: a gradient button turns solid on hover, and back.
    let s = resolved(
        Style::default()
            .bg_gradient(g.clone())
            .hover(|s| s.bg_fill(rgb(0x00FF00))),
    );
    let hover = s.interacted(Interaction::Hover.into());
    assert_eq!(
        (hover.background, hover.gradient),
        (Some(rgb(0x00FF00)), None)
    );
    assert_eq!(s.interacted(Interactions::NONE).gradient, Some(g.clone()));

    // Transitions animate between gradients of the same kind and length.
    let other = Gradient::linear(0., [rgb(0), rgb(1)]);
    assert!(g.interpolates_with(&other));
    assert!(!g.interpolates_with(&Gradient::radial([rgb(0), rgb(1)])));
    assert!(!g.interpolates_with(&Gradient::linear(90., [rgb(0), rgb(1), rgb(2)])));
}

struct Fade {
    solid: bool,
    angle: f32,
}

impl Render for Fade {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        let d = div().size(40.).rounded(8.);
        if self.solid {
            d.bg(rgb(0x00FF00))
        } else {
            d.bg_linear_gradient(self.angle, [(rgb(0xFF0000), 0.), (rgb(0x0000FF), 1.)])
        }
    }
}

#[test]
fn gradients_patch_to_solid_backgrounds_and_back() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Fade {
            solid: false,
            angle: 180.,
        });
        mount(cx, root);
    });
    let style_of = |patches: &[Patch]| {
        patches
            .iter()
            .find_map(|p| match p {
                Patch::SetStyle { style, .. } => Some(style.clone()),
                _ => None,
            })
            .expect("a style patch")
    };
    // A new angle: a style patch with it.
    let s = style_of(h.update::<Fade>(|f| f.angle = 90.));
    assert_eq!(
        s.gradient.unwrap().kind,
        GradientKind::Linear { angle: 90. }
    );
    // Solid: the gradient goes.
    let s = style_of(h.update::<Fade>(|f| f.solid = true));
    assert_eq!((s.background, s.gradient), (Some(rgb(0x00FF00)), None));
    // And back: the gradient returns, the color goes.
    let s = style_of(h.update::<Fade>(|f| f.solid = false));
    assert_eq!(s.background, None);
    assert_eq!(s.gradient.unwrap().stops.len(), 2);
    // The same gradient again: nothing to patch.
    let patches = h.update::<Fade>(|f| f.angle = 90.).to_vec();
    assert!(
        !patches.iter().any(|p| matches!(p, Patch::SetStyle { .. })),
        "{patches:?}"
    );
}

struct Rows {
    disabled: bool,
    n: u32,
}

impl Render for Rows {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(text(format!("{}", self.n))).child(
            div()
                .accessibility_disabled(self.disabled)
                .hover(|s| s.bg(rgb(0xEEEEEE)))
                .disabled(|s| s.opacity(0.5)),
        )
    }
}

#[test]
fn reconciling_states_patches_only_real_changes() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Rows {
            disabled: false,
            n: 0,
        });
        mount(cx, root);
    });
    // Another render with the same refinements: no style patch.
    let patches = h.update::<Rows>(|r| r.n = 1).to_vec();
    assert!(
        !patches.iter().any(|p| matches!(p, Patch::SetStyle { .. })),
        "{patches:?}"
    );
    // Disabled: the disabled refinement is applied by the reconciler.
    let patches = h.update::<Rows>(|r| r.disabled = true).to_vec();
    let style = patches
        .iter()
        .find_map(|p| match p {
            Patch::SetStyle { style, .. } => Some(style.clone()),
            _ => None,
        })
        .expect("a style patch");
    assert_eq!(style.opacity, 0.5);
    assert!(style.states.is_empty(), "no hover while disabled");
}

/// #274: a field styled by the app draws no platform focus ring; an
/// unstyled one keeps it.
#[test]
fn fields_with_their_own_look_skip_the_platform_focus_ring() {
    let look = |s: Style| resolved(s).own_field_look();
    assert!(!look(Style::default()), "unstyled: the platform's ring");
    assert!(!look(Style::default().text_color(rgba(0, 1.))));
    assert!(look(Style::default().border(0.)));
    assert!(look(Style::default().bg(rgba(0, 0.)).border(0.).p(0.)));
    assert!(look(Style::default().p(0.)));
    assert!(look(Style::default().border_bottom(1., rgba(0, 1.))));
    assert!(look(Style::default().focus(|s| s.bg(rgba(0, 0.1)))));
    assert!(look(Style::default().focus_within(|s| s.border(2.))));
    assert!(!look(Style::default().hover(|s| s.bg(rgba(0, 0.1)))));
}
