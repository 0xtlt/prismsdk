# Islands: code that runs in the browser

A Prism web app runs on the server. The browser gets a fixed `client.js` (about 29 KB gzip) that applies DOM patches and sends events back over a WebSocket. That model fits most UI: forms, lists, navigation, dashboards. It is a poor fit when every pointer move must redraw something, because each one would be a round trip.

An **island** is a small part of a page that runs in the browser. It is a Web Component, written in Rust and compiled to WebAssembly (`island("chart")`) or in plain JavaScript (`web_component("time-ago")`). The server gives it props and handles the events it emits. Everything else stays server-driven.

Use an island for:

- a chart or a canvas whose crosshair, tooltip or brush follows the pointer at 60 fps;
- drag-heavy editors, drawing, a map, a color picker;
- something that ticks or animates from data the server gave once (a relative time, a countdown with custom rendering).

Do not use one for forms, validation, navigation or business logic. Those belong in server handlers, where your data and secrets are.

## The server side

```rust
use prism::prelude::*;

struct Dashboard { points: Vec<f64>, selected: Option<(usize, f64)> }

impl Render for Dashboard {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(
                island("chart")                                   // <prism-island-chart>
                    .prop("points", &self.points)                 // any serde::Serialize, as JSON
                    .prop("selected", &self.selected.map(|s| s.0))
                    .on("select", cx.listener(|this, e: &IslandEvent, _| {
                        this.selected = e.detail::<(usize, f64)>();   // the CustomEvent's detail
                    }))
                    .w_full()
                    .h(px(220.))
                    // Children are the fallback: first paint, no JavaScript, native apps.
                    .child(ui::line_chart().series("Values", self.points.iter().copied())),
            )
            .child(button("Add a point").on_click(cx.listener(|this, _, _| this.points.push(42.))))
    }
}
```

