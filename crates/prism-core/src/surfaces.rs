//! System surfaces outside the app window: **widgets**, **Live Activities**
//! (lock screen + Dynamic Island) and **notifications**.
//!
//! The app describes them with the same elements as its UI (`div`, `text`,
//! `image`, `progress`, `timer`, `link`). Prism turns them into static
//! [`SNode`] snapshots (compact JSON) that a platform renders natively, even
//! when the app is not running:
//!
//! | Surface | iOS | macOS | Web | Android | Linux | Windows |
//! |---|---|---|---|---|---|---|
//! | Widgets | WidgetKit (SwiftUI) | WidgetKit | — | `RemoteViews` | — | — |
//! | Live Activities | ActivityKit: lock screen + Dynamic Island | menu bar item | in-page live region | ongoing notification | — | — |
//! | Notifications | UserNotifications | UserNotifications | Notification API / toast | `Notification.Builder` | `GNotification` | App SDK notifications |
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
pub(crate) use crate::widget_links::LinkCheck;
use crate::widget_links::{Signer, WidgetLinks};

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
    /// On Apple platforms, for apps that declare `location` under
    /// `[permissions]` in Prism.toml.
    pub location: bool,
    /// `watch_motion` (accelerometer, gyroscope, attitude).
    pub motion: bool,
    /// `take_photo` (Apple platforms: with `camera` under `[permissions]`).
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
    /// iOS and visionOS: for apps that declare `face_id` under `[permissions]`.
    pub biometry: crate::security::BiometryKind,
    /// `authenticate` can check the owner (biometrics or a passcode); on iOS
    /// and visionOS, with `face_id` under `[permissions]`.
    pub device_auth: bool,
    /// `sign_in_with_apple` (AuthenticationServices, with the entitlement).
    pub sign_in_with_apple: bool,
    /// Browser sign-in sessions (`prism::oauth`).
    pub web_auth: bool,
    /// Remote pushes (`register_for_push`), and the service they use.
    pub push: Option<crate::push::PushService>,
    /// Web views (`web_view`) show their page (an empty box otherwise). On
    /// Linux, only with prismsdk's `web-view` feature (WebKitGTK).
    pub web_view: bool,
    /// Files can be dropped on a drop target (`on_drop_files`, #111).
    pub drop_files: bool,
    /// Elements can be dragged onto drop targets inside the app
    /// (`draggable`, `on_drop_payload`, #251).
    pub drag_and_drop: bool,
    /// Passkeys of the app's own relying party (`webauthn_create`,
    /// `webauthn_get`, #94).
    pub passkeys: bool,
    /// In-app purchases (`purchase`, `owns`, #306): the platform's store
    /// (StoreKit, Google Play Billing) or the app's own
    /// ([`App::set_store`](crate::App::set_store)).
    pub purchases: bool,
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
    /// The number on the app icon (#239); `None` clears it.
    SetBadge(Option<u32>),
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
    /// Register a font face for the session or process
    /// ([`App::load_font`]); see [`crate::fonts`].
    LoadFont(crate::fonts::FontFace),
    /// Add the stylesheet at this URL to the page (web;
    /// [`App::add_stylesheet`]).
    AddStylesheet(String),
    /// Remove the stylesheet at this URL (its last handle was dropped).
    RemoveStylesheet(String),
    /// The page's or window's title ([`App::set_title`], #154); `None`:
    /// back to the default.
    SetTitle(Option<String>),
    /// The whole menu bar, defined by the app ([`App::set_menu_bar`], #161).
    SetMenuBar(Vec<crate::menu::MenuSpec>),
    /// The app has unsaved changes: leaving asks first
    /// ([`App::set_unsaved_changes`], #158).
    SetUnsavedChanges(bool),
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
    /// A timer's text once it has ended ([`Timer::after`](crate::Timer::after)).
    pub placeholder: String,
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
    /// A tap runs this app action without opening the app
    /// ([`Div::widget_action`](crate::Div::widget_action)); its `kind` is
    /// the widget's or Live Activity's, set when the surface is rendered.
    pub action: Option<WidgetAction>,
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
    fn from_vnode(mut v: VNode, mut children: Vec<SNode>) -> SNode {
        // Renderers lay children out in the order given, leading to
        // trailing or top to bottom: `order` and reversed directions are
        // resolved here (stable sort, then reversed; the main-axis packing
        // flips with it).
        children.sort_by_key(|c| c.style.order);
        if v.style.direction.is_reverse() {
            children.reverse();
            v.style.direction = if v.style.direction.is_row() {
                FlexDirection::Row
            } else {
                FlexDirection::Column
            };
            v.style.justify_content = match v.style.justify_content {
                Justify::Start => Justify::End,
                Justify::End => Justify::Start,
                other => other,
            };
        }
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
            placeholder: if v.kind == NodeKind::Timer {
                v.placeholder
            } else {
                String::new()
            },
            link: v.link,
            action: v.widget_action.map(|(action, payload)| WidgetAction {
                kind: String::new(),
                action,
                payload,
            }),
            children,
        }
    }

    /// Sets the surface kind of every action in the tree (`kind` of
    /// [`WidgetAction`]).
    fn tag_actions(&mut self, kind: &str) {
        if let Some(a) = &mut self.action {
            a.kind = kind.to_string();
        }
        self.children.iter_mut().for_each(|c| c.tag_actions(kind));
    }

    /// Compact JSON (Live Activity payloads are limited to 4 KB).
    ///
    /// Keys: `k` kind (`v` view, `t` text, `i` image, `p` progress, `m` timer,
    /// `sh` shape), `d` 1 = row, `g` gap, `pd` padding and `m` margin
    /// `[t,r,b,l]`, `w`/`h` size (number or `"f"` = fill), `f` flex grow,
    /// `ai`/`jc` alignment, `bg`/`fg`/`bc` colors (`"#RRGGBBAA"` or
    /// `["#light","#dark"]`), `gr` a gradient over `bg` (`{"k","a","s"}`,
    /// see `write_gradient`), `r` radius, `bw` border, `fs`/`fw`/`ta` font (`fw` 1-3
    /// medium/semibold/bold, else a CSS weight; typography keys
    /// `ff`/`it`/`ul`/`st`/`tn`/`tt`/`ls`/`lh`/`ml`/`nw`, see `write_typography`),
    /// `o` opacity, `s` text, symbol or drawing, `v` value, `ph` a timer's
    /// text once ended, `l` link,
    /// `wa` widget action (`{"k":kind,"a":action,"p":payload}`, and `t` its
    /// link token in a widget of an app with a widget key),
    /// `al` accessibility label, `ah` hidden, `hd` heading, `c` children.
    ///
    /// A shape's drawing is [`Drawing::encode`](crate::shape::Drawing) with
    /// short numbers ([`compact_drawing`]): paths in fractions of the node's
    /// frame, resolved by the renderer at the size it lays the node out.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out, 3, None);
        out
    }

    /// [`to_json`](Self::to_json), each widget action with its `t` token
    /// for the current drawing when the app has a widget key
    /// ([`App::set_widget_key`]).
    pub(crate) fn to_signed_json(&self, signer: Option<&Signer<'_>>) -> String {
        let mut out = String::new();
        self.write_json(&mut out, 3, signer);
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

    fn write_json(&self, o: &mut String, decimals: usize, signer: Option<&Signer<'_>>) {
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
        // Minimum sizes in points (#321).
        for (key, len) in [("mw", s.min_width), ("mh", s.min_height)] {
            if let Length::Px(v) = len
                && v > 0.
            {
                let _ = write!(o, r#","{key}":{}"#, num(v));
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
            Justify::Start | Justify::Stretch => None,
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
        if let Some(g) = &s.gradient {
            write_gradient(o, g, s.text_color);
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
            // `Color::current()` paths (an SVG's `currentColor`): the node's
            // text color, else the label color (renderers read hex colors).
            let label = Color::adaptive(Color::BLACK, Color::WHITE);
            let resolved;
            let content = if content.contains('~') || !plain_drawing(content) {
                let text = s.text_color.map_or(label, |c| c.resolve_current(label));
                resolved = crate::shape::Drawing::decode(content)
                    .with_current_color(text)
                    .plain()
                    .encode();
                &resolved
            } else {
                content
            };
            let drawing = compact_drawing(content, decimals);
            let _ = write!(o, r#","s":{}"#, json_str(&drawing));
        } else if !content.is_empty() {
            let _ = write!(o, r#","s":{}"#, json_str(content));
        }
        if let Some(v) = self.value {
            let _ = write!(o, r#","v":{}"#, num(v as f32).max_precision(v));
        }
        if !self.placeholder.is_empty() {
            let _ = write!(o, r#","ph":{}"#, json_str(&self.placeholder));
        }
        if let Some(l) = &self.link {
            let _ = write!(o, r#","l":{}"#, json_str(l));
        }
        if let Some(a) = &self.action {
            let _ = write!(
                o,
                r#","wa":{{"k":{},"a":{},"p":{}"#,
                json_str(&a.kind),
                json_str(&a.action),
                json_str(&a.payload)
            );
            if let Some(signer) = signer {
                let _ = write!(o, r#","t":"{}""#, signer.token(a));
            }
            o.push('}');
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
                c.write_json(o, decimals, signer);
            }
            o.push(']');
        }
        o.push('}');
    }
}

/// A background gradient, over `bg` (#330): `"gr":{"k","a","s"}`, `k` the
/// kind (`l` linear, `r` radial, `c` conic), `a` the angle in degrees (CSS
/// convention: a linear one's direction, `0` to the top, `90` to the right;
/// a conic one's start, clockwise from up; `0` for a radial one), `s` the
/// stops `[[color, position], …]` (positions `0`-`1`, increasing, two at
/// least). `Color::current()` stops are resolved against the node's text
/// color, else the label color.
fn write_gradient(o: &mut String, g: &crate::Gradient, text: Option<Color>) {
    use crate::GradientKind;
    let (k, a) = match g.kind {
        GradientKind::Linear { angle } => ("l", angle),
        GradientKind::Radial => ("r", 0.),
        GradientKind::Conic { from } => ("c", from),
    };
    let label = Color::adaptive(Color::BLACK, Color::WHITE);
    let text = text.map_or(label, |c| c.resolve_current(label));
    let _ = write!(o, r#","gr":{{"k":"{k}","a":{},"s":["#, num(a));
    for (i, st) in g.stops.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = write!(
            o,
            "[{},{}]",
            color_json(st.color.resolve_current(text)),
            num(st.at)
        );
    }
    o.push_str("]}");
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

/// Whether a drawing's text form has only what the renderers of surfaces
/// draw: paths in flat colors (no gradient, text, image or group).
fn plain_drawing(text: &str) -> bool {
    text.lines().all(|line| match line.split_once('|') {
        Some((paint, _)) => !paint.split_whitespace().any(|t| t.starts_with(['F', 'S'])),
        None => line.is_empty() || line.starts_with(['i', 'n']),
    })
}

/// A drawing's text form ([`Drawing::encode`](crate::shape::Drawing)) in
/// fewer bytes: numbers rounded to `decimals` without leading zeros
/// (`0.25` → `.25`), opaque colors without their alpha (`#5B5BD6FF` →
/// `#5B5BD6`). Every reader accepts both forms.
pub fn compact_drawing(text: &str, decimals: usize) -> String {
    // Gradients, text, images and groups are not drawn there: the paths
    // in flat colors, each group's opacity in them.
    let plain;
    let text = if plain_drawing(text) {
        text
    } else {
        plain = crate::shape::Drawing::decode(text).plain().encode();
        &plain
    };
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

/// Widget sizes (WidgetKit families; Android gives each a size in dp).
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

type WidgetRender = Rc<dyn Fn(&mut App, WidgetFamily, &WidgetConfig) -> Element>;
type WidgetOptions = Rc<dyn Fn(&mut App) -> Vec<WidgetOption>>;
type WidgetAhead = Rc<dyn Fn(&mut App, &WidgetConfig) -> Vec<SystemTime>>;
type WidgetRefresh = Rc<
    dyn Fn(
        crate::AsyncApp,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>,
>;

/// When the system asks a widget for new drawings on its own, without the
/// app ([`Widget::reload`]): WidgetKit's timeline reload policy, an alarm on
/// Android. Each time, the widget's [`refresh`](Widget::refresh) hook runs
/// (when it has one), then the widget is drawn again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetReload {
    /// Only when the app reloads its widgets ([`App::reload_widgets`]).
    /// WidgetKit's `.never`.
    Never,
    /// When the last drawing ahead ([`Widget::ahead`]) comes, or soon after
    /// the reload for a widget drawn for now only (the system picks when).
    /// WidgetKit's `.atEnd`.
    AtEnd,
    /// This long after each drawing (the system may wait longer: it spaces
    /// reloads out to fit its daily budget). WidgetKit's `.after(date)`.
    After(Duration),
}

impl WidgetReload {
    /// The snapshot's `"reload"` value: `"never"`, `"end"` or the delay in
    /// seconds.
    fn to_json(self) -> String {
        match self {
            WidgetReload::Never => r#""never""#.into(),
            WidgetReload::AtEnd => r#""end""#.into(),
            WidgetReload::After(d) => format!("{}", (d.as_secs_f64() * 1000.).round() / 1000.),
        }
    }
}

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
    /// What the user picks for each instance in the system's « Edit widget »
    /// sheet ([`Widget::parameter`]); none for a static widget.
    pub parameters: Vec<WidgetParameter>,
    /// How long the [`refresh`](Self::refresh) hook may run before it is
    /// given up (default [`REFRESH_TIMEOUT`](Self::REFRESH_TIMEOUT)).
    pub refresh_timeout: Duration,
    /// When the system asks for new drawings ([`Widget::reload`]); `None`
    /// for the default.
    pub reload: Option<WidgetReload>,
    render: WidgetRender,
    ahead: Option<WidgetAhead>,
    refresh: Option<WidgetRefresh>,
}

impl Widget {
    /// The most configurations of one kind rendered on each reload (the
    /// default one included). WidgetKit reads every snapshot of a kind from
    /// one file: this keeps it small and the reload quick.
    pub const MAX_CONFIGURATIONS: usize = 16;

    /// The most instants a configuration is drawn ahead for
    /// ([`Widget::ahead`]): the soonest are kept.
    pub const MAX_AHEAD: usize = 16;

    /// How long a [`refresh`](Self::refresh) hook may run by default.
    /// WidgetKit gives its extension a few seconds for a timeline, and
    /// kills it when it takes much longer.
    pub const REFRESH_TIMEOUT: Duration = Duration::from_secs(10);

    /// How often a widget with a [`refresh`](Self::refresh) hook asks for
    /// new drawings when it sets no [`reload`](Self::reload) policy.
    pub const DEFAULT_RELOAD: Duration = Duration::from_secs(15 * 60);

    /// `kind` is a stable identifier (`"todos"`), `name` is shown in the gallery.
    pub fn new(kind: impl Into<String>, name: impl Into<String>) -> Self {
        Widget {
            kind: kind.into(),
            name: name.into(),
            description: String::new(),
            families: vec![WidgetFamily::Small, WidgetFamily::Medium],
            link: None,
            parameters: Vec::new(),
            refresh_timeout: Self::REFRESH_TIMEOUT,
            reload: None,
            render: Rc::new(|_, _, _| Element::Empty),
            ahead: None,
            refresh: None,
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

    /// Adds a parameter the user sets per instance in the system's « Edit
    /// widget » sheet (iOS 17+, macOS 14+), like a city for a weather widget.
    /// Several instances of the kind can then show different things: render
    /// with [`Widget::render_configured`] to read the chosen values.
    pub fn parameter(mut self, parameter: WidgetParameter) -> Self {
        self.parameters.retain(|p| p.id != parameter.id);
        self.parameters.push(parameter);
        self
    }

    /// The widget's content for a family. Read any entity through `cx`.
    pub fn render<E: IntoElement>(
        mut self,
        f: impl Fn(&mut App, WidgetFamily) -> E + 'static,
    ) -> Self {
        self.render = Rc::new(move |app, family, _| f(app, family).into_element());
        self
    }

    /// Like [`Widget::render`], with the configuration of the instance being
    /// drawn: one value per [`parameter`](Widget::parameter), the default
    /// where the user picked nothing.
    pub fn render_configured<E: IntoElement>(
        mut self,
        f: impl Fn(&mut App, WidgetFamily, &WidgetConfig) -> E + 'static,
    ) -> Self {
        self.render = Rc::new(move |app, family, config| f(app, family, config).into_element());
        self
    }

    /// The instants at which the widget's drawing changes, such as each
    /// departure of a « next bus » widget: the widget is drawn ahead for
    /// each of them, and the system switches to that drawing at that instant
    /// without the app (a countdown then moves on to the next departure
    /// instead of stopping at 0:00).
    ///
    /// Called on every reload, once per configuration (an empty one for a
    /// widget without parameters). The render closure is then called again
    /// for each instant, with [`App::now`] returning it, so a drawing for
    /// 15:40 can leave out what has gone by then. Instants already past are
    /// dropped and the [`MAX_AHEAD`](Self::MAX_AHEAD) soonest are kept.
    ///
    /// ```
    /// # use prism_core::prelude::*;
    /// # use prism_core::{Widget, timer};
    /// # use std::time::{Duration, SystemTime};
    /// # let departures: Vec<SystemTime> = vec![SystemTime::now() + Duration::from_secs(300)];
    /// # let d = departures.clone();
    /// Widget::new("next", "Next departure")
    ///     .render(move |cx, _| {
    ///         let next = d.iter().find(|t| **t > cx.now()).copied();
    ///         match next {
    ///             Some(t) => timer(t).after("Departed").into_element(),
    ///             None => text("No more departures").into_element(),
    ///         }
    ///     })
    ///     .ahead(move |_, _| departures.clone());
    /// ```
    pub fn ahead<I: IntoIterator<Item = SystemTime>>(
        mut self,
        f: impl Fn(&mut App, &WidgetConfig) -> I + 'static,
    ) -> Self {
        self.ahead = Some(Rc::new(move |app, config| {
            f(app, config).into_iter().collect()
        }));
        self
    }

    /// Fetches fresh data for the widget when the system asks for new
    /// drawings on its own, without the app: WidgetKit's timeline reloads
    /// (see [`reload`](Self::reload)) on iOS, iPadOS, macOS and visionOS,
    /// an alarm on Android. The hook runs in the app's code without its
    /// window, in the widget extension's process on Apple platforms: it
    /// can fetch over HTTP (`prism::http`), read and write the app's
    /// storage (shared with the extension through the App Group) and
    /// update entities. When it returns `Ok`, the widget is drawn again
    /// with [`render`](Self::render) and [`ahead`](Self::ahead) from what
    /// it changed.
    ///
    /// An `Err`, or a hook still running after
    /// [`refresh_timeout`](Self::refresh_timeout), leaves the previous
    /// drawings on screen (the error is logged), and the system asks again
    /// at the next reload.
    ///
    /// On Apple platforms the hook runs in the extension, a process of its
    /// own, even while the app is open: keep what both must see in storage.
    /// There, a runtime started from the app's `init` draws only the
    /// widgets whose hook succeeded, so the drawings the app wrote stay for
    /// the others. On Android it runs in the app's process: in the open
    /// app's runtime (which then draws every widget), or in one without a
    /// screen.
    ///
    /// ```
    /// # use prism_core::prelude::*;
    /// # use prism_core::Widget;
    /// # use std::time::Duration;
    /// # struct Board { lines: Vec<String> }
    /// # async fn fetch_board() -> Result<Vec<String>, String> { Ok(Vec::new()) }
    /// # fn build(cx: &mut App) {
    /// let board = cx.new_entity(|_| Board { lines: Vec::new() });
    /// cx.register_widget(
    ///     Widget::new("board", "Departures")
    ///         .render(move |cx, _| text(board.read(cx).lines.join("\n")))
    ///         .refresh(move |cx| async move {
    ///             let lines = fetch_board().await?;
    ///             cx.update_entity(&board, |b, _| b.lines = lines);
    ///             Ok::<_, String>(())
    ///         })
    ///         .reload_after(Duration::from_secs(30 * 60)),
    /// );
    /// # }
    /// ```
    pub fn refresh<F, E>(mut self, hook: impl Fn(crate::AsyncApp) -> F + 'static) -> Self
    where
        F: std::future::Future<Output = Result<(), E>> + 'static,
        E: std::fmt::Display + 'static,
    {
        self.refresh = Some(Rc::new(move |cx| {
            let fut = hook(cx);
            Box::pin(async move { fut.await.map_err(|e| e.to_string()) })
        }));
        self
    }

    /// How long the [`refresh`](Self::refresh) hook may run (default
    /// [`REFRESH_TIMEOUT`](Self::REFRESH_TIMEOUT), 10 s). Past it, the hook
    /// is dropped and the previous drawings stay. Keep it well under the
    /// system's own limit: WidgetKit ends an extension that takes too long
    /// to answer, Android a broadcast after about ten seconds.
    pub fn refresh_timeout(mut self, timeout: Duration) -> Self {
        self.refresh_timeout = timeout;
        self
    }

    /// When the system asks for new drawings on its own, running the
    /// [`refresh`](Self::refresh) hook first.
    ///
    /// Without a policy, a widget with a refresh hook asks
    /// [`DEFAULT_RELOAD`](Self::DEFAULT_RELOAD) (15 minutes) after each
    /// drawing; one without a hook asks when its last drawing ahead comes
    /// ([`WidgetReload::AtEnd`]) and never when it has none (the app
    /// redraws it).
    pub fn reload(mut self, policy: WidgetReload) -> Self {
        self.reload = Some(policy);
        self
    }

    /// Asks for new drawings `delay` after each one: [`reload`](Self::reload)
    /// with [`WidgetReload::After`]. The system may wait longer, to fit its
    /// budget (WidgetKit: a few dozen reloads a day for a widget the user
    /// looks at).
    pub fn reload_after(self, delay: Duration) -> Self {
        self.reload(WidgetReload::After(delay))
    }

    /// Whether the widget has a [`refresh`](Self::refresh) hook.
    pub fn has_refresh(&self) -> bool {
        self.refresh.is_some()
    }

    /// The reload policy written to the snapshot: the widget's, else
    /// [`DEFAULT_RELOAD`](Self::DEFAULT_RELOAD) for a widget with a refresh
    /// hook, else none (the extension's own default: `.atEnd` with
    /// drawings ahead, `.never` without).
    pub fn reload_policy(&self) -> Option<WidgetReload> {
        self.reload.or_else(|| {
            self.refresh
                .as_ref()
                .map(|_| WidgetReload::After(Self::DEFAULT_RELOAD))
        })
    }

    /// Whether the widget has parameters (the edit sheet offers them).
    pub fn is_configurable(&self) -> bool {
        !self.parameters.is_empty()
    }

    /// Renders the default configuration, then `requested` (the instances
    /// the system shows), then, when every combination of options fits in
    /// [`Widget::MAX_CONFIGURATIONS`], all of them, so any choice in the edit
    /// sheet shows at once.
    pub(crate) fn snapshot(&self, app: &mut App, requested: &[WidgetConfig]) -> WidgetSnapshot {
        let options: Vec<ParameterSnapshot> = self
            .parameters
            .iter()
            .map(|p| {
                let options = (p.options)(app);
                let default = p
                    .default
                    .as_ref()
                    .filter(|d| options.iter().any(|o| &o.id == *d))
                    .or(options.first().map(|o| &o.id))
                    .cloned();
                ParameterSnapshot {
                    id: p.id.clone(),
                    default,
                    options,
                }
            })
            .collect();
        let default = WidgetConfig {
            values: options
                .iter()
                .filter_map(|p| Some((p.id.clone(), p.default.clone()?)))
                .collect(),
        }
        .sorted();
        let mut configs = vec![default.clone()];
        let mut add = |config: WidgetConfig| {
            if configs.len() < Self::MAX_CONFIGURATIONS && !configs.contains(&config) {
                configs.push(config);
            }
        };
        for r in requested {
            add(normalize(&options, r));
        }
        let choices: Vec<&ParameterSnapshot> =
            options.iter().filter(|p| !p.options.is_empty()).collect();
        let combinations = choices
            .iter()
            .try_fold(1usize, |n, p| n.checked_mul(p.options.len()));
        if !choices.is_empty() && combinations.is_some_and(|n| n <= Self::MAX_CONFIGURATIONS) {
            // Every combination, like an odometer over the options.
            let mut at = vec![0; choices.len()];
            'all: loop {
                add(WidgetConfig {
                    values: choices
                        .iter()
                        .zip(&at)
                        .map(|(p, &i)| (p.id.clone(), p.options[i].id.clone()))
                        .collect(),
                }
                .sorted());
                for (i, p) in choices.iter().enumerate() {
                    at[i] += 1;
                    if at[i] < p.options.len() {
                        continue 'all;
                    }
                    at[i] = 0;
                }
                break;
            }
        }
        let now = SystemTime::now();
        let mut ahead = Vec::new();
        let mut rendered = configs
            .into_iter()
            .map(|config| {
                let families = self.draw(app, &config, now);
                let entries = self.instants(app, &config, now);
                if !entries.is_empty() {
                    let entries = entries
                        .into_iter()
                        .map(|at| WidgetEntry {
                            at,
                            families: self.draw(app, &config, at),
                        })
                        .collect();
                    ahead.push((config.clone(), entries));
                }
                (config, families)
            })
            .collect::<Vec<_>>();
        let (_, families) = rendered.remove(0);
        WidgetSnapshot {
            kind: self.kind.clone(),
            link: self.link.clone(),
            reload: self.reload_policy(),
            refresh: self.has_refresh(),
            families,
            parameters: options
                .into_iter()
                .map(|p| (p.id, p.default, p.options))
                .collect(),
            default,
            configs: rendered,
            ahead,
        }
    }

    /// Every family drawn for `config` at the instant `at` ([`App::now`]).
    fn draw(
        &self,
        app: &mut App,
        config: &WidgetConfig,
        at: SystemTime,
    ) -> Vec<(WidgetFamily, String)> {
        let outer = app.surfaces.drawing_at.replace(at);
        let families = self
            .families
            .iter()
            .map(|f| {
                let element = (self.render)(app, *f, config);
                let mut node = snapshot_one(app, element);
                node.tag_actions(&self.kind);
                let signer = app
                    .surfaces
                    .widget_links
                    .as_ref()
                    .and_then(WidgetLinks::signer);
                (*f, node.to_signed_json(signer.as_ref()))
            })
            .collect();
        app.surfaces.drawing_at = outer;
        families
    }

    /// The instants `config` is drawn ahead for: after `now`, sorted, at
    /// most [`MAX_AHEAD`](Self::MAX_AHEAD).
    fn instants(&self, app: &mut App, config: &WidgetConfig, now: SystemTime) -> Vec<SystemTime> {
        let Some(ahead) = &self.ahead else {
            return Vec::new();
        };
        let mut at: Vec<SystemTime> = ahead(app, config)
            .into_iter()
            .filter(|t| *t > now)
            .collect();
        at.sort();
        at.dedup();
        at.truncate(Self::MAX_AHEAD);
        at
    }

    /// Static metadata (used to generate the platform widget extension).
    pub fn describe_json(&self) -> String {
        let families: Vec<String> = self.families.iter().map(|f| json_str(f.name())).collect();
        let parameters: Vec<String> = self
            .parameters
            .iter()
            .map(|p| {
                format!(
                    r#"{{"id":{},"title":{}}}"#,
                    json_str(&p.id),
                    json_str(&p.title)
                )
            })
            .collect();
        format!(
            r#"{{"kind":{},"name":{},"description":{},"families":[{}],"parameters":[{}],"refresh":{}}}"#,
            json_str(&self.kind),
            json_str(&self.name),
            json_str(&self.description),
            families.join(","),
            parameters.join(","),
            self.has_refresh()
        )
    }
}

/// A parameter's options and default, read once per reload.
struct ParameterSnapshot {
    id: String,
    default: Option<String>,
    options: Vec<WidgetOption>,
}

/// `config` with only the parameters the widget has, each set to one of its
/// options (the default for a missing or removed choice).
fn normalize(parameters: &[ParameterSnapshot], config: &WidgetConfig) -> WidgetConfig {
    WidgetConfig {
        values: parameters
            .iter()
            .filter_map(|p| {
                let chosen = config
                    .get(&p.id)
                    .filter(|v| p.options.iter().any(|o| o.id == *v))
                    .map(str::to_owned);
                Some((p.id.clone(), chosen.or_else(|| p.default.clone())?))
            })
            .collect(),
    }
    .sorted()
}

/// A setting of a configurable widget, shown in the system's « Edit widget »
/// sheet: a title and a list of options the app provides.
///
/// ```
/// # use prism_core::{WidgetOption, WidgetParameter};
/// let place = WidgetParameter::new("place", "Place")
///     .options(|_cx| vec![WidgetOption::new("all", "Everywhere"), WidgetOption::new("1", "Kitchen")])
///     .default("all");
/// ```
#[derive(Clone)]
pub struct WidgetParameter {
    /// Stable identifier, the key in [`WidgetConfig`] (`"place"`).
    pub id: String,
    /// Shown in the edit sheet.
    pub title: String,
    /// The option picked until the user changes it (default: the first).
    pub default: Option<String>,
    options: WidgetOptions,
}

impl WidgetParameter {
    /// `id` keys the chosen value in [`WidgetConfig`]; `title` is shown in
    /// the edit sheet.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        WidgetParameter {
            id: id.into(),
            title: title.into(),
            default: None,
            options: Rc::new(|_| Vec::new()),
        }
    }

    /// The choices, read from app state on every widget reload: when they
    /// change (a room added, a list renamed), call [`App::reload_widgets`]
    /// and the edit sheet offers the new ones.
    pub fn options(mut self, f: impl Fn(&mut App) -> Vec<WidgetOption> + 'static) -> Self {
        self.options = Rc::new(f);
        self
    }

    /// The option's id picked until the user changes it. Without one, or
    /// when it is not among the options, the first option is.
    pub fn default(mut self, id: impl Into<String>) -> Self {
        self.default = Some(id.into());
        self
    }
}

/// One choice of a [`WidgetParameter`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WidgetOption {
    /// Stable identifier, the value in [`WidgetConfig`] (`"1"`).
    pub id: String,
    /// Shown in the edit sheet (`"Kitchen"`).
    pub title: String,
}

impl WidgetOption {
    /// An option: `id` is what the app reads back, `title` what the user sees.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        WidgetOption {
            id: id.into(),
            title: title.into(),
        }
    }
}

/// The values chosen for one instance of a configurable widget: an option id
/// per parameter id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WidgetConfig {
    /// Sorted by parameter id.
    values: Vec<(String, String)>,
}

impl WidgetConfig {
    /// No value set.
    pub fn new() -> Self {
        WidgetConfig::default()
    }

    /// Sets `parameter` to the option `value`.
    pub fn with(mut self, parameter: impl Into<String>, value: impl Into<String>) -> Self {
        let parameter = parameter.into();
        self.values.retain(|(k, _)| *k != parameter);
        self.values.push((parameter, value.into()));
        self.sorted()
    }

    /// The option chosen for `parameter`.
    pub fn get(&self, parameter: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(k, _)| k == parameter)
            .map(|(_, v)| v.as_str())
    }

    /// Every `(parameter, option)` pair, by parameter id.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// No value set.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// `{"parameter":"option",…}`.
    pub fn to_json(&self) -> String {
        let items: Vec<String> = self
            .values
            .iter()
            .map(|(k, v)| format!("{}:{}", json_str(k), json_str(v)))
            .collect();
        format!("{{{}}}", items.join(","))
    }

    /// For backends: the configurations a widget extension recorded, a JSON
    /// array of `{"parameter":"option"}` objects. Anything else is skipped.
    pub fn list_from_json(json: &str) -> Vec<WidgetConfig> {
        let Some(crate::push::Json::Array(items)) = crate::push::Json::parse(json) else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|item| match item {
                crate::push::Json::Object(fields) => Some(fields),
                _ => None,
            })
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                    .fold(WidgetConfig::new(), |c, (k, v)| c.with(k, v))
            })
            .collect()
    }

    fn sorted(mut self) -> Self {
        self.values.sort();
        self
    }
}

/// A widget's content for each family, as JSON snapshots.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetSnapshot {
    /// The widget's [`Widget::kind`].
    pub kind: String,
    /// Deep link opened when the widget background is tapped.
    pub link: Option<String>,
    /// When the system asks for new drawings ([`Widget::reload_policy`]);
    /// `None` for the extension's default.
    pub reload: Option<WidgetReload>,
    /// Whether the widget has a [`refresh`](Widget::refresh) hook: the
    /// system runs it before drawing the widget again.
    pub refresh: bool,
    /// Each family with its [`SNode`] JSON (the default configuration's, for
    /// a configurable widget).
    pub families: Vec<(WidgetFamily, String)>,
    /// Each parameter's id, default option and options, as read for this
    /// reload (the edit sheet lists them).
    pub parameters: Vec<(String, Option<String>, Vec<WidgetOption>)>,
    /// The default configuration, drawn in [`families`](Self::families).
    pub default: WidgetConfig,
    /// The other configurations rendered, each with its families.
    pub configs: Vec<(WidgetConfig, Vec<(WidgetFamily, String)>)>,
    /// The drawings ahead ([`Widget::ahead`]) of each configuration that has
    /// some, the default one included, soonest first.
    pub ahead: Vec<(WidgetConfig, Vec<WidgetEntry>)>,
}

