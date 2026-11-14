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

## Accessibility

Accessibility is built in, not opt-in. The core resolves one description per node (role, label, hint, value, checked, disabled, hidden) and each backend maps it to the platform API:

| | Web | iOS | macOS |
|---|---|---|---|
| API | ARIA + semantic HTML (`<main>`, `<h1>`, `<button>`, `role=switch`, `tablist`) | `UIAccessibility` traits, labels, values | `NSAccessibility` roles, labels, press action |
| Keyboard | Tab/Enter/Space on custom buttons, arrow keys in tab bars, visible focus | — | Full Keyboard Access, Space/Return, focus ring |
| Navigation | focus moves to the new screen's title; document title follows | native | window title follows |

Defaults are automatic: a clickable `div` becomes a button named after its text ("Try Prism on the web, Done"), inputs use their placeholder, toggles expose their state. Refine with:

```rust
text("Settings").heading(1)
div().on_click(..).accessibility_label("Open profile").accessibility_hint("Shows your account")
text("›").accessibility_hidden()
```

## Components (`prism::ui`)

A themed, accessible component library: `button`, `switch`, `checkbox`, `text_field`, `card`, `heading`, `badge`, `list_row`, `divider`.

Three levels of customization:

1. **Theme**: `ui::set_theme(cx, Theme::dark().accent(rgb(0x0A84FF)).radius(0.))`. It also switches the native chrome (bars, native controls) to dark.
2. **Per instance**: `ui::button("Delete").danger().large()`, plus `.style(|s| s.corner_radius = 0.)` for anything else.
3. **Replace a component app-wide**: `Theme::light().override_component(|b: ui::Button, cx| my_button(b, cx))`, or implement `ui::Component` for your own and register it with `prism_ui::component!(MyThing)`.

Custom or native: `ui::button("Save").native()` / `ui::switch(..).native()` per instance, or `Theme::prefer_native(true)` everywhere (UIButton/UISwitch, NSButton/NSSwitch, `<button>`/`<input role=switch>`). Raw native primitives remain available: `button()`, `toggle()`, `text_input()`.

## Navigation

A URL-style router, rendered with each platform's native navigation:

```rust
Application::new().title("Prism").run(|cx| {
    let store = cx.new_entity(|_| Store::new());
    Router::new()
        .route("/", move |cx, _| cx.new_entity(|cx| Home::new(store, cx)))
        .route("/todos/:id", move |cx, p| {
            let id = p.parse("id").unwrap_or(0);
            cx.new_entity(|cx| TodoDetail::new(store, id, cx))
        })
        .build(cx)
});

// A route's view returns a screen; its title goes in the navigation bar.
screen("Details").child(...)

// Anywhere:
cx.navigate("/todos/2");   cx.back();   cx.replace("/login");   cx.reset("/");
div().on_click(navigate_to("/todos/2"))   // handler helpers: navigate_to, go_back
```

| | Stack | Back | Deep links |
|---|---|---|---|
| **iOS** | `UINavigationController` (large titles, push animation) | system button + edge swipe | `PRISM_PATH=/todos/1` |
| **macOS** | header bar (title, Back button) | Back button, Go ▸ Back ⌘[ | `PRISM_PATH=/todos/1` |
| **Web** | header bar | header button + browser Back/Forward (History API) | any URL, server-rendered |

**Tabs**: `.tab("Todos", "checklist", "/")` per tab (SF Symbol name; a glyph on the web). Each tab keeps its own stack. `navigate()` to a tab's root path switches tab. iOS: `UITabBarController`. macOS: segmented control. Web: ARIA tablist, URL follows the tab.

**Scrolling**: `scroll()` (`UIScrollView` / `NSScrollView` / `overflow:auto`). A screen starting with a scroll view lets it run under the bars, so iOS large titles collapse natively.

Patterns: `/`, `/todos`, `/todos/:id`, `/files/*rest`. Unknown paths show a "Not found" screen (or `.not_found(...)`). Opening a deep link puts `/` underneath so Back works.

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
- **`prism-ui`**: component library on top of the core (themes, overrides, native escape hatch).
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
- macOS dev hooks: `PRISM_CLICK="+1|Dark mode" PRISM_SNAPSHOT=out.png cargo run -p counter` presses controls by title or accessibility label (`|`-separated), writes a PNG of the window, and quits.

## Status (v0.1)

Primitives: `div`, `text`, `button`, `text_input`, `toggle`, `scroll`, `screen`, `Router` (stacks + tabs). Component library `prism::ui`. Accessibility on all backends. Light/dark appearance.
Not yet: modals/sheets, images, timers/async, animations, Android/Windows/Linux backends.
