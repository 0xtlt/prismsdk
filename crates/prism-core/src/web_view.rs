//! Web views: another web page shown inside the app, with messages both
//! ways.
//!
//! ```
//! use prism_core::prelude::*;
//! use prism_core::{WebMessageEvent, web_view};
//!
//! struct Editor { selected: Option<String> }
//!
//! impl Render for Editor {
//!     fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
//!         div().flex_row().child(
//!             web_view("https://example.com/preview/1")
//!                 .id("preview")
//!                 .flex_1()
//!                 .on_message(cx.listener(|this, e: &WebMessageEvent, _| {
//!                     this.selected = e.data::<String>();
//!                 })),
//!         )
//!     }
//! }
//!
//! // Later, from a handler: a message to the page, then a reload.
//! fn highlight(cx: &mut App) {
//!     cx.post_web_message("preview", &"section-2");
//!     cx.reload_web_view("preview");
//! }
//! ```
//!
//! | Platform | View | App → page | Page → app |
//! |---|---|---|---|
//! | Web | `<iframe>` | `postMessage` to the frame | `parent.postMessage(value, "*")` |
//! | macOS, iOS, iPadOS, visionOS | `WKWebView` | a `message` event on `window` | `prismHost.postMessage(JSON.stringify(value))` |
//! | Android | `android.webkit.WebView` | a `message` event on `window` | `prismHost.postMessage(JSON.stringify(value))` |
//! | Windows | `WebView2` (WinUI, the Edge WebView2 Runtime) | a `message` event on `window` | `prismHost.postMessage(JSON.stringify(value))` |
//! | Linux, with prismsdk's `web-view` feature | `WebKitWebView` (WebKitGTK 6.0) | a `message` event on `window` | `prismHost.postMessage(JSON.stringify(value))` |
//! | tvOS, watchOS, Linux without the feature | none (`capabilities().web_view` is false): an empty box | – | – |
//!
//! A page that runs in both kinds of hosts sends with:
//!
//! ```js
//! function postToApp(value) {
//!   if (window.prismHost) prismHost.postMessage(JSON.stringify(value));
//!   else window.parent.postMessage(value, "*");
//! }
//! window.addEventListener("message", (e) => { /* e.data: what the app posted */ });
//! ```
//!
//! Natively, `prismHost` is in the page's main frame only (embedded frames
//! cannot post as the page). On Android it is added as the page starts
//! loading: a script that runs earlier waits for the `prismhost` event.
//!
//! **Allowed origins** ([`allow_origin`](WebView::allow_origin), by default
//! the URL's own) are the pages the app talks to: messages from other
//! origins are dropped, and messages to the page are only delivered while it
//! shows one of them. Native web views also keep their main frame on them:
//! a link to another origin opens in the system browser. On the web a frame
//! cannot be held back (it may navigate anywhere), only its messages are
//! filtered.
//!
//! WebKitGTK does not say which frame a navigation is for: on Linux a click
//! that would take an embedded frame to an origin the view does not allow
//! opens in the system browser too.
//!
//! **Sandbox** ([`sandbox`](WebView::sandbox)): without it the page has the
//! rights of its own origin, as in a browser tab. With it, only what the
//! [`WebSandbox`] allows: scripts, forms, new windows, its origin's storage.

use crate::a11y::{Accessibility, Accessible};
use crate::element::{Element, FluentBuilder, Handler, IntoElement, NodeKind, VNode};
use crate::style::{Style, Styled};

/// A message a web view's page posted to the app.
#[derive(Clone, Debug, PartialEq)]
pub struct WebMessageEvent {
    /// The message, as JSON.
    pub data: String,
    /// The origin of the page that sent it (`https://example.com`), one the
    /// view allows. Natively the platform's web view reports it; on the web
    /// the browser's script does, so a modified page can claim any allowed
    /// origin. It tells which page talks, not that the page's service
    /// vouches for the message: before a privileged effect (a payment, a
    /// grant), confirm with that service, server to server.
    pub origin: String,
}

impl WebMessageEvent {
    /// The message as `T`, if it parses as one.
    pub fn data<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        serde_json::from_str(&self.data).ok()
    }

    /// The message as a JSON value (`Null` if it does not parse).
    pub fn data_value(&self) -> serde_json::Value {
        serde_json::from_str(&self.data).unwrap_or(serde_json::Value::Null)
    }
}

