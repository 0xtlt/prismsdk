//! `/components/media`: a web page and a video inside the app.
//!
//! - `web_view`: natively a local page (written next to the app's
//!   temporary files) that talks to the app both ways: it posts "ready"
//!   once loaded and a message per tap on its button; "Send a message"
//!   posts to it, and it answers `got "hello 1"`. On the web, a public
//!   page in a frame. Tests load their own page under a sandbox:
//!   `PRISM_WEB_URL`, `PRISM_WEB_SANDBOX` (`scripts,forms,popups,same-origin`,
//!   empty for none). Android apps get no environment: there, the values
//!   set when the app was built.
//! - `video`: a short film with the platform's controls, playing muted.
//!   Tests point it at a server that wants a request header:
//!   `PRISM_VIDEO_URL` and `PRISM_VIDEO_HEADER` (`Name: value`).
//! - `on_drop_files`: files dropped from the file manager, listed; the
//!   second zone takes pictures only (it lights up for them alone).

use prism::prelude::*;
use prism::ui::{self, Theme};

use crate::strings;

/// The local page: the bridge of the module docs of `web_view`.
const PAGE: &str = r#"<!doctype html>
<meta charset="utf-8"><meta name="viewport" content="width=device-width">
<style>
  body { font: 15px system-ui, sans-serif; margin: 12px; color: #1b1b1f; background: #eef3fb; }
  h3 { margin: 0 0 6px; } button { font: inherit; padding: 6px 12px; }
  #log { margin-top: 8px; color: #444; }
</style>
<h3>Hello from a web page</h3>
<div id="engine"></div>
<button id="send">Send to the app</button>
<div id="log">No message from the app yet</div>
<script>
  function postToApp(value) {
    if (window.prismHost) prismHost.postMessage(JSON.stringify(value));
    else window.parent.postMessage(value, "*");
  }
  var n = 0;
  document.getElementById("send").onclick = function () { n++; postToApp("tap " + n); };
  window.addEventListener("message", function (e) {
    document.getElementById("log").textContent = "From the app: " + JSON.stringify(e.data);
    // Answers, so the app shows that its message arrived.
    postToApp("got " + JSON.stringify(e.data));
  });
  var ua = navigator.userAgent;
  document.getElementById("engine").textContent =
    ua.indexOf("Edg/") > 0 ? "Engine: Microsoft Edge WebView2" : "Engine: " + ua.split(") ").pop();
  postToApp("ready");
</script>
"#;

const VIDEO: &str = "https://media.w3.org/2010/05/sintel/trailer.mp4";
const POSTER: &str = "https://media.w3.org/2010/05/sintel/poster.png";

pub(crate) struct MediaDemo {
    url: String,
    from_page: Option<String>,
    loaded: Option<String>,
    sent: u32,
    ended: bool,
    dropped: Vec<String>,
    dropped_images: Vec<String>,
}

impl MediaDemo {
    pub fn new(cx: &mut Context<Self>) -> Self {
        MediaDemo {
            url: test_env("PRISM_WEB_URL", option_env!("PRISM_WEB_URL"))
                .unwrap_or_else(|| page_url(cx.platform() == Platform::Web)),
            from_page: None,
            loaded: None,
            sent: 0,
            ended: false,
            dropped: Vec::new(),
            dropped_images: Vec::new(),
        }
    }
}

/// The page to show: natively the local page as a `file://` URL.
fn page_url(web: bool) -> String {
    if web {
        return "https://example.com".into();
    }
    let path = std::env::temp_dir().join("prism-counter-page.html");
    if std::fs::write(&path, PAGE).is_err() {
        return "https://example.com".into();
    }
    let path = path.display().to_string().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

impl Render for MediaDemo {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let (screen, body) = crate::page(strings::media_title(cx), cx);
        let from_page = match &self.from_page {
            Some(m) => strings::media_from_page(cx, m.clone()),
            None => strings::media_no_message(cx),
        };
        let loaded = match &self.loaded {
            Some(url) => strings::media_loaded(cx, url.clone()),
            None => strings::media_loading(cx),
        };
        screen.child(
            body.child(
                ui::card()
                    .child(ui::heading(strings::media_web_heading(cx)))
                    .child(text(strings::media_web_intro(cx)).text_color(t.muted))
                    .child(
                        web_view(self.url.clone())
                            .id("demo-page")
                            // A local page has no origin of its own.
                            .allow_origin("*")
                            .on_message(cx.listener(|this, e: &WebMessageEvent, _| {
                                this.from_page = e.data::<String>();
                            }))
                            .on_load(cx.listener(|this, e: &WebLoadEvent, _| {
                                this.loaded = Some(e.url.clone());
                            }))
                            .when_some(test_sandbox(), |w, s| w.sandbox(s))
                            .w_full()
                            .h(px(200.))
                            .rounded(t.radius)
                            .border(1.)
                            .border_color(t.border),
                    )
                    .child(
                        text(from_page)
                            .id("web-message")
                            .accessibility_role(Role::Status),
                    )
                    .child(text(loaded).text_size(13.).text_color(t.muted).truncate())
                    .child(
                        div()
                            .flex_row()
                            .flex_wrap()
                            .gap(8.)
                            .child(ui::button(strings::media_send(cx)).on_click(cx.listener(
                                |this, _, cx| {
                                    this.sent += 1;
                                    let n = this.sent;
                                    cx.post_web_message("demo-page", &format!("hello {n}"));
                                },
                            )))
                            .child(ui::button(strings::media_reload(cx)).secondary().on_click(
                                cx.listener(|this, _, cx| {
                                    this.loaded = None;
                                    cx.reload_web_view("demo-page");
                                }),
                            )),
                    ),
            )
            .child(
                ui::card()
                    .child(ui::heading(strings::media_video_heading(cx)))
                    .child(
                        test_video()
                            .poster(POSTER)
                            .autoplay(true)
                            .muted(true)
                            .alt(strings::media_video_alt(cx))
                            .on_ended(cx.listener(|this, _: &EndedEvent, _| this.ended = true))
                            .w_full()
                            .rounded(t.radius),
                    )
                    .child(
                        text(if self.ended {
                            strings::media_ended(cx)
                        } else {
                            strings::media_playing(cx)
                        })
                        .text_size(13.)
                        .text_color(t.muted),
                    ),
            )
            .child(
                ui::card()
                    .child(ui::heading(strings::media_drop_heading(cx)))
                    .child(drop_zone(
                        "drop-zone",
                        Vec::new(),
                        &self.dropped,
                        strings::media_drop_hint(cx),
                        cx.listener(|this, e: &FileDropEvent, _| {
                            this.dropped = e.files.iter().map(|f| f.name.clone()).collect();
                        }),
                        cx,
                    ))
                    .child(drop_zone(
                        "drop-images",
                        vec!["image/*".into()],
                        &self.dropped_images,
                        strings::media_drop_images_hint(cx),
                        cx.listener(|this, e: &FileDropEvent, _| {
                            this.dropped_images = e.files.iter().map(|f| f.name.clone()).collect();
                        }),
                        cx,
                    )),
            ),
        )
    }
}

/// A zone taking files of `types` (any file when empty), lit while they
/// are dragged over it, listing the last ones dropped.
fn drop_zone(
    id: &str,
    types: Vec<String>,
    dropped: &[String],
    hint: String,
    on_drop: Handler<FileDropEvent>,
    cx: &mut Context<MediaDemo>,
) -> impl IntoElement + use<> {
    let t = Theme::of(cx);
    div()
        .id(id)
        .h(px(80.))
        .p(12.)
        .rounded(t.radius)
        .border(1.)
        .border_color(t.border)
        .items_center()
        .justify_center()
        .drop_hover(move |s| s.bg(t.accent.with_alpha(0.12)).border_color(t.accent))
        .on_drop_files(types, on_drop)
        .child(
            text(if dropped.is_empty() {
                hint
            } else {
                strings::media_dropped(cx, dropped.join(", "))
            })
            .text_color(t.muted),
        )
}

/// The film, or the test server's video with its header.
fn test_video() -> prism::Video {
    let Ok(url) = std::env::var("PRISM_VIDEO_URL") else {
        return video(VIDEO);
    };
    let v = video(url);
    match std::env::var("PRISM_VIDEO_HEADER").ok().and_then(|h| {
        h.split_once(':')
            .map(|(k, v)| (k.to_string(), v.to_string()))
    }) {
        Some((name, value)) => v.header(name, value),
        None => v,
    }
}

/// A test's setting: the environment's, else the one at build time
/// (`option_env!`: Android starts apps without an environment).
fn test_env(name: &str, built: Option<&str>) -> Option<String> {
    std::env::var(name).ok().or(built.map(str::to_string))
}

/// The sandbox a test asks for (`PRISM_WEB_SANDBOX`).
fn test_sandbox() -> Option<WebSandbox> {
    let list = test_env("PRISM_WEB_SANDBOX", option_env!("PRISM_WEB_SANDBOX"))?;
    let mut s = WebSandbox::new();
    for name in list.split(',').map(str::trim) {
        s = match name {
            "scripts" => s.scripts(),
            "forms" => s.forms(),
            "popups" => s.popups(),
            "same-origin" => s.same_origin(),
            _ => s,
        };
    }
    Some(s)
}
