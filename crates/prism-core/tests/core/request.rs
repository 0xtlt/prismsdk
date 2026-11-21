//! The web request a session renders for (`cx.request()`), and the
//! router's not-found state (the web's 404).

use std::sync::Arc;

use prism_core::prelude::*;
use prism_core::{
    Cookie, DeviceCall, PlatformRequest, RequestKind, Runtime, SameSite, UrlTarget, WebRequest,
};

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

#[test]
fn cookies_are_safe_by_default_and_written_as_set_cookie() {
    use std::time::Duration;
    let c = Cookie::new("session", "abc");
    assert_eq!(
        c.to_header(),
        "session=abc; Path=/; HttpOnly; Secure; SameSite=Lax"
    );
    assert!(c.is_secure() && !c.is_removal());
    let c = Cookie::new("theme", "dark")
        .path("/app")
        .domain("example.com")
        .max_age(Duration::from_secs(3600))
        .http_only(false)
        .same_site(SameSite::Strict);
    assert_eq!(
        c.to_header(),
        "theme=dark; Path=/app; Domain=example.com; Max-Age=3600; Secure; SameSite=Strict"
    );
    assert_eq!(c.domain_value(), Some("example.com"));
    let gone = Cookie::remove("session");
    assert!(gone.is_removal());
    assert_eq!(gone.value(), "");
    assert_eq!(gone.check(), Ok(()));
}

#[test]
fn cookies_a_browser_would_refuse_are_refused() {
    let refused = |c: Cookie| c.check().unwrap_err();
    assert!(refused(Cookie::new("", "1")).contains("not a cookie name"));
    assert!(refused(Cookie::new("a b", "1")).contains("not a cookie name"));
    assert!(refused(Cookie::new("a=b", "1")).contains("not a cookie name"));
    for bad in ["a b", "a;b", "a,b", "a\"b", "a\\b", "é"] {
        assert!(
            refused(Cookie::new("v", bad)).contains("encode it"),
            "{bad}"
        );
    }
    assert!(refused(Cookie::new("p", "1").path("app")).contains("path"));
    assert!(refused(Cookie::new("p", "1").path("/a;Domain=x")).contains("path"));
    assert!(refused(Cookie::new("d", "1").domain("a;b")).contains("domain"));
    assert!(
        refused(
            Cookie::new("n", "1")
                .secure(false)
                .same_site(SameSite::None)
        )
        .contains("SameSite=None needs Secure")
    );
    assert_eq!(
        Cookie::new("n", "1").same_site(SameSite::None).check(),
        Ok(())
    );
    assert!(refused(Cookie::new("__Secure-id", "1").secure(false)).contains("needs Secure"));
    assert_eq!(
        Cookie::new("__Secure-id", "1")
            .domain("example.com")
            .check(),
        Ok(())
    );
    assert!(refused(Cookie::new("__Host-id", "1").path("/app")).contains("Path=/"));
    assert!(refused(Cookie::new("__Host-id", "1").domain("example.com")).contains("no Domain"));
    assert_eq!(Cookie::new("__Host-id", "1").check(), Ok(()));
    assert!(refused(Cookie::new("big", "x".repeat(5000))).contains("4096"));
}

#[test]
fn a_request_takes_the_cookies_the_browser_now_has() {
    let mut r = request();
    r.apply_cookie(&Cookie::new("theme", "light"));
    assert_eq!(r.cookie("theme"), Some("light"));
    assert_eq!(r.cookie("a"), Some("1"));
    assert_eq!(r.cookie("session"), Some("abc=def"));
    r.apply_cookie(&Cookie::new("new", "x"));
    assert_eq!(r.cookie("new"), Some("x"));
    r.apply_cookie(&Cookie::remove("session"));
    assert_eq!(r.cookie("session"), None);
    assert_eq!(r.header("cookie"), Some("a=1; theme=light; new=x"));
    // The header keeps its place; the others are untouched.
    assert_eq!(r.headers[0].0, "cookie");
    assert_eq!(r.header("user-agent"), Some("test"));
    // No Cookie header yet: one is added.
    let mut bare = WebRequest::default();
    bare.apply_cookie(&Cookie::new("s", "1"));
    assert_eq!(bare.header("cookie"), Some("s=1"));
    bare.apply_cookie(&Cookie::remove("s"));
    assert_eq!(bare.header("cookie"), None);
}

#[test]
fn set_cookie_and_reconnect_are_device_calls() {
    let init: prism_core::AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Blank);
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    rt.take_requests();
    rt.app.set_cookie(Cookie::new("session", "abc"));
    rt.app.reconnect();
    let calls: Vec<DeviceCall> = rt
        .take_requests()
        .into_iter()
        .filter_map(|r| match r {
            PlatformRequest::Device { call, .. } => Some(call),
            _ => None,
        })
        .collect();
    assert_eq!(
        calls,
        [
            DeviceCall::SetCookie(Cookie::new("session", "abc")),
            DeviceCall::Reconnect
        ]
    );
}

/// S10: natively there is no browser id, so `renew_browser_id` answers at
/// once without a device call; on the web it asks the backend.
#[test]
fn renew_browser_id_asks_the_web_backend_only() {
    #[derive(Default)]
    struct Renews(Option<String>);
    impl Render for Renews {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    for platform in [prism_core::Platform::Linux, prism_core::Platform::Web] {
        let slot: Arc<std::sync::Mutex<Option<Entity<Renews>>>> = Default::default();
        let out = slot.clone();
        let init: prism_core::AppInit = Arc::new(move |cx: &mut App| {
            let root = cx.new_entity(|cx: &mut Context<Renews>| {
                cx.spawn(async move |this, cx| {
                    let r = cx.renew_browser_id().await;
                    cx.update_entity(&this, |s, _| s.0 = Some(format!("{r:?}")));
                });
                Renews::default()
            });
            *out.lock().unwrap() = Some(root);
            cx.set_root(root);
        });
        let mut rt = Runtime::with_options(
            &init,
            prism_core::RuntimeOptions {
                path: "/".into(),
                platform: Some(platform),
                ..Default::default()
            },
        );
        rt.render();
        rt.poll();
        let calls: Vec<(u64, DeviceCall)> = rt
            .take_requests()
            .into_iter()
            .filter_map(|r| match r {
                PlatformRequest::Device { id, call } => Some((id, call)),
                _ => None,
            })
            .collect();
        let root = slot.lock().unwrap().unwrap();
        if platform == prism_core::Platform::Web {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].1, DeviceCall::RenewBrowserId);
            assert_eq!(root.read(&rt.app).0, None, "waits for the backend");
            rt.device_reply(calls[0].0, Ok(prism_core::DeviceValue::Done));
            rt.poll();
        } else {
            assert!(calls.is_empty(), "{calls:?}");
        }
        assert_eq!(root.read(&rt.app).0.as_deref(), Some("Ok(())"));
    }
}
