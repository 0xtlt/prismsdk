//! Box decorations and interaction states as GTK CSS.
//!
//! GTK 4 draws backgrounds, gradients, per-side and dashed borders and
//! shadows (inset ones too) from CSS, and matches `:hover`, `:active`,
//! `:focus-visible` and `:focus-within` itself: refinements that only change
//! those properties become pseudo-class rules, animated by GTK's CSS
//! transitions. Anything else a state changes (sizes, visibility, opacity,
//! transforms, clipping) goes through prism-native's interaction engine:
//! the widget reports its states and is restyled (see [`needs_engine`]).
//!
//! GTK has no backdrop filter: `backdrop_blur` is not drawn (the translucent
//! background still is).

use prism_core::{BorderStyle, Color, Corners, GradientKind, Interaction, NodeId, Shadow, Style};

/// Whether a color the style paints with has a dark variant (its CSS rule
/// changes with the appearance): background, text, borders (per side too),
/// shadows, gradient stops, state refinements.
pub(crate) fn adapts(s: &Style) -> bool {
    let adaptive = |c: Option<Color>| c.is_some_and(|c| c.dark.is_some());
    adaptive(s.background)
        || adaptive(s.text_color)
        || adaptive(s.border_color)
        || [
            s.border_sides.top,
            s.border_sides.right,
            s.border_sides.bottom,
            s.border_sides.left,
        ]
        .iter()
        .flatten()
        .any(|side| adaptive(side.color))
        || s.shadows.iter().any(|sh| adaptive(Some(sh.color)))
        || s.gradient
            .as_ref()
            .is_some_and(|g| g.stops.iter().any(|st| adaptive(Some(st.color))))
        || s.states.resolved().any(|(_, st)| adapts(st))
}

/// GTK CSS color of the light or dark variant (GTK 4.8 has no `light-dark()`).
pub(crate) fn color(c: Color, dark: bool) -> String {
    let c = if dark {
        c.dark_variant()
    } else {
        c.light_variant()
    };
    let v = |x: f32| (x.clamp(0., 1.) * 255.).round() as u8;
    format!("rgba({},{},{},{})", v(c.r), v(c.g), v(c.b), c.a)
}

/// What the node's main rule paints, as `(property, value)` pairs:
/// background, gradient, corners, borders, shadows, and (when `text` is
/// true: the widget draws its own text) color and font.
pub(crate) fn paint(s: &Style, dark: bool, text: bool) -> Vec<(String, String)> {
    let mut d: Vec<(String, String)> = Vec::new();
    let mut put = |p: &str, v: String| d.push((p.to_string(), v));
    if let Some(bg) = s.background {
        put("background-color", color(bg, dark));
    }
    if let Some(g) = &s.gradient {
        let stops: Vec<String> = g
            .stops
            .iter()
            .map(|st| format!("{} {}%", color(st.color, dark), st.at * 100.))
            .collect();
        let stops = stops.join(", ");
        put(
            "background-image",
            match g.kind {
                GradientKind::Linear { angle } => format!("linear-gradient({angle}deg, {stops})"),
                GradientKind::Radial => {
                    format!("radial-gradient(ellipse farthest-corner, {stops})")
                }
            },
        );
    }
    if s.corner_radius > 0. {
        // Top left, top right, bottom right, bottom left: prism-native
        // already mirrored the corners for right-to-left layouts.
        let r = |c: Corners| {
            if s.rounded_corners.contains(c) {
                s.corner_radius
            } else {
                0.
            }
        };
        put(
            "border-radius",
            format!(
                "{}px {}px {}px {}px",
                r(Corners::TOP_LEADING),
                r(Corners::TOP_TRAILING),
                r(Corners::BOTTOM_TRAILING),
                r(Corners::BOTTOM_LEADING)
            ),
        );
    }
    let style = match s.border_style {
        BorderStyle::Solid => "solid",
        BorderStyle::Dashed => "dashed",
        BorderStyle::Dotted => "dotted",
    };
    let side_color = |c: Option<Color>| c.map_or("currentColor".into(), |c| color(c, dark));
    let sides = s.borders();
    if sides.iter().all(|b| *b == sides[0]) {
        if sides[0].width > 0. {
            put(
                "border",
                format!(
                    "{}px {style} {}",
                    sides[0].width,
                    side_color(sides[0].color)
                ),
            );
        }
    } else {
        // Physical sides: prism-native already swapped leading and trailing
        // for right-to-left layouts.
        for (name, b) in ["top", "right", "bottom", "left"].iter().zip(sides) {
            put(
                &format!("border-{name}"),
                format!("{}px {style} {}", b.width, side_color(b.color)),
            );
        }
    }
    if !s.shadows.is_empty() {
        let shadow = |sh: &Shadow| {
            format!(
                "{}{}px {}px {}px {}px {}",
                if sh.inset { "inset " } else { "" },
                sh.x,
                sh.y,
                sh.blur,
                sh.spread,
                color(sh.color, dark)
            )
        };
        let all: Vec<String> = s.shadows.iter().map(shadow).collect();
        put("box-shadow", all.join(", "));
    }
    if text {
        if let Some(c) = s.text_color {
            put("color", color(c, dark));
        }
        put("font-size", format!("{}px", s.font_size));
        put("font-weight", s.font_weight.css().to_string());
    }
    d
}

