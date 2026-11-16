//! `Widgets` for hosts in another language (the watchOS SwiftUI host, the
//! Android Java host): the same flexbox layout as iOS and macOS, computed in
//! Rust. The host only measures text and draws each node at the frame it is
//! given, so layouts match the other platforms. Changes are queued as JSON
//! (`ops`) for the backend to send once per commit:
//!
//! | op | fields |
//! |---|---|
//! | `create` / `destroy` | `id`, `kind` |
//! | `style` | `s`: `bg`, `radius`, `bw`, `bc`, `opacity`, `color`, `font`, `weight`, `talign` (colors `[r,g,b,a, dark r,g,b,a]`); `tint` on tab bars and stacks when set; `hit: false` (the subtree ignores pointer input) and `inert: true` (no input, no focus, hidden from assistive technologies) only when set |
//! | `text`, `placeholder`, `icon`, `value`, `clickable`, `display` | the value (`value` is `null` for an empty optional date picker, #89) |
//! | `text` of a `picture` | its source: for a remote one, the URL, then one `name: value` line per header to send (`Picture::header`) |
//! | `a11y` | `label`, `hidden`, `role` (ARIA name: `slider`, `combobox`, `status`…), `value`, `selected` (a selected item or the current page: `Accessibility::is_selected`) |
//! | `config` | `config`: a control's config string (`prism_core::controls`: slider `min max step`, picker options one per line, date `mode min max` then `optional`, input tokens; a screen's bar options, `prism_core::navbar`, with `glyphs`: each bar button's icon glyph, `""` if none) |
//! | `children` | `children`: ids in order |
//! | `frame` | `f`: `[x, y, w, h]` in the parent |
//! | `content` | `size`: a scroll view's content size |
//! | `menu`, `swipe`, `scrollev`, `root` | context menu JSON, swipe sides, scroll events, root id |
//! | `env` | `lang`, `rtl`, `back`, `close`, `clear` (no `id`) |
//! | `exit` | `parent`, `to`: `{o, tf}` the view animates to (a picture of it), before it is removed |
//! | `repeat` | `from`, `to`: `{o, tf}` (`to` null stops), back and forth forever |
//! | `draw` | `paths`: `[{d, fill, stroke, w, round, dash}]` in points, `d` flat (`0 x y` move, `1 x y` line, `2 x1 y1 x2 y2 x y` curve, `3` close) |
//!
//! | `pointer` | `mask`: pointer events to report (1 hover, 2 move, 4 drag) |
//! | `keys` | `local`, `global`: report key presses (focus in the node / anywhere) |
//! | `focus`, `reveal` | give the node the focus / scroll it into view |
//! | `sel` | `start`, `end`: select these characters of a text field (#86) |
//! | `selev` | `on`: a text field reports its selection (`select` events) |
//! | `layers` | `ids`: runtime layers above everything, bottom first (no `id`) |
//! | `webpost` | `data`: a message (JSON text) for a `web_view`'s page (#62) |
//! | `webreload` | reload a `web_view`'s page |
//!
//! Hosts report the new input events as `{"t":"click","id","x","y"}`
//! (position optional), `{"t":"pointer","id","phase","x","y"}`,
//! `{"t":"drag","id","phase","x","y","dx","dy"}` and
//! `{"t":"key","id","key","mods","repeat","sel"}` (`sel`: `[start, end]`,
//! the focused text field's selection in characters, #86) and
//! `{"t":"select","id","start","end"}` (a text field's selection moved, in
//! characters) (see [`parse_input_event`]).
//! An overlay's host reports its content area like a modal's
//! (`{"t":"viewport","id":overlay,"w","h"}`). A web view reports its page's
//! messages as `{"t":"webmsg","id","data","origin"}` (`data`: JSON text) and
//! its loads as `{"t":"webload","id","url"}`; its `text` is the URL, its
//! `config` a `prism_core::web_view::WebViewConfig`.
//!
//! Styles carry `tf`: `[x, y, scale, degrees]` (null: none). `style`, `frame`,
//! `exit` and `repeat` may carry `anim`: `{d, delay, ease}` (seconds, cubic
//! Bézier `[x1, y1, x2, y2]`): animate to the new values.
//! | `appearance` | `mode`: `system`, `light` or `dark` (no `id`) |
//! | `sticky` | `scroll`: id of the scroll view (null: not sticky anymore), `top`, `inset`, `max` (see [`Sticky`](crate::Sticky)): move the view down by `clamp(scrollY + inset - top, 0, max)`, above its siblings |
//! | `runs` | a rich text's runs (instead of `config` for texts): `[{t, w, i, u, s, c, bg, ff, z, link, href, icon, glyph}]` (see [`runs_json`]) |
//!
//! A scroll view's `style` carries `hscroll: true` when it scrolls
//! horizontally (its `content` size is then wider than its frame). For
//! `items_baseline()`, hosts answer a measure of kind `baseline` (its
//! `text` is the leaf's kind, `max_width` its height): `(0, distance from
//! the top to the first baseline)`, or `(0, 0)` if unknown.
//!
//! Styles of texts carry typography keys when set (see [`typography`]): `ff`
//! family (`mono`, `serif`, `rounded` or a name), `it` italic, `ul`
//! underline, `st` strikethrough, `tn` tabular digits, `tt` case (`upper`,
//! `lower`, `capitalize`), `ls` letter spacing and `lh` line height in
//! points, `ml` maximum lines (ellipsized), `nw` no wrapping, `tw` line
//! breaking (`pretty`, `balance`; #95). `weight` is any
//! CSS weight from 100 to 900.
//! | `states` | `on`: the interaction states the host reports for the node (`hover`, `active`, `focus`) |
//!
//! Box decorations, in `style` only when set: `shadows`: `[{x, y, blur,
//! spread, color, inset}]` (the first on top); `sides`: per-side borders
//! `[[w, color] × 4]` top, right, bottom, left (already mirrored right to
//! left); `bstyle`: `dashed` or `dotted`; `clip`: children clipped to the
//! rounded box; `grad`: `{kind: "linear" | "radial" | "conic", angle,
//! stops: [[color, at]]}` (angle in CSS degrees: 180 top to bottom for a
//! linear one, where the first color starts for a conic one, clockwise from
//! up); `blur`: backdrop blur
//! radius; `cursor`: a CSS cursor name. Colors are already resolved
//! ([`prism_core::Color::current`] included).
//!
//! A leaf's `style` carries `pad`: `[top, right, bottom, left]` (already
//! mirrored right to left) when it has padding (`[0, 0, 0, 0]` for an
//! explicit `.p(0.)`: no inset at all, #69): a text, timer, button, text
//! input or text area draws its content inset by it inside its frame, in
//! place of the control's own inset (the frame already includes it; other
//! leaves get a frame inside their padding and no `pad`). `bnone: true`
//! (`.border(0.)`): a field draws no frame of its own (an input's
//! underline or bezel, a date picker's pill) unless the style paints one.
//!
//! The host reports interaction states as `{"t":"state","id":…,"s":"hover",
//! "on":true}` (see [`state_event`]).

