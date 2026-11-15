//! System surfaces outside the app window: **widgets**, **Live Activities**
//! (lock screen + Dynamic Island) and **notifications**.
//!
//! The app describes them with the same elements as its UI (`div`, `text`,
//! `image`, `progress`, `timer`, `link`). Prism turns them into static
//! [`SNode`] snapshots (compact JSON) that a platform renders natively, even
//! when the app is not running:
//!
//! | Surface | iOS | macOS | Web | Android (planned) |
//! |---|---|---|---|---|
//! | Widgets | WidgetKit (SwiftUI) | WidgetKit | — | Glance |
//! | Live Activities | ActivityKit: lock screen + Dynamic Island | menu bar item | in-page live region | Live Updates notification |
//! | Notifications | UserNotifications | UserNotifications | Notification API / toast | NotificationCompat |
//!
//! Code never branches on the OS: it asks [`App::capabilities`] what the
//! current platform supports, and every request has a sensible fallback.
//! Backends implement [`PlatformServices`].

use std::fmt::Write;
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use crate::a11y::Role;
use crate::app::App;
use crate::element::{Element, IntoElement, NodeKind, VNode};
use crate::style::{Align, Color, FlexDirection, FontWeight, Justify, Length, TextAlign};

// ---------------------------------------------------------------------------
// Capabilities & the backend contract
// ---------------------------------------------------------------------------

/// What the running platform can show outside the app window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// Home-screen / desktop widgets (`cx.register_widget`).
    pub widgets: bool,
    /// CarPlay / Android Auto templates.
    pub car: bool,
    /// Live Activities (`cx.start_activity`), natively or as a fallback.
    pub live_activities: bool,
    /// Live Activities also appear in a Dynamic Island (iPhone 14 Pro and later).
    pub dynamic_island: bool,
    /// Local notifications (`cx.send_notification`).
    pub notifications: bool,
    /// Tray / menu bar extras (`cx.set_tray`).
    pub tray: bool,
    /// A menu bar for app menus (`cx.set_app_menu`).
    pub app_menu: bool,
    /// Device APIs (see [`crate::device`]): `current_location`, `watch_location`.
    pub location: bool,
    /// `watch_motion` (accelerometer, gyroscope, attitude).
    pub motion: bool,
    /// `take_photo`.
    pub camera: bool,
    /// `pick_images`.
    pub photos: bool,
    /// `haptic` feedback.
    pub haptics: bool,
    /// The system share sheet (`share`).
    pub share: bool,
    /// `copy_text` / `read_clipboard` (none on tvOS and watchOS).
    pub clipboard: bool,
    /// `cloud_storage` syncs between devices (otherwise it is local).
    pub cloud_sync: bool,
    /// `cloud_storage` is backed up and restored on a new device or a
    /// reinstall, without live sync (Android Backup).
    pub cloud_backup: bool,
    /// `secure_storage` keeps secrets in the platform's secure store.
    pub secure_storage: bool,
    /// The biometric sensor (`BiometryKind::None` without one). Enrollment
    /// can change while the app runs: `cx.biometry()` says how it is now.
    pub biometry: crate::security::BiometryKind,
    /// `authenticate` can check the owner (biometrics or a passcode).
    pub device_auth: bool,
    /// `sign_in_with_apple` (AuthenticationServices, with the entitlement).
    pub sign_in_with_apple: bool,
    /// Browser sign-in sessions (`prism::oauth`).
    pub web_auth: bool,
    /// Remote pushes (`register_for_push`), and the service they use.
    pub push: Option<crate::push::PushService>,
}

/// Work the app asked the platform to do. Backends drain these with
/// [`Runtime::take_requests`](crate::Runtime::take_requests).
#[derive(Clone, Debug, PartialEq)]
pub enum PlatformRequest {
    /// Re-rendered snapshots of every registered widget.
    ReloadWidgets(Vec<WidgetSnapshot>),
    /// Show a new Live Activity ([`App::start_activity`]).
    StartActivity {
        /// Identifies the activity in later updates.
        id: ActivityId,
        /// The activity's kind, as passed to [`Activity::new`].
        kind: String,
        /// Rendered regions.
        content: ActivityContent,
    },
    /// Replace a running activity's content ([`App::update_activity`]).
    UpdateActivity {
        /// The activity to update.
        id: ActivityId,
        /// Its new regions.
        content: ActivityContent,
    },
    /// Stop an activity ([`App::end_activity`]).
    EndActivity {
        /// The activity to end.
        id: ActivityId,
        /// A final state left on the lock screen, if any.
        content: Option<ActivityContent>,
        /// Remove it at once instead of letting the system dismiss it later.
        immediately: bool,
    },
    /// Post a local notification ([`App::send_notification`]).
    Notify(Notification),
    /// Present an alert or action sheet.
    ShowAlert(crate::menu::AlertSpec),
    /// Create or replace a tray / menu bar item.
    SetTray(crate::menu::TraySpec),
    /// Remove the tray item with this id.
    RemoveTray(String),
    /// Replace the app's menu bar.
    SetAppMenu(Vec<crate::menu::MenuSpec>),
    /// Withdraw a pending or delivered notification, by id.
    CancelNotification(String),
    /// Ask the user for permission to show notifications.
    RequestNotificationPermission,
    /// Get a push token (answer with `Runtime::push_registered`).
    RegisterPush,
    /// Forget the push token (stop receiving pushes).
    UnregisterPush,
    /// Report the Live Activity push-to-start token (iOS 17.2+) with
    /// `Runtime::activity_push_token`.
    WatchActivityPushToStart,
    /// The car's template stack changed (JSON, see [`crate::car`]).
    SetCar(String),
    /// Run this task periodically (see [`crate::background`]); re-sent after
    /// each run.
    ScheduleBackgroundTask(crate::background::BackgroundTask),
    /// A device call (answer with `Runtime::device_reply`) or a stream
    /// (events with `Runtime::device_event`, keyed by `id`).
    Device {
        /// Matches the reply or the stream's events.
        id: u64,
        /// What to do.
        call: crate::device::DeviceCall,
    },
}

