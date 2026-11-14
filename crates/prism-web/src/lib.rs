//! # prism-web
//!
//! Runs a Prism app as a web server, LiveView-style:
//!
//! 1. `GET /` renders the app on the server and returns plain HTML + CSS
//!    (usable before any JS runs) plus a ~100-line JS client.
//! 2. The client opens `/ws`; the server starts a session (its own [`Runtime`])
//!    and streams [`Patch`]es as JSON; the client applies them to the DOM.
//! 3. DOM events go back over the socket and are dispatched to Rust handlers.
//!
//! No WASM: the app logic stays in native Rust on the server.

mod css;
mod glyphs;
mod html;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use prism_core::surfaces::{ActivityContent, PlatformRequest};
use prism_core::{
    AppConfig, AppInit, Backend, Capabilities, NodeId, NodeKind, Patch, PlatformServices, Runtime,
    UiEvent,
};
use serde_json::{Value, json};
use tungstenite::Message;

pub use css::css;
pub use html::HtmlBackend;

const CLIENT_JS: &str = include_str!("client.js");

/// Serves the app forever on `config.port` (or `PRISM_PORT`).
pub fn run(config: AppConfig, init: AppInit) {
    let port = std::env::var("PRISM_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(config.port);
    let addr = format!("127.0.0.1:{port}");
    let listener =
        TcpListener::bind(&addr).unwrap_or_else(|e| panic!("prism-web: cannot bind {addr}: {e}"));
    println!("prism-web: serving \"{}\" on http://{addr}", config.title);
    for stream in listener.incoming().flatten() {
        let init = init.clone();
        let config = config.clone();
        thread::spawn(move || {
            if let Err(err) = handle(stream, &config, &init) {
                eprintln!("prism-web: {err}");
            }
        });
    }
}

fn handle(stream: TcpStream, config: &AppConfig, init: &AppInit) -> std::io::Result<()> {
    // Peek at the request line so the WebSocket handshake still sees the full request.
    let mut buf = [0u8; 256];
    let n = stream.peek(&mut buf)?;
    let head = String::from_utf8_lossy(&buf[..n]);
    if head.starts_with("GET /ws ") || head.starts_with("GET /ws?") {
        session(stream, init);
        return Ok(());
    }

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut cookie = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("cookie")
        {
            cookie = session_id(value);
        }
    }
    // Each browser gets a stable id: its data lives in its own directory.
    let (sid, set_cookie) = match cookie {
        Some(sid) => (sid, String::new()),
        None => {
            let sid = new_session_id();
            let header = format!(
                "Set-Cookie: prism_sid={sid}; Path=/; Max-Age=31536000; SameSite=Lax; HttpOnly\r\n"
            );
            (sid, header)
        }
    };
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let (status, body, ctype) = match path {
        "/favicon.ico" => ("404 Not Found", "not found".to_string(), "text/plain"),
        // Every other path is an app route: render it on the server.
        path => (
            "200 OK",
            page(config, init, &percent_decode(path), &sid),
            "text/html; charset=utf-8",
        ),
    };
    let mut stream = stream;
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n{set_cookie}Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

/// `prism_sid` from a Cookie header, if well-formed.
fn session_id(cookie_header: &str) -> Option<String> {
    cookie_header
        .split(';')
        .filter_map(|kv| kv.trim().strip_prefix("prism_sid="))
        .find(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_string)
}

/// A random 128-bit id (process-random SipHash keys + time + counter).
fn new_session_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = std::collections::hash_map::RandomState::new();
    let part = |salt: u64| {
        let mut h = seed.build_hasher();
        h.write_u64(salt);
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        h.finish()
    };
    format!("{:016x}{:016x}", part(1), part(2))
}

/// Where a browser's `cx.storage()` lives (`PRISM_DATA_DIR`, default `.prism-data`).
fn storage_for(sid: &str) -> Box<dyn prism_core::Storage> {
    let base = std::env::var("PRISM_DATA_DIR").unwrap_or_else(|_| ".prism-data".into());
    Box::new(prism_core::FileStorage::new(
        std::path::Path::new(&base).join(sid),
    ))
}

