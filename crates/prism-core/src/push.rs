//! Remote push notifications: a device token for your server, and the
//! pushes it sends.
//!
//! ```ignore
//! // At startup: the app may have been launched by a push.
//! cx.on_push_token(|sub, app| upload(app, sub.to_json()));      // first token and every refresh
//! cx.on_push(|push, app| match push.event {
//!     PushEvent::Received => app.refresh(),                         // while the app is open
//!     PushEvent::Opened => {}                                       // tapped (its `link` is already open)
//!     PushEvent::Background => {}                                   // a silent push (data only)
//! });
//! cx.on_background_push(|push, cx| async move { sync(&cx).await }); // the system waits for it
//!
//! cx.spawn(async move |_, cx| {
//!     let sub = cx.register_for_push().await?;   // asks for notification permission first
//!     // sub.service: Apns / Fcm / WebPush / UnifiedPush; send sub.to_json() to your server,
//!     // which sends pushes with the `prism-push` crate.
//! });
//! ```
//!
//! | | Service | Token | Silent pushes |
//! |---|---|---|---|
//! | iOS, iPadOS, macOS, tvOS, visionOS, watchOS | APNs | device token (hex) | `content-available` |
//! | Android | FCM (opt-in, `[push] fcm_*` in Prism.toml) or UnifiedPush | registration token / endpoint | data messages |
//! | Web | Web Push (VAPID) | endpoint + keys | – (browsers always show) |
//! | Linux | UnifiedPush over D-Bus, when a distributor runs | endpoint + keys | yes |
//! | Windows | none: WNS needs package identity (`Unavailable`) | – | – |
//!
//! Payloads sent with `prism-push` use the same keys everywhere: `title`,
//! `subtitle`, `body`, `link` (a deep link opened on tap), and custom `data`.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use crate::app::App;
use crate::device::DeviceError;
use crate::surfaces::{ActivityId, json_str};
use crate::task::AsyncApp;

/// The push service behind a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PushService {
    /// Apple Push Notification service.
    Apns,
    /// Firebase Cloud Messaging (Android with Google Play services).
    Fcm,
    /// The browser's push service (RFC 8030), encrypted (RFC 8291) and
    /// authenticated with VAPID (RFC 8292).
    WebPush,
    /// An open standard with user-chosen distributors (ntfy, NextPush,
    /// KUnifiedPush…); messages are Web Push messages.
    UnifiedPush,
}

impl PushService {
    /// The lowercase identifier used in the subscription JSON (`apns`, `fcm`, `webpush`, `unifiedpush`).
    pub fn name(self) -> &'static str {
        match self {
            PushService::Apns => "apns",
            PushService::Fcm => "fcm",
            PushService::WebPush => "webpush",
            PushService::UnifiedPush => "unifiedpush",
        }
    }

    /// The service called `name`, if there is one (the inverse of [`name`](Self::name)).
    pub fn from_name(name: &str) -> Option<Self> {
        [
            PushService::Apns,
            PushService::Fcm,
            PushService::WebPush,
            PushService::UnifiedPush,
        ]
        .into_iter()
        .find(|s| s.name() == name)
    }
}

/// Where your server sends pushes for this device: give it
/// [`to_json`](Self::to_json) (`prism_push::Subscription` reads it).
#[derive(Clone, PartialEq, Eq)]
pub struct PushSubscription {
    /// The push service the token belongs to.
    pub service: PushService,
    /// APNs: the device token (hex). FCM: the registration token. Web Push
    /// and UnifiedPush: the endpoint URL.
    pub token: String,
    /// Web Push and UnifiedPush: the device's P-256 public key (base64url,
    /// uncompressed), for RFC 8291 encryption.
    pub p256dh: Option<String>,
    /// Web Push and UnifiedPush: the 16-byte authentication secret (base64url).
    pub auth: Option<String>,
    /// APNs: the topic (bundle id) the token belongs to.
    pub topic: Option<String>,
    /// APNs: a development token (`api.sandbox.push.apple.com`).
    pub sandbox: bool,
}

