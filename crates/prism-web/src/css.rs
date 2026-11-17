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
        Align::Baseline => "baseline",
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
    // An overlay takes no room: its content is presented in the top layer.
    if kind == NodeKind::Overlay {
        return "display:contents;".into();
    }
    // Flex items shrink below their content (as on native, see
    // `prism_native::to_taffy`), except switches and spinners, which can't
    // (`prism.css` gives them their drawn width as a minimum).
    let mut c = String::from(if matches!(kind, NodeKind::Toggle | NodeKind::Spinner) {
        "box-sizing:border-box;"
    } else {
        "box-sizing:border-box;min-width:0;"
    });
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
        NodeKind::Picker | NodeKind::DatePicker | NodeKind::TextArea | NodeKind::RichInput => {
            w.push_str("display:block;")
        }
        // The tab bar is above the panels, or a sidebar beside them on wide
        // windows: `prism.css` decides (a media query), so no direction here
        // (a node's rules would win over it).
        NodeKind::Tabs => w.push_str("display:flex;"),
        _ if s.grid => w.push_str("display:grid;justify-items:stretch;"),
        // A box and its label on one line.
        NodeKind::Checkbox => w.push_str("display:flex;flex-direction:row;"),
        _ => {
            let dir = match s.direction {
                FlexDirection::Row => "row",
                FlexDirection::Column => "column",
                FlexDirection::RowReverse => "row-reverse",
                FlexDirection::ColumnReverse => "column-reverse",
            };
            let _ = write!(w, "display:flex;flex-direction:{dir};");
        }
    }
    if s.wrap {
        w.push_str("flex-wrap:wrap;");
    }
    let items = match kind {
        NodeKind::Checkbox => "center",
        _ => align(s.align_items),
    };
    let _ = write!(w, "align-items:{items};");
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
    // Every kind of node: a text, input or button grows by it and its
    // content is inset by it (on a button, input or select it replaces the
    // control's own inset). Logical sides: left/right are leading/trailing,
    // so `dir=rtl` mirrors them like native backends do. An explicit zero
    // (`.p(0.)`) counts: no inset at all (#69).
    if s.has_padding() {
        // A select keeps room for its arrow after the text.
        let end = if kind == NodeKind::Picker {
            p.right + 22.
        } else {
            p.right
        };
        let _ = write!(
            w,
            "padding-block:{}px {}px;padding-inline:{}px {end}px;",
            p.top, p.bottom, p.left
        );
    }
    if s.position != Position::Relative {
        // Logical insets: left/right are leading/trailing, like margins.
        w.push_str(match s.position {
            // Above the content that scrolls under it.
            Position::Sticky => "position:sticky;z-index:1;",
            Position::Fixed => "position:fixed;z-index:50;",
            _ => "position:absolute;",
        });
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
    if s.order != 0 {
        let _ = write!(w, "order:{};", s.order);
    }
    layout_extras(kind, s, w);

    if let Some(bg) = s.background {
        // A picker keeps its drawn arrow, an input its leading icon (a
        // background image).
        let prop = if matches!(kind, NodeKind::Picker | NodeKind::TextInput) {
            "background-color"
        } else {
            "background"
        };
        let _ = write!(w, "{prop}:{};", bg.to_css());
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
    // Clicks and hovers go through (and its subtree's, unless re-enabled).
    if !s.hit_test {
        w.push_str("pointer-events:none;");
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
        NodeKind::Progress
        | NodeKind::Slider
        | NodeKind::DatePicker
        | NodeKind::Spinner
        | NodeKind::Checkbox
        | NodeKind::RadioGroup,
        Some(c),
    ) = (kind, s.text_color)
    {
        let _ = write!(w, "accent-color:{};", c.to_css());
    }
    match kind {
        NodeKind::Button => {
            w.push_str("font-family:inherit;cursor:pointer;justify-content:center;");
            if s.background.is_some() && !has_border(s) {
                w.push_str("border:none;");
            }
            // A filled button's own inset, unless the app sets one.
            if s.background.is_some() && !s.has_padding() {
                w.push_str("padding:6px 14px;");
            }
        }
        NodeKind::TextArea => {
            if s.border_width > 0. {
                w.push_str("font-family:inherit;");
            }
            // Its padding replaces the default one, which `prism.css`'s
            // three-line minimum counts (#69).
            if s.has_padding() && s.min_height == Length::Auto {
                w.push_str("min-height:3lh;");
            }
        }
        NodeKind::TextInput => {
            w.push_str("font-family:inherit;");
            // The field's own inset and border, unless the app sets them
            // (`.border(0.)`: none at all).
            if !s.has_padding() {
                w.push_str("padding:6px 8px;");
            } else {
                // An icon field keeps room for its icon (`prism.css`).
                let _ = write!(w, "--prism-pl:{}px;", p.left);
            }
            if !has_border(s) && !s.border_none {
                w.push_str("border:1px solid rgba(128,128,128,.45);border-radius:6px;");
            }
        }
        _ => {}
    }
    // `.border(0.)`: not even the default border of a field.
    if s.border_none
        && !has_border(s)
        && matches!(
            kind,
            NodeKind::TextInput
                | NodeKind::TextArea
                | NodeKind::RichInput
                | NodeKind::Picker
                | NodeKind::DatePicker
                | NodeKind::Button
        )
        // (A filled button has none already.)
        && !(kind == NodeKind::Button && s.background.is_some())
    {
        w.push_str("border:none;");
    }
    // A field that draws its own box (padding, a background, a border,
    // `.border(0.)`) or its own focus look skips the page's `:focus-visible`
    // outline (`prism.css`), as natively: the caret shows the focus (#52).
    if matches!(
        kind,
        NodeKind::TextInput | NodeKind::TextArea | NodeKind::RichInput
    ) && own_field_look(s)
    {
        w.push_str("outline:none;");
    }
    if kind != NodeKind::View {
        typography(kind, s, w);
    }
    if let Some(cursor) = s.cursor {
        let _ = write!(w, "cursor:{};", cursor.css());
    }
    if shown_in_a_state {
        w.push_str("display:none;");
    }
    // The selected tab, the header's buttons (#30): `prism.css` reads it.
    if let Some(tint) = s.tint
        && matches!(kind, NodeKind::Tabs | NodeKind::Stack)
    {
        let _ = write!(w, "--prism-tint:{};", tint.to_css());
    }
    c
}

/// Font family, italic, spacing, case, decoration and truncation (only
/// what differs from the defaults). Sizes in rem, like `font-size`, so they
/// follow the browser's text size.
pub fn typography(kind: NodeKind, s: &Style, w: &mut String) {
    use prism_core::{FontFamily, TextTransform};
    if s.font_family != FontFamily::System {
        let _ = write!(w, "font-family:{};", s.font_family.css());
    }
    if s.italic {
        w.push_str("font-style:italic;");
    }
    match s.line_height {
        Length::Auto => {}
        Length::Px(v) => {
            let _ = write!(w, "line-height:{}rem;", v / 16.);
        }
        Length::Fraction(f) => {
            let _ = write!(w, "line-height:{f};");
        }
    }
    if s.letter_spacing != 0. {
        let _ = write!(w, "letter-spacing:{}rem;", s.letter_spacing / 16.);
    }
    if s.text_transform != TextTransform::None {
        let _ = write!(w, "text-transform:{};", s.text_transform.css());
    }
    match (s.underline, s.strikethrough) {
        (true, true) => w.push_str("text-decoration-line:underline line-through;"),
        (true, false) => w.push_str("text-decoration-line:underline;"),
        (false, true) => w.push_str("text-decoration-line:line-through;"),
        (false, false) => {}
    }
    if s.tabular_numbers {
        w.push_str("font-variant-numeric:tabular-nums;");
    }
    if kind != NodeKind::Text {
        return;
    }
    // Before `white-space`, which keeps one line when the text never wraps.
    if s.text_wrap != prism_core::TextWrap::Auto {
        let _ = write!(w, "text-wrap:{};", s.text_wrap.css());
    }
    match s.max_lines {
        // One line: cut mid-word with an ellipsis, like native labels.
        Some(1) => w.push_str("white-space:pre;overflow:hidden;text-overflow:ellipsis;"),
        Some(n) => {
            let _ = write!(
                w,
                "display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:{n};line-clamp:{n};overflow:hidden;"
            );
        }
        None if s.no_wrap => w.push_str("white-space:pre;overflow:hidden;"),
        None => {}
    }
}

/// Inline CSS of one rich-text run (see `prism_core::spans`).
pub fn run_css(r: &prism_core::TextRun) -> String {
    let mut w = String::new();
    if let Some(weight) = r.weight {
        let _ = write!(w, "font-weight:{};", weight.css());
    }
    if r.italic {
        w.push_str("font-style:italic;");
    }
    match (r.underline, r.strikethrough) {
        (true, true) => w.push_str("text-decoration-line:underline line-through;"),
        (true, false) => w.push_str("text-decoration-line:underline;"),
        (false, true) => w.push_str("text-decoration-line:line-through;"),
        (false, false) => {}
    }
    if let Some(c) = r.color {
        let _ = write!(w, "color:{};", c.to_css());
    }
    if let Some(c) = r.background {
        let _ = write!(w, "background-color:{};border-radius:3px;", c.to_css());
    }
    if let Some(f) = &r.family {
        let _ = write!(w, "font-family:{};", f.css());
    }
    if let Some(z) = r.size {
        let _ = write!(w, "font-size:{z}em;");
    }
    w
}

/// Whether a text field draws Prism's box instead of the browser's (the
/// Layout chapter) or refines its focused look.
fn own_field_look(s: &Style) -> bool {
    use prism_core::Interaction;
    s.has_padding()
        || s.background.is_some()
        || has_border(s)
        || s.border_none
        || s.states
            .resolved()
            .any(|(i, _)| matches!(i, Interaction::Focus | Interaction::FocusWithin))
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
    let node = format!(r#"[data-pid="{id}"]"#);
    // Refinements that follow a named group ancestor (#212).
    let named = s.states.resolved_named().filter_map(|(state, name, st)| {
        let group = format!(r#"[data-gn="{}"]"#, css_string(name));
        let (selector, media) = match state {
            Interaction::GroupHover => (format!("{group}:hover {node}"), true),
            Interaction::GroupFocusWithin => (format!("{group}:focus-within {node}"), false),
            Interaction::GroupActive => (format!("{group}:active {node}"), false),
            _ => return None,
        };
        Some((selector, media, st))
    });
    let unnamed = s.states.resolved().filter_map(|(state, st)| {
        let (selector, media) = match state {
            // Touch screens keep `:hover` after a tap: pointers only.
            Interaction::Hover => (format!("{node}:hover"), true),
            Interaction::FocusWithin => (format!("{node}:focus-within"), false),
            Interaction::Focus => (format!("{node}:focus-visible"), false),
            Interaction::Active => (format!("{node}:active"), false),
            Interaction::GroupHover => (format!("[data-group]:hover {node}"), true),
            Interaction::GroupFocusWithin => (format!("[data-group]:focus-within {node}"), false),
            Interaction::GroupActive => (format!("[data-group]:active {node}"), false),
            // Set by the client while accepted files are over it (#111).
            Interaction::DropHover => (format!("{node}[data-prism-drop]"), false),
            // Resolved by the server (the disabled refinement is applied).
            Interaction::Disabled => return None,
        };
        Some((selector, media, st))
    });
    // Group states apply first (`Interaction::ALL`), named and unnamed.
    let (groups, own): (Vec<_>, Vec<_>) =
        unnamed.partition(|(sel, ..)| sel.starts_with("[data-group]"));
    for (selector, media, st) in groups.into_iter().chain(named).chain(own) {
        let now = decls(&css(kind, st));
        let mut body = String::new();
        for (p, v) in &now {
            if !base.iter().any(|(bp, bv)| bp == p && bv == v) {
                let _ = write!(body, "{p}:{v} !important;");
            }
        }
        // Gone in this state: back to the initial value.
        for (p, _) in &base {
            // (A state's style has no states of its own: the base decides
            // whether a field keeps the focus outline.)
            if p != "outline" && !now.iter().any(|(np, _)| np == p) {
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

/// `s` inside a double-quoted CSS string: quotes, backslashes, angle
/// brackets and control characters escaped.
fn css_string(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            // `<` too: the rules may sit in a `<style>` element.
            c if c.is_control() || c == '<' || c == '>' => {
                let _ = write!(out, "\\{:x} ", c as u32);
            }
            c => out.push(c),
        }
    }
    out
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

/// Grid, per-axis gaps, aspect ratio and scroll axis (`crate::layout` in
/// prism-core), after the flexbox properties.
fn layout_extras(kind: NodeKind, s: &Style, w: &mut String) {
    let tracks = |t: &[prism_core::GridTrack]| {
        t.iter()
            .map(prism_core::GridTrack::css)
            .collect::<Vec<_>>()
            .join(" ")
    };
    if s.grid {
        if !s.grid_columns.is_empty() {
            let _ = write!(w, "grid-template-columns:{};", tracks(&s.grid_columns));
        }
        if !s.grid_rows.is_empty() {
            let _ = write!(w, "grid-template-rows:{};", tracks(&s.grid_rows));
        }
        if let Some(t) = &s.grid_auto_rows {
            let _ = write!(w, "grid-auto-rows:{};", t.css());
        }
    }
    let line = |start: Option<i16>, span: u16| match (start.filter(|s| *s != 0), span) {
        (Some(l), 1) => Some(format!("{l}")),
        (Some(l), n) => Some(format!("{l} / span {n}")),
        (None, n) if n > 1 => Some(format!("span {n}")),
        _ => None,
    };
    if let Some(c) = line(s.col_start, s.col_span) {
        let _ = write!(w, "grid-column:{c};");
    }
    if let Some(r) = line(s.row_start, s.row_span) {
        let _ = write!(w, "grid-row:{r};");
    }
    if let Some(g) = s.gap_x {
        let _ = write!(w, "column-gap:{g}px;");
    }
    if let Some(g) = s.gap_y {
        let _ = write!(w, "row-gap:{g}px;");
    }
    if let Some(r) = s.aspect_ratio {
        let _ = write!(w, "aspect-ratio:{r};");
    }
    if kind == NodeKind::Scroll && s.scroll_axis == prism_core::ScrollAxis::Horizontal {
        w.push_str("overflow-x:auto;overflow-y:hidden;");
    }
}

/// The CSS of a node with rules on its own width
/// (`.when_width_at_least(…)`), for each width they switch at:
/// `[[480, "css…"], [768, "css…"]]`, or null. The client watches the
/// element's width (`ResizeObserver`) and applies the widest variant it
/// reaches (the style op's `css` below the first).
pub fn own_width_variants(kind: NodeKind, s: &Style) -> serde_json::Value {
    use serde_json::{Value, json};
    let thresholds = s.own_width_thresholds();
    if thresholds.is_empty() {
        return Value::Null;
    }
    Value::Array(
        thresholds
            .into_iter()
            .map(|t| json!([t, css(kind, &s.resolve_own_width(t))]))
            .collect(),
    )
}

/// What a style op adds for the client beyond `css`: `hx` (a horizontal
/// scroll view: class `prism-scroll-x`, its children keep their size), `sb`
/// (its scroll bars, `overlay` or `hidden`) and `cq` (see
/// [`own_width_variants`]).
pub fn layout_op_fields(kind: NodeKind, s: &Style, op: &mut serde_json::Value) {
    if kind == NodeKind::Scroll && s.scroll_axis == prism_core::ScrollAxis::Horizontal {
        op["hx"] = serde_json::json!(true);
    }
    // Overlay or hidden scroll bars (#132): `data-prism-scrollbar`.
    if kind == NodeKind::Scroll && s.scrollbar != prism_core::Scrollbar::Auto {
        op["sb"] = serde_json::json!(s.scrollbar.name());
    }
    let cq = own_width_variants(kind, s);
    if !cq.is_null() {
        op["cq"] = cq;
    }
}