- `island(name)` is a WASM island crate (see below). `name` gives the custom element `prism-island-<name>`.
- `web_component(tag).module("time-ago.js")` is a plain JavaScript Web Component. The module is an app asset (hashed like `picture("logo.png")`) or a URL. Without `.module`, the page must define the element itself, for example with `WebApp::script`.
- `.prop(name, &value)` serializes any `serde::Serialize` value as JSON. `.prop_json(name, json)` takes JSON you already have. A prop that changes in a later render updates the **live** island: the element is kept, nothing is re-mounted, and the island gets only the props that changed. A prop no longer set arrives as `null`.
- `.on(name, handler)` handles the island's `CustomEvent`s. `IslandEvent::detail::<T>()` parses the detail (any `serde::Deserialize`, so a `[1, 2.5]` array reads as `(usize, f64)`). `detail_value()` returns a `serde_json::Value`.
- Islands take styles, `.key`, `.id` and accessibility modifiers like any `div`. Give them a size, because their content is not known to the server.
- Their typography is set on the host element too (#294): `text_size`, `font_weight`, `text_color`, `line_height`, `text_align` and the other text styles, in their `hover`, `focus_within` and breakpoint variants (`.md(…)`) as well. The component's shadow root inherits them, so a component that should look like the app's inputs only uses `font: inherit` and `color: inherit`.
- `.native(element)` shows `element` in native apps instead of the children, which stay the web's fallback.

## An island crate

An island is a crate of the app's workspace, built as a `cdylib`:

```toml
# islands/chart/Cargo.toml
[package]
name = "my-app-chart"            # anything; the island's name is the directory's: "chart"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
prism-island = { package = "prismsdk-island", version = "0.6" }
web-sys = { version = "0.3", features = ["HtmlCanvasElement", "CanvasRenderingContext2d", "MouseEvent"] }
```

```toml
# Prism.toml
[web]
islands = ["islands/chart"]      # one line; paths from Prism.toml
island_budget_kb = 40            # gzip per island; `prism build web` fails above (default 40)
```

Add the crate to the workspace members (`members = ["islands/*"]`). It then shares the lock file, and it compiles on the host too, so `cargo test --workspace` and `clippy` keep working: `wasm-bindgen` stubs its imports outside WebAssembly.

```rust
use prism_island::{Host, Island, Props};

struct Chart { host: Host, points: Vec<f64> /* canvas, listeners… */ }

impl Island for Chart {
    /// The element appeared: build into its shadow root, with every prop.
    fn mount(host: Host, props: &Props) -> Self {
        let canvas = host.document().create_element("canvas").unwrap();
        host.root().append_child(&canvas).unwrap();
        Chart { host, points: props.f64s("points").unwrap_or_default() }
    }

    /// The server changed these props (only these).
    fn update(&mut self, changed: &Props) {
        if let Some(points) = changed.f64s("points") {
            self.points = points;
        }
    }

    /// The element left the page (it is dropped next).
    fn unmount(&mut self) {}
}

prism_island::register!(Chart);
```

- `Host::element()` is the custom element, `Host::root()` its open shadow root (created just before `mount`), `Host::document()` its document.
- `Host::emit(name, detail)` dispatches a `CustomEvent` (bubbling, composed) on the element. Any `Into<JsValue>` works as a detail: numbers, strings, `prism_island::array([...])`, `prism_island::Object::new().set("x", 1.)`.
- `Props` reads values without serde: `f64`, `str`, `bool`, `f64s` (number arrays), `array`, `get` (any `JsValue`), `has`. Turn on the `serde` feature for `Props::parse::<T>()` and `Host::emit_json(&value)`, at about 25 KB gzip more.
- Render with the CSSOM (`element.style().set_property(…)`) or a canvas. A `style` attribute or a `<style>` element would need `'unsafe-inline'` under a strict CSP.

The demo is `examples/counter/islands/chart`: a canvas line chart whose crosshair and read-out follow the pointer. A click emits `select`, and "Add a point" on the server side pushes new points as a prop (the **Components › Islands** screen).

## What the browser does

1. **First paint.** The server renders the island as its custom element with the fallback inside: `<prism-island-chart data-island>…</prism-island-chart>`.
2. **Loading.** `client.js` imports an island's module the first time a page shows it, and only then. A page without islands loads nothing more, and a module loads once per page however many elements use it. A WASM island's module is the wasm-bindgen glue plus the element's definition. It fetches the `.wasm` (hashed and immutable, served as `application/wasm` so it compiles while streaming), then mounts.
3. **Rendering.** An island renders into its shadow root. The element's light DOM holds the fallback, which the shadow root hides once the island mounts. `client.js` keeps patching the fallback like any view and never touches the shadow root, so the server and the island never fight over nodes.
4. **Props.** The element's `prismProps` holds every prop (an object without a prototype). A WASM island gets `prismUpdate(changed)`. A plain web component gets each prop as a property once it is defined (setting it earlier would hide the class's own setters), and strings, numbers and booleans as attributes right away (`observedAttributes` works, and so do CSS selectors), in the first paint too.

   Tags and prop names can come from data, so both are checked. A `web_component` tag must be a valid custom element name (`prism_core::island::is_custom_element_name`); any other tag renders the fallback in a plain container. A prop becomes an attribute only with an ordinary name (a letter or `_` first, then letters, digits, `-`, `_`), and never as an event handler (`on…`), `style`, `class`, `id`, `is`, `slot`, `part`, `nonce`, `xmlns…` or Prism's `data-pid`, `data-kind` and `data-island`. It becomes a property only when the component declares it (a setter in its class, or a field of the instance) or the element has no native member of that name, and never when it starts with `_` or `prism`: a prop cannot set `innerHTML`, a handler or the prototype. To pass such a value, give the prop another name and handle it in the component.
5. **Events.** For each `.on(name)`, `client.js` listens on the element and sends `{t: "island", id, name, detail}` over the socket. It only forwards events dispatched on the element itself, not bubbled ones. Do not reuse DOM event names such as `click`: a Prism container around the island would see them too.
6. **Removal.** Removing the element unmounts the island. Moving it within the page, which the patcher does when siblings change, does not. After a reconnect the server sends a fresh tree, so islands mount again with the current props. Keep anything that must survive a reconnect on the server.

## Sizes, and what makes WASM big

Measured on the demo chart (`examples/counter/islands/chart`, wasm-bindgen 0.2.130, Rust 1.99), as the browser downloads it (KB = 1024 bytes):

| | raw | gzip | brotli |
|---|---|---|---|
| JS (wasm-bindgen glue + the element) | 23.6 KB | 5.5 KB | 4.7 KB |
| WASM, `prism build` profile | 38.4 KB | 16.3 KB | 13.9 KB |
| WASM, then `wasm-opt -Oz` | 33.2 KB | 15.1 KB | 13.0 KB |
| **Island total**, without wasm-opt | 62.0 KB | 21.8 KB | 18.6 KB |
| **Island total**, with wasm-opt | 56.8 KB | 20.5 KB | 17.7 KB |

For comparison, `client.js` is about 29 KB gzip. The same chart built in debug is 7.8 MB out of cargo, and 469 KB (89 KB gzip) after wasm-bindgen. With the workspace's `--release` profile it is 52.4 KB (18.8 KB gzip), and wasm-bindgen warns that `strip` removed the section it reads the WebAssembly features from. The size profile and wasm-bindgen's name stripping take the WASM to 16.3 KB gzip, and wasm-opt to 15.1 KB.

