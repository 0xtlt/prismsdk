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

**The page's `<head>`.** Use `WebApp::favicon`, `apple_touch_icon`, `theme_color`, `meta(name, content)`, `stylesheet(url)`, `preload_font(url)`, `script(url)`, `inline_script(js)`, `style(css)` (`@font-face`, your own rules), `body_background(color)`, `font_family(family)` and `head(raw_html)`. A URL without a leading `/` or a scheme names an app asset, so `"icon.png"` becomes `/assets/icon.<hash>.png` and is cached as immutable. Stylesheets come after Prism's own. With `Application`, configure the same calls in `.web(|w| w.favicon("icon.png").theme_color("#0a84ff"))`.

**Content-Security-Policy.** The first paint has no `style="…"` attribute. Each distinct node style becomes one rule in `<style id="prism-ssr">`, and nodes carry `data-ps="N"`. The client removes that block once the live tree replaces the page. Prism's own tags and the `<head>` additions get the nonce. After that, the client sets styles through the CSSOM (`el.style`), which CSP allows. A Prism page needs:

```
default-src 'self';
style-src 'self' 'nonce-<nonce>';      # + the CDN if PRISM_ASSET_URL is set
script-src 'self' 'nonce-<nonce>';
img-src 'self' data:;                   # + remote picture hosts
connect-src 'self';                     # the socket (same origin)
```

With [islands](../guide/islands.md), WASM islands also need `'wasm-unsafe-eval'` in `script-src` (it allows compiling WebAssembly, not `eval`). Their modules load from the same origin (or the CDN) through `client.js`, so they need no nonce, and the `.wasm` is fetched under `connect-src`. `WebApp::content_security_policy(&nonce)` returns this policy for the app, and `WebApp::strict_csp()` (or `PRISM_STRICT_CSP=1`) makes the built-in server send it with a fresh nonce on every page.

Without a nonce, the page's `<style>` blocks need `'unsafe-inline'` (or their hashes). Under a strict policy that lacks both, the page still works: the first paint is unstyled until the client connects. The sign-in callback page (`/_prism/auth/callback`) takes the nonce too. `client.js` uses no `eval`, inline handlers or `style` attributes.

**Status codes.** An unknown path renders the router's not-found screen (Prism's or `.not_found(...)`) with `404 Not Found`, so crawlers, monitoring and the host see it. An app without a router answers `200` on every path.

**Leaving the app.** `cx.open_url(url)` loads a URL of the page's own origin in the same tab, such as the host's sign-out page or a download. Other origins open in a new tab. `cx.open_url_in(url, UrlTarget::SameTab | NewTab | Auto)` chooses explicitly. Natively, the system opens the URL whatever the target.

**Socket origin and limits.** A handshake whose `Origin` is not the host it was sent to gets `403`, because browsers send cookies with cross-site WebSocket handshakes. `WebApp::allow_origins(["https://admin.example.com"])` adds origins, and `"*"` allows any. Behind a proxy that rewrites `Host` without setting `X-Forwarded-Host`, list your public origin there. `WebApp::max_sessions(n)` refuses handshakes beyond `n` live sessions with `503`, since each session is a thread with its own runtime. `WebApp::idle_timeout(d)` ends a session that hears nothing from its page for `d`. The page then reconnects at the next pointer, key or focus event. `WebApp::accept_socket(&headers)` runs both checks: the Axum router calls it before upgrading, and other servers should too. A refused `connect` ends at once.

**Reconnecting.** When the socket closes, the client retries after 0.5 s, then doubles the delay up to 30 s, with ±25 % jitter. It retries at once when the browser comes back online or the tab is shown again. `<html data-prism-connection>` holds `connecting`, `online`, `reconnecting`, `offline` or `idle`. While it is `reconnecting` or `offline`, `prism.css` dims the stale page, makes it inert and shows `#prism-connection` ("Reconnecting…" in the app's language; override it with the `prism.reconnecting` translation key). Restyle the banner or hide it with your own CSS, and listen for `prism:connection` events on `window` (`detail.state`, `detail.attempt`). From the second failed attempt on, the client fetches the page. A `401`, a `403` or a redirect means the host signed the user out, so the client reloads the page and the host's own answer shows. It reloads at most once every 30 s.

## Docker (Linux)

```sh
docker build -f docker/web.Dockerfile --build-arg PACKAGE=my-app -t my-app .
docker run -p 3000:3000 -v my-app-data:/data my-app
```

`prism build web --docker` runs the same build from any project in the workspace.

This is a multi-stage build on Rust 1.99 / Debian bookworm. It produces the server binary, `public/` (the hashed files for a CDN) and the app's assets, in about 160 MB with the counter example. The image is tested with the same jsdom suites as the macOS build.
