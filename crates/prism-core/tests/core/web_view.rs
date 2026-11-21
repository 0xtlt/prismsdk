//! Web views (#62): the node, its options, messages both ways, reloads.

use prism_core::prelude::*;
use prism_core::web_view::{WebViewConfig, origin_of};
use prism_core::{NodeKind, Patch, UiEvent};
use prism_testing::{Harness, mount};

struct Editor {
    url: String,
    sandboxed: bool,
    selected: Option<String>,
    loads: Vec<String>,
}

impl Render for Editor {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().child(
            web_view(self.url.clone())
                .id("preview")
                .flex_1()
                .accessibility_label("Preview")
                .allow_origin("https://preview.example.com/")
                .when(self.sandboxed, |w| {
                    w.sandbox(WebSandbox::new().scripts().same_origin())
                })
                .on_message(cx.listener(|this, e: &WebMessageEvent, cx| {
                    this.selected = e.data::<String>();
                    // Answer the page.
                    cx.post_web_message("preview", &format!("ok {}", e.origin));
                }))
                .on_load(cx.listener(|this, e: &WebLoadEvent, _| this.loads.push(e.url.clone()))),
        )
    }
}

fn editor() -> Harness {
    Harness::new(|cx| {
        let e = cx.new_entity(|_| Editor {
            url: "https://preview.example.com/page/1".into(),
            sandboxed: false,
            selected: None,
            loads: Vec::new(),
        });
        mount(cx, e);
    })
}

fn web(h: &Harness) -> u64 {
    h.host
        .find(|n| n.kind == NodeKind::WebView)
        .expect("the web view is mounted")
}

#[test]
fn a_web_view_is_a_node_with_its_url_and_options() {
    let h = editor();
    let n = h.host.get(web(&h));
    assert_eq!(n.text, "https://preview.example.com/page/1");
    assert_eq!(n.a11y.label.as_deref(), Some("Preview"));
    let c = WebViewConfig::decode(&n.config);
    assert!(c.messages && c.loads);
    assert_eq!(c.sandbox, None);
    assert_eq!(c.origins, ["https://preview.example.com"]);
    assert_eq!(n.config, "messages loads\nhttps://preview.example.com");
}