use std::collections::HashMap;

use prism_core::{
    Accessibility, Appearance, Color, Direction, FontWeight, NodeId, NodeKind, Pose,
    RenderEnvironment, Style, TextAlign, Transform, Transition,
};
use serde_json::{Value, json};

use crate::{Insets, Viewport, Widgets};

/// What the host is asked to measure.
#[derive(Clone, Debug)]
pub struct MeasureRequest<'a> {
    /// `text`, `button`, `text_input`, `toggle`, `progress`, `image`, `timer`…
    pub kind: &'a str,
    /// The text, the icon name of an `image`, a picker's widest option, a
    /// date picker's config (`date`, `time`, `datetime` first; an optional
    /// one's placeholder on a second line), `large` for a large spinner, a
    /// radio group's config (`row`/`column`, then options).
    pub text: &'a str,
    pub font_size: f32,
    /// CSS weight, 100 to 900.
    pub weight: i32,
    pub max_width: Option<f32>,
    /// Texts: their typography and runs as a JSON object (the style op's
    /// [`typography`] keys plus `runs`, see [`runs_json`]); empty when the
    /// text is plain (font size and weight only).
    pub spec: &'a str,
}

/// Measures a leaf in the host (synchronously, on the UI thread).
pub type Measure = fn(&MeasureRequest) -> (f32, f32);

pub struct RemoteWidgets {
    /// Changes for the host, in order.
    pub ops: Vec<Value>,
    kinds: HashMap<NodeId, NodeKind>,
    texts: HashMap<NodeId, String>,
    /// Icon names of images (what they measure).
    icons: HashMap<NodeId, String>,
    /// Control configs (a picker measures its options).
    configs: HashMap<NodeId, String>,
    styles: HashMap<NodeId, Style>,
    /// Content size of each stack or tab container, reported by the host.
    pub viewports: HashMap<NodeId, (f32, f32)>,
    frames: HashMap<NodeId, (f32, f32)>,
    measure: Measure,
    /// The root view (overlays default to its size).
    root: Option<NodeId>,
}

impl RemoteWidgets {
    pub fn new(measure: Measure) -> Self {
        RemoteWidgets {
            ops: Vec::new(),
            kinds: HashMap::new(),
            texts: HashMap::new(),
            icons: HashMap::new(),
            configs: HashMap::new(),
            styles: HashMap::new(),
            viewports: HashMap::new(),
            frames: HashMap::new(),
            measure,
            root: None,
        }
    }
}

/// A `change` event's value: a number, or `null` for an optional date
/// picker the user cleared (`NaN`, #89).
pub fn change_value(v: &Value) -> Option<f64> {
    if v.is_null() {
        Some(f64::NAN)
    } else {
        v.as_f64()
    }
}

/// The input events of #17 a host reports (`click` with a position,
/// `pointer`, `drag`, `key`), shared by the Android and watchOS backends.
pub fn parse_input_event(v: &Value) -> Option<prism_core::UiEvent> {
    use prism_core::{
        DragEvent, DragPhase, KeyEvent, Modifiers, Point, PointerEvent, PointerPhase, UiEvent,
    };
    let num = |k: &str| v.get(k).and_then(Value::as_f64).map(|n| n as f32);
    let id = v.get("id").and_then(Value::as_u64);
    match v.get("t")?.as_str()? {
        "click" => Some(match (num("x"), num("y")) {
            (Some(x), Some(y)) => UiEvent::ClickAt(id?, Point::new(x, y)),
            _ => UiEvent::Click(id?),
        }),
        // A tap on a 3D scene: `{"t":"scene_tap","id","node","point"}`.
        "scene_tap" => Some(UiEvent::SceneTap(
            id?,
            prism_core::SceneTapEvent::decode(&v.to_string()),
        )),
        "pointer" => Some(UiEvent::Pointer(
            id?,
            PointerEvent {
                phase: PointerPhase::from_name(v.get("phase")?.as_str()?)?,
                position: Point::new(num("x").unwrap_or(0.), num("y").unwrap_or(0.)),
            },
        )),
        "drag" => Some(UiEvent::Drag(
            id?,
            DragEvent {
                phase: DragPhase::from_name(v.get("phase")?.as_str()?)?,
                start: Point::new(num("x").unwrap_or(0.), num("y").unwrap_or(0.)),
                translation: Point::new(num("dx").unwrap_or(0.), num("dy").unwrap_or(0.)),
                // `button` and `mods` (#136): the primary button, no keys
                // when absent.
                button: v
                    .get("button")
                    .and_then(Value::as_str)
                    .and_then(prism_core::PointerButton::from_name)
                    .unwrap_or_default(),
                modifiers: Modifiers::from_bits(
                    v.get("mods").and_then(Value::as_u64).unwrap_or(0) as u8
                ),
            },
        )),
        // Wheel and pinch (#133): `{"t":"wheel","id","dx","dy","x","y","mods",
        // "phase","precise"}`, `{"t":"magnify","id","scale","x","y","phase"}`.
        "wheel" => Some(UiEvent::Wheel(
            id?,
            prism_core::WheelEvent {
                delta: Point::new(num("dx").unwrap_or(0.), num("dy").unwrap_or(0.)),
                position: Point::new(num("x").unwrap_or(0.), num("y").unwrap_or(0.)),
                modifiers: Modifiers::from_bits(
                    v.get("mods").and_then(Value::as_u64).unwrap_or(0) as u8
                ),
                phase: v
                    .get("phase")
                    .and_then(Value::as_str)
                    .and_then(prism_core::GesturePhase::from_name)
                    .unwrap_or_default(),
                precise: v.get("precise").and_then(Value::as_bool).unwrap_or(false),
            },
        )),
        "magnify" => Some(UiEvent::Magnify(
            id?,
            prism_core::MagnifyEvent {
                scale: num("scale").filter(|s| s.is_finite() && *s > 0.)?,
                position: Point::new(num("x").unwrap_or(0.), num("y").unwrap_or(0.)),
                phase: v
                    .get("phase")
                    .and_then(Value::as_str)
                    .and_then(prism_core::GesturePhase::from_name)
                    .unwrap_or_default(),
            },
        )),

        "key" => Some(UiEvent::Key(
            id.filter(|i| *i != 0),
            KeyEvent {
                key: v.get("key")?.as_str()?.to_string(),
                modifiers: Modifiers::from_bits(
                    v.get("mods").and_then(Value::as_u64).unwrap_or(0) as u8
                ),
                repeat: v.get("repeat").and_then(Value::as_bool).unwrap_or(false),
                selection: selection(v.get("sel")),
            },
        )),
        "select" => Some(UiEvent::Select(
            id?,
            v.get("start")?.as_u64()? as usize,
            v.get("end")?.as_u64()? as usize,
        )),
        "webmsg" => Some(prism_core::UiEvent::WebMessage(
            id?,
            v.get("data")?.as_str()?.to_string(),
            v.get("origin")?.as_str()?.to_string(),
        )),
        "webload" => Some(prism_core::UiEvent::WebLoad(
            id?,
            v.get("url")?.as_str()?.to_string(),
        )),
        // Files dropped on a drop target (#111), copied where the app reads
        // them: `{"t":"drop","id","files":[{path,name,mime,size}]}`.
        "drop" => Some(prism_core::UiEvent::DropFiles(
            id?,
            picked_files(v.get("files")?.as_array()?),
        )),
        _ => None,
    }
}

