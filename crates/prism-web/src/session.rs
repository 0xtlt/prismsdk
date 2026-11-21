//! A live session: one [`Runtime`] per open page, on its own thread, fed by
//! the page's socket messages. Transport-agnostic: [`WebApp::connect`] takes
//! a function that sends text to the page and returns a [`Session`] that
//! receives the page's messages, so any WebSocket implementation works.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use http::{HeaderMap, Uri};
use prism_core::surfaces::{ActivityContent, PlatformRequest};
use prism_core::{Capabilities, NodeId, NodeKind, Patch, PlatformServices, Runtime, UiEvent};
use serde_json::{Value, json};

use crate::WebApp;
use crate::html::AssetUrls;
use crate::{css, glyphs};

pub(crate) enum Inbound {
    /// A text message from the page.
    Message(String),
    /// Async work is ready (a task, a timer from another thread).
    Wake,
    /// The socket closed.
    Close,
    /// A sign-in provider redirected (to `/_prism/auth/callback`) for call `id`.
    AuthRedirect(u64, String),
    /// The page fetched the cookie of call `id` (`/_prism/cookie`): the
    /// browser has it.
    CookieSet(u64, prism_core::Cookie),
}

/// What one waiting message costs besides its text (its queue slot): a
/// flood of tiny messages fills the budget too.
const MESSAGE_OVERHEAD: usize = 64;

/// The server side of one open page. Pass it the page's messages; drop it
/// (or call [`close`](Session::close)) when the socket closes.
pub struct Session {
    tx: mpsc::Sender<Inbound>,
    /// Bytes of messages the session has not handled yet.
    queued: Arc<AtomicUsize>,
    max_message: usize,
    max_queued: usize,
}