#[test]
fn messages_reach_the_handler_and_answers_reach_the_page() {
    let mut h = editor();
    let id = web(&h);
    h.dispatch(UiEvent::WebMessage(
        id,
        r#""section-2""#.into(),
        "https://preview.example.com".into(),
    ));
    let e = h.root::<Editor>();
    assert_eq!(e.read(&h.rt.app).selected.as_deref(), Some("section-2"));
    assert_eq!(
        h.host.web_messages,
        [(id, r#""ok https://preview.example.com""#.to_string())]
    );
    h.dispatch(UiEvent::WebLoad(
        id,
        "https://preview.example.com/page/1".into(),
    ));
    assert_eq!(
        h.root::<Editor>().read(&h.rt.app).loads,
        ["https://preview.example.com/page/1"]
    );
    // Other nodes are ignored.
    h.dispatch(UiEvent::WebMessage(id + 100, "1".into(), String::new()));
    assert_eq!(h.host.web_messages.len(), 1);
}

#[test]
fn the_app_posts_and_reloads_by_name() {
    let mut h = editor();
    let id = web(&h);
    h.rt.app
        .post_web_message("preview", &serde_json::json!({ "select": "s1" }));
    h.rt.app.post_web_message_json("preview", "[1, 2]");
    // Not JSON, an unknown name: nothing.
    h.rt.app.post_web_message_json("preview", "{oops");
    h.rt.app.post_web_message("nowhere", &1);
    h.rt.app.reload_web_view("preview");
    h.rt.app.refresh();
    h.render();
    assert_eq!(
        h.host.web_messages,
        [
            (id, r#"{"select":"s1"}"#.to_string()),
            (id, "[1,2]".to_string())
        ]
    );
    assert_eq!(h.host.web_reloads, [id]);
    // After every node of the render.
    let posted = h
        .last
        .iter()
        .position(|p| matches!(p, Patch::PostWebMessage { .. }))
        .unwrap();
    assert!(
        !h.last[posted..]
            .iter()
            .any(|p| matches!(p, Patch::SetText { .. } | Patch::Create { .. }))
    );
}

#[test]
fn only_web_views_take_web_messages() {
    struct Plain;
    impl Render for Plain {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div().id("preview")
        }
    }
    let mut h = Harness::new(|cx| {
        let e = cx.new_entity(|_| Plain);
        mount(cx, e);
    });
    h.rt.app.post_web_message("preview", &1);
    h.rt.app.reload_web_view("preview");
    h.rt.app.refresh();
    h.render();
    assert!(h.host.web_messages.is_empty() && h.host.web_reloads.is_empty());
}

#[test]
fn a_new_url_or_sandbox_updates_the_same_view() {
    let mut h = editor();
    let id = web(&h);
    h.update::<Editor>(|e| {
        e.url = "https://preview.example.com/page/2".into();
        e.sandboxed = true;
    });
    assert_eq!(web(&h), id, "updated in place");
    let n = h.host.get(id);
    assert_eq!(n.text, "https://preview.example.com/page/2");
    let c = WebViewConfig::decode(&n.config);
    assert_eq!(c.sandbox, Some(WebSandbox::new().scripts().same_origin()));
    assert!(c.scripts());
}

#[test]
fn configs_and_origins() {
    let c = WebViewConfig {
        messages: true,
        loads: false,
        sandbox: Some(WebSandbox::new().forms().popups()),
        origins: vec!["*".into()],
    };
    assert_eq!(c.encode(), "messages sandbox=forms,popups\n*");
    assert_eq!(WebViewConfig::decode(&c.encode()), c);
    assert!(!c.scripts());
    assert!(c.allows("https://a.example", "https://anything.example"));
    assert_eq!(
        WebSandbox::new().scripts().popups().attribute(),
        "allow-scripts allow-popups allow-popups-to-escape-sandbox"
    );
    // No origins: the URL's own.
    let own = WebViewConfig::default();
    assert!(own.scripts());
    assert!(own.allows("https://a.example:443/x", "https://a.example"));
    assert!(!own.allows("https://a.example/x", "https://b.example"));
    assert_eq!(
        origin_of("HTTPS://User@Example.com:8443/a?b#c").as_deref(),
        Some("https://example.com:8443")
    );
    assert_eq!(
        origin_of("http://example.com:80").as_deref(),
        Some("http://example.com")
    );
    assert_eq!(origin_of("/relative/path"), None);
    assert_eq!(origin_of("about:blank"), None);
}

#[test]
fn message_data() {
    let e = WebMessageEvent {
        data: r#"{"x":1}"#.into(),
        origin: "https://a.example".into(),
    };
    assert_eq!(e.data::<u32>(), None);
    assert_eq!(e.data_value()["x"], 1);
}

/// Only web pages (and local files natively) load: never `javascript:`,
/// which would run in the app's own page on the web, nor `data:` / `blob:`.
#[test]
fn web_views_load_only_web_pages() {
    use prism_core::web_view::is_web_view_url;
    for ok in [
        "https://example.com/a",
        "HTTP://example.com",
        "file:///app/index.html",
        "/preview/1",
        "preview?x=a:b",
        "//cdn.example.com/page",
        "",
    ] {
        assert!(is_web_view_url(ok), "{ok}");
    }
    for bad in [
        "javascript:alert(1)",
        " JavaScript:alert(1)",
        "java\tscript:alert(1)",
        "\u{1}javascript:alert(1)",
        "data:text/html,<script>alert(1)</script>",
        "blob:https://example.com/1",
        "vbscript:x",
        "myapp://open",
    ] {
        assert!(!is_web_view_url(bad), "{bad:?}");
    }
    // A refused URL leaves the view blank, and a later good one loads.
    let mut h = editor();
    h.update::<Editor>(|e| e.url = "javascript:alert(document.cookie)".into());
    assert_eq!(h.host.get(web(&h)).text, "");
    h.update::<Editor>(|e| e.url = "https://preview.example.com/page/2".into());
    assert_eq!(
        h.host.get(web(&h)).text,
        "https://preview.example.com/page/2"
    );
}

/// S12 (security review): on the web the browser's script reports the
/// origin of a message, so a modified page can send any. The dispatcher
/// keeps the web view's own rules: a message whose origin the view does
/// not allow, or for a view that does not listen, never reaches the app.
#[test]
fn messages_from_origins_the_view_does_not_allow_are_dropped() {
    let mut h = editor();
    let id = web(&h);
    for origin in ["https://evil.example", "http://preview.example.com", ""] {
        h.dispatch(UiEvent::WebMessage(id, r#""forged""#.into(), origin.into()));
    }
    assert_eq!(h.root::<Editor>().read(&h.rt.app).selected, None);
    assert!(h.host.web_messages.is_empty());
    // Case and a trailing slash do not matter.
    h.dispatch(UiEvent::WebMessage(
        id,
        r#""ok""#.into(),
        "HTTPS://Preview.Example.com/".into(),
    ));
    assert_eq!(
        h.root::<Editor>().read(&h.rt.app).selected.as_deref(),
        Some("ok")
    );
}