/// `[{path, name, mime, size}]` from a host.
pub fn picked_files(files: &[Value]) -> Vec<prism_core::PickedFile> {
    let s = |f: &Value, k: &str| f.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    files
        .iter()
        .map(|f| prism_core::PickedFile {
            path: s(f, "path"),
            name: s(f, "name"),
            mime: s(f, "mime"),
            size: f.get("size").and_then(Value::as_u64).unwrap_or(0),
        })
        .collect()
}

/// A key event's `sel`: `[start, end]` in characters, the focused text
/// field's selection (#86).
pub fn selection(v: Option<&Value>) -> Option<prism_core::SelectionEvent> {
    let a = v?.as_array()?;
    let at = |i: usize| a.get(i).and_then(Value::as_u64).map(|n| n as usize);
    Some(prism_core::SelectionEvent::new(at(0)?, at(1)?))
}

fn color(c: Option<Color>) -> Value {
    match c {
        None => Value::Null,
        Some(c) => {
            let (l, d) = (c.light_variant(), c.dark_variant());
            json!([l.r, l.g, l.b, l.a, d.r, d.g, d.b, d.a])
        }
    }
}

/// `[{d, fill, stroke, w, round, dash}]` (and `eo: true` for the even-odd
/// fill rule), `d` flat: `0 x y` move, `1 x y` line, `2 x1 y1 x2 y2 x y`
/// curve, `3` close.
fn drawing(paths: &[prism_core::Path]) -> Value {
    use prism_core::Segment as S;
    let paths: Vec<Value> = paths
        .iter()
        .map(|p| {
            let mut d: Vec<f32> = Vec::new();
            for s in &p.segments {
                match *s {
                    S::Move(x, y) => d.extend([0., x, y]),
                    S::Line(x, y) => d.extend([1., x, y]),
                    S::Cubic(a, b, c, e, f, g) => d.extend([2., a, b, c, e, f, g]),
                    S::Close => d.push(3.),
                    S::Arc { .. } => {}
                }
            }
            let mut path = json!({
                "d": d,
                "fill": color(p.fill),
                "stroke": color(p.stroke),
                "w": p.width,
                "round": p.round,
                "dash": p.dash.map(|(a, b)| [a, b]),
            });
            if p.even_odd {
                path["eo"] = json!(true);
            }
            path
        })
        .collect();
    Value::Array(paths)
}

fn transform(t: Transform) -> Value {
    if t.is_identity() {
        Value::Null
    } else if (t.scale_x, t.scale_y) == (1., 1.) {
        json!([t.x, t.y, t.scale, t.rotate])
    } else {
        // Non-uniform: `[x, y, scale, degrees, scale x, scale y]`.
        json!([t.x, t.y, t.scale, t.rotate, t.scale_x, t.scale_y])
    }
}

fn anim(t: Transition) -> Value {
    let (x1, y1, x2, y2) = t.bezier();
    json!({ "d": t.duration, "delay": t.delay, "ease": [x1, y1, x2, y2] })
}

fn pose(opacity: f32, t: Transform) -> Value {
    json!({ "o": opacity, "tf": transform(t) })
}

fn weight(w: FontWeight) -> i32 {
    w.css() as i32
}

/// The typography keys of a style that differ from the defaults (see the
/// module docs), as a JSON object.
pub fn typography(s: &Style) -> serde_json::Map<String, Value> {
    use prism_core::{FontFamily, TextTransform};
    let mut m = serde_json::Map::new();
    if s.font_family != FontFamily::System {
        // A run-time font's alias (`cx.load_font`) → its file's family.
        m.insert(
            "ff".into(),
            json!(crate::fonts::resolve(&s.font_family).encode()),
        );
    }
    for (key, on) in [
        ("it", s.italic),
        ("ul", s.underline),
        ("st", s.strikethrough),
        ("tn", s.tabular_numbers),
        ("nw", s.no_wrap),
    ] {
        if on {
            m.insert(key.into(), json!(true));
        }
    }
    let tt = match s.text_transform {
        TextTransform::None => None,
        TextTransform::Uppercase => Some("upper"),
        TextTransform::Lowercase => Some("lower"),
        TextTransform::Capitalize => Some("capitalize"),
    };
    if let Some(t) = tt {
        m.insert("tt".into(), json!(t));
    }
    if s.letter_spacing != 0. {
        m.insert("ls".into(), json!(s.letter_spacing));
    }
    if let Some(lh) = s.line_height_points() {
        m.insert("lh".into(), json!(lh));
    }
    if let Some(n) = s.max_lines {
        m.insert("ml".into(), json!(n));
    }
    if s.text_wrap != prism_core::TextWrap::Auto {
        m.insert("tw".into(), json!(s.text_wrap.css()));
    }
    m
}

