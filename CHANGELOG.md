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
- Layout: CSS grid (`grid()`, `grid_cols` with points, `fr`, `minmax`,
  `repeat`, `auto_fill`/`auto_fit`, `cols(n)`, `grid_rows`, `col_span`,
  `row_span`, `col_start`, `row_start`; equal-height cells), `flex_grow(weight)`
  and `flex_shrink(weight)`, `gap_x`/`gap_y`, `items_baseline`/`self_baseline`
  (native text measures report baselines), `aspect_ratio`, `.sticky()` and
  `.fixed()` (against the screen, never scrolling), and horizontal scroll
  views (`scroll_x()`, `scroll().horizontal()`) on every backend.
- Responsive: `Application::breakpoints([…])` / `App::set_breakpoints`,
  `.at_width(px, …)` for any window width (re-renders when crossed), and
  `.when_width_at_least(px, …)`, rules on an element's own laid-out width
  (container queries) resolved by the layout pass natively and by the client
  on the web.
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

### Changed

- `Styled::flex_grow()` takes a weight: `flex_grow(1.)`.
- The Settings tab of the example lays its cards out with `grid().cols(2)`
  instead of wrapped flex cells.

- The Swift sources moved from `platform/apple` to
  `crates/prism-apple-build/apple`, the Android host and the bundling scripts
  from `platform/android` and `scripts/` to `crates/prism-cli/platform`, so
  the published crates contain them.
