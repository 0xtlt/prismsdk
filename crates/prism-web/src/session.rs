//! A live session: one [`Runtime`] per open page, on its own thread, fed by
//! the page's socket messages. Transport-agnostic: [`WebApp::connect`] takes
//! a function that sends text to the page and returns a [`Session`] that
//! receives the page's messages, so any WebSocket implementation works.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use http::{HeaderMap, Uri};
use prism_core::surfaces::{ActivityContent, PlatformRequest};
use prism_core::{Capabilities, NodeId, NodeKind, Patch, PlatformServices, Runtime, UiEvent};
use serde_json::{Value, json};

use crate::WebApp;
use crate::html::AssetUrls;
use crate::{css, glyphs};

enum Inbound {
    /// A text message from the page.
    Message(String),
    /// Async work is ready (a task, a timer from another thread).
    Wake,
    /// The socket closed.
    Close,
}

/// The server side of one open page. Pass it the page's messages; drop it
/// (or call [`close`](Session::close)) when the socket closes.
pub struct Session {
    tx: mpsc::Sender<Inbound>,
}

impl Session {
    /// A text message from the page. Returns `false` once the session ended.
    pub fn receive(&self, text: impl Into<String>) -> bool {
        self.tx.send(Inbound::Message(text.into())).is_ok()
    }