/// Server-side render of a fresh session opened on `path`, as a full HTML document.
pub fn page(config: &AppConfig, init: &AppInit, path: &str, sid: &str) -> String {
    let mut runtime = Runtime::with_options(
        init,
        prism_core::RuntimeOptions {
            path: path.into(),
            capabilities: WebServices::default().capabilities(),
            storage: Some(storage_for(sid)),
            wake_ui: None,
        },
    );
    let mut html = HtmlBackend::default();
    html.apply(&runtime.render());
    let title = match html.screen_title() {
        Some(screen) if screen != config.title => format!("{screen} · {}", config.title),
        Some(_) => config.title.clone(),
        None => config.title.clone(),
    };
    format!(
        r#"<!doctype html>
<html lang="en" style="color-scheme: {scheme}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>
  :root {{ color-scheme: light dark; }}
  html, body {{ margin: 0; height: 100%; }}
  body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif; font-size: 16px; background: Canvas; color: CanvasText; }}
  #prism-root {{ display: flex; flex-direction: column; min-height: 100%; }}
  #prism-root > * {{ flex-grow: 1; }}
  #prism-root:has(> .prism-tabs) {{ height: 100%; }}
  .prism-panels > * {{ overflow: auto; }}
  [hidden] {{ display: none !important; }}
  :focus-visible {{ outline: 2px solid Highlight; outline-offset: 2px; }}
  .prism-title {{ margin: 0; font: inherit; }}
  .prism-scroll {{ overflow-y: auto; overscroll-behavior: contain; }}
  .prism-tabs {{ display: flex; flex-direction: column; }}
  .prism-panels {{ flex: 1; display: flex; flex-direction: column; min-height: 0; }}
  .prism-panels > * {{ flex: 1; display: flex; flex-direction: column; }}
  .prism-tabbar {{ flex: 0 0 auto; display: flex; justify-content: center; gap: 4px; padding: 6px; border-top: 1px solid rgba(128,128,128,.25); background: Canvas; }}
  .prism-tab {{ flex: 0 1 140px; display: flex; flex-direction: column; align-items: center; gap: 2px; padding: 4px 8px; border: 0; border-radius: 8px; background: none; color: GrayText; font: inherit; font-size: 12px; cursor: pointer; }}
  .prism-tab[aria-selected="true"] {{ color: LinkText; background: rgba(128,128,128,.12); }}
  .prism-tab-icon {{ font-size: 18px; line-height: 1; }}
  .prism-switch {{ appearance: none; width: 44px; height: 26px; margin: 0; border-radius: 13px; background: rgba(128,128,128,.35); position: relative; cursor: pointer; transition: background .15s; flex-shrink: 0; }}
  .prism-switch::after {{ content: ""; position: absolute; top: 2px; left: 2px; width: 22px; height: 22px; border-radius: 50%; background: white; box-shadow: 0 1px 3px rgba(0,0,0,.3); transition: transform .15s; }}
  .prism-switch:checked {{ background: #34c759; }}
  .prism-switch:checked::after {{ transform: translateX(18px); }}
  .prism-symbol {{ display: inline-flex; align-items: center; justify-content: center; line-height: 1; }}
  progress {{ width: 100%; accent-color: currentColor; }}
  #prism-island {{ position: fixed; top: 10px; left: 50%; transform: translateX(-50%); z-index: 1000; display: flex; flex-direction: column; gap: 8px; align-items: center; pointer-events: none; }}
  .prism-island {{ pointer-events: auto; min-width: 180px; max-width: min(92vw, 420px); padding: 8px 14px; border-radius: 22px; background: #000; color: #fff; box-shadow: 0 6px 24px rgba(0,0,0,.25); cursor: pointer; border: 0; font: inherit; text-align: left; }}
  .prism-island-compact {{ display: flex; justify-content: space-between; align-items: center; gap: 16px; }}
  .prism-island[aria-expanded="true"] {{ border-radius: 28px; padding: 14px 16px; }}
  #prism-toasts {{ position: fixed; bottom: 80px; right: 16px; z-index: 1000; display: flex; flex-direction: column; gap: 8px; }}
  .prism-toast {{ max-width: 340px; padding: 12px 14px; border-radius: 14px; background: Canvas; color: CanvasText; border: 1px solid rgba(128,128,128,.3); box-shadow: 0 6px 24px rgba(0,0,0,.18); cursor: pointer; text-align: left; font: inherit; }}
  .prism-toast strong {{ display: block; }}
  @media (prefers-reduced-motion: reduce) {{ * {{ transition: none !important; }} }}
  .prism-header {{ position: relative; flex: 0 0 44px; display: flex; align-items: center; justify-content: center; border-bottom: 1px solid rgba(128,128,128,.25); background: Canvas; font-size: 15px; font-weight: 600; }}
  .prism-back {{ position: absolute; left: 8px; top: 50%; transform: translateY(-50%); font: inherit; font-size: 14px; font-weight: 400; cursor: pointer; }}
  .prism-body {{ flex: 1; display: flex; flex-direction: column; min-height: 0; }}
</style>
</head>
<body>
<main id="prism-root">{body}</main>
<div id="prism-island" role="status" aria-live="polite"></div>
<div id="prism-toasts" role="status" aria-live="polite"></div>
<script>{js}</script>
</body>
</html>"#,
        title = html::escape(&title),
        scheme = html.color_scheme(),
        body = html.to_html(),
        js = CLIENT_JS.replace("__GLYPHS__", &glyphs::js_table()),
    )
}

#[allow(clippy::result_large_err)] // tungstenite's handshake callback signature
fn session(stream: TcpStream, init: &AppInit) {
    // The client passes its current URL: `/ws?path=/todos/1`.
    let mut path = String::from("/");
    let mut sid = None;
    let handshake = tungstenite::accept_hdr(
        stream,
        |req: &tungstenite::handshake::server::Request, resp| {
            sid = req
                .headers()
                .get("cookie")
                .and_then(|c| c.to_str().ok())
                .and_then(session_id);
            if let Some(p) = req
                .uri()
                .query()
                .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("path=")))
            {
                path = percent_decode(p);
            }
            Ok(resp)
        },
    );
    let Ok(mut ws) = handshake else {
        return;
    };
    // Async work wakes this loop (the socket read times out while tasks run).
    let woken = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = woken.clone();
    let mut runtime = Runtime::with_options(
        init,
        prism_core::RuntimeOptions {
            path,
            capabilities: WebServices::default().capabilities(),
            storage: Some(storage_for(&sid.unwrap_or_else(new_session_id))),
            wake_ui: Some(std::sync::Arc::new(move || {
                flag.store(true, std::sync::atomic::Ordering::SeqCst)
            })),
        },
    );
    let mut encoder = Encoder::default();
    let first = runtime.render();
    if send(&mut ws, &mut encoder, "reset", &first).is_err() {
        return;
    }
    let mut services = WebServices::default();
    loop {
        for request in runtime.take_requests() {
            services.handle(request);
        }
        for msg in services.outbox.drain(..) {
            if ws.send(Message::text(msg.to_string())).is_err() {
                return;
            }
        }
        // While tasks run, wake up at their next timer (or every 25 ms).
        let timeout = runtime.has_tasks().then(|| {
            let until_timer = runtime
                .next_timer()
                .map(|t| t.saturating_duration_since(std::time::Instant::now()));
            until_timer
                .unwrap_or(std::time::Duration::from_millis(25))
                .clamp(
                    std::time::Duration::from_millis(1),
                    std::time::Duration::from_millis(25),
                )
        });
        let _ = ws.get_ref().set_read_timeout(timeout);
        let read = ws.read();
        let due = runtime
            .next_timer()
            .is_some_and(|t| t <= std::time::Instant::now());
        if woken.swap(false, std::sync::atomic::Ordering::SeqCst) || due {
            let patches = runtime.poll();
            if !patches.is_empty() && send(&mut ws, &mut encoder, "patch", &patches).is_err() {
                return;
            }
        }
        let msg = match read {
            Ok(Message::Text(t)) => t,
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Ok(Message::Close(_)) | Err(_) => return,
            Ok(_) => continue,
        };
        // A notification or live banner was tapped: open its deep link.
        if let Ok(v) = serde_json::from_str::<Value>(&msg)
            && v.get("t").and_then(Value::as_str) == Some("link")
            && let Some(path) = v.get("path").and_then(Value::as_str)
        {
            let patches = runtime.open_link(path);
            if !patches.is_empty() && send(&mut ws, &mut encoder, "patch", &patches).is_err() {
                return;
            }
            continue;
        }
        let Some(event) = parse_event(&msg) else {
            continue;
        };
        let patches = runtime.dispatch(event);
        if !patches.is_empty() && send(&mut ws, &mut encoder, "patch", &patches).is_err() {
            return;
        }
    }
}

