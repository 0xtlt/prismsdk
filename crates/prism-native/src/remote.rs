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
//! | `draw` | `paths`: `[{d, fill, stroke, w, round, dash}]` in points, `d` flat (`0 x y` move, `1 x y` line, `2 x1 y1 x2 y2 x y` curve, `3` close), `fg` / `sg` a gradient for the fill / stroke (`{k, c, m, sp, o, cl}`); or, for a host that draws them (`with_drawing_items`), `items` when the drawing has more than paths (an SVG document's text, images, groups): `[{t: "p" path, "x" text, "i" image, "g" group, …}]` (see `drawing_items`) |
//!
//! | `pointer` | `mask`: pointer events to report (1 hover, 2 move, 4 drag) |
//! | `keys` | `local`, `global`: report key presses (focus in the node / anywhere) |
//! | `focus`, `reveal` | give the node the focus / scroll it into view |
//! | `sel` | `start`, `end`: select these characters of a text field (#86) |
//! | `selev` | `on`: a text field reports its selection (`select` events) |
//! | `layers` | `ids`: runtime layers above everything, bottom first (no `id`) |
//! | `webpost` | `data`: a message (JSON text) for a `web_view`'s page (#62) |
//! | `webreload` | reload a `web_view`'s page |
//! | `kbar` | `bar`: the keyboard overlay shown above the IME while this text field has the focus (null: none, #242) |
//! | `table` | `rows`: a table's rows (#284), each `{id, header, cells: [ids]}`, the header row first: the host gives the table its rows and columns, each cell its row, column and header (kinds `table`, `table_row`, `column_header`, `table_cell`) |
//!
//! Hosts report the new input events as `{"t":"click","id","x","y","mods","n"}`
//! (position, keys and click count optional, #159), `{"t":"pointer","id","phase","x","y"}`,
//! `{"t":"drag","id","phase","x","y","dx","dy"}` and
//! `{"t":"key","id","key","mods","repeat","sel"}` (`sel`: `[start, end]`,
//! the focused text field's selection in characters, #86),
//! `{"t":"keyup","id","key","mods"}` (a reported key released, #159) and
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
    /// A rich input's runs (encoded): its chips measure as wide as they
    /// are drawn (#329).
    rich_runs: HashMap<NodeId, String>,
    /// Icon names of images (what they measure).
    icons: HashMap<NodeId, String>,
    /// Control configs (a picker measures its options).
    configs: HashMap<NodeId, String>,
    /// What each countdown shows once ended (`timer(..).after`, #281).
    timer_after: HashMap<NodeId, String>,
    styles: HashMap<NodeId, Style>,
    /// Content size of each stack or tab container, reported by the host.
    pub viewports: HashMap<NodeId, (f32, f32)>,
    /// Each video's size, reported by the host once known (#207).
    pub video_sizes: HashMap<NodeId, (f32, f32)>,
    frames: HashMap<NodeId, (f32, f32)>,
    /// The content size last sent for each scroll view (`set_content_size`).
    content_sizes: HashMap<NodeId, (f32, f32)>,
    measure: Measure,
    /// The root view (overlays default to its size).
    root: Option<NodeId>,
    /// Whether the host edits rich text (#115, [`with_rich_inputs`](Self::with_rich_inputs)).
    rich: bool,
    /// Whether the host draws text, images and groups
    /// ([`with_drawing_items`](Self::with_drawing_items)).
    items: bool,
}

impl RemoteWidgets {
    pub fn new(measure: Measure) -> Self {
        RemoteWidgets {
            ops: Vec::new(),
            kinds: HashMap::new(),
            texts: HashMap::new(),
            rich_runs: HashMap::new(),
            icons: HashMap::new(),
            configs: HashMap::new(),
            timer_after: HashMap::new(),
            styles: HashMap::new(),
            viewports: HashMap::new(),
            video_sizes: HashMap::new(),
            frames: HashMap::new(),
            content_sizes: HashMap::new(),
            measure,
            root: None,
            rich: false,
            items: false,
        }
    }

    /// The host edits rich text (#115): a rich input is created as a
    /// `text_area` with `"rich": true`, and its runs come in a `rich` op
    /// (decoded like a text's, chips included). Otherwise it is a plain
    /// text area.
    pub fn with_rich_inputs(mut self) -> Self {
        self.rich = true;
        self
    }

