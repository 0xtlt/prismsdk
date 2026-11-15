# Changelog

All notable changes to Prism. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (before 1.0, a breaking change
bumps the minor version). Every crate of the workspace shares one version.

## Unreleased (0.1.0)

The first version, not yet on crates.io.

### Added

- Elements, styles, entities and the reconciler (`prism-core`); flexbox layout
  shared by the native backends (`prism-native`).
- Backends: web (server-rendered, DOM patches over a WebSocket, no WASM),
  macOS (AppKit), iOS, iPadOS, tvOS and visionOS (UIKit), watchOS (SwiftUI
  host), Linux (GTK 4), Android (phones, tablets, Wear OS, Android TV).
- Navigation (stacks, tabs, sheets), the `prism::ui` component library,
  charts and vector shapes, motion, accessibility on every backend,
  responsive breakpoints, translations checked at build time, right-to-left
  layouts, Dynamic Type.
- Widgets, Live Activities and the Dynamic Island, notifications with actions,
  car screens (CarPlay, Android Auto, Android Automotive), background tasks,
  device APIs, storage, iCloud key-value storage, SQL (`prism-db`), HTTP
  (`prism-http`).
- Form controls on every backend: `slider`, `picker`, `date_picker`,
  `text_area`, `spinner`, password fields, keyboard types and submit; toasts
  (`ui::toaster`) and absolute positioning for overlays.
- Security: secure storage (Keychain, Android Keystore, Secret Service,
  encrypted server store on the web), biometric and device-owner checks
  (LocalAuthentication, BiometricPrompt, fprintd, WebAuthn), Sign in with
  Apple, OAuth 2 / OpenID Connect sign-in with PKCE; Android cloud backup.
- Remote push: APNs, FCM (without Gradle), Web Push and UnifiedPush in the
  app, Live Activity push tokens, and `prism-push` to send from a server.
- Charts in widgets, Live Activities, the Dynamic Island and car screens.
- Per-corner rounding (`rounded_top`, `rounded_bottom`, `rounded_corners` with
  `Corners`, leading/trailing aware) on every backend; bar charts round only
  the top of their bars by default (`bar_radius`, `bar_corners`).
- Shipping: Android App Bundles and Google Play upload, App Store builds and
  upload, Developer ID notarization and DMGs, `.deb` packages, web Docker
  images, all with `--dry-run`.
- The `prism` command: `new`, `run`, `dev` (restart on save, keeping the
  screen), `build`, `publish`, `devices`. `prism new` depends on the crates.io release
  when it finds no Prism checkout.
- Packaging for crates.io: metadata, licenses (MIT OR Apache-2.0) and a README
  for every crate, versions on path dependencies, docs.rs configuration.
- The documentation site (`docs/`, mdBook), deployed with the API docs to
  GitHub Pages, and API documentation for every public item of `prism`,
  `prism-core` and `prism-ui`.
- Pointer and keyboard input on every backend (#17): `ClickEvent::position`
  (where a click or tap landed), `Div::on_hover`, `on_pointer_move`,
  `on_drag` (start / move / end / cancel with the translation), and
  `on_key_down` / `on_global_key_down` with web key names and modifiers,
  TV remote presses and Android D-pads included; `cx.stop_propagation()`.
  Web moves and drags are throttled to one message per animation frame.
- `.id(…)` on every element, `cx.focus(id)`, `.autofocus()` and
  `cx.scroll_into_view(id)` (#17).
- `checkbox` and `radio_group`, native where the platform has them and drawn
  with the right accessibility traits on UIKit; `time_picker` for a time of
  day independent of the time zone (`ChangeEvent::minutes`); `text_input`
  `max_length`, `leading_icon` and `suggestions` (web `<datalist>`, native
  completion); `cx.pick_files(PickFiles)`, `PickedFile::read` and
  `ui::file_input`, whose bytes reach the app on every backend (the web
  uploads them over the socket) (#18).
- `sheet`, `popover` and `dialog` overlays a screen opens without a route,
  presented natively (`UISheetPresentationController`, `NSPopover`, `<dialog>`
  and the popover API, `GtkPopover`, Android dialogs…), above the whole
  window, with the focus kept inside and Escape / Back closing them; the
  `hit_test(false)` (`pointer-events: none`) and `inert(true)` styles (#19).
- Runtime layers (`cx.add_layer`): entities mounted above the router, tabs,
  sheets and dialogs on every backend (#25).

### Changed

- The toaster is a layer the runtime mounts above the whole app the first
  time `ui::show_toast` runs: toasts work from anywhere, also right before
  `cx.back()` / `cx.reset()`. `ui::toaster()` is deprecated and renders
  nothing (#25).
- `ClickEvent` carries an optional position; `ClickEvent` still works as a
  value (`handler(&ClickEvent, cx)`).
- The Swift sources moved from `platform/apple` to
  `crates/prism-apple-build/apple`, the Android host and the bundling scripts
  from `platform/android` and `scripts/` to `crates/prism-cli/platform`, so
  the published crates contain them.

### Fixed

- A web `picker` given a height centres its text vertically and draws its
  own arrow (`appearance: none`) (#18).