/// The token (an endpoint, for Web Push) and the secret show hidden (S34).
impl std::fmt::Debug for PushSubscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use crate::security::Hidden;
        f.debug_struct("PushSubscription")
            .field("service", &self.service)
            .field("token", &Hidden::text(&self.token))
            .field("p256dh", &self.p256dh)
            .field("auth", &Hidden::opt(self.auth.as_deref()))
            .field("topic", &self.topic)
            .field("sandbox", &self.sandbox)
            .finish()
    }
}

/// The token shows hidden (S34).
impl std::fmt::Debug for ActivityPushToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivityPushToken")
            .field("activity", &self.activity)
            .field("kind", &self.kind)
            .field("token", &crate::security::Hidden::text(&self.token))
            .finish()
    }
}

impl PushSubscription {
    /// A subscription with only a service and a token; the other fields are unset.
    pub fn new(service: PushService, token: impl Into<String>) -> Self {
        PushSubscription {
            service,
            token: token.into(),
            p256dh: None,
            auth: None,
            topic: None,
            sandbox: false,
        }
    }

    /// `{"service":"apns","token":"…","topic":"…","sandbox":true}`, or
    /// `{"service":"webpush","endpoint":"…","keys":{"p256dh":…,"auth":…}}`
    /// for Web Push and UnifiedPush (a browser's `PushSubscription.toJSON()`
    /// plus `service`).
    pub fn to_json(&self) -> String {
        let mut o = match (&self.p256dh, &self.auth) {
            (Some(p), Some(a)) => format!(
                r#"{{"service":"{}","endpoint":{},"keys":{{"p256dh":{},"auth":{}}}"#,
                self.service.name(),
                json_str(&self.token),
                json_str(p),
                json_str(a)
            ),
            _ => format!(
                r#"{{"service":"{}","token":{}"#,
                self.service.name(),
                json_str(&self.token)
            ),
        };
        if let Some(t) = &self.topic {
            o += &format!(r#","topic":{}"#, json_str(t));
        }
        if self.service == PushService::Apns {
            o += &format!(r#","sandbox":{}"#, self.sandbox);
        }
        o.push('}');
        o
    }

    /// Reads [`to_json`](Self::to_json) (and a browser's subscription JSON,
    /// with `service` added).
    pub fn from_json(json: &str) -> Option<Self> {
        let v = Json::parse(json)?;
        let service = PushService::from_name(v.get("service")?.as_str()?)?;
        let keys = v.get("keys");
        let key = |k: &str| keys.and_then(|o| o.get(k)).and_then(Json::as_str);
        Some(PushSubscription {
            service,
            token: v
                .get("token")
                .or_else(|| v.get("endpoint"))?
                .as_str()?
                .to_string(),
            p256dh: key("p256dh").map(str::to_string),
            auth: key("auth").map(str::to_string),
            topic: v.get("topic").and_then(Json::as_str).map(str::to_string),
            sandbox: v.get("sandbox").and_then(Json::as_bool).unwrap_or(false),
        })
    }
}

/// When a push reached the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushEvent {
    /// Arrived while the app is open (on iOS the banner shows too).
    Received,
    /// The user tapped the notification (or one of its actions).
    Opened,
    /// A silent push (`content-available`, a data message): the app may be
    /// running in the background, without a window. See
    /// [`App::on_background_push`].
    Background,
}

/// A push notification the app received.
#[derive(Clone, Debug, PartialEq)]
pub struct PushMessage {
    /// How the push reached the app.
    pub event: PushEvent,
    /// The service that delivered it, when the backend knows.
    pub service: Option<PushService>,
    /// The service's message id (`apns-id`, FCM message id), if any.
    pub id: Option<String>,
    /// The notification's title.
    pub title: Option<String>,
    /// The notification's subtitle.
    pub subtitle: Option<String>,
    /// The notification's body text.
    pub body: Option<String>,
    /// A deep link in the app (`/orders/42`): opened when the push is tapped.
    pub link: Option<String>,
    /// Opened with an action button: its id (and typed text for a reply).
    pub action: Option<String>,
    /// The text typed for a reply action.
    pub text: Option<String>,
    /// Custom keys: strings as-is, other values as JSON.
    pub data: Vec<(String, String)>,
}

impl PushMessage {
    /// An empty message for `event`.
    pub fn new(event: PushEvent) -> Self {
        PushMessage {
            event,
            service: None,
            id: None,
            title: None,
            subtitle: None,
            body: None,
            link: None,
            action: None,
            text: None,
            data: Vec::new(),
        }
    }

