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
    if kind == NodeKind::View && (p.top, p.right, p.bottom, p.left) != (0., 0., 0., 0.) {
        let _ = write!(
            w,
            "padding:{}px {}px {}px {}px;",
            p.top, p.right, p.bottom, p.left
        );
    }
    let m = s.margin;
    let _ = write!(
        w,
        "margin:{}px {}px {}px {}px;",
        m.top, m.right, m.bottom, m.left
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

    if kind != NodeKind::View {
        let _ = write!(
            w,
            "font-size:{}px;font-weight:{};",
            s.font_size,
            s.font_weight.css()
        );
        if let Some(c) = s.text_color {
            let _ = write!(w, "color:{};", c.to_css());
        }
        let ta = match s.text_align {
            TextAlign::Left => "left",
            TextAlign::Center => "center",
            TextAlign::Right => "right",
        };
        let _ = write!(w, "text-align:{ta};");
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