/// `paint` as a declaration block.
pub(crate) fn declarations(d: &[(String, String)]) -> String {
    d.iter().map(|(p, v)| format!("{p}:{v};")).collect()
}

/// `st` with everything CSS can draw taken from `base`: equal to `base` when
/// the refinement only changes what [`paint`] writes.
fn without_paint(base: &Style, st: &Style) -> Style {
    Style {
        background: base.background,
        gradient: base.gradient.clone(),
        corner_radius: base.corner_radius,
        rounded_corners: base.rounded_corners,
        border_width: base.border_width,
        border_color: base.border_color,
        border_sides: base.border_sides,
        border_style: base.border_style,
        shadows: base.shadows.clone(),
        text_color: base.text_color,
        font_size: base.font_size,
        font_weight: base.font_weight,
        states: base.states.clone(),
        responsive: Default::default(),
        ..st.clone()
    }
}

/// Whether some refinement of `s` changes more than CSS draws (a size, the
/// visibility, opacity, a transform…): its states must then be reported to
/// prism-native, which restyles the node.
pub(crate) fn needs_engine(s: &Style) -> bool {
    let mut base = s.clone();
    base.responsive = Default::default();
    s.states
        .resolved()
        .any(|(_, st)| without_paint(&base, st) != base)
}

/// Pseudo-class rules for the refinements of node `id`: in each state, the
/// declarations that differ from the base (`unset` for those it drops).
/// `focus_within_is_focus`: the widget's focus is on an inner child (an
/// entry's text): its own focus matches `:focus-within`.
pub(crate) fn state_rules(
    id: NodeId,
    s: &Style,
    dark: bool,
    text: bool,
    focus_within_is_focus: bool,
) -> String {
    if s.states.is_empty() {
        return String::new();
    }
    let base = paint(s, dark, text);
    let node = format!(".p{id}");
    let mut out = String::new();
    for (state, st) in s.states.resolved() {
        let selector = match state {
            Interaction::Hover => format!("{node}:hover"),
            Interaction::Active => format!("{node}:active"),
            Interaction::Focus if focus_within_is_focus => format!("{node}:focus-within"),
            Interaction::Focus => format!("{node}:focus-visible"),
            Interaction::FocusWithin => format!("{node}:focus-within"),
            Interaction::GroupHover => format!(".prism-group:hover {node}"),
            Interaction::GroupFocusWithin => format!(".prism-group:focus-within {node}"),
            Interaction::GroupActive => format!(".prism-group:active {node}"),
            // Applied by prism-core before the backend sees the style.
            Interaction::Disabled => continue,
        };
        let now = paint(st, dark, text);
        let mut body = String::new();
        for (p, v) in &now {
            if !base.iter().any(|(bp, bv)| bp == p && bv == v) {
                body += &format!("{p}:{v};");
            }
        }
        for (p, _) in &base {
            if !now.iter().any(|(np, _)| np == p) {
                body += &format!("{p}:unset;");
            }
        }
        if !body.is_empty() {
            out += &format!("{selector}{{{body}}}\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::{Gradient, StyleContext, Styled, rgb, rgba};

    fn resolved(s: Style) -> Style {
        s.resolve(StyleContext::default())
    }

    #[test]
    fn paints_gradients_shadows_and_borders_in_gtk_css() {
        let s = Style::default()
            .bg(rgb(0xFFFFFF))
            .bg_gradient(Gradient::linear(90., [rgb(0xFF0000), rgb(0x0000FF)]))
            .shadow(Shadow::new(0., 4., 12., rgba(0x000000, 0.25)))
            .shadow(
                Shadow::new(0., 1., 2., rgba(0x000000, 0.5))
                    .inset()
                    .spread(1.),
            )
            .border(2.)
            .border_color(rgb(0x00FF00))
            .border_dashed();
        let css = declarations(&paint(&s, false, false));
        assert!(
            css.contains("background-color:rgba(255,255,255,1);"),
            "{css}"
        );
        assert!(css.contains(
            "background-image:linear-gradient(90deg, rgba(255,0,0,1) 0%, rgba(0,0,255,1) 100%);"
        ));
        assert!(css.contains(
            "box-shadow:0px 4px 12px 0px rgba(0,0,0,0.25), inset 0px 1px 2px 1px rgba(0,0,0,0.5);"
        ));
        assert!(css.contains("border:2px dashed rgba(0,255,0,1);"));
        assert!(!css.contains("font-size"), "views draw no text");
        let radial = Style::default().bg_gradient(Gradient::radial([rgb(0xFFFFFF), rgb(0)]));
        assert!(
            declarations(&paint(&radial, false, false))
                .contains("radial-gradient(ellipse farthest-corner, rgba(255,255,255,1) 0%")
        );
    }

    #[test]
    fn per_side_borders_and_dark_variants() {
        let c = prism_core::adaptive(rgb(0x000000), rgb(0xFFFFFF));
        let s = Style::default().border_bottom(1., c).border_dotted();
        let light = declarations(&paint(&s, false, false));
        assert!(
            light.contains("border-top:0px dotted currentColor;"),
            "{light}"
        );
        assert!(light.contains("border-bottom:1px dotted rgba(0,0,0,1);"));
        let dark = declarations(&paint(&s, true, false));
        assert!(dark.contains("border-bottom:1px dotted rgba(255,255,255,1);"));
    }

    #[test]
    fn adaptive_colors_are_found_wherever_they_paint() {
        let c = prism_core::adaptive(rgb(0xFFFFFF), rgb(0x000000));
        assert!(!adapts(&Style::default().bg(rgb(0xFFFFFF)).border(1.)));
        assert!(adapts(&Style::default().border(1.).border_color(c)));
        assert!(adapts(&Style::default().border_bottom(1., c)));
        assert!(adapts(&Style::default().shadow(Shadow::new(0., 1., 2., c))));
        assert!(adapts(
            &Style::default().bg_gradient(Gradient::linear(90., [rgb(0), c]))
        ));
        assert!(adapts(&resolved(
            Style::default().hover(move |s| s.border_color(c))
        )));
    }

    #[test]
    fn states_become_pseudo_class_rules_with_only_their_changes() {
        let s = resolved(
            Style::default()
                .bg(rgb(0xFFFFFF))
                .shadow(Shadow::new(0., 1., 2., rgba(0, 0.2)))
                .hover(|s| s.bg(rgb(0xEEEEEE)))
                .active(|s| s.shadow_none())
                .focus_within(|s| s.border(2.).border_color(rgb(0x0000FF)))
                .group_hover(|s| s.text_color(rgb(0xFF0000))),
        );
        let rules = state_rules(7, &s, false, true, false);
        assert!(
            rules.contains(".p7:hover{background-color:rgba(238,238,238,1);}"),
            "{rules}"
        );
        assert!(rules.contains(".p7:active{box-shadow:unset;}"));
        assert!(rules.contains(".p7:focus-within{border:2px solid rgba(0,0,255,1);}"));
        assert!(rules.contains(".prism-group:hover .p7{color:rgba(255,0,0,1);}"));
        // Group states first, the node's own after: later rules win.
        assert!(rules.find(".prism-group").unwrap() < rules.find(":hover{").unwrap());
        assert!(!needs_engine(&s), "colors, borders and shadows are CSS");
        // An entry's focus is on its inner text: `:focus-within`.
        let input = resolved(Style::default().focus(|s| s.bg(rgb(0xFFFF00))));
        assert!(state_rules(3, &input, false, true, true).starts_with(".p3:focus-within{"));
        assert!(state_rules(3, &input, false, true, false).starts_with(".p3:focus-visible{"));
    }

    #[test]
    fn layout_visibility_and_opacity_changes_need_the_engine() {
        let hidden = resolved(
            Style::default()
                .with_style(|s| s.display_none = true)
                .group_focus_within(|s| s.with_style(|s| s.display_none = false)),
        );
        assert!(needs_engine(&hidden));
        assert!(needs_engine(&resolved(
            Style::default().active(|s| s.opacity(0.7))
        )));
        assert!(needs_engine(&resolved(Style::default().hover(|s| s.p(4.)))));
        assert!(!needs_engine(&resolved(Style::default().bg(rgb(0)))));
    }
}