/// A rich text's runs for a host: `[{t, w, i, u, s, c, bg, ff, z, link,
/// href, icon, glyph}]`, only the keys a run sets (`t` text, `w` CSS weight,
/// `i`/`u`/`s` italic, underline, strikethrough, `c`/`bg` colors, `ff`
/// family, `z` size as a multiple of the text's, `link` the id to report as
/// `{"t":"menu","id":link}`, `href` its path, `icon` an SF Symbol name and
/// `glyph` its fallback glyph).
pub fn runs_json(config: &str) -> Value {
    let runs: Vec<Value> = prism_core::decode_runs(config)
        .into_iter()
        .map(|r| {
            let mut m = serde_json::Map::new();
            m.insert("t".into(), json!(r.text));
            if let Some(w) = r.weight {
                m.insert("w".into(), json!(w.css()));
            }
            for (key, on) in [("i", r.italic), ("u", r.underline), ("s", r.strikethrough)] {
                if on {
                    m.insert(key.into(), json!(true));
                }
            }
            if r.color.is_some() {
                m.insert("c".into(), color(r.color));
            }
            if r.background.is_some() {
                m.insert("bg".into(), color(r.background));
            }
            if let Some(f) = &r.family {
                m.insert("ff".into(), json!(crate::fonts::resolve(f).encode()));
            }
            if let Some(z) = r.size {
                m.insert("z".into(), json!(z));
            }
            if let Some(l) = r.link {
                m.insert("link".into(), json!(l));
            }
            if let Some(h) = r.href {
                m.insert("href".into(), json!(h));
            }
            if let Some(g) = r.icon {
                m.insert("glyph".into(), json!(prism_core::glyphs::glyph(&g)));
                m.insert("icon".into(), json!(g));
            }
            Value::Object(m)
        })
        .collect();
    Value::Array(runs)
}

impl RemoteWidgets {
    fn style_op(&mut self, id: &NodeId, s: &Style) -> Value {
        self.styles.insert(*id, s.clone());
        let mut op = json!({ "op": "style", "id": id, "s": {
            "bg": color(s.background),
            "radius": s.corner_radius,
            "bw": s.border_width,
            "bc": if s.border_width > 0. { color(s.border_color) } else { Value::Null },
            "opacity": s.opacity,
            "tf": transform(s.transform),
            "color": color(s.text_color),
            "font": s.font_size,
            "weight": weight(s.font_weight),
            "talign": match s.text_align { TextAlign::Left => "left", TextAlign::Center => "center", TextAlign::Right => "right" },
        }});
        for (k, v) in typography(s) {
            op["s"][k] = v;
        }
        // Only some corners rounded: bits 1 top left, 2 top right, 4 bottom
        // right, 8 bottom left (already mirrored for right to left).
        if s.rounded_corners != prism_core::Corners::ALL {
            op["s"]["corners"] = json!(s.rounded_corners.bits());
        }
        if s.scroll_axis == prism_core::ScrollAxis::Horizontal {
            op["s"]["hscroll"] = json!(true);
        }
        // `overlay` or `hidden` scroll bars (#132).
        if s.scrollbar != prism_core::Scrollbar::Auto {
            op["s"]["scrollbar"] = json!(s.scrollbar.name());
        }
        decorations(&mut op["s"], s);
        // A leaf's padding (#14), only when set: inset the content (text,
        // title, an input's text) by it, in place of the control's own. An
        // explicit zero (`.p(0.)`) is sent too: no inset at all (#69).
        let p = s.padding;
        if s.has_padding() {
            op["s"]["pad"] = json!([p.top, p.right, p.bottom, p.left]);
        }
        // `.border(0.)`: not even a field's own frame.
        if s.border_none {
            op["s"]["bnone"] = json!(true);
        }
        // Hit testing (#19), only when not the default.
        if !s.hit_test {
            op["s"]["hit"] = json!(false);
        }
        if s.inert {
            op["s"]["inert"] = json!(true);
        }
        // A tab bar's or a stack's tint (#30), only when set.
        if s.tint.is_some() {
            op["s"]["tint"] = color(s.tint);
        }
        op
    }
}

/// The glyph of a bar button's icon, `""` when Prism has none for it.
fn bar_glyph(icon: &str) -> String {
    prism_core::glyphs::GLYPHS
        .iter()
        .find(|(s, _)| *s == icon)
        .map_or(String::new(), |(_, g)| g.to_string())
}

/// Box decorations of a style op (only those set: older hosts ignore them).
fn decorations(o: &mut Value, s: &Style) {
    use prism_core::{BorderStyle, GradientKind};
    if !s.shadows.is_empty() {
        o["shadows"] = s
            .shadows
            .iter()
            .map(|sh| {
                json!({
                    "x": sh.x, "y": sh.y, "blur": sh.blur, "spread": sh.spread,
                    "color": color(Some(sh.color)), "inset": sh.inset,
                })
            })
            .collect();
    }
    if s.has_side_borders() {
        o["sides"] = s
            .borders()
            .iter()
            .map(|b| {
                json!([
                    b.width,
                    if b.width > 0. {
                        color(b.color)
                    } else {
                        Value::Null
                    }
                ])
            })
            .collect();
    }
    if s.border_style != BorderStyle::Solid {
        o["bstyle"] = json!(s.border_style.css());
    }
    if s.clip {
        o["clip"] = json!(true);
    }
    if let Some(g) = &s.gradient {
        let (kind, angle) = match g.kind {
            GradientKind::Linear { angle } => ("linear", angle),
            GradientKind::Radial => ("radial", 0.),
            // `angle`: where the first color starts, clockwise from up.
            GradientKind::Conic { from } => ("conic", from),
        };
        o["grad"] = json!({
            "kind": kind,
            "angle": angle,
            "stops": g.stops.iter().map(|st| json!([color(Some(st.color)), st.at])).collect::<Vec<_>>(),
        });
    }
    if s.backdrop_blur > 0. {
        o["blur"] = json!(s.backdrop_blur);
    }
    if let Some(c) = s.cursor {
        o["cursor"] = json!(c.css());
    }
}

/// A host's `{"t":"state","id":…,"s":"hover"|"active"|"focus","on":bool}`:
/// the node entered or left an interaction state. Hand it to
/// [`NativeRenderer::set_interaction`](crate::NativeRenderer::set_interaction).
pub fn state_event(v: &Value) -> Option<(NodeId, prism_core::Interaction, bool)> {
    if v.get("t")?.as_str()? != "state" {
        return None;
    }
    Some((
        v.get("id")?.as_u64()?,
        prism_core::Interaction::from_name(v.get("s")?.as_str()?)?,
        v.get("on")?.as_bool()?,
    ))
}