    /// A custom value sent with the push.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.data
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Whether it carries something to show (otherwise it is data only).
    pub fn is_visible(&self) -> bool {
        self.title.is_some() || self.body.is_some()
    }

    /// Reads a Prism payload (what `prism-push` sends to Web Push and
    /// UnifiedPush, and FCM data): `title`, `subtitle`, `body`, `link`,
    /// `id`, `data` (an object), `silent`. Other top-level keys are data
    /// too. Returns the message and whether it is silent.
    pub fn from_payload(event: PushEvent, json: &str) -> Option<(Self, bool)> {
        let v = Json::parse(json)?;
        let Json::Object(fields) = v else { return None };
        let mut m = PushMessage::new(event);
        let mut silent = false;
        for (k, v) in fields {
            let s = v.as_str().map(str::to_string);
            match k.as_str() {
                "title" => m.title = s,
                "subtitle" => m.subtitle = s,
                "body" => m.body = s,
                "link" => m.link = s,
                "id" => m.id = s,
                "silent" => silent = v.as_bool().unwrap_or(false),
                "data" => {
                    if let Json::Object(items) = v {
                        for (k, v) in items {
                            m.data.push((k, v.to_text()));
                        }
                    }
                }
                _ => m.data.push((k, v.to_text())),
            }
        }
        if !m.is_visible() {
            silent = true;
        }
        if silent && event == PushEvent::Received {
            m.event = PushEvent::Background;
        }
        Some((m, silent))
    }
}

/// A Live Activity's push token (iOS 16.1+), for updating it from your
/// server (`apns-push-type: liveactivity`, topic `<bundle id>.push-type.liveactivity`).
#[derive(Clone, PartialEq, Eq)]
pub struct ActivityPushToken {
    /// The activity, or `None` for the push-to-start token (iOS 17.2+): it
    /// lets your server start an activity of `kind`.
    ///
    /// An activity the app started: the id `start_activity` returned. One
    /// a push started: the id the push gave it (`prism_push::Push::start_live_activity`),
    /// so your server knows which of its activities the token updates.
    pub activity: Option<ActivityId>,
    /// The activity's kind, as passed to `Activity::new`.
    pub kind: String,
    /// Hex.
    pub token: String,
}

type PushHandler = Rc<dyn Fn(&PushMessage, &mut App)>;
type TokenHandler = Rc<dyn Fn(&PushSubscription, &mut App)>;
type ActivityTokenHandler = Rc<dyn Fn(&ActivityPushToken, &mut App)>;
type BackgroundJob = Rc<dyn Fn(PushMessage, AsyncApp) -> Pin<Box<dyn Future<Output = ()>>>>;
type Waiter = Rc<
    RefCell<(
        Option<Result<PushSubscription, DeviceError>>,
        Option<std::task::Waker>,
    )>,
>;

/// Push state kept by the [`App`].
#[derive(Default)]
pub(crate) struct Push {
    handlers: Vec<PushHandler>,
    token_handlers: Vec<TokenHandler>,
    activity_handlers: Vec<ActivityTokenHandler>,
    background: Option<BackgroundJob>,
    subscription: Option<PushSubscription>,
    waiting: Vec<Waiter>,
    /// Pushes shown as local notifications (UnifiedPush, FCM data), by
    /// notification id: a tap on one is an `Opened` push.
    displayed: Vec<(String, PushMessage)>,
    shown: u64,
}

/// Ids of the notifications Prism shows for pushes start with this.
pub const PUSH_NOTIFICATION: &str = "prism-push:";

impl App {
    /// A tap on a notification shown for a push: the push handlers see it
    /// as [`PushEvent::Opened`] (its link is opened by the response).
    pub(crate) fn push_notification_opened(&mut self, response: &crate::NotificationResponse) {
        if !response.id.starts_with(PUSH_NOTIFICATION) {
            return;
        }
        let mut push = self
            .push
            .displayed
            .iter()
            .find(|(id, _)| *id == response.id)
            .map(|(_, p)| p.clone())
            // The app was relaunched by the tap: only the id is known.
            .unwrap_or_else(|| {
                let mut p = PushMessage::new(PushEvent::Opened);
                p.id = response
                    .id
                    .strip_prefix(PUSH_NOTIFICATION)
                    .map(str::to_string);
                p
            });
        push.event = PushEvent::Opened;
        push.action = response.action.clone();
        push.text = response.text.clone();
        for h in self.push.handlers.clone() {
            h(&push, self);
        }
    }
}

impl App {
    /// Runs `f` for every push: received while open, opened from a
    /// notification, or silent. Register it at startup (a tap may have
    /// launched the app).
    pub fn on_push(&mut self, f: impl Fn(&PushMessage, &mut App) + 'static) {
        self.push.handlers.push(Rc::new(f));
    }

