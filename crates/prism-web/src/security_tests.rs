//! Regression tests for the pre-release security review: each test shows
//! the attack a finding describes and checks that it now fails.

use prism_core::surfaces::PlatformRequest;
use prism_core::{
    AuthRequest, DeviceCall, DeviceError, DeviceValue, PlatformServices, SecureOptions,
};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::security::{SecureStore, base64url, base64url_decode};
use crate::session::WebServices;

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "prism-sec-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

// --- S1: WebAuthn ceremonies ----------------------------------------------------------------

const ORIGIN: &str = "http://localhost:3000";

/// A platform authenticator: one P-256 key and its credential id.
struct Authenticator {
    id: &'static str,
    pair: EcdsaKeyPair,
}

impl Authenticator {
    fn new(id: &'static str) -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        Authenticator { id, pair }
    }

    fn spki(&self) -> Vec<u8> {
        let mut spki = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
        ];
        spki.extend_from_slice(self.pair.public_key().as_ref());
        spki
    }

    /// authenticatorData: RP id hash, flags (UP, UV, and AT when
    /// `attested`), the counter, then the attested credential's id.
    fn auth_data(&self, count: u32, attested: bool) -> Vec<u8> {
        let mut a = Sha256::digest(b"localhost").to_vec();
        a.push(if attested { 0x45 } else { 0x05 });
        a.extend(count.to_be_bytes());
        if attested {
            a.extend([0u8; 16]); // AAGUID
            a.extend((self.id.len() as u16).to_be_bytes());
            a.extend(self.id.as_bytes());
        }
        a
    }

    fn client(kind: &str, challenge: &str) -> Vec<u8> {
        json!({ "type": kind, "challenge": challenge, "origin": ORIGIN })
            .to_string()
            .into_bytes()
    }

    /// What the page answers to `navigator.credentials.create`.
    fn register(&self, challenge: &str) -> Value {
        json!({ "webauthn": {
            "type": "create", "id": base64url(self.id.as_bytes()), "alg": -7,
            "key": base64url(&self.spki()),
            "client": base64url(&Self::client("webauthn.create", challenge)),
            "auth": base64url(&self.auth_data(0, true)),
        }})
    }

    /// What the page answers to `navigator.credentials.get`.
    fn assert(&self, challenge: &str, count: u32) -> Value {
        let auth = self.auth_data(count, false);
        let client = Self::client("webauthn.get", challenge);
        let mut signed = auth.clone();
        signed.extend_from_slice(&Sha256::digest(&client));
        let sig = self.pair.sign(&SystemRandom::new(), &signed).unwrap();
        json!({ "webauthn": {
            "type": "get", "id": base64url(self.id.as_bytes()),
            "client": base64url(&client), "auth": base64url(&auth),
            "sig": base64url(sig.as_ref()),
        }})
    }
}

/// A browser session with a platform authenticator.
fn services(dir: &std::path::Path) -> WebServices {
    let mut s = WebServices::new(None);
    s.secure = Some(SecureStore::new(dir, "sid-1", Ok([7u8; 32])));
    s.origin = ORIGIN.into();
    s.uvpa = true;
    s
}

/// Sends a device call; returns the page's message (if any).
fn call(s: &mut WebServices, id: u64, call: DeviceCall) -> Option<Value> {
    s.handle(PlatformRequest::Device { id, call });
    s.take_outbox().pop()
}

fn get_secret(key: &str) -> DeviceCall {
    DeviceCall::SecureGet {
        key: key.into(),
        prompt: None,
        shared: false,
    }
}

fn set_secret(key: &str, value: &str) -> DeviceCall {
    DeviceCall::SecureSet {
        key: key.into(),
        value: value.into(),
        options: SecureOptions::new().biometric(),
    }
}

fn enrolled_id(s: &WebServices) -> Option<String> {
    let cred = s.secure.as_ref()?.credential()?;
    Some(String::from_utf8(base64url_decode(&cred.id)?).unwrap())
}

/// Enrolls `a` through the first biometric secret, as an app would.
fn enroll(s: &mut WebServices, a: &Authenticator) {
    let msg = call(s, 1, set_secret("pin", "1234")).expect("a registration");
    assert_eq!(msg["mode"], "create", "{msg}");
    let challenge = msg["challenge"].as_str().unwrap().to_string();
    assert_eq!(
        s.finish_ceremony(1, &a.register(&challenge)),
        Ok(DeviceValue::Done)
    );
    assert_eq!(enrolled_id(s).as_deref(), Some(a.id));
}