impl RemoteWidgets {
    /// A button, text input or text area with padding: its content alone,
    /// measured as text (the padding replaces the control's own insets, and
    /// the layout adds it). An input is one line of text 10 em wide (as wide
    /// as it may be), a text area at least three lines, 17.5 em wide.
    fn measure_padded(
        &mut self,
        id: &NodeId,
        kind: NodeKind,
        style: &Style,
        max_width: Option<f32>,
    ) -> (f32, f32) {
        let value = self.texts.get(id).cloned().unwrap_or_default();
        let text = match kind {
            NodeKind::TextInput => "Ag".to_string(),
            NodeKind::TextArea if value.lines().count() < 3 => "Ag\nAg\nAg".to_string(),
            _ => value,
        };
        let spec = Value::Object(typography(style)).to_string();
        let em = style.font_size;
        let width = match kind {
            NodeKind::TextInput => Some(max_width.unwrap_or(10. * em)),
            NodeKind::TextArea => Some(max_width.unwrap_or(17.5 * em)),
            _ => None,
        };
        let (w, h) = (self.measure)(&MeasureRequest {
            kind: "text",
            text: &text,
            font_size: style.font_size,
            weight: weight(style.font_weight),
            max_width: width.or(max_width),
            spec: if spec == "{}" { "" } else { &spec },
        });
        (width.unwrap_or(w), h)
    }
}

impl Widgets for RemoteWidgets {
    type View = NodeId;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> NodeId {
        self.kinds.insert(id, kind);
        self.ops
            .push(json!({ "op": "create", "id": id, "kind": kind.name() }));
        id
    }

    /// Only what SwiftUI draws: layout is ours (frames).
    fn set_style(&mut self, id: &NodeId, s: &Style) {
        let op = self.style_op(id, s);
        self.ops.push(op);
    }

    fn set_style_animated(&mut self, id: &NodeId, s: &Style, t: Transition) {
        let mut op = self.style_op(id, s);
        op["anim"] = anim(t);
        self.ops.push(op);
    }

    fn set_frame_animated(&mut self, id: &NodeId, x: f32, y: f32, w: f32, h: f32, t: Transition) {
        self.frames.insert(*id, (w, h));
        self.ops
            .push(json!({ "op": "frame", "id": id, "f": [x, y, w, h], "anim": anim(t) }));
    }

    fn animate_exit(&mut self, parent: &NodeId, id: &NodeId, to: &Style, t: Transition) {
        self.ops.push(json!({
            "op": "exit", "id": id, "parent": parent, "to": pose(to.opacity, to.transform), "anim": anim(t),
        }));
    }

    fn set_repeat(&mut self, id: &NodeId, s: &Style, repeat: Option<(Pose, Transition)>) {
        let op = match repeat {
            None => json!({ "op": "repeat", "id": id, "to": Value::Null }),
            Some((p, t)) => {
                let (o, tf) = p.apply(s.opacity, s.transform);
                let mut op = json!({
                    "op": "repeat", "id": id, "from": pose(s.opacity, s.transform),
                    "to": pose(o, tf), "anim": anim(t),
                });
                // Starts over from `from` instead of reversing (spinners).
                if s.animations.restart {
                    op["restart"] = json!(true);
                }
                op
            }
        };
        self.ops.push(op);
    }

    fn set_text(&mut self, id: &NodeId, text: &str) {
        self.texts.insert(*id, text.to_string());
        self.ops
            .push(json!({ "op": "text", "id": id, "text": text }));
    }

    fn set_placeholder(&mut self, id: &NodeId, text: &str) {
        // What an optional date picker measures while empty (#89).
        if self.kinds.get(id) == Some(&NodeKind::DatePicker) {
            self.texts.insert(*id, text.to_string());
        }
        self.ops
            .push(json!({ "op": "placeholder", "id": id, "text": text }));
    }

    fn set_clickable(&mut self, id: &NodeId, clickable: bool) {
        self.ops
            .push(json!({ "op": "clickable", "id": id, "on": clickable }));
    }

    fn set_children(&mut self, parent: &NodeId, children: &[NodeId]) {
        self.ops
            .push(json!({ "op": "children", "id": parent, "children": children }));
    }

    fn set_root(&mut self, id: &NodeId) {
        self.root = Some(*id);
        self.ops.push(json!({ "op": "root", "id": id }));
    }

    fn destroy(&mut self, id: &NodeId) {
        self.kinds.remove(id);
        self.texts.remove(id);
        self.icons.remove(id);
        self.configs.remove(id);
        self.styles.remove(id);
        self.viewports.remove(id);
        self.frames.remove(id);
        self.ops.push(json!({ "op": "destroy", "id": id }));
    }

    fn measure(&mut self, id: &NodeId, max_width: Option<f32>) -> (f32, f32) {
        let kind = self.kinds.get(id).copied().unwrap_or(NodeKind::View);
        let style = self.styles.get(id).cloned().unwrap_or_default();
        if style.has_padding() && kind != NodeKind::Text && kind != NodeKind::Timer {
            return self.measure_padded(id, kind, &style, max_width);
        }
        let text = match kind {
            NodeKind::Timer => "00:00".to_string(),
            NodeKind::Image => self.icons.get(id).cloned().unwrap_or_default(),
            // The widest option decides the width; the host adds its chrome.
            NodeKind::Picker => self
                .configs
                .get(id)
                .map(|c| prism_core::controls::PickerConfig::decode(c).options)
                .unwrap_or_default()
                .into_iter()
                .max_by_key(|o| o.chars().count())
                .unwrap_or_default(),
            // An optional one is as wide as its placeholder too (#89).
            NodeKind::DatePicker => {
                let config = self.configs.get(id).cloned().unwrap_or_default();
                match self.texts.get(id) {
                    Some(placeholder)
                        if !placeholder.is_empty()
                            && prism_core::controls::DateConfig::decode(&config).optional =>
                    {
                        format!("{config}\n{placeholder}")
                    }
                    _ => config,
                }
            }
            // `row` or `column`, then the options: the host lays them out.
            NodeKind::RadioGroup => self.configs.get(id).cloned().unwrap_or_default(),
            _ => self.texts.get(id).cloned().unwrap_or_default(),
        };
        let spec = match kind {
            NodeKind::Text => {
                let mut spec = typography(&style);
                let runs = self.configs.get(id).map(String::as_str).unwrap_or("");
                if !runs.is_empty() {
                    spec.insert("runs".into(), runs_json(runs));
                }
                if spec.is_empty() {
                    String::new()
                } else {
                    Value::Object(spec).to_string()
                }
            }
            _ => String::new(),
        };
        (self.measure)(&MeasureRequest {
            kind: kind.name(),
            text: &text,
            font_size: style.font_size,
            weight: weight(style.font_weight),
            max_width,
            spec: &spec,
        })
    }