    /// Runs `f` with the device's push subscription: after
    /// [`register_for_push`](App::register_for_push) and whenever the
    /// service changes the token. Send it to your server.
    pub fn on_push_token(&mut self, f: impl Fn(&PushSubscription, &mut App) + 'static) {
        if let Some(sub) = self.push.subscription.clone() {
            f(&sub, self);
        }
        self.push.token_handlers.push(Rc::new(f));
    }

    /// Work for silent pushes. The platform keeps the app running (iOS gives
    /// about 30 seconds) until the future completes; the app may have been
    /// launched in the background just for it.
    pub fn on_background_push<F: Future<Output = ()> + 'static>(
        &mut self,
        job: impl Fn(PushMessage, AsyncApp) -> F + 'static,
    ) {
        self.push.background = Some(Rc::new(move |m, cx| Box::pin(job(m, cx))));
    }

    /// Live Activity push tokens: the push-to-start token (iOS 17.2+), the
    /// update tokens of activities started with [`Activity::push_updates`](crate::Activity::push_updates)
    /// and of those a push started (iOS 17.2+), each again when it changes.
    pub fn on_activity_push_token(&mut self, f: impl Fn(&ActivityPushToken, &mut App) + 'static) {
        if self.push.activity_handlers.is_empty() {
            self.surfaces
                .requests
                .push(crate::PlatformRequest::WatchActivityPushToStart);
        }
        self.push.activity_handlers.push(Rc::new(f));
    }

    /// A Live Activity token handler is registered (bundlers then embed
    /// the extension push-to-start needs).
    pub(crate) fn watches_activity_tokens(&self) -> bool {
        !self.push.activity_handlers.is_empty()
    }

    /// Asks the platform for a push token (and for permission to show
    /// notifications). The answer goes to [`on_push_token`](App::on_push_token);
    /// in a task, `cx.register_for_push().await` returns it.
    pub fn register_for_push(&mut self) {
        self.surfaces
            .requests
            .push(crate::PlatformRequest::RegisterPush);
    }

    /// Stops receiving pushes (the token becomes invalid).
    pub fn unregister_for_push(&mut self) {
        self.push.subscription = None;
        self.surfaces
            .requests
            .push(crate::PlatformRequest::UnregisterPush);
    }

