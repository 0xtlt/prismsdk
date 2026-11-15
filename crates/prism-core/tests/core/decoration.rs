//! Box decorations and interaction states: builders, resolution, the
//! text-relative color, and how they reach the patches.

use prism_core::prelude::*;
use prism_core::{
    BorderStyle, GradientKind, Interaction, Interactions, Patch, Style, StyleContext, rgba,
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