What adds weight:

- **serde** (~25 KB gzip): read props through `Props` instead, and emit plain values.
- **Formatting floats** (`format!("{:.1}", v)`, ~10 KB): format in JS instead (`js_sys::Number::to_fixed`), as the demo does.
- **web-sys features**: each one adds glue. Enable only what you call.
- **Panics with messages**: `unwrap_throw()` / `expect_throw()` (wasm-bindgen's `UnwrapThrowExt`) cost less than `unwrap()`.
- **Big crates**: regex, chrono and image codecs. Every dependency ends up in the `.wasm`.

`prism run web`, `prism dev web` and `prism build web` print every island's raw, gzip and brotli size on each build. `prism build web` fails when an island's gzip total exceeds `[web] island_budget_kb` (default 40), and the others warn. The budget makes a dependency that doubles the bundle fail the build the day it is added, rather than show up later on a slow phone.

## How `prism` builds islands

For each crate in `[web] islands`, `prism run|dev|build web`:

1. compiles it for `wasm32-unknown-unknown`. `prism build` and `--release` use `opt-level = "z"`, fat LTO, one codegen unit, `panic = "abort"`, no debug info and no `strip` (wasm-bindgen needs its custom sections and removes names itself). `prism dev` and `run` without `--release` use a faster `opt-level = "s"` profile, so a save rebuilds in seconds. The profiles are passed with `--config`, so your `Cargo.toml` needs none. A missing target is reported as `rustup target add wasm32-unknown-unknown`.
2. runs **wasm-bindgen as a library**, linked into `prism`, so you install no CLI. Its version must match the island's `wasm-bindgen`, because the two share an unstable format. `prism-island` pins the exact version `prism` links (0.2.130), so the lock file resolves to it. If your lock file has another version (another crate forced it), `prism` uses a `wasm-bindgen` CLI of exactly that version from `PATH`. Otherwise it stops with both versions and the fix: `cargo update -p wasm-bindgen --precise 0.2.130`, or `cargo install wasm-bindgen-cli --version <yours>`. A library can only link one version, so matching any version as a library is not possible.
3. runs `wasm-opt -Oz` when it is on `PATH` (binaryen), and says so when it is not.
4. writes `prism-island-<name>.<hash>.js` and `.<hash>.wasm` plus `manifest.json` to `target/prism/islands/<release|dev>/`, and starts the server with `PRISM_ISLANDS_DIR` pointing there. The server serves them as `/_prism/islands/…` with `immutable` caching (from the CDN with `PRISM_ASSET_URL`), and `prism build web` exports them to `public/` with the other hashed files.

`prism dev web` watches the island crates with the rest of the workspace: a save rebuilds the island and restarts the server, and the page reconnects. `prism build web --docker` builds the islands on the host and passes them to the image as a build context. Hosts that build their own server (Axum) call `WebApp::islands_dir(dir)` or set `PRISM_ISLANDS_DIR`. An island that was not built shows its fallback, with a warning on the server and in the console.

## Content-Security-Policy

Islands work under the strict nonce policy ([Web › Content-Security-Policy](../platforms/web.md)):

- Modules are imported by `client.js` from the app's origin (or its CDN), which `script-src 'self'` allows. They need no nonce.
- Compiling WebAssembly needs `'wasm-unsafe-eval'` in `script-src`. That permits WebAssembly compilation only, not `eval`.
- The `.wasm` is fetched, which `connect-src 'self'` (plus the CDN) covers.
- Islands style their shadow roots through the CSSOM, never with `style` attributes.

`WebApp::content_security_policy(&nonce)` returns the policy an app needs, with `'wasm-unsafe-eval'` only when it has WASM islands and the CDN when `asset_url` is set. `WebApp::strict_csp()` (or `PRISM_STRICT_CSP=1`) makes the built-in server and router send it with a fresh nonce on every page.

## Native apps

Native backends see an island as a plain container with its children, so they show the fallback. The demo's fallback is a `ui::line_chart`, drawn natively on macOS, iOS, Android, Linux and Windows. Use `.native(element)` for something else on native only. No other backend changes: islands are web-only by design.

## Limitations

- Islands cannot use wasm-bindgen's `inline_js` or JS snippet modules yet (`prism` stops with a message).
- After a reconnect, islands mount again: state that only lives in the island is lost.
- Prism's shapes and charts draw on the client, so on a page without JavaScript a fallback chart shows its axes but no lines. Add a short text to the fallback, as the demo does.
- `[web] islands` is read as a one-line TOML array.
