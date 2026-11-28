# Web views: another page inside the app

`web_view(url)` shows a web page inside the app: an `<iframe>` on the web, a `WKWebView` on Apple platforms, an `android.webkit.WebView` on Android, a WinUI `WebView2` on Windows, a WebKitGTK `WebKitWebView` on Linux (with prismsdk's `web-view` feature, [below](#linux)). The page and the app talk through messages, both ways. A content editor uses it for its live preview: the rendered page beside the form, reloaded after each change, the selection going back and forth.

```rust
use prism::prelude::*;

struct Editor { selected: Option<String> }

impl Render for Editor {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex_row().flex_1()
            .child(form().w(px(360.)))
            .child(
                web_view("https://preview.example.com/pages/1")
                    .id("preview")                       // to post to it, reload it
                    .flex_1()                            // it has no natural size
                    .accessibility_label("Preview")
                    .on_message(cx.listener(|this, e: &WebMessageEvent, _| {
                        this.selected = e.data::<String>();  // the page's message, as JSON
                    }))
                    .on_load(cx.listener(|_, e: &WebLoadEvent, _| println!("loaded {}", e.url))),
            )
    }
}

// From any handler:
cx.post_web_message("preview", &serde_json::json!({ "select": "section-2" }));
cx.reload_web_view("preview");
```

- `web_view(url)` takes an absolute URL (`https://…`, `file://…` natively). On the web a path (`/preview/1`) is relative to the app's page. A new URL in a later render loads it in the same view. Other schemes leave the view blank (`prism_core::web_view::is_web_view_url`): a `javascript:` URL would run in the app's own page on the web, and `data:` or `blob:` pages are not ones the app talks to. A URL that comes from users (a link to preview) is safe to pass as it is.
- It has no natural size: give it one (`.flex_1()`, `.h(px(…))`). Its background, border and corners are styled like a `div`'s; the background shows while the page loads.
- `cx.post_web_message(id, &value)` sends any `serde::Serialize` value (`post_web_message_json` takes JSON text). Messages posted before the page finished loading are delivered once it has, in order.
- `cx.reload_web_view(id)` reloads the page. On the web, a frame showing another origin than the app's cannot be reloaded in place: it loads the URL the app gave again.
- `on_load` runs each time the web view finishes loading a page. On the web, the URL of a page of another origin cannot be read: `WebLoadEvent::url` is then the URL the app gave.

## The page's side

The page receives the app's messages as `message` events on `window`, everywhere. To send, it calls `window.parent.postMessage` in a browser frame, and `prismHost.postMessage` in a native web view, which Prism adds to the page's main frame (only when the app listens with `on_message`). Embedded frames do not get it: only the main frame speaks for the page. On Apple platforms, Windows and Linux, `prismHost` is there before the page's scripts run. On Android it is added as the page starts loading, so a script that runs first may not see it yet: wait for the `prismhost` event on `window` (messages posted before the page finished loading are held until then). A page that works in both:

```js
function postToApp(value) {
  if (window.prismHost) prismHost.postMessage(JSON.stringify(value));   // native: JSON text
  else window.parent.postMessage(value, "*");                          // web: any cloneable value
}
window.addEventListener("message", (e) => {
  const message = e.data;   // what the app posted
});
```

The app gets the value as JSON either way (`WebMessageEvent::data::<T>()`, `data_value()`), with the origin of the page that sent it (`WebMessageEvent::origin`). In a native web view, the app's messages arrive with an empty `origin` and no `source`; on the web, from the app's origin with `source === window.parent`.

## Origins and the sandbox

**Allowed origins** are the pages the app talks to: by default the URL's own origin, or the ones given with `.allow_origin("https://cdn.example.com")` (once per origin; `"*"` for any). A `file://` page has no origin of its own: talk to it with `"*"`.

- Messages from other origins are dropped, and messages to the page are delivered only while it shows an allowed origin. A message is for the origin the page shows when the app sends it: if the page has moved to another origin by the time it arrives, it is dropped.
- Native web views keep their main frame on them: a link to another origin opens in the system browser. Scripts and redirects may still navigate; their messages are filtered. WebKitGTK does not say which frame a navigation is for, so on Linux a click that would take an embedded frame to an origin the view does not allow opens in the system browser too.
- Natively, nothing opens outside the app unless the user tapped or clicked for it: a link, or a new window (unless a sandbox leaves out `popups()`). Only web links open (in the system browser), and `mailto:` and `tel:` ones (in the mail and phone apps); never another app's own scheme (`intent:`, `market:`, a custom one).
- On the web, a frame cannot be held back: it may navigate anywhere. Only its messages are filtered.
- Messages must come from the main frame of the web view (an `<iframe>` inside the page does not speak for it).
- The app checks the origin again before `on_message` runs, whatever the platform reports. On the web the browser's script reports it, so a modified page can claim any origin the view allows: `WebMessageEvent::origin` tells which page talks, not that its service vouches for the message. Before a privileged effect (a payment, a grant), confirm with that service, server to server.

**Sandbox**: without `.sandbox(…)` the page has the rights of its own origin, as in a browser tab. With it, only what the `WebSandbox` allows:

```rust
web_view(url).sandbox(WebSandbox::new().scripts().same_origin())
```

| `WebSandbox` | Web (`sandbox` attribute) | Apple | Android | Windows | Linux |
|---|---|---|---|---|---|
| `scripts()` | `allow-scripts` | `allowsContentJavaScript` | `setJavaScriptEnabled` | `IsScriptEnabled` | `enable-javascript-markup` |
| `forms()` | `allow-forms` | form navigations cancelled otherwise | not enforced | otherwise a script in every frame stops submissions (`submit`, `form.submit()`, `requestSubmit()`) and every POST of a document is answered 403 before it leaves | form navigations cancelled otherwise, in every frame (a script's `fetch` still sends) |
| `popups()` | `allow-popups allow-popups-to-escape-sandbox` | `target="_blank"`, `window.open` after a tap: the system browser; ignored otherwise | the same | the same (`NewWindowRequested`, user-initiated) | the same (a new-window policy decision with a user gesture) |
| `same_origin()` | `allow-same-origin` | otherwise a private, non-persistent data store | otherwise DOM storage off | otherwise InPrivate | otherwise an ephemeral network session (cookies and storage in memory, for that view) |

A window the page opens with `popups()` is not sandboxed: on the web it is a normal tab (`allow-popups-to-escape-sandbox`), as natively, where it opens in the system browser.

Without `scripts()` there are no messages from the page. A page sandboxed without `same_origin()` has an opaque origin on the web: its messages come from `"null"`, so talk to it with `.allow_origin("*")`.

Changes to the sandbox apply from the next page the view loads (on Apple platforms and Linux, a change of `same_origin` makes a new web view).

## Platforms

| | Web | macOS, iOS, iPadOS, visionOS | Android | Windows | Linux (`web-view` feature) | tvOS, watchOS |
|---|---|---|---|---|---|---|
| View | `<iframe>` | `WKWebView` | `android.webkit.WebView` | WinUI `WebView2` (Edge WebView2 Runtime) | `WebKitWebView` (WebKitGTK 6.0) | an empty box |
| Page → app | `parent.postMessage` | `prismHost.postMessage` (script message handler, main frame) | `prismHost.postMessage` (a message port given to the main frame) | `prismHost.postMessage` (`chrome.webview`) | `prismHost.postMessage` (a script message handler in an isolated world, main frame) | – |
| App → page | `postMessage` to the frame's origin | `message` event (`evaluateJavaScript`) | `message` event (through the same port) | `message` event (`ExecuteScriptAsync`) | `message` event (`webkit_web_view_evaluate_javascript`) | – |

`cx.capabilities().web_view` says whether the platform shows the page. tvOS and watchOS have no WebKit, and on Linux web views need the `web-view` feature: without it the node is an empty box sized by its style, so show something else there:

```rust
if cx.capabilities().web_view {
    web_view(url).flex_1().into_element()
} else {
    button("Open the preview")
        .on_click(Rc::new(move |_, cx: &mut App| cx.open_url(url.clone())))
        .into_element()
}
```

On the web, a strict CSP (`WebApp::strict_csp()`) only allows frames of the app's own origin: add the others with `WebApp::frame_origin("https://preview.example.com")` (`frame-src`). The first paint draws the frame without loading it: the live page loads it once, when the client starts.

## Linux

On Linux, web views use WebKitGTK 6.0 (the GTK 4 build of WebKit), which apps link only with prismsdk's `web-view` feature, so apps without web views do not depend on it:

```toml
[dependencies]
prism = { package = "prismsdk", version = "0.7", features = ["web-view"] }
```

- Building needs `libwebkitgtk-6.0-dev` (in `docker/linux-dev.Dockerfile`); running needs `libwebkitgtk-6.0-4`, which `prism build linux` adds to the `.deb`'s dependencies (`dpkg-shlibdeps`). Debian 12, Ubuntu 23.04, Fedora 38 and later ship it, with GTK 4.8 or later.
- Cookies and storage go to `$XDG_DATA_HOME/<title>/WebKit`, the cache to `$XDG_CACHE_HOME/<title>/WebKit`, shared by the app's web views (a sandbox without `same_origin()` keeps its own in memory).
- `prismHost` lives in the main frame. WebKitGTK does not say which frame posts a message, so Prism's handler sits in an isolated script world, which page scripts never reach: the page's `prismHost` hands it each message, and it posts it with the main frame's origin.
- WebKit runs pages in a sandbox of its own (bubblewrap), which needs user namespaces. Containers often lack them: there, tests start the app with `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1` (never in production).
