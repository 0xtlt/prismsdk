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
- Router query strings: paths keep `?period=7d&room=3` (URL, deep links,
  `prism dev` restarts); `Params::query`, `query_all`, `parse_query`,
  `query_pairs`; `cx.replace_query(..)` rewrites the visible screen's query
  without pushing one (web: `history.replaceState`); `encode_query` /
  `parse_query` helpers; `cx.current_query()`. (#7)
- Navigation state: `cx.navigate_with(path, value)` and
  `cx.replace_with(path, value)` hand a typed value to the next screen,
  read with `Params::state::<T>()`. (#7)
- Hiding the stack's header bar: `Router::new().header(false)` and
  `screen(..).bare()`, on every backend (UIKit navigation bar, AppKit and GTK
  header views, web `.prism-header`, Android/Wear OS header, watchOS
  toolbar); `Stack::header`. (#8)
- `scroll().fit()`: a scroll view as tall as its content up to its `max_h`,
  then scrolling, on every backend. (#16)
- `App::update_global_quiet` and `App::global_mut`: change a global without
  re-rendering. (#23)
- `TaskHandle::cancel_on_release` and `cancel_with(&entity)`: a task stops
  when its entity is released, skipping background work not started yet.
  (#23)

### Changed

- The Swift sources moved from `platform/apple` to
  `crates/prism-apple-build/apple`, the Android host and the bundling scripts
  from `platform/android` and `scripts/` to `crates/prism-cli/platform`, so
  the published crates contain them.
- `AsyncApp::background` runs on a bounded process-wide pool (16 threads by
  default; `set_background_threads`, `PRISM_BACKGROUND_THREADS`) instead of
  one OS thread per call; a panicking job no longer leaves its work counted
  as in flight. (#23)
- Web: the root stack and its visible screen fill the window
  (`100dvh`), so `scroll().flex_1()` scrolls inside a fixed frame; a screen
  taller than the window scrolls by itself instead of the page. (#8)
- `normalize_path` keeps the query string (`/a/?x=1#y` → `/a?x=1`), and
  `App::current_path` includes it. (#7)
- `Shape::inset(x, y)` is now `Shape::stroke_inset(x, y)`, so
  `shape().absolute().inset(0.)` reaches `Styled::inset`. (#15)
- A stack's value is a set of flags (`STACK_CLOSABLE`, `STACK_NO_HEADER`),
  and a screen's value says whether it is bare. (#8)

### Fixed

- Tasks spawned by a screen built during navigation (a first load in its
  constructor) start at once instead of at the next event. (#23)