/// Implemented by each backend. Unsupported requests should fall back
/// gracefully (log, in-app banner, …), never panic.
pub trait PlatformServices {
    /// What this platform supports; reported to the app by [`App::capabilities`].
    fn capabilities(&self) -> Capabilities;
    /// Carries out one request from the app.
    fn handle(&mut self, request: PlatformRequest);
}

// ---------------------------------------------------------------------------
// Static snapshots
// ---------------------------------------------------------------------------

/// A static, serializable node: an element tree without closures, rendered
/// by platforms that cannot run the app (WidgetKit, ActivityKit, Android…).
#[derive(Clone, Debug, PartialEq)]
pub struct SNode {
    /// View, text, image, progress bar or timer.
    pub kind: NodeKind,
    /// The element's resolved style.
    pub style: crate::style::Style,
    /// Text content (text and button nodes).
    pub text: String,
    /// SF Symbol name (image nodes).
    pub icon: String,
    /// Progress fraction, or a timer's end as a Unix timestamp.
    pub value: Option<f64>,
    /// Accessibility label.
    pub label: Option<String>,
    /// Hidden from assistive technologies.
    pub hidden: bool,
    /// Announced as a heading.
    pub heading: bool,
    /// Deep link opened when the node is tapped.
    pub link: Option<String>,
    /// Child nodes, in order.
    pub children: Vec<SNode>,
}

/// Resolves an element (entities, lazy components) into static nodes.
/// Components see [`App::in_snapshot`] while they render, so they can
/// draw a non-interactive variant.
pub fn snapshot(app: &mut App, element: impl IntoElement) -> Vec<SNode> {
    let mut out = Vec::new();
    app.surfaces.snapshot_depth += 1;
    resolve(app, element.into_element(), &mut out);
    app.surfaces.snapshot_depth -= 1;
    out
}

/// Like [`snapshot`], wrapping several roots in a column.
pub fn snapshot_one(app: &mut App, element: impl IntoElement) -> SNode {
    let mut nodes = snapshot(app, element);
    if nodes.len() == 1 {
        return nodes.pop().unwrap();
    }
    SNode::from_vnode(VNode::new(NodeKind::View), nodes)
}

fn resolve(app: &mut App, element: Element, out: &mut Vec<SNode>) {
    match element {
        Element::Empty => {}
        Element::Fragment(items) => items.into_iter().for_each(|e| resolve(app, e, out)),
        Element::Lazy(build) => {
            let e = build(app);
            resolve(app, e, out)
        }
        Element::Entity(entity) => {
            let e = entity.render(app);
            resolve(app, e, out)
        }
        Element::Node(mut vnode) => {
            let mut children = Vec::new();
            for c in std::mem::take(&mut vnode.children) {
                resolve(app, c, &mut children);
            }
            out.push(SNode::from_vnode(*vnode, children));
        }
    }
}

impl SNode {
    fn from_vnode(mut v: VNode, children: Vec<SNode>) -> SNode {
        // Renderers take explicit sizes: a missing side follows the ratio.
        if let Some(r) = v.style.aspect_ratio {
            match (v.style.width, v.style.height) {
                (Length::Px(w), Length::Auto) => v.style.height = Length::Px(w / r),
                (Length::Auto, Length::Px(h)) => v.style.width = Length::Px(h * r),
                _ => {}
            }
        }
        SNode {
            kind: v.kind,
            heading: matches!(v.a11y.role, Some(Role::Heading(_))),
            label: v.a11y.label,
            hidden: v.a11y.hidden,
            style: v.style,
            text: v.text,
            icon: v.icon,
            value: v.value,
            link: v.link,
            children,
        }
    }

