//! `Widgets` for hosts in another language (the watchOS SwiftUI host, the
//! Android Java host): the same flexbox layout as iOS and macOS, computed in
//! Rust. The host only measures text and draws each node at the frame it is
//! given, so layouts match the other platforms. Changes are queued as JSON
//! (`ops`) for the backend to send once per commit:
//!
//! | op | fields |
//! |---|---|
//! | `create` / `destroy` | `id`, `kind` |
//! | `style` | `s`: `bg`, `radius`, `bw`, `bc`, `opacity`, `color`, `font`, `weight`, `talign` (colors `[r,g,b,a, dark r,g,b,a]`) |
//! | `text`, `placeholder`, `icon`, `value`, `clickable`, `display` | the value |
//! | `a11y` | `label`, `hidden`, `role` (ARIA name: `slider`, `combobox`, `status`…), `value` |
//! | `config` | `config`: a control's config string (`prism_core::controls`: slider `min max step`, picker options one per line, date `mode min max`, input tokens) |
//! | `children` | `children`: ids in order |
//! | `frame` | `f`: `[x, y, w, h]` in the parent |
//! | `content` | `size`: a scroll view's content size |
//! | `menu`, `swipe`, `scrollev`, `root` | context menu JSON, swipe sides, scroll events, root id |
//! | `env` | `lang`, `rtl`, `back`, `close` (no `id`) |
//! | `exit` | `parent`, `to`: `{o, tf}` the view animates to (a picture of it), before it is removed |
//! | `repeat` | `from`, `to`: `{o, tf}` (`to` null stops), back and forth forever |
//! | `draw` | `paths`: `[{d, fill, stroke, w, round, dash}]` in points, `d` flat (`0 x y` move, `1 x y` line, `2 x1 y1 x2 y2 x y` curve, `3` close) |
//!
//! Styles carry `tf`: `[x, y, scale, degrees]` (null: none). `style`, `frame`,
//! `exit` and `repeat` may carry `anim`: `{d, delay, ease}` (seconds, cubic
//! Bézier `[x1, y1, x2, y2]`): animate to the new values.
//! | `appearance` | `mode`: `system`, `light` or `dark` (no `id`) |
//! | `states` | `on`: the interaction states the host reports for the node (`hover`, `active`, `focus`) |
//!
//! Box decorations, in `style` only when set: `shadows`: `[{x, y, blur,
//! spread, color, inset}]` (the first on top); `sides`: per-side borders
//! `[[w, color] × 4]` top, right, bottom, left (already mirrored right to
//! left); `bstyle`: `dashed` or `dotted`; `clip`: children clipped to the
//! rounded box; `grad`: `{kind: "linear" | "radial", angle, stops: [[color,
//! at]]}` (angle in CSS degrees, 180 top to bottom); `blur`: backdrop blur
//! radius; `cursor`: a CSS cursor name. Colors are already resolved
//! ([`prism_core::Color::current`] included).
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
    /// date picker's config (`date`, `time`, `datetime` first), `large` for a
    /// large spinner.
    pub text: &'a str,
    pub font_size: f32,
    /// 400, 500, 600, 700.
    pub weight: i32,
    pub max_width: Option<f32>,
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
        }
    }
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

/// `[{d, fill, stroke, w, round, dash}]`, `d` flat: `0 x y` move, `1 x y`
/// line, `2 x1 y1 x2 y2 x y` curve, `3` close.
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
            json!({
                "d": d,
                "fill": color(p.fill),
                "stroke": color(p.stroke),
                "w": p.width,
                "round": p.round,
                "dash": p.dash.map(|(a, b)| [a, b]),
            })
        })
        .collect();
    Value::Array(paths)
}

fn transform(t: Transform) -> Value {
    if t.is_identity() {
        Value::Null
    } else {
        json!([t.x, t.y, t.scale, t.rotate])
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
    match w {
        FontWeight::Regular => 400,
        FontWeight::Medium => 500,
        FontWeight::Semibold => 600,
        FontWeight::Bold => 700,
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
        // Only some corners rounded: bits 1 top left, 2 top right, 4 bottom
        // right, 8 bottom left (already mirrored for right to left).
        if s.rounded_corners != prism_core::Corners::ALL {
            op["s"]["corners"] = json!(s.rounded_corners.bits());
        }
        decorations(&mut op["s"], s);
        op
    }
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
                json!({
                    "op": "repeat", "id": id, "from": pose(s.opacity, s.transform),
                    "to": pose(o, tf), "anim": anim(t),
                })
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
            NodeKind::DatePicker => self.configs.get(id).cloned().unwrap_or_default(),
            _ => self.texts.get(id).cloned().unwrap_or_default(),
        };
        (self.measure)(&MeasureRequest {
            kind: kind.name(),
            text: &text,
            font_size: style.font_size,
            weight: weight(style.font_weight),
            max_width,
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
        let mut widest = 0f32;
        for word in words {
            self.texts.insert(*id, word);
            widest = widest.max(self.measure(id, None).0);
        }
        if let Some(full) = full {
            self.texts.insert(*id, full);
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
    fn set_config(&mut self, id: &NodeId, _kind: NodeKind, config: &str) {
        self.configs.insert(*id, config.to_string());
        self.ops
            .push(json!({ "op": "config", "id": id, "config": config }));
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
}

/// Device calls as JSON for a host (the same shape as the web client's), and
/// the host's answers back.
pub mod device {
    use prism_core::{
        Accuracy, DeviceCall, DeviceError, DeviceEvent, DeviceValue, Location, Motion,
        PermissionState, PickedFile,
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
            DeviceCall::ReadClipboard => json!({ "call": "paste" }),
            DeviceCall::Haptic(h) => json!({ "call": "haptic", "kind": h.name() }),
            DeviceCall::CopyText(t) => json!({ "call": "copy", "text": t }),
            DeviceCall::Share(s) => {
                json!({ "call": "share", "text": s.text, "url": s.url, "files": s.files })
            }
            DeviceCall::OpenUrl(u) => json!({ "call": "open_url", "url": u }),
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
            let s = |f: &Value, k: &str| f.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            return Ok(DeviceValue::Files(
                files
                    .iter()
                    .map(|f| PickedFile {
                        path: s(f, "path"),
                        name: s(f, "name"),
                        mime: s(f, "mime"),
                        size: f.get("size").and_then(Value::as_u64).unwrap_or(0),
                    })
                    .collect(),
            ));
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