    /// The longest word: texts wrap down to it, like CSS min-content.
    fn measure_min_content(&mut self, id: &NodeId) -> f32 {
        let kind = self.kinds.get(id).copied().unwrap_or(NodeKind::View);
        let text = self.texts.get(id).cloned().unwrap_or_default();
        if !matches!(kind, NodeKind::Text | NodeKind::Button) || text.is_empty() {
            return self.measure(id, None).0;
        }
        let words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
        let full = self.texts.insert(*id, String::new());
        // Words are measured as plain text in the text's style (runs set
        // their own sizes and weights per run, not per word).
        let runs = self.configs.remove(id);
        let mut widest = 0f32;
        for word in words {
            self.texts.insert(*id, word);
            widest = widest.max(self.measure(id, None).0);
        }
        if let Some(full) = full {
            self.texts.insert(*id, full);
        }
        if let Some(runs) = runs {
            self.configs.insert(*id, runs);
        }
        widest
    }

    fn set_frame(&mut self, id: &NodeId, x: f32, y: f32, w: f32, h: f32) {
        self.frames.insert(*id, (w, h));
        self.ops
            .push(json!({ "op": "frame", "id": id, "f": [x, y, w, h] }));
    }

    /// Stacks and tabs: the area SwiftUI gives their content (below the
    /// navigation bar), as the host reported it.
    fn screen_viewport(&mut self, container: &NodeId, child: &NodeId) -> Viewport {
        // A modal reports its own size; screens get their container's.
        let (width, height) = self
            .viewports
            .get(child)
            .or_else(|| self.viewports.get(container))
            .or_else(|| self.frames.get(container))
            .copied()
            .unwrap_or((0., 0.));
        Viewport {
            x: 0.,
            y: 0.,
            width,
            height,
            insets: Insets::default(),
        }
    }

    fn set_accessibility(&mut self, id: &NodeId, a11y: &Accessibility) {
        self.ops.push(json!({
            "op": "a11y", "id": id, "label": a11y.label, "hidden": a11y.hidden,
            "role": a11y.role.map(|r| r.aria()), "value": a11y.value,
            // Switches, checkboxes and drawn radios (#84): on or chosen.
            "checked": a11y.checked,
            "selected": a11y.is_selected(),
        }));
    }

    fn set_value(&mut self, id: &NodeId, value: f64) {
        if self.kinds.get(id) == Some(&NodeKind::Spinner) {
            // What it measures: `large` or the regular size.
            let size = if value != 0. { "large" } else { "" };
            self.texts.insert(*id, size.to_string());
        }
        self.ops
            .push(json!({ "op": "value", "id": id, "value": value }));
    }

    /// Raw config string (see `prism_core::controls`); hosts decode it.
    /// A text's runs are sent decoded, as a `runs` op.
    fn set_config(&mut self, id: &NodeId, kind: NodeKind, config: &str) {
        self.configs.insert(*id, config.to_string());
        if kind == NodeKind::Text {
            self.ops
                .push(json!({ "op": "runs", "id": id, "runs": runs_json(config) }));
            return;
        }
        let mut op = json!({ "op": "config", "id": id, "config": config });
        // A screen's bar buttons (#30): the glyph of each one's icon ("":
        // none), for hosts without SF Symbols.
        if kind == NodeKind::Screen {
            let c = prism_core::navbar::ScreenConfig::decode(config);
            op["glyphs"] = c
                .items
                .iter()
                .map(|i| bar_glyph(&i.icon))
                .collect::<Vec<_>>()
                .into();
        }
        self.ops.push(op);
    }

    fn set_icon(&mut self, id: &NodeId, icon: &str) {
        self.icons.insert(*id, icon.to_string());
        self.ops
            .push(json!({ "op": "icon", "id": id, "icon": icon, "glyph": prism_core::glyphs::glyph(icon) }));
    }

    fn set_drawing(&mut self, id: &NodeId, paths: &[prism_core::Path]) {
        self.ops
            .push(json!({ "op": "draw", "id": id, "paths": drawing(paths) }));
    }

    fn set_scene(&mut self, id: &NodeId, _update: &prism_core::scene3d::SceneUpdate, json: &str) {
        // The host applies it to its scene player (`prism_core::scene3d::wire`).
        let update: Value = serde_json::from_str(json).unwrap_or(Value::Null);
        self.ops
            .push(json!({ "op": "scene", "id": id, "u": update }));
    }

    fn set_hidden(&mut self, id: &NodeId, hidden: bool) {
        self.ops
            .push(json!({ "op": "display", "id": id, "hidden": hidden }));
    }

    fn set_content_size(&mut self, id: &NodeId, width: f32, height: f32) {
        self.ops
            .push(json!({ "op": "content", "id": id, "size": [width, height] }));
    }

    fn set_menu(&mut self, id: &NodeId, menu: Option<&prism_core::menu::MenuSpec>) {
        let menu = menu.and_then(|m| serde_json::from_str::<Value>(&m.to_json()).ok());
        self.ops
            .push(json!({ "op": "menu", "id": id, "menu": menu }));
    }

    fn set_swipe(&mut self, id: &NodeId, left: bool, right: bool) {
        self.ops
            .push(json!({ "op": "swipe", "id": id, "left": left, "right": right }));
    }

    fn set_scroll_events(&mut self, id: &NodeId, on: bool) {
        self.ops
            .push(json!({ "op": "scrollev", "id": id, "on": on }));
    }

    /// Which interaction states the host reports for the node. Focus-within
    /// and group states are derived (hosts always report the focus of text
    /// inputs and text areas); disabled is resolved before the host sees it.
    fn set_interactions(&mut self, id: &NodeId, states: prism_core::Interactions) {
        use prism_core::Interaction as I;
        let on: Vec<&str> = [I::Hover, I::Active, I::Focus]
            .into_iter()
            .filter(|i| states.contains(*i))
            .map(I::name)
            .collect();
        self.ops.push(json!({ "op": "states", "id": id, "on": on }));
    }

    /// Language direction and the chrome's words (the host's header, bars).
    fn set_environment(&mut self, env: &RenderEnvironment) {
        self.ops.push(json!({
            "op": "env",
            "lang": env.lang,
            "rtl": env.direction == Direction::Rtl,
            "back": env.back,
            "close": env.close,
            "clear": env.clear,
        }));
    }

