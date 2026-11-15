# prism

Write a UI once in Rust; ship it as a web server (HTML/CSS/JS) or a native app.

Write a UI once in Rust (GPUI-style API), ship it as a server-rendered web
app (HTML/CSS + a small JS client, no WASM) or a native app: AppKit on macOS,
UIKit on iOS, tvOS and visionOS, SwiftUI on watchOS, GTK 4 on Linux and real
Android views on phones, tablets, Wear OS and Android TV.

```rust,no_run
use prism::prelude::*;

struct Counter { count: i32 }

impl Render for Counter {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().p(24.).gap(12.)
            .child(text(format!("Count: {}", self.count)).text_size(32.))
            .child(button("+1").on_click(cx.listener(|this, _, _| this.count += 1)))
    }
}

fn main() {
    Application::new().title("Counter").run(|cx| cx.new_entity(|_| Counter { count: 0 }));
}
```

This is the crate apps depend on. It re-exports `prism-core` (elements,
styles, entities), `prism-ui` as `prism::ui` and `prism-http` as
`prism::http`, and picks the backend from the target and its features:

| Feature | |
|---|---|
| `native` (default) | the platform's native backend for the compilation target |
| `web` | the web backend (`prism-web`), as a server or inside Axum |
| `db` | SQL with sqlx (`prism-db`) |

Install the `prism` command (`cargo install prism-cli`) to create, run and
bundle apps: `prism new my-app`, `prism dev web`, `prism dev ios`.

Part of [Prism](https://github.com/0xtlt/prism). Guide: [https://0xtlt.github.io/prism/](https://0xtlt.github.io/prism/) · API: [docs.rs/prism](https://docs.rs/prism)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
