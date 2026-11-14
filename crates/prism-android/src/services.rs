//! Android system surfaces. Calls go to the Java host as JSON ops (it uses
//! the platform APIs: LocationManager, SensorManager, the photo picker,
//! camera intents, Vibrator, ClipboardManager, ACTION_SEND, NotificationManager,
//! AlertDialog, JobScheduler, AppWidgetManager); answers come back as events.

use prism_core::surfaces::PlatformRequest;
use prism_core::{Capabilities, PlatformServices, Runtime};
use prism_native::remote::device;
use serde_json::{Value, json};

pub(crate) struct AndroidServices {
    caps: Capabilities,
    /// Changes for the host.
    pub ops: Vec<Value>,
    /// Background tasks while the app runs: id → (next run, interval).
    background: Vec<(String, std::time::Instant, std::time::Duration)>,
}

impl AndroidServices {
    /// `info`: what the host found (sensors, camera…).
    pub fn new(info: &Value) -> Self {
        let has = |k: &str| info.get(k).and_then(Value::as_bool).unwrap_or(false);
        AndroidServices {
            caps: Capabilities {
                car: has("car"),
                widgets: true,
                notifications: true,
                live_activities: true,
                location: has("location"),
                motion: has("gyroscope"),
                camera: has("camera"),
                photos: true,
                haptics: has("vibrator"),
                share: true,
                clipboard: true,
                ..Default::default()
            },
            ops: Vec::new(),
            background: Vec::new(),
        }
    }

    /// Background tasks due now (removed until they reschedule).
    pub fn due_background(&mut self) -> Vec<String> {
        let now = std::time::Instant::now();
        let (due, later): (Vec<_>, Vec<_>) =
            self.background.drain(..).partition(|(_, at, _)| *at <= now);
        self.background = later;
        due.into_iter().map(|(id, _, _)| id).collect()
    }
}

impl PlatformServices for AndroidServices {
    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    fn handle(&mut self, request: PlatformRequest) {
        let op = match request {
            PlatformRequest::Device { id, call } => device::call(id, call),
            PlatformRequest::Notify(n) => json!({
                "op": "notify",
                "n": serde_json::from_str::<Value>(&n.to_json()).unwrap_or_default(),
            }),
            PlatformRequest::CancelNotification(id) => json!({ "op": "notify_cancel", "nid": id }),
            PlatformRequest::RequestNotificationPermission => json!({ "op": "notify_permission" }),
            PlatformRequest::ShowAlert(a) => json!({
                "op": "alert",
                "alert": serde_json::from_str::<Value>(&a.to_json()).unwrap_or_default(),
            }),
            PlatformRequest::ScheduleBackgroundTask(task) => {
                let every = std::env::var("PRISM_BACKGROUND_INTERVAL")
                    .ok()
                    .and_then(|s| s.parse::<f64>().ok())
                    .map_or(task.every, std::time::Duration::from_secs_f64);
                // Closed or not, the system runs it (JobScheduler, PrismJob.java).
                let (network, power) = match task.kind {
                    prism_core::BackgroundKind::Refresh => (false, false),
                    prism_core::BackgroundKind::Processing {
                        requires_network,
                        requires_power,
                    } => (requires_network, requires_power),
                };
                self.ops.push(json!({
                    "op": "bg_schedule",
                    "task": task.id,
                    "every": every.as_secs_f64(),
                    "network": network,
                    "power": power,
                }));
                // While the app is open, at the interval itself.
                self.background.retain(|(id, _, _)| *id != task.id);
                self.background
                    .push((task.id, std::time::Instant::now() + every, every));
                crate::jni::post(every.as_millis() as i64);
                return;
            }
            // Live Activities: an ongoing notification drawing the lock-screen
            // content (PrismSurface.java), its timers counting natively.
            PlatformRequest::StartActivity { id, kind, content } => json!({
                "op": "activity", "aid": id.0.to_string(), "kind": kind,
                "content": surface(&content.to_json()),
            }),
            PlatformRequest::UpdateActivity { id, content } => json!({
                "op": "activity", "aid": id.0.to_string(), "content": surface(&content.to_json()),
            }),
            PlatformRequest::EndActivity {
                id,
                content,
                immediately,
            } => json!({
                "op": "activity_end", "aid": id.0.to_string(), "immediately": immediately,
                "content": content.map(|c| surface(&c.to_json())),
            }),
            // Home-screen widgets (PrismWidget.java) keep the latest snapshots.
            PlatformRequest::ReloadWidgets(snaps) => json!({
                "op": "widgets",
                "snaps": snaps.iter().map(|s| surface(&s.to_json())).collect::<Vec<_>>(),
            }),
            // Android Auto / Automotive templates (PrismCarService.java).
            PlatformRequest::SetCar(json) => json!({ "op": "car", "json": json }),
            // Trays and menu bars are desktop surfaces.
            PlatformRequest::SetTray(_)
            | PlatformRequest::RemoveTray(_)
            | PlatformRequest::SetAppMenu(_) => return,
        };
        self.ops.push(op);
    }
}

/// Answers from the host: device replies and events, notification taps.
pub(crate) fn event(runtime: &mut Runtime, v: &Value) -> Vec<prism_core::Patch> {
    let id = v.get("id").and_then(Value::as_u64);
    match (v.get("t").and_then(Value::as_str), id) {
        (Some("device_reply"), Some(id)) => runtime.device_reply(id, device::reply(v)),
        (Some("device_event"), Some(id)) => runtime.device_event(id, device::event(v)),
        (Some("notif"), _) => {
            let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
            let Some(nid) = s("nid") else {
                return Vec::new();
            };
            runtime.notification_response(
                prism_core::NotificationResponse {
                    id: nid,
                    action: s("action"),
                    text: s("text"),
                },
                s("link"),
            )
        }
        _ => Vec::new(),
    }
}

/// A surface snapshot (`SNode` JSON) with each icon's glyph (`"gl"`): the host
/// draws icons as text.
fn surface(json: &str) -> Value {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(map) => {
                if map.get("k").and_then(Value::as_str) == Some("i")
                    && let Some(icon) = map.get("s").and_then(Value::as_str)
                {
                    let glyph = prism_core::glyphs::glyph(icon).to_string();
                    map.insert("gl".into(), Value::String(glyph));
                }
                map.values_mut().for_each(walk);
            }
            Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    let mut v = serde_json::from_str(json).unwrap_or(Value::Null);
    walk(&mut v);
    v
}
