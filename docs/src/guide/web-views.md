# Web views: another page inside the app

`web_view(url)` shows a web page inside the app: an `<iframe>` on the web, a `WKWebView` on Apple platforms, an `android.webkit.WebView` on Android. The page and the app talk through messages, both ways. A content editor uses it for its live preview: the rendered page beside the form, reloaded after each change, the selection going back and forth.

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

The page receives the app's messages as `message` events on `window`, everywhere. To send, it calls `window.parent.postMessage` in a browser frame, and `prismHost.postMessage` in a native web view, which Prism adds to the page before its scripts run (only when the app listens with `on_message`). A page that works in both:

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

- Messages from other origins are dropped, and messages to the page are delivered only while it shows an allowed origin.
- Native web views keep their main frame on them: a link to another origin opens in the system browser. Scripts and redirects may still navigate; their messages are filtered.
- On the web, a frame cannot be held back: it may navigate anywhere. Only its messages are filtered.
- Messages must come from the main frame of the web view (an `<iframe>` inside the page does not speak for it). Android cannot tell frames apart: there the origin is the main frame's URL.

**Sandbox**: without `.sandbox(…)` the page has the rights of its own origin, as in a browser tab. With it, only what the `WebSandbox` allows:

```rust
web_view(url).sandbox(WebSandbox::new().scripts().same_origin())
```

| `WebSandbox` | Web (`sandbox` attribute) | Apple | Android | Windows |
|---|---|---|---|---|
| `scripts()` | `allow-scripts` | `allowsContentJavaScript` | `setJavaScriptEnabled` | `IsScriptEnabled` |
| `forms()` | `allow-forms` | form navigations cancelled otherwise | not enforced | otherwise a script in every frame stops submissions (`submit`, `form.submit()`, `requestSubmit()`) and every POST of a document is answered 403 before it leaves |
| `popups()` | `allow-popups allow-popups-to-escape-sandbox` | `target="_blank"`, `window.open`: the system browser; ignored otherwise | the same | the same (`NewWindowRequested`) |
| `same_origin()` | `allow-same-origin` | otherwise a private, non-persistent data store | otherwise DOM storage off | otherwise InPrivate |

A window the page opens with `popups()` is not sandboxed: on the web it is a normal tab (`allow-popups-to-escape-sandbox`), as natively, where it opens in the system browser.

Without `scripts()` there are no messages from the page. A page sandboxed without `same_origin()` has an opaque origin on the web: its messages come from `"null"`, so talk to it with `.allow_origin("*")`.

Changes to the sandbox apply from the next page the view loads (on Apple platforms, a change of `same_origin` makes a new web view).

## Platforms

| | Web | macOS, iOS, iPadOS, visionOS | Android | Windows | tvOS, watchOS, Linux |
|---|---|---|---|---|---|
| View | `<iframe>` | `WKWebView` | `android.webkit.WebView` | WinUI `WebView2` (Edge WebView2 Runtime) | an empty box |
| Page → app | `parent.postMessage` | `prismHost.postMessage` (script message handler) | `prismHost.postMessage` (`@JavascriptInterface`) | `prismHost.postMessage` (`chrome.webview`) | – |
| App → page | `postMessage` to the frame's origin | `message` event (`evaluateJavaScript`) | `message` event (`evaluateJavascript`) | `message` event (`ExecuteScriptAsync`) | – |

`cx.capabilities().web_view` says whether the platform shows the page. tvOS and watchOS have no WebKit, and Linux has no web view yet (WebKitGTK would add a system dependency): the node is an empty box sized by its style, so show something else there:

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