    /// Ends the session (also done on drop).
    pub fn close(&self) {
        let _ = self.tx.send(Inbound::Close);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

impl WebApp {
    /// Starts the live session for a socket opened at `/ws` (`uri` and
    /// `headers` are the upgrade request's). `send` delivers a text message to
    /// the page and returns `false` when the socket is gone.
    ///
    /// The session runs on its own thread (the app's state is not `Send`);
    /// `send` is called from that thread.
    pub fn connect(
        &self,
        uri: &Uri,
        headers: &HeaderMap,
        send: impl Fn(String) -> bool + Send + 'static,
    ) -> Session {
        let (tx, rx) = mpsc::channel();
        let (sid, path, visitor) = self.socket_visitor(uri, headers);
        let app = self.clone();
        let waker = tx.clone();
        std::thread::Builder::new()
            .name("prism-session".into())
            .spawn(move || {
                let mut options = app.options(visitor.with_sid(&sid), path);
                let device = options.device_info.clone();
                options.wake_ui = Some(std::sync::Arc::new(move || {
                    let _ = waker.send(Inbound::Wake);
                }));
                let runtime = Runtime::with_options(&app.inner.init, options);
                Live {
                    device: device.unwrap_or_else(|| runtime.app.device_info()),
                    runtime,
                    encoder: Encoder {
                        asset_urls: Some(app.asset_urls()),
                        ..Default::default()
                    },
                    services: WebServices::default(),
                    send: Box::new(send),
                    app,
                    sid,
                }
                .run(rx);
            })
            .expect("prism-web: cannot start a session thread");
        Session { tx }
    }
}

struct Live {
    app: WebApp,
    sid: String,
    runtime: Runtime,
    encoder: Encoder,
    services: WebServices,
    device: prism_core::DeviceInfo,
    send: Box<dyn Fn(String) -> bool + Send>,
}

impl Live {
    /// Serves the page until it closes.
    fn run(mut self, rx: mpsc::Receiver<Inbound>) {
        let first = self.runtime.render();
        if !self.patches("reset", &first) {
            return;
        }
        loop {
            if !self.flush_requests() {
                return;
            }
            // Sleep until a message, a wake-up, the next timer or the next
            // background task.
            let deadline = [self.runtime.next_timer(), self.services.next_background()]
                .into_iter()
                .flatten()
                .min();
            let inbound = match deadline {
                Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
                None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            };
            let patches = match inbound {
                // Busy pages still get their timers on time.
                Ok(Inbound::Message(text)) if deadline.is_some_and(|at| at <= Instant::now()) => {
                    let mut patches = self.message(&text);
                    patches.extend(self.tick());
                    patches
                }
                Ok(Inbound::Message(text)) => self.message(&text),
                Ok(Inbound::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => self.tick(),
                Ok(Inbound::Close) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            if !self.patches("patch", &patches) {
                return;
            }
        }
    }

    /// Runs due background tasks, timers and ready async work.
    fn tick(&mut self) -> Vec<Patch> {
        for id in self.services.due_background() {
            self.runtime.run_background_task(&id);
        }
        self.runtime.poll()
    }

    fn patches(&mut self, kind: &str, patches: &[Patch]) -> bool {
        patches.is_empty()
            || (self.send)(json!({ "t": kind, "p": self.encoder.encode(patches) }).to_string())
    }

    /// Hands platform requests to the page (notifications, device calls…).
    fn flush_requests(&mut self) -> bool {
        for request in self.runtime.take_requests() {
            self.services.handle(request);
        }
        self.services
            .outbox
            .drain(..)
            .all(|msg| (self.send)(msg.to_string()))
    }

    /// Applies one message from the page; returns the resulting patches.
    fn message(&mut self, text: &str) -> Vec<Patch> {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return Vec::new();
        };
        let rt = &mut self.runtime;
        match v.get("t").and_then(Value::as_str) {
            // A notification or live banner was tapped: open its deep link.
            Some("link") => match v.get("path").and_then(Value::as_str) {
                Some(path) => rt.open_link(path),
                None => Vec::new(),
            },
            // Device answers (location, photos…) and sensor events.
            Some("device_reply") => match v.get("id").and_then(Value::as_u64) {
                Some(id) => {
                    let result = device_result(&self.app, &self.sid, &v);
                    rt.device_reply(id, result)
                }
                None => Vec::new(),
            },
            Some("device_event") => {
                let Some(id) = v.get("id").and_then(Value::as_u64) else {
                    return Vec::new();
                };
                let event = if let Some(err) = v.get("err").and_then(Value::as_str) {
                    prism_core::DeviceEvent::Error(prism_core::DeviceError::from_message(err))
                } else if let Some(l) = v.get("location").and_then(location_from) {
                    prism_core::DeviceEvent::Location(l)
                } else {
                    prism_core::DeviceEvent::Motion(motion_from(&v))
                };
                rt.device_event(id, event)
            }
            // The browser window was resized.
            Some("viewport") => match (
                v.get("w").and_then(Value::as_f64),
                v.get("h").and_then(Value::as_f64),
            ) {
                (Some(w), Some(h)) => rt.set_viewport(w as f32, h as f32),
                _ => Vec::new(),
            },
            // The page described its device (on connect, rotation, fold).
            Some("device") => {
                crate::device_info::apply_report(&mut self.device, &v);
                rt.set_device_info(self.device.clone())
            }
            // The page was hidden or shown again.
            Some("lifecycle") => rt.set_lifecycle(match v.get("state").and_then(Value::as_str) {
                Some("background") => prism_core::Lifecycle::Background,
                Some("inactive") => prism_core::Lifecycle::Inactive,
                _ => prism_core::Lifecycle::Active,
            }),
            // A notification's body or action button was chosen.
            Some("notif") => {
                let Some(id) = v.get("id").and_then(Value::as_str) else {
                    return Vec::new();
                };
                let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
                let response = prism_core::NotificationResponse {
                    id: id.to_string(),
                    action: s("action"),
                    text: s("text"),
                };
                rt.notification_response(response, s("link"))
            }
            _ => match parse_event(&v) {
                Some(event) => rt.dispatch(event),
                None => Vec::new(),
            },
        }
    }
}

fn parse_event(v: &Value) -> Option<UiEvent> {
    let id = v.get("id")?.as_u64()?;
    match v.get("t")?.as_str()? {
        "click" => Some(UiEvent::Click(id)),
        "input" => Some(UiEvent::Input(id, v.get("v")?.as_str()?.to_string())),
        "back" => Some(UiEvent::Back(id, v.get("depth")?.as_u64()? as usize)),
        "change" => Some(UiEvent::Change(id, v.get("v")?.as_f64()?)),
        "menu" => Some(UiEvent::MenuSelect(id)),
        "scroll" => Some(UiEvent::Scroll(
            id,
            v.get("o")?.as_f64()? as f32,
            v.get("v")?.as_f64()? as f32,
        )),
        "swipe" => Some(UiEvent::Swipe(
            id,
            if v.get("dir")?.as_str()? == "left" {
                prism_core::SwipeDirection::Left
            } else {
                prism_core::SwipeDirection::Right
            },
        )),
        "alert" => Some(UiEvent::AlertAction(id, v.get("index")?.as_u64()? as usize)),
        "navigate" => Some(UiEvent::Navigate(id, v.get("path")?.as_str()?.to_string())),
        _ => None,
    }
}

/// Standard base64 (what `FileReader.readAsDataURL` produces).
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in input.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Saves files uploaded by the client; returns what the app sees.
fn save_files(app: &WebApp, sid: &str, files: &[Value]) -> Vec<prism_core::PickedFile> {
    let dir = app.files_dir(sid);
    let _ = std::fs::create_dir_all(&dir);
    files
        .iter()
        .filter_map(|f| {
            let data = base64_decode(f.get("data")?.as_str()?)?;
            let name = f.get("name").and_then(Value::as_str).unwrap_or("file");
            let mime = f
                .get("mime")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream");
            let ext = std::path::Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .filter(|e| e.chars().all(|c| c.is_ascii_alphanumeric()))
                .unwrap_or("bin")
                .to_ascii_lowercase();
            let stored = format!("{}.{ext}", crate::app::new_session_id());
            std::fs::write(dir.join(&stored), &data).ok()?;
            Some(prism_core::PickedFile {
                path: app.file_url(&stored),
                name: name.to_string(),
                mime: mime.to_string(),
                size: data.len() as u64,
            })
        })
        .collect()
}

fn location_from(v: &Value) -> Option<prism_core::Location> {
    let n = |k: &str| v.get(k).and_then(Value::as_f64);
    Some(prism_core::Location {
        latitude: n("latitude")?,
        longitude: n("longitude")?,
        altitude: n("altitude"),
        accuracy: n("accuracy").unwrap_or(0.),
        speed: n("speed"),
        heading: n("heading"),
        timestamp: n("timestamp").unwrap_or(0.),
    })
}

fn motion_from(v: &Value) -> prism_core::Motion {
    let triple = |k: &str| {
        let a = v.get(k).and_then(Value::as_array);
        let at = |i: usize| {
            a.and_then(|a| a.get(i))
                .and_then(Value::as_f64)
                .unwrap_or(0.)
        };
        [at(0), at(1), at(2)]
    };
    prism_core::Motion {
        acceleration: triple("acceleration"),
        gravity: triple("gravity"),
        rotation_rate: triple("rotation_rate"),
        attitude: triple("attitude"),
    }
}

/// A `device_reply` from the client.
fn device_result(
    app: &WebApp,
    sid: &str,
    v: &Value,
) -> Result<prism_core::DeviceValue, prism_core::DeviceError> {
    use prism_core::DeviceValue;
    if let Some(err) = v.get("err").and_then(Value::as_str) {
        return Err(prism_core::DeviceError::from_message(err));
    }
    if let Some(p) = v.get("permission").and_then(Value::as_str) {
        return Ok(DeviceValue::Permission(
            prism_core::PermissionState::from_name(p),
        ));
    }
    if let Some(l) = v.get("location").and_then(location_from) {
        return Ok(DeviceValue::Location(l));
    }
    if let Some(files) = v.get("files").and_then(Value::as_array) {
        return Ok(DeviceValue::Files(save_files(app, sid, files)));
    }
    if let Some(t) = v.get("text") {
        return Ok(DeviceValue::Text(t.as_str().map(str::to_string)));
    }
    Ok(DeviceValue::Done)
}

/// Web implementation of system surfaces: browser notifications (with an
/// in-page toast fallback) and Live Activities as an in-page live banner.
/// Widgets do not exist on the web.
#[derive(Default)]
pub(crate) struct WebServices {
    outbox: Vec<Value>,
    /// Periodic background tasks: (id, next run, interval). On the web they
    /// run in each open page's session.
    background: Vec<(String, Instant, Duration)>,
}

/// `PRISM_BACKGROUND_INTERVAL=<seconds>` shortens every background task's
/// interval (development).
fn background_interval(every: Duration) -> Duration {
    std::env::var("PRISM_BACKGROUND_INTERVAL")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .map_or(every, Duration::from_secs_f64)
}

impl WebServices {
    /// Background tasks due now (removed until they reschedule themselves).
    fn due_background(&mut self) -> Vec<String> {
        let now = Instant::now();
        let (due, later): (Vec<_>, Vec<_>) =
            self.background.drain(..).partition(|(_, at, _)| *at <= now);
        self.background = later;
        due.into_iter().map(|(id, _, _)| id).collect()
    }

    fn next_background(&self) -> Option<Instant> {
        self.background.iter().map(|(_, at, _)| *at).min()
    }
}

fn activity_json(content: &ActivityContent) -> Value {
    serde_json::from_str(&content.to_json()).unwrap_or(Value::Null)
}

impl PlatformServices for WebServices {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            car: false,
            widgets: false,
            live_activities: true,
            dynamic_island: false,
            notifications: true,
            tray: false,
            app_menu: false,
            // The browser decides; calls fail with "unavailable" otherwise.
            location: true,
            motion: true,
            camera: true,
            photos: true,
            haptics: true,
            share: true,
            clipboard: true,
            // Cloud storage is the server's, per browser.
            cloud_sync: false,
        }
    }

    fn handle(&mut self, request: PlatformRequest) {
        let msg = match request {
            PlatformRequest::ReloadWidgets(_) => return,
            PlatformRequest::StartActivity { id, content, .. }
            | PlatformRequest::UpdateActivity { id, content } => {
                json!({ "t": "activity", "id": id.0, "c": activity_json(&content) })
            }
            PlatformRequest::EndActivity {
                id,
                content,
                immediately,
            } => json!({
                "t": "activity_end", "id": id.0, "immediately": immediately,
                "c": content.as_ref().map(activity_json),
            }),
            PlatformRequest::Notify(n) => json!({
                "t": "notify", "id": n.id, "title": n.title, "subtitle": n.subtitle,
                "body": n.body, "link": n.link,
                "delay": n.delay.map(|d| d.as_millis() as u64),
                "actions": n.actions.iter().map(|a| json!({
                    "id": a.id, "title": a.title, "destructive": a.destructive,
                    "reply": a.reply, "link": a.link,
                })).collect::<Vec<_>>(),
            }),
            PlatformRequest::CancelNotification(id) => json!({ "t": "notify_cancel", "id": id }),
            PlatformRequest::RequestNotificationPermission => json!({ "t": "notify_permission" }),
            PlatformRequest::Device { id, call } => device_json(id, call),
            PlatformRequest::ScheduleBackgroundTask(task) => {
                let every = background_interval(task.every);
                self.background.retain(|(id, _, _)| *id != task.id);
                self.background
                    .push((task.id, Instant::now() + every, every));
                return;
            }
            PlatformRequest::ShowAlert(a) => json!({
                "t": "alert",
                "a": serde_json::from_str::<Value>(&a.to_json()).unwrap_or(Value::Null),
            }),
            // No tray or menu bar in a browser tab (capabilities say so).
            PlatformRequest::SetTray(_)
            | PlatformRequest::RemoveTray(_)
            | PlatformRequest::SetAppMenu(_)
            | PlatformRequest::SetCar(_) => return,
        };
        self.outbox.push(msg);
    }
}

/// Encodes patches to the JSON wire format. Styles are pre-rendered to CSS on
/// the server, which needs each node's kind, so the encoder remembers them.
#[derive(Default)]
pub struct Encoder {
    kinds: HashMap<NodeId, NodeKind>,
    /// Hashed asset URLs for pictures (`/assets/<name>` if unset).
    pub asset_urls: Option<AssetUrls>,
}

impl Encoder {
    pub fn encode(&mut self, patches: &[Patch]) -> Vec<Value> {
        patches.iter().map(|p| self.encode_one(p)).collect()
    }