/// S1: a protected read asks for an assertion by the enrolled credential;
/// a registration answered instead neither replaces it nor opens the gate.
#[test]
fn s1_a_registration_cannot_answer_a_protected_read() {
    let dir = scratch("s1-read");
    let mut s = services(&dir);
    let owner = Authenticator::new("owner");
    let intruder = Authenticator::new("intruder");
    enroll(&mut s, &owner);

    let msg = call(&mut s, 2, get_secret("pin")).expect("an assertion");
    assert_eq!(msg["mode"], "get", "{msg}");
    let challenge = msg["challenge"].as_str().unwrap().to_string();
    let answer = s.finish_ceremony(2, &intruder.register(&challenge));
    assert!(answer.is_err(), "the gate opened: {answer:?}");
    assert_eq!(enrolled_id(&s).as_deref(), Some("owner"));

    // The owner's assertion still opens it.
    let msg = call(&mut s, 3, get_secret("pin")).unwrap();
    let challenge = msg["challenge"].as_str().unwrap().to_string();
    assert_eq!(
        s.finish_ceremony(3, &owner.assert(&challenge, 1)),
        Ok(DeviceValue::Text(Some("1234".into())))
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// S1: `authenticate` once a credential is enrolled is an assertion too.
#[test]
fn s1_a_registration_cannot_answer_authenticate() {
    let dir = scratch("s1-auth");
    let mut s = services(&dir);
    let owner = Authenticator::new("owner");
    let intruder = Authenticator::new("intruder");
    enroll(&mut s, &owner);
    let msg = call(&mut s, 2, DeviceCall::Authenticate(AuthRequest::new("Pay"))).unwrap();
    let challenge = msg["challenge"].as_str().unwrap().to_string();
    assert!(
        s.finish_ceremony(2, &intruder.register(&challenge))
            .is_err()
    );
    assert_eq!(enrolled_id(&s).as_deref(), Some("owner"));
    let _ = std::fs::remove_dir_all(dir);
}

/// S27: WebAuthn's user verification may be a PIN or a password, and the
/// page cannot tell: a biometrics-only request is unavailable on the web.
#[test]
fn s27_biometrics_only_is_unavailable_on_the_web() {
    let dir = scratch("s27");
    let mut s = services(&dir);
    let only = DeviceCall::Authenticate(AuthRequest::new("Pay").biometrics_only());
    let msg = call(&mut s, 1, only);
    assert!(msg.is_none(), "asked the page: {msg:?}");
    assert_eq!(s.local.pop(), Some((1, Err(DeviceError::Unavailable))));
    // With the passcode fallback, the ceremony runs.
    let msg = call(&mut s, 2, DeviceCall::Authenticate(AuthRequest::new("Pay"))).unwrap();
    assert_eq!(msg["mode"], "create");
    let _ = std::fs::remove_dir_all(dir);
}

/// S1: two enrollments started together: the first one wins, the second
/// fails without replacing it.
#[test]
fn s1_concurrent_enrollments_keep_the_first() {
    let dir = scratch("s1-race");
    let mut s = services(&dir);
    let (a, b) = (Authenticator::new("first"), Authenticator::new("second"));
    let auth = || DeviceCall::Authenticate(AuthRequest::new("Unlock"));
    let one = call(&mut s, 1, auth()).unwrap();
    let two = call(&mut s, 2, auth()).unwrap();
    assert_eq!(
        (one["mode"].as_str(), two["mode"].as_str()),
        (Some("create"), Some("create"))
    );
    let c1 = one["challenge"].as_str().unwrap().to_string();
    let c2 = two["challenge"].as_str().unwrap().to_string();
    assert_eq!(
        s.finish_ceremony(1, &a.register(&c1)),
        Ok(DeviceValue::Done)
    );
    assert!(s.finish_ceremony(2, &b.register(&c2)).is_err());
    assert_eq!(enrolled_id(&s).as_deref(), Some("first"));
    let _ = std::fs::remove_dir_all(dir);
}

/// S1: a biometric secret is stored once this browser's owner enrolled, so
/// reading one never enrolls whoever reads it first.
#[test]
fn s1_reading_a_biometric_secret_never_enrolls() {
    let dir = scratch("s1-tofu");
    let mut s = services(&dir);
    // Stored behind the server's back (or the credential was lost).
    s.secure.as_ref().unwrap().set("pin", "1234", true).unwrap();
    let msg = call(&mut s, 1, get_secret("pin"));
    assert!(msg.is_none(), "asked the page: {msg:?}");
    let (id, answer) = s.local.pop().unwrap();
    assert_eq!(id, 1);
    assert!(matches!(answer, Err(DeviceError::Failed(_))), "{answer:?}");
    // Storing one without an authenticator: unavailable, as on read.
    s.uvpa = false;
    assert!(call(&mut s, 2, set_secret("other", "x")).is_none());
    assert_eq!(s.local.pop(), Some((2, Err(DeviceError::Unavailable))));
    let _ = std::fs::remove_dir_all(dir);
}

// --- S2: nested push payloads -----------------------------------------------------------------

/// S2: a push payload nested far deeper than any real one is refused like
/// malformed JSON; the session (and the next one) keep working.
#[test]
fn s2_a_deeply_nested_push_payload_is_refused() {
    use crate::tests::{Page, counter_init, op_with_text, web_app};
    let (app, dir) = web_app(counter_init(), "s2");
    let page = Page::open(&app, "/ws?path=%2F&w=1280&h=800");
    let reset = page.next();
    let count = op_with_text(&reset["p"], "Count: 0");
    let plus = op_with_text(&reset["p"], "+1");
    let deep = format!(
        "{{\"title\":\"hi\",\"data\":{}{}}}",
        "[".repeat(200_000),
        "]".repeat(200_000)
    );
    page.send(json!({ "t": "push", "event": "received", "payload": deep }));
    page.send(json!({ "t": "click", "id": plus }));
    assert_eq!(
        page.next()["p"],
        json!([{ "op": "text", "id": count, "text": "Count: 1" }])
    );
    let second = Page::open(&app, "/ws?path=%2F&w=1280&h=800");
    assert_eq!(second.next()["t"], "reset");
    let _ = std::fs::remove_dir_all(dir);
}

// --- S3: session limits on any transport ----------------------------------------------------

/// A button whose click keeps the session busy for a while.
struct Slow;

impl prism_core::prelude::Render for Slow {
    fn render(
        &mut self,
        cx: &mut prism_core::prelude::Context<Self>,
    ) -> impl prism_core::prelude::IntoElement {
        use prism_core::prelude::*;
        div().child(button("slow").on_click(cx.listener(|_: &mut Self, _, _| {
            std::thread::sleep(std::time::Duration::from_millis(500))
        })))
    }
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !ok() {
        assert!(start.elapsed().as_secs() < 10, "{what}");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// S3: `WebApp::connect` (any WebSocket stack) enforces the message size
/// and the backlog a session lets build up, and frees its permit.
#[test]
fn s3_connect_bounds_message_size_and_backlog() {
    use crate::tests::{Page, op_with_text};
    let init: prism_core::AppInit = std::sync::Arc::new(|cx: &mut prism_core::App| {
        let root = cx.new_entity(|_| Slow);
        cx.set_root(root);
    });
    let dir = scratch("s3");
    let app = crate::WebApp::new(prism_core::AppConfig::default(), init)
        .data_dir(dir.join("data"))
        .max_message_size(1024)
        .max_queued_bytes(4096)
        .max_sessions(1);

    let page = Page::open(&app, "/ws?path=%2F");
    page.next();
    assert_eq!(app.session_count(), 1);
    // One permit: a second page is refused.
    let refused = Page::open(&app, "/ws?path=%2F");
    assert_eq!(refused.next()["t"], "refused");
    assert!(!page.session.receive("x".repeat(2048)), "oversized");
    wait_until("the permit is freed", || app.session_count() == 0);

    // A page sending faster than its session handles messages.
    let page = Page::open(&app, "/ws?path=%2F");
    let slow = op_with_text(&page.next()["p"], "slow");
    page.send(json!({ "t": "click", "id": slow }));
    let small = json!({ "t": "viewport", "w": 100, "h": 100 }).to_string();
    let accepted = (0..100)
        .take_while(|_| page.session.receive(small.clone()))
        .count();
    assert!(accepted < 100, "the backlog is bounded");
    wait_until("the permit is freed", || app.session_count() == 0);
    let _ = std::fs::remove_dir_all(dir);
}

// --- S6: pictures fetched by the server ------------------------------------------------------

/// An upstream on a loopback port answering every request with `reply`:
/// its origin, how many requests reached it, and the last one's head.
fn upstream(
    reply: Vec<u8>,
) -> (
    String,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    std::sync::Arc<std::sync::Mutex<String>>,
) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let head = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let (count, last) = (hits.clone(), head.clone());
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            *last.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).into_owned();
            let _ = s.write_all(&reply);
        }
    });
    (origin, hits, head)
}

/// A page showing `url` with header `X-Token`: the proxied picture's
/// answer, from the browser that rendered it.
fn fetch_picture(tag: &str, url: &str, trust: Option<&str>) -> http::Response<Vec<u8>> {
    use prism_core::prelude::*;
    struct Pic(String);
    impl Render for Pic {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            picture(self.0.clone()).header("X-Token", "t0ken")
        }
    }
    let url = url.to_string();
    let init: prism_core::AppInit = std::sync::Arc::new(move |cx: &mut App| {
        let url = url.clone();
        let root = cx.new_entity(|_| Pic(url));
        cx.set_root(root);
    });
    let mut app = crate::WebApp::new(prism_core::AppConfig::default(), init).data_dir(scratch(tag));
    if let Some(origin) = trust {
        app = app.trust_media_origin(origin);
    }
    let cookie = ("cookie", "prism_sid=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    let get = |path: &str| {
        let req = http::Request::get(path)
            .header(cookie.0, cookie.1)
            .body(())
            .unwrap();
        app.handle(&req)
    };
    let html = String::from_utf8(get("/").body().clone()).unwrap();
    let id = html
        .split("/_prism/picture/")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("a proxied picture");
    get(&format!("/_prism/picture/{id}"))
}