/// A web view finished loading a page (the first one, a reload, or a page
/// the user navigated to).
#[derive(Clone, Debug, PartialEq)]
pub struct WebLoadEvent {
    /// The page's URL. On the web, a page of another origin than the app's
    /// cannot be read: it is the URL the app gave.
    pub url: String,
}

/// What a sandboxed page may do ([`WebView::sandbox`]). Everything is
/// refused unless allowed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WebSandbox {
    /// Run JavaScript (messages need it).
    pub scripts: bool,
    /// Submit forms. Not enforced on Android.
    pub forms: bool,
    /// Open new windows (`target="_blank"`, `window.open`): natively, in
    /// the system browser. The window is not sandboxed itself (on the web
    /// `allow-popups-to-escape-sandbox`, as the system browser natively).
    pub popups: bool,
    /// Keep its origin: cookies and storage. Without it the web treats the
    /// page as an opaque origin (its messages come from `"null"`), Apple
    /// platforms give it a private, non-persistent store, Windows browses
    /// InPrivate, Linux gives it an ephemeral network session, Android
    /// turns DOM storage off.
    pub same_origin: bool,
}

impl WebSandbox {
    /// Nothing allowed: a static page.
    pub fn new() -> Self {
        Self::default()
    }

    /// Allows JavaScript.
    pub fn scripts(mut self) -> Self {
        self.scripts = true;
        self
    }

    /// Allows submitting forms.
    pub fn forms(mut self) -> Self {
        self.forms = true;
        self
    }

    /// Allows new windows.
    pub fn popups(mut self) -> Self {
        self.popups = true;
        self
    }

    /// Keeps the page's origin (cookies, storage).
    pub fn same_origin(mut self) -> Self {
        self.same_origin = true;
        self
    }

    /// The `sandbox` attribute of an `<iframe>`: `allow-scripts allow-forms…`.
    pub fn attribute(&self) -> String {
        let mut tokens = Vec::new();
        for (on, token) in [
            (self.scripts, "allow-scripts"),
            (self.forms, "allow-forms"),
            (self.popups, "allow-popups allow-popups-to-escape-sandbox"),
            (self.same_origin, "allow-same-origin"),
        ] {
            if on {
                tokens.push(token);
            }
        }
        tokens.join(" ")
    }
}

/// A web view's options, sent to backends as its config
/// ([`Patch::SetConfig`](crate::Patch::SetConfig)): a first line of tokens
/// (`messages`, `loads`, `sandbox=scripts,forms,popups,same-origin`), then
/// the allowed origins, one per line (`*`: any).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebViewConfig {
    /// The app listens to the page's messages: install the bridge.
    pub messages: bool,
    /// The app listens to loads.
    pub loads: bool,
    /// Restrictions of the page, if sandboxed.
    pub sandbox: Option<WebSandbox>,
    /// Origins the app talks to (`https://example.com`, `*` for any). Empty:
    /// the origin of the URL (resolved by the backend, for a URL relative to
    /// the web app's page).
    pub origins: Vec<String>,
}

impl WebViewConfig {
    /// See the type's docs.
    pub fn encode(&self) -> String {
        let mut first = Vec::new();
        if self.messages {
            first.push("messages".to_string());
        }
        if self.loads {
            first.push("loads".to_string());
        }
        if let Some(s) = self.sandbox {
            let mut allowed = Vec::new();
            for (on, name) in [
                (s.scripts, "scripts"),
                (s.forms, "forms"),
                (s.popups, "popups"),
                (s.same_origin, "same-origin"),
            ] {
                if on {
                    allowed.push(name);
                }
            }
            first.push(format!("sandbox={}", allowed.join(",")));
        }
        let mut out = first.join(" ");
        for o in &self.origins {
            out.push('\n');
            out.push_str(o);
        }
        out
    }

    /// Parses [`encode`](Self::encode)'s output.
    pub fn decode(s: &str) -> Self {
        let mut lines = s.split('\n');
        let mut c = WebViewConfig::default();
        for token in lines.next().unwrap_or("").split_whitespace() {
            match token {
                "messages" => c.messages = true,
                "loads" => c.loads = true,
                t => {
                    if let Some(list) = t.strip_prefix("sandbox=") {
                        let mut s = WebSandbox::new();
                        for name in list.split(',') {
                            match name {
                                "scripts" => s.scripts = true,
                                "forms" => s.forms = true,
                                "popups" => s.popups = true,
                                "same-origin" => s.same_origin = true,
                                _ => {}
                            }
                        }
                        c.sandbox = Some(s);
                    }
                }
            }
        }
        c.origins = lines
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        c
    }

