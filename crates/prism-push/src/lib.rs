//! # prism-push
//!
//! Sends push notifications to the devices of a Prism app, from your server:
//!
//! ```no_run
//! # async fn demo(json_from_the_app: &str) -> Result<(), prism_push::PushError> {
//! use prism_push::{Push, Sender, Subscription, apns::ApnsKey, fcm::ServiceAccount, webpush::VapidKey};
//!
//! let sender = Sender::new()
//!     .apns(ApnsKey::from_p8(&std::fs::read_to_string("AuthKey_ABC123.p8")?, "ABC123", "TEAM123456")?)
//!     .fcm(ServiceAccount::from_json(&std::fs::read_to_string("service-account.json")?)?)
//!     .vapid(VapidKey::from_base64(&std::env::var("VAPID_PRIVATE_KEY").unwrap())?, "mailto:ops@example.com");
//!
//! // What the app's `on_push_token` gave (`PushSubscription::to_json`).
//! let sub = Subscription::from_json(json_from_the_app)?;
//! match sender.send(&sub, &Push::new().title("Order shipped").body("It arrives tomorrow").link("/orders/42")).await {
//!     Err(prism_push::PushError::Gone) => { /* delete `sub`: the app was removed or the token changed */ }
//!     other => other?,
//! }
//! # Ok(()) }
//! ```
//!
//! | Service | Endpoint | Authentication | Module |
//! |---|---|---|---|
//! | APNs (iOS, iPadOS, macOS, tvOS, visionOS, watchOS) | `api.push.apple.com` (HTTP/2) | `.p8` key → ES256 JWT | [`apns`] |
//! | FCM (Android) | `fcm.googleapis.com/v1` | service account → RS256 assertion → OAuth token | [`fcm`] |
//! | Web Push (browsers) and UnifiedPush (Android, Linux) | the subscription's endpoint | VAPID (RFC 8292), payload encrypted with RFC 8291 | [`webpush`] |
//!
//! Every module also builds plain [`HttpRequest`]s, for servers that have
//! their own HTTP client. Without the default `send` feature the crate is
//! pure Rust (no TLS, no ring): Prism's UnifiedPush receivers use it to
//! decrypt messages on the device.

use std::time::{Duration, SystemTime};

use serde::Deserialize;
use serde_json::{Map, Value};

pub mod apns;
pub mod fcm;
mod jwt;
pub mod webpush;

#[cfg(feature = "send")]
mod sender;
#[cfg(feature = "send")]
pub use sender::Sender;

/// Why a push was not sent.
#[derive(Debug)]
pub enum PushError {
    /// The subscription is no longer valid (APNs 410 `Unregistered`, FCM
    /// `UNREGISTERED`, Web Push 404/410): delete it.
    Gone,
    /// The service refused the push.
    Rejected { status: u16, reason: String },
    /// A key, a subscription or a configuration is missing or invalid.
    Config(String),
    /// A network or HTTP error.
    Http(String),
}

impl std::fmt::Display for PushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PushError::Gone => f.write_str("the subscription is no longer valid"),
            PushError::Rejected { status, reason } => write!(f, "rejected ({status}): {reason}"),
            PushError::Config(m) => write!(f, "configuration: {m}"),
            PushError::Http(m) => write!(f, "http: {m}"),
        }
    }
}

impl std::error::Error for PushError {}

impl From<std::io::Error> for PushError {
    fn from(e: std::io::Error) -> Self {
        PushError::Config(e.to_string())
    }
}

/// The push service of a subscription.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Apns,
    Fcm,
    #[serde(rename = "webpush")]
    WebPush,
    #[serde(rename = "unifiedpush")]
    UnifiedPush,
}

/// The Web Push keys of a subscription (base64url).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Keys {
    pub p256dh: String,
    pub auth: String,
}