impl Session {
    /// A text message from the page. Returns `false` once the session
    /// ended, or when it ends it: a message longer than
    /// [`WebApp::max_message_size`], or one the session cannot keep up
    /// with ([`WebApp::max_queued_bytes`] waiting).
    pub fn receive(&self, text: impl Into<String>) -> bool {
        let text = text.into();
        let cost = text.len() + MESSAGE_OVERHEAD;
        if text.len() > self.max_message
            || self.queued.fetch_add(cost, Ordering::SeqCst) + cost > self.max_queued
        {
            self.close();
            return false;
        }
        self.tx.send(Inbound::Message(text)).is_ok()
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
    ///
    /// Check the handshake with [`accept_socket`](WebApp::accept_socket)
    /// before upgrading it: a session it refuses ends at once.
    pub fn connect(
        &self,
        uri: &Uri,
        headers: &HeaderMap,
        send: impl Fn(String) -> bool + Send + 'static,
    ) -> Session {
        self.start(uri, headers, None, None, send)
    }

    /// [`connect`](WebApp::connect) with the host's context for the app
    /// (`cx.request_context::<T>()`), evaluated for this handshake (so again
    /// at every reconnect). The context the page request had stays readable
    /// once, in the page's first session (`cx.page_request_context::<T>()`).
    pub fn connect_with(
        &self,
        uri: &Uri,
        headers: &HeaderMap,
        context: crate::RequestContext,
        send: impl Fn(String) -> bool + Send + 'static,
    ) -> Session {
        self.start(uri, headers, Some(context), None, send)
    }

    /// [`connect`](WebApp::connect) for an upgrade request: its URI, headers
    /// and the [`RequestContext`](crate::RequestContext) in its extensions.
    pub fn connect_request<B>(
        &self,
        req: &http::Request<B>,
        send: impl Fn(String) -> bool + Send + 'static,
    ) -> Session {
        let context = req.extensions().get::<crate::RequestContext>().cloned();
        self.start(req.uri(), req.headers(), context, None, send)
    }

    /// [`connect_request`](WebApp::connect_request) with the permit the
    /// handshake took before upgrading.
    pub(crate) fn connect_reserved<B>(
        &self,
        req: &http::Request<B>,
        permit: SessionPermit,
        send: impl Fn(String) -> bool + Send + 'static,
    ) -> Session {
        let context = req.extensions().get::<crate::RequestContext>().cloned();
        self.start(req.uri(), req.headers(), context, Some(permit), send)
    }

    fn start(
        &self,
        uri: &Uri,
        headers: &HeaderMap,
        context: Option<crate::RequestContext>,
        permit: Option<SessionPermit>,
        send: impl Fn(String) -> bool + Send + 'static,
    ) -> Session {
        let (tx, rx) = mpsc::channel();
        let session = Session {
            tx: tx.clone(),
            queued: Arc::default(),
            max_message: self.inner.max_message,
            max_queued: self.inner.max_queued,
        };
        // The permit is taken atomically: concurrent handshakes never get
        // more than `max_sessions`.
        let permit = match permit.map_or_else(|| self.reserve_session(headers), Ok) {
            Ok(permit) => permit,
            Err(status) => {
                // A foreign origin or too many sessions: tell the page, end now.
                send(json!({ "t": "refused", "status": status.as_u16() }).to_string());
                return session;
            }
        };
        let (sid, path, visitor) = self.socket_visitor(uri, headers);
        let origin = crate::host::page_origin(headers, Some(uri));
        let search = crate::app::query_param(uri, "search").map(crate::app::percent_decode);
        // The page's first handshake takes its request over (`cx.page_request()`).
        let page_request =
            crate::app::query_param(uri, "page").and_then(|id| self.inner.handoffs.take(id, &sid));
        let request = crate::host::web_request(
            prism_core::RequestKind::Socket,
            self.page_url(&origin, &path, search.as_deref()),
            origin.clone(),
            headers,
            context,
        );
        let idle = self.inner.idle_timeout;
        let app = self.clone();
        let waker = tx.clone();
        let inbox = tx.clone();
        let queued = session.queued.clone();
        // One wake-up waits at a time, however often async work wakes us.
        let wake_pending = Arc::new(AtomicBool::new(false));
        let woken = wake_pending.clone();
        std::thread::Builder::new()
            .name("prism-session".into())
            .spawn(move || {
                let _permit = permit;
                let mut options = app.options(visitor.with_sid(&sid), path);
                let device = options.device_info.clone();
                options.wake_ui = Some(std::sync::Arc::new(move || {
                    if !wake_pending.swap(true, Ordering::SeqCst) {
                        let _ = waker.send(Inbound::Wake);
                    }
                }));
                let runtime = Runtime::with_options(&app.init_with(request, page_request), options);
                Live {
                    device: device.unwrap_or_else(|| runtime.app.device_info()),
                    runtime,
                    encoder: Encoder {
                        asset_urls: Some(app.session_asset_urls(&sid)),
                        module_urls: Some(app.module_urls()),
                        ..Default::default()
                    },
                    services: WebServices {
                        secure: Some(crate::security::SecureStore::new(
                            &app.session_dir(&sid),
                            &sid,
                            crate::security::server_key(&app.inner.data_root),
                        )),
                        origin,
                        base: app.base().to_string(),
                        title: app.config().title.clone(),
                        inbox: Some(inbox),
                        sid: Some(sid.clone()),
                        ..app.services()
                    },
                    send: Box::new(send),
                    app,
                    sid,
                    idle,
                    last_seen: Instant::now(),
                    drops: HashMap::new(),
                    queued,
                    woken,
                }
                .run(rx);
            })
            .expect("prism-web: cannot start a session thread");
        session
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
    /// Ends the session after this long without a message from the page.
    idle: Option<Duration>,
    last_seen: Instant,
    /// Drops uploading (#111): their number → the target node.
    drops: HashMap<u64, NodeId>,
    /// Bytes of the page's messages not handled yet (see `Session::receive`).
    queued: Arc<AtomicUsize>,
    /// A wake-up is waiting in the inbox.
    woken: Arc<AtomicBool>,
}

/// One of the app's [`max_sessions`](WebApp::max_sessions), held by a live
/// session (or a handshake about to become one) and freed when it ends.
pub(crate) struct SessionPermit(Arc<AtomicUsize>);

impl SessionPermit {
    /// A permit, unless `max` are taken: checked and taken in one step.
    pub(crate) fn take(count: &Arc<AtomicUsize>, max: Option<usize>) -> Option<Self> {
        let mut n = count.load(Ordering::SeqCst);
        loop {
            if max.is_some_and(|max| n >= max) {
                return None;
            }
            match count.compare_exchange_weak(n, n + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Some(SessionPermit(count.clone())),
                Err(now) => n = now,
            }
        }
    }
}

impl Drop for SessionPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Live {
    /// Serves the page until it closes.
    fn run(mut self, rx: mpsc::Receiver<Inbound>) {
        let reset = self.reset();
        if !(self.send)(reset) {
            return;
        }
        loop {
            if !self.flush_requests() {
                return;
            }
            // `cx.reconnect()`: the page was told, it opens a new session.
            if self.services.reconnect {
                return;
            }
            // Idle for too long: the page reconnects at its next interaction.
            if self.idle.is_some_and(|d| self.last_seen.elapsed() >= d) {
                (self.send)(json!({ "t": "idle" }).to_string());
                return;
            }
            // Calls the server answers itself (secure storage, redirects).
            if !self.services.local.is_empty() {
                let mut patches = Vec::new();
                for (id, result) in std::mem::take(&mut self.services.local) {
                    patches.extend(self.runtime.device_reply(id, result));
                }
                if !self.patches(&patches) {
                    return;
                }
                continue;
            }
            // Sleep until a message, a wake-up, the next timer or the next
            // background task.
            let deadline = [
                self.runtime.next_timer(),
                self.services.next_background(),
                self.idle.map(|d| self.last_seen + d),
            ]
            .into_iter()
            .flatten()
            .min();
            let inbound = match deadline {
                Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
                None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            };
            match &inbound {
                Ok(Inbound::Message(text)) => {
                    self.last_seen = Instant::now();
                    self.queued
                        .fetch_sub(text.len() + MESSAGE_OVERHEAD, Ordering::SeqCst);
                }
                Ok(Inbound::Wake) => self.woken.store(false, Ordering::SeqCst),
                _ => {}
            }
            let patches = match inbound {
                // Busy pages still get their timers on time.
                Ok(Inbound::Message(text)) if deadline.is_some_and(|at| at <= Instant::now()) => {
                    let mut patches = self.message(&text);
                    patches.extend(self.tick());
                    patches
                }
                Ok(Inbound::Message(text)) => self.message(&text),
                Ok(Inbound::AuthRedirect(id, url)) => self
                    .runtime
                    .device_reply(id, Ok(prism_core::DeviceValue::Text(Some(url)))),
                // The browser has the cookie: so does the session's request.
                Ok(Inbound::CookieSet(id, cookie)) => {
                    let app = &mut self.runtime.app;
                    if let Some(mut request) = app.request().cloned() {
                        request.apply_cookie(&cookie);
                        app.set_request(request);
                    }
                    self.runtime
                        .device_reply(id, Ok(prism_core::DeviceValue::Done))
                }
                Ok(Inbound::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => self.tick(),
                Ok(Inbound::Close) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            if !self.patches(&patches) {
                return;
            }
        }
    }

    /// The page's first message: the whole tree, its stylesheets, title
    /// and unsaved state. The tree's patches and JSON are freed once it is
    /// written, not kept for the session's life.
    fn reset(&mut self) -> String {
        // The tree that replaces the first paint waits for its data as the
        // first paint did, or the page would flash its loading state.
        let first = self.runtime.render_settled(self.app.inner.first_paint_wait);
        let mut reset = message("reset", self.encoder.encode(&first));
        // The page learns its stylesheets with the first tree (none: the
        // field is left out), so a reconnected page drops those of its
        // previous session and keeps the first paint's.
        let sheets = self.runtime.app.stylesheets();
        if !sheets.is_empty() {
            reset.insert("sheets".into(), json!(sheets));
        }
        // Its title too (#154): a reconnected page drops a stale one.
        reset.insert("title".into(), json!(self.runtime.app.title()));
        // And whether leaving should ask (#158).
        reset.insert(
            "unsaved".into(),
            self.runtime.app.has_unsaved_changes().into(),
        );
        self.services.sheets = sheets.into_iter().collect();
        Value::Object(reset).to_string()
    }

    /// Runs due background tasks, timers and ready async work.
    fn tick(&mut self) -> Vec<Patch> {
        for id in self.services.due_background() {
            self.runtime.run_background_task(&id);
        }
        self.runtime.poll()
    }

    /// Sends `patches` to the page (none: nothing). `false` once it is gone.
    fn patches(&mut self, patches: &[Patch]) -> bool {
        patches.is_empty()
            || (self.send)(
                Value::Object(message("patch", self.encoder.encode(patches))).to_string(),
            )
    }

    /// Hands platform requests to the page (notifications, device calls…).
    fn flush_requests(&mut self) -> bool {
        for request in self.runtime.take_requests() {
            let request = match (request, &self.encoder.asset_urls) {
                (PlatformRequest::LoadFont(font), Some(urls)) => {
                    PlatformRequest::LoadFont(font_at(font, urls.as_ref()))
                }
                (other, _) => other,
            };
            self.services.handle(request);
        }
        self.services.sync_sheets();
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
                // A WebAuthn ceremony the server checks.
                Some(id) if self.services.ceremonies.contains_key(&id) => {
                    let result = self.services.finish_ceremony(id, &v);
                    rt.device_reply(id, result)
                }
                // Picked files, sent in chunks (#112).
                Some(id) if self.services.uploads.has(crate::uploads::Key::Call(id)) => {
                    let failed = v.get("err").is_some();
                    let key = crate::uploads::Key::Call(id);
                    let result = match self.services.uploads.finish(&self.app, key, failed) {
                        Some(Ok(files)) => Ok(prism_core::DeviceValue::Files(files)),
                        Some(Err(e)) => Err(prism_core::DeviceError::Failed(e)),
                        None => device_result(&v),
                    };
                    rt.device_reply(id, result)
                }
                Some(id) => {
                    if v.get("err").is_some() {
                        crate::security::forget_redirect(id);
                        crate::cookies::forget(id, &self.sid);
                    }
                    let result = device_result(&v);
                    rt.device_reply(id, result)
                }
                None => Vec::new(),
            },
            // The chosen files' sizes, then their bytes (#112).
            Some("upload_start") => {
                let (Some(id), Some(files)) = (
                    v.get("id").and_then(Value::as_u64),
                    v.get("files").and_then(Value::as_array),
                ) else {
                    return Vec::new();
                };
                let key = crate::uploads::Key::Call(id);
                match self
                    .services
                    .uploads
                    .start(&self.app, &self.sid, key, files)
                {
                    Some((sent, total)) => {
                        rt.device_event(id, prism_core::DeviceEvent::Upload { sent, total })
                    }
                    None => Vec::new(),
                }
            }
            Some("upload_chunk") => {
                let (Some(file), Some(data)) = (
                    v.get("file").and_then(Value::as_u64),
                    v.get("data").and_then(Value::as_str),
                ) else {
                    return Vec::new();
                };
                let key = match (
                    v.get("id").and_then(Value::as_u64),
                    v.get("drop").and_then(Value::as_u64),
                ) {
                    (Some(id), None) => crate::uploads::Key::Call(id),
                    (None, Some(drop)) => crate::uploads::Key::Drop(drop),
                    _ => return Vec::new(),
                };
                match (self.services.uploads.chunk(key, file as usize, data), key) {
                    (Some((sent, total)), crate::uploads::Key::Call(id)) => {
                        rt.device_event(id, prism_core::DeviceEvent::Upload { sent, total })
                    }
                    // A drop's progress goes to its target (#199).
                    (Some((sent, total)), crate::uploads::Key::Drop(drop)) => {
                        match self.drops.get(&drop) {
                            Some(&node) => {
                                rt.dispatch(prism_core::UiEvent::DropProgress(node, sent, total))
                            }
                            None => Vec::new(),
                        }
                    }
                    _ => Vec::new(),
                }
            }
            // Files dropped on a drop target (#111): their sizes, the
            // chunks, then the end.
            Some("drop_start") => {
                let (Some(drop), Some(node), Some(files)) = (
                    v.get("drop").and_then(Value::as_u64),
                    v.get("node").and_then(Value::as_u64),
                    v.get("files").and_then(Value::as_array),
                ) else {
                    return Vec::new();
                };
                let accepted = self.encoder.drop_targets.get(&node).is_some_and(|accept| {
                    files.iter().all(|f| {
                        let s = |k: &str| f.get(k).and_then(Value::as_str).unwrap_or("");
                        accept.accepts(s("name"), s("mime"))
                    })
                });
                let uploads = &mut self.services.uploads;
                if accepted && !files.is_empty() && uploads.expect_drop(drop) {
                    let key = crate::uploads::Key::Drop(drop);
                    if let Some((sent, total)) = uploads.start(&self.app, &self.sid, key, files) {
                        self.drops.insert(drop, node);
                        // Its first progress, as the upload starts (#199).
                        return rt.dispatch(prism_core::UiEvent::DropProgress(node, sent, total));
                    }
                    uploads.finish(&self.app, key, true);
                }
                Vec::new()
            }
            Some("drop_end") => {
                let Some(drop) = v.get("drop").and_then(Value::as_u64) else {
                    return Vec::new();
                };
                let Some(node) = self.drops.remove(&drop) else {
                    return Vec::new();
                };
                let failed = v.get("err").is_some();
                let key = crate::uploads::Key::Drop(drop);
                match self.services.uploads.finish(&self.app, key, failed) {
                    Some(Ok(files)) if !files.is_empty() => {
                        rt.dispatch(prism_core::UiEvent::DropFiles(node, files))
                    }
                    _ => Vec::new(),
                }
            }
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
            // The browser window was resized; `safe`: what the device's
            // notch, bars and rounded corners cover (`env(safe-area-inset-*)`,
            // top, right, bottom, left).
            Some("viewport") => match (
                v.get("w").and_then(Value::as_f64),
                v.get("h").and_then(Value::as_f64),
            ) {
                (Some(w), Some(h)) => rt.set_window(w as f32, h as f32, safe_area(&v)),
                _ => Vec::new(),
            },
            // The page described its device (on connect, rotation, fold).
            Some("device") => {
                crate::device_info::apply_report(&mut self.device, &v);
                // A platform authenticator (WebAuthn) for `authenticate`.
                if let Some(uvpa) = v.get("uvpa").and_then(Value::as_bool) {
                    self.services.uvpa = uvpa;
                    rt.app.set_capabilities(self.services.capabilities());
                }
                rt.set_device_info(self.device.clone())
            }
            // The page was hidden or shown again.
            Some("lifecycle") => rt.set_lifecycle(match v.get("state").and_then(Value::as_str) {
                Some("background") => prism_core::Lifecycle::Background,
                Some("inactive") => prism_core::Lifecycle::Inactive,
                _ => prism_core::Lifecycle::Active,
            }),
            // Web Push: the browser's subscription, or why there is none.
            Some("push_sub") => {
                let sub = &v["sub"];
                let s = |k: &str| sub["keys"][k].as_str().map(str::to_string);
                match sub["endpoint"].as_str() {
                    Some(endpoint) => {
                        let mut s_ = prism_core::PushSubscription::new(
                            prism_core::PushService::WebPush,
                            endpoint,
                        );
                        s_.p256dh = s("p256dh");
                        s_.auth = s("auth");
                        rt.push_registered(Ok(s_))
                    }
                    None => Vec::new(),
                }
            }
            Some("push_err") => {
                let err = v["err"].as_str().unwrap_or("failed");
                rt.push_registered(Err(prism_core::DeviceError::from_message(err)))
            }
            // A push reached the page (from the service worker), or its
            // notification was clicked.
            Some("push") => {
                let event = match v["event"].as_str() {
                    Some("opened") => prism_core::PushEvent::Opened,
                    _ => prism_core::PushEvent::Received,
                };
                let payload = v["payload"].as_str().unwrap_or("{}");
                match prism_core::PushMessage::from_payload(event, payload) {
                    Some((mut m, _)) => {
                        m.service = Some(prism_core::PushService::WebPush);
                        // The click opened this page on the link already.
                        if v["launched"].as_bool() == Some(true) {
                            m.link = None;
                        }
                        rt.push_received(m, false)
                    }
                    None => Vec::new(),
                }
            }
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

/// A message for the page, `{"p": ops, "t": t}`. The ops are moved in:
/// `json!` would copy every one of them.
fn message(t: &str, ops: Vec<Value>) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("p".into(), Value::Array(ops));
    m.insert("t".into(), t.into());
    m
}

/// A `change` event's value: a number, or `null` for an optional date
/// picker the user cleared (`NaN`, #89).
fn change_value(v: &Value) -> Option<f64> {
    if v.is_null() {
        Some(f64::NAN)
    } else {
        v.as_f64()
    }
}

pub(crate) fn parse_event(v: &Value) -> Option<UiEvent> {
    if let Some(event) = parse_input_event(v) {
        return Some(event);
    }
    let id = v.get("id")?.as_u64()?;
    match v.get("t")?.as_str()? {
        "click" => Some(UiEvent::Click(id)),
        "submit" => Some(UiEvent::Submit(id)),
        // A rich input reports its runs (#115).
        "input" => match v.get("runs").and_then(Value::as_str) {
            Some(runs) => Some(UiEvent::RichInput(id, runs.to_string())),
            None => Some(UiEvent::Input(id, v.get("v")?.as_str()?.to_string())),
        },
        // An overlay closed by the user says how (#156).
        "back" => match v.get("reason").and_then(Value::as_str) {
            Some(reason) => Some(UiEvent::Dismiss(
                id,
                prism_core::DismissReason::from_name(reason),
            )),
            None => Some(UiEvent::Back(id, v.get("depth")?.as_u64()? as usize)),
        },
        // `null`: an optional date picker was cleared (#89).
        "change" => Some(UiEvent::Change(id, change_value(v.get("v")?)?)),
        "menu" => Some(UiEvent::MenuSelect(id)),
        // A frame the page measured (#192): `{"t":"layout","id","x","y","w","h"}`.
        "layout" => Some(UiEvent::Layout(
            id,
            prism_core::Rect::new(
                v.get("x")?.as_f64()? as f32,
                v.get("y")?.as_f64()? as f32,
                v.get("w")?.as_f64()? as f32,
                v.get("h")?.as_f64()? as f32,
            ),
        )),
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
        // A tap on a 3D scene: `{"t":"scene_tap","id","node","point"}`.
        "scene_tap" => Some(UiEvent::SceneTap(
            id,
            prism_core::SceneTapEvent::decode(&v.to_string()),
        )),
        // An island's CustomEvent: `{"t":"island","id","name","detail"}`.
        "island" => Some(UiEvent::Island(
            id,
            v.get("name")?.as_str()?.to_string(),
            v.get("detail")
                .map_or_else(|| "null".into(), Value::to_string),
        )),
        // A web view's page posted a message (from an allowed origin):
        // `{"t":"webmsg","id","data","origin"}`; it loaded a page:
        // `{"t":"webload","id","url"}`.
        "webmsg" => Some(UiEvent::WebMessage(
            id,
            v.get("data")
                .map_or_else(|| "null".into(), Value::to_string),
            v.get("origin")?.as_str()?.to_string(),
        )),
        "webload" => Some(UiEvent::WebLoad(id, v.get("url")?.as_str()?.to_string())),
        // A video played to the end (#207).
        "ended" => Some(UiEvent::Ended(id)),
        _ => None,
    }
}

/// Pointer, drag and key events (#17): `{"t":"click","id","x","y"}` (a
/// click with its position), `{"t":"pointer","id","phase","x","y"}`,
/// `{"t":"drag","id","phase","x","y","dx","dy"}`,
/// `{"t":"key","id","key","mods","repeat","sel"}` (`id` 0 or absent:
/// nothing Prism knows has the focus; `sel`: `[start, end]`, the focused
/// text field's selection in characters, #86) and
/// `{"t":"select","id","start","end"}` (a text field's selection, #86).
fn parse_input_event(v: &Value) -> Option<UiEvent> {
    use prism_core::{
        DragEvent, DragPhase, KeyEvent, Modifiers, Point, PointerEvent, PointerPhase,
    };
    let num = |k: &str| v.get(k).and_then(Value::as_f64).map(|n| n as f32);
    let id = v.get("id").and_then(Value::as_u64);
    match v.get("t")?.as_str()? {
        // With the keys held and the click count (#159) when the page says.
        "click" => {
            let position = Point::new(num("x")?, num("y")?);
            let mods = v.get("mods").and_then(Value::as_u64).unwrap_or(0) as u8;
            let count = v.get("n").and_then(Value::as_u64).unwrap_or(1).max(1) as u32;
            Some(if mods == 0 && count == 1 {
                UiEvent::ClickAt(id?, position)
            } else {
                UiEvent::ClickWith(
                    id?,
                    prism_core::ClickEvent {
                        position: Some(position),
                        modifiers: Modifiers::from_bits(mods),
                        count,
                        button: prism_core::PointerButton::Primary,
                        text_offset: None,
                    },
                )
            })
        }
        // The middle, back or forward button (#215): `{"t":"auxclick","id",
        // "button","x","y","mods","n"}`.
        "auxclick" => Some(UiEvent::AuxClick(
            id?,
            prism_core::ClickEvent {
                position: Some(Point::new(num("x")?, num("y")?)),
                modifiers: Modifiers::from_bits(
                    v.get("mods").and_then(Value::as_u64).unwrap_or(0) as u8
                ),
                count: v.get("n").and_then(Value::as_u64).unwrap_or(1).max(1) as u32,
                button: prism_core::PointerButton::from_name(v.get("button")?.as_str()?).filter(
                    |b| {
                        !matches!(
                            b,
                            prism_core::PointerButton::Primary
                                | prism_core::PointerButton::Secondary
                        )
                    },
                )?,
                text_offset: None,
            },
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
                selection: v.get("sel").and_then(Value::as_array).and_then(|a| {
                    let at = |i: usize| a.get(i).and_then(Value::as_u64).map(|n| n as usize);
                    Some(prism_core::SelectionEvent::new(at(0)?, at(1)?))
                }),
                // Where the field's caret was (#211): `"sfl"`…
                edges: v
                    .get("edges")
                    .and_then(Value::as_str)
                    .map(prism_core::CaretEdges::decode),
            },
        )),
        // A node took or lost the focus (#224).
        "focus" => Some(UiEvent::FocusChange(id?, v.get("on")?.as_bool()?)),
        // A text field's selection moved (#86), in characters.
        "select" => Some(UiEvent::Select(
            id?,
            v.get("start")?.as_u64()? as usize,
            v.get("end")?.as_u64()? as usize,
        )),
        // A rich input's editing hooks (#217): `{"t":"selrect","id","start",
        // "end","x","y","w","h"}` in the field's coordinates; `{"t":"copy",
        // "id","cut","start","end","text"}`; `{"t":"paste","id","start","end",
        // "text","html","types","data":[[type, text]…]}`; `{"t":"spanclick",
        // "id","at","mods"}`.
        "selrect" => Some(UiEvent::SelectionRect(
            id?,
            v.get("start")?.as_u64()? as usize,
            v.get("end")?.as_u64()? as usize,
            prism_core::Rect::new(num("x")?, num("y")?, num("w")?.max(0.), num("h")?.max(0.)),
        )),
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
        // One selection across several fields (#231): `{"t":"across","id":
        // group,"a":[field, offset],"h":[field, offset]}` (without `a` and
        // `h`: it ended); `{"t":"acrossedit","id","cause","text"}`;
        // `{"t":"acrosscopy","id","cut"}`.
        "across" => {
            let point = |k: &str| -> Option<(u64, usize)> {
                let a = v.get(k)?.as_array()?;
                Some((a.first()?.as_u64()?, a.get(1)?.as_u64()? as usize))
            };
            Some(UiEvent::SelectionAcross(id?, point("a").zip(point("h"))))
        }
        "acrossedit" => Some(UiEvent::EditAcross(
            id?,
            prism_core::EditCause::from_name(v.get("cause")?.as_str()?)?,
            v.get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        )),
        "acrosscopy" => Some(UiEvent::CopyAcross(
            id?,
            v.get("cut").and_then(Value::as_bool).unwrap_or(false),
        )),
        // A click in a rich input (#266): the character under it.
        "fieldclick" => Some(UiEvent::FieldClick(
            id?,
            prism_core::ClickEvent {
                position: Some(Point::new(num("x").unwrap_or(0.), num("y").unwrap_or(0.))),
                modifiers: Modifiers::from_bits(
                    v.get("mods").and_then(Value::as_u64).unwrap_or(0) as u8
                ),
                count: v.get("n").and_then(Value::as_u64).unwrap_or(1).max(1) as u32,
                button: prism_core::PointerButton::Primary,
                text_offset: Some(v.get("at")?.as_u64()? as usize),
            },
        )),
        "spanclick" => Some(UiEvent::SpanClick(
            id?,
            v.get("at")?.as_u64()? as usize,
            Modifiers::from_bits(v.get("mods").and_then(Value::as_u64).unwrap_or(0) as u8),
        )),
        _ => None,
    }
}

/// Standard base64 (what `FileReader.readAsDataURL` produces).
pub(crate) fn base64_decode(input: &str) -> Option<Vec<u8>> {
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
pub(crate) fn device_result(v: &Value) -> Result<prism_core::DeviceValue, prism_core::DeviceError> {
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
    /// Answers the server gives itself, without the page.
    pub local: Vec<(
        u64,
        Result<prism_core::DeviceValue, prism_core::DeviceError>,
    )>,
    /// This browser's secrets (`None` while rendering a page).
    pub secure: Option<crate::security::SecureStore>,
    /// The page's origin (`https://app.example.com`) and the mount point.
    pub origin: String,
    pub base: String,
    pub title: String,
    /// The session's own inbox (sign-in redirects arrive there).
    pub inbox: Option<mpsc::Sender<Inbound>>,
    /// The browser's `prism_sid` (`None` while rendering a page).
    pub sid: Option<String>,
    /// `cx.reconnect()`: end the session once the page is told.
    pub reconnect: bool,
    /// WebAuthn ceremonies waiting for the page, by call id.
    pub ceremonies: HashMap<u64, (crate::security::Ceremony, Gate)>,
    /// The browser has a platform authenticator with user verification.
    pub uvpa: bool,
    /// Periodic background tasks: (id, next run, interval). On the web they
    /// run in each open page's session.
    background: Vec<(String, Instant, Duration)>,
    /// The VAPID public key: Web Push is available.
    vapid: Option<String>,
    /// Stylesheets the app added (`App::add_stylesheet`), and whether the
    /// page has not been told the current set yet.
    pub sheets: std::collections::BTreeSet<String>,
    sheets_dirty: bool,
    /// Picked files on their way from the page (#112).
    pub uploads: crate::uploads::Uploads,
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
    pub(crate) fn new(vapid: Option<String>) -> Self {
        WebServices {
            vapid,
            ..Default::default()
        }
    }

    /// Background tasks due now (removed until they reschedule themselves).
    fn due_background(&mut self) -> Vec<String> {
        let now = Instant::now();
        let (due, later): (Vec<_>, Vec<_>) =
            self.background.drain(..).partition(|(_, at, _)| *at <= now);
        self.background = later;
        due.into_iter().map(|(id, _, _)| id).collect()
    }

    /// The page's stylesheets, once they changed.
    pub(crate) fn sync_sheets(&mut self) {
        if std::mem::take(&mut self.sheets_dirty) {
            self.outbox
                .push(json!({ "t": "sheets", "urls": self.sheets.iter().collect::<Vec<_>>() }));
        }
    }

    fn next_background(&self) -> Option<Instant> {
        self.background.iter().map(|(_, at, _)| *at).min()
    }

    /// Messages for the page so far (tests).
    #[cfg(test)]
    pub(crate) fn take_outbox(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.outbox)
    }
}

/// `font` with an asset's name (`fonts/lora.ttf`) turned into its served URL
/// (hashed, from the assets folder or the embedded assets), like a picture's.
pub(crate) fn font_at(
    font: prism_core::FontFace,
    urls: &dyn Fn(&str) -> String,
) -> prism_core::FontFace {
    match &font.source {
        prism_core::FontSource::Url(url) => {
            let served = urls(url);
            prism_core::FontFace {
                source: prism_core::FontSource::Url(served),
                ..font
            }
        }
        prism_core::FontSource::Bytes(_) => font,
    }
}

/// The page message registering `font`: its family, its source (a URL, or
/// the file in base64url) and descriptors.
fn font_message(font: &prism_core::FontFace) -> Value {
    let mut msg = json!({
        "t": "font",
        "family": font.family,
        "style": if font.italic { "italic" } else { "normal" },
    });
    match &font.source {
        prism_core::FontSource::Url(url) => msg["url"] = json!(url),
        prism_core::FontSource::Bytes(b) => msg["data"] = json!(crate::security::base64url(b)),
    }
    if let Some(w) = font.css_weight() {
        msg["weight"] = json!(w);
    }
    msg
}

/// `@font-face` rules for the faces a first paint asked for by URL (bytes
/// wait for the live page: they would weigh on every page load).
pub(crate) fn font_face_css(fonts: &[prism_core::FontFace]) -> String {
    let clean = |s: &str| s.replace(['"', '\\', '<', '>', '\n'], "");
    fonts
        .iter()
        .filter_map(|f| match &f.source {
            prism_core::FontSource::Url(url) => Some(format!(
                "@font-face{{font-family:\"{}\";src:url(\"{}\");{}font-style:{};font-display:swap}}\n",
                clean(&f.family),
                clean(url),
                f.css_weight()
                    .map(|w| format!("font-weight:{w};"))
                    .unwrap_or_default(),
                if f.italic { "italic" } else { "normal" },
            )),
            prism_core::FontSource::Bytes(_) => None,
        })
        .collect()
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
            drop_files: true,
            drag_and_drop: true,
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
            cloud_backup: false,
            // On the server, per browser, encrypted at rest.
            secure_storage: true,
            biometry: if self.uvpa {
                prism_core::BiometryKind::Platform
            } else {
                prism_core::BiometryKind::None
            },
            device_auth: self.uvpa,
            sign_in_with_apple: false,
            web_auth: true,
            // The browser's push service, once a VAPID key is configured.
            push: self
                .vapid
                .as_ref()
                .map(|_| prism_core::PushService::WebPush),
            // An `<iframe>`.
            web_view: true,
            // navigator.credentials (#94); a browser without it answers
            // « unavailable ».
            passkeys: true,
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
            // The Badging API (#239): `null` clears it.
            PlatformRequest::SetBadge(n) => json!({ "t": "badge", "n": n }),
            PlatformRequest::RegisterPush => match &self.vapid {
                Some(key) => json!({ "t": "push_register", "key": key }),
                // No VAPID key: capabilities().push is None (register_for_push
                // fails with Unavailable before asking).
                None => return,
            },
            PlatformRequest::UnregisterPush => json!({ "t": "push_unregister" }),
            // Live Activities are an in-page banner here: no push tokens.
            PlatformRequest::WatchActivityPushToStart => return,
            PlatformRequest::Device { id, call } => match self.security(id, call) {
                // The page sends the chosen files in chunks (#112).
                Some(msg)
                    if matches!(
                        msg["call"].as_str(),
                        Some("pick_files" | "pick_images" | "pick_folder" | "read_clipboard_item")
                    ) =>
                {
                    self.uploads.expect(id);
                    msg
                }
                Some(msg) => msg,
                None => return,
            },
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
            // No tray or menu bar in a browser tab (capabilities say so),
            // but the app menus' shortcuts are the app's (#161): the page
            // takes them before the browser and runs the item.
            PlatformRequest::SetAppMenu(menus) | PlatformRequest::SetMenuBar(menus) => {
                json!({ "t": "shortcuts", "list": menu_shortcuts(&menus) })
            }
            PlatformRequest::SetTray(_)
            | PlatformRequest::RemoveTray(_)
            | PlatformRequest::SetCar(_) => return,
            // A `FontFace` in `document.fonts`, for this page.
            PlatformRequest::LoadFont(font) => font_message(&font),
            // Sent as the whole set (`sync_sheets`).
            PlatformRequest::AddStylesheet(url) => {
                self.sheets_dirty |= self.sheets.insert(url);
                return;
            }
            // The app's title (#154): the tab's, over the screens'.
            PlatformRequest::SetTitle(title) => json!({ "t": "title", "title": title }),
            // Unsaved changes (#158): the browser asks before leaving.
            PlatformRequest::SetUnsavedChanges(on) => json!({ "t": "unsaved", "on": on }),
            PlatformRequest::RemoveStylesheet(url) => {
                self.sheets_dirty |= self.sheets.remove(&url);
                return;
            }
        };
        self.outbox.push(msg);
    }
}

/// Encodes patches to the JSON wire format. Styles are pre-rendered to CSS on
/// the server, which needs each node's kind, so the encoder remembers them.
///
/// A style op whose CSS another node of the same message got first says
/// `"like": <that node>` instead of repeating it: the rows of a list share
/// a few styles, which were half of a first tree's bytes.
#[derive(Default)]
pub struct Encoder {
    kinds: HashMap<NodeId, NodeKind>,
    /// While one message is encoded: each CSS sent so far → the node that
    /// carries it, and back (to forget a carrier restyled or destroyed).
    sent: HashMap<String, NodeId>,
    carriers: HashMap<NodeId, String>,
    /// The page's drop targets and the types they accept (#111): files
    /// dropped elsewhere are refused.
    pub drop_targets: HashMap<NodeId, prism_core::PickFiles>,
    /// Hashed asset URLs for pictures (`/assets/<name>` if unset).
    pub asset_urls: Option<AssetUrls>,
    /// URLs of island modules (none if unset: islands show their fallback).
    pub module_urls: Option<crate::ModuleUrls>,
}

impl Encoder {
    /// The ops of one message, in order (the client applies them in order).
    pub fn encode(&mut self, patches: &[Patch]) -> Vec<Value> {
        let ops = patches.iter().map(|p| self.encode_one(p)).collect();
        // References stay inside their message.
        self.sent = HashMap::new();
        self.carriers = HashMap::new();
        ops
    }

