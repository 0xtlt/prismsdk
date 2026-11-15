//! Prism mounted inside a host Axum server with its own middleware: the
//! host signs people in (a cookie), refuses the signed-out, attaches the
//! user and a CSP nonce to each request, and sends a strict CSP header.
//! A real server on a loopback port, spoken to over plain TCP.
#![cfg(feature = "server")]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use prism_core::prelude::*;
use prism_core::{AppConfig, AppInit};
use prism_web::{CspNonce, RequestContext, WebApp};

#[derive(Debug)]
struct User {
    id: u32,
}

struct Home;

impl Render for Home {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let who = cx
            .request_context::<User>()
            .map_or("nobody".into(), |u| format!("user {}", u.id));
        screen("Home")
            .child(text(who).p(4.))
            .child(text(format!("at {}", cx.page_url().unwrap_or("-"))))
    }
}

fn prism(dir: &std::path::Path) -> WebApp {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let router = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| Home))
            .build(cx);
        cx.set_root(router);
    });
    let config = AppConfig {
        title: "Dashboard".into(),
        ..Default::default()
    };
    WebApp::new(config, init)
        .data_dir(dir)
        .base_path("/app")
        .theme_color("#123456")
}

/// The host's own sign-in: `host_session=<user id>`, else 401.
async fn sign_in(mut req: Request, next: Next) -> Response {
    let user = req
        .headers()
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|c| {
            c.split(';')
                .find_map(|kv| kv.trim().strip_prefix("host_session="))
        })
        .and_then(|id| id.parse().ok());
    let Some(id) = user else {
        return (axum::http::StatusCode::UNAUTHORIZED, "sign in first").into_response();
    };
    let nonce = CspNonce::random();
    req.extensions_mut()
        .insert(RequestContext::new(User { id }));
    req.extensions_mut().insert(nonce.clone());
    let mut res = next.run(req).await;
    let csp = format!(
        "default-src 'self'; style-src 'self' 'nonce-{0}'; script-src 'self' 'nonce-{0}'; img-src 'self' data:; connect-src 'self'",
        nonce.0
    );
    res.headers_mut()
        .insert("content-security-policy", csp.parse().unwrap());
    res
}

/// Starts the host on a free port; returns `host:port`.
fn start(dir: &std::path::Path) -> String {
    let prism = prism(dir);
    let host = axum::Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .nest("/app", prism.router())
        .layer(middleware::from_fn(sign_in));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap().to_string()).unwrap();
            axum::serve(listener, host).await.unwrap();
        });
    });
    rx.recv_timeout(Duration::from_secs(10)).unwrap()
}

struct Answer {
    status: u16,
    head: String,
    body: String,
}

impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }
}

fn parse(raw: &[u8]) -> Answer {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    Answer {
        status,
        head: head.to_string(),
        body: body.to_string(),
    }
}

fn get_page(addr: &str, path: &str, cookie: Option<&str>) -> Answer {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let cookie = cookie.map_or(String::new(), |c| format!("Cookie: {c}\r\n"));
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\n{cookie}Connection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    parse(&raw)
}

/// Opens the socket: the handshake's answer, and the stream when upgraded.
fn handshake(addr: &str, origin: &str, cookie: Option<&str>) -> (Answer, TcpStream) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let cookie = cookie.map_or(String::new(), |c| format!("Cookie: {c}\r\n"));
    write!(
        s,
        "GET /app/ws?path=%2F&search=%3Fq%3D1&w=800&h=600 HTTP/1.1\r\nHost: {addr}\r\nOrigin: {origin}\r\n{cookie}\
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
    (parse(&raw), s)
}

/// One text frame from the server (unmasked).
fn frame(s: &mut TcpStream) -> String {
    let mut h = [0u8; 2];
    s.read_exact(&mut h).unwrap();
    assert_eq!(h[0] & 0x0f, 1, "a text frame");
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
    String::from_utf8(payload).unwrap()
}

#[test]
fn prism_nested_in_a_host_with_its_own_middleware() {
    let dir = std::env::temp_dir().join(format!("prism-axum-host-{}", std::process::id()));
    let addr = start(&dir);
    let me = Some("host_session=42");

    // The host's own routes come first.
    let health = get_page(&addr, "/api/health", me);
    assert_eq!((health.status, health.body.as_str()), (200, "ok"));

    // A page: rendered for the signed-in user, with the host's nonce on
    // every tag and no style attribute (the strict CSP allows it).
    let page = get_page(&addr, "/app?tab=1", me);
    assert_eq!(page.status, 200, "{}", page.head);
    assert!(page.body.contains("user 42"), "{}", page.body);
    assert!(
        page.body.contains(&format!("at http://{addr}/app?tab=1")),
        "{}",
        page.body
    );
    let csp = page.header("content-security-policy").unwrap();
    let nonce = csp
        .split("'nonce-")
        .nth(1)
        .unwrap()
        .split('\'')
        .next()
        .unwrap();
    for tag in [
        r#"<style id="prism-page""#,
        r#"<style id="prism-ssr""#,
        r#"<link rel="stylesheet""#,
        r#"<script src=""#,
    ] {
        let at = page.body.find(tag).unwrap_or_else(|| panic!("{tag}"));
        let end = at + page.body[at..].find('>').unwrap();
        assert!(
            page.body[at..end].contains(&format!(r#"nonce="{nonce}""#)),
            "{tag}"
        );
    }
    assert!(!page.body.contains("style="));
    assert!(
        page.body
            .contains(r##"<meta name="theme-color" content="#123456">"##)
    );

    // An unknown path: the not-found screen, with 404.
    assert_eq!(get_page(&addr, "/app/nope/nope", me).status, 404);
    // Signed out: the host refuses the page and the socket.
    assert_eq!(get_page(&addr, "/app", None).status, 401);
    let (refused, _) = handshake(&addr, &format!("http://{addr}"), None);
    assert_eq!(refused.status, 401);

    // A foreign page cannot open a session with the user's cookie.
    let (foreign, _) = handshake(&addr, "https://evil.example", me);
    assert_eq!(foreign.status, 403, "{}", foreign.head);

    // The live session sees the handshake's context and the page's URL.
    let (ok, mut ws) = handshake(&addr, &format!("http://{addr}"), me);
    assert_eq!(ok.status, 101, "{}", ok.head);
    let reset: serde_json::Value = serde_json::from_str(&frame(&mut ws)).unwrap();
    assert_eq!(reset["t"], "reset");
    let texts: Vec<&str> = reset["p"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["text"].as_str())
        .collect();
    assert!(texts.contains(&"user 42"), "{texts:?}");
    let url = format!("at http://{addr}/app?q=1");
    assert!(texts.contains(&url.as_str()), "{texts:?}");
    let _ = std::fs::remove_dir_all(dir);
}
