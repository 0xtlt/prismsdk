//! Prism style → inline CSS. Mirrors `prism_native::to_taffy` so both agree.

use std::fmt::Write;

use prism_core::{Align, FlexDirection, Justify, Length, NodeKind, Style, TextAlign};

fn len(l: Length) -> Option<String> {
    match l {
        Length::Auto => None,
        Length::Px(v) => Some(format!("{v}px")),
        Length::Fraction(f) => Some(format!("{}%", f * 100.)),
    }
}

fn align(a: Align) -> &'static str {
    match a {
        Align::Start => "flex-start",
        Align::Center => "center",
        Align::End => "flex-end",
        Align::Stretch => "stretch",
    }
}

pub fn css(kind: NodeKind, s: &Style) -> String {
    let mut c = String::from("box-sizing:border-box;min-width:0;");
    let w = &mut c;
    match kind {
        NodeKind::Text => w.push_str("display:block;white-space:pre-wrap;overflow-wrap:anywhere;"),
        NodeKind::Timer => w.push_str("display:block;font-variant-numeric:tabular-nums;"),
        NodeKind::Image => w.push_str("display:inline-flex;"),
        NodeKind::Progress => w.push_str("display:block;"),
        _ => {
            let dir = match s.direction {
                FlexDirection::Row => "row",
                FlexDirection::Column => "column",
            };
            let _ = write!(w, "display:flex;flex-direction:{dir};");
        }
    }
    if s.wrap {
        w.push_str("flex-wrap:wrap;");
    }
    let _ = write!(w, "align-items:{};", align(s.align_items));
    if let Some(a) = s.align_self {
        let _ = write!(w, "align-self:{};", align(a));
    }
    let justify = match s.justify_content {
        Justify::Start => "flex-start",
        Justify::Center => "center",
        Justify::End => "flex-end",
        Justify::SpaceBetween => "space-between",
        Justify::SpaceAround => "space-around",
        Justify::SpaceEvenly => "space-evenly",
    };
    let _ = write!(w, "justify-content:{justify};");
    if s.gap > 0. {
        let _ = write!(w, "gap:{}px;", s.gap);
    }
    let p = s.padding;
    let container = matches!(
        kind,
        NodeKind::View
            | NodeKind::Stack
            | NodeKind::Screen
            | NodeKind::Scroll
            | NodeKind::Tabs
            | NodeKind::Tab
    );
    // Logical sides: left/right are leading/trailing, so `dir=rtl` mirrors
    // them like native backends do.
    if container && (p.top, p.right, p.bottom, p.left) != (0., 0., 0., 0.) {
        let _ = write!(
            w,
            "padding-block:{}px {}px;padding-inline:{}px {}px;",
            p.top, p.bottom, p.left, p.right
        );
    }
    let m = s.margin;
    let _ = write!(
        w,
        "margin-block:{}px {}px;margin-inline:{}px {}px;",
        m.top, m.bottom, m.left, m.right
    );
    for (prop, v) in [
        ("width", s.width),
        ("height", s.height),
        ("min-width", s.min_width),
        ("min-height", s.min_height),
        ("max-width", s.max_width),
        ("max-height", s.max_height),
    ] {
        if let Some(v) = len(v) {
            let _ = write!(w, "{prop}:{v};");
        }
    }
    let basis = len(s.flex_basis).unwrap_or_else(|| "auto".into());
    let _ = write!(w, "flex:{} {} {basis};", s.flex_grow, s.flex_shrink);

    if let Some(bg) = s.background {
        let _ = write!(w, "background:{};", bg.to_css());
    }
    if s.corner_radius > 0. {
        let _ = write!(w, "border-radius:{}px;", s.corner_radius);
    }
    if s.border_width > 0. {
        let color = s.border_color.map_or("currentColor".into(), |c| c.to_css());
        let _ = write!(w, "border:{}px solid {color};", s.border_width);
    }
    if s.opacity < 1. {
        let _ = write!(w, "opacity:{};", s.opacity);
    }
    if let Some(t) = s.transition {
        let curve = match t.curve {
            prism_core::Curve::EaseInOut => "ease-in-out",
            prism_core::Curve::Linear => "linear",
            prism_core::Curve::Spring => "cubic-bezier(.34,1.56,.64,1)",
        };
        let _ = write!(w, "transition:all {}s {curve};", t.duration);
    }

    if kind != NodeKind::View {
        // rem: follows the browser's text size setting (web Dynamic Type).
        let _ = write!(
            w,
            "font-size:{}rem;font-weight:{};",
            s.font_size / 16.,
            s.font_weight.css()
        );
        if let Some(c) = s.text_color {
            let _ = write!(w, "color:{};", c.to_css());
        }
        let ta = match s.text_align {
            TextAlign::Left => "start",
            TextAlign::Center => "center",
            TextAlign::Right => "end",
        };
        let _ = write!(w, "text-align:{ta};");
    }
    if let (NodeKind::Progress, Some(c)) = (kind, s.text_color) {
        let _ = write!(w, "accent-color:{};", c.to_css());
    }
    match kind {
        NodeKind::Button => {
            w.push_str("font-family:inherit;cursor:pointer;justify-content:center;");
            if s.background.is_some() {
                let _ = write!(
                    w,
                    "border:{};padding:{}px {}px;",
                    if s.border_width > 0. { "" } else { "none" },
                    6. + p.top,
                    14. + p.left
                );
            }
        }
        NodeKind::TextInput => {
            w.push_str("font-family:inherit;padding:6px 8px;");
            if s.border_width == 0. {
                w.push_str("border:1px solid rgba(128,128,128,.45);border-radius:6px;");
            }
        }
        _ => {}
    }
    c
}
