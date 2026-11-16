//! Embedding in a host server: the page's head, strict CSP, request context,
//! page origin, 404s, socket origins and session limits.

use std::sync::{Arc, mpsc};
use std::time::Duration;

use http::{HeaderMap, HeaderValue, Request, Response, StatusCode, Uri};
use prism_core::prelude::*;
use prism_core::{AppConfig, AppInit};
use serde_json::Value;

use crate::{CspNonce, RequestContext, WebApp};

/// The signed-in user, as a host would attach it.
#[derive(Debug, PartialEq)]
struct User {
    id: u32,
}

/// Shows who it renders for and where.
struct Whoami;

impl Render for Whoami {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let user = cx
            .request_context::<User>()
            .map_or("nobody".to_string(), |u| format!("user {}", u.id));
        let request = cx.request();
        let kind = request.map_or("none", |r| match r.kind {
            prism_core::RequestKind::Page => "page",
            prism_core::RequestKind::Socket => "socket",
        });
        let theme = request.and_then(|r| r.cookie("theme")).unwrap_or("-");
        let page = match (cx.page_request(), cx.page_request_context::<User>()) {
            (Some(page), Some(user)) => format!("page {} of user {}", page.path(), user.id),
            (Some(page), None) => format!("page {} of nobody", page.path()),
            (None, _) => "no page".to_string(),
        };
        div()
            .p(8.)
            .child(text(user))
            .child(text(format!("origin {}", cx.page_origin().unwrap_or("-"))))
            .child(text(format!("url {}", cx.page_url().unwrap_or("-"))))
            .child(text(format!("kind {kind} theme {theme}")))
            .child(text(page))
            .child(text("same").p(8.))
            .child(text("<b>").p(8.))
    }
}

fn app(tag: &str, init: AppInit) -> (WebApp, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("prism-host-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets/icon.png"), b"png").unwrap();
    std::fs::write(dir.join("assets/app.css"), b"body{}").unwrap();
    let config = AppConfig {
        title: "Host".into(),
        assets: Some(dir.join("assets").to_string_lossy().into()),
        ..Default::default()
    };
    (WebApp::new(config, init).data_dir(dir.join("data")), dir)
}

fn whoami() -> AppInit {
    Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Whoami);
        cx.set_root(root);
    })
}

fn routed(custom_not_found: bool) -> AppInit {
    Arc::new(move |cx: &mut App| {
        let mut router = Router::new().route("/", |cx, _| cx.new_entity(|_| Whoami));
        if custom_not_found {
            router = router.not_found(|cx, _| cx.new_entity(|_| Whoami));
        }
        let root = router.build(cx);
        cx.set_root(root);
    })
}

fn body(r: &Response<Vec<u8>>) -> String {
    String::from_utf8(r.body().clone()).unwrap()
}