const PNG: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 4\r\nConnection: close\r\n\r\npng!";

#[test]
fn s6_a_picture_with_headers_reaches_no_private_origin_unless_trusted() {
    let (origin, hits, _) = upstream(PNG.to_vec());
    let url = format!("{origin}/a.png");
    let res = fetch_picture("s6-private", &url, None);
    assert_eq!(res.status(), http::StatusCode::BAD_GATEWAY);
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    // `localhost` is no way around it.
    let local = url.replace("127.0.0.1", "localhost");
    let res = fetch_picture("s6-localhost", &local, None);
    assert_eq!(res.status(), http::StatusCode::BAD_GATEWAY);
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);

    // Trusted: one request, with the header the app gave.
    let res = fetch_picture("s6-trusted", &url, Some(&origin));
    assert_eq!(res.status(), http::StatusCode::OK);
    assert_eq!(res.body().as_slice(), b"png!");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn s6_a_trusted_origin_cannot_redirect_elsewhere() {
    let (target, redirected, _) = upstream(PNG.to_vec());
    let (origin, hits, _) = upstream(
        format!("HTTP/1.1 302 Found\r\nLocation: {target}/b.png\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .into_bytes(),
    );
    let res = fetch_picture("s6-redirect", &format!("{origin}/a.png"), Some(&origin));
    assert_eq!(res.status(), http::StatusCode::BAD_GATEWAY);
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(redirected.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// Links whose target would run script in the app's page, as a browser
/// reads them (case, leading controls and spaces, tabs and newlines inside).
const SCRIPT_LINKS: [&str; 8] = [
    "javascript:alert(1)",
    "JaVaScRiPt:alert(1)",
    " javascript:alert(1)",
    "\u{1}javascript:alert(1)",
    "java\tscr\nipt:alert(1)",
    "vbscript:msgbox(1)",
    "data:text/html,<script>alert(1)</script>",
    "blob:https://example.com/0f8e",
];

// --- S21: what the server reads from a picture or media upstream ------------------------

/// The proxied id of `url` with an app header, and the origin to trust.
fn proxied_id(url: &str) -> (String, Vec<String>) {
    let src = format!("{url}\nX-Token: t0ken");
    let proxied = crate::pictures::proxied("", Some("s21"), &src).unwrap();
    let id = proxied.rsplit('/').next().unwrap().to_string();
    (id, crate::pictures::origin(url).into_iter().collect())
}

/// S21: a picture upstream that sends more than the app accepts (here
/// without a `Content-Length`) is refused once past the limit, not held
/// in memory whole.
#[test]
fn s21_a_picture_past_the_limit_is_refused() {
    let mut reply =
        b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nConnection: close\r\n\r\n".to_vec();
    reply.resize(
        reply.len() + crate::app::DEFAULT_MAX_PICTURE as usize + 1,
        b'x',
    );
    let (origin, _, _) = upstream(reply);
    let (id, trusted) = proxied_id(&format!("{origin}/big.png"));
    let res = crate::pictures::serve(&id, Some("s21"), &trusted, crate::app::DEFAULT_MAX_PICTURE);
    assert_eq!(res.status(), http::StatusCode::BAD_GATEWAY);
    assert!(res.body().is_empty());
}

/// S21: a media upstream that ignores the range and sends a whole large
/// file is refused past one range's worth of bytes.
#[test]
fn s21_a_media_upstream_ignoring_ranges_is_read_one_range_at_most() {
    let size = crate::pictures::MEDIA_CHUNK as usize + 1;
    let mut reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
    )
    .into_bytes();
    reply.resize(reply.len() + size, b'v');
    let (origin, _, head) = upstream(reply);
    let (id, trusted) = proxied_id(&format!("{origin}/talk.mp4"));
    let res = crate::pictures::serve_media(&id, Some("s21"), Some("bytes=0-99"), &trusted);
    assert!(
        head.lock()
            .unwrap()
            .to_ascii_lowercase()
            .contains("range: bytes=0-99")
    );
    assert_eq!(res.status(), http::StatusCode::BAD_GATEWAY);
}

/// S21: a partial answer must be the range that was asked for, whole.
#[test]
fn s21_a_partial_answer_must_match_its_range() {
    let partial = |range: &str, body: &str| {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: video/mp4\r\nContent-Range: {range}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    };
    let serve = |reply: Vec<u8>, asked: &str| {
        let (origin, _, _) = upstream(reply);
        let (id, trusted) = proxied_id(&format!("{origin}/talk.mp4"));
        crate::pictures::serve_media(&id, Some("s21"), Some(asked), &trusted)
    };
    // Another range than the one asked for, a body that is not the range,
    // nonsense.
    for (reply, asked) in [
        (partial("bytes 0-9/100", "0123456789"), "bytes=50-59"),
        (partial("bytes 50-59/100", "01234"), "bytes=50-59"),
        (
            partial("bytes 50-69/100", "01234567890123456789"),
            "bytes=50-59",
        ),
        (partial("items 50-59/100", "0123456789"), "bytes=50-59"),
        (partial("bytes 59-50/100", "0123456789"), "bytes=50-59"),
    ] {
        let res = serve(reply, asked);
        assert_eq!(res.status(), http::StatusCode::BAD_GATEWAY, "{asked}");
    }
    // A well-formed one passes, shorter than asked included (the end of
    // the file), with an unknown total.
    let res = serve(partial("bytes 50-59/100", "0123456789"), "bytes=50-59");
    assert_eq!(res.status(), http::StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.body(), b"0123456789");
    assert_eq!(
        res.headers()[http::header::CONTENT_RANGE],
        "bytes 50-59/100"
    );
    let res = serve(partial("bytes 95-99/*", "01234"), "bytes=95-120");
    assert_eq!(res.status(), http::StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.body(), b"01234");
}

/// S21: upstream URLs in the server's log keep no query or credentials
/// (signed URLs carry their tokens there).
#[test]
fn s21_logged_upstream_urls_keep_no_secrets() {
    use crate::pictures::shown;
    assert_eq!(
        shown("https://user:pw@cdn.example/a/b.mp4?X-Amz-Signature=abc#t=1"),
        "https://cdn.example/a/b.mp4"
    );
    assert_eq!(
        shown("http://127.0.0.1:8080/x.png"),
        "http://127.0.0.1:8080/x.png"
    );
    assert_eq!(shown("not a url"), "(an invalid URL)");
}

/// The server-rendered page of one view, one text and one rich text,
/// each linking to `href`.
fn ssr_links(href: &str) -> String {
    use prism_core::{Backend, NodeKind, Patch, TextRun, encode_runs};
    let mut b = crate::html::HtmlBackend::default();
    b.base = "/app".into();
    let create = |id, kind| Patch::Create { id, kind };
    let link = |id| Patch::SetLink {
        id,
        href: Some(href.into()),
    };
    b.apply(&[
        create(1, NodeKind::View),
        create(2, NodeKind::View),
        link(2),
        create(3, NodeKind::Text),
        link(3),
        create(4, NodeKind::Text),
        Patch::SetConfig {
            id: 4,
            config: encode_runs(&[TextRun {
                text: "x".into(),
                link: Some(42),
                href: Some(href.into()),
                ..TextRun::default()
            }]),
        },
        Patch::SetChildren {
            id: 1,
            children: vec![2, 3, 4],
        },
        Patch::SetRoot { id: 1 },
    ]);
    b.to_html()
}

#[test]
fn s8_script_links_get_no_href() {
    for href in SCRIPT_LINKS {
        let html = ssr_links(href);
        assert!(!html.contains("href="), "{href:?}: {html}");
    }
    for (href, expect) in [
        ("/settings", "/app/settings"),
        ("https://example.com/docs", "https://example.com/docs"),
        ("mailto:ada@example.com", "mailto:ada@example.com"),
        ("tel:+33123456789", "tel:+33123456789"),
        (
            "zoommtg://zoom.us/join?confno=1",
            "zoommtg://zoom.us/join?confno=1",
        ),
    ] {
        let html = ssr_links(href);
        assert_eq!(
            html.matches(&format!("href=\"{expect}\"")).count(),
            3,
            "{href:?}: {html}"
        );
    }
}

#[test]
fn s8_the_page_is_never_told_to_open_a_script_url() {
    for url in SCRIPT_LINKS {
        for call in [
            DeviceCall::OpenUrl(url.into()),
            DeviceCall::OpenUrlIn(url.into(), prism_core::UrlTarget::SameTab),
        ] {
            let v = crate::session::device_json(1, call);
            assert_eq!(v["url"], Value::Null, "{url:?}: {v}");
        }
    }
    let v = crate::session::device_json(1, DeviceCall::OpenUrl("/logout".into()));
    assert_eq!(v["url"], "/logout");
}

/// An app with two files uploaded in session `SID`: an SVG that runs a
/// script when opened as a document, and a PNG. Their URL paths.
const SID: &str = "5e55105e55105e55105e55105e55105e";

fn uploads_app(tag: &str, strict: bool) -> (crate::WebApp, String, String) {
    use prism_core::prelude::*;
    struct Blank;
    impl Render for Blank {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            text("blank")
        }
    }
    let init: prism_core::AppInit = std::sync::Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Blank);
        cx.set_root(root);
    });
    let mut app = crate::WebApp::new(prism_core::AppConfig::default(), init).data_dir(scratch(tag));
    if strict {
        app = app.strict_csp();
    }
    let files = app.files_dir(SID);
    std::fs::create_dir_all(&files).unwrap();
    let svg = format!("{}.svg", "0".repeat(32));
    let png = format!("{}.png", "1".repeat(32));
    std::fs::write(
        files.join(&svg),
        r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(document.cookie)</script></svg>"#,
    )
    .unwrap();
    std::fs::write(files.join(&png), b"\x89PNG\r\n\x1a\n").unwrap();
    (app, format!("/files/{svg}"), format!("/files/{png}"))
}