    /// Light/dark chrome and the light or dark variant of every color.
    fn set_appearance(&mut self, appearance: Appearance) {
        let mode = match appearance {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        };
        self.ops.push(json!({ "op": "appearance", "mode": mode }));
    }

    /// The host moves the view as its scroll view scrolls.
    fn set_sticky(&mut self, id: &NodeId, sticky: Option<(&NodeId, crate::Sticky)>) {
        let op = match sticky {
            Some((scroll, s)) => json!({
                "op": "sticky", "id": id, "scroll": scroll,
                "top": s.top, "inset": s.inset, "max": s.max_offset,
            }),
            None => json!({ "op": "sticky", "id": id, "scroll": Value::Null }),
        };
        self.ops.push(op);
    }

    /// Asked of the host: where the first line of text sits in the leaf.
    fn baseline(&mut self, id: &NodeId, _width: f32, height: f32) -> Option<f32> {
        let kind = self.kinds.get(id).copied().unwrap_or(NodeKind::View);
        if !matches!(
            kind,
            NodeKind::Text
                | NodeKind::Button
                | NodeKind::TextInput
                | NodeKind::Timer
                | NodeKind::Picker
                | NodeKind::DatePicker
        ) {
            return None;
        }
        let style = self.styles.get(id).cloned().unwrap_or_default();
        // The text's typography (family, line height), for hosts that use it.
        let spec = match typography(&style) {
            t if t.is_empty() => String::new(),
            t => Value::Object(t).to_string(),
        };
        let (_, b) = (self.measure)(&MeasureRequest {
            kind: "baseline",
            text: kind.name(),
            font_size: style.font_size,
            weight: weight(style.font_weight),
            max_width: Some(height),
            spec: &spec,
        });
        (b > 0.).then_some(b)
    }

    fn set_pointer_events(&mut self, id: &NodeId, mask: u8) {
        self.ops
            .push(json!({ "op": "pointer", "id": id, "mask": mask }));
    }

    fn set_drop_target(&mut self, id: &NodeId, types: Option<&[String]>) {
        self.ops
            .push(json!({ "op": "drop", "id": id, "types": types }));
    }

    fn set_key_events(&mut self, id: &NodeId, local: bool, global: bool) {
        self.ops
            .push(json!({ "op": "keys", "id": id, "local": local, "global": global }));
    }

    fn focus(&mut self, id: &NodeId) {
        self.ops.push(json!({ "op": "focus", "id": id }));
    }

    fn set_selection(&mut self, id: &NodeId, start: usize, end: usize) {
        self.ops
            .push(json!({ "op": "sel", "id": id, "start": start, "end": end }));
    }

    fn set_select_events(&mut self, id: &NodeId, on: bool) {
        self.ops.push(json!({ "op": "selev", "id": id, "on": on }));
    }

    fn scroll_into_view(&mut self, id: &NodeId) {
        self.ops.push(json!({ "op": "reveal", "id": id }));
    }

    fn dismiss_keyboard(&mut self) {
        self.ops.push(json!({ "op": "blur" }));
    }

    fn set_layers(&mut self, layers: &[NodeId]) {
        self.ops.push(json!({ "op": "layers", "ids": layers }));
    }

    fn post_web_message(&mut self, id: &NodeId, data: &str) {
        self.ops
            .push(json!({ "op": "webpost", "id": id, "data": data }));
    }

    fn reload_web_view(&mut self, id: &NodeId) {
        self.ops.push(json!({ "op": "webreload", "id": id }));
    }

    /// The area the host reported for the overlay (its sheet, dialog or
    /// popover), else the root's size.
    fn overlay_viewport(&mut self, overlay: &NodeId, _content: &NodeId) -> Viewport {
        let (width, height) = self
            .viewports
            .get(overlay)
            .or_else(|| self.root.and_then(|r| self.frames.get(&r)))
            .copied()
            .unwrap_or((0., 0.));
        Viewport {
            x: 0.,
            y: 0.,
            width,
            height,
            insets: Insets::default(),
        }
    }
}

/// Device calls as JSON for a host (the same shape as the web client's), and
/// the host's answers back.
pub mod device {
    use prism_core::{
        Accuracy, DeviceCall, DeviceError, DeviceEvent, DeviceValue, Location, Motion,
        PermissionState,
    };
    use serde_json::{Value, json};

    /// `{"t":"device","id":…,"call":"location","accuracy":"best"}`…
    pub fn call(id: u64, call: DeviceCall) -> Value {
        let accuracy = |a: Accuracy| match a {
            Accuracy::Coarse => "coarse",
            Accuracy::Balanced => "balanced",
            Accuracy::Best => "best",
        };
        let mut body = match call {
            DeviceCall::PermissionStatus(p) => {
                json!({ "call": "permission_status", "permission": p.name() })
            }
            DeviceCall::RequestPermission(p) => {
                json!({ "call": "request_permission", "permission": p.name() })
            }
            DeviceCall::CurrentLocation(a) => {
                json!({ "call": "location", "accuracy": accuracy(a) })
            }
            DeviceCall::WatchLocation(a) => {
                json!({ "call": "watch_location", "accuracy": accuracy(a) })
            }
            DeviceCall::WatchMotion(every) => {
                json!({ "call": "watch_motion", "interval": every.as_millis() as u64 })
            }
            DeviceCall::StopWatch => json!({ "call": "stop" }),
            DeviceCall::PickImages(o) => {
                json!({ "call": "pick_images", "limit": o.limit, "camera": o.camera })
            }
            DeviceCall::PickFiles(o) => {
                json!({ "call": "pick_files", "types": o.types, "multiple": o.multiple })
            }
            DeviceCall::ReadClipboard => json!({ "call": "paste" }),
            DeviceCall::Haptic(h) => json!({ "call": "haptic", "kind": h.name() }),
            DeviceCall::CopyText(t) => json!({ "call": "copy", "text": t }),
            DeviceCall::Share(s) => {
                json!({ "call": "share", "text": s.text, "url": s.url, "files": s.files })
            }
            DeviceCall::OpenUrl(u) => json!({ "call": "open_url", "url": u }),
            // Android has no tabs to choose: an ACTION_VIEW intent either way.
            DeviceCall::OpenUrlIn(u, _) => json!({ "call": "open_url", "url": u }),
            DeviceCall::SecureGet {
                key,
                prompt,
                shared,
            } => json!({ "call": "secure_get", "key": key, "prompt": prompt, "shared": shared }),
            DeviceCall::SecureSet {
                key,
                value,
                options,
            } => json!({
                "call": "secure_set", "key": key, "value": value,
                "biometric": options.biometric, "shared": options.shared,
            }),
            DeviceCall::SecureDelete { key, shared } => {
                json!({ "call": "secure_delete", "key": key, "shared": shared })
            }
            DeviceCall::BiometryStatus => json!({ "call": "biometry" }),
            DeviceCall::Authenticate(r) => {
                json!({ "call": "authenticate", "reason": r.reason, "passcode": r.passcode })
            }
            DeviceCall::SignInWithApple(r) => json!({
                "call": "apple_sign_in", "email": r.email, "name": r.full_name,
                "nonce": r.nonce, "state": r.state,
            }),
            DeviceCall::AuthRedirect(preferred) => {
                json!({ "call": "auth_redirect", "preferred": preferred })
            }
            DeviceCall::WebAuth(r) => json!({
                "call": "web_auth", "url": r.url, "redirect": r.redirect,
                "ephemeral": r.ephemeral, "provider": r.provider,
            }),
            // The app's own relying party (#94): `options` as the server's
            // JSON text; the host answers the credential's JSON (`text`).
            DeviceCall::WebAuthn(r) => json!({
                "call": "webauthn", "create": r.create, "options": r.options,
            }),
            // No cookies natively: the host answers « unavailable ».
            DeviceCall::SetCookie(_) => json!({ "call": "set_cookie" }),
            // No live session to renew: the host ignores it.
            DeviceCall::Reconnect => json!({ "call": "reconnect" }),
        };
        body["op"] = json!("device");
        body["id"] = json!(id);
        body
    }