#[test]
fn the_app_adds_fonts_icons_metas_styles_and_scripts_to_the_head() {
    let (app, dir) = app("head", whoami());
    let app = app
        .favicon("icon.png")
        .apple_touch_icon("/static/touch.png")
        .theme_color("#0a84ff")
        .meta("apple-mobile-web-app-title", "Dash \"board\"")
        .stylesheet("app.css")
        .stylesheet("https://fonts.example.com/inter.css")
        .preload_font("/fonts/inter.woff2")
        .script("/static/analytics.js")
        .inline_script("window.hostReady = true;</script>")
        .style("@font-face{font-family:Inter;src:url(/fonts/inter.woff2)}")
        .body_background("#101418")
        .font_family("Inter, system-ui")
        .head(r#"<link rel="manifest" href="/manifest.json">"#);
    let mut req = Request::get("/").body(()).unwrap();
    req.extensions_mut().insert(CspNonce("n0nce".into()));
    let page = body(&app.handle(&req));
    let head = page.split("</head>").next().unwrap();
    let icon = format!(
        r#"<link rel="icon" href="/assets/icon.{}.png" type="image/png" nonce="n0nce">"#,
        crate::content_hash(b"png")
    );
    assert!(head.contains(&icon), "{head}");
    assert!(head.contains(r#"<link rel="apple-touch-icon" href="/static/touch.png""#));
    assert!(head.contains(r##"<meta name="theme-color" content="#0a84ff">"##));
    assert!(
        head.contains(
            r#"<meta name="apple-mobile-web-app-title" content="Dash &quot;board&quot;">"#
        )
    );
    let css = format!(
        r#"<link rel="stylesheet" href="/assets/app.{}.css" nonce="n0nce">"#,
        crate::content_hash(b"body{}")
    );
    assert!(head.contains(&css), "{head}");
    // After Prism's own stylesheet, so its rules win.
    assert!(head.find("/_prism/prism.").unwrap() < head.find(&css).unwrap());
    assert!(head.contains(
        r#"<link rel="preload" href="/fonts/inter.woff2" as="font" type="font/woff2" crossorigin nonce="n0nce">"#
    ));
    assert!(head.contains(r#"<script src="/static/analytics.js" defer nonce="n0nce"></script>"#));
    assert!(head.contains(r#"<script nonce="n0nce">window.hostReady = true;<\/script></script>"#));
    assert!(head.contains(r#"<link rel="manifest" href="/manifest.json">"#));
    let page_css = head
        .split(r#"<style id="prism-page" nonce="n0nce">"#)
        .nth(1)
        .unwrap();
    assert!(
        page_css.contains("@font-face{font-family:Inter;"),
        "{page_css}"
    );
    assert!(page_css.contains("body{background:#101418}"));
    assert!(page_css.contains("body{font-family:Inter, system-ui}"));
    // Prism's own tags carry the nonce too.
    assert!(page.contains(r#"<style id="prism-ssr" nonce="n0nce">"#));
    assert!(page.contains(r#" nonce="n0nce"></script>"#), "{page}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_first_paint_has_no_style_attribute() {
    let (app, dir) = app("csp", whoami());
    let page = body(&app.handle(&Request::get("/").body(()).unwrap()));
    assert!(!page.contains("style="), "{page}");
    assert!(
        !page.contains(" nonce="),
        "no nonce unless the host gives one"
    );
    let ssr = page
        .split(r#"<style id="prism-ssr">"#)
        .nth(1)
        .unwrap()
        .split("</style>")
        .next()
        .unwrap();
    // One rule per distinct declaration set, scoped to the root.
    let rules: Vec<&str> = ssr.lines().collect();
    assert!(
        rules
            .iter()
            .all(|r| r.starts_with("#prism-root [data-ps=\"")),
        "{ssr}"
    );
    let ids: Vec<&str> = page
        .split(r#"data-ps=""#)
        .skip(1)
        .map(|s| s.split('"').next().unwrap())
        .collect();
    let distinct: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(rules.len(), distinct.len(), "{ssr}");
    assert!(
        ids.len() > distinct.len(),
        "the two padded texts share a rule: {page}"
    );
    // Text cannot close the style element.
    assert_eq!(crate::host::css_text("a</style>"), "a\\3c /style>");
    let _ = std::fs::remove_dir_all(dir);
}

/// An order screen: `404` when its record is missing.
struct Order(Option<u32>);

impl Render for Order {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        match self.0 {
            Some(id) => text(format!("order {id}")),
            None => {
                cx.set_page_status(404);
                text("no such order")
            }
        }
    }
}

#[test]
fn a_matched_route_answers_the_status_it_sets() {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| Whoami))
            .route("/orders/:id", |cx, p| {
                let id = p
                    .get("id")
                    .and_then(|id| id.parse().ok())
                    .filter(|id| *id < 10);
                cx.new_entity(|_| Order(id))
            })
            .route("/admin", |cx, _| {
                cx.set_page_status(403);
                cx.new_entity(|_| Whoami)
            })
            .build(cx);
        cx.set_root(root);
    });
    let (app, dir) = app("status", init);
    let found = app.handle(&Request::get("/orders/3").body(()).unwrap());
    assert_eq!(found.status(), StatusCode::OK);
    assert!(body(&found).contains("order 3"));
    let missing = app.handle(&Request::get("/orders/42").body(()).unwrap());
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    // Still the page, with the screen's own not-found state.
    let page = body(&missing);
    assert!(page.contains(r#"<main id="prism-root">"#), "{page}");
    assert!(page.contains("no such order"), "{page}");
    assert_eq!(
        app.handle(&Request::head("/orders/42").body(()).unwrap())
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.handle(&Request::get("/admin").body(()).unwrap())
            .status(),
        StatusCode::FORBIDDEN
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unknown_paths_answer_404_with_the_not_found_screen() {
    for custom in [false, true] {
        let (app, dir) = app(&format!("404-{custom}"), routed(custom));
        let ok = app.handle(&Request::get("/").body(()).unwrap());
        assert_eq!(ok.status(), StatusCode::OK);
        let missing = app.handle(&Request::get("/nope/nope").body(()).unwrap());
        assert_eq!(missing.status(), StatusCode::NOT_FOUND, "custom: {custom}");
        // Still the page, with its not-found screen and a session cookie.
        let page = body(&missing);
        assert!(page.contains(r#"<main id="prism-root">"#));
        if custom {
            assert!(page.contains("nobody"), "{page}");
        } else {
            assert!(page.contains("/nope/nope"), "{page}");
        }
        let head = app.handle(&Request::head("/nope").body(()).unwrap());
        assert_eq!(head.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(dir);
    }
    // Without a router, every path is the app.
    let (app, dir) = app("404-none", whoami());
    let any = app.handle(&Request::get("/whatever").body(()).unwrap());
    assert_eq!(any.status(), StatusCode::OK);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_page_knows_its_request_origin_and_the_host_context() {
    let (app, dir) = app("context", whoami());
    let app = app.base_path("/dash");
    // Behind a proxy, mounted with `nest` (the prefix stripped).
    let mut req = Request::get("/orders?page=2")
        .header("host", "10.0.0.5:8080")
        .header("x-forwarded-host", "dash.example.com")
        .header("x-forwarded-proto", "https")
        .header("cookie", "theme=dark; other=1")
        .body(())
        .unwrap();
    req.extensions_mut()
        .insert(RequestContext::new(User { id: 42 }));
    let page = body(&app.handle(&req));
    assert!(page.contains("user 42"), "{page}");
    assert!(page.contains("origin https://dash.example.com"), "{page}");
    assert!(
        page.contains("url https://dash.example.com/dash/orders?page=2"),
        "{page}"
    );
    assert!(page.contains("kind page theme dark"), "{page}");
    // `handle_with` instead of an extension; `Forwarded` too.
    let req = Request::get("/dash")
        .header("host", "internal")
        .header("forwarded", "for=1.2.3.4;proto=https;host=\"a.example\"")
        .body(())
        .unwrap();
    let page = body(&app.handle_with(&req, RequestContext::new(User { id: 7 })));
    assert!(page.contains("user 7"), "{page}");
    assert!(page.contains("url https://a.example/dash<"), "{page}");
    // No context: nobody.
    let page = body(&app.handle(&Request::get("/").header("host", "x").body(()).unwrap()));
    assert!(
        page.contains("nobody") && page.contains("origin http://x"),
        "{page}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A live session over channels.
fn open(
    app: &WebApp,
    uri: &str,
    headers: &[(&str, &str)],
    context: Option<User>,
) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    let mut map = HeaderMap::new();
    for (k, v) in headers {
        map.insert(
            http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            HeaderValue::from_str(v).unwrap(),
        );
    }
    let uri: Uri = uri.parse().unwrap();
    let send = move |m: String| tx.send(m).is_ok();
    let session = match context {
        Some(user) => app.connect_with(&uri, &map, RequestContext::new(user), send),
        None => app.connect(&uri, &map, send),
    };
    // Keep the session open while the test reads.
    std::mem::forget(session);
    rx
}

fn next(rx: &mpsc::Receiver<String>) -> Value {
    serde_json::from_str(&rx.recv_timeout(Duration::from_secs(10)).expect("a message")).unwrap()
}

fn texts(v: &Value) -> Vec<String> {
    v["p"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["text"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn a_session_sees_its_handshake_and_context() {
    let (app, dir) = app("socket", whoami());
    let rx = open(
        &app,
        "/ws?path=%2Forders&search=%3Fpage%3D2&w=800&h=600",
        &[
            ("host", "localhost:3000"),
            ("origin", "http://localhost:3000"),
            ("cookie", "theme=light"),
        ],
        Some(User { id: 42 }),
    );
    let reset = next(&rx);
    let t = texts(&reset);
    assert!(t.contains(&"user 42".to_string()), "{t:?}");
    assert!(
        t.contains(&"origin http://localhost:3000".to_string()),
        "{t:?}"
    );
    assert!(
        t.contains(&"url http://localhost:3000/orders?page=2".to_string()),
        "{t:?}"
    );
    assert!(t.contains(&"kind socket theme light".to_string()), "{t:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// The page's `data-page` id.
fn page_id(page: &str) -> String {
    let start = page.find(r#"data-page=""#).expect("a page id") + 11;
    page[start..start + 32].to_string()
}

#[test]
fn the_first_session_takes_the_page_request_over_once() {
    let (app, dir) = app("handoff", whoami());
    let sid = "0123456789abcdef0123456789abcdef";
    let cookie = format!("prism_sid={sid}");
    let page_for = |user: u32| {
        let mut req = Request::get("/orders")
            .header("host", "localhost:3000")
            .header("cookie", &cookie)
            .body(())
            .unwrap();
        // A one-time value, given to the page request only (a flash message).
        req.extensions_mut()
            .insert(RequestContext::new(User { id: user }));
        body(&app.handle(&req))
    };
    let page = page_for(5);
    assert!(page.contains("page /orders of user 5"), "{page}");
    let id = page_id(&page);
    let socket = |id: &str, cookie: &str| {
        let uri = format!("/ws?path=%2Forders&page={id}");
        let headers = [
            ("host", "localhost:3000"),
            ("origin", "http://localhost:3000"),
            ("cookie", cookie),
        ];
        texts(&next(&open(&app, &uri, &headers, Some(User { id: 6 }))))
    };
    // The live session: its handshake's context, and the page's.
    let t = socket(&id, &cookie);
    assert!(t.contains(&"user 6".to_string()), "{t:?}");
    assert!(t.contains(&"kind socket theme -".to_string()), "{t:?}");
    assert!(t.contains(&"page /orders of user 5".to_string()), "{t:?}");
    // A reconnect with the same id: gone.
    let t = socket(&id, &cookie);
    assert!(t.contains(&"no page".to_string()), "{t:?}");
    // Another browser with a page's id: refused (and the page's request gone).
    let id = page_id(&page_for(7));
    let other = "prism_sid=fedcba9876543210fedcba9876543210";
    let t = socket(&id, other);
    assert!(t.contains(&"no page".to_string()), "{t:?}");
    assert!(socket(&id, &cookie).contains(&"no page".to_string()));
    // A HEAD has no live session: nothing is kept for it.
    let waiting = app.inner.handoffs.len();
    let head = Request::head("/orders")
        .header("host", "localhost:3000")
        .body(())
        .unwrap();
    assert_eq!(app.handle(&head).status(), StatusCode::OK);
    assert_eq!(app.inner.handoffs.len(), waiting);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn foreign_origins_are_refused_unless_allowed() {
    let (app, dir) = app("origins", whoami());
    let headers = |origin: &str, host: &str| {
        let mut h = HeaderMap::new();
        h.insert("origin", HeaderValue::from_str(origin).unwrap());
        h.insert("host", HeaderValue::from_str(host).unwrap());
        h
    };
    assert_eq!(
        app.accept_socket(&headers("http://localhost:3000", "localhost:3000")),
        Ok(())
    );
    assert_eq!(
        app.accept_socket(&headers("https://app.example.com", "app.example.com:443")),
        Ok(())
    );
    assert_eq!(
        app.accept_socket(&headers("HTTPS://App.Example.com", "app.example.com")),
        Ok(())
    );
    assert_eq!(
        app.accept_socket(&headers("https://evil.example", "app.example.com")),
        Err(StatusCode::FORBIDDEN)
    );
    assert_eq!(
        app.accept_socket(&headers("null", "app.example.com")),
        Err(StatusCode::FORBIDDEN)
    );
    assert_eq!(
        app.accept_socket(&headers("http://localhost:3001", "localhost:3000")),
        Err(StatusCode::FORBIDDEN)
    );
    // A proxy's public host.
    let mut proxied = headers("https://app.example.com", "127.0.0.1:3000");
    proxied.insert(
        "x-forwarded-host",
        HeaderValue::from_static("app.example.com"),
    );
    assert_eq!(app.accept_socket(&proxied), Ok(()));
    // Not a browser: no Origin.
    assert_eq!(app.accept_socket(&HeaderMap::new()), Ok(()));
    // A refused session ends at once and says why.
    let rx = open(
        &app,
        "/ws?path=%2F",
        &[("origin", "https://evil.example"), ("host", "a")],
        None,
    );
    assert_eq!(
        next(&rx),
        serde_json::json!({ "t": "refused", "status": 403 })
    );
    assert_eq!(app.session_count(), 0);
    let allowed = app.allow_origins(["https://admin.example.com/"]);
    assert_eq!(
        allowed.accept_socket(&headers("https://admin.example.com", "app.example.com")),
        Ok(())
    );
    let any = app_any(&dir);
    assert_eq!(
        any.accept_socket(&headers("https://evil.example", "app.example.com")),
        Ok(())
    );
    let _ = std::fs::remove_dir_all(dir);
}

fn app_any(dir: &std::path::Path) -> WebApp {
    WebApp::new(AppConfig::default(), whoami())
        .data_dir(dir.join("data2"))
        .allow_origins(["*"])
}

#[test]
fn sessions_are_capped_and_idle_ones_end() {
    let (app, dir) = app("limits", whoami());
    let app = app.max_sessions(1).idle_timeout(Duration::from_millis(300));
    let first = open(&app, "/ws?path=%2F", &[], None);
    assert_eq!(next(&first)["t"], "reset");
    assert_eq!(app.session_count(), 1);
    assert_eq!(
        app.accept_socket(&HeaderMap::new()),
        Err(StatusCode::SERVICE_UNAVAILABLE)
    );
    let second = open(&app, "/ws?path=%2F", &[], None);
    assert_eq!(
        next(&second),
        serde_json::json!({ "t": "refused", "status": 503 })
    );
    // Nothing from the page: the session says so and ends, freeing its place.
    assert_eq!(next(&first), serde_json::json!({ "t": "idle" }));
    for _ in 0..100 {
        if app.session_count() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.session_count(), 0);
    assert_eq!(app.accept_socket(&HeaderMap::new()), Ok(()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn open_url_in_tells_the_page_where() {
    let v = crate::session::device_json(
        3,
        prism_core::DeviceCall::OpenUrlIn("/logout".into(), prism_core::UrlTarget::SameTab),
    );
    assert_eq!(
        v,
        serde_json::json!({ "t": "device", "id": 3, "call": "open_url", "url": "/logout", "target": "same" })
    );
}

#[test]
fn the_reconnecting_banner_follows_the_language() {
    let (app, dir) = app("banner", whoami());
    let page = |lang: &str| {
        body(
            &app.handle(
                &Request::get("/")
                    .header("accept-language", lang)
                    .body(())
                    .unwrap(),
            ),
        )
    };
    assert!(page("en").contains(
        r#"<div id="prism-connection" role="status" aria-live="polite" hidden>Reconnecting…</div>"#
    ));
    assert!(page("fr-FR").contains(">Reconnexion…</div>"));
    let _ = std::fs::remove_dir_all(dir);
}

/// Sets a session cookie on its first render and shows the outcome.
#[derive(Default)]
struct SignsIn {
    started: bool,
    outcome: Option<String>,
}

impl Render for SignsIn {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.started {
            self.started = true;
            cx.spawn(async move |this, cx| {
                let r = cx
                    .set_cookie(prism_core::Cookie::new("session", "s1"))
                    .await;
                let outcome = match r {
                    Ok(()) => "ok".to_string(),
                    Err(e) => e.to_string(),
                };
                cx.update_entity(&this, |s, _| s.outcome = Some(outcome));
            });
        }
        let cookie = cx
            .request()
            .and_then(|r| r.cookie("session"))
            .unwrap_or("-")
            .to_string();
        text(format!(
            "cookie {cookie} {}",
            self.outcome.as_deref().unwrap_or("…")
        ))
    }
}

fn signs_in() -> AppInit {
    Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| SignsIn::default());
        cx.set_root(root);
    })
}

const SID: &str = "0123456789abcdef0123456789abcdef";

/// The next message of type `t`.
fn next_of(rx: &mpsc::Receiver<String>, t: &str) -> Value {
    loop {
        let v = next(rx);
        if v["t"] == t {
            return v;
        }
    }
}

/// The page's `POST /_prism/cookie`.
fn redeem(app: &WebApp, ticket: Option<&str>, headers: &[(&str, &str)]) -> Response<Vec<u8>> {
    let mut req = Request::post("/_prism/cookie");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if let Some(t) = ticket {
        req = req.header("x-prism-cookie", t);
    }
    app.handle(&req.body(()).unwrap())
}

#[test]
fn a_session_sets_a_cookie_through_a_one_time_ticket() {
    let (app, dir) = app("cookie", signs_in());
    let cookie = format!("prism_sid={SID}; theme=dark");
    let rx = open(
        &app,
        "/ws?path=%2F",
        &[
            ("host", "localhost:3000"),
            ("origin", "http://localhost:3000"),
            ("cookie", &cookie),
        ],
        None,
    );
    let call = next_of(&rx, "device");
    assert_eq!(call["call"], "set_cookie");
    assert_eq!(call["url"], "/_prism/cookie");
    let ticket = call["ticket"].as_str().unwrap().to_string();
    assert_eq!(ticket.len(), 22, "128 bits, base64url");
    let page = [
        ("host", "localhost:3000"),
        ("origin", "http://localhost:3000"),
        ("sec-fetch-site", "same-origin"),
        ("cookie", cookie.as_str()),
    ];
    // Not the page's own fetch: refused, and the ticket still works.
    assert_eq!(redeem(&app, None, &page).status(), StatusCode::BAD_REQUEST);
    let mut cross = page;
    cross[2] = ("sec-fetch-site", "cross-site");
    assert_eq!(
        redeem(&app, Some(&ticket), &cross).status(),
        StatusCode::FORBIDDEN
    );
    let mut foreign = page;
    foreign[1] = ("origin", "https://evil.example");
    assert_eq!(
        redeem(&app, Some(&ticket), &foreign).status(),
        StatusCode::FORBIDDEN
    );
    let no_origin = [page[0], page[2], page[3]];
    assert_eq!(
        redeem(&app, Some(&ticket), &no_origin).status(),
        StatusCode::FORBIDDEN
    );
    // A GET never redeems it.
    let get = Request::get("/_prism/cookie")
        .header("x-prism-cookie", &ticket)
        .body(())
        .unwrap();
    assert!(app.handle(&get).headers().get("set-cookie").is_none());
    // The page's fetch: the cookie, never cached.
    let r = redeem(&app, Some(&ticket), &page);
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        r.headers()["set-cookie"],
        "session=s1; Path=/; HttpOnly; Secure; SameSite=Lax"
    );
    assert_eq!(r.headers()["cache-control"], "no-store");
    // The app hears it, and its request has the cookie.
    loop {
        let t = texts(&next_of(&rx, "patch"));
        if t.contains(&"cookie s1 ok".to_string()) {
            break;
        }
    }
    // Once only.
    assert_eq!(
        redeem(&app, Some(&ticket), &page).status(),
        StatusCode::FORBIDDEN
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_ticket_works_for_its_own_browser_only() {
    let (app, dir) = app("cookie-sid", signs_in());
    let cookie = format!("prism_sid={SID}");
    let rx = open(
        &app,
        "/ws?path=%2F",
        &[("host", "localhost:3000"), ("cookie", &cookie)],
        None,
    );
    let ticket = next_of(&rx, "device")["ticket"]
        .as_str()
        .unwrap()
        .to_string();
    let page = |cookie: &str| {
        redeem(
            &app,
            Some(&ticket),
            &[
                ("host", "localhost:3000"),
                ("origin", "http://localhost:3000"),
                ("cookie", cookie),
            ],
        )
        .status()
    };
    assert_eq!(
        page("prism_sid=ffffffffffffffffffffffffffffffff"),
        StatusCode::FORBIDDEN
    );
    // Seen by another browser: spent.
    assert_eq!(page(&cookie), StatusCode::FORBIDDEN);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_secure_cookie_on_plain_http_fails_instead_of_vanishing() {
    let (app, dir) = app("cookie-http", signs_in());
    let cookie = format!("prism_sid={SID}");
    let rx = open(
        &app,
        "/ws?path=%2F",
        &[
            ("host", "app.example.com"),
            ("origin", "http://app.example.com"),
            ("cookie", &cookie),
        ],
        None,
    );
    loop {
        let v = next(&rx);
        assert_ne!(v["t"], "device", "no ticket for the page");
        if texts(&v).iter().any(|t| t.contains("needs https")) {
            break;
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn reconnect_tells_the_page_and_ends_the_session() {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Whoami);
        cx.set_root(root);
        cx.reconnect();
    });
    let (app, dir) = app("reconnect", init);
    let rx = open(&app, "/ws?path=%2F", &[], None);
    assert_eq!(next(&rx)["t"], "reset");
    assert_eq!(next(&rx), serde_json::json!({ "t": "reconnect" }));
    for _ in 0..100 {
        if app.session_count() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.session_count(), 0);
    // The first paint has no session to renew: it renders as usual.
    let page = app.handle(&Request::get("/").body(()).unwrap());
    assert_eq!(page.status(), StatusCode::OK);
    let _ = std::fs::remove_dir_all(dir);
}