/// Where to send pushes for one device: the JSON a Prism app reports with
/// `on_push_token` (`PushSubscription::to_json`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Subscription {
    pub service: Service,
    /// APNs device token (hex), FCM registration token, or the Web Push /
    /// UnifiedPush endpoint.
    #[serde(alias = "endpoint")]
    pub token: String,
    #[serde(default)]
    pub keys: Option<Keys>,
    /// APNs: the app's bundle id.
    #[serde(default)]
    pub topic: Option<String>,
    /// APNs: a development build (sandbox environment).
    #[serde(default)]
    pub sandbox: bool,
}

impl Subscription {
    pub fn from_json(json: &str) -> Result<Self, PushError> {
        serde_json::from_str(json).map_err(|e| PushError::Config(format!("subscription: {e}")))
    }
}

/// How urgently to deliver (APNs priority, FCM priority, Web Push `Urgency`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Urgency {
    VeryLow,
    Low,
    #[default]
    Normal,
    /// Wakes the device now (APNs priority 10, FCM `high`).
    High,
}

/// A Live Activity push (APNs only, iOS 16.1+).
#[derive(Clone, Debug, PartialEq)]
pub struct LiveActivity {
    /// `update`, `end` or `start` (push-to-start, iOS 17.2+).
    pub event: &'static str,
    /// `{"json":"…"}`: `prism_core::Activity::content_state_json`.
    pub content_state: Value,
    /// For `start`: the activity's kind (Prism's `PrismActivityAttributes`).
    pub kind: Option<String>,
    /// For `start`: the id the app reports the activity's own push token
    /// with (`ActivityPushToken::activity`, `ActivityId(id)`).
    pub id: Option<u64>,
    pub stale_date: Option<SystemTime>,
    pub dismissal_date: Option<SystemTime>,
}

/// What to send. The same push works on every service; fields a service
/// lacks are left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Push {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub body: Option<String>,
    /// A deep link in the app, opened when the notification is tapped.
    pub link: Option<String>,
    /// Custom values, delivered in `PushMessage::data`.
    pub data: Map<String, Value>,
    pub badge: Option<u32>,
    /// A sound name (`"default"` when the push has a title or body).
    pub sound: Option<String>,
    /// Data only: wakes the app in the background without showing anything
    /// (APNs `content-available`, an FCM data message). Browsers always show
    /// something, so Web Push pushes should stay visible.
    pub silent: bool,
    /// Replaces an earlier push with the same id (APNs collapse id, FCM
    /// collapse key, Web Push `Topic`).
    pub collapse_id: Option<String>,
    /// How long the service keeps it for an offline device (default 4 weeks
    /// on APNs and FCM; Web Push requires one: 1 day by default).
    pub ttl: Option<Duration>,
    pub urgency: Urgency,
    /// Groups notifications (APNs `thread-id`, Android notification tag).
    pub thread: Option<String>,
    pub live_activity: Option<LiveActivity>,
}

