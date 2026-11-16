# Web

`prism dev web` serves the app on <http://localhost:3000> and restarts it on
save; `prism build web` makes the release server and its static files.

The app runs on the server: a page request renders plain HTML (usable before
any JavaScript runs), then a small JS client opens `/ws`, the server starts a
session (its own `Runtime`) and streams DOM patches; DOM events go back over
the socket to the Rust handlers. No WASM. The browser reports its size,
touch, orientation and posture, so [breakpoints](../guide/responsive.md) and
[device information](../guide/device-info.md) work without media queries.

## Inside your server, behind a CDN

Prism's web backend is a library. Mount it in Axum next to your API (`examples/axum-server`):

```rust
let prism = Application::new().title("Shop").web_app(|cx| routes(cx)).base_path("/app");
let app = axum::Router::new()
    .route("/api/orders", get(orders))
    .nest("/app", prism.router());          // or .merge(prism.router()) at the root
axum::serve(listener, app).await?;
```

The example draws its own chrome (`Router::new().header(false)`): the app fills the window, its title bar stays put while `scroll().flex_1()` scrolls the list, and its filter lives in the URL (`/app?status=shipped`, `Params::query`, `cx.replace_query`). See [Navigation](../guide/navigation.md).

Other servers (actix, hyper, …): `prism-web` without default features has no server dependency. `WebApp::handle(&http::Request)` answers pages and static files, and `WebApp::connect(uri, headers, send)` runs a live session over any WebSocket (`session.receive(text)` for incoming messages). Each session is a thread that sleeps until a message, a timer or async work wakes it.

| Path | Cache-Control | |
|---|---|---|
| pages | `private, no-store` | rendered per browser, sets the session cookie |
| `/_prism/client.<hash>.js`, `/_prism/prism.<hash>.css` | `public, max-age=31536000, immutable` | ETag, `304` |
| `/assets/logo.<hash>.png` (`picture("logo.png")`) | `public, max-age=31536000, immutable` | unhashed `/assets/logo.png`: revalidated |
| `/files/*` (uploads) | `private` | per browser |
| `/ws` | – | proxy as a WebSocket |

`prism build web` also writes `public/` (hashed files plus `manifest.json`). Upload it to a CDN and set `PRISM_ASSET_URL=https://cdn.example.com`, or let the CDN cache `/_prism/*` and `/assets/*` from the server. Settings: `PRISM_HOST`, `PRISM_PORT`, `PRISM_BASE_PATH`, `PRISM_ASSET_URL`, `PRISM_DATA_DIR` (or `WebApp::base_path` / `asset_url` / `data_dir`).

### Data in the first paint

A screen that loads its data in a task (`cx.spawn` + `cx.background`) renders its loading state on the server, and the data arrives over the socket once it connects: the first paint, which a slow phone shows for a while and which crawlers and tests read, has a spinner and no content. `WebApp::first_paint_wait(timeout)` lets the page wait for that work:

```rust
Application::new()
    .web(|w| w.first_paint_wait(Duration::from_millis(500)))   // or .web_app(routes).first_paint_wait(..)
    .run(routes);
```

The page is rendered, then while `background` work started by `init`, a screen's constructor or a `render` is in flight, the server waits for it, runs the tasks it resumes and renders again; work those tasks start next is waited for too. It sends the page when no work is left, or at the deadline with what is still loading in its loading state, as without a wait (the data then comes over the socket). Timers and channels are not waited for, so a clock, a refresh every 30 seconds or a live feed does not hold the page. The live session's first tree waits the same way before it replaces the page, so the page does not flash back to its loading state when the socket connects. The data is loaded twice, once for the page and once for the session, as any task is today. The default, zero, sends the page at once. Hosts with a `Runtime` of their own get the same with `Runtime::render_settled(timeout)`.

### Layers in the first paint

Layers (`cx.add_layer`: the toaster of `ui::show_toast`, an app-wide confirmation dialog) are part of the server-rendered page, in `<div id="prism-layers">` after `<main id="prism-root">`, styled by the same rules. A toast shown while the page is built, by `init` or a screen's constructor (« Signed in », a flash after a redirect), is on the first paint, before any script runs. The live session builds its own tree, layers included, and replaces both when the socket connects. A toast's timer starts again in the live session: it stays on screen for its whole duration from then on.

