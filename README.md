# Prism

Write a UI once in Rust (GPUI-style API), ship it as:

| Target | How it renders | Command |
|---|---|---|
| **Web** | Server-side Rust → HTML/CSS + a small JS client receiving DOM patches over a WebSocket. **No WASM.** | `prism dev web` → http://localhost:3000 |
| **macOS** | Real AppKit views (`NSView`, `NSTextField`, `NSButton`), menu bar, tray | `prism dev macos` |
| **iOS** | Real UIKit views (`UIView`, `UILabel`, `UIButton`, `UITextField`), widgets + Dynamic Island | `prism dev ios` (simulator) / `prism run ios --device "My iPhone"` |

## Quick start

```sh
mise run cli                 # installs the `prism` command
prism new my-app && cd my-app
prism dev web                # rebuilds and restarts on every save, on the same screen
prism dev ios                # same, in the iOS simulator
prism run ios --device "My iPhone"
prism build macos            # release .app with widgets and icon
prism devices                # simulators and paired iPhones
```

`Prism.toml` configures the app: name, `bundle_id`, `version`, `icon` (one 1024×1024 PNG, compiled with `actool` for iOS and macOS), and `[ios] team` / `device` / `simulator`. Real devices are signed with your Apple Development identity and a matching provisioning profile (found automatically, or `PRISM_PROFILE`), then installed with `devicectl`. The bundling scripts and Apple sources are embedded in the CLI, so projects don't need this repository's scripts.

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

## Async, network, storage

```rust
// Runs on the UI thread; await timers or background work in between.
cx.spawn(async move |this, cx| {
    cx.sleep(Duration::from_secs(1)).await;
    let quote = cx.background(|| prism::http::Request::get("https://api.github.com/zen").send_blocking()).await;
    cx.update_entity(&this, |this, _| this.quote = quote.ok().map(|r| r.text()));
});

cx.storage().set("todos", &data);   // App Group (iOS), Application Support (macOS), per browser (web)
```

## Responsive

```rust
// Styles, from a width up (like Tailwind): phones get a column, tablets a row.
div().flex_col().gap(12.).md(|s| s.flex_row().gap(24.))
text("Swipe to delete").hidden_from(Breakpoint::Medium)   // phones only
sidebar().visible_from(Breakpoint::Expanded)              // laptops and up

// Structure: anything else.
if cx.breakpoint() >= Breakpoint::Expanded { two_panes() } else { one_pane() }
cx.on_resize(|(w, h), app| { … });                        // every size change
```

| Breakpoint | Width | Typically |
|---|---|---|
| `Compact` | < 600 | phones |
| `Medium` (`md`) | ≥ 600 | tablets portrait, small windows, phones in landscape |
| `Expanded` (`lg`) | ≥ 1024 | iPad landscape / 13", laptops |
| `Large` (`xl`) | ≥ 1440 | desktops |