    /// The last subscription the platform gave, if any.
    pub fn push_subscription(&self) -> Option<&PushSubscription> {
        self.push.subscription.as_ref()
    }

    /// For backends: the answer to a registration (or a refreshed token).
    pub fn push_registered(&mut self, result: Result<PushSubscription, DeviceError>) {
        for w in std::mem::take(&mut self.push.waiting) {
            let waker = {
                let mut s = w.borrow_mut();
                s.0 = Some(result.clone());
                s.1.take()
            };
            if let Some(waker) = waker {
                waker.wake();
            }
        }
        let Ok(sub) = result else { return };
        if self.push.subscription.as_ref() == Some(&sub) {
            return;
        }
        self.push.subscription = Some(sub.clone());
        for h in self.push.token_handlers.clone() {
            h(&sub, self);
        }
    }

    /// For backends: a push arrived. Opens its link when it was tapped.
    /// `display`: the platform does not show pushes by itself (UnifiedPush,
    /// FCM data): a visible push received while the app is not on screen
    /// becomes a local notification.
    pub fn push_received(&mut self, push: PushMessage, display: bool) {
        if push.event == PushEvent::Opened
            && let Some(link) = &push.link
        {
            self.navigate(link.clone());
        }
        if display
            && push.event != PushEvent::Opened
            && push.is_visible()
            && self.lifecycle() != crate::Lifecycle::Active
        {
            let mut n = crate::Notification::new(push.title.clone().unwrap_or_default())
                .body(push.body.clone().unwrap_or_default());
            self.push.shown += 1;
            let id = match &push.id {
                Some(id) => format!("{PUSH_NOTIFICATION}{id}"),
                None => format!("{PUSH_NOTIFICATION}{}-{}", n.id, self.push.shown),
            };
            n = n.id(id);
            n.subtitle = push.subtitle.clone();
            n.link = push.link.clone();
            self.push.displayed.push((n.id.clone(), push.clone()));
            if self.push.displayed.len() > 20 {
                self.push.displayed.remove(0);
            }
            self.send_notification(n);
        }
        for h in self.push.handlers.clone() {
            h(&push, self);
        }
    }

    /// For backends: a silent push. Runs the handlers, then the
    /// [`on_background_push`](App::on_background_push) job; returns its task
    /// (tell the platform when it is no longer running).
    pub(crate) fn start_background_push(&mut self, push: PushMessage) -> Option<crate::TaskHandle> {
        self.push_received(push.clone(), false);
        let job = self.push.background.clone()?;
        Some(self.spawn(move |cx| job(push, cx)).finish_in_background())
    }

    /// For backends: a Live Activity token arrived or changed.
    pub fn activity_push_token(&mut self, token: ActivityPushToken) {
        for h in self.push.activity_handlers.clone() {
            h(&token, self);
        }
    }
}

/// Resolves with the push subscription.
pub struct PushFuture {
    slot: Waiter,
}

impl Future for PushFuture {
    type Output = Result<PushSubscription, DeviceError>;
    fn poll(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let mut s = self.slot.borrow_mut();
        match s.0.take() {
            Some(r) => std::task::Poll::Ready(r),
            None => {
                s.1 = Some(cx.waker().clone());
                std::task::Poll::Pending
            }
        }
    }
}

impl AsyncApp {
    /// Registers for remote pushes (asking to show notifications first) and
    /// returns where your server sends them. Fails with
    /// [`DeviceError::Unavailable`] where push does not exist (see
    /// `capabilities().push`).
    pub fn register_for_push(&self) -> PushFuture {
        let slot: Waiter = Rc::new(RefCell::new((None, None)));
        self.update(|app| {
            if app.capabilities().push.is_none() {
                slot.borrow_mut().0 = Some(Err(DeviceError::Unavailable));
                return;
            }
            app.push.waiting.push(slot.clone());
            app.register_for_push();
        });
        PushFuture { slot }
    }
}

