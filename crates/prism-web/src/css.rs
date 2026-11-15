//! Prism style → inline CSS. Mirrors `prism_native::to_taffy` so both agree.

use std::fmt::Write;

use prism_core::{Align, FlexDirection, Justify, Length, NodeKind, Position, Style, TextAlign};

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
    if s.display_none {
        w.push_str("display:none !important;");
    }
    match kind {
        NodeKind::Text => w.push_str("display:block;white-space:pre-wrap;overflow-wrap:anywhere;"),
        NodeKind::Timer => w.push_str("display:block;font-variant-numeric:tabular-nums;"),
        NodeKind::Image => w.push_str("display:inline-flex;"),
        NodeKind::Progress | NodeKind::Slider => w.push_str("display:block;"),
        NodeKind::Spinner => w.push_str("display:inline-block;"),
        NodeKind::Picker | NodeKind::DatePicker | NodeKind::TextArea => {
            w.push_str("display:block;")
        }
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
    if s.position == Position::Absolute {
        // Logical insets: left/right are leading/trailing, like margins.
        w.push_str("position:absolute;");
        for (prop, v) in [
            ("inset-block-start", s.inset.top),
            ("inset-block-end", s.inset.bottom),
            ("inset-inline-start", s.inset.left),
            ("inset-inline-end", s.inset.right),
        ] {
            if let Some(v) = len(v) {
                let _ = write!(w, "{prop}:{v};");
            }
        }
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
    if let Some(r) = s.aspect_ratio {
        let _ = write!(w, "aspect-ratio:{r};");
    }

    if let Some(bg) = s.background {
        let _ = write!(w, "background:{};", bg.to_css());
    }
    if s.corner_radius > 0. {
        let _ = write!(w, "border-radius:{}px;", s.corner_radius);
        // Square corners, as logical properties (they follow `dir`).
        for (corner, prop) in [
            (
                prism_core::Corners::TOP_LEADING,
                "border-start-start-radius",
            ),
            (prism_core::Corners::TOP_TRAILING, "border-start-end-radius"),
            (
                prism_core::Corners::BOTTOM_TRAILING,
                "border-end-end-radius",
            ),
            (
                prism_core::Corners::BOTTOM_LEADING,
                "border-end-start-radius",
            ),
        ] {
            if !s.rounded_corners.contains(corner) {
                let _ = write!(w, "{prop}:0;");
            }
        }
    }
    if s.border_width > 0. {
        let color = s.border_color.map_or("currentColor".into(), |c| c.to_css());
        let _ = write!(w, "border:{}px solid {color};", s.border_width);
    }
    if s.opacity < 1. {
        let _ = write!(w, "opacity:{};", s.opacity);
    }
    if s.translate_percent != (0., 0.) {
        let (t, (px, py)) = (s.transform, s.translate_percent);
        // Percentages of the element's own size (label anchoring).
        let _ = write!(
            w,
            "transform:translate(calc(var(--prism-dir,1)*({}px + {px}%)),calc({}px + {py}%)) rotate(calc(var(--prism-dir,1)*{}deg)) {};",
            t.x,
            t.y,
            t.rotate,
            t.css_scale()
        );
    } else if !s.transform.is_identity() {
        let t = s.transform;
        // Offsets are leading/trailing: `--prism-dir` is -1 right to left.
        let _ = write!(
            w,
            "transform:translate(calc(var(--prism-dir,1)*{}px),{}px) rotate(calc(var(--prism-dir,1)*{}deg)) {};",
            t.x,
            t.y,
            t.rotate,
            t.css_scale()
        );
    }
    if let Some(t) = s.transition {
        let _ = write!(
            w,
            "transition:all {}s {} {}s;",
            t.duration,
            t.css_easing(),
            t.delay
        );
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
    if let (
        NodeKind::Progress | NodeKind::Slider | NodeKind::DatePicker | NodeKind::Spinner,
        Some(c),
    ) = (kind, s.text_color)
    {
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
        NodeKind::TextArea if s.border_width > 0. => w.push_str("font-family:inherit;"),
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

/// Enter, exit and loop animations for the client (Web Animations API), or
/// null: `{rest, enter, exit, repeat, t, xt}` with poses `{o, tf: [x, y,
/// scale, degrees]}` (then `scale x, scale y, x %, y %` when the scale is
/// not uniform or the style has a translation in percent), transitions `{d,
/// delay, ease}`, and `repeat.restart` for loops that start over.
pub fn motion(s: &Style) -> serde_json::Value {
    use prism_core::animation::{DEFAULT_ENTER, DEFAULT_EXIT};
    use prism_core::{Pose, Transition};
    use serde_json::{Value, json};
    let a = s.animations;
    if a.is_empty() {
        return Value::Null;
    }
    let (px, py) = s.translate_percent;
    let pose = |p: Pose| {
        let (o, t) = p.apply(s.opacity, s.transform);
        if (t.scale_x, t.scale_y, px, py) == (1., 1., 0., 0.) {
            json!({ "o": o, "tf": [t.x, t.y, t.scale, t.rotate] })
        } else {
            json!({ "o": o, "tf": [t.x, t.y, t.scale, t.rotate, t.scale_x, t.scale_y, px, py] })
        }
    };
    let anim = |t: Transition| json!({ "d": t.duration, "delay": t.delay, "ease": t.css_easing() });
    let t = s.transition;
    json!({
        "rest": pose(Pose::new()),
        "enter": a.enter.map(pose),
        "exit": a.exit.map(pose),
        "repeat": a.repeat.map(|(p, t)| {
            if a.restart {
                // Starts over with its own curve (linear: a steady spin);
                // springs would jump at every restart.
                let t = match t.curve {
                    prism_core::Curve::Spring => Transition { curve: prism_core::Curve::EaseInOut, ..t },
                    _ => t,
                };
                json!({ "to": pose(p), "t": anim(t), "restart": true })
            } else {
                json!({ "to": pose(p), "t": anim(t) })
            }
        }),
        "t": anim(t.unwrap_or(DEFAULT_ENTER)),
        "xt": anim(t.map_or(DEFAULT_EXIT, |t| Transition { delay: 0., ..t })),
    })
}
