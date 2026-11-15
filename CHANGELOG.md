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
- Box decorations on every backend: shadows (several, offset, blur, spread,
  inset; `shadow_sm/md/lg`, `Shadow::ring`), per-side borders
  (`border_top`…), dashed and dotted borders, `clip()` to the rounded box,
  linear and radial gradients (`bg_gradient`), backdrop blur
  (`backdrop_blur`; no blur on GTK), and `Color::current()`, a color relative
  to the text color (`color-mix()` with `currentColor` on the web).
- Interaction states: `.hover`, `.active`, `.focus`, `.focus_within`,
  `.disabled` style refinements and group states (`.group()`,
  `.group_hover`, `.group_active`, `.group_focus_within`), resolved natively
  (CSS and GTK pseudo-classes, AppKit tracking areas, UIKit gestures and the
  tvOS focus engine, Android touch, hover and focus listeners, SwiftUI
  pressed state); `cursor(…)`, with a pointing hand on clickable nodes.
- `prism::ui` buttons react to hover and press; cards have a faint shadow.
- Motion: loops that start over (`repeat_from_start`; `Pose::rotate(360.)`
  is a full turn on every backend, for spinners), non-uniform scale
  (`scale_x`, `scale_y` on styles and poses) and offsets in percent of the
  element's own size (`translate_percent`, for anchoring labels).
- Pictures from memory: `picture_data(bytes, mime)` (a `data:` URL, decoded
  natively on every backend) and `svg(markup)` (SVG drawn as native vector
  paths everywhere, widgets included, keeping its proportions);
  `ui::qr_code(text)` with a built-in QR encoder (`prism_ui::qr`: byte
  mode, versions 1–40, four error-correction levels), drawn as one shape.
- Charts: reference lines with labels (`reference_line`), colored bands
  (`band`, `band_labeled`), tick control (`y_ticks`, `y_tick_count`,
  `format_y`, `x_label_every`), grouped bars (`bar_chart().labels(…)
  .series(…)`), min–max bands (`min_max`) and dashed series (`dashed`) on
  line charts, and a read-out of the selected point or bars (`selected`,
  and `on_select` on line charts: one tap target per point).
- Shapes: the even-odd fill rule (`Path::even_odd`) on every renderer;
  `aspect_ratio` on styles (taffy, CSS `aspect-ratio`, widget snapshots).
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
- Typography on every backend: font families (`font_mono`, `font_serif`,
  `font_rounded`, `font_family("Inter")`), any weight from 100 to 900
  (`weight(650)`, `FontWeight::Thin` … `Black`), `italic`, `line_height` /
  `leading`, `letter_spacing`, `uppercase` / `lowercase` / `text_transform`,
  `underline`, `strikethrough`, `tabular_nums`, `truncate`, `line_clamp(n)`
  and `nowrap`, measured natively by the shared layout.
- Bundled fonts: files in `assets/fonts/` are registered on every platform
  (Core Text, `Typeface.createFromAsset`, fontconfig) and become `@font-face`
  rules on the web.
- Rich text: `rich_text([span(…).bold(), link("settings", "/settings"),
  icon("star.fill"), …])`, one paragraph of styled runs with inline links
  (keyboard and screen reader reachable) and SF Symbols, mapped to
  `NSAttributedString`, `AttributedString`, `SpannableString`, Pango
  attributes and inline HTML.
- Clickable texts: `text(…).on_click(…)` and `text(…).link(path)`.
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

- Web: an unknown path answers `404` with the not-found screen (it was `200`).
- Web: the first paint has no `style` attributes: node styles are rules of a
  `<style>` block (with the host's nonce), so `style-src` needs no
  `'unsafe-inline'`. `HtmlBackend::to_html_and_css` returns both.
- Web: `cx.open_url` stays in the tab for a URL of the page's own origin (a
  new tab otherwise).
- Web: socket handshakes from a foreign `Origin` are refused (`403`) unless
  allowed.
- `Styled::flex_grow()` takes a weight: `flex_grow(1.)`.
- The Settings tab of the example lays its cards out with `grid().cols(2)`
  instead of wrapped flex cells.
- `FontWeight` has the whole CSS scale (`Thin`, `ExtraLight`, `Light`,
  `ExtraBold`, `Black`, `Custom(u16)`); matches on it need the new variants.
- Links (`Div::link`, `Text::link`, `link(…)` runs) to a URL with a scheme
  (`https://…`, `mailto:`) open it instead of navigating in the app.
- Android: rounded views no longer clip their children unless they ask for it
  with `clip()` (pictures still do), like the other backends; this also lets
  their shadows show.
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
- Line charts leave a gap where a value is missing (NaN) instead of joining
  its neighbours (`.connect_gaps()` bridges it); a lone point is a dot.
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

- A web `picker` given a height centres its text vertically and draws its
  own arrow (`appearance: none`) (#18).
- iOS, iPadOS, tvOS, visionOS and watchOS builds (simulators and devices)
  link clang's runtime for their target (`libclang_rt.<os>[sim].a`, found
  with `clang -print-resource-dir`): Swift availability checks need
  `__isPlatformVersionAtLeast`, which rustc's builtins lack there, so
  simulator builds no longer fail to link without a `RUSTFLAGS` workaround.
- The watch host clamps frames that are negative or not finite before
  handing them to SwiftUI (absolutely positioned overlays next to chart
  bars).
- Tasks spawned by a screen built during navigation (a first load in its
  constructor) start at once instead of at the next event. (#23)
