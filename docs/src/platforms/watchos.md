# watchOS

`prism run watchos` (simulator) / `prism build watchos`: a watch-only app (`WKApplication`, `WKWatchOnly`).

watchOS has no UIKit views (`UIView` is `API_UNAVAILABLE(watchos)` in the SDK), so the app runs in Rust and a SwiftUI host draws it (`crates/prism-apple-build/apple/Watch`).

- Layout is prism-native's flexbox, as on iOS and macOS. Swift measures text (UIKit string drawing) and places each node at its frame, so layouts match the other platforms.
- Navigation stacks are `NavigationStack`, tabs are vertical pages (Digital Crown), toggles are SwiftUI toggles.
- Styles are zoomed like tvOS (body text size ÷ 17); `cx.device_info()` reports the watch (model and screen from Xcode's profiles).
- Light and dark: SwiftUI draws text and controls in the app's appearance (`cx.set_appearance`, or a theme's), as on the other platforms; without one it follows the system, which is dark on a watch. A screen's background fills the whole display, under the title and the clock and at both ends of a scroll (the clock itself is the system's, white).
- Device APIs: location (Core Location), motion (Core Motion), the photo picker (PhotosUI's `PhotosPicker`), haptics (`WKInterfaceDevice.play`), sharing (`ShareLink`), links and the microphone permission.
- Also on the watch: notifications with actions, alerts, and context menus (press and hold opens a dialog, since SwiftUI's `contextMenu` is deprecated on watchOS). It also handles swipes, scroll events, pull to refresh, the scene lifecycle, Dynamic Type and iCloud key-value storage.
- Background tasks use WatchKit's `scheduleBackgroundRefresh`, because `BGTaskScheduler` is `API_UNAVAILABLE(watchos)`.
- What the watch lacks (SDK): camera, clipboard, [web views](../guide/web-views.md) (an empty box). `capabilities` says so.
- The minimum version is watchOS 10 (`WATCHOS_DEPLOYMENT_TARGET`, or `[watchos] deployment_target` in `Prism.toml`). Devices and store builds use Rust's `aarch64-apple-watchos` (arm64 watches, watchOS 26+), so their minimum version is 26.0. Older `arm64_32` watches would need Rust's tier-3 `arm64_32-apple-watchos` target, which Prism does not build.
- Dev hooks: `PRISM_CLICK`, `PRISM_SNAPSHOT` (SwiftUI `ImageRenderer`: text fields render as a placeholder), `PRISM_DEBUG_LAYOUT=1` (logs text frames).