// ---------------------------------------------------------------------------
// A small JSON reader (payloads, subscriptions, widget actions).
// ---------------------------------------------------------------------------

/// How deep arrays and objects may nest in [`Json::parse`]: payloads come
/// from the network (a browser reports its pushes), and the reader is
/// recursive. Real payloads nest a few levels.
pub(crate) const MAX_JSON_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Reads `s`; `None` when malformed or nested deeper than
    /// [`MAX_JSON_DEPTH`].
    pub(crate) fn parse(s: &str) -> Option<Json> {
        Self::parse_with_depth(s, MAX_JSON_DEPTH)
    }

    pub(crate) fn parse_with_depth(s: &str, max_depth: usize) -> Option<Json> {
        let mut p = Parser {
            s: s.as_bytes(),
            i: 0,
            depth: max_depth,
        };
        let v = p.value()?;
        p.ws();
        (p.i == p.s.len()).then_some(v)
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Strings as-is, anything else as JSON.
    fn to_text(&self) -> String {
        match self {
            Json::String(s) => s.clone(),
            other => other.to_json(),
        }
    }

    fn to_json(&self) -> String {
        match self {
            Json::Null => "null".into(),
            Json::Bool(b) => b.to_string(),
            Json::Number(n) if n.fract() == 0. && n.abs() < 1e15 => format!("{}", *n as i64),
            Json::Number(n) => n.to_string(),
            Json::String(s) => json_str(s),
            Json::Array(items) => {
                let parts: Vec<String> = items.iter().map(Json::to_json).collect();
                format!("[{}]", parts.join(","))
            }
            Json::Object(items) => {
                let parts: Vec<String> = items
                    .iter()
                    .map(|(k, v)| format!("{}:{}", json_str(k), v.to_json()))
                    .collect();
                format!("{{{}}}", parts.join(","))
            }
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    /// How many more arrays or objects may open.
    depth: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        let c = *self.s.get(self.i)?;
        if matches!(c, b'{' | b'[') {
            // Too deep: refused before descending (the stack is bounded).
            self.depth = self.depth.checked_sub(1)?;
            let v = self.container(c);
            self.depth += 1;
            return v;
        }
        self.scalar(c)
    }

    fn container(&mut self, c: u8) -> Option<Json> {
        match c {
            b'{' => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.eat("}") {
                    return Some(Json::Object(items));
                }
                loop {
                    self.ws();
                    let Json::String(k) = self.value()? else {
                        return None;
                    };
                    self.ws();
                    if !self.eat(":") {
                        return None;
                    }
                    items.push((k, self.value()?));
                    self.ws();
                    if self.eat("}") {
                        return Some(Json::Object(items));
                    }
                    if !self.eat(",") {
                        return None;
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.eat("]") {
                    return Some(Json::Array(items));
                }
                loop {
                    items.push(self.value()?);
                    self.ws();
                    if self.eat("]") {
                        return Some(Json::Array(items));
                    }
                    if !self.eat(",") {
                        return None;
                    }
                }
            }
            _ => None,
        }
    }

    fn scalar(&mut self, c: u8) -> Option<Json> {
        match c {
            b'"' => self.string().map(Json::String),
            b't' if self.eat("true") => Some(Json::Bool(true)),
            b'f' if self.eat("false") => Some(Json::Bool(false)),
            b'n' if self.eat("null") => Some(Json::Null),
            _ => {
                let start = self.i;
                while self.i < self.s.len()
                    && matches!(
                        self.s[self.i],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i])
                    .ok()?
                    .parse()
                    .ok()
                    .map(Json::Number)
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = *self.s.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.s.get(self.i)?;
                    self.i += 1;
                    let ch = match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'u' => {
                            let mut code = self.hex4()?;
                            if (0xD800..0xDC00).contains(&code) && self.eat("\\u") {
                                let low = self.hex4()?;
                                code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                            }
                            char::from_u32(code)?
                        }
                        other => other as char,
                    };
                    let mut buf = [0; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                c => out.push(c),
            }
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(h, 16).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S34: a subscription's secret and endpoint, and a Live Activity's
    /// token, do not show when printed (`{:?}`, in a log).
    #[test]
    fn s34_push_tokens_print_hidden() {
        let sub = PushSubscription {
            service: PushService::WebPush,
            token: "https://push.example/wpush/S34-MARK-EP".into(),
            p256dh: Some("BPUBLIC".into()),
            auth: Some("S34-MARK-AUTH".into()),
            topic: None,
            sandbox: false,
        };
        let activity = ActivityPushToken {
            activity: None,
            kind: "delivery".into(),
            token: "S34-MARK-ACT".into(),
        };
        for shown in [
            format!("{sub:?}"),
            format!("{sub:#?}"),
            format!("{activity:?}"),
        ] {
            assert!(!shown.contains("S34-MARK"), "{shown}");
        }
        assert!(format!("{sub:?}").contains("WebPush"));
        assert!(format!("{activity:?}").contains("delivery"));
    }

    /// S2: nesting is bounded before descending; at the limit it reads.
    #[test]
    fn nesting_deeper_than_the_limit_is_refused() {
        let nested = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(Json::parse_with_depth(&nested(3), 3).is_some());
        assert!(Json::parse_with_depth(&nested(4), 3).is_none());
        let obj = r#"{"a":{"b":{"c":1}}}"#;
        assert!(Json::parse_with_depth(obj, 3).is_some());
        assert!(Json::parse_with_depth(obj, 2).is_none());
        // Far deeper than the stack would take: an error, not an abort.
        assert!(Json::parse(&nested(1_000_000)).is_none());
        assert!(Json::parse(&nested(MAX_JSON_DEPTH)).is_some());
        let payload = format!(r#"{{"title":"t","data":{}}}"#, nested(500_000));
        assert!(PushMessage::from_payload(PushEvent::Received, &payload).is_none());
    }

    #[test]
    fn subscription_round_trips() {
        let mut sub = PushSubscription::new(PushService::WebPush, "https://push.example/abc");
        sub.p256dh = Some("BCVx".into());
        sub.auth = Some("BTBZ".into());
        let json = sub.to_json();
        assert_eq!(
            json,
            r#"{"service":"webpush","endpoint":"https://push.example/abc","keys":{"p256dh":"BCVx","auth":"BTBZ"}}"#
        );
        assert_eq!(PushSubscription::from_json(&json), Some(sub));
        let mut apns = PushSubscription::new(PushService::Apns, "ab12");
        apns.topic = Some("dev.prism.counter".into());
        apns.sandbox = true;
        assert_eq!(PushSubscription::from_json(&apns.to_json()), Some(apns));
    }

    #[test]
    fn payloads_become_messages() {
        let (m, silent) = PushMessage::from_payload(
            PushEvent::Received,
            r#"{"title":"Order shipped","body":"On its way é","link":"/orders/42","data":{"order":42,"tags":["a"]},"extra":"x"}"#,
        )
        .unwrap();
        assert!(!silent);
        assert_eq!(m.title.as_deref(), Some("Order shipped"));
        assert_eq!(m.body.as_deref(), Some("On its way é"));
        assert_eq!(m.link.as_deref(), Some("/orders/42"));
        assert_eq!(m.get("order"), Some("42"));
        assert_eq!(m.get("tags"), Some(r#"["a"]"#));
        assert_eq!(m.get("extra"), Some("x"));
        let (m, silent) =
            PushMessage::from_payload(PushEvent::Received, r#"{"data":{"sync":"inbox"}}"#).unwrap();
        assert!(silent);
        assert_eq!(m.event, PushEvent::Background);
        assert!(PushMessage::from_payload(PushEvent::Received, "not json").is_none());
    }
}