fn sandboxed(csp: &str) -> bool {
    csp.split(';').any(|d| d.trim() == "sandbox") && csp.contains("default-src 'none'")
}

#[test]
fn s9_uploads_never_run_as_documents() {
    let (app, svg, png) = uploads_app("s9-headers", false);
    let get = |path: &str, sid: &str| {
        let req = http::Request::get(path)
            .header("cookie", format!("prism_sid={sid}"))
            .body(())
            .unwrap();
        app.handle(&req)
    };
    let h = |r: &http::Response<Vec<u8>>, name: &str| {
        r.headers()
            .get(name)
            .map(|v| v.to_str().unwrap().to_string())
    };
    // The SVG: a download, without scripts or sniffing even if opened.
    let r = get(&svg, SID);
    assert_eq!(r.status(), http::StatusCode::OK);
    assert_eq!(h(&r, "x-content-type-options").as_deref(), Some("nosniff"));
    assert!(
        sandboxed(&h(&r, "content-security-policy").unwrap_or_default()),
        "{r:?}"
    );
    assert!(
        h(&r, "content-disposition").is_some_and(|d| d.starts_with("attachment")),
        "{r:?}"
    );
    // A raster picture still shows inline (previews), sandboxed too.
    let r = get(&png, SID);
    assert_eq!(r.status(), http::StatusCode::OK);
    assert_eq!(h(&r, "content-type").as_deref(), Some("image/png"));
    assert_eq!(h(&r, "content-disposition"), None);
    assert!(
        sandboxed(&h(&r, "content-security-policy").unwrap_or_default()),
        "{r:?}"
    );
    // Another browser's session has none of them.
    let other = "0123456789abcdef0123456789abcdef";
    assert_eq!(get(&svg, other).status(), http::StatusCode::NOT_FOUND);
    assert_eq!(get(&png, other).status(), http::StatusCode::NOT_FOUND);
}

/// The head of the answer to `GET path` from the router serving `app`.
#[cfg(feature = "server")]
fn served_head(app: crate::WebApp, paths: &[&str]) -> Vec<String> {
    use std::io::{Read, Write};
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            axum::serve(listener, app.router::<()>()).await.unwrap();
        });
    });
    let addr = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    paths
        .iter()
        .map(|path| {
            let mut s = std::net::TcpStream::connect(addr).unwrap();
            write!(
                s,
                "GET {path} HTTP/1.1\r\nHost: {addr}\r\nCookie: prism_sid={SID}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut out = Vec::new();
            s.read_to_end(&mut out).unwrap();
            let out = String::from_utf8_lossy(&out).to_ascii_lowercase();
            out.split("\r\n\r\n").next().unwrap_or("").to_string()
        })
        .collect()
}

#[cfg(feature = "server")]
#[test]
fn s9_the_strict_page_policy_keeps_the_upload_sandbox() {
    let (app, svg, png) = uploads_app("s9-strict", true);
    let heads = served_head(app, &[&svg, &png, "/"]);
    let csp = |head: &str| {
        head.lines()
            .find_map(|l| l.strip_prefix("content-security-policy: "))
            .unwrap_or("")
            .to_string()
    };
    assert!(sandboxed(&csp(&heads[0])), "{}", heads[0]);
    assert!(sandboxed(&csp(&heads[1])), "{}", heads[1]);
    // Pages still get the strict policy.
    assert!(csp(&heads[2]).contains("'nonce-"), "{}", heads[2]);
}

fn page_cookie(app: &crate::WebApp, path: &str, headers: &[(&str, &str)]) -> Option<String> {
    let mut req = http::Request::get(path);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let r = app.handle(&req.body(()).unwrap());
    assert_eq!(r.status(), http::StatusCode::OK);
    r.headers()
        .get("set-cookie")
        .map(|v| v.to_str().unwrap().to_string())
}

#[test]
fn s10_https_pages_set_a_host_only_secure_cookie() {
    let (app, _, _) = uploads_app("s10-https", false);
    let https = [("x-forwarded-proto", "https")];
    let cookie = page_cookie(&app, "/", &https).unwrap();
    assert!(cookie.starts_with("__Host-prism_sid="), "{cookie}");
    assert!(
        cookie.contains("; Secure") && cookie.contains("; Path=/;"),
        "{cookie}"
    );
    // A mounted app too: `__Host-` needs `Path=/`.
    let mounted = uploads_app("s10-mounted", false).0.base_path("/app");
    let cookie = page_cookie(&mounted, "/app", &https).unwrap();
    assert!(
        cookie.starts_with("__Host-prism_sid=") && cookie.contains("; Path=/;"),
        "{cookie}"
    );
    // Plain HTTP (local development) keeps a plain cookie.
    let cookie = page_cookie(&app, "/", &[]).unwrap();
    assert!(
        cookie.starts_with("prism_sid=") && !cookie.contains("Secure"),
        "{cookie}"
    );
}