/// A widget drawn ahead for an instant ([`Widget::ahead`]): the system shows
/// it from `at` on.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetEntry {
    /// From when this drawing is shown.
    pub at: SystemTime,
    /// Each family with its [`SNode`] JSON, drawn for `at`.
    pub families: Vec<(WidgetFamily, String)>,
}

impl WidgetSnapshot {
    /// JSON for the widget extension: kind, link, update time and families;
    /// the drawings ahead (`"ahead":[{"at":…,"families":…}]`, only when there
    /// are some); the reload policy (`"reload"`: `"never"`, `"end"` or a
    /// delay in seconds, only when set) and `"refresh":true` for a widget
    /// with a refresh hook; for a configurable widget, its parameters (with
    /// their options) and the other configurations rendered, each with its
    /// own drawings ahead.
    pub fn to_json(&self) -> String {
        fn families(families: &[(WidgetFamily, String)]) -> String {
            let fams: Vec<String> = families
                .iter()
                .map(|(f, json)| format!("{}:{json}", json_str(f.name())))
                .collect();
            format!("{{{}}}", fams.join(","))
        }
        let ahead = |config: &WidgetConfig| -> String {
            let entries = self.ahead_for(config);
            if entries.is_empty() {
                return String::new();
            }
            let entries: Vec<String> = entries
                .iter()
                .map(|e| {
                    let at = crate::element::unix_seconds(e.at);
                    format!(
                        r#"{{"at":{},"families":{}}}"#,
                        (at * 1000.).round() / 1000.,
                        families(&e.families)
                    )
                })
                .collect();
            format!(r#","ahead":[{}]"#, entries.join(","))
        };
        let mut out = format!(
            r#"{{"kind":{},"link":{},"updated":{},"families":{}{}"#,
            json_str(&self.kind),
            self.link.as_deref().map_or("null".into(), json_str),
            unix_now(),
            families(&self.families),
            ahead(&self.default)
        );
        if let Some(reload) = self.reload {
            let _ = write!(out, r#","reload":{}"#, reload.to_json());
        }
        if self.refresh {
            out.push_str(r#","refresh":true"#);
        }
        if !self.parameters.is_empty() {
            let params: Vec<String> = self
                .parameters
                .iter()
                .map(|(id, default, options)| {
                    let options: Vec<String> = options
                        .iter()
                        .map(|o| {
                            format!(
                                r#"{{"id":{},"title":{}}}"#,
                                json_str(&o.id),
                                json_str(&o.title)
                            )
                        })
                        .collect();
                    format!(
                        r#"{{"id":{},"default":{},"options":[{}]}}"#,
                        json_str(id),
                        default.as_deref().map_or("null".into(), json_str),
                        options.join(",")
                    )
                })
                .collect();
            let configs: Vec<String> = self
                .configs
                .iter()
                .map(|(config, fams)| {
                    format!(
                        r#"{{"values":{},"families":{}{}}}"#,
                        config.to_json(),
                        families(fams),
                        ahead(config)
                    )
                })
                .collect();
            let _ = write!(
                out,
                r#","parameters":[{}],"default":{},"configs":[{}]"#,
                params.join(","),
                self.default.to_json(),
                configs.join(",")
            );
        }
        out.push('}');
        out
    }

    /// The families drawn for `config` (the default configuration's for an
    /// empty or unknown one).
    pub fn families_for(&self, config: &WidgetConfig) -> &[(WidgetFamily, String)] {
        self.configs
            .iter()
            .find(|(c, _)| c == config)
            .map_or(&self.families, |(_, f)| f)
    }

    /// The drawings ahead of `config` (none for an unknown one, or one the
    /// widget does not draw ahead).
    pub fn ahead_for(&self, config: &WidgetConfig) -> &[WidgetEntry] {
        self.ahead
            .iter()
            .find(|(c, _)| c == config)
            .map_or(&[], |(_, e)| e)
    }
}

// ---------------------------------------------------------------------------
// Widget actions: buttons that run app code without opening the app (#47)
// ---------------------------------------------------------------------------

/// A tap on a widget or Live Activity button
/// ([`Div::widget_action`](crate::Div::widget_action)), handled by
/// [`App::on_widget_action`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WidgetAction {
    /// The kind of the widget (or Live Activity) that was tapped.
    pub kind: String,
    /// The action, as given to `widget_action` (`"run-shortcut"`).
    pub action: String,
    /// Its payload, as given to `widget_action` (an id, JSON…).
    pub payload: String,
}

impl WidgetAction {
    /// The app path that runs an action where a widget cannot run app code
    /// in place: the tap opens the app on it, and
    /// [`Runtime::open_link`](crate::Runtime::open_link) runs the action
    /// instead of navigating, once, when the link carries a token of the
    /// widgets' drawing ([`App::widget_action_link`]).
    pub const LINK_PATH: &'static str = "/__prism/widget-action";

    /// An action of the widget `kind`.
    pub fn new(
        kind: impl Into<String>,
        action: impl Into<String>,
        payload: impl Into<String>,
    ) -> Self {
        WidgetAction {
            kind: kind.into(),
            action: action.into(),
            payload: payload.into(),
        }
    }

    /// The deep link of this action
    /// (`/__prism/widget-action?kind=…&action=…&payload=…`). It runs the
    /// action only with a `token` of the app added
    /// ([`App::widget_action_link`]), once: any app or web page can open an
    /// app's links, and a link opened once can be opened again.
    pub fn to_link(&self) -> String {
        format!(
            "{}?{}",
            Self::LINK_PATH,
            crate::router::encode_query([
                ("kind", self.kind.as_str()),
                ("action", self.action.as_str()),
                ("payload", self.payload.as_str()),
            ])
        )
    }

    /// The action of a [`to_link`](Self::to_link) path, `None` for any
    /// other path. Whether the link may run it is the token's business
    /// ([`Runtime::open_link`](crate::Runtime::open_link)).
    pub fn from_link(path: &str) -> Option<WidgetAction> {
        let (path, query) = crate::router::split_query(path);
        if path != Self::LINK_PATH {
            return None;
        }
        let mut action = WidgetAction::default();
        for (k, v) in crate::router::parse_query(query) {
            match k.as_str() {
                "kind" => action.kind = v,
                "action" => action.action = v,
                "payload" => action.payload = v,
                _ => {}
            }
        }
        (!action.action.is_empty()).then_some(action)
    }

    /// The `token` of a widget-action link, if it has one.
    pub(crate) fn link_token(path: &str) -> Option<String> {
        let (_, query) = crate::router::split_query(path);
        crate::router::parse_query(query)
            .into_iter()
            .find_map(|(k, v)| (k == "token").then_some(v))
    }

    /// For backends: actions a widget extension queued for the app (a JSON
    /// array of `{"k":kind,"a":action,"p":payload}`). Anything else is
    /// skipped.
    pub fn list_from_json(json: &str) -> Vec<WidgetAction> {
        let Some(crate::push::Json::Array(items)) = crate::push::Json::parse(json) else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|item| {
                let crate::push::Json::Object(fields) = item else {
                    return None;
                };
                let get = |key: &str| {
                    fields
                        .iter()
                        .find(|(k, _)| k == key)
                        .and_then(|(_, v)| v.as_str())
                        .map(str::to_owned)
                };
                Some(WidgetAction {
                    kind: get("k").unwrap_or_default(),
                    action: get("a").filter(|a| !a.is_empty())?,
                    payload: get("p").unwrap_or_default(),
                })
            })
            .collect()
    }
}

type WidgetActionJob = Rc<
    dyn Fn(
        WidgetAction,
        crate::AsyncApp,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
>;

impl App {
    /// Runs `job` when a widget or Live Activity button made with
    /// [`Div::widget_action`](crate::Div::widget_action) is tapped. The
    /// platform keeps the app running until the future completes (it may
    /// have been launched in the background just for it: register the
    /// handler at startup), then the widgets are re-rendered, so a job only
    /// changes state.
    ///
    /// ```ignore
    /// cx.on_widget_action(move |action, cx| async move {
    ///     if action.action == "toggle" {
    ///         cx.update_entity(&store, |s, cx| s.toggle(&action.payload, cx));
    ///     }
    /// });
    /// ```
    ///
    /// iOS gives an action a few seconds (keep network calls short);
    /// Android about ten.
    pub fn on_widget_action<F: std::future::Future<Output = ()> + 'static>(
        &mut self,
        job: impl Fn(WidgetAction, crate::AsyncApp) -> F + 'static,
    ) {
        self.surfaces.widget_action = Some(Rc::new(move |a, cx| Box::pin(job(a, cx))));
    }

    /// Whether a widget action handler is registered.
    pub fn handles_widget_actions(&self) -> bool {
        self.surfaces.widget_action.is_some()
    }

    /// Starts the app's [`on_widget_action`](App::on_widget_action) job for
    /// `action`, then reloads the widgets.
    pub(crate) fn start_widget_action(&mut self, action: WidgetAction) -> crate::TaskHandle {
        let job = self.surfaces.widget_action.clone();
        if job.is_none() {
            eprintln!(
                "prism: widget action {:?} tapped, but the app has no on_widget_action handler",
                action.action
            );
        }
        self.spawn(move |cx| async move {
            if let Some(job) = job {
                job(action, cx.clone()).await;
            }
            cx.update(|app| app.reload_widgets());
        })
        .finish_in_background()
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
        let kind = self.kind.clone();
        let mut snap = |e: Option<Element>| {
            e.map(|e| {
                let mut node = snapshot_one(app, e);
                node.tag_actions(&kind);
                node
            })
        };
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

/// Refresh requests for a widget closer together than this share one run
/// of its hook (WidgetKit asks once per instance on screen).
pub const REFRESH_COALESCE: Duration = Duration::from_secs(5);

/// `fut`'s output, or `None` once `timeout` has passed (`fut` is dropped).
async fn with_timeout<F: std::future::Future>(
    cx: &crate::AsyncApp,
    timeout: Duration,
    fut: F,
) -> Option<F::Output> {
    let mut fut = std::pin::pin!(fut);
    let mut sleep = std::pin::pin!(cx.sleep(timeout));
    std::future::poll_fn(|task| {
        if let std::task::Poll::Ready(v) = fut.as_mut().poll(task) {
            return std::task::Poll::Ready(Some(v));
        }
        sleep.as_mut().poll(task).map(|()| None)
    })
    .await
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
    /// Reads the configurations the system shows for a kind (backends:
    /// [`App::set_widget_configurations_source`]).
    pub widget_configurations: Option<WidgetConfigurations>,
    /// What that source said at the last reload, by kind.
    pub widget_configurations_seen: Vec<(String, Vec<WidgetConfig>)>,
    pub requests: Vec<PlatformRequest>,
    pub next_activity: u64,
    pub notification_handler: Option<NotificationHandler>,
    /// Inside [`snapshot`] (nested for snapshots taken while rendering one).
    pub snapshot_depth: u32,
    /// The instant a widget is being drawn for ([`App::now`]).
    pub drawing_at: Option<SystemTime>,
    /// Widgets are drawn only after their refresh hook succeeded
    /// ([`App::set_widgets_refresh_only`]).
    pub widgets_refresh_only: bool,
    /// The kinds whose refresh hook just succeeded, drawn at the next
    /// [`App::take_requests`] in refresh-only mode.
    pub widgets_refreshed: Vec<String>,
    /// The refreshes running, as (kind, task id).
    pub widget_refreshes: Vec<(String, u64)>,
    /// When each kind's last refresh ended.
    pub widget_refreshed_at: Vec<(String, std::time::Instant)>,
    // --- Bundling: what the app declares (`PRISM_DESCRIBE`) ---
    /// [`App::enable_live_activities`] was called.
    pub live_activities: bool,

    // --- Widget actions (#47) ---
    /// [`App::on_widget_action`]'s job.
    pub widget_action: Option<WidgetActionJob>,
    /// Signs the widget actions' links and keeps which ones still work
    /// ([`App::set_widget_key`]).
    pub widget_links: Option<WidgetLinks>,
}

type NotificationHandler = std::rc::Rc<dyn Fn(&NotificationResponse, &mut App)>;
type WidgetConfigurations = Rc<dyn Fn(&str) -> Vec<WidgetConfig>>;

impl App {
    /// What the current platform supports (set by the backend).
    pub fn capabilities(&self) -> Capabilities {
        let mut caps = self.surfaces.capabilities;
        // The app's own store sells wherever it runs.
        caps.purchases |= self.store().is_some();
        caps
    }

    /// Called by backends at startup with what the platform supports.
    pub fn set_capabilities(&mut self, caps: Capabilities) {
        self.surfaces.capabilities = caps;
    }

    /// What the platform supports changed after startup: re-renders
    /// everything if anything differs.
    pub(crate) fn update_capabilities(&mut self, caps: Capabilities) {
        if self.surfaces.capabilities != caps {
            self.surfaces.capabilities = caps;
            self.refresh();
        }
    }

    /// True while a widget, Live Activity or other static surface is being
    /// rendered: nothing there is tappable except links, nothing animates,
    /// and sizes relative to the parent are not kept. Components use it to
    /// draw a static variant (charts draw their bars as one shape).
    pub fn in_snapshot(&self) -> bool {
        self.surfaces.snapshot_depth > 0
    }

    /// The current time, or, while a widget is drawn, the instant it is drawn
    /// for: the reload's time, then each instant of [`Widget::ahead`]. Read
    /// it instead of `SystemTime::now()` in what a widget shows, so a drawing
    /// ahead leaves out what has gone by then.
    pub fn now(&self) -> SystemTime {
        self.surfaces.drawing_at.unwrap_or_else(SystemTime::now)
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

    /// Re-renders every widget from current state (coalesced per event),
    /// in every configuration known for a configurable one.
    pub fn reload_widgets(&mut self) {
        self.surfaces.widgets_dirty = true;
    }

    /// For backends whose widgets run actions by opening the app on a link
    /// (macOS): the app's secret for those links, and the store where
    /// Prism keeps which of them still work. Keep both between runs: a
    /// widget shows its links long after the app drew it, and a tap may
    /// launch the app.
    ///
    /// Each drawing of the widgets signs its buttons' links with `key`, for
    /// that drawing: snapshots carry each action's token (`"t"`), which the
    /// renderer adds to the link. [`Runtime::open_link`](crate::Runtime::open_link)
    /// runs an action only from such a link, **once**, and only while its
    /// drawing may be on screen: the current one, or one the app replaced
    /// less than an hour ago. `ledger` holds those drawings and the links
    /// already used, under one entry (`widget-link-ledger`, a few hundred
    /// bytes). Without a key, no link runs an action. Redraws the widgets.
    ///
    /// # Platforms
    ///
    /// macOS. iOS, visionOS and Android widgets run actions through a
    /// channel only the system can use (an App Intent, a private
    /// broadcast), so their backends set no key.
    pub fn set_widget_key(&mut self, key: [u8; 32], ledger: Box<dyn crate::Storage>) {
        self.surfaces.widget_links = Some(WidgetLinks::new(key, ledger));
        self.surfaces.widgets_dirty = true;
    }

    /// The link that runs `action` once when the app opens on it, as the
    /// widgets' current drawing has it ([`WidgetAction::to_link`] with its
    /// `token`). `None` without a widget key
    /// ([`set_widget_key`](Self::set_widget_key)) or before the widgets
    /// were drawn with it.
    pub fn widget_action_link(&self, action: &WidgetAction) -> Option<String> {
        let signer = self.surfaces.widget_links.as_ref()?.signer()?;
        Some(format!(
            "{}&token={}",
            action.to_link(),
            signer.token(action)
        ))
    }

    /// Whether a link with `token` may run `action` now (it is then
    /// recorded as used).
    pub(crate) fn check_widget_link(
        &mut self,
        action: &WidgetAction,
        token: Option<&str>,
    ) -> LinkCheck {
        let Some(links) = &mut self.surfaces.widget_links else {
            return LinkCheck::Forged;
        };
        let check = links.check(action, token, SystemTime::now());
        match check {
            LinkCheck::Used => eprintln!(
                "prism: widget action {:?} refused: its link was used already",
                action.action
            ),
            LinkCheck::Expired => eprintln!(
                "prism: widget action {:?} refused: its link expired (the widgets were redrawn since)",
                action.action
            ),
            LinkCheck::Run | LinkCheck::Forged => {}
        }
        check
    }

    /// For backends: reads the configurations the system shows (or asked
    /// for) of a configurable widget kind, which the widget extension
    /// records. They are rendered on each reload, after the default one.
    pub fn set_widget_configurations_source(
        &mut self,
        source: impl Fn(&str) -> Vec<WidgetConfig> + 'static,
    ) {
        self.surfaces.widget_configurations = Some(Rc::new(source));
    }

    /// The recorded configurations of `kind` (none without a source).
    fn widget_configurations(&self, kind: &str) -> Vec<WidgetConfig> {
        self.surfaces
            .widget_configurations
            .as_ref()
            .map_or_else(Vec::new, |source| source(kind))
    }

    /// Reloads the widgets when the system shows configurations they were
    /// not rendered for (a widget added or edited while the app was away).
    pub(crate) fn check_widget_configurations(&mut self) {
        let stale = self.surfaces.widgets.iter().any(|w| {
            w.is_configurable() && {
                let now = self.widget_configurations(&w.kind);
                let seen = self
                    .surfaces
                    .widget_configurations_seen
                    .iter()
                    .find(|(k, _)| *k == w.kind)
                    .map(|(_, c)| c);
                seen != Some(&now)
            }
        });
        if stale {
            self.surfaces.widgets_dirty = true;
        }
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

    /// The number on the app's icon (#239), set on its own, without a
    /// notification: unread items read in the app, banners turned off.
    /// `None` (or 0) clears it. The Dock tile on macOS, the home screen on
    /// iOS, iPadOS, tvOS and visionOS (it needs the notification permission,
    /// badges included), the Badging API (`navigator.setAppBadge`) on the
    /// web where the browser has it (installed web apps), an overlay on the
    /// taskbar button on Windows. Nothing on Android, Linux and watchOS,
    /// whose badges come from notifications.
    pub fn set_badge(&mut self, count: Option<u32>) {
        self.surfaces
            .requests
            .push(PlatformRequest::SetBadge(count));
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
        let dirty = std::mem::take(&mut self.surfaces.widgets_dirty);
        let widgets: Vec<Widget> = if self.surfaces.widgets_refresh_only {
            // Only what a refresh hook just brought: every other widget
            // keeps the app's drawing.
            let kinds = std::mem::take(&mut self.surfaces.widgets_refreshed);
            self.surfaces
                .widgets
                .iter()
                .filter(|w| kinds.contains(&w.kind))
                .cloned()
                .collect()
        } else if dirty {
            self.surfaces.widgets.clone()
        } else {
            Vec::new()
        };
        if !widgets.is_empty() {
            // A new drawing: its links replace the previous drawing's (for
            // the widgets it draws, when it draws only those refreshed).
            if let Some(links) = &mut self.surfaces.widget_links {
                if self.surfaces.widgets_refresh_only {
                    let kinds = widgets.iter().map(|w| w.kind.clone()).collect();
                    links.begin_partial_drawing(SystemTime::now(), kinds);
                } else {
                    links.begin_drawing(SystemTime::now());
                }
            }
            let mut seen = Vec::new();
            let snaps = widgets
                .iter()
                .map(|w| {
                    let requested = if w.is_configurable() {
                        self.widget_configurations(&w.kind)
                    } else {
                        Vec::new()
                    };
                    let snap = w.snapshot(self, &requested);
                    seen.push((w.kind.clone(), requested));
                    snap
                })
                .collect();
            let drawn: Vec<&str> = widgets.iter().map(|w| w.kind.as_str()).collect();
            self.surfaces
                .widget_configurations_seen
                .retain(|(k, _)| !drawn.contains(&k.as_str()));
            self.surfaces.widget_configurations_seen.extend(seen);
            self.surfaces
                .requests
                .push(PlatformRequest::ReloadWidgets(snaps));
        }
        if let Some(json) = self.car_update() {
            self.surfaces.requests.push(PlatformRequest::SetCar(json));
        }
        let sheets = self.take_stylesheet_requests();
        self.surfaces.requests.extend(sheets);
        // After the widgets: their drawing may ask what the user owns.
        self.store_watch_request();
        std::mem::take(&mut self.surfaces.requests)
    }

    /// For backends that run the app's widget code without the app (a
    /// widget extension, a runtime started only to refresh a widget): from
    /// now on, a widget is drawn only when its [`refresh`](Widget::refresh)
    /// hook succeeded, whatever [`reload_widgets`](Self::reload_widgets) the
    /// app's code asks for. Such a runtime starts from the app's `init`, not
    /// from the state the app had when it drew its widgets: drawing every
    /// widget from it would replace good drawings with a fresh start's.
    pub fn set_widgets_refresh_only(&mut self, on: bool) {
        self.surfaces.widgets_refresh_only = on;
    }

    /// Starts the [`refresh`](Widget::refresh) hook of widget `kind`, unless
    /// one is running (its task is returned) or ended less than
    /// [`REFRESH_COALESCE`] ago (several instances of a widget ask at once).
    /// `None` when there is nothing to wait for: no such widget, no hook, a
    /// refresh just done.
    pub(crate) fn start_widget_refresh(&mut self, kind: &str) -> Option<u64> {
        let widget = self.surfaces.widgets.iter().find(|w| w.kind == kind)?;
        let hook = widget.refresh.clone()?;
        let timeout = widget.refresh_timeout;
        if let Some((_, task)) = self
            .surfaces
            .widget_refreshes
            .iter()
            .find(|(k, _)| k == kind)
        {
            return Some(*task);
        }
        let recent = self
            .surfaces
            .widget_refreshed_at
            .iter()
            .any(|(k, at)| k == kind && at.elapsed() < REFRESH_COALESCE);
        if recent {
            return None;
        }
        let kind = kind.to_owned();
        let task = {
            let kind = kind.clone();
            self.spawn(move |cx| async move {
                let outcome = with_timeout(&cx, timeout, hook(cx.clone())).await;
                cx.update(|app| app.widget_refresh_ended(&kind, outcome, timeout));
            })
            .finish_in_background()
            .id()
        };
        // A hook that returned at once already ended.
        if self.executor.is_running(task) {
            self.surfaces.widget_refreshes.push((kind, task));
        }
        Some(task)
    }

    /// A refresh hook returned (`Some`) or ran out of time (`None`).
    fn widget_refresh_ended(
        &mut self,
        kind: &str,
        outcome: Option<Result<(), String>>,
        timeout: Duration,
    ) {
        self.surfaces.widget_refreshes.retain(|(k, _)| k != kind);
        self.surfaces.widget_refreshed_at.retain(|(k, _)| k != kind);
        self.surfaces
            .widget_refreshed_at
            .push((kind.to_owned(), std::time::Instant::now()));
        match outcome {
            Some(Ok(())) => {
                if self.surfaces.widgets_refresh_only {
                    if !self.surfaces.widgets_refreshed.iter().any(|k| k == kind) {
                        self.surfaces.widgets_refreshed.push(kind.to_owned());
                    }
                } else {
                    self.surfaces.widgets_dirty = true;
                }
            }
            Some(Err(e)) => {
                eprintln!("prism: widget {kind:?} refresh failed, its drawings stay: {e}")
            }
            None => eprintln!(
                "prism: widget {kind:?} refresh still running after {timeout:?}, given up: its drawings stay"
            ),
        }
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
