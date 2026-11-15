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
    // `Color::current()` against the node's own text color when it has one
    // (`currentColor` is inherited on the web, not on native platforms).
    let resolved;
    let s = match s.text_color {
        Some(text) if !text.is_current() => {
            resolved = s.with_current_color(text);
            &resolved
        }
        _ => s,
    };
    let mut c = String::from("box-sizing:border-box;min-width:0;");
    let w = &mut c;
    // Hidden, but shown in some interaction state: a plain `display:none`
    // last, which the state's rule can override (`!important` inline can't be).
    let shown_in_a_state = s.display_none && s.states.resolved().any(|(_, st)| !st.display_none);
    if s.display_none && !shown_in_a_state {
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
    decorations(w, kind, s);
    if s.opacity < 1. {
        let _ = write!(w, "opacity:{};", s.opacity);
    }
    if !s.transform.is_identity() {
        let t = s.transform;
        // Offsets are leading/trailing: `--prism-dir` is -1 right to left.
        let _ = write!(
            w,
            "transform:translate(calc(var(--prism-dir,1)*{}px),{}px) rotate(calc(var(--prism-dir,1)*{}deg)) scale({});",
            t.x, t.y, t.rotate, t.scale
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
                    if has_border(s) { "" } else { "none" },
                    6. + p.top,
                    14. + p.left
                );
            }
        }
        NodeKind::TextArea if s.border_width > 0. => w.push_str("font-family:inherit;"),
        NodeKind::TextInput => {
            w.push_str("font-family:inherit;padding:6px 8px;");
            if !has_border(s) {
                w.push_str("border:1px solid rgba(128,128,128,.45);border-radius:6px;");
            }
        }
        _ => {}
    }
    if let Some(cursor) = s.cursor {
        let _ = write!(w, "cursor:{};", cursor.css());
    }
    if shown_in_a_state {
        w.push_str("display:none;");
    }
    c
}

fn has_border(s: &Style) -> bool {
    s.borders().iter().any(|b| b.width > 0.)
}

/// Borders (uniform or per side, solid, dashed or dotted), shadows,
/// clipping, gradient and backdrop blur.
fn decorations(w: &mut String, kind: NodeKind, s: &Style) {
    let sides = s.borders();
    let style = s.border_style.css();
    let color = |c: Option<prism_core::Color>| c.map_or("currentColor".into(), |c| c.to_css());
    if sides.iter().all(|b| *b == sides[0]) {
        if sides[0].width > 0. {
            let _ = write!(
                w,
                "border:{}px {style} {};",
                sides[0].width,
                color(sides[0].color)
            );
        }
    } else {
        // Logical sides: left/right are leading/trailing.
        let [t, r, b, l] = sides;
        for (prop, side) in [
            ("border-block-start", t),
            ("border-inline-end", r),
            ("border-block-end", b),
            ("border-inline-start", l),
        ] {
            if side.width > 0. {
                let _ = write!(w, "{prop}:{}px {style} {};", side.width, color(side.color));
            }
        }
    }
    if !s.shadows.is_empty() {
        let shadows: Vec<String> = s.shadows.iter().map(|sh| sh.to_css()).collect();
        let _ = write!(w, "box-shadow:{};", shadows.join(","));
    }
    // A scroll view scrolls (its class sets the overflow).
    if s.clip && kind != NodeKind::Scroll {
        w.push_str("overflow:hidden;");
    }
    if let Some(g) = &s.gradient {
        let _ = write!(w, "background-image:{};", g.to_css());
    }
    if s.backdrop_blur > 0. {
        let _ = write!(
            w,
            "-webkit-backdrop-filter:blur({0}px);backdrop-filter:blur({0}px);",
            s.backdrop_blur
        );
    }
}

/// The interaction refinements of node `id` as CSS rules (pseudo-classes
/// on `[data-pid]`): for each state, the declarations its style changes,
/// `!important` to win over the inline style. Empty when it has none.
/// Rules come in the order states apply, so later ones win, like natively.
pub fn states_css(kind: NodeKind, id: prism_core::NodeId, s: &Style) -> String {
    use prism_core::Interaction;
    if s.states.is_empty() {
        return String::new();
    }
    // Each property's effective (last) value.
    let decls = |css: &str| -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (p, v) in css.split(';').filter_map(|d| d.split_once(':')) {
            match out.iter_mut().find(|(q, _)| q == p) {
                Some(slot) => slot.1 = v.to_string(),
                None => out.push((p.to_string(), v.to_string())),
            }
        }
        out
    };
    let base = decls(&css(kind, s));
    let mut out = String::new();
    for (state, st) in s.states.resolved() {
        let node = format!(r#"[data-pid="{id}"]"#);
        let (selector, media) = match state {
            // Touch screens keep `:hover` after a tap: pointers only.
            Interaction::Hover => (format!("{node}:hover"), true),
            Interaction::FocusWithin => (format!("{node}:focus-within"), false),
            Interaction::Focus => (format!("{node}:focus-visible"), false),
            Interaction::Active => (format!("{node}:active"), false),
            Interaction::GroupHover => (format!("[data-group]:hover {node}"), true),
            Interaction::GroupFocusWithin => (format!("[data-group]:focus-within {node}"), false),
            Interaction::GroupActive => (format!("[data-group]:active {node}"), false),
            // Resolved by the server (the disabled refinement is applied).
            Interaction::Disabled => continue,
        };
        let now = decls(&css(kind, st));
        let mut body = String::new();
        for (p, v) in &now {
            if !base.iter().any(|(bp, bv)| bp == p && bv == v) {
                let _ = write!(body, "{p}:{v} !important;");
            }
        }
        // Gone in this state: back to the initial value.
        for (p, _) in &base {
            if !now.iter().any(|(np, _)| np == p) {
                let _ = write!(body, "{p}:unset !important;");
            }
        }
        if body.is_empty() {
            continue;
        }
        let rule = format!("{selector}{{{body}}}");
        if media {
            let _ = write!(out, "@media (hover:hover){{{rule}}}");
        } else {
            out.push_str(&rule);
        }
    }
    out
}

/// Enter, exit and loop animations for the client (Web Animations API), or
/// null: `{rest, enter, exit, repeat, t, xt}` with poses `{o, tf: [x, y,
/// scale, degrees]}` and transitions `{d, delay, ease}`.
pub fn motion(s: &Style) -> serde_json::Value {
    use prism_core::animation::{DEFAULT_ENTER, DEFAULT_EXIT};
    use prism_core::{Pose, Transition};
    use serde_json::{Value, json};
    let a = s.animations;
    if a.is_empty() {
        return Value::Null;
    }
    let pose = |p: Pose| {
        let (o, t) = p.apply(s.opacity, s.transform);
        json!({ "o": o, "tf": [t.x, t.y, t.scale, t.rotate] })
    };
    let anim = |t: Transition| json!({ "d": t.duration, "delay": t.delay, "ease": t.css_easing() });
    let t = s.transition;
    json!({
        "rest": pose(Pose::new()),
        "enter": a.enter.map(pose),
        "exit": a.exit.map(pose),
        "repeat": a.repeat.map(|(p, t)| json!({ "to": pose(p), "t": anim(t) })),
        "t": anim(t.unwrap_or(DEFAULT_ENTER)),
        "xt": anim(t.map_or(DEFAULT_EXIT, |t| Transition { delay: 0., ..t })),
    })
}
