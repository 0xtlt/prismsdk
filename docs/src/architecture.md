# Architecture

```
            your app (Render entities)
                      │ render()
                      ▼
 prism-core   Element tree ──► Reconciler ──► Vec<Patch> ◄── UiEvent (click, input)
                                                 │
        ┌────────────────────────┬───────────────┴──────────────┐
        ▼                        ▼                              ▼
   prism-web                prism-native (taffy flexbox, Widgets trait)
   SSR HTML + JSON              ├── prism-macos    (AppKit)
   patches over WS              ├── prism-ios      (UIKit: iOS, tvOS, visionOS)
                                ├── prism-watchos  (SwiftUI host)
                                ├── prism-linux    (GTK 4)
                                └── prism-android  (Java host, Android views)
```

- **`prism-core`**: styles, element builders, entities/`Context`, reconciler, `Backend` trait, `Runtime`, the router, and the `PlatformServices` that backends implement (device APIs, storage, widgets, notifications). No platform code.
- **`prism-native`**: turns patches into calls on the `Widgets` trait (create, style, measure and place native views) and runs flexbox layout with taffy. Shared by every native backend.
- **`prism-web`**: an Axum router (or any HTTP and WebSocket stack, without the `server` feature). One `Runtime` per browser session (LiveView model).
- **`prism-macos`** / **`prism-ios`**: `Widgets` implementations over objc2. **`prism-watchos`** and **`prism-android`** keep the tree in Rust and send it to a host (SwiftUI, Java) that draws it at the computed frames. **`prism-linux`**: GTK 4 widgets placed by a small layout manager.
- **`prism-ui`**: component library on top of the core (themes, overrides, native escape hatch).
- **`prism`**: facade; picks the backend from features (`web`, `native`) and the target OS.
- **`prism-push`**: for your server: sends pushes to APNs, FCM and Web Push / UnifiedPush (see [Remote push](#remote-push-notifications)).
- **`prism-http`**, **`prism-db`**: HTTP and SQL, resolved on the UI thread.
- **`prism-build`**: typed translations from `locales/*.json`, in `build.rs`, and `link_app()`: the link settings of the app's own binary (Apple: Swift's rpath, deployment target), which the backends' build scripts cannot give it.
- **`prism-apple-build`**: compiles the Swift sources (`apple/`) in the Apple backends' build scripts.
- **`prism-cli`**: the `prism` command, with the bundling scripts and the Android host (`platform/`).

Layout is a flexbox subset (column by default, like React Native) and maps 1:1 to CSS on the web and to taffy on native, so all targets lay out the same way.

## Platform sources

The parts written in the platforms' own languages ship inside the crates that
use them, so the published crates are complete:

| Path | | Used by |
|---|---|---|
| `crates/prism-apple-build/apple/Bridge`, `Shared` | WidgetKit, ActivityKit, UserNotifications, CarPlay and device APIs | every Apple backend (linked by its build script) |
| `crates/prism-apple-build/apple/Watch` | the SwiftUI host | `prism-watchos` |
| `crates/prism-apple-build/apple/Extension` | the generic SwiftUI renderer of widgets and Live Activities | `prism` (bundled into the widget extension) |
| `crates/prism-cli/platform/scripts` | bundling, signing and installing (`ios-run.sh`, `macos-app.sh`, `android-run.sh`) | `prism` |
| `crates/prism-cli/platform/android` | the Java host (`PrismActivity`, `PrismHost`…) and the car service | `prism` (compiled into the APK) |

## Adding a platform

1. New crate `prism-<os>` implementing `prism_native::Widgets` (create/style/measure/place native views).
2. An entry point that builds a `Runtime`, feeds `runtime.render()` / `runtime.dispatch(event)` into a `NativeRenderer`, and calls `layout()` on resize.
3. One `cfg` line in `crates/prism/src/lib.rs`.
4. The smoke hooks (`PRISM_PATH`, `PRISM_CLICK`, `PRISM_SNAPSHOT`, `PRISM_DELAY`) and a script that runs the example on each tab with them, like `scripts/linux-smoke.sh` or `scripts/windows-smoke.sh` (a remote machine over SSH, see CONTRIBUTING.md › Windows).

`crates/prism-windows` is the newest example of a backend that draws its own views: a retained scene (`scene.rs`, no platform types, tested everywhere) that `Widgets` builds and Direct2D draws, with child windows only for text fields.
