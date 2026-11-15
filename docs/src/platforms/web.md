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

Other servers (actix, hyper, …): `prism-web` without default features has no server dependency. `WebApp::handle(&http::Request)` answers pages and static files, and `WebApp::connect(uri, headers, send)` runs a live session over any WebSocket (`session.receive(text)` for incoming messages). Each session is a thread that sleeps until a message, a timer or async work wakes it.

| Path | Cache-Control | |
|---|---|---|
| pages | `private, no-store` | rendered per browser, sets the session cookie |
| `/_prism/client.<hash>.js`, `/_prism/prism.<hash>.css` | `public, max-age=31536000, immutable` | ETag, `304` |
| `/assets/logo.<hash>.png` (`picture("logo.png")`) | `public, max-age=31536000, immutable` | unhashed `/assets/logo.png`: revalidated |
| `/files/*` (uploads) | `private` | per browser |
| `/ws` | – | proxy as a WebSocket |

`prism build web` also writes `public/` (hashed files plus `manifest.json`). Upload it to a CDN and set `PRISM_ASSET_URL=https://cdn.example.com`, or let the CDN cache `/_prism/*` and `/assets/*` from the server. Settings: `PRISM_HOST`, `PRISM_PORT`, `PRISM_BASE_PATH`, `PRISM_ASSET_URL`, `PRISM_DATA_DIR` (or `WebApp::base_path` / `asset_url` / `data_dir`).

## Docker (Linux)

```sh
docker build -f docker/web.Dockerfile --build-arg PACKAGE=my-app -t my-app .
docker run -p 3000:3000 -v my-app-data:/data my-app
```

This is a multi-stage build on Rust 1.99 / Debian bookworm. It produces the server binary, `public/` (the hashed files for a CDN) and the app's assets, in about 160 MB with the counter example. The image is tested with the same jsdom suites as the macOS build.