#[test]
fn s10_a_planted_cookie_cannot_choose_the_browser_on_https() {
    let (app, svg, _) = uploads_app("s10-fixation", false);
    // A sibling site set `prism_sid` (here SID, whose files exist) before
    // the first visit: the page ignores it and issues its own id.
    let planted = format!("prism_sid={SID}");
    let https = ("x-forwarded-proto", "https");
    let cookie = page_cookie(&app, "/", &[https, ("cookie", &planted)]).expect("a new id");
    assert!(cookie.starts_with("__Host-prism_sid="), "{cookie}");
    assert!(!cookie.contains(SID), "{cookie}");
    // Nor do the planted id's files open with it.
    let get = |cookie: &str| {
        let req = http::Request::get(&svg)
            .header(https.0, https.1)
            .header("cookie", cookie)
            .body(())
            .unwrap();
        app.handle(&req).status()
    };
    assert_eq!(get(&planted), http::StatusCode::NOT_FOUND);
    // The host-only cookie wins over a planted one.
    let mine = format!("prism_sid=0123456789abcdef0123456789abcdef; __Host-prism_sid={SID}");
    assert_eq!(get(&mine), http::StatusCode::OK);
    assert_eq!(page_cookie(&app, "/", &[https, ("cookie", &mine)]), None);
    // Plain HTTP: as before.
    assert_eq!(page_cookie(&app, "/", &[("cookie", &planted)]), None);
}

