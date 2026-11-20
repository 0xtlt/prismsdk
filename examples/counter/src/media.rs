//! `/components/media`: a web page and a video inside the app.
//!
//! - `web_view`: natively a local page (written next to the app's
//!   temporary files) that talks to the app both ways: it posts "ready"
//!   once loaded and a message per tap on its button; "Send a message"
//!   posts to it, and it answers `got "hello 1"`. On the web, a public
//!   page in a frame.
//! - `video`: a short film with the platform's controls, playing muted.
//! - `on_drop_files`: files dropped from the file manager, listed.

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
}

impl MediaDemo {
    pub fn new(cx: &mut Context<Self>) -> Self {
        MediaDemo {
            url: page_url(cx.platform() == Platform::Web),
            from_page: None,
            loaded: None,
            sent: 0,
            ended: false,
            dropped: Vec::new(),
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
                        video(VIDEO)
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
                    .child(
                        div()
                            .id("drop-zone")
                            .h(px(80.))
                            .p(12.)
                            .rounded(t.radius)
                            .border(1.)
                            .border_color(t.border)
                            .items_center()
                            .justify_center()
                            .drop_hover(move |s| s.bg(t.accent.with_alpha(0.12)))
                            .on_drop_files(
                                Vec::<String>::new(),
                                cx.listener(|this, e: &FileDropEvent, _| {
                                    this.dropped = e.files.iter().map(|f| f.name.clone()).collect();
                                }),
                            )
                            .child(
                                text(if self.dropped.is_empty() {
                                    strings::media_drop_hint(cx)
                                } else {
                                    strings::media_dropped(cx, self.dropped.join(", "))
                                })
                                .text_color(t.muted),
                            ),
                    ),
            ),
        )
    }
}
