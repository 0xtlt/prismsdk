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
    pub widgets: bool,
    pub live_activities: bool,
    /// Live Activities also appear in a Dynamic Island (iPhone 14 Pro and later).
    pub dynamic_island: bool,
    pub notifications: bool,
    /// Tray / menu bar extras (`cx.set_tray`).
    pub tray: bool,
    /// A menu bar for app menus (`cx.set_app_menu`).
    pub app_menu: bool,
    /// Device APIs (see [`crate::device`]).
    pub location: bool,
    pub motion: bool,
    pub camera: bool,
    pub photos: bool,
    pub haptics: bool,
    pub share: bool,
    /// `copy_text` / `read_clipboard` (none on tvOS and watchOS).
    pub clipboard: bool,
    /// `cloud_storage` syncs between devices (otherwise it is local).
    pub cloud_sync: bool,
}

/// Work the app asked the platform to do. Backends drain these with
/// [`Runtime::take_requests`](crate::Runtime::take_requests).
#[derive(Clone, Debug, PartialEq)]
pub enum PlatformRequest {
    /// Re-rendered snapshots of every registered widget.
    ReloadWidgets(Vec<WidgetSnapshot>),
    StartActivity {
        id: ActivityId,
        kind: String,
        content: ActivityContent,
    },
    UpdateActivity {
        id: ActivityId,
        content: ActivityContent,
    },
    EndActivity {
        id: ActivityId,
        content: Option<ActivityContent>,
        immediately: bool,
    },
    Notify(Notification),
    ShowAlert(crate::menu::AlertSpec),
    SetTray(crate::menu::TraySpec),
    RemoveTray(String),
    SetAppMenu(Vec<crate::menu::MenuSpec>),
    CancelNotification(String),
    RequestNotificationPermission,
    /// Run this task periodically (see [`crate::background`]); re-sent after
    /// each run.
    ScheduleBackgroundTask(crate::background::BackgroundTask),
    /// A device call (answer with `Runtime::device_reply`) or a stream
    /// (events with `Runtime::device_event`, keyed by `id`).
    Device {
        id: u64,
        call: crate::device::DeviceCall,
    },
}

/// Implemented by each backend. Unsupported requests should fall back
/// gracefully (log, in-app banner, …), never panic.
pub trait PlatformServices {
    fn capabilities(&self) -> Capabilities;
    fn handle(&mut self, request: PlatformRequest);
}

// ---------------------------------------------------------------------------
// Static snapshots
// ---------------------------------------------------------------------------

/// A static, serializable node: an element tree without closures, rendered
/// by platforms that cannot run the app (WidgetKit, ActivityKit, Android…).
#[derive(Clone, Debug, PartialEq)]
pub struct SNode {
    pub kind: NodeKind,
    pub style: crate::style::Style,
    pub text: String,
    pub icon: String,
    pub value: Option<f64>,
    pub label: Option<String>,
    pub hidden: bool,
    pub heading: bool,
    pub link: Option<String>,
    pub children: Vec<SNode>,
}

