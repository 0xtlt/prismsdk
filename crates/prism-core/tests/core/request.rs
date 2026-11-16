//! The web request a session renders for (`cx.request()`), and the
//! router's not-found state (the web's 404).

use std::sync::Arc;

use prism_core::prelude::*;
use prism_core::{RequestKind, Runtime, UrlTarget, WebRequest};

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[derive(Debug, PartialEq)]
struct User(u32);

fn request() -> WebRequest {
    WebRequest {
        kind: RequestKind::Socket,
        url: "https://dash.example.com/app/orders?page=2".into(),
        origin: "https://dash.example.com".into(),
        headers: vec![
            ("cookie".into(), "a=1; session=abc=def; theme=dark".into()),
            ("user-agent".into(), "test".into()),
        ],
        context: Some(Arc::new(User(42))),
    }
}

#[test]
fn a_request_reads_its_headers_cookies_path_and_context() {
    let r = request();
    assert_eq!(r.header("User-Agent"), Some("test"));
    assert_eq!(r.header("accept"), None);
    assert_eq!(r.cookie("theme"), Some("dark"));
    assert_eq!(r.cookie("session"), Some("abc=def"));
    assert_eq!(r.cookie("them"), None);
    assert_eq!(r.path(), "/app/orders");
    assert_eq!(r.context::<User>().as_deref(), Some(&User(42)));
    assert_eq!(r.context::<String>(), None);
    let root = WebRequest {
        url: "http://x".into(),
        origin: "http://x".into(),
        ..Default::default()
    };
    assert_eq!(root.path(), "/");
    assert!(format!("{r:?}").contains("dash.example.com"));
}

#[test]
fn the_app_sees_the_request_set_before_init() {
    let init: prism_core::AppInit = Arc::new(|cx: &mut App| {
        // Readable during init (to pick the user's data).
        assert_eq!(cx.request_context::<User>().as_deref(), Some(&User(42)));
        let root = cx.new_entity(|_| Blank);
        cx.set_root(root);
    });
    let with_request: prism_core::AppInit = Arc::new(move |cx: &mut App| {
        cx.set_request(request());
        init(cx)
    });
    let rt = Runtime::new(&with_request);
    assert_eq!(rt.app.page_origin(), Some("https://dash.example.com"));
    assert_eq!(
        rt.app.page_url(),
        Some("https://dash.example.com/app/orders?page=2")
    );
    assert_eq!(rt.app.request().unwrap().kind, RequestKind::Socket);
}

#[test]
fn native_apps_have_no_request() {
    let init: prism_core::AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Blank);
        cx.set_root(root);
    });
    let rt = Runtime::new(&init);
    assert!(rt.app.request().is_none());
    assert_eq!(rt.app.page_origin(), None);
    assert_eq!(rt.app.page_url(), None);
    assert_eq!(rt.app.request_context::<User>(), None);
    assert_eq!(UrlTarget::default(), UrlTarget::Auto);
    assert_eq!(
        [UrlTarget::Auto, UrlTarget::SameTab, UrlTarget::NewTab].map(UrlTarget::name),
        ["auto", "same", "new"]
    );
}

#[test]
fn the_router_says_when_it_shows_not_found() {
    let at = |path: &str, custom: bool| {
        let init: prism_core::AppInit = Arc::new(move |cx: &mut App| {
            let mut b = Router::new()
                .route("/", |cx, _| cx.new_entity(|_| Blank))
                .route("/todos/:id", |cx, _| cx.new_entity(|_| Blank));
            if custom {
                b = b.not_found(|cx, _| cx.new_entity(|_| Blank));
            }
            let root = b.build(cx);
            cx.set_root(root);
        });
        let mut rt = Runtime::with_options(
            &init,
            prism_core::RuntimeOptions {
                path: path.into(),
                ..Default::default()
            },
        );
        rt.render();
        rt.app.shows_not_found()
    };
    for custom in [false, true] {
        assert!(!at("/", custom));
        assert!(!at("/todos/3", custom));
        assert!(!at("/todos/3/", custom));
        assert!(at("/nope/nope", custom));
        assert!(at("/todos", custom));
    }
    // No router: never.
    let init: prism_core::AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Blank);
        cx.set_root(root);
    });
    assert!(!Runtime::new(&init).app.shows_not_found());
}

/// An order screen whose record may be missing.
struct Order(u32);

impl Render for Order {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.0 != 7 {
            // From `render`: no re-render loop.
            cx.set_page_status(404);
        }
        div()
    }
}

#[test]
fn a_matched_route_sets_the_page_status() {
    let at = |path: &str| {
        let init: prism_core::AppInit = Arc::new(|cx: &mut App| {
            let root = Router::new()
                .route("/", |cx, _| cx.new_entity(|_| Blank))
                .route("/orders/:id", |cx, p| {
                    let id = p.get("id").and_then(|id| id.parse().ok()).unwrap_or(0);
                    cx.new_entity(|_| Order(id))
                })
                .route("/gone", |cx, _| {
                    // From the screen's build.
                    cx.set_page_status(410);
                    cx.new_entity(|_| Blank)
                })
                .build(cx);
            cx.set_root(root);
        });
        let mut rt = Runtime::with_options(
            &init,
            prism_core::RuntimeOptions {
                path: path.into(),
                ..Default::default()
            },
        );
        rt.render();
        rt.app.page_status()
    };
    assert_eq!(at("/"), 200);
    assert_eq!(at("/orders/7"), 200);
    assert_eq!(at("/orders/8"), 404);
    assert_eq!(at("/gone"), 410);
    assert_eq!(at("/nope"), 404);
}

#[test]
fn only_success_and_error_statuses_are_kept() {
    let mut app = App::default();
    assert_eq!(app.page_status(), 200);
    for ignored in [0, 100, 301, 302, 399, 600] {
        app.set_page_status(ignored);
        assert_eq!(app.page_status(), 200, "{ignored}");
    }
    app.set_page_status(403);
    app.set_page_status(302);
    assert_eq!(app.page_status(), 403);
    // The last call wins.
    app.set_page_status(204);
    assert_eq!(app.page_status(), 204);
}
