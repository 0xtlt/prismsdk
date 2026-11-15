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
- Web, embedding in a host server: additions to the page's `<head>`
  (`WebApp::favicon`, `stylesheet`, `script`, `meta`, `theme_color`,
  `preload_font`, `style`, `body_background`, `font_family`, `head`; or
  `Application::web(...)`); a per-request context from the host
  (`prism_web::RequestContext` as a request extension, `handle_with`,
  `connect_with`, `connect_request`) read with `cx.request_context::<T>()`,
  re-read at every reconnect; `cx.request()` (URL, origin, headers, cookies of
  the page request or socket handshake), `cx.page_origin()` and
  `cx.page_url()` (`None` natively); `cx.open_url_in(url, UrlTarget)`.
- Web hardening: `WebApp::allow_origins`, `max_sessions`, `idle_timeout`,
  `accept_socket`, `session_count`; a host-given CSP nonce
  (`prism_web::CspNonce`) on Prism's tags.
- Web client: reconnect back-off (0.5 s doubling to 30 s), a
  `data-prism-connection` state on `<html>` with a « Reconnecting… » banner and
  `prism:connection` events, and a page fetch after failed reconnects that
  reloads when the host answers `401`, `403` or a redirect.
- `crates/prism-web/examples/embed.rs`: Prism nested in an Axum host with its
  own sign-in and a strict CSP.

### Changed

- Web: an unknown path answers `404` with the not-found screen (it was `200`).
- Web: the first paint has no `style` attributes: node styles are rules of a
  `<style>` block (with the host's nonce), so `style-src` needs no
  `'unsafe-inline'`. `HtmlBackend::to_html_and_css` returns both.
- Web: `cx.open_url` stays in the tab for a URL of the page's own origin (a
  new tab otherwise).
- Web: socket handshakes from a foreign `Origin` are refused (`403`) unless
  allowed.

- The Swift sources moved from `platform/apple` to
  `crates/prism-apple-build/apple`, the Android host and the bundling scripts
  from `platform/android` and `scripts/` to `crates/prism-cli/platform`, so
  the published crates contain them.