### Assets compiled into the binary

`Application::assets(dir)` (or `PRISM_ASSETS_DIR`) names a folder read when the server starts. A server shipped as one file, or in a scratch container, can carry its assets inside the binary instead:

```rust
let assets = EmbeddedAssets::new()
    .file("logo.png", include_bytes!("../assets/logo.png"))
    .file("fonts/Inter.woff2", include_bytes!("../assets/fonts/Inter.woff2"));
Application::new().embedded_assets(assets).run(|cx| cx.new_entity(Home::new));
// or, mounted in your own server: WebApp::new(config, init).embedded_assets(assets)
```

Names are the paths a folder would give them (`img/logo.png`), so `picture("logo.png")` does not change. Embedded files are hashed and served like the folder's (`/assets/logo.<hash>.png`, immutable; unhashed names revalidated), written by `export_static`, and those under `fonts/` become `@font-face` rules. An embedded file wins over the folder's file of the same name; the folder, if there is one, adds the rest. With [rust-embed](https://crates.io/crates/rust-embed), collect its files:

```rust
#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
struct Files;

let assets: EmbeddedAssets = Files::iter()
    .filter_map(|name| Some((name.to_string(), Files::get(&name)?.data)))
    .collect();
```

### Transport and compression

A page loads in two steps. First the server sends HTML that works without JavaScript. Then the client opens `/ws?path=…&tz=…&w=…&h=…` and receives JSON over the socket: a `reset` with the whole live tree, then small `patch` messages. Events go back as JSON text.

**Socket messages.** Axum's WebSocket stack (tungstenite 0.29) has no `permessage-deflate`, so the server never accepts the extension Chrome offers. Compression happens at the application level instead:

- A browser with `DecompressionStream("deflate-raw")` adds `&compress=deflate-raw` to the socket URL.
- For such a page, every message of at least 1 KB (`Frame::MIN_COMPRESSED`) goes out as a **binary frame** of raw DEFLATE, but only when compression makes it smaller.
- Smaller messages, such as most patches, stay text frames.
- Compression runs on the session's thread, at level 3. On the counter's 387 KB first tree, a release build takes about 0.9 ms and sends 35 KB.
- The client inflates binary frames as they arrive and applies messages strictly in arrival order. A text patch that arrives while an earlier frame is still inflating waits in a queue behind it.
- If a frame cannot be inflated, the client closes the socket and reconnects for a fresh tree.
- Browsers without `DecompressionStream` do not ask for compressed frames, and get text only.

With another WebSocket server, frame the messages yourself:

```rust
let compress = prism_web::Frame::negotiated(&uri);
let session = app.connect(&uri, &headers, move |text| match prism_web::Frame::encode(text, compress) {
    prism_web::Frame::Text(t) => out.send_text(t),
    prism_web::Frame::Binary(b) => out.send_binary(b),
});
```

Plain `send(text)` still works: the client reads text frames whether or not it asked for compression.

**HTTP.** `WebApp::handle` gzips its answers when `Accept-Encoding` allows it (q-values are respected, so `gzip;q=0` gets no gzip):

- Pages are gzipped per request (level 6, about 1 ms for 156 KB).
- `client.js`, `prism.css`, island JS and WASM, and app assets are gzipped once and cached by content hash. Unhashed assets are gzipped on each request.
- Only text, JSON, JavaScript, SVG, WASM and `ttf`/`otf` fonts of 1 KB or more are gzipped. PNG, JPEG, WebP, `woff2`, video and PDF are sent as they are.
- Each answer that can be gzipped carries `Vary: Accept-Encoding`.
- The gzipped form of a static file has its own ETag, `"<hash>-gzip"`. `If-None-Match` matches either form, and a weak `W/` match also counts, so revalidation still ends in `304`.
- A response that already has `Content-Encoding` is left alone.

On the counter, a cold load of `/` goes from 283 KB to 63 KB over HTTP, and from 390 KB to 37 KB over the socket. There is no brotli yet. A CDN in front can re-encode static files in brotli.

## HTML attributes

Browser integrations and end-to-end tests read attributes the app sets
itself. `.attr(name, value)`, on every element, adds one to the element's
HTML, in the first paint and live:

```rust
text_input()
    .placeholder("Code")
    .attr("autocomplete", "one-time-code") // the code from a text message
    .attr("data-1p-ignore", "")            // no password manager prompt
    .attr("data-testid", "otp")            // `page.getByTestId("otp")`
```

Setting a name again replaces its value; a name the next render leaves out is
removed. Only names that cannot run code or change how Prism draws are taken
(`prism_core::attributes`):

- `data-*`, lower case (`a-z`, `0-9`, `-`, `_`, `.`), except the ones Prism
  uses itself (`data-pid`, `data-kind`…, `data-prism…`:
  `attributes::RESERVED_DATA_ATTRIBUTES`);
- `autocomplete` (wins over the `current-password` a `secure` field gets by
  default) and `translate` (`"no"` keeps browser translators off a name).

Any other name is ignored, and fails a debug assertion. The rest has a typed
API that also works natively: `inputmode`, `enterkeyhint`, `autocapitalize`,
`autocorrect` and `spellcheck` come from a text input's `keyboard`,
`return_key`, `capitalize` and `autocorrect` ([Forms](../guide/forms.md)),
ARIA from the [accessibility](../guide/accessibility.md) modifiers. Native
backends ignore `.attr`.

## Embedding in an existing server

`crates/prism-web/examples/embed.rs` is a complete host: its own sign-in, a strict CSP, Prism nested at `/app` (`cargo run -p prism-web --example embed`). `crates/prism-web/tests/axum_host.rs` tests the same setup over real sockets.

**Your middleware runs first.** It can refuse a request, and attach two request extensions that Prism reads from the page request and from every socket handshake. A reconnect is a new handshake, so the middleware runs again and the session gets a fresh context.

```rust
async fn host_layer(mut req: Request, next: Next) -> Response {
    let Some(user) = users::from_cookie(req.headers()).await else {
        return Redirect::to("/signin").into_response();       // the socket: a 401
    };
    let nonce = prism_web::CspNonce::random();
    req.extensions_mut().insert(prism_web::RequestContext::new(user));   // cx.request_context::<User>()
    req.extensions_mut().insert(nonce.clone());                          // on Prism's <style>/<script>/<link>
    let mut res = next.run(req).await;
    res.headers_mut().insert("content-security-policy", csp(&nonce).parse().unwrap());
    res
}

let app = axum::Router::new()
    .route("/signout", get(sign_out))
    .nest("/app", prism.base_path("/app").router().layer(middleware::from_fn(host_layer)));
```

Without Axum, pass the context yourself with `WebApp::handle_with(&req, context)` and `WebApp::connect_with(uri, headers, context, send)`, or put it in the `http::Request` extensions and call `handle` / `connect_request`.

**What the app sees.** On the web, `cx.request()` is the page request during the first render, then the socket handshake in the live session. It gives `kind`, `url`, `origin`, `headers`, `header(name)`, `cookie(name)`, `path()` and `context::<T>()`. `cx.request_context::<T>()` returns the host's value, `cx.page_origin()` returns `https://example.com`, and `cx.page_url()` returns the full URL as the browser shows it, query included. The origin comes from the handshake's `Origin` header, or from `X-Forwarded-Proto` / `X-Forwarded-Host` / `Forwarded` / `Host`. All of these return `None` in native apps, which have no request and no address.

**Values the host gives once.** The first paint and the live session are two runs of your app, one for the page request and one for the socket handshake. Something the host hands out once on the page request, such as a flash message it clears after a redirect or a one-time token, is not in the handshake. `cx.page_request()` keeps it: during the first render it is the page request, and in the live session it is that same page request, with the context the host attached to it then (`cx.page_request_context::<T>()`). The page carries a random id (`data-page` on Prism's script), and its first socket sends it back, so the session takes that page's request out of the server's memory. This works once, within a minute of the page, and only from the browser that loaded it. A reconnect gets `None`, so read the value in `init` or on the first render and keep it in your state.

```rust
// The host's middleware, on the page request only: read and clear the flash.
req.extensions_mut().insert(prism_web::RequestContext::new(Flash(message)));

// The app: the same message in the first paint and in the live session.
let flash = cx.page_request_context::<Flash>();
```

**The page's `<head>`.** Use `WebApp::favicon`, `apple_touch_icon`, `theme_color`, `meta(name, content)`, `stylesheet(url)`, `preload_font(url)`, `script(url)`, `inline_script(js)`, `style(css)` (`@font-face`, your own rules), `body_background(color)`, `font_family(family)` and `head(raw_html)`. A URL without a leading `/` or a scheme names an app asset, so `"icon.png"` becomes `/assets/icon.<hash>.png` and is cached as immutable. Stylesheets come after Prism's own. With `Application`, configure the same calls in `.web(|w| w.favicon("icon.png").theme_color("#0a84ff"))`.

**Content-Security-Policy.** The first paint has no `style="…"` attribute. Each distinct node style becomes one rule in `<style id="prism-ssr">`, and nodes carry `data-ps="pHASH"`. The client removes that block once the live tree replaces the page. Prism's own tags and the `<head>` additions get the nonce. After that, the client keeps the same scheme: each distinct style is one rule of a shared sheet (`<style id="prism-styles">`), written through the CSSOM, which CSP allows, and nodes carry its class, a short hash of the style (`p1x5rl6u`), the same name the first paint gives it in `data-ps`. Rules no node uses any more are reused, so the sheet stays the size of the styles on screen. Interaction states (`.hover(…)` and the like) share one `<style id="prism-states">`. A Prism page needs:

```
default-src 'self';
style-src 'self' 'nonce-<nonce>';      # + the CDN if PRISM_ASSET_URL is set
script-src 'self' 'nonce-<nonce>';
img-src 'self' data:;                   # + remote picture hosts
connect-src 'self';                     # the socket (same origin)
```

With [islands](../guide/islands.md), WASM islands also need `'wasm-unsafe-eval'` in `script-src` (it allows compiling WebAssembly, not `eval`). Their modules load from the same origin (or the CDN) through `client.js`, so they need no nonce, and the `.wasm` is fetched under `connect-src`. `WebApp::content_security_policy(&nonce)` returns this policy for the app, and `WebApp::strict_csp()` (or `PRISM_STRICT_CSP=1`) makes the built-in server send it with a fresh nonce on every page.

Without a nonce, the page's `<style>` blocks need `'unsafe-inline'` (or their hashes). Under a strict policy that lacks both, the page still works: the first paint is unstyled until the client connects. The sign-in callback page (`/_prism/auth/callback`) takes the nonce too. `client.js` uses no `eval`, inline handlers or `style` attributes.

**Status codes.** An unknown path renders the router's not-found screen (Prism's or `.not_found(...)`) with `404 Not Found`, so crawlers, monitoring and the host see it. A route that matches but whose data is missing (`/orders/:id` with an unknown id, or another user's) calls `cx.set_page_status(404)` while the first paint renders, in the screen's build or its `render`, and keeps drawing its own « not found » state; `403`, `410` or `500` work the same way. Only `2xx`, `4xx` and `5xx` codes are kept, the last call wins, and `cx.page_status()` reads the status the page will get. The live session and native apps ignore it: the response is already sent, or there is none. An app without a router answers `200` on every path unless it sets a status.

**Leaving the app.** `cx.open_url(url)` loads a URL of the page's own origin in the same tab, such as the host's sign-out page or a download. Other origins open in a new tab. `cx.open_url_in(url, UrlTarget::SameTab | NewTab | Auto)` chooses explicitly. Natively, the system opens the URL whatever the target.

**Cookies from a handler.** Signing in happens in a handler, a button or a form sent over the socket, but only an HTTP response can set a cookie. `cx.set_cookie(cookie)` does it for you: the server gives the cookie a one-time ticket, and the page posts it to `<base>/_prism/cookie`, whose `204` answer carries the `Set-Cookie`. Then `cx.reconnect()` opens a new session at once, without the « Reconnecting… » banner, once the browser has stored the cookies set before it. Your middleware runs on that new handshake and attaches the signed-in user:

```rust
button("Sign in").on_click(cx.listener(|this, _, cx| {
    let (email, code) = (this.email.clone(), this.code.clone());
    cx.spawn(async move |this, cx| {
        // Your server code: checks the code, returns a session token.
        let Some(token) = cx.background(move || sessions::create(&email, &code)).await else {
            return cx.update_entity(&this, |s, _| s.error = Some("Wrong code".into()));
        };
        let cookie = Cookie::new("session", token).max_age(Duration::from_secs(30 * 86400));
        match cx.set_cookie(cookie).await {
            Ok(()) => cx.update(|cx| cx.reconnect()), // the host now sees the user
            Err(e) => cx.update_entity(&this, |s, _| s.error = Some(e.to_string())),
        }
    });
}))
// Signing out: `cx.set_cookie(Cookie::remove("session"))`, then `cx.reconnect()`.
```

- `Cookie::new(name, value)` is `HttpOnly; Secure; SameSite=Lax; Path=/` until you change it with `.path`, `.domain`, `.max_age`, `.http_only(false)`, `.secure(false)` or `.same_site(SameSite::Strict | None)`. Without `max_age`, it lasts until the browser closes. `Cookie::remove(name)` deletes one; give it the path and domain it was set with.
- `cx.set_cookie` on `App` does not wait. `AsyncApp::set_cookie(cookie).await` answers once the browser has the cookie. After that, `cx.request().cookie(name)` shows the new value, even before the reconnect. Cookies are stored one after the other, in the order they were set.
- It fails with `DeviceError::Failed`, instead of being lost without a word, when the cookie is invalid (`Cookie::check`: a name that is not a token, a value with spaces, quotes, commas, `;` or `\`, `SameSite=None` without `Secure`, a `__Host-` or `__Secure-` name without what it needs, more than 4 KB), when it is `Secure` on a plain `http://` page other than `localhost`, or when its `Domain` does not cover the page's host. On `App`, an invalid cookie fails a debug assertion. Natively there are no cookies (keep secrets with [secure storage](../guide/security.md)): `set_cookie` answers `Unavailable` and `reconnect` does nothing.
- `cx.reconnect()` starts the app again with the new handshake's request and context, so the app's state is not kept; keep what must survive in [storage](../guide/data.md) or on your server. The page stays where it is.
- The ticket is 128 random bits, valid for 60 s, used once, and works only for the browser it was issued to (its `prism_sid` cookie). The page sends it in an `X-Prism-Cookie` header, never in the URL, so it stays out of access logs and the history. `POST <base>/_prism/cookie` is refused (`403`) without an `Origin` of the page's host, or with a `Sec-Fetch-Site` other than `same-origin`. The Axum router serves it; a host that calls `WebApp::handle` itself must also pass it that `POST`.
- Safari does not treat `http://localhost` as secure for cookies: during development there, use `.secure(false)` or `https`.

**Socket origin and limits.** A handshake whose `Origin` is not the host it was sent to gets `403`, because browsers send cookies with cross-site WebSocket handshakes. `WebApp::allow_origins(["https://admin.example.com"])` adds origins, and `"*"` allows any. Behind a proxy that rewrites `Host` without setting `X-Forwarded-Host`, list your public origin there. `WebApp::max_sessions(n)` refuses handshakes beyond `n` live sessions with `503`, since each session is a thread with its own runtime. `WebApp::idle_timeout(d)` ends a session that hears nothing from its page for `d`. The page then reconnects at the next pointer, key or focus event. `WebApp::accept_socket(&headers)` runs both checks: the Axum router calls it before upgrading, and other servers should too. A refused `connect` ends at once.

**Reconnecting.** When the socket closes, the client retries after 0.5 s, then doubles the delay up to 30 s, with ±25 % jitter. It retries at once when the browser comes back online or the tab is shown again. `<html data-prism-connection>` holds `connecting`, `online`, `reconnecting`, `offline` or `idle`. While it is `reconnecting` or `offline`, `prism.css` dims the stale page, makes it inert and shows `#prism-connection` ("Reconnecting…" in the app's language; override it with the `prism.reconnecting` translation key). Restyle the banner or hide it with your own CSS, and listen for `prism:connection` events on `window` (`detail.state`, `detail.attempt`). From the second failed attempt on, the client fetches the page. A `401`, a `403` or a redirect means the host signed the user out, so the client reloads the page and the host's own answer shows. It reloads at most once every 30 s.

## Docker (Linux)

```sh
docker build -f docker/web.Dockerfile --build-arg PACKAGE=my-app -t my-app .
docker run -p 3000:3000 -v my-app-data:/data my-app
```

`prism build web --docker` runs the same build from any project in the workspace.

This is a multi-stage build on Rust 1.99 / Debian bookworm. It produces the server binary, `public/` (the hashed files for a CDN) and the app's assets, in about 160 MB with the counter example. The image is tested with the same jsdom suites as the macOS build.