    /// The node that carries `css` earlier in this message, if any; else
    /// `id` becomes its carrier. `id` stops carrying its previous style.
    fn carrier(&mut self, id: NodeId, css: &str) -> Option<NodeId> {
        self.forget(id);
        if let Some(&node) = self.sent.get(css) {
            return Some(node);
        }
        self.sent.insert(css.to_string(), id);
        self.carriers.insert(id, css.to_string());
        None
    }

    /// `id` no longer carries the style it had in this message.
    fn forget(&mut self, id: NodeId) {
        if let Some(css) = self.carriers.remove(&id) {
            self.sent.remove(&css);
        }
    }

    fn encode_one(&mut self, p: &Patch) -> Value {
        match p {
            Patch::Create { id, kind } => {
                self.kinds.insert(*id, *kind);
                json!({ "op": "create", "id": id, "kind": kind.name() })
            }
            Patch::SetStyle { id, style } => {
                let kind = self.kinds.get(id).copied().unwrap_or(NodeKind::View);
                let css = css(kind, style);
                let mut op = match self.carrier(*id, &css) {
                    Some(like) => json!({ "op": "style", "id": id, "like": like }),
                    None => json!({ "op": "style", "id": id, "css": css }),
                };
                let motion = crate::css::motion(style);
                if !motion.is_null() {
                    op["motion"] = motion;
                }
                crate::css::layout_op_fields(kind, style, &mut op);
                // Hover, press and focus refinements: pseudo-class rules.
                let states = crate::css::states_css(kind, *id, style);
                if !states.is_empty() {
                    op["states"] = json!(states);
                }
                if style.group {
                    op["group"] = json!(true);
                }
                if let Some(name) = &style.group_name {
                    op["gn"] = json!(name.as_ref());
                }
                // `inert` is an attribute, not CSS.
                if style.inert {
                    op["inert"] = json!(true);
                }
                op
            }
            Patch::SetText { id, text } => {
                let text = match self.kinds.get(id) {
                    Some(NodeKind::Picture) => match &self.asset_urls {
                        Some(urls) => urls(text),
                        None => crate::html::picture_url(text),
                    },
                    // A video (#207): its asset or proxy URL (ranges).
                    Some(NodeKind::Video) => crate::html::media_url(&match &self.asset_urls {
                        Some(urls) => urls(text),
                        None => crate::html::picture_url(text),
                    }),
                    _ => text.clone(),
                };
                json!({ "op": "text", "id": id, "text": text })
            }
            Patch::SetPlaceholder { id, text } => {
                json!({ "op": "placeholder", "id": id, "text": text })
            }
            // A scene's update; its textures are pictures too (`p3d:` ones
            // are blobs the client fetches from `/_prism/3d/<id>`).
            Patch::SetScene { id, update } => {
                let update = crate::html::scene_urls(update, &|src: &str| match &self.asset_urls {
                    Some(urls) => urls(src),
                    None => crate::html::picture_url(src),
                });
                json!({ "op": "scene", "id": id, "u": update.to_value() })
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
            Patch::SetConfig { id, config } => {
                // A video's poster is a picture: its URL (#207).
                let config = match self.kinds.get(id) {
                    Some(NodeKind::Video) => {
                        let mut c = prism_core::VideoConfig::decode(config);
                        c.poster = c.poster.map(|p| match &self.asset_urls {
                            Some(urls) => urls(&p),
                            None => crate::html::picture_url(&p),
                        });
                        c.encode()
                    }
                    // A picture's focal point (#218): CSS for the page.
                    Some(NodeKind::Picture) => crate::html::object_position_css(config),
                    _ => config.clone(),
                };
                json!({ "op": "config", "id": id, "config": config })
            }
            Patch::SetIcon { id, icon } => {
                json!({ "op": "icon", "id": id, "icon": icon, "glyph": glyphs::glyph(icon) })
            }
            Patch::SetAccessibility { id, a11y } => {
                let level = match a11y.role {
                    Some(prism_core::Role::Heading(l)) => Some(l),
                    _ => None,
                };
                let mut a = json!({
                    "role": a11y.role.map(|r| r.aria()),
                    "level": level,
                    "label": a11y.label,
                    "hint": a11y.hint,
                    "value": a11y.value,
                    "checked": a11y.checked,
                    "disabled": a11y.disabled,
                    "hidden": a11y.hidden,
                });
                // Rare states only when set: most patches stay as they were.
                if let Some(selected) = a11y.selected {
                    a["selected"] = json!(selected);
                }
                if a11y.current {
                    a["current"] = json!(true);
                }
                // A tooltip (#216): its text, or `true` for the full text
                // while truncated.
                match &a11y.tooltip {
                    Some(prism_core::Tooltip::Text(t)) => a["tip"] = json!(t),
                    Some(prism_core::Tooltip::WhenTruncated) => a["tip"] = json!(true),
                    None => {}
                }
                json!({ "op": "a11y", "id": id, "a11y": a })
            }
            Patch::SetRoot { id } => json!({ "op": "root", "id": id }),
            Patch::Destroy { id } => {
                self.kinds.remove(id);
                self.drop_targets.remove(id);
                self.forget(*id);
                json!({ "op": "destroy", "id": id })
            }
            // Drop targets (#111): `types: null` is none.
            Patch::SetDropTarget { id, types } => {
                match types {
                    Some(t) => self
                        .drop_targets
                        .insert(*id, prism_core::PickFiles::new().types(t.clone())),
                    None => self.drop_targets.remove(id),
                };
                json!({ "op": "drop", "id": id, "types": types })
            }
            // Drag and drop between elements (#251): the kind it carries
            // (`null`: not draggable), its preview, the kinds it takes.
            Patch::SetDragAndDrop {
                id,
                kind,
                preview,
                accepts,
            } => json!({
                "op": "dnd",
                "id": id,
                "kind": kind,
                "preview": *preview == prism_core::DragPreview::Snapshot,
                "accepts": accepts,
            }),
            // Pointer, keys, focus, layers (#17, #25).
            Patch::SetPointerEvents { id, mask } => {
                json!({ "op": "pointer", "id": id, "mask": mask })
            }
            // A touch drag after a long press (#243).
            Patch::SetDragDelay { id, ms } => json!({ "op": "dragdelay", "id": id, "ms": ms }),
            Patch::SetKeyEvents { id, local, global } => {
                json!({ "op": "keys", "id": id, "local": local, "global": global })
            }
            // Scroll a scroll view (#193): finite numbers for JSON (the end
            // is a big one, the browser clamps).
            Patch::ScrollTo {
                id,
                x,
                y,
                relative,
                animated,
            } => {
                let n = |v: &f32| if v.is_finite() { v.min(1e9) } else { 1e9 };
                json!({ "op": "scrollto", "id": id, "x": n(x), "y": n(y), "rel": relative, "anim": animated })
            }
            // Elements whose frame the page reports (#192).
            Patch::SetLayoutEvents { id, on } => json!({ "op": "layout", "id": id, "on": on }),
            // Keys the page keeps from the focused field (#181).
            Patch::SetConsumedKeys { id, keys } => {
                json!({ "op": "consume", "id": id, "keys": keys })
            }
            Patch::Focus { id } => json!({ "op": "focus", "id": id }),
            // Text field selections (#86), in characters.
            Patch::SetSelection { id, start, end } => {
                json!({ "op": "sel", "id": id, "start": start, "end": end })
            }
            Patch::SetSelectEvents { id, on } => json!({ "op": "selev", "id": id, "on": on }),
            // Other people's carets (#240), drawn over the field by the client.
            Patch::SetRemoteCarets { id, carets } => json!({
                "op": "carets",
                "id": id,
                "carets": carets.iter().map(remote_caret_json).collect::<Vec<_>>(),
            }),
            Patch::ScrollIntoView { id } => json!({ "op": "reveal", "id": id }),
            Patch::DismissKeyboard => json!({ "op": "blur" }),
            // A pop-up menu (#247): over the node (`id` null: the page), at
            // `at` in it or below it.
            Patch::ShowMenu { id, menu, at } => json!({
                "op": "popup", "id": id,
                "menu": serde_json::from_str::<Value>(&menu.to_json()).unwrap_or(Value::Null),
                "at": at.map(|p| [p.x, p.y]),
            }),
            Patch::SetLayers { ids } => json!({ "op": "layers", "ids": ids }),
            // Islands: `client.js` swaps in the custom element, imports its
            // module once, forwards its events and updates its props.
            Patch::SetIsland {
                id,
                tag,
                source,
                events,
            } => {
                let module = self.module_urls.as_ref().and_then(|urls| urls(source));
                let wasm = matches!(source, prism_core::IslandSource::Wasm(_));
                json!({ "op": "island", "id": id, "tag": tag, "module": module, "wasm": wasm, "events": events })
            }
            // The node becomes an `<a href>` (`client.js` resolves the path
            // under the mount point, like rich text links).
            Patch::SetLink { id, href } => json!({ "op": "link", "id": id, "href": href }),
            Patch::SetIslandProps { id, props } => {
                let props: serde_json::Map<String, Value> = props
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::from_str(v).unwrap_or(Value::Null)))
                    .collect();
                json!({ "op": "props", "id": id, "props": props })
            }
            // App-defined attributes (#85): every one, in order.
            Patch::SetAttributes { id, attributes } => {
                json!({ "op": "attrs", "id": id, "attrs": attributes })
            }
            // Web views (#62): `client.js` posts to the frame (once loaded).
            Patch::PostWebMessage { id, data } => {
                let data: Value = serde_json::from_str(data).unwrap_or(Value::Null);
                json!({ "op": "webpost", "id": id, "data": data })
            }
            Patch::ReloadWebView { id } => json!({ "op": "webreload", "id": id }),
            // A rich input's runs (#115).
            Patch::SetRichValue { id, runs } => json!({ "op": "rich", "id": id, "runs": runs }),
            // A rich input's editing hooks (#217): which events it reports,
            // what a copy writes, a paste the app left to the field.
            Patch::SetEditEvents { id, mask } => json!({ "op": "editev", "id": id, "mask": mask }),
            Patch::WriteClipboard { id, content } => json!({
                "op": "clip",
                "id": id,
                "items": content.entries().into_iter().map(|(m, d)| json!([m, d])).collect::<Vec<_>>(),
            }),
            Patch::InsertText { id, text } => json!({ "op": "instext", "id": id, "text": text }),
            // One selection across several fields (#231): a group, a
            // field's highlighted part, the selection the runtime set.
            Patch::SetSelectionGroup { id, on, edits } => {
                json!({ "op": "selgroup", "id": id, "on": on, "edits": edits })
            }
            Patch::SetSelectionHighlight { id, start, end } => {
                json!({ "op": "selhl", "id": id, "start": start, "end": end })
            }
            Patch::SetSelectionAcross { group, range } => match range {
                Some(((a, ao), (h, ho))) => {
                    json!({ "op": "across", "id": group, "a": [a, ao], "h": [h, ho] })
                }
                None => json!({ "op": "across", "id": group }),
            },
        }
    }
}