    fn encode_one(&mut self, p: &Patch) -> Value {
        match p {
            Patch::Create { id, kind } => {
                self.kinds.insert(*id, *kind);
                json!({ "op": "create", "id": id, "kind": kind.name() })
            }
            Patch::SetStyle { id, style } => {
                let kind = self.kinds.get(id).copied().unwrap_or(NodeKind::View);
                json!({ "op": "style", "id": id, "css": css(kind, style) })
            }
            Patch::SetText { id, text } => {
                let text = match self.kinds.get(id) {
                    Some(NodeKind::Picture) => match &self.asset_urls {
                        Some(urls) => urls(text),
                        None => crate::html::picture_url(text),
                    },
                    _ => text.clone(),
                };
                json!({ "op": "text", "id": id, "text": text })
            }
            Patch::SetPlaceholder { id, text } => {
                json!({ "op": "placeholder", "id": id, "text": text })
            }
            Patch::SetClickable { id, clickable } => {
                json!({ "op": "clickable", "id": id, "on": clickable })
            }
            Patch::SetChildren { id, children } => {
                json!({ "op": "children", "id": id, "children": children })
            }
            Patch::SetAppearance { appearance } => {
                json!({ "op": "appearance", "value": match appearance {
                    prism_core::Appearance::System => "light dark",
                    prism_core::Appearance::Light => "light",
                    prism_core::Appearance::Dark => "dark",
                }})
            }
            Patch::SetValue { id, value } => json!({ "op": "value", "id": id, "value": value }),
            Patch::SetEnvironment { env } => json!({
                "op": "env",
                "lang": env.lang,
                "dir": if env.direction == prism_core::Direction::Rtl { "rtl" } else { "ltr" },
                "back": env.back,
                "close": env.close,
            }),
            Patch::SetSwipe { id, left, right } => {
                json!({ "op": "swipe", "id": id, "left": left, "right": right })
            }
            Patch::SetScrollEvents { id, on } => json!({ "op": "scrollev", "id": id, "on": on }),
            Patch::SetMenu { id, menu } => json!({
                "op": "menu", "id": id,
                "menu": menu.as_ref().map(|m| serde_json::from_str::<Value>(&m.to_json()).unwrap_or(Value::Null)),
            }),
            Patch::SetIcon { id, icon } => {
                json!({ "op": "icon", "id": id, "icon": icon, "glyph": glyphs::glyph(icon) })
            }
            Patch::SetAccessibility { id, a11y } => {
                let level = match a11y.role {
                    Some(prism_core::Role::Heading(l)) => Some(l),
                    _ => None,
                };
                json!({ "op": "a11y", "id": id, "a11y": {
                    "role": a11y.role.map(|r| r.aria()),
                    "level": level,
                    "label": a11y.label,
                    "hint": a11y.hint,
                    "value": a11y.value,
                    "checked": a11y.checked,
                    "disabled": a11y.disabled,
                    "hidden": a11y.hidden,
                }})
            }
            Patch::SetRoot { id } => json!({ "op": "root", "id": id }),
            Patch::Destroy { id } => {
                self.kinds.remove(id);
                json!({ "op": "destroy", "id": id })
            }
        }
    }
}

/// A device call for the JS client.
fn device_json(id: u64, call: prism_core::DeviceCall) -> Value {
    use prism_core::DeviceCall as C;
    let accuracy = |a: prism_core::Accuracy| match a {
        prism_core::Accuracy::Coarse => "coarse",
        prism_core::Accuracy::Balanced => "balanced",
        prism_core::Accuracy::Best => "best",
    };
    let body = match call {
        C::PermissionStatus(p) => json!({ "call": "permission_status", "permission": p.name() }),
        C::RequestPermission(p) => json!({ "call": "request_permission", "permission": p.name() }),
        C::CurrentLocation(a) => json!({ "call": "location", "accuracy": accuracy(a) }),
        C::WatchLocation(a) => json!({ "call": "watch_location", "accuracy": accuracy(a) }),
        C::WatchMotion(every) => {
            json!({ "call": "watch_motion", "interval": every.as_millis() as u64 })
        }
        C::StopWatch => json!({ "call": "stop" }),
        C::PickImages(o) => json!({ "call": "pick_images", "limit": o.limit, "camera": o.camera }),
        C::ReadClipboard => json!({ "call": "paste" }),
        C::Haptic(h) => json!({ "call": "haptic", "kind": h.name() }),
        C::CopyText(t) => json!({ "call": "copy", "text": t }),
        C::Share(s) => json!({ "call": "share", "text": s.text, "url": s.url }),
        C::OpenUrl(u) => json!({ "call": "open_url", "url": u }),
    };
    let mut body = body;
    body["t"] = json!("device");
    body["id"] = json!(id);
    body
}
