//! S3 (security review): live sessions are bounded. A real Axum server on
//! a loopback port, spoken to over plain TCP: concurrent handshakes take at
//! most the configured permits, an oversized message or a page that stops
//! reading ends its session and frees its permit, and a healthy page still
//! renders afterwards.
#![cfg(feature = "server")]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use prism_core::prelude::*;
use prism_core::{AppConfig, AppInit};
use prism_web::WebApp;

/// A button whose every click adds a large text: the page must read it.
struct Big {
    clicks: usize,
}

impl Render for Big {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(button("more").on_click(cx.listener(|this: &mut Self, _, _| this.clicks += 1)))
            .child(text(format!("{}{}", self.clicks, "x".repeat(1 << 20))))
    }
}

fn app(tag: &str, limit: impl FnOnce(WebApp) -> WebApp) -> WebApp {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Big { clicks: 0 });
        cx.set_root(root);
    });
    let dir = std::env::temp_dir().join(format!("prism-limits-{tag}-{}", std::process::id()));
    let config = AppConfig {
        title: "Limits".into(),
        ..Default::default()
    };
    limit(WebApp::new(config, init).data_dir(dir))
}

/// Serves `app` on a free loopback port; returns `host:port`.
fn start(app: WebApp) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap().to_string()).unwrap();
            axum::serve(listener, app.router::<()>()).await.unwrap();
        });
    });
    rx.recv_timeout(Duration::from_secs(10)).unwrap()
}

/// Opens the socket: the handshake's status, and the stream.
fn handshake(addr: &str) -> (u16, TcpStream) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    write!(
        s,
        "GET /ws?path=%2F&w=800&h=600 HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://{addr}\r\n\
Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    )
    .unwrap();
    let mut raw = Vec::new();
    let mut byte = [0u8];
    while !raw.ends_with(b"\r\n\r\n") {
        if s.read(&mut byte).unwrap() == 0 {
            break;
        }
        raw.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&raw);
    let status = head.split(' ').nth(1).unwrap_or("0").parse().unwrap_or(0);
    (status, s)
}

/// One frame from the server: its opcode and payload.
fn frame(s: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut h = [0u8; 2];
    s.read_exact(&mut h).unwrap();
    let len = match h[1] & 0x7f {
        126 => {
            let mut b = [0u8; 2];
            s.read_exact(&mut b).unwrap();
            u16::from_be_bytes(b) as usize
        }
        127 => {
            let mut b = [0u8; 8];
            s.read_exact(&mut b).unwrap();
            u64::from_be_bytes(b) as usize
        }
        n => n as usize,
    };
    let mut payload = vec![0u8; len];
    s.read_exact(&mut payload).unwrap();
    (h[0] & 0x0f, payload)
}

/// The next text frame (pings skipped).
fn next_text(s: &mut TcpStream) -> String {
    loop {
        let (op, payload) = frame(s);
        if op == 1 {
            return String::from_utf8(payload).unwrap();
        }
    }
}

/// A masked text frame from the page.
fn send(s: &mut TcpStream, text: &str) -> std::io::Result<()> {
    let mut f = vec![0x81u8];
    let len = text.len();
    if len < 126 {
        f.push(0x80 | len as u8);
    } else if len < 65536 {
        f.push(0x80 | 126);
        f.extend((len as u16).to_be_bytes());
    } else {
        f.push(0x80 | 127);
        f.extend((len as u64).to_be_bytes());
    }
    let mask = [1u8, 2, 3, 4];
    f.extend(mask);
    f.extend(text.bytes().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    s.write_all(&f)
}

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ok() {
        assert!(start.elapsed() < Duration::from_secs(20), "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The id of the button labelled `more` in a reset message.
fn more_button(reset: &str) -> u64 {
    let v: serde_json::Value = serde_json::from_str(reset).unwrap();
    v["p"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["op"] == "text" && p["text"] == "more")
        .and_then(|p| p["id"].as_u64())
        .unwrap()
}

#[test]
fn concurrent_handshakes_take_at_most_the_permits() {
    let app = app("permits", |a| a.max_sessions(2));
    let addr = start(app.clone());
    let handshakes: Vec<_> = (0..8)
        .map(|_| {
            let addr = addr.clone();
            std::thread::spawn(move || handshake(&addr))
        })
        .collect();
    let answers: Vec<(u16, TcpStream)> =
        handshakes.into_iter().map(|h| h.join().unwrap()).collect();
    let upgraded = answers.iter().filter(|(status, _)| *status == 101).count();
    assert_eq!(
        upgraded,
        2,
        "{:?}",
        answers.iter().map(|a| a.0).collect::<Vec<_>>()
    );
    assert!(answers.iter().all(|(s, _)| *s == 101 || *s == 503));
    // Closing them frees the permits: a new page renders.
    drop(answers);
    wait_for("the permits are released", || app.session_count() == 0);
    let (status, mut ws) = handshake(&addr);
    assert_eq!(status, 101);
    assert!(next_text(&mut ws).contains("\"reset\""));
}

#[test]
fn an_oversized_message_ends_its_session() {
    let app = app("oversized", |a| a);
    let addr = start(app.clone());
    let (status, mut ws) = handshake(&addr);
    assert_eq!(status, 101);
    next_text(&mut ws);
    // Over the default limit (8 MiB).
    let _ = send(&mut ws, &format!("\"{}\"", "a".repeat(9 << 20)));
    wait_for("the session ends", || app.session_count() == 0);
    let (status, mut ws) = handshake(&addr);
    assert_eq!(status, 101);
    assert!(next_text(&mut ws).contains("\"reset\""));
}

#[test]
fn a_page_that_stops_reading_is_disconnected() {
    let app = app("stalled", |a| a);
    let addr = start(app.clone());
    let (status, mut ws) = handshake(&addr);
    assert_eq!(status, 101);
    let more = more_button(&next_text(&mut ws));
    // Each click sends a megabyte back; the page never reads it. Beyond
    // the default send queue (32 MiB) the session ends.
    let click = format!(r#"{{"t":"click","id":{more}}}"#);
    ws.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
    for _ in 0..200 {
        if send(&mut ws, &click).is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    wait_for("the stalled session ends", || app.session_count() == 0);
    let (status, mut ws) = handshake(&addr);
    assert_eq!(status, 101);
    assert!(next_text(&mut ws).contains("\"reset\""));
}