fn socket_accepted(app: &crate::WebApp, headers: &[(&str, &str)]) -> bool {
    let mut h = http::HeaderMap::new();
    for (k, v) in headers {
        h.append(
            http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    app.accept_socket(&h).is_ok()
}

#[test]
fn s11_socket_origins_are_compared_whole() {
    let app = uploads_app("s11-origins", false).0;
    // Behind a TLS proxy that says so.
    let https = |origin: &str| {
        socket_accepted(
            &app,
            &[
                ("host", "app.example.com"),
                ("x-forwarded-proto", "https"),
                ("origin", origin),
            ],
        )
    };
    assert!(https("https://app.example.com"));
    assert!(https("https://APP.example.com:443"));
    for origin in [
        "http://app.example.com",
        "http://app.example.com:443",
        "https://app.example.com:8443",
        "https://app.example.com.evil.example",
        "https://user@app.example.com",
        "app.example.com",
        "ftp://app.example.com",
        "null",
    ] {
        assert!(!https(origin), "{origin}");
    }
    // Two Origin fields: refused, whatever the first says.
    assert!(!socket_accepted(
        &app,
        &[
            ("host", "app.example.com"),
            ("x-forwarded-proto", "https"),
            ("origin", "https://app.example.com"),
            ("origin", "https://evil.example"),
        ],
    ));
    // Plain HTTP (development, or TLS ended in front without saying so).
    let plain =
        |origin: &str| socket_accepted(&app, &[("host", "localhost:3000"), ("origin", origin)]);
    assert!(plain("http://localhost:3000"));
    assert!(plain("https://localhost:3000"));
    assert!(!plain("http://localhost:3001"));
    assert!(!plain("http://localhost"));
    // No Origin: not a browser (a native client); it carries no page's cookies.
    assert!(socket_accepted(&app, &[("host", "app.example.com")]));
}

#[cfg(feature = "server")]
#[test]
fn s11_a_loopback_server_refuses_rebound_host_names() {
    use std::io::{Read, Write};
    let app = uploads_app("s11-rebinding", false).0;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = format!("127.0.0.1:{port}");
    let bind = addr.clone();
    std::thread::spawn(move || crate::serve(app, &bind));
    let status = |host: &str, extra: &str| {
        for _ in 0..100 {
            if let Ok(mut s) = std::net::TcpStream::connect(&addr) {
                write!(
                    s,
                    "GET / HTTP/1.1\r\nHost: {host}\r\n{extra}Connection: close\r\n\r\n"
                )
                .unwrap();
                let mut out = String::new();
                let _ = s.read_to_string(&mut out);
                return out.split(' ').nth(1).unwrap_or("").to_string();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the server did not start");
    };
    // A page at evil.example whose name now points at 127.0.0.1.
    assert_eq!(status(&format!("evil.example:{port}"), ""), "421");
    assert_eq!(status(&format!("127.0.0.1:{port}"), ""), "200");
    assert_eq!(status(&format!("localhost:{port}"), ""), "200");
    assert_eq!(status(&format!("[::1]:{port}"), ""), "200");
    // A reverse proxy on this machine (nginx, Caddy, a tunnel) says so.
    assert_eq!(
        status("app.example.com", "X-Forwarded-Proto: https\r\n"),
        "200"
    );
}

#[test]
fn s11_a_public_origin_ignores_host_and_forwarded_headers() {
    let app = uploads_app("s11-public", false)
        .0
        .public_origin("https://App.example.com:443/")
        .allow_origins(["https://admin.example.com"]);
    // A proxy that rewrites `Host` and says nothing of the scheme.
    let internal = ("host", "10.0.0.5:3000");
    assert!(socket_accepted(
        &app,
        &[internal, ("origin", "https://app.example.com")]
    ));
    assert!(socket_accepted(
        &app,
        &[internal, ("origin", "https://admin.example.com")]
    ));
    // Forwarded headers cannot choose another origin, nor another scheme.
    assert!(!socket_accepted(
        &app,
        &[
            ("host", "evil.example"),
            ("x-forwarded-host", "evil.example"),
            ("x-forwarded-proto", "https"),
            ("origin", "https://evil.example"),
        ]
    ));
    assert!(!socket_accepted(
        &app,
        &[internal, ("origin", "http://app.example.com")]
    ));
    // The page is HTTPS: a host-only `Secure` id.
    let cookie = page_cookie(&app, "/", &[internal]).unwrap();
    assert!(cookie.starts_with("__Host-prism_sid="), "{cookie}");
    // Its host passes a loopback server's check.
    let mut h = http::HeaderMap::new();
    h.insert("host", "app.example.com".parse().unwrap());
    assert!(crate::host::loopback_host_allowed(&h, app.public()));
}

#[test]
#[should_panic(expected = "is not an origin")]
fn s11_a_public_origin_must_be_an_origin() {
    let _ = uploads_app("s11-bad", false)
        .0
        .public_origin("app.example.com/path");
}

/// S12: the page's script reports a web message's origin. A message from an
/// origin the web view does not allow never reaches the app; a view with a
/// relative URL allows the page's own origin.
#[test]
fn s12_web_messages_keep_their_view_allowed_origins() {
    use crate::tests::{Page, op_with_text, web_app};
    use prism_core::prelude::*;
    use prism_core::web_view::{WebMessageEvent, web_view};

    struct Host {
        got: String,
    }
    impl Render for Host {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(web_view("/preview").on_message(cx.listener(
                    |this, e: &WebMessageEvent, _| {
                        this.got = format!("{} {}", e.data_value(), e.origin);
                    },
                )))
                .child(text(self.got.clone()))
        }
    }
    let init: prism_core::AppInit = std::sync::Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Host { got: "-".into() });
        cx.set_root(root);
    });
    let (app, dir) = web_app(init, "s12");
    let page = Page::open(&app, "/ws?path=%2F&w=1280&h=800");
    let reset = page.next();
    let got = op_with_text(&reset["p"], "-");
    let view = reset["p"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["op"] == "create" && p["kind"] == "web_view")
        .and_then(|p| p["id"].as_u64())
        .expect("the web view");
    // The socket opened without an `Origin`: the page is `http://localhost`.
    for origin in ["https://evil.example", "null", ""] {
        page.send(json!({ "t": "webmsg", "id": view, "data": "forged", "origin": origin }));
    }
    page.send(json!({ "t": "webmsg", "id": view, "data": "ok", "origin": "http://localhost" }));
    assert_eq!(
        page.next(),
        json!({ "t": "patch", "p": [{ "op": "text", "id": got, "text": "\"ok\" http://localhost" }] })
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// S13: numbers past `f32::MAX` would reach the app as infinities; the
/// page's reports that carry one are dropped.
#[test]
fn s13_reports_with_numbers_past_f32_are_dropped() {
    use crate::session::parse_event;
    for event in [
        json!({ "t": "layout", "id": 1, "x": 0, "y": 0, "w": 1e39, "h": 1 }),
        json!({ "t": "scroll", "id": 1, "o": -1e300, "v": 1 }),
    ] {
        assert!(parse_event(&event).is_none(), "{event}");
    }
    // A click keeps no such position: it is a click without one.
    assert!(matches!(
        parse_event(&json!({ "t": "click", "id": 1, "x": 1e39, "y": 0 })),
        Some(prism_core::UiEvent::Click(1))
    ));
    assert!(parse_event(&json!({ "t": "scroll", "id": 1, "o": 12.5, "v": 300 })).is_some());
}

/// S19: the page's `link` message opens the app's links (a notification
/// tap). A widget action's link runs nothing there: only a link with the
/// app's token does, and the web has no widgets, so no key.
#[test]
fn s19_a_link_from_the_page_runs_no_widget_action() {
    use crate::tests::{Page, web_app};
    use prism_core::prelude::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    static RAN: AtomicBool = AtomicBool::new(false);
    struct Here(&'static str);
    impl Render for Here {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            text(self.0)
        }
    }
    let init: prism_core::AppInit = std::sync::Arc::new(|cx: &mut App| {
        let router = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| Here("home")))
            .route("/next", |cx, _| cx.new_entity(|_| Here("next")))
            .build(cx);
        cx.set_root(router);
        cx.on_widget_action(|_, _| async { RAN.store(true, Ordering::SeqCst) });
    });
    let (app, dir) = web_app(init, "s19");
    let page = Page::open(&app, "/ws?path=%2F");
    page.next();
    page.send(json!({ "t": "link", "path": "/__prism/widget-action?kind=w&action=go&payload=1" }));
    page.send(json!({ "t": "link", "path": "/__prism/widget-action?kind=w&action=go&payload=1&token=00000000000000000000000000000000" }));
    // Messages are applied in order: once `/next` shows, both links were.
    page.send(json!({ "t": "link", "path": "/next" }));
    let mut shown = false;
    for _ in 0..3 {
        if page.next()["p"].to_string().contains(r#""next""#) {
            shown = true;
            break;
        }
    }
    assert!(shown, "the page link navigates");
    assert!(
        !RAN.load(Ordering::SeqCst),
        "a page link ran a widget action"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// S20: a web component's tag and prop names can come from data (a CMS).
/// The first paint keeps both out of markup: an invalid tag is a plain
/// container around the fallback, and props never become event handlers,
/// styles or Prism's own attributes.
#[test]
fn s20_component_tags_and_prop_names_stay_out_of_markup() {
    use prism_core::prelude::*;
    use prism_core::{Backend, Runtime};

    struct Page;
    impl Render for Page {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(web_component("x-a><i data-marker=tag></i").child(text("fallback one")))
                .child(web_component("font-face").child(text("fallback two")))
                .child(
                    web_component("x-card")
                        .prop("label", "ok")
                        .prop("onclick", "marker()")
                        .prop("OnFocus", "marker()")
                        .prop("style", "color: red")
                        .prop("data-pid", "99")
                        .prop("class", "x")
                        .prop("is", "x")
                        .prop("1st", "x")
                        .child(text("card")),
                )
        }
    }
    let init: prism_core::AppInit = std::sync::Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Page);
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    let mut html = crate::HtmlBackend::default();
    html.apply(&rt.render());
    let out = html.to_html();
    assert!(!out.contains("marker"), "{out}");
    assert!(!out.contains("<x-a"), "{out}");
    assert!(!out.contains("<font-face"), "{out}");
    assert!(
        out.contains("fallback one") && out.contains("fallback two"),
        "{out}"
    );
    let card = &out[out.find("<x-card").expect("the valid component")..];
    let card = &card[..card.find('>').unwrap()];
    assert!(card.contains(r#" label="ok""#), "{card}");
    for name in [
        "onclick",
        "onfocus",
        "OnFocus",
        "color: red",
        "data-pid=\"99",
        "class=\"x",
        " is=",
        "1st",
    ] {
        assert!(!card.contains(name), "{name} in {card}");
    }
}

// --- S14, S15: sign-ins and cookies the server completes ------------------------------------

/// S14: two sessions wait for sign-ins under the same call number. One
/// page's failure, or a stray one, cancels only its own sign-in; a
/// redirect is redeemed once.
#[test]
fn s14_a_sign_in_is_cancelled_only_by_its_own_session() {
    use crate::security::{
        forget_redirect, random_id, redirect_page, session_key, wait_for_redirect,
    };
    use crate::session::Inbound;
    let (a, b) = (session_key(), session_key());
    let (tx_a, rx_a) = std::sync::mpsc::channel();
    let (tx_b, rx_b) = std::sync::mpsc::channel();
    let (state_a, state_b) = (random_id(), random_id());
    assert!(wait_for_redirect(state_a.clone(), tx_a, a, 1));
    assert!(wait_for_redirect(state_b.clone(), tx_b, b, 1));
    // Another session cannot take a waiting state over.
    let (tx_c, _rx_c) = std::sync::mpsc::channel();
    assert!(!wait_for_redirect(state_a.clone(), tx_c, session_key(), 1));
    // B's page reports its call 1 failed; a session that never called too.
    forget_redirect(b, 1);
    forget_redirect(session_key(), 1);
    let page = redirect_page(&format!("code=c&state={state_a}"), None);
    assert!(page.contains("Signed in"), "{page}");
    assert!(matches!(rx_a.try_recv(), Ok(Inbound::AuthRedirect(1, _))));
    assert!(redirect_page(&format!("code=c&state={state_a}"), None).contains("expired"));
    assert!(redirect_page(&format!("code=c&state={state_b}"), None).contains("expired"));
    assert!(rx_b.try_recv().is_err());
}

/// S14: sign-ins left waiting expire, end with their session, and a
/// session cannot keep more than a few waiting.
#[test]
fn s14_abandoned_sign_ins_expire_and_end_with_their_session() {
    use crate::security::{
        REDIRECT_TTL, forget_session, random_id, session_key, take_redirect, wait_for_redirect,
    };
    let session = session_key();
    let (tx, _rx) = std::sync::mpsc::channel();
    let states: Vec<String> = (0..8).map(|_| random_id()).collect();
    for (id, state) in states.iter().enumerate() {
        assert!(wait_for_redirect(
            state.clone(),
            tx.clone(),
            session,
            id as u64
        ));
    }
    assert!(!wait_for_redirect(random_id(), tx.clone(), session, 8));
    // The session ends: none of its sign-ins can be redeemed.
    forget_session(session);
    let now = std::time::Instant::now();
    assert!(states.iter().all(|s| take_redirect(s, now).is_none()));
    // Ten minutes later, a sign-in left open has expired.
    let state = random_id();
    assert!(wait_for_redirect(state.clone(), tx.clone(), session, 9));
    let later = std::time::Instant::now() + REDIRECT_TTL;
    assert!(take_redirect(&state, later).is_none());
    let state = random_id();
    assert!(wait_for_redirect(state.clone(), tx, session, 10));
    assert!(take_redirect(&state, std::time::Instant::now()).is_some());
}

/// S15: the page cannot answer a cookie or a sign-in in the server's
/// place: its success is ignored, its failure cancels. The cookie
/// succeeds once the browser fetched its ticket.
#[test]
fn s15_the_page_cannot_complete_a_cookie_or_a_sign_in() {
    use crate::tests::{Page, op_with_text, web_app};
    use prism_core::prelude::*;

    #[derive(Default)]
    struct Calls {
        started: bool,
        cookie: String,
        sign_in: String,
    }
    impl Render for Calls {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            if !std::mem::replace(&mut self.started, true) {
                cx.spawn(async move |this, cx| {
                    let r = cx
                        .set_cookie(prism_core::Cookie::new("theme", "dark"))
                        .await;
                    cx.update_entity(&this, |c, _| c.cookie = format!("cookie {r:?}"));
                });
                cx.spawn(async move |this, cx| {
                    let r = cx
                        .web_auth(prism_core::WebAuthRequest {
                            url: "https://id.example/authorize?state=s15-state".into(),
                            redirect: String::new(),
                            ephemeral: false,
                            provider: "Example".into(),
                        })
                        .await;
                    cx.update_entity(&this, |c, _| c.sign_in = format!("sign-in {r:?}"));
                });
            }
            div()
                .child(text(format!("[{}]", self.cookie)))
                .child(text(format!("<{}>", self.sign_in)))
        }
    }
    let init: prism_core::AppInit = std::sync::Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Calls::default());
        cx.set_root(root);
    });
    let (app, dir) = web_app(init, "s15");
    let sid = "0123456789abcdef0123456789abcdef";
    let mut headers = http::HeaderMap::new();
    headers.insert("cookie", format!("prism_sid={sid}").parse().unwrap());
    let page = Page::open_with(&app, "/ws?path=%2F&w=1280&h=800", &headers);
    let reset = page.next();
    let (cookie, sign_in) = (
        op_with_text(&reset["p"], "[]"),
        op_with_text(&reset["p"], "<>"),
    );
    let mut calls = std::collections::HashMap::new();
    while calls.len() < 2 {
        let m = page.next();
        if m["t"] == "device" {
            calls.insert(m["call"].as_str().unwrap().to_string(), m);
        }
    }
    let (set, auth) = (&calls["set_cookie"], &calls["web_auth"]);
    // The page claims both succeeded: ignored.
    page.send(json!({ "t": "device_reply", "id": set["id"] }));
    page.send(json!({ "t": "device_reply", "id": auth["id"],
        "text": "https://app.example/_prism/auth/callback?code=forged&state=s15-state" }));
    // Its failure cancels the sign-in (the first patch since: nothing
    // completed the cookie).
    page.send(json!({ "t": "device_reply", "id": auth["id"], "err": "cancelled" }));
    assert_eq!(
        page.next(),
        json!({ "t": "patch", "p": [{ "op": "text", "id": sign_in,
            "text": "<sign-in Err(Cancelled)>" }] })
    );
    // The browser fetches the cookie: now it succeeds.
    let mut fetch = http::HeaderMap::new();
    for (k, v) in [
        ("host", "localhost"),
        ("origin", "http://localhost"),
        ("sec-fetch-site", "same-origin"),
        ("x-prism-cookie", set["ticket"].as_str().unwrap()),
    ] {
        fetch.insert(k, v.parse().unwrap());
    }
    let answer = crate::cookies::redeem(&fetch, Some(sid.into()), None);
    assert_eq!(answer.status(), http::StatusCode::NO_CONTENT);
    assert_eq!(
        page.next(),
        json!({ "t": "patch", "p": [{ "op": "text", "id": cookie, "text": "[cookie Ok(())]" }] })
    );
    let _ = std::fs::remove_dir_all(dir);
}