    /// Whether the page may run JavaScript.
    pub fn scripts(&self) -> bool {
        self.sandbox.is_none_or(|s| s.scripts)
    }

    /// The allowed origins for a page at `url`: the configured ones, or the
    /// URL's own origin.
    pub fn allowed_origins(&self, url: &str) -> Vec<String> {
        if self.origins.is_empty() {
            origin_of(url).into_iter().collect()
        } else {
            self.origins.clone()
        }
    }

    /// Whether `origin` is one the app talks to, for a page given as `url`.
    pub fn allows(&self, url: &str, origin: &str) -> bool {
        self.allowed_origins(url)
            .iter()
            .any(|o| o == "*" || o.eq_ignore_ascii_case(origin))
    }
}

/// The script a native backend runs in its web view to hand the page the
/// app's message `json` (valid JSON, so a JavaScript literal) as a
/// `message` event on `window`.
///
/// `origin` is the allowed origin the page showed when the message was
/// sent. The script runs later, and the view may have moved to another
/// document by then: it checks, first and without calling anything the
/// page could replace (only `location`, which a page cannot redefine),
/// that the page still shows `origin` (`location.protocol + "//" +
/// location.host`, which is how [`origin_of`] writes it, `file://` too).
/// `None`: no origin to hold to (the app allows every origin, `*`, and the
/// page has none, like `about:blank`): whatever page is there gets it.
///
/// The Apple (`PrismWebView.swift`) and Android (`PrismWeb.java`) backends
/// build the same script.
pub fn message_script(json: &str, origin: Option<&str>) -> String {
    let dispatch =
        format!("window.dispatchEvent(new MessageEvent(\"message\", {{ data: {json} }}));");
    match origin {
        Some(origin) => format!(
            "if (location.protocol + \"//\" + location.host === {}) {dispatch}",
            serde_json::Value::from(origin)
        ),
        None => dispatch,
    }
}

/// The origin of an absolute URL (`https://example.com:8443`), lower-case,
/// without the default port. `None` for a relative URL or one without a
/// host (`about:blank`, `data:`).
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    if host_port.is_empty() {
        return None;
    }
    let host_port = host_port.to_ascii_lowercase();
    let default = match scheme.as_str() {
        "http" | "ws" => Some(":80"),
        "https" | "wss" => Some(":443"),
        _ => None,
    };
    let host_port = match default {
        Some(port) => host_port
            .strip_suffix(port)
            .map(str::to_string)
            .unwrap_or(host_port),
        None => host_port,
    };
    Some(format!("{scheme}://{host_port}"))
}

/// Whether a web view may load `url` ([`web_view`]): an `http:`, `https:`
/// or (natively) `file:` URL, or a path relative to the app's page. Other
/// schemes are refused: a `javascript:` URL would run in the app's own
/// page on the web, and `data:`, `blob:` or an app's custom scheme are not
/// pages the app talks to. The scheme is read as browsers read it:
/// surrounding spaces and control characters are ignored, and so are tabs
/// and newlines inside (`java\tscript:`).
pub fn is_web_view_url(url: &str) -> bool {
    let url: String = url
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    // A scheme: a letter, then letters, digits, `+`, `-` or `.`, before a
    // `:` that comes before any `/`, `?` or `#`.
    let end = url.find([':', '/', '?', '#']);
    let scheme = match end {
        Some(i) if url[i..].starts_with(':') => &url[..i],
        _ => return true,
    };
    let is_scheme = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !is_scheme {
        return true;
    }
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "file"
    )
}

/// A web page shown inside the app (see the [module docs](self)). `url` is
/// absolute; on the web a path (`/preview/1`) is relative to the app's page.
/// Give it a size (`.flex_1()`, `.h(…)`): it has no natural one.
///
/// Only `http:`, `https:` and (natively) `file:` URLs and relative paths
/// are loaded ([`is_web_view_url`]); with any other the view stays blank.
pub fn web_view(url: impl Into<String>) -> WebView {
    let mut node = VNode::new(NodeKind::WebView);
    let url = url.into();
    node.text = if is_web_view_url(&url) {
        url
    } else {
        String::new()
    };
    WebView {
        node,
        config: WebViewConfig::default(),
    }
}

/// A web view under construction. See [`web_view`].
pub struct WebView {
    node: VNode,
    config: WebViewConfig,
}

