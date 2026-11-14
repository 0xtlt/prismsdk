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
mod html;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use prism_core::{AppConfig, AppInit, Backend, NodeId, NodeKind, Patch, Runtime, UiEvent};
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
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let (status, body, ctype) = match path {
        "/" => ("200 OK", page(config, init), "text/html; charset=utf-8"),
        _ => ("404 Not Found", "not found".to_string(), "text/plain"),
    };
    let mut stream = stream;
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

/// Server-side render of a fresh session, as a full HTML document.
pub fn page(config: &AppConfig, init: &AppInit) -> String {
    let mut runtime = Runtime::new(init);
    let mut html = HtmlBackend::default();
    html.apply(&runtime.render());
    format!(
        r#"<!doctype html>
<html lang="en">
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
</style>
</head>
<body>
<div id="prism-root">{body}</div>
<script>{js}</script>
</body>
</html>"#,
        title = html::escape(&config.title),
        body = html.to_html(),
        js = CLIENT_JS,
    )
}

fn session(stream: TcpStream, init: &AppInit) {
    let Ok(mut ws) = tungstenite::accept(stream) else {
        return;
    };
    let mut runtime = Runtime::new(init);
    let mut encoder = Encoder::default();
    let first = runtime.render();
    if send(&mut ws, &mut encoder, "reset", &first).is_err() {
        return;
    }
    loop {
        let msg = match ws.read() {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) | Err(_) => return,
            Ok(_) => continue,
        };
        let Some(event) = parse_event(&msg) else {
            continue;
        };
        let patches = runtime.dispatch(event);
        if !patches.is_empty() && send(&mut ws, &mut encoder, "patch", &patches).is_err() {
            return;
        }
    }
}

fn parse_event(msg: &str) -> Option<UiEvent> {
    let v: Value = serde_json::from_str(msg).ok()?;
    let id = v.get("id")?.as_u64()?;
    match v.get("t")?.as_str()? {
        "click" => Some(UiEvent::Click(id)),
        "input" => Some(UiEvent::Input(id, v.get("v")?.as_str()?.to_string())),
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
            Patch::SetRoot { id } => json!({ "op": "root", "id": id }),
            Patch::Destroy { id } => {
                self.kinds.remove(id);
                json!({ "op": "destroy", "id": id })
            }
        }
    }
}
