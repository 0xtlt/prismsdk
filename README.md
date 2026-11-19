# Prism

Write a UI once in Rust (GPUI-style API), ship it as:

| Target | How it renders | Command |
|---|---|---|
| **Web** | Server-side Rust → HTML/CSS + a small JS client receiving DOM patches over a WebSocket. **No WASM.** | `prism dev web` → http://localhost:3000 |
| **macOS** | Real AppKit views (`NSView`, `NSTextField`, `NSButton`), menu bar, tray | `prism dev macos` |
| **iOS** | Real UIKit views (`UIView`, `UILabel`, `UIButton`, `UITextField`), widgets + Dynamic Island | `prism dev ios` (simulator) / `prism run ios --device "My iPhone"` |
| **Android** | Real Android views (`TextView`, `Button`, `EditText`, `Switch`), home-screen widgets, ongoing notifications | `prism dev android` (emulator or phone) |

Also tvOS, watchOS, visionOS, Linux (GTK 4), Wear OS, Android TV and car screens.

**Documentation: the [Prism book](https://0xtlt.github.io/prismsdk/)** (sources in [`docs/`](docs/src/SUMMARY.md)), and the API reference on docs.rs. Each section below is a summary of a chapter.

## Quick start

```sh
cargo install prismsdk-cli   # or, in this repository: mise run cli
prism new my-app && cd my-app
prism dev web                # rebuilds and restarts on every save, on the same screen
prism dev ios                # same, in the iOS simulator
prism run ios --device "My iPhone"
prism build macos            # release .app with widgets and icon
prism publish ios --dry-run  # App Store build, checked (see Shipping to stores)
prism dev android            # the running emulator or a USB phone
prism devices                # simulators, paired iPhones, Android devices
```

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

`Prism.toml` configures the app (name, bundle id, icon, Apple team). → [Getting started](docs/src/getting-started.md), [the `prism` command](docs/src/cli.md)

## Updating state

State lives in entities, GPUI-style; listeners re-render their entity, and only notified entities re-render. → [Updating state](docs/src/guide/state.md)

## Async, network, storage

`cx.spawn` for async work on the UI thread, `cx.background` for blocking work, `prism::http` and `cx.storage()`. → [Async, network, storage](docs/src/guide/async.md)

## Responsive

Tailwind-style breakpoints (`.md(|s| …)`, `hidden_from`, `cx.breakpoint()`), resolved in the core for every backend. → [Responsive](docs/src/guide/responsive.md)

## Platform-specific UI

`.only(Platform::MOBILE)`, `.except(…)`, `.on(Platform::Web, …)` and `cx.platform()`. → [Platform-specific UI](docs/src/guide/platform-ui.md)

## Device information

`cx.device_info()`: kind, OS version, model, screen, orientation, foldable posture, window mode, browser. → [Device information](docs/src/guide/device-info.md)

## Data: local, synced, SQL

`cx.storage()` (this device), `cx.cloud_storage()` (iCloud key-value store, Android backup) and SQL with sqlx (`prism::db`). → [Data](docs/src/guide/data.md)

## Device APIs

Permissions, location, motion, photos and camera, haptics, clipboard, share sheet, URLs. → [Device APIs](docs/src/guide/device-apis.md)

## Security: secrets, biometrics, sign-in

Secrets in the Keychain, Android Keystore or Secret Service; Face ID, Touch ID, fingerprint and WebAuthn checks; Sign in with Apple and OAuth/OpenID Connect sign-in with PKCE. → [Security](docs/src/guide/security.md)

## Background execution

Lifecycle events, `finish_in_background`, periodic tasks even when the app is closed. → [Background execution](docs/src/guide/background.md)

## Dialogs, menus, tray

Alerts, sheets, context menus, the macOS menu bar and tray. → [Dialogs, menus, tray](docs/src/guide/dialogs-menus.md)

## Images, long lists, gestures

`picture` (cached), `VirtualList`, pull to refresh, swipes. → [Images, long lists, gestures](docs/src/guide/images-lists-gestures.md)

## Languages, text size, motion

`locales/*.json` turned into typed functions at build time, CLDR plurals, right-to-left layouts, Dynamic Type, Reduce Motion. → [Languages, text size, motion](docs/src/guide/i18n.md)

## Motion

Transitions, transforms, enter/exit animations, loops and `with_animation`, native on every backend. → [Motion](docs/src/guide/motion.md)

## Charts and vector drawing

Line, bar and pie charts, rings, stats and sparklines in `prism::ui`, and `shape()` for your own drawings; also in widgets, Live Activities and car screens. → [Charts and vector drawing](docs/src/guide/charts.md)

## 3D scenes

`scene3d()`: meshes, glTF models with skeletal animations, instancing, physically based materials, shadows and environment lighting, drawn by the GPU (SceneKit, WebGL 2, OpenGL ES 3, OpenGL); orbit, spin, clips, transitions and taps run on the device, and only changed nodes are sent. → [3D scenes](docs/src/guide/scene3d.md)

## Accessibility

Built in: one description per node, mapped to ARIA, `UIAccessibility`, `NSAccessibility`, GTK and Android. → [Accessibility](docs/src/guide/accessibility.md)

## Components (`prism::ui`)

A themed, accessible component library with three levels of customization and a native escape hatch. → [Components](docs/src/guide/components.md)

## Forms and feedback

Native sliders, pickers, date and time pickers, password fields, text areas, spinners and toasts, plus absolute positioning for overlays. → [Forms and feedback](docs/src/guide/forms.md)

## Widgets, Live Activities, Dynamic Island, notifications

System surfaces described with the same elements as the UI: widgets, Live Activities, notifications with actions, light and dark. → [Widgets, Live Activities, notifications](docs/src/guide/system-surfaces.md)

## Remote push notifications

APNs, FCM, Web Push and UnifiedPush tokens and messages in the app, and `prism-push` to send them from your server. → [Remote push notifications](docs/src/guide/push.md)

## In the car (CarPlay, Android Auto, Android Automotive)

Car screens from the platforms' templates (lists, grids, panes, tabs), rebuilt when the app changes. → [In the car](docs/src/guide/car.md)

## Navigation

A URL-style router with native stacks, tabs, sheets and deep links. → [Navigation](docs/src/guide/navigation.md)

## Web: inside your server, behind a CDN

The web backend is a library: mount it in Axum (or any server), export hashed static files for a CDN, or ship the Docker image. → [Web](docs/src/platforms/web.md)

### Docker (Linux)

`docker/web.Dockerfile` builds the server, its static files and assets. → [Web: Docker](docs/src/platforms/web.md#docker-linux)

## Performance

~0 % idle CPU, local updates (one patch for one changed row), GPU-composited views. → [Performance](docs/src/performance.md)

## Architecture

`prism-core` (elements, reconciler) → patches → `prism-web` or `prism-native` and the native backends. → [Architecture](docs/src/architecture.md)

### Adding a platform (Windows…)

A crate implementing `prism_native::Widgets`, an entry point and one `cfg` line. → [Adding a platform](docs/src/architecture.md#adding-a-platform-windows)

## Development

```sh
mise install        # Rust 1.99 + platform targets
mise run test       # unit tests
mise run check      # type-check every target
mise run web        # the example, restarting on save (also: macos, ios, android, macos-app, device)
mise run docs       # the book, into docs/book
```

→ [Development](docs/src/development.md), [CONTRIBUTING.md](CONTRIBUTING.md), [RELEASING.md](RELEASING.md), [CHANGELOG.md](CHANGELOG.md)

## Shipping to stores

`prism build android --aab` and `prism publish android` (Google Play), `--store` builds and `prism publish` for the App Store, notarized DMGs, `.deb` packages and Docker images, each with a `--dry-run`. → [Shipping](docs/src/shipping.md)

## tvOS

The UIKit backend adapted to the Siri Remote and the focus engine. → [tvOS](docs/src/platforms/tvos.md)

## watchOS

The app runs in Rust and a SwiftUI host draws it. → [watchOS](docs/src/platforms/watchos.md)

## visionOS

The UIKit backend in a window, like an iPad app. → [visionOS](docs/src/platforms/visionos.md)

## Linux (GTK 4)

Native GTK 4 widgets placed by prism-native's flexbox; notifications, tray, menu bar. → [Linux](docs/src/platforms/linux.md)

## Android

An APK without Gradle; a small Java host shows real Android views. Phones, tablets, Wear OS, Android TV. → [Android](docs/src/platforms/android.md)

## Status

Primitives: `div`, `text`, `button`, `text_input` (secure, keyboards, submit), `text_area`, `toggle`, `slider`, `picker`, `date_picker`, `spinner`, `scroll`, `picture`, `shape`, `scene3d`, `VirtualList`, `screen`, `Router` (stacks, tabs, sheets). Component library `prism::ui`. Accessibility on all backends. Light/dark appearance.
Widgets, Live Activities (Dynamic Island), car screens (CarPlay, Android Auto, Automotive), notifications with actions, remote push (APNs, FCM, Web Push, UnifiedPush) with a server-side sender, background tasks, device APIs (location, motion, photos, camera, haptics, clipboard, share), secure storage, biometrics, Sign in with Apple, OAuth/OIDC sign-in, alerts, context menus, tray and app menus, charts (also in widgets), async tasks, HTTP, storage, i18n/RTL, Dynamic Type, animations, `prism` CLI.
Platforms: web, macOS, iOS/iPadOS, tvOS, watchOS, visionOS, Linux (GTK 4), Android (phones, tablets, Wear OS, Android TV). Not yet: Windows (the `Widgets` trait and `PlatformServices` are the extension points).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option. Contributions are dual licensed the same way ([CONTRIBUTING.md](CONTRIBUTING.md)).