/// Resolves an element (entities, lazy components) into static nodes.
pub fn snapshot(app: &mut App, element: impl IntoElement) -> Vec<SNode> {
    let mut out = Vec::new();
    resolve(app, element.into_element(), &mut out);
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
    fn from_vnode(v: VNode, children: Vec<SNode>) -> SNode {
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
    /// Keys: `k` kind (`v` view, `t` text, `i` image, `p` progress, `m` timer),
    /// `d` 1 = row, `g` gap, `pd` padding `[t,r,b,l]`, `w`/`h` size (number or
    /// `"f"` = fill), `f` flex grow, `ai`/`jc` alignment, `bg`/`fg`/`bc` colors
    /// (`"#RRGGBBAA"` or `["#light","#dark"]`), `r` radius, `bw` border,
    /// `fs`/`fw`/`ta` font, `o` opacity, `s` text or symbol, `v` value,
    /// `l` link, `al` accessibility label, `ah` hidden, `hd` heading, `c` children.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    fn write_json(&self, o: &mut String) {
        let s = &self.style;
        let kind = match self.kind {
            NodeKind::Text | NodeKind::Button => "t",
            NodeKind::Image => "i",
            NodeKind::Progress => "p",
            NodeKind::Timer => "m",
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
        }
        if s.border_width > 0. {
            let _ = write!(o, r#","bw":{}"#, num(s.border_width));
        }
        if s.font_size != crate::style::DEFAULT_FONT_SIZE {
            let _ = write!(o, r#","fs":{}"#, num(s.font_size));
        }
        let fw = match s.font_weight {
            FontWeight::Regular => 0,
            FontWeight::Medium => 1,
            FontWeight::Semibold => 2,
            FontWeight::Bold => 3,
        };
        if fw > 0 {
            let _ = write!(o, r#","fw":{fw}"#);
        }
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
        if !content.is_empty() {
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
                c.write_json(o);
            }
            o.push(']');
        }
        o.push('}');
    }
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
    Small,
    Medium,
    Large,
    ExtraLarge,
    /// Lock screen / watch complications.
    AccessoryCircular,
    AccessoryRectangular,
    AccessoryInline,
}

impl WidgetFamily {
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
    pub kind: String,
    pub name: String,
    pub description: String,
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

    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    pub fn families(mut self, families: &[WidgetFamily]) -> Self {
        self.families = families.to_vec();
        self
    }

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
    pub kind: String,
    pub link: Option<String>,
    pub families: Vec<(WidgetFamily, String)>,
}

impl WidgetSnapshot {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActivityId(pub u64);

/// The rendered regions of a Live Activity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActivityContent {
    pub lock_screen: Option<SNode>,
    pub compact_leading: Option<SNode>,
    pub compact_trailing: Option<SNode>,
    pub minimal: Option<SNode>,
    pub expanded_leading: Option<SNode>,
    pub expanded_trailing: Option<SNode>,
    pub expanded_center: Option<SNode>,
    pub expanded_bottom: Option<SNode>,
    /// Deep link opened when the activity is tapped.
    pub link: Option<String>,
    /// Background tint of the lock-screen presentation.
    pub tint: Option<Color>,
    /// The content is considered outdated after this moment.
    pub stale_after: Option<SystemTime>,
    /// A one-line summary: accessibility, and fallbacks (menu bar, web banner).
    pub summary: String,
}

impl ActivityContent {
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
}

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
}

impl Activity {
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
        }
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

    pub fn link(mut self, path: impl Into<String>) -> Self {
        self.link = Some(path.into());
        self
    }

    pub fn tint(mut self, c: Color) -> Self {
        self.tint = Some(c);
        self
    }

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
        let content = ActivityContent {
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
        };
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
    pub title: String,
    pub subtitle: Option<String>,
    pub body: String,
    pub link: Option<String>,
    pub delay: Option<Duration>,
    pub sound: bool,
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
    pub id: String,
    pub title: String,
    pub destructive: bool,
    /// Brings the app to the foreground (otherwise handled in the background).
    pub foreground: bool,
    /// Shows a text field (quick reply); the text arrives in the response.
    pub reply: Option<String>,
    pub link: Option<String>,
}

impl NotificationAction {
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
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }
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

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }
    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = Some(s.into());
        self
    }
    pub fn body(mut self, b: impl Into<String>) -> Self {
        self.body = b.into();
        self
    }
    pub fn link(mut self, path: impl Into<String>) -> Self {
        self.link = Some(path.into());
        self
    }
    /// Delivers after `delay` instead of immediately.
    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }
    pub fn silent(mut self) -> Self {
        self.sound = false;
        self
    }
    pub fn badge(mut self, n: u32) -> Self {
        self.badge = Some(n);
        self
    }
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
}

type NotificationHandler = std::rc::Rc<dyn Fn(&NotificationResponse, &mut App)>;

impl App {
    /// What the current platform supports (set by the backend).
    pub fn capabilities(&self) -> Capabilities {
        self.surfaces.capabilities
    }

    pub fn set_capabilities(&mut self, caps: Capabilities) {
        self.surfaces.capabilities = caps;
    }

    /// Registers a widget and schedules its first render.
    pub fn register_widget(&mut self, widget: Widget) {
        self.surfaces.widgets.retain(|w| w.kind != widget.kind);
        self.surfaces.widgets.push(widget);
        self.surfaces.widgets_dirty = true;
    }

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
        if let Some(handler) = self.surfaces.notification_handler.clone() {
            handler(&response, self);
        }
    }

    /// Posts a local notification (asks for permission first if needed).
    pub fn send_notification(&mut self, n: Notification) {
        self.surfaces.requests.push(PlatformRequest::Notify(n));
    }

    pub fn cancel_notification(&mut self, id: impl Into<String>) {
        self.surfaces
            .requests
            .push(PlatformRequest::CancelNotification(id.into()));
    }

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