    /// The host draws an SVG document's text, images and groups (#342): a
    /// drawing that has them comes as `items` in its `draw` op. Otherwise
    /// it comes as the paths alone, like any other drawing.
    pub fn with_drawing_items(mut self) -> Self {
        self.items = true;
        self
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
        // A chip of a rich input clicked (#327): with its box in the
        // field's coordinates (`x`, `y`, `w`, `h`, points). Without them,
        // the backend reports a plain `MenuSelect`.
        "menu" => {
            let (x, y, w, h) = (num("x")?, num("y")?, num("w")?, num("h")?);
            Some(UiEvent::RunClick(id?, prism_core::Rect::new(x, y, w, h)))
        }
        // With the keys held and the click's number in a series (`mods`,
        // `n`, #159) when the host says.
        "click" => {
            let mods = v.get("mods").and_then(Value::as_u64).unwrap_or(0) as u8;
            let count = v.get("n").and_then(Value::as_u64).unwrap_or(1).max(1) as u32;
            let position = match (num("x"), num("y")) {
                (Some(x), Some(y)) => Some(Point::new(x, y)),
                _ => None,
            };
            Some(match position {
                _ if mods != 0 || count > 1 => UiEvent::ClickWith(
                    id?,
                    prism_core::ClickEvent {
                        position,
                        modifiers: Modifiers::from_bits(mods),
                        count,
                        button: prism_core::PointerButton::Primary,
                        text_offset: None,
                        span_rect: None,
                        span_frame: None,
                    },
                ),
                Some(p) => UiEvent::ClickAt(id?, p),
                None => UiEvent::Click(id?),
            })
        }
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
        // A draggable's drag over a drop target (#251): `"over"` (the
        // target), `"ox"`, `"oy"` (the pointer in it, scrolling included).
        "drag" => {
            let e = DragEvent {
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
            };
            Some(match v.get("over").and_then(Value::as_u64) {
                Some(target) => UiEvent::PayloadDrag(
                    id?,
                    e,
                    Some(prism_core::DropPoint {
                        target,
                        position: Point::new(num("ox").unwrap_or(0.), num("oy").unwrap_or(0.)),
                    }),
                ),
                None => UiEvent::Drag(id?, e),
            })
        }
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

        // A key released (#159): like `key`, without a selection.
        "keyup" => Some(UiEvent::KeyUp(
            id.filter(|i| *i != 0),
            KeyEvent {
                key: v.get("key")?.as_str()?.to_string(),
                modifiers: Modifiers::from_bits(
                    v.get("mods").and_then(Value::as_u64).unwrap_or(0) as u8
                ),
                repeat: false,
                selection: None,
                edges: None,
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
                // Where the field's caret was (#211): `"sfl"`…
                edges: v
                    .get("edges")
                    .and_then(Value::as_str)
                    .map(prism_core::CaretEdges::decode),
            },
        )),
        // A pointer drag across a selection group's fields (#231, #264):
        // `{"t":"across","id":group,"a":[field,n],"h":[field,n]}`.
        "across" => {
            let end = |k: &str| {
                let p = v.get(k)?;
                Some((p.get(0)?.as_u64()?, p.get(1)?.as_u64()? as usize))
            };
            Some(UiEvent::SelectionAcross(id?, Some((end("a")?, end("h")?))))
        }
        // Text an input method typed over a selection across fields (#231).
        "typeacross" => Some(UiEvent::TypeAcross(
            id?,
            v.get("text")?.as_str()?.to_string(),
        )),
        "select" => Some(UiEvent::Select(
            id?,
            v.get("start")?.as_u64()? as usize,
            v.get("end")?.as_u64()? as usize,
        )),
        // A rich input's editing hooks (#217), as the web client sends them.
        "selrect" => {
            let num = |k: &str| v.get(k).and_then(Value::as_f64).map(|n| n as f32);
            Some(UiEvent::SelectionRect(
                id?,
                v.get("start")?.as_u64()? as usize,
                v.get("end")?.as_u64()? as usize,
                prism_core::Rect::new(num("x")?, num("y")?, num("w")?.max(0.), num("h")?.max(0.)),
            ))
        }
        "copy" => Some(UiEvent::Copy(
            id?,
            v.get("cut").and_then(Value::as_bool).unwrap_or(false),
            v.get("start")?.as_u64()? as usize,
            v.get("end")?.as_u64()? as usize,
            v.get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        )),
        "paste" => Some(UiEvent::Paste(id?, prism_core::PasteEvent::from_json(v)?)),
        "spanclick" => Some(UiEvent::SpanClick(
            id?,
            v.get("at")?.as_u64()? as usize,
            Modifiers::from_bits(v.get("mods").and_then(Value::as_u64).unwrap_or(0) as u8),
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
        // Dropped files that did not reach the app (#290):
        // `{"t":"drop_error","id","error":"failed","reason"}`.
        "drop_error" => Some(prism_core::UiEvent::DropFailed(
            id?,
            prism_core::DropError::from_name(v.get("error")?.as_str()?)?,
            v.get("reason")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
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
/// fill rule, `fg` / `sg` a [`gradient`] for the fill / stroke), `d` flat:
/// `0 x y` move, `1 x y` line, `2 x1 y1 x2 y2 x y` curve, `3` close.
fn drawing(paths: &[prism_core::Path]) -> Value {
    Value::Array(paths.iter().map(path).collect())
}

/// Segments as a flat list (see [`drawing`]).
fn segments(segments: &[prism_core::Segment]) -> Vec<f32> {
    use prism_core::Segment as S;
    let mut d: Vec<f32> = Vec::new();
    for s in segments {
        match *s {
            S::Move(x, y) => d.extend([0., x, y]),
            S::Line(x, y) => d.extend([1., x, y]),
            S::Cubic(a, b, c, e, f, g) => d.extend([2., a, b, c, e, f, g]),
            S::Close => d.push(3.),
            S::Arc { .. } => {}
        }
    }
    d
}

fn path(p: &prism_core::Path) -> Value {
    let mut path = json!({
        "d": segments(&p.segments),
        "fill": color(p.fill),
        "stroke": color(p.stroke),
        "w": p.width,
        "round": p.round,
        "dash": p.dash.map(|(a, b)| [a, b]),
    });
    if p.even_odd {
        path["eo"] = json!(true);
    }
    if let Some(g) = &p.fill_gradient {
        path["fg"] = gradient(g);
    }
    if let Some(g) = &p.stroke_gradient {
        path["sg"] = gradient(g);
    }
    path
}

/// A gradient: `k` `"l"` (`c`: `x1 y1 x2 y2`) or `"r"` (`c`: `cx cy r fx
/// fy`), `m` the transform from those coordinates to points (`a b c d e
/// f`), `sp` 0 pad, 1 reflect, 2 repeat, `o` the stops' offsets and `cl`
/// their colors.
fn gradient(g: &prism_core::shape::Gradient) -> Value {
    use prism_core::shape::{GradientShape, Spread};
    let (kind, coords) = match g.shape {
        GradientShape::Linear { x1, y1, x2, y2 } => ("l", vec![x1, y1, x2, y2]),
        GradientShape::Radial { cx, cy, r, fx, fy } => ("r", vec![cx, cy, r, fx, fy]),
    };
    json!({
        "k": kind,
        "c": coords,
        "m": g.transform,
        "sp": match g.spread {
            Spread::Pad => 0,
            Spread::Reflect => 1,
            Spread::Repeat => 2,
        },
        "o": g.stops.iter().map(|s| s.offset).collect::<Vec<_>>(),
        "cl": g.stops.iter().map(|s| color(Some(s.color))).collect::<Vec<_>>(),
    })
}

/// What a shape draws when it is more than paths
/// (`prism_core::shape::Item`), each with its kind `t`:
///
/// - `"p"`: a path, as in [`drawing`];
/// - `"x"`: a line of text: `m` its transform to points, `x` / `y` its
///   anchor, `a` 0 start, 1 middle, 2 end, `b` 0 alphabetic, 1 middle, 2
///   central, 3 hanging, `r` its runs `[{s, ff, z, w, it, k, c, dx, dy}]`
///   (text, family as in [`typography`], size, weight, italic, letter
///   spacing, color, room before, baseline shift down);
/// - `"i"`: an image: `m`, `r` its rectangle `x y w h`, `src` a `data:`
///   URL, `f` 0 meet, 1 slice, 2 stretch, `a` its alignment `x y`, `o` its
///   opacity;
/// - `"g"`: a group: `o` its opacity, `clip` `{d, eo}`, `mask` `{l, i}`
///   (luminance or alpha, its items), `sh` its shadows `[[dx, dy, blur,
///   color]]`, `b` its blur, `i` its items.
fn drawing_items(items: &[prism_core::shape::Item]) -> Value {
    use prism_core::shape::{Anchor, Baseline, Fit, Item};
    let all: Vec<Value> = items
        .iter()
        .map(|item| match item {
            Item::Path(p) => {
                let mut v = path(p);
                v["t"] = json!("p");
                v
            }
            Item::Text(t) => json!({
                "t": "x",
                "m": t.transform,
                "x": t.x,
                "y": t.y,
                "a": match t.anchor {
                    Anchor::Start => 0,
                    Anchor::Middle => 1,
                    Anchor::End => 2,
                },
                "b": match t.baseline {
                    Baseline::Alphabetic => 0,
                    Baseline::Middle => 1,
                    Baseline::Central => 2,
                    Baseline::Hanging => 3,
                },
                "r": t.runs.iter().map(|r| json!({
                    "s": r.text,
                    "ff": crate::fonts::resolve(&r.family).encode(),
                    "z": r.size,
                    "w": r.weight,
                    "it": r.italic,
                    "k": r.spacing,
                    "c": color(Some(r.fill)),
                    "dx": r.dx,
                    "dy": r.dy,
                })).collect::<Vec<_>>(),
            }),
            Item::Image(i) => json!({
                "t": "i",
                "m": i.transform,
                "r": [i.x, i.y, i.w, i.h],
                "src": i.src,
                "f": match i.fit {
                    Fit::Meet => 0,
                    Fit::Slice => 1,
                    Fit::Stretch => 2,
                },
                "a": [i.align.0, i.align.1],
                "o": i.opacity,
            }),
            Item::Group(g) => json!({
                "t": "g",
                "o": g.opacity,
                "clip": g.clip.as_ref().map(|c| json!({ "d": segments(&c.segments), "eo": c.even_odd })),
                "mask": g.mask.as_ref().map(|m| json!({ "l": m.luminance, "i": drawing_items(&m.items) })),
                "sh": g.shadows.iter().map(|s| json!([s.dx, s.dy, s.blur, color(Some(s.color))])).collect::<Vec<_>>(),
                "b": g.blur,
                "i": drawing_items(&g.items),
            }),
        })
        .collect();
    Value::Array(all)
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

/// A rich text's runs for a host: `[{t, w, i, u, uc, us, s, c, bg, ff, z,
/// link, href, icon, glyph}]`, only the keys a run sets (`t` text, `w` CSS
/// weight, `i`/`u`/`s` italic, underline, strikethrough, `uc` the
/// underline's color, `us` its style when not single (`thick`, `double`,
/// `dotted`, `dashed`, #221), `c`/`bg` colors, `ff`
/// family, `z` size as a multiple of the text's, `link` the id to report as
/// `{"t":"menu","id":link}` (with `x`, `y`, `w`, `h`: the chip's box in the
/// field, in points, when the host knows it, #327), `href` its path, `icon`
/// an SF Symbol name and `glyph` its fallback glyph, `tt` its tool tip; a
/// chip's `chip` key, `cl` label, `ci` icon glyph and options, see
/// `chip_options`).
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
            if r.underline_color.is_some() {
                m.insert("uc".into(), color(r.underline_color));
            }
            if r.underline_style != prism_core::UnderlineStyle::Single {
                m.insert("us".into(), json!(r.underline_style.name()));
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
            if let Some(h) = &r.href {
                m.insert("href".into(), json!(h));
            }
            if let Some(g) = &r.icon {
                m.insert("glyph".into(), json!(prism_core::glyphs::glyph(g)));
                m.insert("icon".into(), json!(g));
            }
            // A rich input's chip (#115): its key, label and icon's glyph.
            if let Some(k) = &r.chip {
                m.insert("chip".into(), json!(k));
                m.insert("cl".into(), json!(r.chip_label.clone().unwrap_or_default()));
                if let Some(g) = r.chip_icon.as_deref().filter(|_| r.chip_drawing.is_none()) {
                    m.insert("ci".into(), json!(prism_core::glyphs::glyph(g)));
                }
                chip_options(&mut m, &r);
            }
            if let Some(t) = &r.tooltip {
                m.insert("tt".into(), json!(t));
            }
            Value::Object(m)
        })
        .collect();
    Value::Array(runs)
}

/// A chip's options for a host (#329): `cv` its vector icon (paths of
/// [`drawing`] in a box 100 units tall, `cvr` times as wide; a path's
/// `fc` / `sc` is the alpha of a fill / stroke in the chip's text color),
/// `ring` `[width, color]`, `rad` its radius, `ht` its height, `mw` its
/// largest width (points), `nr` its label runs `[{t, w, i, c}]`, `st` its
/// state and `tid` its test id.
fn chip_options(m: &mut serde_json::Map<String, Value>, r: &prism_core::TextRun) {
    if let Some(d) = &r.chip_drawing {
        let ratio = r.chip_icon_ratio.filter(|v| *v > 0.).unwrap_or(1.);
        let current: Vec<(Option<f32>, Option<f32>)> = d
            .paths
            .iter()
            .map(|p| {
                let alpha = |c: Option<Color>| c.filter(|c| c.is_current()).map(|c| c.a);
                (alpha(p.fill), alpha(p.stroke))
            })
            .collect();
        let paths = d
            .with_current_color(Color::BLACK)
            .resolve(100. * ratio, 100., 1.);
        let mut paths = drawing(&paths);
        if let Value::Array(list) = &mut paths {
            for (p, (fc, sc)) in list.iter_mut().zip(current) {
                if let Some(a) = fc {
                    p["fc"] = json!(a);
                }
                if let Some(a) = sc {
                    p["sc"] = json!(a);
                }
            }
        }
        m.insert("cv".into(), paths);
        m.insert("cvr".into(), json!(ratio));
    }
    if let Some((w, c)) = r.chip_ring {
        m.insert("ring".into(), json!([w, color(Some(c))]));
    }
    for (key, v) in [
        ("rad", r.chip_radius),
        ("ht", r.chip_height),
        ("mw", r.chip_max_width),
    ] {
        if let Some(v) = v {
            m.insert(key.into(), json!(v));
        }
    }
    if !r.chip_label_runs.is_empty() {
        let runs: Vec<Value> = r
            .chip_label_runs
            .iter()
            .map(|l| {
                let mut o = json!({ "t": l.text });
                if let Some(w) = l.weight {
                    o["w"] = json!(w.css());
                }
                if l.italic {
                    o["i"] = json!(true);
                }
                if l.color.is_some() {
                    o["c"] = color(l.color);
                }
                o
            })
            .collect();
        m.insert("nr".into(), Value::Array(runs));
    }
    if let Some(st) = &r.chip_state {
        m.insert("st".into(), json!(st));
    }
    if let Some(t) = &r.chip_test_id {
        m.insert("tid".into(), json!(t));
    }
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
    /// as it may be), a text area at least its minimum lines, 17.5 em wide.
    fn measure_padded(
        &mut self,
        id: &NodeId,
        kind: NodeKind,
        style: &Style,
        max_width: Option<f32>,
    ) -> (f32, f32) {
        let spec = Value::Object(typography(style)).to_string();
        let spec = if spec == "{}" { "" } else { &spec };
        // A rich input's value with its runs (#329).
        let value = self.texts.get(id).cloned().unwrap_or_default();
        let rich = if kind == NodeKind::TextArea {
            self.rich_spec(id, &value, typography(style))
        } else {
            String::new()
        };
        let em = style.font_size;
        let width = match kind {
            NodeKind::TextInput => Some(max_width.unwrap_or(10. * em)),
            NodeKind::TextArea => Some(max_width.unwrap_or(17.5 * em)),
            _ => None,
        };
        let measure = self.measure;
        let at = |text: &str, max_width| {
            measure(&MeasureRequest {
                kind: "text",
                text,
                font_size: style.font_size,
                weight: weight(style.font_weight),
                max_width,
                spec: if !rich.is_empty() && text == value {
                    &rich
                } else {
                    spec
                },
            })
        };
        let (w, h) = match kind {
            NodeKind::TextInput => at("Ag", width),
            NodeKind::TextArea => self.measure_rows(id, width, at),
            _ => at(
                self.texts.get(id).map(String::as_str).unwrap_or(""),
                width.or(max_width),
            ),
        };
        (width.unwrap_or(w), h)
    }

    /// The spec a rich input's value `value` is measured with: `base`
    /// (its typography) with its runs (`"runs"`, as the `rich` op sends
    /// them, and `"rich": true`) when they hold that text, so the host
    /// lays its chips out as it draws them (#329); else `base` alone.
    fn rich_spec(&self, id: &NodeId, value: &str, base: serde_json::Map<String, Value>) -> String {
        let mut spec = base;
        if let Some(runs) = self.rich_runs.get(id).filter(|_| self.rich)
            && prism_core::decode_runs(runs)
                .iter()
                .map(|r| r.text.as_str())
                .collect::<String>()
                == value
        {
            spec.insert("rich".into(), json!(true));
            spec.insert("runs".into(), runs_json(runs));
        }
        if spec.is_empty() {
            String::new()
        } else {
            Value::Object(spec).to_string()
        }
    }

    /// A text area: its text, at least its minimum lines tall (#235; one
    /// for a rich input). Both are measured: a long line wraps to more.
    fn measure_rows(
        &self,
        id: &NodeId,
        max_width: Option<f32>,
        at: impl Fn(&str, Option<f32>) -> (f32, f32),
    ) -> (f32, f32) {
        // A rich input's value is measured with its runs (the spec), its
        // chips as wide and tall as drawn (#329); its minimum lines are
        // plain.
        let value = self.texts.get(id).map(String::as_str).unwrap_or("");
        let rows = prism_core::controls::InputConfig::decode(
            self.configs.get(id).map(String::as_str).unwrap_or(""),
        )
        .min_rows() as usize;
        let (w, h) = at(value, max_width);
        if value.split('\n').count() >= rows {
            return (w, h);
        }
        let lines = vec!["Ag"; rows].join("\n");
        let (_, min) = at(&lines, max_width);
        (w, h.max(min))
    }
}

impl Widgets for RemoteWidgets {
    type View = NodeId;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> NodeId {
        // A rich input is a text area that shows runs (#115): it measures
        // and takes its config as one.
        if kind == NodeKind::RichInput {
            self.kinds.insert(id, NodeKind::TextArea);
            self.ops.push(
                json!({ "op": "create", "id": id, "kind": NodeKind::TextArea.name(), "rich": true }),
            );
            return id;
        }
        self.kinds.insert(id, kind);
        self.ops
            .push(json!({ "op": "create", "id": id, "kind": kind.name() }));
        id
    }

    fn rich_input(&self) -> bool {
        self.rich
    }

    fn set_rich_value(&mut self, id: &NodeId, runs: &str) {
        self.rich_runs.insert(*id, runs.to_string());
        self.ops
            .push(json!({ "op": "rich", "id": id, "runs": runs_json(runs) }));
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
        // What an optional date picker measures while empty (#89), and a
        // countdown once it ends (#281).
        match self.kinds.get(id) {
            Some(NodeKind::DatePicker) => {
                self.texts.insert(*id, text.to_string());
            }
            Some(NodeKind::Timer) => {
                self.timer_after.insert(*id, text.to_string());
            }
            _ => {}
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
        self.content_sizes.remove(id);
        self.kinds.remove(id);
        self.texts.remove(id);
        self.rich_runs.remove(id);
        self.icons.remove(id);
        self.configs.remove(id);
        self.timer_after.remove(id);
        self.styles.remove(id);
        self.viewports.remove(id);
        self.frames.remove(id);
        self.ops.push(json!({ "op": "destroy", "id": id }));
    }

    fn measure(&mut self, id: &NodeId, max_width: Option<f32>) -> (f32, f32) {
        let kind = self.kinds.get(id).copied().unwrap_or(NodeKind::View);
        // A video (#207): its size once the host knows it, else 16:9.
        if kind == NodeKind::Video {
            return match self.video_sizes.get(id) {
                Some(&(w, h)) if w > 0. => {
                    let shown = max_width.map_or(w, |m| m.min(w));
                    (shown, shown * h / w)
                }
                _ => {
                    let w = max_width.unwrap_or(320.);
                    (w, w * 9. / 16.)
                }
            };
        }
        let style = self.styles.get(id).cloned().unwrap_or_default();
        if style.has_padding() && kind != NodeKind::Text && kind != NodeKind::Timer {
            return self.measure_padded(id, kind, &style, max_width);
        }
        if kind == NodeKind::TextArea {
            let measure = self.measure;
            let value = self.texts.get(id).map(String::as_str).unwrap_or("");
            let runs = self.rich_spec(id, value, serde_json::Map::new());
            return self.measure_rows(id, max_width, |text, max_width| {
                measure(&MeasureRequest {
                    kind: kind.name(),
                    text,
                    font_size: style.font_size,
                    weight: weight(style.font_weight),
                    max_width,
                    spec: if text == value { &runs } else { "" },
                })
            });
        }
        let text = match kind {
            // As wide as its digits, or its `after` text when wider (#281).
            NodeKind::Timer => match self.timer_after.get(id) {
                Some(after) if after.chars().count() > 5 => after.clone(),
                _ => "00:00".to_string(),
            },
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
        let mut op = json!({
            "op": "a11y", "id": id, "label": a11y.label, "hidden": a11y.hidden,
            "role": a11y.role.map(|r| r.aria()), "value": a11y.value,
            // Switches, checkboxes and drawn radios (#84): on or chosen.
            "checked": a11y.checked,
            "selected": a11y.is_selected(),
        });
        // A disclosure (#291): expanded or collapsed, only when it is one.
        if let Some(expanded) = a11y.expanded {
            op["expanded"] = json!(expanded);
        }
        // A tooltip (#216): its text, or `true` for the full text while
        // ellipsized.
        match &a11y.tooltip {
            Some(prism_core::Tooltip::Text(t)) => op["tip"] = json!(t),
            Some(prism_core::Tooltip::WhenTruncated) => op["tip"] = json!(true),
            None => {}
        }
        self.ops.push(op);
    }

    fn set_table(&mut self, table: &NodeId, rows: &[crate::table::TableRow<'_, NodeId>]) {
        let rows: Vec<Value> = rows
            .iter()
            .map(|r| json!({ "id": r.view, "header": r.header, "cells": r.cells }))
            .collect();
        self.ops
            .push(json!({ "op": "table", "id": table, "rows": rows }));
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

    fn set_drawing_items(&mut self, id: &NodeId, items: &[prism_core::shape::Item]) {
        if !self.items {
            return self.set_drawing(id, &prism_core::shape::flatten(items));
        }
        self.ops
            .push(json!({ "op": "draw", "id": id, "items": drawing_items(items) }));
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
        // Every layout reports it, and the host lays its content out again
        // on each `content` op: unchanged sizes aren't sent (#316). Overlays
        // still get every one (they may open on it).
        if self.kinds.get(id) != Some(&NodeKind::Overlay)
            && self.content_sizes.insert(*id, (width, height)) == Some((width, height))
        {
            return;
        }
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
            "expanded": env.expanded,
            "collapsed": env.collapsed,
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

    fn set_drag_delay(&mut self, id: &NodeId, ms: u32) {
        self.ops
            .push(json!({ "op": "dragdelay", "id": id, "ms": ms }));
    }

    fn set_drop_target(&mut self, id: &NodeId, types: Option<&[String]>) {
        self.ops
            .push(json!({ "op": "drop", "id": id, "types": types }));
    }

    /// `{"op":"dnd","id","kind","preview","accepts"}` (#251): the host
    /// finds the drop target under a draggable's drag and reports it with
    /// the drag (`over`, `ox`, `oy`), shows the preview and scrolls.
    fn set_drag_and_drop(
        &mut self,
        id: &NodeId,
        kind: Option<&str>,
        preview: prism_core::DragPreview,
        accepts: &[String],
    ) {
        self.ops.push(json!({
            "op": "dnd",
            "id": id,
            "kind": kind,
            "preview": preview == prism_core::DragPreview::Snapshot,
            "accepts": accepts,
        }));
    }

    fn set_key_events(&mut self, id: &NodeId, local: bool, global: bool) {
        self.ops
            .push(json!({ "op": "keys", "id": id, "local": local, "global": global }));
    }

    /// `{"op":"consume","id","keys"}` (#181): the host keeps the focused
    /// control from acting on these keys.
    fn set_consumed_keys(&mut self, id: &NodeId, keys: &[String]) {
        self.ops
            .push(json!({ "op": "consume", "id": id, "keys": keys }));
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

    /// Other people's carets (#240): drawn in the field's overlay.
    fn set_remote_carets(&mut self, id: &NodeId, carets: &[prism_core::RemoteCaret]) {
        let carets: Vec<Value> = carets
            .iter()
            .map(|c| {
                json!({
                    "anchor": c.anchor,
                    "head": c.head,
                    "color": color(Some(c.color)),
                    "label": c.label,
                    // The label's text (#267); white by default.
                    "text": color(c.label_text_color),
                    "idle": c.idle,
                })
            })
            .collect();
        self.ops
            .push(json!({ "op": "carets", "id": id, "carets": carets }));
    }

    fn set_edit_events(&mut self, id: &NodeId, mask: u8) {
        self.ops
            .push(json!({ "op": "editev", "id": id, "mask": mask }));
    }

    fn write_clipboard(&mut self, id: &NodeId, content: &prism_core::ClipboardContent) {
        let items: Vec<Value> = content
            .entries()
            .into_iter()
            .map(|(m, d)| json!([m, d]))
            .collect();
        self.ops
            .push(json!({ "op": "clip", "id": id, "items": items }));
    }

    fn insert_text(&mut self, id: &NodeId, text: &str) {
        self.ops
            .push(json!({ "op": "instext", "id": id, "text": text }));
    }

    /// `{"op":"selhl","id","start","end"}` (#231): a field's part of a
    /// selection across fields, drawn like a selection (`start == end`:
    /// none).
    fn set_selection_highlight(&mut self, id: &NodeId, start: usize, end: usize) {
        self.ops
            .push(json!({ "op": "selhl", "id": id, "start": start, "end": end }));
    }

    /// `{"op":"selgroup","id","on"}` (#231): the host extends a pointer
    /// drag across the group's text fields (`across` events).
    fn set_selection_group(&mut self, id: &NodeId, on: bool) {
        self.ops
            .push(json!({ "op": "selgroup", "id": id, "on": on }));
    }

    fn scroll_into_view(&mut self, id: &NodeId) {
        self.ops.push(json!({ "op": "reveal", "id": id }));
    }

    /// `{"op":"popup","id","menu","at"}` (#247): a pop-up menu over the
    /// node (`id` null: the window), at `at` in pixels from its top-left
    /// corner, or below it when `at` is null.
    fn show_menu(
        &mut self,
        id: Option<&NodeId>,
        menu: &prism_core::menu::MenuSpec,
        at: Option<(f32, f32)>,
    ) {
        let menu = serde_json::from_str::<Value>(&menu.to_json()).unwrap_or(Value::Null);
        self.ops.push(json!({
            "op": "popup", "id": id, "menu": menu, "at": at.map(|(x, y)| [x, y]),
        }));
    }

    /// `{"op":"scrollto","id","x","y","rel","anim"}` (#193): `x`, `y` in
    /// the host's units, the renderer's zoom already applied (pixels on
    /// Android, #308; by them from the offset when `rel`), the host keeps
    /// them within the content.
    fn scroll_to(&mut self, id: &NodeId, x: f32, y: f32, relative: bool, animated: bool) {
        let clamp = |v: f32| if v.is_finite() { v.min(1e9) } else { 1e9 };
        self.ops.push(json!({
            "op": "scrollto", "id": id, "x": clamp(x), "y": clamp(y),
            "rel": relative, "anim": animated,
        }));
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

    fn set_keyboard_bar(&mut self, field: &NodeId, bar: Option<&NodeId>) {
        self.ops
            .push(json!({ "op": "kbar", "id": field, "bar": bar }));
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
            // A folder and its files (#225).
            DeviceCall::PickFolder(o) => json!({ "call": "pick_folder", "title": o.title }),
            DeviceCall::ReadClipboard => json!({ "call": "paste" }),
            // #246: the host answers files named by their type.
            DeviceCall::ReadClipboardItem => json!({ "call": "paste_item" }),
            // Text types as text, others in base64.
            DeviceCall::WriteClipboard(item) => json!({
                "call": "copy_item",
                "entries": item.entries().map(|(ty, data)| {
                    if prism_core::ClipboardItem::is_text(ty, data) {
                        json!({ "type": ty, "text": String::from_utf8_lossy(data) })
                    } else {
                        json!({ "type": ty, "data": prism_core::clipboard::base64(data) })
                    }
                }).collect::<Vec<_>>(),
            }),
            DeviceCall::Haptic(h) => json!({ "call": "haptic", "kind": h.name() }),
            DeviceCall::CopyText(t) => json!({ "call": "copy", "text": t }),
            DeviceCall::Share(s) => {
                json!({ "call": "share", "text": s.text, "url": s.url, "files": s.files })
            }
            DeviceCall::OpenUrl(u) => json!({ "call": "open_url", "url": u }),
            // #210: the host stages bytes first (`crate::save::stage`), so a
            // file arrives here; the host removes a staged copy once done.
            DeviceCall::SaveFile(f) => match &f.source {
                prism_core::SaveSource::Path(p) => json!({
                    "call": "save_file", "path": p.to_string_lossy(),
                    "name": f.file_name(), "mime": f.mime_type(),
                }),
                prism_core::SaveSource::Bytes(_) => json!({ "call": "unavailable" }),
            },
            DeviceCall::RevealPath(p) => json!({ "call": "reveal", "path": p.to_string_lossy() }),
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
                // Apple gets the nonce hashed (the token's `nonce` claim).
                "nonce": r.apple_nonce(), "state": r.state,
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
                "conditional": r.conditional,
            }),
            // No cookies natively: the host answers « unavailable ».
            DeviceCall::SetCookie(_) => json!({ "call": "set_cookie" }),
            // No live session to renew: the host ignores it.
            DeviceCall::Reconnect => json!({ "call": "reconnect" }),
            // No browser id natively (`renew_browser_id` never sends it).
            DeviceCall::RenewBrowserId => json!({ "call": "renew_browser_id" }),
            // In-app purchases (#306): Google Play Billing (PrismBilling).
            // Products answer `text` (a JSON array), a purchase `text`
            // `purchased` / `pending`, entitlements `text` (a JSON array of
            // ids); the watch sends `entitlements` events.
            DeviceCall::StoreProducts(ids) => json!({ "call": "store_products", "ids": ids }),
            DeviceCall::Purchase(p) => json!({
                "call": "purchase", "product": p.product, "account": p.account_token,
            }),
            DeviceCall::StoreEntitlements => json!({ "call": "store_entitlements" }),
            DeviceCall::RestorePurchases => json!({ "call": "restore_purchases" }),
            DeviceCall::WatchEntitlements => json!({ "call": "watch_entitlements" }),
            // A checkout page coming back to the server is a web thing.
            DeviceCall::StoreReturnUrl | DeviceCall::StoreCheckout { .. } => {
                json!({ "call": "unavailable" })
            }
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

    /// A `device_event` of a stream: an error, a location, what the user
    /// owns (`entitlements`, product ids) or motion.
    pub fn event(v: &Value) -> DeviceEvent {
        if let Some(err) = v.get("err").and_then(Value::as_str) {
            DeviceEvent::Error(DeviceError::from_message(err))
        } else if let Some(ids) = v.get("entitlements").and_then(Value::as_array) {
            DeviceEvent::Entitlements(
                ids.iter()
                    .filter_map(|id| id.as_str().map(str::to_string))
                    .collect(),
            )
        } else if let Some(l) = v.get("location").and_then(location) {
            DeviceEvent::Location(l)
        } else {
            DeviceEvent::Motion(motion(v))
        }
    }
}

#[cfg(test)]
mod apple_tests {
    /// S39: what Apple gets as the nonce (and puts in the identity token's
    /// `nonce` claim) is the SHA-256, in hex, of the app's nonce, as
    /// `AppleSignIn::nonce` documents: the server hashes the nonce it
    /// handed out and compares.
    #[test]
    fn s39_apple_gets_the_nonce_hashed() {
        let request = prism_core::AppleSignIn::new()
            .nonce("server-nonce-39")
            .state("st-39");
        let sent = super::device::call(7, prism_core::DeviceCall::SignInWithApple(request));
        assert_eq!(
            sent["nonce"],
            "3c0f900191b65765eddb001bcd76dc504a5b1a4cc3a231e1463606a05a69e377"
        );
        assert_eq!(sent["state"], "st-39");
        let none = super::device::call(
            8,
            prism_core::DeviceCall::SignInWithApple(Default::default()),
        );
        assert!(none["nonce"].is_null());
    }
}

#[cfg(test)]
mod scene_tests {
    #[test]
    fn purchases_speak_play_billing_json() {
        use prism_core::{DeviceCall, DeviceError, DeviceEvent, Purchase};
        let sent = super::device::call(3, DeviceCall::StoreProducts(vec!["pro".into()]));
        assert_eq!(sent["call"], "store_products");
        assert_eq!(sent["ids"], serde_json::json!(["pro"]));
        let sent = super::device::call(
            4,
            DeviceCall::Purchase(
                Purchase::new("pro").account_token("8b2f0c1e-0000-4000-8000-000000000001"),
            ),
        );
        assert_eq!(sent["call"], "purchase");
        assert_eq!(sent["product"], "pro");
        assert_eq!(sent["account"], "8b2f0c1e-0000-4000-8000-000000000001");
        assert!(
            super::device::call(5, DeviceCall::Purchase(Purchase::new("pro")))["account"].is_null()
        );
        for (call, name) in [
            (DeviceCall::StoreEntitlements, "store_entitlements"),
            (DeviceCall::RestorePurchases, "restore_purchases"),
            (DeviceCall::WatchEntitlements, "watch_entitlements"),
            (DeviceCall::StoreReturnUrl, "unavailable"),
        ] {
            assert_eq!(super::device::call(6, call)["call"], name);
        }
        let owned = super::device::event(&serde_json::json!({ "entitlements": ["pro", "themes"] }));
        assert!(matches!(owned, DeviceEvent::Entitlements(ids) if ids == ["pro", "themes"]));
        // Play's named errors keep their name.
        let failed =
            super::device::event(&serde_json::json!({ "err": "NETWORK_ERROR\u{1f}offline" }));
        assert!(
            matches!(failed, DeviceEvent::Error(DeviceError::Platform { ref name, .. }) if name == "NETWORK_ERROR"),
            "{failed:?}"
        );
    }

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
    /// #115: where the host edits rich text, a rich input is a text area
    /// marked rich, and its runs (chips included) come in a `rich` op;
    /// elsewhere (watchOS) it stays a plain text area.
    #[test]
    fn a_rich_input_is_a_rich_text_area_with_its_runs() {
        use prism_core::{Backend, NodeKind, Patch};
        let runs = "Hi \u{1e}\u{fffc}\u{1f}k=p1\u{1f}n=Road\u{1f}m=star\u{1f}l=9";
        let patches = [
            Patch::Create {
                id: 1,
                kind: NodeKind::RichInput,
            },
            Patch::SetRichValue {
                id: 1,
                runs: runs.into(),
            },
            Patch::SetRoot { id: 1 },
        ];
        let mut r =
            crate::NativeRenderer::new(super::RemoteWidgets::new(|_| (0., 0.)).with_rich_inputs());
        r.apply(&patches);
        let ops = &r.widgets.ops;
        assert_eq!(ops[0]["kind"], "text_area");
        assert_eq!(ops[0]["rich"], true);
        let rich = ops.iter().find(|o| o["op"] == "rich").expect("a rich op");
        assert_eq!(rich["runs"][0]["t"], "Hi ");
        let chip = &rich["runs"][1];
        assert_eq!(chip["t"], "\u{fffc}");
        assert_eq!((&chip["chip"], &chip["cl"]), (&"p1".into(), &"Road".into()));
        assert_eq!(chip["ci"], prism_core::glyphs::glyph("star"));
        assert_eq!(chip["link"], 9);

        let mut plain = crate::NativeRenderer::new(super::RemoteWidgets::new(|_| (0., 0.)));
        plain.apply(&patches);
        assert_eq!(plain.widgets.ops[0]["kind"], "text_area");
        assert!(plain.widgets.ops[0].get("rich").is_none());
        assert!(!plain.widgets.ops.iter().any(|o| o["op"] == "rich"));
    }

    /// #329: a rich input's value is measured with its runs, so the host
    /// lays its chips out as it draws them (a chip's U+FFFC alone is one
    /// narrow character); its minimum lines are plain text.
    #[test]
    fn a_rich_input_is_measured_with_its_runs() {
        use crate::Widgets;
        use prism_core::{NodeKind, TextRun, encode_runs};
        use std::sync::Mutex;
        static SPECS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
        fn measure(m: &super::MeasureRequest) -> (f32, f32) {
            let mut specs = SPECS.lock().unwrap_or_else(|e| e.into_inner());
            specs.push((m.text.to_string(), m.spec.to_string()));
            (100., 20.)
        }
        let runs = encode_runs(&[
            TextRun {
                text: "Hi ".into(),
                ..TextRun::default()
            },
            TextRun {
                text: "\u{fffc}".into(),
                chip: Some("u".into()),
                chip_label: Some("Ada Lovelace".into()),
                chip_max_width: Some(90.),
                ..TextRun::default()
            },
        ]);
        let mut w = super::RemoteWidgets::new(measure).with_rich_inputs();
        w.create(1, NodeKind::RichInput);
        w.set_text(&1, "Hi \u{fffc}");
        w.set_rich_value(&1, &runs);
        w.measure(&1, None);
        let specs = SPECS.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let (_, spec) = specs
            .iter()
            .find(|(t, _)| t == "Hi \u{fffc}")
            .expect("the value measured");
        let spec: serde_json::Value = serde_json::from_str(spec).expect("a JSON spec");
        assert_eq!(spec["rich"], true);
        assert_eq!(spec["runs"][1]["cl"], "Ada Lovelace");
        assert_eq!(spec["runs"][1]["mw"], 90.);
        // The minimum lines: plain.
        assert!(
            specs
                .iter()
                .any(|(t, s)| t.starts_with("Ag") && s.is_empty())
        );
        // Stale runs (the user typed since): the value alone.
        SPECS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        w.set_text(&1, "Hi \u{fffc}!");
        w.measure(&1, None);
        let specs = SPECS.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert!(specs.iter().all(|(_, s)| s.is_empty()));
    }

    /// #329: a chip's options reach the host: its vector icon in a box 100
    /// units tall (its text color marked), ring, radius, height, largest
    /// width, label runs, tool tip, state and test id.
    #[test]
    fn a_chips_options_reach_the_host() {
        use prism_core::{Color, Drawing, FontWeight, Path, TextRun, encode_runs, rgb};
        let chip = TextRun {
            text: "\u{fffc}".into(),
            chip: Some("u".into()),
            chip_label: Some("Ada · 3".into()),
            chip_label_runs: vec![
                TextRun {
                    text: "Ada".into(),
                    weight: Some(FontWeight::Bold),
                    ..TextRun::default()
                },
                TextRun {
                    text: " · 3".into(),
                    italic: true,
                    color: Some(rgb(0x8e8e93)),
                    ..TextRun::default()
                },
            ],
            chip_icon: None,
            chip_drawing: Some(Drawing {
                inset: (0., 0.),
                paths: vec![
                    Path::rect(0., 0., 1., 1.).fill(Color::current().with_alpha(0.5)),
                    Path::rect(0.25, 0.25, 0.5, 0.5).fill(rgb(0xff0000)),
                ],
                ..Default::default()
            }),
            chip_icon_ratio: Some(2.),
            tooltip: Some("Ada Lovelace".into()),
            chip_ring: Some((1.5, rgb(0x0a84ff))),
            chip_radius: Some(9.),
            chip_height: Some(22.),
            chip_max_width: Some(120.),
            chip_state: Some("open".into()),
            chip_test_id: Some("ada".into()),
            ..TextRun::default()
        };
        let runs = super::runs_json(&encode_runs(&[chip]));
        let c = &runs[0];
        assert_eq!(c["tt"], "Ada Lovelace");
        assert_eq!((&c["st"], &c["tid"]), (&"open".into(), &"ada".into()));
        assert_eq!(
            (&c["rad"], &c["ht"], &c["mw"]),
            (&9.0.into(), &22.0.into(), &120.0.into())
        );
        assert_eq!(c["ring"][0], 1.5);
        assert_eq!(c["nr"][0]["t"], "Ada");
        assert_eq!(c["nr"][0]["w"], 700);
        assert_eq!(c["nr"][1]["i"], true);
        assert!(c["nr"][1]["c"].is_array());
        assert_eq!(c["cvr"], 2.0);
        let icon = c["cv"].as_array().expect("paths");
        assert_eq!(icon.len(), 2);
        let half = icon[0]["fc"].as_f64().expect("the chip's text color");
        assert!((half - 0.5).abs() < 0.01, "at half: {half}");
        assert!(icon[1].get("fc").is_none());
        // In a box 100 units tall, twice as wide: the rect's right edge.
        let d: Vec<f64> = icon[0]["d"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        assert!(d.contains(&200.), "{d:?}");
        assert!(c.get("ci").is_none(), "a drawing replaces the glyph");
    }
}
