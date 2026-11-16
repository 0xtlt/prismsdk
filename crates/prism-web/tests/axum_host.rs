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
            // A first tree large enough to be sent compressed.
            .child(text("Lorem ipsum dolor sit amet. ".repeat(60)))
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
    handshake_with(addr, "", origin, cookie)
}

/// [`handshake`] with more query parameters (`&compress=deflate-raw`).
fn handshake_with(
    addr: &str,
    query: &str,
    origin: &str,
    cookie: Option<&str>,
) -> (Answer, TcpStream) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let cookie = cookie.map_or(String::new(), |c| format!("Cookie: {c}\r\n"));
    write!(
        s,
        "GET /app/ws?path=%2F&search=%3Fq%3D1&w=800&h=600{query} HTTP/1.1\r\nHost: {addr}\r\nOrigin: {origin}\r\n{cookie}\
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
    let (opcode, payload) = raw_frame(s);
    assert_eq!(opcode, 1, "a text frame");
    String::from_utf8(payload).unwrap()
}

/// One frame from the server (unmasked): its opcode and payload.
fn raw_frame(s: &mut TcpStream) -> (u8, Vec<u8>) {
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

/// A GET with extra headers: the answer's head and its raw body.
fn get_raw(addr: &str, path: &str, headers: &[(&str, &str)]) -> (Answer, Vec<u8>) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let extra: String = headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}\r\n"))
        .collect();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nCookie: host_session=42\r\n{extra}Connection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let at = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let body = raw.split_off(at);
    (parse(&raw), body)
}

fn gunzip(z: &[u8]) -> String {
    let mut s = String::new();
    flate2::read::GzDecoder::new(z)
        .read_to_string(&mut s)
        .unwrap();
    s
}

#[test]
fn pages_files_and_socket_messages_are_compressed() {
    let dir = std::env::temp_dir().join(format!("prism-axum-gzip-{}", std::process::id()));
    let addr = start(&dir);
    let gzip = ("Accept-Encoding", "gzip, deflate, br, zstd");

    // A page: gzipped on the fly, and it varies with Accept-Encoding.
    let (page, body) = get_raw(&addr, "/app", &[gzip]);
    assert_eq!(page.status, 200, "{}", page.head);
    assert_eq!(page.header("content-encoding"), Some("gzip"));
    assert_eq!(page.header("vary"), Some("accept-encoding"));
    let html = gunzip(&body);
    assert!(html.contains("user 42"));
    let (plain, plain_body) = get_raw(&addr, "/app", &[]);
    assert_eq!(plain.header("content-encoding"), None);
    assert_eq!(plain.header("vary"), Some("accept-encoding"));
    assert!(String::from_utf8(plain_body).unwrap().contains("user 42"));

    // The client script: gzipped once, with the ETag of its gzipped form.
    let src = html.split(r#"<script src=""#).nth(1).unwrap();
    let src = &src[..src.find('"').unwrap()];
    let (js, js_body) = get_raw(&addr, src, &[gzip]);
    assert_eq!(js.header("content-encoding"), Some("gzip"));
    assert_eq!(js.header("vary"), Some("accept-encoding"));
    let etag = js.header("etag").unwrap().to_string();
    assert!(etag.ends_with("-gzip\""), "{etag}");
    assert!(gunzip(&js_body).contains("DecompressionStream"));
    // Revalidated with either form's ETag: 304, nothing sent.
    let (again, again_body) = get_raw(&addr, src, &[gzip, ("If-None-Match", &etag)]);
    assert_eq!(again.status, 304);
    assert_eq!(again.header("etag"), Some(etag.as_str()));
    assert!(again_body.is_empty());
    let (identity, _) = get_raw(&addr, src, &[]);
    assert_eq!(identity.header("content-encoding"), None);
    let plain_etag = identity.header("etag").unwrap();
    assert_eq!(
        format!("{}-gzip\"", &plain_etag[..plain_etag.len() - 1]),
        etag
    );
    let weak = format!("W/{etag}");
    let (weak, _) = get_raw(&addr, src, &[("If-None-Match", &weak)]);
    assert_eq!(weak.status, 304);

    // The socket: a page that can inflate gets its first tree deflated.
    let me = Some("host_session=42");
    let origin = format!("http://{addr}");
    let (ok, mut ws) = handshake_with(&addr, "&compress=deflate-raw", &origin, me);
    assert_eq!(ok.status, 101, "{}", ok.head);
    let (opcode, payload) = raw_frame(&mut ws);
    assert_eq!(opcode, 2, "a binary frame");
    let mut json = String::new();
    flate2::read::DeflateDecoder::new(payload.as_slice())
        .read_to_string(&mut json)
        .unwrap();
    assert!(payload.len() < json.len());
    let reset: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(reset["t"], "reset");
    // Without it: text, as before.
    let (_, mut ws) = handshake(&addr, &origin, me);
    assert!(serde_json::from_str::<serde_json::Value>(&frame(&mut ws)).is_ok());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_cookie_endpoint_is_reached_through_the_host_router() {
    let dir = std::env::temp_dir().join(format!("prism-axum-cookie-{}", std::process::id()));
    let addr = start(&dir);
    let post = |ticket: &str| {
        let mut s = TcpStream::connect(&addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        write!(
            s,
            "POST /app/_prism/cookie HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://{addr}\r\n\
             Sec-Fetch-Site: same-origin\r\nCookie: host_session=42; prism_sid=0123456789abcdef0123456789abcdef\r\n\
             X-Prism-Cookie: {ticket}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).unwrap();
        parse(&raw)
    };
    // Prism's answer (not the router's 405): an unknown ticket, refused.
    let r = post("not-a-ticket");
    assert_eq!(r.status, 403, "{}", r.head);
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert!(r.header("set-cookie").is_none());
    let _ = std::fs::remove_dir_all(dir);
}