Rules are resolved in the core for the current window, so every backend agrees and the web needs no media queries: the iOS window (rotation, Split View, iPad), the macOS window (resize) and the browser (resize, plus a cookie so the server's first render already fits) report their size, and the app re-renders when it crosses a breakpoint. On wide browser windows the tab bar becomes a sidebar.

## Platform-specific UI

```rust
text("Swipe left to delete").only(Platform::MOBILE)       // iOS, Android
text("Right-click for more").except(Platform::MOBILE)     // macOS, Windows, Linux, web
apple_pay_button().only(Platform::APPLE)
div().p(16.).on(Platform::Web, |s| s.p(24.)).md(|s| s.flex_row())   // combine with breakpoints

match cx.platform() {
    Platform::Ios | Platform::Android => mobile_onboarding(),
    Platform::Web => landing_page(),
    _ => desktop_welcome(),
}
```

Platforms: `Ios`, `Android`, `Macos`, `Windows`, `Linux`, `Web`; sets with `|` or `Platform::MOBILE`, `DESKTOP`, `APPLE`, `NATIVE` (all but web). Excluded elements are left out of the tree (no hidden view). `cx.platform()` is the runtime platform (`Web` for the web backend, even on a phone browser: use breakpoints for screen size); `prism::PLATFORM` is the same at compile time. For code that only compiles on one platform, use `cfg`: `#[cfg(target_os = "ios")]`, `#[cfg(feature = "web")]` (your app's feature), or per-platform files with `#[cfg_attr(target_os = "ios", path = "share_ios.rs")]`.

## Device information

```rust
let d = cx.device_info();
if d.is_tablet() && d.is_landscape() { two_columns() } else { one_column() }
if d.os.version.at_least(18, 0) { new_feature() }
d.screen.diagonal            // Some(6.3) inches
if let Some(b) = &d.browser { /* web: "Safari", 18, WebKit, standalone (installed) */ }

div().flex_col().landscape(|s| s.flex_row())                // style rules, like breakpoints
    .posture(Posture::Tabletop, |s| s.justify_end())
    .on_device(DeviceKind::Tablet, |s| s.p(32.))
ui::dual_pane(list, detail)   // one pane per side of a foldable's hinge, side by side on tablets
cx.on_device_info_change(|d, cx| …);   // the tree already re-renders on rotation, fold, split view
```

| Field | iOS / iPadOS | macOS | Web |
|---|---|---|---|
| `kind` (phone, tablet, desktop, tv) | idiom | desktop | user agent + touch (iPads in desktop mode included) |
| `os`, `model`, `model_name` | `iOS 18.2`, `iPhone17,1`, `iPhone 16 Pro` | `macOS 15.1`, `Mac15,6` | the visitor's OS, Android model, client hints |
| `screen` (pt, scale, inches, ppi, Hz, cutout) | screen pixels ÷ the model's ppi (table generated from Xcode's simulator profiles: `scripts/apple-models.py`) | `CGDisplayScreenSize` | estimated inches |
| `orientation`, `rotation` | interface orientation | screen shape | `screen.orientation` |
| `posture`, `hinge_angle`, `fold` (Flat, Book, Tabletop, Closed) | `UIHingeInteraction` + division reserved regions (iOS 27.1+, iPhone Duo) | – | `devicePosture`, viewport segments |
| `window_mode` (fullscreen, split, slide over, windowed) | Split View / Stage Manager | fullscreen or windowed | installed app or browser tab |
| `browser` | `None` | `None` | always set |

On the web the server reads the `User-Agent` for the first HTML, then the page reports touch, screen, orientation, posture and client hints (and remembers them in a cookie for the next visit). Android: types ready, backend to come.

## Data: local, synced, SQL

```rust
cx.storage().set("draft", &text);              // this device (web: this browser, on the server)
cx.cloud_storage().set("theme", "dark");       // the user's devices: iCloud key-value store
cx.on_cloud_change(|keys, app| { … });         // another device changed something

// SQL with sqlx (`prism` feature `db`)
let db = prism::db::Db::open(cx, "app.db", &["CREATE TABLE notes (id INTEGER PRIMARY KEY, text TEXT)"])?;
cx.set_global(db);
cx.spawn(async move |this, cx| {
    let notes: Vec<(i64, String)> = Db::of(&cx)
        .run(&cx, |pool| async move { sqlx::query_as("SELECT id, text FROM notes").fetch_all(&pool).await })
        .await?;
});
```

| | `storage` | `cloud_storage` | `prism::db` (SQLite) |
|---|---|---|---|
| iOS | App Group container | iCloud KVS (`[ios] icloud = true`; signed builds) | on the device |
| macOS | Application Support | iCloud KVS (needs a Mac provisioning profile) | on the device |
| Web | server, per browser | server, per browser | **on the server, shared by every user**: the app runs there, so this is your backend |
| Android (planned) | app files | Block Store / Drive app data | on the device |

The database runs queries on a small Tokio runtime and hands results back to the UI thread. Migrations are SQL statements applied once each, in order (`PRAGMA user_version`). On the web the same sqlx code can point at Postgres instead of SQLite. `cx.capabilities().cloud_sync` says whether `cloud_storage` really syncs (iCloud signed in); elsewhere it is local storage.

## Device APIs

```rust
cx.spawn(async move |this, cx| {
    if cx.request_permission(Permission::Location).await.is_granted() {
        let here = cx.current_location().await?;            // Location { latitude, longitude, accuracy, … }
    }
    let photos = cx.pick_images(PickImages::new().limit(3)).await?;  // picture(photo.path) shows them
    let photo = cx.take_photo().await?;
    let text = cx.read_clipboard().await;
});
let gyro = cx.watch_motion(Duration::from_millis(50), |m, app| { /* m.rotation_rate, m.attitude */ });
let follow = cx.watch_location(Accuracy::Best, |l, app| { … });
cx.stop_watching(gyro);
cx.haptic(Haptic::Success);   cx.copy_text("Hi");   cx.share(Share::text("Look").url(url));   cx.open_url(url);
```

| | iOS | macOS | Web |
|---|---|---|---|
| Permissions | system prompts (location, camera, photos, microphone, motion, notifications) | same (bundled app) | Permissions API, `getUserMedia` |
| Location | CoreLocation | CoreLocation (bundled app) | `navigator.geolocation` |
| Motion | CoreMotion (device only) | — | `devicemotion` |
| Photos / camera | PHPicker (no permission needed) / camera | open panel / — | `<input type=file>`, uploaded to the session |
| Haptics, clipboard, share, URLs | UIFeedbackGenerator, UIPasteboard, share sheet | trackpad, NSPasteboard, sharing picker | `vibrate`, Clipboard API, Web Share |

Usage texts go in `Prism.toml`; `prism` writes them to Info.plist (iOS refuses to ask without them):

```toml
[permissions]
location = "Shows todos near you."
camera = "Adds a photo to a todo."
```

`cx.capabilities()` says what exists (`location`, `motion`, `camera`, `photos`, `haptics`, `share`); unsupported calls fail with `DeviceError::Unavailable`.

## Background execution

```rust
cx.on_lifecycle(|phase, app| if phase == Lifecycle::Background { save(app) });

// Finishes even if the user leaves the app (iOS: beginBackgroundTask, ~30 s).
cx.spawn(async move |this, cx| upload(&cx).await).finish_in_background();

// Periodic work, even when the app is closed (the system picks the moment).
cx.register_background_task(
    BackgroundTask::refresh("sync").every(Duration::from_secs(15 * 60)),
    |cx| async move { sync(&cx).await },
);
```

| | iOS | macOS | Web |
|---|---|---|---|
| Lifecycle | active / inactive / background | active / inactive / hidden or minimized | page focus / visibility |
| `finish_in_background` | `beginBackgroundTask` | keeps running | keeps running (server) |
| Periodic tasks | `BGTaskScheduler`; the app is relaunched without UI if needed | `NSBackgroundActivityScheduler` | timer while a page is open |

`prism build/run` declares the tasks in Info.plist (`BGTaskSchedulerPermittedIdentifiers`, `UIBackgroundModes`). The iOS Simulator cannot run `BGTaskScheduler`: use a device, or `PRISM_RUN_BACKGROUND=sync` to run a task at launch and `PRISM_BACKGROUND_INTERVAL=5` to shorten intervals (macOS, web).

## Dialogs, menus, tray

```rust
cx.show_alert(Alert::new("Delete “Milk”?").message("This cannot be undone.")
    .destructive("Delete", on_delete).cancel("Cancel"));          // UIAlertController, NSAlert, <dialog>
cx.present("/todos/new");                                          // sheet: page sheet, NSWindow sheet, <dialog>
div().context_menu(Menu::new().item(MenuItem::new("Open").icon("arrow.up.forward.square")))
cx.set_tray(Tray::new("focus").title("25:00").menu(menu));         // macOS menu bar (NSStatusItem)
cx.set_app_menu(vec![Menu::titled("File").item(MenuItem::new("New").shortcut("cmd+n"))]);
```

Context menus open with a long press (iOS), right click or ⌃-click (macOS), right click / Shift+F10 (web), and from VoiceOver.

## Images, long lists, gestures

```rust
picture("https://…/photo.jpg").alt("A river").fill()   // cached on disk; or picture("logo.png") from assets/
VirtualList::new(10_000, 56., |i| row(i))               // only the visible rows exist
scroll().on_refresh(handler).refreshing(self.loading)   // pull to refresh (UIRefreshControl)
div().on_swipe(SwipeDirection::Left, handler)           // swipe gestures
```

## Languages, text size, motion

One JSON file per language in `locales/` (`en.json`, `fr.json`, `ar.json`…), flat or nested, plurals as `{"one": …, "other": …}`. `build.rs` turns them into one function per key:

```rust
// build.rs
prism_build::translations("locales").fallback("en").generate();   // .strict() to fail instead of warn

// src/main.rs
mod strings { prism::include_strings!(); }
cx.set_translations(strings::translations());
text(strings::home_title(cx, &name));            // {name} → argument; a typo is a compile error
text(strings::todos(cx, n));                     // CLDR plurals: one/few/many/other…
text(cx.format_date(now, DateStyle::Long));      // Foundation formatters on Apple, Accept-Language on the web
cx.set_locale(Some(Locale::new("ar")));          // or follow the system
div().w(relative(fill)).animate(250)             // UIView/NSAnimationContext/CSS transitions
```

- **Build-time checks**: a key missing in a language, an unknown key, a placeholder the reference text lacks, or a plural without `other` is a cargo warning (`warning: counter: fr.json: missing "todos.add"`).
- **Native packaging**: `prism build/run` declares the languages to iOS/macOS (`CFBundleLocalizations` + `.lproj`), so iOS Settings offers a per-app language.
- `cx.t("key")` / `cx.t_with` / `cx.t_count` remain for dynamic keys.
- **Right-to-left** languages mirror layouts on every backend (native frames and chrome, `dir=rtl` + logical CSS).
- **Dynamic Type**: native text follows the iOS text size live; the web uses `rem`.
- **Reduce Motion** turns transitions off. **iOS keyboard**: screens shrink above it and the focused field stays visible.

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

## Widgets, Live Activities, Dynamic Island, notifications

System surfaces are described with the **same elements as the UI**, plus `image("sf.symbol")`, `progress(0.4)`, `timer(until)` (counts down by itself, even when the app is not running) and `div().link("/path")` (deep link everywhere):

```rust
// A home-screen widget, re-rendered whenever `store` changes.
cx.register_widget(
    Widget::new("todos", "Todos")
        .families(&[WidgetFamily::Small, WidgetFamily::Medium])
        .link("/")
        .render(move |cx, family| text(format!("{} open", store.read(cx).open()))),
);
cx.reload_widgets_on(&store);

// A Live Activity: lock screen + Dynamic Island (compact, minimal, expanded).
let id = cx.start_activity(
    Activity::new("focus")
        .summary("Focus session, 25 minutes")
        .compact(image("brain.head.profile"), timer(ends))
        .minimal(image("timer"))
        .lock_screen(div().flex_row().child(text("Focus")).child(timer(ends)))
        .link("/focus"),
);
cx.update_activity(id, …);  cx.end_activity(id, None, true);

// A local notification that opens a screen when tapped, with action buttons.
cx.send_notification(
    Notification::new("Done").body("Take a break").link("/focus").after(total)
        .action(NotificationAction::new("again", "5 more minutes").link("/focus"))
        .action(NotificationAction::new("note", "Add a todo…").reply("What's next?")),
);
cx.on_notification_response(|r, app| match r.action.as_deref() { Some("note") => add(app, r.text.clone()), _ => {} });
```

| | iOS | macOS | Web | Android (planned) |
|---|---|---|---|---|
| Widgets | WidgetKit | WidgetKit (`mise run macos-app`) | — | Glance |
| Live Activities | lock screen + **Dynamic Island** | menu bar item with live countdown | in-page live banner (`aria-live`) | Live Updates notification |
| Notifications | UserNotifications | UserNotifications (bundled app) | Notification API / toast | NotificationCompat |

How it works: Prism resolves the elements into a static snapshot (compact JSON, under ActivityKit's 4 KB limit). A **generic SwiftUI renderer** shipped with Prism (`platform/apple/Extension`) draws it in a WidgetKit extension. `scripts/ios-run.sh` generates that extension from the app's registered widgets, then builds and signs it with the App Group. The Rust app talks to WidgetKit/ActivityKit/UserNotifications through a small Swift bridge linked automatically (`crates/prism-apple-build`). Platforms implement `PlatformServices`; apps query `cx.capabilities()` instead of checking the OS.

**Light & dark**: colors can be adaptive, `adaptive(rgb(0xFFFFFF), rgb(0x1C1C22))`, resolved natively everywhere: CSS `light-dark()`, dynamic `UIColor`/`NSColor`, SwiftUI color scheme in widgets and Live Activities. `Theme::system()` makes the whole component library follow the system appearance live.

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

## Performance

Measured on the example app (release build, Apple silicon):

| | Idle CPU | Memory |
|---|---|---|
| macOS, 460×720 / 1200×800 window | ~0 % | 42 MB / 49 MB |
| iOS (simulator) | ~0.2 % | 30–35 MB |
| Web server, per open session | ~0 % (sleeps until a message or timer) | ~0.3 MB |

- **Updates are local**: changing one row of a 5,000-row list re-renders that row only (1 patch, ~20 µs) and touches no other native view (`cargo test -p prism-native --release -- --ignored --nocapture bench`).
- **GPU-composited views**: backgrounds, borders, corners and pictures are layer properties, not per-view bitmaps (macOS graphics memory 28 MB → 0.4 MB in a large window).
- **Web**: one database pool per file for the whole server (not per visitor); sessions sleep until a message, a timer or finished background work.
- **Release profile**: thin LTO and stripped symbols (binaries −24 %).

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
mise run web        # the example, restarting on save (also: macos, ios, macos-app, device)
```

- `prism dev` keeps the current screen across restarts (`PRISM_DEV_STATE`); the web client reconnects by itself.
- `PRISM_PORT=8080`, `PRISM_SIM="iPhone 17"` (simulators are slimmed with `simslim` when installed), `PRISM_LOCALE=ar`, `PRISM_TEXT_SCALE=1.5`.
- macOS dev hooks: `PRISM_CLICK="+1|Dark mode" PRISM_SCROLL=600 PRISM_DELAY=2 PRISM_SNAPSHOT=out.png cargo run -p counter` presses controls by title or accessibility label, scrolls, waits, writes a PNG of the window, and quits.
- CI (`.github/workflows/ci.yml`): format, tests, clippy for macOS / iOS simulator / iOS device / web, then release bundles and a web smoke test.

## Status

Primitives: `div`, `text`, `button`, `text_input`, `toggle`, `scroll`, `picture`, `VirtualList`, `screen`, `Router` (stacks, tabs, sheets). Component library `prism::ui`. Accessibility on all backends. Light/dark appearance.
Widgets, Live Activities (Dynamic Island), notifications with actions, background tasks, device APIs (location, motion, photos, camera, haptics, clipboard, share), alerts, context menus, tray and app menus, async tasks, HTTP, storage, i18n/RTL, Dynamic Type, animations, `prism` CLI.
Not yet: Android, Windows, Linux backends (the `Widgets` trait and `PlatformServices` are the extension points).