fn upload_file(name: &str, size: u64) -> Value {
    json!({ "name": name, "mime": "text/plain", "size": size })
}

fn stored_files(app: &crate::WebApp, sid: &str) -> usize {
    std::fs::read_dir(app.files_dir(sid)).map_or(0, |d| d.count())
}

/// S16: the files a page announces are checked against the app's limits
/// before anything is stored (how many, their bytes per browser and
/// server-wide, their names), and an upload that ends any way gives its
/// bytes back.
#[test]
fn s16_uploads_past_the_limits_are_refused_before_anything_is_stored() {
    use crate::uploads::{Key, Uploads};
    use std::sync::atomic::Ordering::SeqCst;
    let (app, _, _) = uploads_app("s16-limits", false);
    let app = app
        .max_upload_files(2)
        .max_upload_bytes(10)
        .max_uploading_bytes(8);
    let sid = "5e16a05e16a05e16a05e16a05e16a05e";
    let mut up = Uploads::default();
    let mut refused = |id: u64, max: Option<usize>, files: &[Value]| -> String {
        up.expect(id, max);
        assert_eq!(up.start(&app, sid, Key::Call(id), files), None, "{files:?}");
        match up.finish(&app, Key::Call(id), true) {
            Some(Err(why)) => why,
            other => panic!("{other:?}"),
        }
    };
    let f = upload_file;
    assert!(refused(1, None, &[f("a", 1), f("b", 1), f("c", 1)]).contains("too many files"));
    // A pick of one file takes one.
    assert!(refused(2, Some(1), &[f("a", 1), f("b", 1)]).contains("too many files"));
    assert!(refused(3, None, &[f("a", 6), f("b", 6)]).contains("too large"));
    // Sizes that overflow once added.
    assert!(refused(4, None, &[f("a", u64::MAX), f("b", u64::MAX)]).contains("too large"));
    assert!(refused(5, None, &[f(&"n".repeat(2000), 1)]).contains("too long"));
    assert_eq!(stored_files(&app, sid), 0);
    // Accepted: nothing on disk before its first chunk, its bytes reserved.
    let mut up = Uploads::default();
    up.expect(6, None);
    assert_eq!(
        up.start(&app, sid, Key::Call(6), &[f("a.txt", 6)]),
        Some((0, 6))
    );
    assert_eq!(stored_files(&app, sid), 0);
    assert_eq!(app.uploading().load(SeqCst), 6);
    // The server receives at most 8 bytes at once: another browser waits.
    let other = "5e16b05e16b05e16b05e16b05e16b05e";
    let mut up2 = Uploads::default();
    up2.expect(1, None);
    assert_eq!(up2.start(&app, other, Key::Call(1), &[f("b.txt", 6)]), None);
    assert!(
        matches!(up2.finish(&app, Key::Call(1), false), Some(Err(why)) if why.contains("try again later"))
    );
    // Cancelled: what arrived is deleted, its bytes given back.
    assert_eq!(up.chunk(Key::Call(6), 0, "aGVs"), Some((3, 6)));
    assert_eq!(stored_files(&app, sid), 1);
    assert_eq!(up.finish(&app, Key::Call(6), true), None);
    assert_eq!(stored_files(&app, sid), 0);
    assert_eq!(app.uploading().load(SeqCst), 0);
    // Now the other one fits; its session ending gives its bytes back too.
    up2.expect(2, None);
    assert!(
        up2.start(&app, other, Key::Call(2), &[f("b.txt", 6)])
            .is_some()
    );
    assert!(up2.chunk(Key::Call(2), 0, "aGVs").is_some());
    drop(up2);
    assert_eq!(stored_files(&app, other), 0);
    assert_eq!(app.uploading().load(SeqCst), 0);
}

