# Changelog

All notable changes to Prism. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (before 1.0, a breaking change
bumps the minor version). Every crate of the workspace shares one version.

## Unreleased (0.1.0)

The first version, not yet on crates.io.

### Added

- Popover side and alignment (#113): `popover(content).side(PopoverSide::Trailing)` opens a popover beside its anchor (`Bottom` by default, `Top`, `Trailing`, `Leading`; trailing is the right in left-to-right layouts), on the opposite side when the window has no room there, and `.align(PopoverAlign::End)` lines it up with the anchor's start (the default), center or end: a submenu beside its item with tops lined up, a panel lined up with the trailing edge of a toolbar button. Nested popovers and `light_dismiss(false)` / `take_focus(false)` keep working. Web: placed by the client; macOS: the `NSPopover`'s preferred edge and the part of the anchor it points at, the `take_focus(false)` panel placed exactly; iPadOS / visionOS: `permittedArrowDirections` and `sourceRect`; Linux: `GtkPopover::set_position` / `set_pointing_to`; Android: the `PopupWindow` placed beside or above its anchor. Arrowed popovers (macOS, iPadOS, Linux) line up exactly when they are no longer than the anchor along that edge, and are centered on it otherwise. `OverlayConfig::side` / `align` (`side=…`, `align=…` in the config string), `PopoverAlign::arrow_span` for backends.
- Cookies from a handler (#91): `cx.set_cookie(Cookie::new(name, value))` sets a cookie in the browser from a handler running over the live socket (signing in or out). `Cookie` defaults to `HttpOnly; Secure; SameSite=Lax; Path=/` (`.path`, `.domain`, `.max_age`, `.http_only`, `.secure`, `.same_site`, `Cookie::remove`), and `Cookie::check` refuses what a browser would drop. The server gives each cookie a one-time ticket (128 bits, 60 s, bound to the browser's `prism_sid`); the page posts it in an `X-Prism-Cookie` header to `POST <base>/_prism/cookie` (same origin only), whose `204` sets it, one cookie after the other. `AsyncApp::set_cookie(..).await` answers once the browser has it, and `cx.request()` shows it from then on; a `Secure` cookie on a plain `http://` page (not `localhost`) or a `Domain` that does not cover the page fails instead of being lost. `cx.reconnect()` opens a new live session at once, without the « Reconnecting… » banner, after the cookies set before it, so the host's middleware sees them. Native apps answer `set_cookie` with `Unavailable` and ignore `reconnect`. `DeviceCall::SetCookie` / `Reconnect`, `WebRequest::apply_cookie` for backends.
- Data in the web's first paint (#63): `WebApp::first_paint_wait(timeout)` lets the server-rendered page wait, up to `timeout`, for the `cx.background` work (HTTP, database, files) that `init`, a screen's constructor or a `render` started, and for the work the resumed tasks start next, so a data-driven screen ships its content instead of its loading state. Timers and channels are not waited for; past the deadline the page shows what is still loading, as before. The live session's first tree waits the same way, so the page does not flash back to its loading state when the socket connects. Zero (the default) changes nothing. `Runtime::render_settled(timeout)` for other hosts.
- `cx.page_request()` and `cx.page_request_context::<T>()` on the web (#92): the live session gets back the page request that drew the first paint, with the host's context of that request, so a value the host gives once (a flash message after a redirect, a one-time token) shows in the first paint and stays when the page goes live. The page carries a random id (`data-page`); its first socket sends it back and the session takes the request over once, within a minute, from the same browser (`prism_sid`). Reconnects and native apps get `None`.
- Reversed directions and `order` (#87): `.flex_row_reverse()` / `.flex_col_reverse()` (`FlexDirection::RowReverse` / `ColumnReverse`, `is_row`, `is_reverse`) and `.order(i)` (`Style::order`), usable in responsive and own-width refinements and interaction states. Web: CSS `flex-direction` and `order`; macOS, iOS / iPadOS / tvOS / visionOS, watchOS, Linux and Android: taffy's reversed directions, and the layout children sorted by `order` (stable) while the views keep the app's order, so focus and assistive technologies follow the tree as on the web. Widgets, Live Activities and notifications get their children already sorted and reversed.
- OAuth token revocation (#77): `OAuth::revoke(&cx, token, hint)` and `revoke_blocking(token, hint)` (RFC 7009) post the token, an optional `TokenTypeHint` (`RefreshToken`, `AccessToken`) and the client's credentials to the server's revocation endpoint, for signing out. The endpoint is kept from the metadata's `revocation_endpoint` (`OAuth::revocation_url`; read from the metadata when `resolve` left it unknown, preset for `OAuth::google`); `revocation_request` and `parse_revocation_response` are public. `scripts/mock-oidc.py` serves `/revoke`.
- Drawn radios (#84): `Role::Radio` (with `accessibility_checked`) on choices the app draws, such as option cards, and `Role::RadioGroup` on their container, are now reported as radios on every backend. Web: `aria-checked`, one tab stop per group (the checked radio, else the first) and the arrow keys move the focus and click the radio they land on; iOS: the group is a semantic group; watchOS: the selected trait; Android: `RadioButton` / `RadioGroup` with the checked state (also `CheckBox` and `Switch` for drawn checkboxes and switches); Linux: the GTK radio, radio group, checkbox and switch roles with the checked state. The native backends' `a11y` op now carries `checked`.
- Text field selection and caret (#86): `text_input()` / `text_area()` take `.on_select(handler)`, called with a `SelectionEvent { start, end }` in characters (`start == end` for a caret, `byte_range(&text)` to slice the text; `prism::char_to_byte` / `byte_to_char`) whenever the selection or caret moves, typing included (after its `on_input`), and `.selection(range)`, applied when the app changes it (echoes of the user's own selection leave the field alone). `KeyEvent::selection` carries the focused field's selection before the key, so `on_key_down` can split a block at the caret on Enter or merge on Backspace at the start. Web: `selectionchange` / `select` / keys / pointer, one report per animation frame; AppKit: `NSTextViewDidChangeSelectionNotification` (field editor and text views); UIKit: `textFieldDidChangeSelection:` / `textViewDidChangeSelection:`; GTK: `notify::cursor-position` / `selection-bound` and text buffer marks; Android: `onSelectionChanged`. Not on watchOS. `Patch::SetSelection` / `SetSelectEvents`, `UiEvent::Select`, `Widgets::set_selection` / `set_select_events` for backends.
- `cx.set_page_status(code)` (#60): a route that matches but whose data is missing (`/orders/:id` with an unknown id) answers the web's first paint with `404` (or `403`, `410`, `500`…) while it draws its own « not found » state, set from the screen's build or `render` without re-rendering. Only `2xx`, `4xx` and `5xx` are kept, the last call wins; `cx.page_status()` reads the status the page gets (the router's not-found screen still gives `404`). The live session and native apps ignore it.
- App assets compiled into the binary (#61): `EmbeddedAssets` (`include_bytes!`, or a few lines from rust-embed) given to `Application::embedded_assets` or `WebApp::embedded_assets`, for a server shipped as one file. Web: hashed and served like the assets folder's files (immutable, unhashed names revalidated), exported by `export_static`, `fonts/` files declared as `@font-face`. macOS and iOS: `picture()` reads them from memory and fonts are registered from memory (`CTFontManagerRegisterGraphicsFont`); Linux: pictures from memory, fonts written once to the user's cache for fontconfig. An embedded file wins over the folder's file of the same name. Android and watchOS read the bundle's assets folder only.
- `.attr(name, value)` on every element (#85): an attribute of the element on the web, set in the first paint and kept in sync live, for browser integrations and end-to-end tests (`data-1p-ignore`, `data-testid`, `autocomplete="one-time-code"`). Only `data-*` names Prism does not use itself and a safe list (`autocomplete`, `translate`) are taken (`prism_core::attributes`), so it cannot add handlers or styles; others are ignored and fail a debug assertion. An app's `autocomplete` wins over a secure field's default. Native backends ignore it. `Patch::SetAttributes` for backends.
- `.capitalize()` (#90): the first letter of each word in capitals, the shorthand for `text_transform(TextTransform::Capitalize)` next to `.uppercase()` and `.lowercase()` (CSS `text-transform: capitalize` on the web, the shared case mapping on the native backends). `TextTransform::apply_after` transforms a rich-text run after the one before it.
- Selected and current items for screen readers (#64): `.accessibility_current(true)` marks the destination a navigation shows (`aria-current="page"` on a link, `"true"` otherwise), `.accessibility_selected(bool)` the chosen item among several (`aria-selected`). Native platforms read both as selected: `UIAccessibilityTraitSelected` (also on native controls), `accessibilitySelected` on macOS, `.isSelected` on watchOS, `View.setSelected` on Android, `GTK_ACCESSIBLE_STATE_SELECTED` on Linux. `Accessibility::is_selected`; the remote `a11y` op carries `selected`.
- Optional date and time pickers (#89): `date_picker_opt(Option<SystemTime>)`, `time_picker_opt(Option<u32>)` and `ui::date_field_opt` may be empty, show `.placeholder(…)` while they are, and let the user clear a date; `ChangeEvent::date_opt()` / `minutes_opt()` read `None` for a cleared one. The empty value is `NaN` (`null` on the wire), accepted from hosts only for optional pickers; `DateConfig::optional` (`optional` in the config string) and `DateConfig::start` (what an empty picker starts from: now, to the minute, within the bounds); the built-in `clear` chrome text (`RenderEnvironment::clear`, `prism.clear`). Web: an empty input, the browser's own clear (non-optional inputs are now `required`); iOS / iPadOS / visionOS: a placeholder pill over the compact `UIDatePicker` and a clear button; macOS: the placeholder in the `NSDatePicker`'s bezel and a clear button; Linux: the placeholder on the button and « Clear » in the popover; Android: the placeholder and « Clear » in the system dialogs; watchOS: « Clear » under the wheels; tvOS: Play/Pause clears.
- Conic gradients (#80): `Gradient::conic(from, colors)` turns the colors around the center, clockwise from `from` degrees (`0` up), like CSS `conic-gradient(from …)`: hue wheels, pie-like progress. `.with_stops` places them (hard edges with two stops at one position). Web: `conic-gradient()`; macOS, iOS, tvOS, visionOS: a conic `CAGradientLayer`; watchOS: `AngularGradient`; Linux: GTK CSS `conic-gradient()`; Android: `SweepGradient`.
- Fonts loaded at run time (#81): `cx.load_font(FontFace::url(family, url))` or `FontFace::bytes(family, bytes)` (`.weight(…)`, `.weights(min, max)`, `.italic()`) registers a face for the session (web: `FontFace` in `document.fonts`, in the first paint's `@font-face` rules when given by URL) or the process (Apple platforms: `CTFontManagerRegisterGraphicsFont`; Android: `Typeface.Builder(File)`; Linux: fontconfig, then Pango's font map reloads). Native backends download URLs in the background, read the family the file declares and make the app's name an alias of it, then draw the texts again (`NativeRenderer::restyle_all`). `cx.add_stylesheet(url)` adds a stylesheet to the page (web) while the returned `Stylesheet` handle lives, for one screen's needs. `prism_core::fonts::font_file_info` reads a TrueType / OpenType file's family, weight and style.
- Custom window chrome on macOS (#67): `Application::window_chrome(WindowChrome::Custom { traffic_lights: Some(Point::new(16., 13.)) })` removes the title bar (full-size content view, transparent untitled title bar; the window title still names it in Mission Control, the Window menu and for VoiceOver) and keeps the traffic lights where the app puts them through resizes, full screen and appearance changes. `.window_drag()` on any element moves the window on press and follows the user's « double-click a window's title bar » setting (zoom, minimize, nothing), while buttons and clickable views inside keep their clicks. `cx.window_controls()` (the lights' frame, `None` in full screen) and `cx.is_fullscreen()` re-render on change so the app's title bar can make room for them. Accepted and ignored on the web and other backends; `Runtime::set_window_chrome` for hosts.
- Inline popovers (#70): `popover(menu).light_dismiss(false)` is not closed by a click or tap outside (only by the app, or Escape while the focus is inside), and `.take_focus(false)` leaves the keyboard focus with the anchor's text field, clicks on the popover's items included, for slash-command, mention and autocomplete menus. Web: `popover="manual"`, no focus move, presses inside keep the field focused; macOS: a borderless non-activating child panel instead of an `NSPopover`; iOS / iPadOS: the field stays first responder and taps around the popover reach the window; Linux: a `GtkPopover` without autohide; Android: a non-focusable `PopupWindow`. `OverlayConfig::manual` / `keep_focus` (`manual`, `keepfocus` in the config string).
- `prism::channel()` (#53): an unbounded channel whose `Sender` (`Send + Clone`) works from any thread and whose `Receiver::recv()` is awaited in a task. Sending wakes the UI thread on every backend, so a thread the app owns (a WebSocket client, a file watcher, an SDK callback) feeds the UI without a polling loop or a pool thread held per session. `try_recv`, `drain`, `Sender::is_closed`.
- `Picture::tint(color)` (#51): paints every path of an `svg()` picture in one color (adaptive or `Color::current()` included), for icons that hard-code their color.
- Widget buttons (#47): `div().widget_action(action, payload)` (also on `text` and `button`) in a widget or Live Activity runs the app's `cx.on_widget_action(|action, cx| async move { .. })` job without opening the app, then reloads the widgets. iOS/iPadOS 17+: `Button(intent:)` with a shared `LiveActivityIntent` that iOS runs in the app's process (launched in the background if needed; the bundler now writes the app's own App Intents metadata); actions the extension ran itself are queued in the App Group and run when the app becomes active. Android: a broadcast to `PrismWidget$Action`, run without an Activity. macOS degrades to opening the app on `/__prism/widget-action?…`, which runs the action (`WidgetAction::to_link`, handled by `Runtime::open_link`). `Runtime::run_widget_action` for backends, `PRISM_DESCRIBE=widget_actions` for bundlers. The counter's « Todo » widget toggles its todo in place.
- Lazy tabs: a router tab renders on its first selection, then stays rendered (its screens, state and views are kept). An unvisited tab keeps its tab-bar entry over an empty panel; deep links render their tab on the server, in the first paint. `RouterBuilder::eager()` renders the tab declared before it up front; `Router::tab_rendered(i)`. On the example app, `/` ships 44 nodes instead of 715 (HTML 150 KB → 16 KB, first WebSocket update 384 KB → 24 KB). The web keeps the shown panel up until the server's render of a new tab arrives (no empty flash); watchOS also renders the pages next to the selected one, which the Digital Crown shows mid-scroll.
- Configurable widgets (#46): `Widget::parameter(WidgetParameter::new(..).options(..).default(..))` and `Widget::render_configured(|cx, family, config| ..)`. Each instance shows what the user picked in the system's « Edit widget » sheet (iOS 17+, macOS 14+, visionOS 26: `AppIntentConfiguration` with an entity query over the options the app writes to the App Group); the app renders the default configuration, the ones on screen and, when they fit, every combination. Android shows the default configuration.
- Islands: parts of a web page that run in the browser, as Web Components (`docs/src/guide/islands.md`). `island("chart")` is a WASM island crate built by `prism`, and `web_component("time-ago").module("time-ago.js")` is a plain JS component. Both take `.prop(name, &serde_value)` (changed props update the live island, with no re-mount), `.on(name, handler)` with `IslandEvent::detail::<T>()`, children as the fallback (first paint, no JavaScript, native apps) and `.native(element)`. Modules load the first time a page shows the island. The new `prism-island` crate writes islands in Rust: the `Island` trait (`mount` / `update` / `unmount`), `Host` (shadow root, `emit`), `Props` without serde by default, and `register!`. `prism run|dev|build web` builds `[web] islands` with a size profile and wasm-bindgen as a library (no CLI to install), runs `wasm-opt -Oz` when available, prints raw / gzip / brotli sizes, and fails `prism build` above `[web] island_budget_kb`. Islands are included in the Docker image. The counter's demo is a 20.5 KB gzip canvas chart.
- `WebApp::content_security_policy(&nonce)` and `WebApp::strict_csp()` / `PRISM_STRICT_CSP=1`: the strict policy an app needs (with `'wasm-unsafe-eval'` only when it has WASM islands), sent by the built-in server. `WebApp::islands_dir` serves islands built elsewhere.
- `cx.safe_area()` and `cx.window_size()`: what the system's bars, cutouts, side bars and a root tab bar cover of the window, and the whole window (#28). Reported by iOS / iPadOS / tvOS / visionOS, macOS, Android, watchOS and the web (`env(safe-area-inset-*)`); `Runtime::set_window` for hosts.
- `cx.dismiss_keyboard()`: the focused field lets go and the on-screen keyboard hides, on every backend (#29).
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
- `deployment_target` in the `[ios]`, `[tvos]`, `[watchos]`, `[visionos]` and
  `[macos]` sections of `Prism.toml` (defaults 17.0, 17.0, 10.0, 26.0, 13.0),
  written by `prism new` as comments (#27).
- `prism_build::link_app()` (and `prism_apple_build::link_app`), called first
  in the app's `build.rs` (`prism new` writes it): the Swift runtime's
  `-rpath /usr/lib/swift` and Prism's deployment target for the app's own
  binary, so plain `cargo build` links like `prism` (#27).
- `cx.enable_live_activities()` declares Live Activities at startup
  (`PRISM_DESCRIBE=live_activities`), so iOS bundles without widgets still
  get the extension that draws them (#35).
- `prism::oauth`: RFC 8414 discovery (`/.well-known/oauth-authorization-server`,
  path inserted for issuers with a path) with fallback to and from OpenID
  Connect discovery, `OAuth::server(issuer, client_id)` for plain OAuth 2
  servers; RFC 7591 dynamic client registration (`.dynamic_registration(name)`,
  a public client registered once and kept in secure storage, a returned
  secret used as `client_secret_basic` or `_post`); `resolve`,
  `refresh_blocking` for background threads and a public `token_request`
  (#32).
- `picture(url).header(name, value)`: remote images behind a bearer token,
  loaded natively with the header on every backend (Rust loader on Apple,
  `HttpURLConnection` on Android, `URLRequest` on watchOS, GTK) and fetched by
  the server on the web (a session-bound `/_prism/picture/<id>`: the token
  never reaches the browser). GTK now loads remote pictures at all (#32).
- Navigation bar options per screen (#30): `screen(..).leading(bar_button(..))`
  and `.trailing(..)` (title or SF Symbol, an action, an accessibility label)
  as `UIBarButtonItem`s, macOS header buttons, GTK header buttons, Android
  header buttons, watchOS toolbar items and buttons in the web's
  `.prism-header`; `.large_title(false | true)` (`largeTitleDisplayMode` on
  iOS, iPadOS and visionOS). The tab bar's selected tab and the header bars'
  buttons take the app's accent color (`App::set_accent_color`, set by
  `prism_ui::set_theme`) or `RouterBuilder::tint`, on every backend
  (`Style::tint`, `Tabs::tint`, `Stack::tint`).
- `Router::close` releases every screen of every tab and the sheet (#31);
  `App::entity_count` counts live entities.

### Changed

- The router picks the most specific route that matches a path, whatever the order the routes were added in (#83): segment by segment from the left, a static segment beats a `:param`, which beats a `*rest`, so `/sites/new` wins over `/sites/:site` and `/files` over `/files/*rest`. Routes of the same shape keep their order (the first added wins) and debug builds warn about the one never reached.
- Web deep links no longer build the screen under them (#65): opening `/orders/42` keeps `/` underneath (Back, the browser's history and the header's Back still go there), but that screen is built when Back reaches it, not in the first paint. A deep link no longer loads the root screen's data nor ships its HTML; the page carries an empty hidden placeholder instead. Native backends still build it up front, since their Back shows it (iOS names it in the Back button, the swipe reveals it).
- Bundling asks the app what it declares once (#74): `PRISM_DESCRIBE=all` prints the assets directory, widgets, background tasks, Live Activities, widget buttons and car screen as one JSON object, so `ios-run.sh`, `android-run.sh` and `macos-app.sh` start the host build once instead of up to six times (the app's `init` runs once). An unknown `PRISM_DESCRIBE` question exits with status 2 instead of starting the app, and a failed host build says why the bundler needs it. The `prism` command chapter documents the host build and the `Cargo.toml` layout for apps that enable the native backend only on their device targets (« What the bundlers ask the app »).
- `ui::DateField::value` is an `Option<SystemTime>` (#89); `ChangeEvent::date()` and `minutes()` read an empty value as the epoch and 0.
- Web: the server compresses what it sends.
  - **Socket.** Browsers with `DecompressionStream` ask for compressed frames (`&compress=deflate-raw`). Messages of 1 KB or more then arrive as binary frames of raw DEFLATE, and the client applies them in order. Small patches stay text. Axum's tungstenite has no `permessage-deflate`, so this is done at the application level. `prism_web::Frame` frames messages for other WebSocket servers.
  - **HTTP.** `WebApp::handle` gzips pages per request. It gzips the client, CSS, islands and text assets once, cached by hash, with a `"<hash>-gzip"` ETag. It honors `Accept-Encoding` q-values and sends `Vary: Accept-Encoding`.
  - **Result.** On the counter, a cold load of `/` goes from 283 KB to 63 KB over HTTP and from 390 KB to 37 KB over the socket.
- Web: the client no longer writes a `style="…"` attribute on every node. Each distinct style is one rule of a shared sheet, applied by a class named after a short hash of the style (`p1x5rl6u`, the same name as the first paint's `data-ps`), and unused rules are recycled. Interaction states share one `<style>` instead of one per node. Picture fit is a `prism-fit` class.
- Releasing an entity also releases the entities it created with `cx.new`
  and drops the observers it registered with `cx.observe`; releasing a
  router releases its screens and, for the app's router, clears the
  navigator (#31). A screen a route hands out without creating it is never
  released by the router.
- Web: the stack header holds Back and the leading bar buttons in
  `.prism-bar-leading`, the trailing ones in `.prism-bar-trailing`, around a
  centered title (#30).

- `cx.viewport()` is the content area: the window minus the safe area (side tab bars on foldables, iPad sidebars, notches, status bars). Breakpoints and `.at_width(…)` resolve against its width (#28).
- Native layout: a node's minimum width defaults to `0`, like the web's `min-width: 0`, so rows shrink their items instead of overflowing (a label beside an icon wraps in a narrow cell); switches and spinners keep their width on both. Texts' min-content is their longest word on macOS too (#37).
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
- Apps that register no widget and declare no Live Activity are bundled
  without the widget extension (`PrismWidgets.appex`), its provisioning
  profile (`<bundle id>.widgets`) and the App Group, on iOS, iPadOS,
  visionOS, macOS, tvOS and watchOS; their data stays in the app's own
  container. On iOS, `start_activity` needs `cx.enable_live_activities()` (or
  a widget, or `on_activity_push_token`). Android already added widget
  receivers only for registered widgets. (#35)

### Fixed

- Web: a `picker()` with a value no longer shows an empty first row (#96). The placeholder `<option>` is only in the `<select>` while nothing is picked; Safari showed the hidden option as a blank row. The server's first paint already left it out.
- Web: layers (`cx.add_layer`: the toaster, an app-wide dialog) are in the server-rendered page (#93), in `<div id="prism-layers">` with their styles, so a toast shown while the page is built (« Signed in », a flash after a redirect) is on the first paint instead of appearing once the socket connects; the live tree replaces them like the root. The first paint's style rules now match `:is(#prism-root,#prism-layers) [data-ps=…]`. `HtmlBackend::to_page()` returns the root's and the layers' markup with their rules (`PageHtml`), `HtmlBackend::layers()` the layer roots.
- Web: `div().link(path)` and `text(..).link(path)` are `<a href>` elements instead of `div[role=link]` (#59), on the server's first paint and in the client: Cmd/Ctrl-click, a middle click and the context menu open the link in a new tab or copy it, hovering shows its URL, and it works before the socket connects (a page load). A plain click still navigates in the app. App paths are under the mount point (`WebApp::base_path`), rich text links of the first paint too; a URL gets `rel="noopener"`. A new `Patch::SetLink`, sent right after the node's `Create`, carries the destination (native backends ignore it); on the web a node that gains or loses its link is a new element.
- Capitalized rich text no longer capitalizes a word again where a span splits it (#90): `rich_text([span("hel"), span("lo")]).capitalize()` reads « Hello », not « HelLo », on the native backends (AppKit, UIKit, GTK, Android, watchOS), as the web's CSS already did. Each run is transformed after the text of the run before it; an inline icon ends a word.
- A screen's leading `scroll()` that runs under the bars lays its content out in what they leave visible (#76): on iOS, `scroll().child(div().min_h(relative(1.)).justify_center())` sized the column to the whole screen while UIKit insets it by the status bar (and navigation bar) and the home indicator, so centred content sat half the bars below the centre and a page that fits scrolled. `prism-native` now pads the scroll view's layout by the bars it really runs under (measured from its frame, so a footer below it takes the bottom one), places its children and sticky views in the content's coordinates and keeps the content size to the visible height. Android, macOS, Linux and watchOS report no bars there (their screens sit inside the safe area), so nothing changes for them; the web already sized to the visible area.
- A value the app sets on a focused text input or text area now reaches the field (#73): a Markdown shortcut stripping « # », clearing after a command or normalising the input no longer leaves the field showing the old text (the next keystroke sent it back, and app and field drifted apart). Echoes of what the user just typed still leave the field alone, caret and IME composition included. The web client remembers each value it sent until the server renders it back, so a late echo is never mistaken for a change; an app value applies even while focused, the caret keeping its distance from the end, and waits for `compositionend` during an IME composition. GTK does the same (its events reach the app on a later turn), and writes into a focused `GtkEntry` / `GtkTextView` instead of skipping it; AppKit, UIKit and Android, which already wrote app values, keep the caret's distance from the end too. `prism_native::fields` (`Typed`, `caret_from_end`) for backends.
- An explicit zero padding on a field (`text_input().p(0.)`, `.px(0.)`, `.pt(0.)`…) is a real zero inset instead of « no padding set » (#69): the default 6 × 8 inset (and, on the web, the 36 px it made) applies only when no padding was given, so the field is as tall as its line plus its padding unless `.h(…)` is set. `Style::padding_set` records that a padding setter was called and `Style::has_padding()` answers for every backend: the web writes a zero padding (a text area's three-line minimum drops the default inset too), native hosts get `pad: [0, 0, 0, 0]` and measure the line alone, AppKit and UIKit draw their text at the edge of Prism's box, and GTK drops the theme's padding and minimum entry height. The same holds for a filled button's and a text area's own inset.
- An overlay inserted already open, in the same update as its parent (a menu appearing under a field), is shown (#70). The web showed nothing (`showPopover()` / `showModal()` ran before the box was in the document, and a dialog fell back to a non-modal `open`): overlays now open once the batch has placed them, on the first render too. iOS and iPadOS presented a popover without a source view, and Linux and Android had no anchor yet: they now present it after the layout that places it.
- macOS: restyling a view whose decoration has its own layer (a gradient, an inset shadow, per-side or dashed borders), e.g. tabs switching between `.border_bottom(…)` and `.border_left(…).border_right(…)`, no longer aborts with « mutation detected during enumeration » (#66): the old sublayers are cleared in one call instead of being removed while their live array is enumerated. The UIKit backend copies `subviews` / `childViewControllers` before removing from them, too.
- A task spawned from `render` (a lazily started `cx.spawn`, `cx.background` included) starts right away instead of waiting for the next event (#57): spawning outside the task loop wakes the UI thread, as a waker does. On the web the session no longer sleeps until the pointer moves; Linux queues its wake instead of running it nested in the update that spawned the task. A task that finishes without changing anything renders nothing, so nothing loops.
- `svg()`: `currentColor` is `Color::current()` instead of black when the SVG sets no `color` (#51), so `svg(icon).text_color(…)` tints it and an adaptive text color follows dark mode, natively, on the web and in widgets. Shapes painted in `Color::current()` follow their node's text color, interaction states included.
- Web: text inputs and text areas that draw their own box (padding, a background, a border, `.border(0.)`) or refine `.focus(…)` no longer get the global `:focus-visible` outline (#52).
- Web: pictures drop their gray loading background once loaded (#54), so transparent images show their parent.
- macOS: a styled text field taller than its line (`.h(36.)`) centers its text vertically, like the web and iOS, instead of drawing it at the top (#55).
- An element with `.animate(ms)` (or `.spring`, `.transition`) that appears
  at an update no longer animates from no style at all (#50). Where an
  element without a transition was at the same place, the reconciler reused
  that node and restyled it, so the card faded in from transparent with its
  padding growing from 0, on the web and on the native backends. Such an
  element is now a new node, which every backend shows with its style at
  once; keyed nodes and entity roots stay the same node. On the web, the
  rule an element's own width picks (`.when_width_at_least`) at its first
  layout also applies without a transition. Later changes still animate.
- iOS, tvOS, visionOS and watchOS simulators: `secure_storage()` works again
  in apps without widgets or Live Activities (#38). Since the App Group was
  left out of those apps, their simulator builds had no entitlement the
  keychain could use as an access group, so `set` failed with
  `errSecMissingEntitlement`. Simulator builds now carry the
  `application-identifier` a device profile grants (`SIMULATOR.<bundle id>`,
  the extension `<bundle id>.widgets`). It goes in the binary's entitlement
  sections only, because the simulator does not launch an ad hoc signature
  that claims it.
- iOS device and store builds: the « no Push Notifications capability » note
  (and the iCloud and Sign in with Apple ones) comes from the app's own
  provisioning profile only, never from the widget extension's, and names
  the bundle id (#45). The profile logic moved to
  `platform/scripts/profile-entitlements.py`, with unit tests.

- Configurable widgets: the iOS « Edit widget » sheet shows each untouched parameter's default option instead of the parameter's title as a placeholder (#49). The generated intent's parameters are required, so WidgetKit fills them with the entity query's `defaultResult()` when the widget is added; the query never answers nil, and the extension reads values through `valueState` (iOS 18.2+), so widgets added before the fix keep their default.
- macOS: the app handles its URL scheme (`application(_:open:)`), so a widget's links open their path in the app (they only brought it to the front), and widget buttons' action links run (#47).
- iOS: a Live Activity started by a push reports its update token through
  `on_activity_push_token`, with `activity: Some(id)`, and again whenever
  the token changes (#43). `Push::start_live_activity(kind, id, ..)` gives the
  activity its id, and the app reports the token with that id. The app now
  registers activities it did not start in this run (started by a push, or
  by an earlier run) at launch and as they start, so `update_activity` and
  `end_activity` reach them (`App::adopt_activity` for backends). A widget's
  root `.bg(..)` fills the whole widget (its container background).
- Web: form fields (inputs, selects, text areas, checkboxes, switches) carry a `name` (`prism-<node>`), so Chrome no longer reports a form field without an id or name.
- An iOS app built outside the Prism checkout no longer stops at launch on
  `@rpath/libswift_Concurrency.dylib` (#27): `prism` sets
  `IPHONEOS_DEPLOYMENT_TARGET`, `TVOS_…`, `WATCHOS_…`, `XROS_…` and
  `MACOSX_DEPLOYMENT_TARGET` for every `cargo` it runs (from `Prism.toml`,
  unless the environment sets them; only Prism's own `mise.toml` did), and
  the app's binary gets `-rpath /usr/lib/swift` from `prism_build::link_app`
  (a backend's `rustc-link-arg` never reached it).
- Docs: the iOS deployment target and how to change it, and the exact OAuth
  redirect URI (`prism-<cargo package, _ as ->:/oauth`, with an example)
  (#34). Unbundled macOS apps use the same scheme (it kept `_`).
- `cx.current_path()` returns `None` once the router is released instead of
  panicking, and an entity released by its own handler (a Sign out button
  on a screen) stays released (#31).
- iOS: the keyboard's inset no longer stays when the root is swapped while the keyboard is up (#29).
- iOS / tvOS / visionOS: borders (and field borders) of an adaptive color follow the appearance; layer colors resolve with the app's new traits rather than the views' stale ones. macOS redraws layer colors on appearance changes; GTK rebuilds adaptive CSS rules when the appearance changes (#36).
- iOS: a tab's screen shown for the first time lays out inside its safe area (content no longer runs under a foldable's side tab bar) (#28).
- iOS: bordered text inputs inset their text and placeholder by their padding, checked with a 44 pt field `.px(12.)` (#33).
- Padding works on every kind of node (#14): `.p()`, `.px()`, `.pl()`… on a
  text, button, text input, text area, timer or control grow its box and
  inset its content on the web and every native backend (they were ignored
  outside containers). On buttons, inputs and text areas the app's padding
  replaces the control's own inset; a filled web button keeps its 6 × 14
  only when it sets none (it used to add them up).
- Web text inputs get their default padding and border only when the app
  sets none, and `.border(0.)` now removes the default border of inputs,
  text areas, pickers and date pickers on every backend (#14).

- Web: on windows 1024 px wide or more the tab bar is a sidebar again; the
  Tabs node no longer carries a `flex-direction` that overrode the media
  query in `prism.css`.
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
