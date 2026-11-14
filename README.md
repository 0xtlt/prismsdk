# Prism

Write a UI once in Rust (GPUI-style API), ship it as:

| Target | How it renders | Command |
|---|---|---|
| **Web** | Server-side Rust → HTML/CSS + ~100 lines of JS receiving DOM patches over a WebSocket. **No WASM.** | `mise run web` → http://localhost:3000 |
| **macOS** | Real AppKit views (`NSView`, `NSTextField`, `NSButton`) | `mise run macos` |
| **iOS** | Real UIKit views (`UIView`, `UILabel`, `UIButton`, `UITextField`), UIScene lifecycle | `mise run ios` |

```rust
use prism::prelude::*;

struct Counter { count: i32 }

impl Render for Counter {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().p(24.).gap(12.)
            .child(text(format!("Count: {}", self.count)).text_size(32.).bold())
            .child(button("+1").on_click(cx.listener(|this, _, _| this.count += 1)))
    }
}

fn main() {
    Application::new().title("Counter").run(|cx| cx.new_entity(|_| Counter { count: 0 }));
}
```

## Updating state

- State lives in **entities** (`cx.new(|_| MyView { .. })`), GPUI-style.
- Mutate it in a `cx.listener`: the entity re-renders automatically. Elsewhere, call `cx.notify()`.
- Only notified entities re-render. A clean child entity is neither rendered nor diffed: its mounted views are reused as-is. So split your UI into entities where updates are frequent (an input, a live counter, a list row).
- Derived state: `cx.observe(&other, |this, other, cx| this.total = other.read(cx).total())` runs when `other` changes, then re-renders `this`.
- Each entity renders one root node (a fragment gets wrapped in a `div`).

## Architecture

```
            your app (Render entities)
                      │ render()
                      ▼
 prism-core   Element tree ──► Reconciler ──► Vec<Patch> ◄── UiEvent (click, input)
                                                 │
        ┌────────────────────────┬───────────────┴──────────────┐
        ▼                        ▼                              ▼
   prism-web                prism-native (taffy flexbox, Widgets trait)
   SSR HTML + JSON              ├── prism-macos  (AppKit)
   patches over WS              └── prism-ios    (UIKit)
```

- **`prism-core`**: styles, element builders, entities/`Context`, reconciler, `Backend` trait, `Runtime`. No platform code.
- **`prism-native`**: turns patches into calls on a 10-method `Widgets` trait and runs flexbox layout with taffy. Shared by every native backend.
- **`prism-web`**: std-only HTTP server + tungstenite. One `Runtime` per browser session (LiveView model).
- **`prism-macos`** / **`prism-ios`**: `Widgets` implementations over objc2.
- **`prism`**: facade; picks the backend from features (`web`, `native`) and the target OS.

Layout is a flexbox subset (column by default, like React Native) and maps 1:1 to CSS on the web and to taffy on native, so all targets lay out the same way.

### Adding a platform (Android, Windows, Linux, tvOS…)

1. New crate `prism-<os>` implementing `prism_native::Widgets` (create/style/measure/place native views).
2. An entry point that builds a `Runtime`, feeds `runtime.render()` / `runtime.dispatch(event)` into a `NativeRenderer`, and calls `layout()` on resize.
3. One `cfg` line in `crates/prism/src/lib.rs`.

## Development

```sh
mise install        # Rust 1.99 + macOS/iOS targets
mise run test       # unit tests
mise run check      # type-check host, web and iOS-simulator builds
```

- `PRISM_PORT=8080 mise run web` changes the port.
- `PRISM_SIM="iPhone 17" mise run ios` picks a simulator (slimmed with `simslim` when installed).
- macOS dev hooks: `PRISM_CLICK="+1,+1" PRISM_SNAPSHOT=out.png cargo run -p counter` presses buttons by title, writes a PNG of the window, and quits.

## Status (v0.1)

Primitives: `div`, `text`, `button`, `text_input`. Keyed lists, nested entities, controlled inputs.
Not yet: scrolling, images, timers/async, animations, Android/Windows/Linux backends.