impl Push {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }
    pub fn subtitle(mut self, t: impl Into<String>) -> Self {
        self.subtitle = Some(t.into());
        self
    }
    pub fn body(mut self, t: impl Into<String>) -> Self {
        self.body = Some(t.into());
        self
    }
    pub fn link(mut self, path: impl Into<String>) -> Self {
        self.link = Some(path.into());
        self
    }
    pub fn data(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.data.insert(key.into(), value.into());
        self
    }
    pub fn badge(mut self, n: u32) -> Self {
        self.badge = Some(n);
        self
    }
    pub fn sound(mut self, name: impl Into<String>) -> Self {
        self.sound = Some(name.into());
        self
    }
    /// A silent, data-only push for `on_background_push`.
    pub fn silent(mut self) -> Self {
        self.silent = true;
        self
    }
    pub fn collapse_id(mut self, id: impl Into<String>) -> Self {
        self.collapse_id = Some(id.into());
        self
    }
    pub fn ttl(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }
    pub fn urgency(mut self, u: Urgency) -> Self {
        self.urgency = u;
        self
    }
    pub fn thread(mut self, t: impl Into<String>) -> Self {
        self.thread = Some(t.into());
        self
    }

    /// Updates (or ends) a Live Activity with new content: send it to the
    /// activity's push token (`on_activity_push_token`).
    pub fn live_activity(mut self, content_state_json: &str, end: bool) -> Self {
        self.live_activity = Some(LiveActivity {
            event: if end { "end" } else { "update" },
            content_state: serde_json::from_str(content_state_json).unwrap_or(Value::Null),
            kind: None,
            id: None,
            stale_date: None,
            dismissal_date: None,
        });
        self
    }

    /// Starts a Live Activity of `kind` remotely (push-to-start token,
    /// iOS 17.2+). Set `title`/`body` for the alert it shows.
    ///
    /// `id` (non-zero) names the activity: the app reports the activity's
    /// own push token, for updating and ending it, with
    /// `ActivityPushToken::activity` = `Some(ActivityId(id))`, and can
    /// update or end it locally with that id. Pick ids the app's own
    /// activities will not use (they count from 1 at each launch), such as
    /// a timestamp or a random number.
    pub fn start_live_activity(
        mut self,
        kind: impl Into<String>,
        id: u64,
        content_state_json: &str,
    ) -> Self {
        self.live_activity = Some(LiveActivity {
            event: "start",
            content_state: serde_json::from_str(content_state_json).unwrap_or(Value::Null),
            kind: Some(kind.into()),
            id: Some(id),
            stale_date: None,
            dismissal_date: None,
        });
        self
    }

    /// The Prism payload (Web Push, UnifiedPush): what the app's
    /// `PushMessage::from_payload` reads.
    pub fn payload(&self) -> Value {
        let mut o = Map::new();
        for (k, v) in [
            ("title", &self.title),
            ("subtitle", &self.subtitle),
            ("body", &self.body),
            ("link", &self.link),
            ("id", &self.collapse_id),
        ] {
            if let Some(v) = v {
                o.insert(k.into(), Value::String(v.clone()));
            }
        }
        if self.silent {
            o.insert("silent".into(), Value::Bool(true));
        }
        if let Some(b) = self.badge {
            o.insert("badge".into(), b.into());
        }
        if !self.data.is_empty() {
            o.insert("data".into(), Value::Object(self.data.clone()));
        }
        Value::Object(o)
    }
}

/// A request to send with any HTTP client (APNs needs HTTP/2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub(crate) fn unix(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// base64url without padding (JWT, Web Push keys).
pub(crate) fn b64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// Decodes base64url or standard base64, with or without padding.
pub(crate) fn unb64(s: &str) -> Result<Vec<u8>, PushError> {
    use base64::Engine;
    let s: String = s
        .trim()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|e| PushError::Config(format!("base64: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_app_subscriptions() {
        // prism-core's PushSubscription::to_json.
        let s = Subscription::from_json(
            r#"{"service":"apns","token":"a1b2","topic":"dev.prism.counter","sandbox":true}"#,
        )
        .unwrap();
        assert_eq!(s.service, Service::Apns);
        assert!(s.sandbox);
        // Web Push and UnifiedPush: a browser's subscription JSON (endpoint +
        // keys), which is also what the app reports.
        let b = Subscription::from_json(
            r#"{"service":"webpush","endpoint":"https://p.example/1","expirationTime":null,"keys":{"p256dh":"BCV","auth":"BTB"}}"#,
        )
        .unwrap();
        assert_eq!(b.token, "https://p.example/1");
        assert_eq!(b.keys.unwrap().auth, "BTB");
    }

    #[test]
    fn payload_has_prism_keys() {
        let p = Push::new()
            .title("Hi")
            .body("There")
            .link("/x")
            .data("n", 1)
            .payload();
        assert_eq!(
            p,
            serde_json::json!({"title": "Hi", "body": "There", "link": "/x", "data": {"n": 1}})
        );
    }
}