    /// Compact JSON (Live Activity payloads are limited to 4 KB).
    ///
    /// Keys: `k` kind (`v` view, `t` text, `i` image, `p` progress, `m` timer,
    /// `sh` shape), `d` 1 = row, `g` gap, `pd` padding and `m` margin
    /// `[t,r,b,l]`, `w`/`h` size (number or `"f"` = fill), `f` flex grow,
    /// `ai`/`jc` alignment, `bg`/`fg`/`bc` colors (`"#RRGGBBAA"` or
    /// `["#light","#dark"]`), `r` radius, `bw` border, `fs`/`fw`/`ta` font (`fw` 1-3
    /// medium/semibold/bold, else a CSS weight; typography keys
    /// `ff`/`it`/`ul`/`st`/`tn`/`tt`/`ls`/`lh`/`ml`/`nw`, see `write_typography`),
    /// `o` opacity, `s` text, symbol or drawing, `v` value, `l` link,
    /// `al` accessibility label, `ah` hidden, `hd` heading, `c` children.
    ///
    /// A shape's drawing is [`Drawing::encode`](crate::shape::Drawing) with
    /// short numbers ([`compact_drawing`]): paths in fractions of the node's
    /// frame, resolved by the renderer at the size it lays the node out.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out, 3);
        out
    }

    /// Rounds every drawing to `decimals` (fewer bytes, coarser curves).
    fn round_drawings(&mut self, decimals: usize) {
        if self.kind == NodeKind::Shape {
            self.text = compact_drawing(&self.text, decimals);
        }
        self.children
            .iter_mut()
            .for_each(|c| c.round_drawings(decimals));
    }

    /// Removes every drawing's paths (their frames stay): the last resort
    /// when a payload is over its limit.
    fn strip_drawings(&mut self) {
        if self.kind == NodeKind::Shape {
            self.text = self.text.lines().next().unwrap_or("i0,0").to_string() + "\n";
        }
        self.children.iter_mut().for_each(SNode::strip_drawings);
    }

    fn write_json(&self, o: &mut String, decimals: usize) {
        let s = &self.style;
        let kind = match self.kind {
            NodeKind::Text | NodeKind::Button => "t",
            NodeKind::Image => "i",
            NodeKind::Progress => "p",
            NodeKind::Timer => "m",
            NodeKind::Shape => "sh",
            _ => "v",
        };
        let _ = write!(o, r#"{{"k":"{kind}""#);
        if s.direction == FlexDirection::Row {
            o.push_str(r#","d":1"#);
        }
        if s.gap > 0. {
            let _ = write!(o, r#","g":{}"#, num(s.gap));
        }
        let p = s.padding;
        if (p.top, p.right, p.bottom, p.left) != (0., 0., 0., 0.) {
            let _ = write!(
                o,
                r#","pd":[{},{},{},{}]"#,
                num(p.top),
                num(p.right),
                num(p.bottom),
                num(p.left)
            );
        }
        let m = s.margin;
        if (m.top, m.right, m.bottom, m.left) != (0., 0., 0., 0.) {
            let _ = write!(
                o,
                r#","m":[{},{},{},{}]"#,
                num(m.top),
                num(m.right),
                num(m.bottom),
                num(m.left)
            );
        }
        for (key, len) in [("w", s.width), ("h", s.height)] {
            match len {
                Length::Px(v) => {
                    let _ = write!(o, r#","{key}":{}"#, num(v));
                }
                Length::Fraction(f) if f >= 1. => {
                    let _ = write!(o, r#","{key}":"f""#);
                }
                _ => {}
            }
        }
        if s.flex_grow > 0. {
            let _ = write!(o, r#","f":{}"#, num(s.flex_grow));
        }
        let align = |a: Align| match a {
            Align::Start => Some("s"),
            Align::Center => Some("c"),
            Align::End => Some("e"),
            Align::Stretch => None,
            Align::Baseline => Some("b"),
        };
        if let Some(a) = align(s.align_items) {
            let _ = write!(o, r#","ai":"{a}""#);
        }
        let jc = match s.justify_content {
            Justify::Start => None,
            Justify::Center => Some("c"),
            Justify::End => Some("e"),
            Justify::SpaceBetween => Some("b"),
            Justify::SpaceAround => Some("a"),
            Justify::SpaceEvenly => Some("v"),
        };
        if let Some(j) = jc {
            let _ = write!(o, r#","jc":"{j}""#);
        }
        for (key, c) in [
            ("bg", s.background),
            ("fg", s.text_color),
            ("bc", s.border_color),
        ] {
            if let Some(c) = c {
                let _ = write!(o, r#","{key}":{}"#, color_json(c));
            }
        }
        if s.corner_radius > 0. {
            let _ = write!(o, r#","r":{}"#, num(s.corner_radius));
            // Some corners only (logical bits, see `Corners::bits`).
            if s.rounded_corners != crate::Corners::ALL {
                let _ = write!(o, r#","rc":{}"#, s.rounded_corners.bits());
            }
        }
        if s.border_width > 0. {
            let _ = write!(o, r#","bw":{}"#, num(s.border_width));
        }
        if s.font_size != crate::style::DEFAULT_FONT_SIZE {
            let _ = write!(o, r#","fs":{}"#, num(s.font_size));
        }
        // 1-3: medium, semibold, bold (the first renderers' scale); other
        // weights as their CSS number (100-900).
        let fw = match s.font_weight {
            FontWeight::Regular => 0,
            FontWeight::Medium => 1,
            FontWeight::Semibold => 2,
            FontWeight::Bold => 3,
            w => w.css(),
        };
        if fw > 0 {
            let _ = write!(o, r#","fw":{fw}"#);
        }
        write_typography(o, s);
        match s.text_align {
            TextAlign::Center => o.push_str(r#","ta":"c""#),
            TextAlign::Right => o.push_str(r#","ta":"r""#),
            TextAlign::Left => {}
        }
        if s.opacity < 1. {
            let _ = write!(o, r#","o":{}"#, num(s.opacity));
        }
        let content = if self.kind == NodeKind::Image {
            &self.icon
        } else {
            &self.text
        };
        if self.kind == NodeKind::Shape {
            let drawing = compact_drawing(content, decimals);
            let _ = write!(o, r#","s":{}"#, json_str(&drawing));
        } else if !content.is_empty() {
            let _ = write!(o, r#","s":{}"#, json_str(content));
        }
        if let Some(v) = self.value {
            let _ = write!(o, r#","v":{}"#, num(v as f32).max_precision(v));
        }
        if let Some(l) = &self.link {
            let _ = write!(o, r#","l":{}"#, json_str(l));
        }
        if let Some(l) = &self.label {
            let _ = write!(o, r#","al":{}"#, json_str(l));
        }
        if self.hidden {
            o.push_str(r#","ah":1"#);
        }
        if self.heading {
            o.push_str(r#","hd":1"#);
        }
        if !self.children.is_empty() {
            o.push_str(r#","c":["#);
            for (i, c) in self.children.iter().enumerate() {
                if i > 0 {
                    o.push(',');
                }
                c.write_json(o, decimals);
            }
            o.push(']');
        }
        o.push('}');
    }
}

/// Typography keys of a node (only what differs from the defaults): `ff`
/// family (`mono`, `serif`, `rounded` or a name), `it` italic, `ul`
/// underline, `st` strikethrough, `tn` tabular digits, `tt` case (`u`, `l`,
/// `c`), `ls` letter spacing, `lh` line height in points, `ml` maximum
/// lines, `nw` no wrapping.
fn write_typography(o: &mut String, s: &crate::style::Style) {
    use crate::style::{FontFamily, TextTransform};
    if s.font_family != FontFamily::System {
        let _ = write!(o, r#","ff":{}"#, json_str(s.font_family.encode()));
    }
    for (key, on) in [
        ("it", s.italic),
        ("ul", s.underline),
        ("st", s.strikethrough),
        ("tn", s.tabular_numbers),
        ("nw", s.no_wrap),
    ] {
        if on {
            let _ = write!(o, r#","{key}":1"#);
        }
    }
    let tt = match s.text_transform {
        TextTransform::None => None,
        TextTransform::Uppercase => Some("u"),
        TextTransform::Lowercase => Some("l"),
        TextTransform::Capitalize => Some("c"),
    };
    if let Some(t) = tt {
        let _ = write!(o, r#","tt":"{t}""#);
    }
    if s.letter_spacing != 0. {
        let _ = write!(o, r#","ls":{}"#, num(s.letter_spacing));
    }
    if let Some(lh) = s.line_height_points() {
        let _ = write!(o, r#","lh":{}"#, num(lh));
    }
    if let Some(n) = s.max_lines {
        let _ = write!(o, r#","ml":{n}"#);
    }
}

/// A drawing's text form ([`Drawing::encode`](crate::shape::Drawing)) in
/// fewer bytes: numbers rounded to `decimals` without leading zeros
/// (`0.25` → `.25`), opaque colors without their alpha (`#5B5BD6FF` →
/// `#5B5BD6`). Every reader accepts both forms.
pub fn compact_drawing(text: &str, decimals: usize) -> String {
    let scale = 10f32.powi(decimals as i32);
    let short = |t: &str| -> String {
        let Ok(v) = t.parse::<f32>() else {
            return t.to_string();
        };
        let v = (v * scale).round() / scale;
        let s = format!("{v:.decimals$}");
        let s = if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.')
        } else {
            &s
        };
        match s {
            "-0" => "0".into(),
            s if s.starts_with("0.") => s[1..].into(),
            s if s.starts_with("-0.") => format!("-{}", &s[2..]),
            s => s.into(),
        }
    };
    let color = |c: &str| -> String {
        c.split('/')
            .map(|h| match h.strip_suffix("FF") {
                Some(rgb) if h.len() == 9 => rgb,
                _ => h,
            })
            .collect::<Vec<_>>()
            .join("/")
    };
    let mut o = String::with_capacity(text.len());
    for line in text.lines() {
        if line.starts_with('i') {
            o.push_str(line);
            o.push('\n');
            continue;
        }
        let Some((paint, segs)) = line.split_once('|') else {
            continue;
        };
        let paint: Vec<String> = paint
            .split_whitespace()
            .map(|t| match t.split_at(1) {
                (tag @ ("f" | "s"), c) => format!("{tag}{}", color(c)),
                _ => t.to_string(),
            })
            .collect();
        o.push_str(&paint.join(" "));
        o.push('|');
        let segs: Vec<String> = segs.split_whitespace().map(short).collect();
        o.push_str(&segs.join(" "));
        o.push('\n');
    }
    o
}

/// Numbers without trailing `.0` noise.
struct Num(String);

impl Num {
    /// Values such as Unix timestamps need full precision.
    fn max_precision(self, v: f64) -> String {
        if v.abs() > 1e6 {
            format!("{}", v.round() as i64)
        } else {
            self.0
        }
    }
}

impl std::fmt::Display for Num {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn num(v: f32) -> Num {
    if v.fract() == 0. && v.abs() < 1e7 {
        Num(format!("{}", v as i64))
    } else {
        Num(format!("{:.3}", v).trim_end_matches('0').to_string())
    }
}

fn color_json(c: Color) -> String {
    match c.dark {
        Some(_) => format!(
            r#"["{}","{}"]"#,
            c.light_variant().hex(),
            c.dark_variant().hex()
        ),
        None => format!(r#""{}""#, c.hex()),
    }
}

/// A JSON string literal.
pub fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

// ---------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------

/// Widget sizes (WidgetKit families; Android maps them to Glance sizes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WidgetFamily {
    /// Small square (`systemSmall`).
    Small,
    /// Wide rectangle (`systemMedium`).
    Medium,
    /// Large square (`systemLarge`).
    Large,
    /// Extra large, iPad and Mac only (`systemExtraLarge`).
    ExtraLarge,
    /// Lock screen / watch complications.
    AccessoryCircular,
    /// Lock screen / watch: a small rectangle (`accessoryRectangular`).
    AccessoryRectangular,
    /// Lock screen / watch: one line of text (`accessoryInline`).
    AccessoryInline,
}

impl WidgetFamily {
    /// The WidgetKit family name (`"systemSmall"`).
    pub fn name(self) -> &'static str {
        match self {
            WidgetFamily::Small => "systemSmall",
            WidgetFamily::Medium => "systemMedium",
            WidgetFamily::Large => "systemLarge",
            WidgetFamily::ExtraLarge => "systemExtraLarge",
            WidgetFamily::AccessoryCircular => "accessoryCircular",
            WidgetFamily::AccessoryRectangular => "accessoryRectangular",
            WidgetFamily::AccessoryInline => "accessoryInline",
        }
    }
}

type WidgetRender = Rc<dyn Fn(&mut App, WidgetFamily) -> Element>;

/// A home-screen / desktop widget, rendered from app state.
#[derive(Clone)]
pub struct Widget {
    /// Stable identifier (`"todos"`).
    pub kind: String,
    /// Shown in the widget gallery.
    pub name: String,
    /// Shown under the name in the widget gallery.
    pub description: String,
    /// The sizes offered (default: small and medium).
    pub families: Vec<WidgetFamily>,
    /// Deep link opened when the widget background is tapped.
    pub link: Option<String>,
    render: WidgetRender,
}

impl Widget {
    /// `kind` is a stable identifier (`"todos"`), `name` is shown in the gallery.
    pub fn new(kind: impl Into<String>, name: impl Into<String>) -> Self {
        Widget {
            kind: kind.into(),
            name: name.into(),
            description: String::new(),
            families: vec![WidgetFamily::Small, WidgetFamily::Medium],
            link: None,
            render: Rc::new(|_, _| Element::Empty),
        }
    }

    /// Shown under the name in the widget gallery.
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    /// The sizes offered (default: small and medium).
    pub fn families(mut self, families: &[WidgetFamily]) -> Self {
        self.families = families.to_vec();
        self
    }

    /// Deep link opened when the widget background is tapped.
    pub fn link(mut self, path: impl Into<String>) -> Self {
        self.link = Some(path.into());
        self
    }

    /// The widget's content for a family. Read any entity through `cx`.
    pub fn render<E: IntoElement>(
        mut self,
        f: impl Fn(&mut App, WidgetFamily) -> E + 'static,
    ) -> Self {
        self.render = Rc::new(move |app, family| f(app, family).into_element());
        self
    }

    pub(crate) fn snapshot(&self, app: &mut App) -> WidgetSnapshot {
        let families = self
            .families
            .iter()
            .map(|f| {
                let element = (self.render)(app, *f);
                (*f, snapshot_one(app, element).to_json())
            })
            .collect();
        WidgetSnapshot {
            kind: self.kind.clone(),
            link: self.link.clone(),
            families,
        }
    }

    /// Static metadata (used to generate the platform widget extension).
    pub fn describe_json(&self) -> String {
        let families: Vec<String> = self.families.iter().map(|f| json_str(f.name())).collect();
        format!(
            r#"{{"kind":{},"name":{},"description":{},"families":[{}]}}"#,
            json_str(&self.kind),
            json_str(&self.name),
            json_str(&self.description),
            families.join(",")
        )
    }
}

/// A widget's content for each family, as JSON snapshots.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetSnapshot {
    /// The widget's [`Widget::kind`].
    pub kind: String,
    /// Deep link opened when the widget background is tapped.
    pub link: Option<String>,
    /// Each family with its [`SNode`] JSON.
    pub families: Vec<(WidgetFamily, String)>,
}

impl WidgetSnapshot {
    /// JSON for the widget extension: kind, link, update time and families.
    pub fn to_json(&self) -> String {
        let fams: Vec<String> = self
            .families
            .iter()
            .map(|(f, json)| format!("{}:{json}", json_str(f.name())))
            .collect();
        format!(
            r#"{{"kind":{},"link":{},"updated":{},"families":{{{}}}}}"#,
            json_str(&self.kind),
            self.link.as_deref().map_or("null".into(), json_str),
            unix_now(),
            fams.join(",")
        )
    }
}

// ---------------------------------------------------------------------------
// Live Activities (lock screen + Dynamic Island)
// ---------------------------------------------------------------------------

/// Identifies a running Live Activity ([`App::start_activity`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActivityId(pub u64);

/// The rendered regions of a Live Activity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActivityContent {
    /// Lock screen and notification banner presentation.
    pub lock_screen: Option<SNode>,
    /// Dynamic Island, compact: left of the camera.
    pub compact_leading: Option<SNode>,
    /// Dynamic Island, compact: right of the camera.
    pub compact_trailing: Option<SNode>,
    /// Dynamic Island, minimal: when several activities share the island.
    pub minimal: Option<SNode>,
    /// Dynamic Island, expanded: leading region.
    pub expanded_leading: Option<SNode>,
    /// Dynamic Island, expanded: trailing region.
    pub expanded_trailing: Option<SNode>,
    /// Dynamic Island, expanded: center region.
    pub expanded_center: Option<SNode>,
    /// Dynamic Island, expanded: bottom region.
    pub expanded_bottom: Option<SNode>,
    /// Deep link opened when the activity is tapped.
    pub link: Option<String>,
    /// Background tint of the lock-screen presentation.
    pub tint: Option<Color>,
    /// The content is considered outdated after this moment.
    pub stale_after: Option<SystemTime>,
    /// A one-line summary: accessibility, and fallbacks (menu bar, web banner).
    pub summary: String,
    /// Started with [`Activity::push_updates`]: the platform reports a push
    /// token for it (`Runtime::activity_push_token`).
    pub push_updates: bool,
}

impl ActivityContent {
    /// Compact JSON for ActivityKit (under its 4 KB limit) and the fallbacks.
    pub fn to_json(&self) -> String {
        let mut parts = Vec::new();
        for (key, node) in [
            ("ls", &self.lock_screen),
            ("cl", &self.compact_leading),
            ("ct", &self.compact_trailing),
            ("mn", &self.minimal),
            ("el", &self.expanded_leading),
            ("et", &self.expanded_trailing),
            ("ec", &self.expanded_center),
            ("eb", &self.expanded_bottom),
        ] {
            if let Some(n) = node {
                parts.push(format!(r#""{key}":{}"#, n.to_json()));
            }
        }
        if let Some(l) = &self.link {
            parts.push(format!(r#""l":{}"#, json_str(l)));
        }
        if let Some(t) = self.tint {
            parts.push(format!(r#""tn":{}"#, color_json(t)));
        }
        parts.push(format!(r#""sm":{}"#, json_str(&self.summary)));
        format!("{{{}}}", parts.join(","))
    }

    /// The bytes ActivityKit counts for this content with an activity of
    /// `kind`: the content state (`{"json":"…"}`, the JSON escaped in a
    /// string) plus the static attributes (`{"kind":"…"}`).
    pub fn payload_size(&self, kind: &str) -> usize {
        json_str(&self.to_json()).len() + json_str(kind).len() + r#"{"json":}{"kind":}"#.len()
    }

    fn nodes_mut(&mut self) -> impl Iterator<Item = &mut SNode> {
        [
            &mut self.lock_screen,
            &mut self.compact_leading,
            &mut self.compact_trailing,
            &mut self.minimal,
            &mut self.expanded_leading,
            &mut self.expanded_trailing,
            &mut self.expanded_center,
            &mut self.expanded_bottom,
        ]
        .into_iter()
        .flatten()
    }

    /// Keeps the payload under ActivityKit's limit: drawings are rounded to
    /// two decimals first, then left out (their frames stay empty).
    fn fit(&mut self, kind: &str) {
        let before = self.payload_size(kind);
        if before <= ACTIVITY_PAYLOAD_LIMIT {
            return;
        }
        self.nodes_mut().for_each(|n| n.round_drawings(2));
        let mut action = "rounded its drawings";
        if self.payload_size(kind) > ACTIVITY_PAYLOAD_LIMIT {
            self.nodes_mut().for_each(SNode::strip_drawings);
            action = "left its drawings out";
        }
        eprintln!(
            "prism: Live Activity \"{kind}\" is {before} bytes (ActivityKit allows {ACTIVITY_PAYLOAD_LIMIT}): {action}, now {} bytes",
            self.payload_size(kind)
        );
    }
}

/// ActivityKit's limit for a Live Activity's static and dynamic data
/// combined, in bytes (ActivityKit documentation, "Displaying live data with
/// Live Activities").
pub const ACTIVITY_PAYLOAD_LIMIT: usize = 4096;

/// Builder for a Live Activity: what shows on the lock screen and in each
/// region of the Dynamic Island.
pub struct Activity {
    kind: String,
    lock_screen: Option<Element>,
    compact_leading: Option<Element>,
    compact_trailing: Option<Element>,
    minimal: Option<Element>,
    expanded_leading: Option<Element>,
    expanded_trailing: Option<Element>,
    expanded_center: Option<Element>,
    expanded_bottom: Option<Element>,
    link: Option<String>,
    tint: Option<Color>,
    stale_after: Option<SystemTime>,
    summary: String,
    push_updates: bool,
}

impl Activity {
    /// An empty activity; `kind` is a stable identifier (`"focus"`).
    pub fn new(kind: impl Into<String>) -> Self {
        Activity {
            kind: kind.into(),
            lock_screen: None,
            compact_leading: None,
            compact_trailing: None,
            minimal: None,
            expanded_leading: None,
            expanded_trailing: None,
            expanded_center: None,
            expanded_bottom: None,
            link: None,
            tint: None,
            stale_after: None,
            summary: String::new(),
            push_updates: false,
        }
    }

    /// Lets your server update this activity with pushes (ActivityKit
    /// `pushType: .token`): its token arrives in
    /// [`App::on_activity_push_token`]. The app needs push enabled
    /// (`[push]` in Prism.toml).
    pub fn push_updates(mut self) -> Self {
        self.push_updates = true;
        self
    }

    /// The content-state JSON that ActivityKit pushes carry
    /// (`{"json":"…"}`): render it on your server and send it with
    /// `prism_push::Push::live_activity`.
    pub fn content_state_json(self) -> String {
        let mut app = App::new();
        let (_, content) = self.render(&mut app);
        format!(r#"{{"json":{}}}"#, json_str(&content.to_json()))
    }

    /// Lock screen and notification banner presentation.
    pub fn lock_screen(mut self, e: impl IntoElement) -> Self {
        self.lock_screen = Some(e.into_element());
        self
    }

    /// Dynamic Island, compact: either side of the camera.
    pub fn compact(mut self, leading: impl IntoElement, trailing: impl IntoElement) -> Self {
        self.compact_leading = Some(leading.into_element());
        self.compact_trailing = Some(trailing.into_element());
        self
    }

    /// Dynamic Island, minimal: when several activities share the island.
    pub fn minimal(mut self, e: impl IntoElement) -> Self {
        self.minimal = Some(e.into_element());
        self
    }

    /// Dynamic Island, expanded (long press): four regions.
    pub fn expanded(
        mut self,
        leading: impl IntoElement,
        trailing: impl IntoElement,
        center: impl IntoElement,
        bottom: impl IntoElement,
    ) -> Self {
        self.expanded_leading = Some(leading.into_element());
        self.expanded_trailing = Some(trailing.into_element());
        self.expanded_center = Some(center.into_element());
        self.expanded_bottom = Some(bottom.into_element());
        self
    }

    /// Deep link opened when the activity is tapped.
    pub fn link(mut self, path: impl Into<String>) -> Self {
        self.link = Some(path.into());
        self
    }

    /// Background tint of the lock-screen presentation.
    pub fn tint(mut self, c: Color) -> Self {
        self.tint = Some(c);
        self
    }

    /// Marks the content as outdated after `at`.
    pub fn stale_after(mut self, at: SystemTime) -> Self {
        self.stale_after = Some(at);
        self
    }

    /// One-line text summary (accessibility and fallbacks). Required.
    pub fn summary(mut self, s: impl Into<String>) -> Self {
        self.summary = s.into();
        self
    }

    fn render(self, app: &mut App) -> (String, ActivityContent) {
        let mut snap = |e: Option<Element>| e.map(|e| snapshot_one(app, e));
        let mut content = ActivityContent {
            lock_screen: snap(self.lock_screen),
            compact_leading: snap(self.compact_leading),
            compact_trailing: snap(self.compact_trailing),
            minimal: snap(self.minimal),
            expanded_leading: snap(self.expanded_leading),
            expanded_trailing: snap(self.expanded_trailing),
            expanded_center: snap(self.expanded_center),
            expanded_bottom: snap(self.expanded_bottom),
            link: self.link,
            tint: self.tint,
            stale_after: self.stale_after,
            summary: self.summary,
            push_updates: self.push_updates,
        };
        content.fit(&self.kind);
        (self.kind, content)
    }
}

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

/// A local notification, optionally opening a deep link when tapped.
#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    /// Re-using an id replaces the pending/delivered notification.
    pub id: String,
    /// The bold first line.
    pub title: String,
    /// A second line under the title.
    pub subtitle: Option<String>,
    /// The message.
    pub body: String,
    /// Deep link opened when the notification is tapped.
    pub link: Option<String>,
    /// Delivered after this delay; `None` delivers immediately.
    pub delay: Option<Duration>,
    /// Plays the default sound (default: true).
    pub sound: bool,
    /// Sets the app icon's badge number.
    pub badge: Option<u32>,
    /// Groups related notifications.
    pub thread: Option<String>,
    /// Buttons shown with the notification (long press / expand).
    pub actions: Vec<NotificationAction>,
}

/// A button on a notification. Choosing it calls the app's
/// [`App::on_notification_response`] handler (and opens `link`, if any).
#[derive(Clone, Debug, PartialEq)]
pub struct NotificationAction {
    /// Reported as [`NotificationResponse::action`].
    pub id: String,
    /// The button's label.
    pub title: String,
    /// Shown in red (deletes or cancels something).
    pub destructive: bool,
    /// Brings the app to the foreground (otherwise handled in the background).
    pub foreground: bool,
    /// Shows a text field (quick reply); the text arrives in the response.
    pub reply: Option<String>,
    /// Deep link opened in the app when the action is chosen.
    pub link: Option<String>,
}

impl NotificationAction {
    /// A button with an id (reported in the response) and a label.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        NotificationAction {
            id: id.into(),
            title: title.into(),
            destructive: false,
            foreground: false,
            reply: None,
            link: None,
        }
    }
    /// Shown in red (deletes or cancels something).
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }
    /// Brings the app to the foreground when chosen.
    pub fn opens_app(mut self) -> Self {
        self.foreground = true;
        self
    }
    /// A quick-reply text field with this placeholder.
    pub fn reply(mut self, placeholder: impl Into<String>) -> Self {
        self.reply = Some(placeholder.into());
        self
    }
    /// Opens `path` in the app (implies [`opens_app`](Self::opens_app)).
    pub fn link(mut self, path: impl Into<String>) -> Self {
        self.link = Some(path.into());
        self.foreground = true;
        self
    }
}

/// What the user did with a notification.
#[derive(Clone, Debug, PartialEq)]
pub struct NotificationResponse {
    /// The notification's id.
    pub id: String,
    /// The chosen action, or `None` for a tap on the notification itself.
    pub action: Option<String>,
    /// Text typed in a reply action.
    pub text: Option<String>,
}

impl Notification {
    /// JSON for platform bridges (Swift, browser).
    pub fn to_json(&self) -> String {
        fn s(v: &str) -> String {
            json_str(v)
        }
        let opt = |v: &Option<String>| v.as_deref().map_or("null".into(), s);
        let actions: Vec<String> = self
            .actions
            .iter()
            .map(|a| {
                format!(
                    r#"{{"id":{},"title":{},"destructive":{},"foreground":{},"reply":{},"link":{}}}"#,
                    s(&a.id),
                    s(&a.title),
                    a.destructive,
                    a.foreground,
                    opt(&a.reply),
                    opt(&a.link),
                )
            })
            .collect();
        format!(
            r#"{{"id":{},"title":{},"subtitle":{},"body":{},"link":{},"delay":{},"sound":{},"badge":{},"thread":{},"actions":[{}]}}"#,
            s(&self.id),
            s(&self.title),
            opt(&self.subtitle),
            s(&self.body),
            opt(&self.link),
            self.delay.map_or(0., |d| d.as_secs_f64()),
            self.sound,
            self.badge.map_or("null".into(), |b| b.to_string()),
            opt(&self.thread),
            actions.join(","),
        )
    }
}

impl Notification {
    /// A notification with a title, delivered immediately with a sound.
    pub fn new(title: impl Into<String>) -> Self {
        Notification {
            id: format!("prism-{}", unix_now()),
            title: title.into(),
            subtitle: None,
            body: String::new(),
            link: None,
            delay: None,
            sound: true,
            badge: None,
            thread: None,
            actions: Vec::new(),
        }
    }

    /// Adds a button (iOS and macOS show up to 4 in the expanded notification).
    pub fn action(mut self, action: NotificationAction) -> Self {
        self.actions.push(action);
        self
    }

    /// A stable id: re-using it replaces the pending or delivered notification.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }
    /// A second line under the title.
    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = Some(s.into());
        self
    }
    /// The message.
    pub fn body(mut self, b: impl Into<String>) -> Self {
        self.body = b.into();
        self
    }
    /// Deep link opened when the notification is tapped.
    pub fn link(mut self, path: impl Into<String>) -> Self {
        self.link = Some(path.into());
        self
    }
    /// Delivers after `delay` instead of immediately.
    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }
    /// Delivers without a sound.
    pub fn silent(mut self) -> Self {
        self.sound = false;
        self
    }
    /// Sets the app icon's badge number.
    pub fn badge(mut self, n: u32) -> Self {
        self.badge = Some(n);
        self
    }
    /// Groups related notifications under this thread id.
    pub fn thread(mut self, t: impl Into<String>) -> Self {
        self.thread = Some(t.into());
        self
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ---------------------------------------------------------------------------
// App API
// ---------------------------------------------------------------------------

/// Surface state kept by the [`App`].
#[derive(Default)]
pub(crate) struct Surfaces {
    pub capabilities: Capabilities,
    pub widgets: Vec<Widget>,
    pub widgets_dirty: bool,
    pub requests: Vec<PlatformRequest>,
    pub next_activity: u64,
    pub notification_handler: Option<NotificationHandler>,
    /// Inside [`snapshot`] (nested for snapshots taken while rendering one).
    pub snapshot_depth: u32,
    // --- Bundling: what the app declares (`PRISM_DESCRIBE`) ---
    /// [`App::enable_live_activities`] was called.
    pub live_activities: bool,
}

type NotificationHandler = std::rc::Rc<dyn Fn(&NotificationResponse, &mut App)>;

impl App {
    /// What the current platform supports (set by the backend).
    pub fn capabilities(&self) -> Capabilities {
        self.surfaces.capabilities
    }

    /// Called by backends at startup with what the platform supports.
    pub fn set_capabilities(&mut self, caps: Capabilities) {
        self.surfaces.capabilities = caps;
    }

    /// True while a widget, Live Activity or other static surface is being
    /// rendered: nothing there is tappable except links, nothing animates,
    /// and sizes relative to the parent are not kept. Components use it to
    /// draw a static variant (charts draw their bars as one shape).
    pub fn in_snapshot(&self) -> bool {
        self.surfaces.snapshot_depth > 0
    }

    /// Registers a widget and schedules its first render.
    pub fn register_widget(&mut self, widget: Widget) {
        self.surfaces.widgets.retain(|w| w.kind != widget.kind);
        self.surfaces.widgets.push(widget);
        self.surfaces.widgets_dirty = true;
    }

    /// The registered widgets.
    pub fn widgets(&self) -> &[Widget] {
        &self.surfaces.widgets
    }

    /// Re-renders every widget from current state (coalesced per event).
    pub fn reload_widgets(&mut self) {
        self.surfaces.widgets_dirty = true;
    }

    /// Starts a Live Activity. Falls back to a menu bar item (macOS) or an
    /// in-page live banner (web) where Live Activities do not exist.
    pub fn start_activity(&mut self, activity: Activity) -> ActivityId {
        self.surfaces.next_activity += 1;
        let id = ActivityId(self.surfaces.next_activity);
        let (kind, content) = activity.render(self);
        self.surfaces
            .requests
            .push(PlatformRequest::StartActivity { id, kind, content });
        id
    }

    /// For backends: a Live Activity the app did not start in this run
    /// (started by a push-to-start push, or before a relaunch) joins the
    /// app's activities, so `update_activity` and `end_activity` reach it.
    /// It keeps `requested` (the id its push or earlier run gave it) when
    /// there is one, or gets a new id. Ids `start_activity` hands out later
    /// are above it.
    pub fn adopt_activity(&mut self, requested: Option<ActivityId>) -> ActivityId {
        match requested {
            Some(id) if id.0 > 0 => {
                self.surfaces.next_activity = self.surfaces.next_activity.max(id.0);
                id
            }
            _ => {
                self.surfaces.next_activity += 1;
                ActivityId(self.surfaces.next_activity)
            }
        }
    }

    /// Replaces a running activity's content (its kind is kept).
    pub fn update_activity(&mut self, id: ActivityId, activity: Activity) {
        let (_, content) = activity.render(self);
        self.surfaces
            .requests
            .push(PlatformRequest::UpdateActivity { id, content });
    }

    /// Ends an activity, optionally showing a final state on the lock screen.
    pub fn end_activity(&mut self, id: ActivityId, last: Option<Activity>, immediately: bool) {
        let content = last.map(|a| a.render(self).1);
        self.surfaces.requests.push(PlatformRequest::EndActivity {
            id,
            content,
            immediately,
        });
    }

    /// Handles taps and action buttons on the app's notifications. Register it
    /// at startup: the app may have been launched by the notification.
    pub fn on_notification_response(
        &mut self,
        f: impl Fn(&NotificationResponse, &mut App) + 'static,
    ) {
        self.surfaces.notification_handler = Some(std::rc::Rc::new(f));
    }

    /// Called by backends: opens the action's (or notification's) link and
    /// runs the response handler.
    pub fn notification_response(&mut self, response: NotificationResponse, link: Option<String>) {
        if let Some(link) = link {
            self.navigate(link);
        }
        self.push_notification_opened(&response);
        if let Some(handler) = self.surfaces.notification_handler.clone() {
            handler(&response, self);
        }
    }

    /// Posts a local notification (asks for permission first if needed).
    pub fn send_notification(&mut self, n: Notification) {
        self.surfaces.requests.push(PlatformRequest::Notify(n));
    }

    /// Withdraws a pending or delivered notification, by id.
    pub fn cancel_notification(&mut self, id: impl Into<String>) {
        self.surfaces
            .requests
            .push(PlatformRequest::CancelNotification(id.into()));
    }

    /// Asks for permission to show notifications now (otherwise asked on the
    /// first notification).
    pub fn request_notification_permission(&mut self) {
        self.surfaces
            .requests
            .push(PlatformRequest::RequestNotificationPermission);
    }

    /// Pending platform work; widgets are re-rendered here, once per event.
    pub(crate) fn take_requests(&mut self) -> Vec<PlatformRequest> {
        if std::mem::take(&mut self.surfaces.widgets_dirty) && !self.surfaces.widgets.is_empty() {
            let widgets = self.surfaces.widgets.clone();
            let snaps = widgets.iter().map(|w| w.snapshot(self)).collect();
            self.surfaces
                .requests
                .push(PlatformRequest::ReloadWidgets(snaps));
        }
        if let Some(json) = self.car_update() {
            self.surfaces.requests.push(PlatformRequest::SetCar(json));
        }
        std::mem::take(&mut self.surfaces.requests)
    }

    /// JSON description of registered widgets (for generating extensions).
    pub fn describe_widgets(&self) -> String {
        let items: Vec<String> = self
            .surfaces
            .widgets
            .iter()
            .map(Widget::describe_json)
            .collect();
        format!("[{}]", items.join(","))
    }
}

impl App {
    /// Declares that the app starts Live Activities ([`App::start_activity`]),
    /// so the bundler embeds the widget extension that draws them on iOS.
    /// Call it at startup, like [`App::register_widget`].
    ///
    /// The `prism` CLI leaves the extension (and its provisioning profile and
    /// App Group) out of apps that register no widget and declare no Live
    /// Activity; there, `start_activity` does nothing on iOS. Apps with a
    /// widget, or listening for push-to-start tokens
    /// ([`App::on_activity_push_token`]), get the extension anyway.
    pub fn enable_live_activities(&mut self) {
        self.surfaces.live_activities = true;
    }

    /// Whether the app declared Live Activities: [`App::enable_live_activities`]
    /// or a handler for their push tokens. `PRISM_DESCRIBE=live_activities`
    /// prints it (`1` or `0`) for bundlers.
    pub fn uses_live_activities(&self) -> bool {
        self.surfaces.live_activities || self.watches_activity_tokens()
    }
}

#[cfg(test)]
mod bundling_tests {
    use crate::App;

    #[test]
    fn live_activities_are_declared_or_implied_by_push_to_start() {
        let mut app = App::new();
        assert!(!app.uses_live_activities());
        app.enable_live_activities();
        assert!(app.uses_live_activities());
        let mut app = App::new();
        app.on_activity_push_token(|_, _| {});
        assert!(app.uses_live_activities());
        // Widgets alone are no declaration (the extension is built for them).
        let mut app = App::new();
        app.register_widget(crate::Widget::new("w", "W"));
        assert!(!app.uses_live_activities());
        assert_ne!(app.describe_widgets(), "[]");
    }

    #[test]
    fn adopted_activities_keep_their_id_and_the_app_counts_above_it() {
        use crate::ActivityId;
        let mut app = App::new();
        assert_eq!(app.adopt_activity(Some(ActivityId(42))), ActivityId(42));
        assert_eq!(app.adopt_activity(Some(ActivityId(7))), ActivityId(7));
        assert_eq!(app.adopt_activity(None), ActivityId(43));
        assert_eq!(app.adopt_activity(Some(ActivityId(0))), ActivityId(44));
        let id = app.start_activity(crate::Activity::new("focus").summary("Focus"));
        assert_eq!(id, ActivityId(45));
    }
}