    fn location(v: &Value) -> Option<Location> {
        let n = |k: &str| v.get(k).and_then(Value::as_f64);
        Some(Location {
            latitude: n("latitude")?,
            longitude: n("longitude")?,
            altitude: n("altitude"),
            accuracy: n("accuracy").unwrap_or(0.),
            speed: n("speed"),
            heading: n("heading"),
            timestamp: n("timestamp").unwrap_or(0.),
        })
    }

    fn motion(v: &Value) -> Motion {
        let triple = |k: &str| {
            let a = v.get(k).and_then(Value::as_array);
            let at = |i: usize| {
                a.and_then(|a| a.get(i))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.)
            };
            [at(0), at(1), at(2)]
        };
        Motion {
            acceleration: triple("acceleration"),
            gravity: triple("gravity"),
            rotation_rate: triple("rotation_rate"),
            attitude: triple("attitude"),
        }
    }

    /// A `device_reply`: `{"err":…}`, `{"permission":…}`, `{"location":{…}}`,
    /// `{"files":[{path,name,mime,size}]}`, `{"text":…}`, `{"fields":{…}}`
    /// or nothing (done).
    pub fn reply(v: &Value) -> Result<DeviceValue, DeviceError> {
        if let Some(err) = v.get("err").and_then(Value::as_str) {
            return Err(DeviceError::from_message(err));
        }
        if let Some(fields) = v.get("fields").and_then(Value::as_object) {
            return Ok(DeviceValue::Fields(
                fields
                    .iter()
                    .filter_map(|(k, v)| {
                        let v = match v {
                            Value::String(s) => s.clone(),
                            Value::Bool(b) => (if *b { "1" } else { "0" }).to_string(),
                            Value::Number(n) => n.to_string(),
                            _ => return None,
                        };
                        Some((k.clone(), v))
                    })
                    .collect(),
            ));
        }
        if let Some(p) = v.get("permission").and_then(Value::as_str) {
            return Ok(DeviceValue::Permission(PermissionState::from_name(p)));
        }
        if let Some(l) = v.get("location").and_then(location) {
            return Ok(DeviceValue::Location(l));
        }
        if let Some(files) = v.get("files").and_then(Value::as_array) {
            return Ok(DeviceValue::Files(super::picked_files(files)));
        }
        if let Some(t) = v.get("text") {
            return Ok(DeviceValue::Text(t.as_str().map(str::to_string)));
        }
        Ok(DeviceValue::Done)
    }

    /// A `device_event` of a stream: an error, a location or motion.
    pub fn event(v: &Value) -> DeviceEvent {
        if let Some(err) = v.get("err").and_then(Value::as_str) {
            DeviceEvent::Error(DeviceError::from_message(err))
        } else if let Some(l) = v.get("location").and_then(location) {
            DeviceEvent::Location(l)
        } else {
            DeviceEvent::Motion(motion(v))
        }
    }
}

#[cfg(test)]
mod scene_tests {
    #[test]
    fn scene_taps_parse() {
        let v = serde_json::json!({"t": "scene_tap", "id": 4, "node": "cube", "part": "lid", "instance": 2, "point": [1., 2., 3.]});
        assert_eq!(
            super::parse_input_event(&v),
            Some(prism_core::UiEvent::SceneTap(
                4,
                prism_core::SceneTapEvent {
                    node: Some("cube".into()),
                    part: Some("lid".into()),
                    instance: Some(2),
                    point: Some([1., 2., 3.]),
                }
            ))
        );
        let v = serde_json::json!({"t": "scene_tap", "id": 4, "node": null, "point": null});
        assert!(matches!(
            super::parse_input_event(&v),
            Some(prism_core::UiEvent::SceneTap(4, e)) if e == prism_core::SceneTapEvent::default()
        ));
    }

    #[test]
    fn scene_updates_go_to_the_host_as_scene_ops() {
        use prism_core::scene3d::{SceneData, SceneUpdate, cube};
        use prism_core::{Backend, NodeKind, Patch};
        let mut r = crate::NativeRenderer::new(super::RemoteWidgets::new(|_| (0., 0.)));
        let mut scene = SceneData::default();
        scene.nodes.push(cube(1.).name("a"));
        let update = SceneUpdate::full(&scene);
        r.apply(&[
            Patch::Create {
                id: 1,
                kind: NodeKind::Scene3d,
            },
            Patch::SetScene {
                id: 1,
                update: update.clone(),
            },
            Patch::SetRoot { id: 1 },
        ]);
        let op = r
            .widgets
            .ops
            .iter()
            .find(|o| o["op"] == "scene")
            .expect("a scene op");
        assert_eq!(op["id"], 1);
        assert_eq!(op["u"]["v"], 2);
        assert_eq!(op["u"]["full"], true);
        assert_eq!(op["u"]["nodes"][0]["key"], "#a");
        assert_eq!(SceneUpdate::from_json(&op["u"].to_string()), Some(update));
        // Floats as short as the app's (not widened to f64).
        assert!(op.to_string().contains(r#""near":0.1,"#), "{op}");
    }
}