/// S16: a chunk past its file's declared size, or longer than a chunk can
/// be (measured before it is decoded), fails the whole upload.
#[test]
fn s16_a_chunk_beyond_its_budget_fails_the_upload() {
    use crate::uploads::{CHUNK, Key, Uploads};
    let (app, _, _) = uploads_app("s16-chunks", false);
    let sid = "5e16c05e16c05e16c05e16c05e16c05e";
    let mut up = Uploads::default();
    up.expect(1, None);
    assert!(
        up.start(&app, sid, Key::Call(1), &[upload_file("a.txt", 3)])
            .is_some()
    );
    assert_eq!(up.chunk(Key::Call(1), 0, "AQIDBA=="), None);
    assert_eq!(
        up.chunk(Key::Call(1), 0, "AQID"),
        None,
        "a failed upload takes nothing more"
    );
    assert!(matches!(up.finish(&app, Key::Call(1), false), Some(Err(_))));
    up.expect(2, None);
    assert!(
        up.start(&app, sid, Key::Call(2), &[upload_file("b.bin", 1 << 20)])
            .is_some()
    );
    let long = "A".repeat(CHUNK.div_ceil(3) * 4 + 4);
    assert_eq!(up.chunk(Key::Call(2), 0, &long), None);
    assert!(matches!(up.finish(&app, Key::Call(2), false), Some(Err(_))));
    assert_eq!(stored_files(&app, sid), 0);
    assert_eq!(app.uploading().load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// S16: received files count against what their browser keeps, and go
/// after `keep_uploads` with their URLs, those of browsers that never come
/// back included.
#[test]
fn s16_received_files_expire_with_their_urls() {
    use crate::uploads::{Key, Uploads, sweep, sweep_all};
    use prism_core::device::has_local_file;
    use std::time::{Duration, SystemTime};
    let (app, _, _) = uploads_app("s16-expiry", false);
    let app = app
        .keep_uploads(Duration::from_secs(3600))
        .max_upload_bytes(10);
    let sid = "5e16d05e16d05e16d05e16d05e16d05e";
    let mut up = Uploads::default();
    up.expect(1, None);
    let files = [upload_file("a.txt", 3), upload_file("empty.txt", 0)];
    assert!(up.start(&app, sid, Key::Call(1), &files).is_some());
    assert!(up.chunk(Key::Call(1), 0, "AQID").is_some());
    let files = up.finish(&app, Key::Call(1), false).unwrap().unwrap();
    assert_eq!(files[0].read().unwrap(), [1, 2, 3]);
    assert_eq!(files[1].read().unwrap(), Vec::<u8>::new());
    assert_eq!(stored_files(&app, sid), 2);
    // They count against what the browser keeps: 3 + 8 > 10.
    up.expect(2, None);
    assert_eq!(
        up.start(&app, sid, Key::Call(2), &[upload_file("b.txt", 8)]),
        None
    );
    up.finish(&app, Key::Call(2), true);
    let now = SystemTime::now();
    assert_eq!(sweep(&app, &app.files_dir(sid), now), 3);
    assert!(has_local_file(&files[0].path));
    // An hour on, they are gone with their URLs (and the files of the
    // browser that never came back).
    sweep_all(&app, now + Duration::from_secs(3600));
    assert_eq!(stored_files(&app, sid), 0);
    assert_eq!(stored_files(&app, SID), 0);
    assert!(!has_local_file(&files[0].path) && !has_local_file(&files[1].path));
    assert!(files[0].read().is_err());
    up.expect(3, None);
    assert!(
        up.start(&app, sid, Key::Call(3), &[upload_file("b.txt", 8)])
            .is_some()
    );
}

/// S16: a pick the server refuses tells the page to stop sending, and
/// the app learns why.
#[test]
fn s16_a_refused_pick_stops_the_page_and_says_why() {
    use crate::tests::{Page, op_with_text, web_app};
    use prism_core::prelude::*;

    #[derive(Default)]
    struct Pick {
        started: bool,
        got: String,
    }
    impl Render for Pick {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            if !std::mem::replace(&mut self.started, true) {
                cx.spawn(async move |this, cx| {
                    let r = cx.pick_files(prism_core::PickFiles::new()).await;
                    let r = r.map(|f| f.len());
                    cx.update_entity(&this, |p, _| p.got = format!("picked {r:?}"));
                });
            }
            text(format!("[{}]", self.got))
        }
    }
    let init: prism_core::AppInit = std::sync::Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Pick::default());
        cx.set_root(root);
    });
    let (app, dir) = web_app(init, "s16-pick");
    let sid = "5e16e05e16e05e16e05e16e05e16e05e";
    let mut headers = http::HeaderMap::new();
    headers.insert("cookie", format!("prism_sid={sid}").parse().unwrap());
    let page = Page::open_with(&app, "/ws?path=%2F&w=1280&h=800", &headers);
    let reset = page.next();
    let got = op_with_text(&reset["p"], "[]");
    let pick = loop {
        let m = page.next();
        if m["t"] == "device" {
            break m;
        }
    };
    assert_eq!(
        (pick["call"].as_str(), pick["multiple"].as_bool()),
        (Some("pick_files"), Some(false))
    );
    let id = pick["id"].clone();
    let two = json!([upload_file("a.txt", 1), upload_file("b.txt", 1)]);
    page.send(json!({ "t": "upload_start", "id": id, "files": two }));
    assert_eq!(
        page.next(),
        json!({ "t": "device", "id": id, "call": "stop" })
    );
    page.send(json!({ "t": "upload_chunk", "id": id, "file": 0, "data": "AQ==" }));
    page.send(json!({ "t": "device_reply", "id": id, "err": "cancelled" }));
    assert_eq!(
        page.next(),
        json!({ "t": "patch", "p": [{ "op": "text", "id": got,
            "text": "[picked Err(Failed(\"too many files (at most 1)\"))]" }] })
    );
    assert_eq!(stored_files(&app, sid), 0);
    let _ = std::fs::remove_dir_all(dir);
}