impl WebView {
    /// An origin the app talks to (`https://example.com`), or `"*"` for any.
    /// Call it once per origin; without it, only the URL's own origin.
    pub fn allow_origin(mut self, origin: impl Into<String>) -> Self {
        let origin = origin.into();
        let origin = if origin == "*" {
            origin
        } else {
            origin_of(&origin).unwrap_or(origin)
        };
        if !self.config.origins.contains(&origin) {
            self.config.origins.push(origin);
        }
        self
    }

    /// Restricts the page to what `sandbox` allows (see [`WebSandbox`]).
    pub fn sandbox(mut self, sandbox: WebSandbox) -> Self {
        self.config.sandbox = Some(sandbox);
        self
    }

    /// Called when the page posts a message (see the [module docs](self)
    /// for the page's side). Only from the allowed origins.
    pub fn on_message(mut self, handler: Handler<WebMessageEvent>) -> Self {
        self.node.on_web_message = Some(handler);
        self.config.messages = true;
        self
    }

    /// Called each time the web view finishes loading a page.
    pub fn on_load(mut self, handler: Handler<WebLoadEvent>) -> Self {
        self.node.on_web_load = Some(handler);
        self.config.loads = true;
        self
    }

    /// Stable identity among siblings (see `Div::key`): a keyed web view
    /// keeps its page when its siblings move.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.node.key = Some(key.into());
        self
    }

    /// A name to address the web view from code:
    /// [`App::post_web_message`](crate::App::post_web_message),
    /// [`App::reload_web_view`](crate::App::reload_web_view).
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.node.id = Some(id.into());
        self
    }
}

impl Styled for WebView {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.node.style
    }
}

impl Accessible for WebView {
    fn a11y_mut(&mut self) -> &mut Accessibility {
        &mut self.node.a11y
    }
}

impl FluentBuilder for WebView {}

impl IntoElement for WebView {
    fn into_element(self) -> Element {
        let WebView { mut node, config } = self;
        node.config = config.encode();
        Element::Node(Box::new(node))
    }
}

#[cfg(test)]
mod tests {
    use super::message_script;

    /// Runs `script` with node in a page showing `protocol` and `host`;
    /// returns the `data` of each `message` event it dispatched, as JSON
    /// (`None` without node).
    fn run(script: &str, protocol: &str, host: &str) -> Option<String> {
        const PAGE: &str = r#"
            const vm = require("node:vm");
            const [script, protocol, host] = process.argv.slice(1);
            const got = [];
            class MessageEvent { constructor(type, init) { this.type = type; this.data = init.data; } }
            const window = { dispatchEvent: (e) => got.push([e.type, e.data]) };
            vm.runInNewContext(script, { window, MessageEvent, location: { protocol, host } });
            process.stdout.write(JSON.stringify(got));
        "#;
        let out = match std::process::Command::new("node")
            .args(["-e", PAGE, script, protocol, host])
            .output()
        {
            Ok(out) => out,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("no node: the delivery script is not run");
                return None;
            }
            Err(e) => panic!("node: {e}"),
        };
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Some(String::from_utf8(out.stdout).unwrap())
    }

    /// S32: a message for a page of one origin, run after the view moved
    /// to a page of another, is not handed to it.
    #[test]
    fn s32_a_message_reaches_only_the_origin_it_was_sent_to() {
        let json = r#"{"token":"s3cret"}"#;
        let script = message_script(json, Some("https://app.example:8443"));
        let Some(same) = run(&script, "https:", "app.example:8443") else {
            return;
        };
        assert_eq!(same, r#"[["message",{"token":"s3cret"}]]"#);
        for (protocol, host) in [
            ("https:", "evil.example"),
            ("https:", "app.example"),
            ("http:", "app.example:8443"),
        ] {
            assert_eq!(
                run(&script, protocol, host).unwrap(),
                "[]",
                "{protocol}//{host}"
            );
        }
        // Local files: `file://`.
        let file = message_script(json, Some("file://"));
        assert_eq!(
            run(&file, "file:", "").unwrap(),
            r#"[["message",{"token":"s3cret"}]]"#
        );
        assert_eq!(run(&file, "https:", "evil.example").unwrap(), "[]");
        // No origin to hold to (`*`): whatever page is there.
        let any = message_script(json, None);
        assert_eq!(
            run(&any, "https:", "other.example").unwrap(),
            r#"[["message",{"token":"s3cret"}]]"#
        );
        // An origin is a string literal, whatever it holds.
        let odd = message_script(
            "1",
            Some(r#"https://a"); window.dispatchEvent({type:"x",data:2}); ("#),
        );
        assert_eq!(run(&odd, "https:", "a").unwrap(), "[]");
    }
}