/// Web implementation of system surfaces: browser notifications (with an
/// in-page toast fallback) and Live Activities as an in-page live banner.
/// Widgets do not exist on the web.
#[derive(Default)]
struct WebServices {
    outbox: Vec<Value>,
}

fn activity_json(content: &ActivityContent) -> Value {
    serde_json::from_str(&content.to_json()).unwrap_or(Value::Null)
}

impl PlatformServices for WebServices {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            widgets: false,
            live_activities: true,
            dynamic_island: false,
            notifications: true,
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
            }),
            PlatformRequest::CancelNotification(id) => json!({ "t": "notify_cancel", "id": id }),
            PlatformRequest::RequestNotificationPermission => json!({ "t": "notify_permission" }),
        };
        self.outbox.push(msg);
    }
}

fn parse_event(msg: &str) -> Option<UiEvent> {
    let v: Value = serde_json::from_str(msg).ok()?;
    let id = v.get("id")?.as_u64()?;
    match v.get("t")?.as_str()? {
        "click" => Some(UiEvent::Click(id)),
        "input" => Some(UiEvent::Input(id, v.get("v")?.as_str()?.to_string())),
        "back" => Some(UiEvent::Back(id, v.get("depth")?.as_u64()? as usize)),
        "change" => Some(UiEvent::Change(id, v.get("v")?.as_f64()?)),
        "navigate" => Some(UiEvent::Navigate(id, v.get("path")?.as_str()?.to_string())),
        _ => None,
    }
}

fn send(
    ws: &mut tungstenite::WebSocket<TcpStream>,
    encoder: &mut Encoder,
    kind: &str,
    patches: &[Patch],
) -> tungstenite::Result<()> {
    let msg = json!({ "t": kind, "p": encoder.encode(patches) });
    ws.send(Message::text(msg.to_string()))
}

/// Encodes patches to the JSON wire format. Styles are pre-rendered to CSS on
/// the server, which needs each node's kind, so the encoder remembers them.
#[derive(Default)]
pub struct Encoder {
    kinds: HashMap<NodeId, NodeKind>,
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
            Patch::SetText { id, text } => json!({ "op": "text", "id": id, "text": text }),
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

/// Decodes `%XX` escapes (URL paths and query values).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