/// A remote caret for the client (#240): its offsets in characters, its
/// color in CSS (`light-dark()` when it adapts), its label, whether idle.
fn remote_caret_json(c: &prism_core::RemoteCaret) -> Value {
    json!({
        "anchor": c.anchor,
        "head": c.head,
        "color": c.color.to_css(),
        "label": c.label,
        // The label's text (#267); white by default.
        "text": c.label_text_color.map(|t| t.to_css()),
        "idle": c.idle,
    })
}

/// The shortcuts of the app menus' enabled items, as `[keys, id]` (#161),
/// hints left out (#247).
fn menu_shortcuts(menus: &[prism_core::menu::MenuSpec]) -> Vec<Value> {
    use prism_core::menu::MenuItemSpec;
    fn walk(items: &[MenuItemSpec], out: &mut Vec<Value>) {
        for item in items {
            match item {
                // A hint (#247) is only shown: the key stays the page's.
                MenuItemSpec::Action {
                    id,
                    shortcut: Some(keys),
                    shortcut_hint: false,
                    enabled: true,
                    ..
                } => out.push(json!([keys.to_ascii_lowercase(), id])),
                MenuItemSpec::Submenu { menu, .. } => walk(&menu.items, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for m in menus {
        walk(&m.items, &mut out);
    }
    out
}

/// A device call for the JS client.
pub(crate) fn device_json(id: u64, call: prism_core::DeviceCall) -> Value {
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
        C::PickFiles(o) => {
            json!({ "call": "pick_files", "types": o.types, "multiple": o.multiple })
        }
        // A folder and its files, uploaded like picked files (#225).
        C::PickFolder(_) => json!({ "call": "pick_folder" }),
        C::ReadClipboard => json!({ "call": "paste" }),
        // #246: its types go up like picked files, each named by its type.
        C::ReadClipboardItem => json!({ "call": "read_clipboard_item" }),
        C::Haptic(h) => json!({ "call": "haptic", "kind": h.name() }),
        C::CopyText(t) => json!({ "call": "copy", "text": t }),
        C::Share(s) => json!({ "call": "share", "text": s.text, "url": s.url }),
        C::OpenUrl(u) => json!({ "call": "open_url", "url": u }),
        C::OpenUrlIn(u, target) => json!({ "call": "open_url", "url": u, "target": target.name() }),
        // The app's own relying party (#94): the page runs the ceremony with
        // the server's options and answers the credential as JSON (`text`).
        C::WebAuthn(r) => json!({
            "call": "webauthn_rp",
            "create": r.create,
            "conditional": r.conditional,
            "options": serde_json::from_str::<Value>(&r.options).unwrap_or(Value::Null),
        }),
        // Handled by WebServices::security.
        _ => json!({ "call": "unavailable" }),
    };
    let mut body = body;
    body["t"] = json!("device");
    body["id"] = json!(id);
    body
}

/// What a WebAuthn ceremony unlocks once the owner is verified.
pub(crate) enum Gate {
    /// `authenticate`: nothing more.
    Owner,
    /// A biometric secret's value.
    Secret(String),
    /// Stores a biometric secret (the owner enrolls first).
    Store { key: String, value: String },
}

type Answer = Result<prism_core::DeviceValue, prism_core::DeviceError>;

impl WebServices {
    fn reply(&mut self, id: u64, r: Answer) {
        self.local.push((id, r));
    }

    /// Security calls, answered on the server (secrets, redirects) or with
    /// WebAuthn; everything else goes to the page as it is.
    fn security(&mut self, id: u64, call: prism_core::DeviceCall) -> Option<Value> {
        use prism_core::{DeviceCall as C, DeviceError as E, DeviceValue as V};
        match call {
            C::SecureSet {
                key,
                value,
                options,
            } => {
                // A biometric secret needs an enrolled owner: the first one
                // enrolls this browser's authenticator, so reading one never
                // enrolls whoever reads it first.
                let enrolled = self.secure.as_ref().map(|s| s.credential().is_some());
                if options.biometric && enrolled == Some(false) {
                    if !self.uvpa {
                        self.reply(id, Err(E::Unavailable));
                        return None;
                    }
                    return self.ceremony(id, String::new(), Gate::Store { key, value });
                }
                let r = match &self.secure {
                    Some(s) => s.set(&key, &value, options.biometric).map(|_| V::Done),
                    None => Err("no session".into()),
                };
                self.reply(id, r.map_err(E::Failed));
                None
            }
            C::SecureGet { key, prompt, .. } => {
                let found = match &self.secure {
                    Some(s) => s.get(&key),
                    None => Err("no session".into()),
                };
                match found {
                    Ok(Some(secret)) if secret.biometric => {
                        if !self.uvpa {
                            self.reply(id, Err(E::Unavailable));
                            return None;
                        }
                        let reason = prompt.unwrap_or_default();
                        self.ceremony(id, reason, Gate::Secret(secret.value))
                    }
                    Ok(secret) => {
                        self.reply(id, Ok(V::Text(secret.map(|s| s.value))));
                        None
                    }
                    Err(e) => {
                        self.reply(id, Err(E::Failed(e)));
                        None
                    }
                }
            }
            C::SecureDelete { key, .. } => {
                if let Some(s) = &self.secure {
                    s.delete(&key);
                }
                self.reply(id, Ok(V::Done));
                None
            }
            C::BiometryStatus => {
                let flag = |b: bool| (if b { "1" } else { "0" }).to_string();
                let kind = self.capabilities().biometry.name().to_string();
                let fields = [
                    ("kind".to_string(), kind),
                    ("enrolled".to_string(), flag(self.uvpa)),
                    ("passcode".to_string(), flag(self.uvpa)),
                ];
                self.reply(id, Ok(V::Fields(fields.into_iter().collect())));
                None
            }
            C::Authenticate(r) => {
                if !self.uvpa {
                    self.reply(id, Err(E::Unavailable));
                    return None;
                }
                self.ceremony(id, r.reason, Gate::Owner)
            }
            C::SignInWithApple(_) => {
                // Apple's web sign-in is OAuth: use prism::oauth with its endpoints.
                self.reply(id, Err(E::Unavailable));
                None
            }
            C::AuthRedirect(_) => {
                let uri = format!("{}{}/_prism/auth/callback", self.origin, self.base);
                self.reply(id, Ok(V::Text(Some(uri))));
                None
            }
            C::WebAuth(r) => {
                match (crate::security::state_of(&r.url), self.inbox.clone()) {
                    (Some(state), Some(tx)) => crate::security::wait_for_redirect(state, tx, id),
                    _ => {
                        self.reply(id, Err(E::Failed("the sign-in URL has no state".into())));
                        return None;
                    }
                }
                Some(json!({
                    "t": "device", "id": id, "call": "web_auth",
                    "url": r.url, "provider": r.provider,
                }))
            }
            C::SetCookie(cookie) => {
                let refused = cookie
                    .check()
                    .err()
                    .or_else(|| crate::cookies::refused(&cookie, &self.origin));
                if let Some(why) = refused {
                    self.reply(id, Err(E::Failed(why)));
                    return None;
                }
                let (Some(sid), Some(tx)) = (self.sid.clone(), self.inbox.clone()) else {
                    // The first paint: no page to fetch it yet.
                    self.reply(id, Err(E::Unavailable));
                    return None;
                };
                let ticket = crate::cookies::issue(&sid, cookie, tx, id);
                Some(json!({
                    "t": "device", "id": id, "call": "set_cookie",
                    "url": format!("{}/_prism/cookie", self.base), "ticket": ticket,
                }))
            }
            C::Reconnect => {
                // Not during the first paint: there is no session to renew.
                self.inbox.as_ref()?;
                self.reconnect = true;
                Some(json!({ "t": "reconnect" }))
            }
            // #210: the server keeps the file, the page downloads it.
            C::SaveFile(f) => {
                let bytes = match f.read() {
                    Ok(b) => b.into_owned(),
                    Err(e) => {
                        self.reply(id, Err(E::Failed(e.to_string())));
                        return None;
                    }
                };
                let (name, mime, size) = (f.file_name(), f.mime_type(), bytes.len() as u64);
                let url = crate::downloads::offer(
                    &self.base,
                    self.sid.as_deref(),
                    name.clone(),
                    mime.clone(),
                    bytes,
                );
                // Answered now: the browser downloads it where it saves files.
                self.reply(
                    id,
                    Ok(V::Files(vec![prism_core::PickedFile {
                        path: String::new(),
                        name: name.clone(),
                        mime,
                        size,
                    }])),
                );
                Some(
                    json!({ "t": "device", "id": id, "call": "save_file", "url": url, "name": name }),
                )
            }
            // No file manager to show it in.
            C::RevealPath(_) => None,
            // #246: text types go as text; images and other bytes stay on
            // the server, the page fetches them from the session's URL.
            C::WriteClipboard(item) => {
                let entries: Vec<Value> = item
                    .entries()
                    .map(|(ty, data)| {
                        if prism_core::ClipboardItem::is_text(ty, data) {
                            json!({ "type": ty, "text": String::from_utf8_lossy(data) })
                        } else {
                            let url = crate::downloads::offer(
                                &self.base,
                                self.sid.as_deref(),
                                "clipboard".into(),
                                ty.to_string(),
                                data.to_vec(),
                            );
                            json!({ "type": ty, "url": url })
                        }
                    })
                    .collect();
                Some(
                    json!({ "t": "device", "id": id, "call": "write_clipboard", "entries": entries }),
                )
            }
            other => Some(device_json(id, other)),
        }
    }

    /// Starts a WebAuthn ceremony: an assertion with this browser's
    /// credential, or its enrollment the first time (`authenticate`, or the
    /// first biometric secret). A biometric secret is never read through an
    /// enrollment: without a credential it stays closed.
    fn ceremony(&mut self, id: u64, reason: String, gate: Gate) -> Option<Value> {
        use crate::security::Mode;
        let credential = self.secure.as_ref().and_then(|s| s.credential());
        let mode = match (&credential, &gate) {
            (Some(cred), _) => Mode::Assert(cred.id.clone()),
            (None, Gate::Secret(_)) => {
                self.reply(
                    id,
                    Err(prism_core::DeviceError::Failed(
                        "no authenticator is enrolled in this browser: store the secret again"
                            .into(),
                    )),
                );
                return None;
            }
            (None, _) => Mode::Register,
        };
        let c = crate::security::Ceremony::new(&self.origin, mode);
        let msg = match &c.mode {
            Mode::Assert(cred) => json!({
                "t": "device", "id": id, "call": "webauthn", "mode": "get",
                "challenge": c.challenge, "credential": cred, "reason": reason,
            }),
            Mode::Register => json!({
                "t": "device", "id": id, "call": "webauthn", "mode": "create",
                "challenge": c.challenge, "user": crate::security::random_id(),
                "rp": self.title, "name": self.title, "reason": reason,
            }),
        };
        self.ceremonies.insert(id, (c, gate));
        Some(msg)
    }

    /// Checks the page's WebAuthn answer for call `id`: the operation the
    /// server chose (an assertion by the enrolled credential, or the first
    /// enrollment), whatever the page says it did.
    pub(crate) fn finish_ceremony(&mut self, id: u64, v: &Value) -> Answer {
        use crate::security::Mode;
        use prism_core::{DeviceError as E, DeviceValue as V};
        let Some((c, gate)) = self.ceremonies.remove(&id) else {
            return Err(E::Failed("no ceremony".into()));
        };
        if let Some(err) = v.get("err").and_then(Value::as_str) {
            return Err(E::from_message(err));
        }
        let (Some(r), Some(store)) = (v.get("webauthn"), &self.secure) else {
            return Err(E::Failed("webauthn: no answer".into()));
        };
        let kind = r.get("type").and_then(Value::as_str);
        let checked = match (&c.mode, kind) {
            (Mode::Register, Some("create")) => {
                crate::security::verify_registration(&c, r).and_then(|cred| store.enroll(&cred))
            }
            (Mode::Assert(enrolled), Some("get")) => match store.credential() {
                // Still the credential the ceremony was started for.
                Some(mut cred) if cred.id == *enrolled => {
                    crate::security::verify_assertion(&c, &mut cred, r)
                        .and_then(|_| store.save_credential(&cred))
                }
                _ => Err("webauthn: the enrolled credential changed".into()),
            },
            _ => Err("webauthn: wrong ceremony".into()),
        };
        if let Err(e) = checked {
            return Err(E::from_message(&e));
        }
        match gate {
            Gate::Owner => Ok(V::Done),
            Gate::Secret(value) => Ok(V::Text(Some(value))),
            Gate::Store { key, value } => store
                .set(&key, &value, true)
                .map(|_| V::Done)
                .map_err(E::Failed),
        }
    }
}

/// A viewport report's `safe: [top, right, bottom, left]` (#28): zero when
/// absent or malformed.
fn safe_area(v: &Value) -> prism_core::Edges {
    let side = |i: usize| {
        v.get("safe")
            .and_then(|s| s.get(i))
            .and_then(Value::as_f64)
            .filter(|x| x.is_finite() && *x >= 0.)
            .unwrap_or(0.) as f32
    };
    prism_core::Edges {
        top: side(0),
        right: side(1),
        bottom: side(2),
        left: side(3),
    }
}
