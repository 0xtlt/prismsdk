# Changelog

All notable changes to Prism. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (before 1.0, a breaking change
bumps the minor version). Every crate of the workspace shares one version.

## Unreleased

### Fixed

- Pictures no longer stay in memory forever (#343). macOS, iOS and Windows
  kept every decoded picture by its source, so an app showing a new
  `picture_data` after each gesture grew by one decoded image each time
  (about 20 MB for 2880×1800, past 2 GB after 57 images). A decoded picture
  now stays while a node shows it; once none does, it waits in a pool of
  at most 64 MB, the least recently used going first, so going back to a
  screen still shows its pictures at once. The bitmaps of `svg()` images on
  macOS and iOS are bounded the same way (they were 64 at most, whatever
  their size). In a test app swapping a 2880×1800 `picture_data` every
  100 ms on macOS, memory stays at about 170 MB instead of reaching 4 GB
  after 200 images.
- Changing a picture's source no longer flashes the background (#343): the
  picture keeps its image until the new one is ready, like `<img>`, on
  macOS, iOS, Windows and Linux (remote pictures), as it already did on
  Android and the web. A load that finishes after the picture moved on to
  another source is dropped (macOS could show an older picture over a
  newer one, Linux a remote picture over a newer local one). A source that
  fails to load, or an empty one, shows nothing instead of the previous
  picture (Linux and Android kept it). On the web a loaded picture keeps
  no loading background while its next source loads.

## 0.7.0 (2026-11-27)

`svg(markup)` (and `picture_data(bytes, "image/svg+xml")`) now draws full
SVG documents natively on every app backend: linear and radial gradients,
`<text>` and `<tspan>`, `data:` URL `<image>`s, whole-group `opacity`,
`clip-path`, luminance and alpha `mask`, drop shadows, glows and blurs,
`<use>`, `<symbol>`, nested `<svg>` and `<style>` sheets (#342). Inline
`<svg>` on the web, Core Graphics and Core Text on Apple platforms,
`Canvas` on Android, cairo and pango on Linux, and Direct2D and
DirectWrite on Windows; flat-path surfaces (widgets, Live Activities, car
screens, watchOS, chip icons) fall back to paths with each gradient's
middle color. `shape()` paths also gain gradient fills and strokes
(`Path::fill_gradient`, `Path::stroke_gradient`), and `Svg::skipped`
reports unsupported SVG features instead of dropping them silently.
**Breaking:** `Path` adds `fill_gradient` and `stroke_gradient`, `Drawing`
adds `size` and `items`, and `Svg` adds `skipped`.

### Added

- Full SVG document rendering in `svg()` and `picture_data(bytes, "image/svg+xml")`
  (#342): `linearGradient` and `radialGradient` on fills and strokes
  (`gradientUnits`, `gradientTransform`, `spreadMethod`, `href`), `text` and
  `tspan` (`x`, `y`, `dx`, `dy`, `font-family`, `font-size`, `font-weight`,
  `font-style`, `letter-spacing`, `text-anchor`, `dominant-baseline`), `image`
  with `data:` URLs (`preserveAspectRatio` `meet`, `slice`, `none` and all nine
  alignments), groups with whole-group `opacity`, `clip-path`, luminance and
  alpha `mask`, drop shadows, glows and blurs (`feDropShadow`, standard shadow
  filter chains, `feGaussianBlur`, CSS `drop-shadow()` and `blur()`), `symbol`,
  `use`, nested `svg`, `viewBox` with `preserveAspectRatio`, CSS length units,
  and `<style>` sheets (type, `.class`, `#id`, descendant and child selectors).
  Drawn natively on every app backend: inline `<svg>` on the web (server and
  client), Core Graphics and Core Text on macOS, iOS, iPadOS, tvOS and
  visionOS, `Canvas` on Android (render effects on Android 12+, bitmaps before),
  cairo and pango on Linux, and Direct2D and DirectWrite on Windows (flat paths
  stay XAML `Path`s). Surfaces that draw flat paths only (widgets, Live
  Activities, CarPlay, Android Auto, watchOS, and a `rich_input` chip's icon)
  keep the shapes with each gradient's middle color.
- Gradient fills and strokes on `shape()` paths (#342): `Path::fill_gradient`,
  `Path::stroke_gradient`, `shape::Gradient` (`linear`, `radial`, `spread`,
  `color_at`), `shape::Item` (`Path`, `Text`, `Image`, `Group`), `Drawing::size`,
  `Drawing::items`, `Drawing::resolve_items`, `Drawing::is_flat`,
  `Drawing::flattened`, `Drawing::plain`, `Widgets::set_drawing_items`, and
  `RemoteWidgets::with_drawing_items`.
- `Svg::skipped` (#342): `prism::svg::parse` reports unsupported features
  (patterns, markers, `foreignObject`, `textPath`, other filters, non-`data:`
  images, unsupported CSS selectors, animations) instead of dropping them
  silently; debug builds log them once per document.
- Counter example: `/components/svg` screen showing card, diagram and effects
  documents at full and half sizes (#342).

### Changed

- **Breaking:** `Path` has new public fields `fill_gradient` and
  `stroke_gradient`, `Drawing` has `size` and `items`, and `Svg` has `skipped`
  (#342): code building them with struct literals adds them or starts from
  `..Default::default()`.
- `svg().tint(color)` now recolors painted `<text>` runs as well as paths
  (#342).

## 0.6.0 (2026-11-26)

Widgets stay current without the app: `Widget::refresh` runs on each of
the system's reloads, in the widget extension on Apple platforms and from
an alarm on Android, and `Widget::reload` sets when (#331). Popovers and
dialogs can open at a text field's caret, for completion lists (#328), or
under a clicked chip of a `rich_input` (#327), whose chips take a vector
icon, styled label runs, a ring, radius, height, max width, tool tip and
a state (#329). Boxes take linear gradients that animate
(`bg_linear_gradient`, #330). A push inside the app stays in the selected
tab, so Back returns where the user came from (#332), and `rich_input`
keeps its undo history across the app's answer (#338).
**Breaking:** in-app pushes under another tab's root no longer switch tab;
`TextRun`, `ClickEvent` and `WidgetSnapshot` have new fields, `UiEvent`
a new variant, and macOS apps with widgets move their data into their
App Group.

### Added

- Linear gradient fills (#330): `.bg_linear_gradient(angle, [(color, stop), …])`
  on `div` and every styled box, shorthand for
  `.bg_gradient(Gradient::linear_stops(angle, stops))`, in the CSS
  convention everywhere (`0` to the top, `90` to the right, `180` to the
  bottom; the first and last colors reach the corners). The end of a
  scrolling list fades into its background in one box instead of stacked
  strips. `.bg_fill(fill)` sets the whole background to a `Fill` (a solid
  color or a gradient), replacing the other, where `.bg` and `.bg_gradient`
  layer: `.hover(|s| s.bg_fill(accent))` turns a gradient button solid.
  `Gradient::normalized` places stops like CSS (clamped, never going back,
  one stop a solid fill), applied by the builders, so native APIs always get
  increasing positions and two colors at least; `Gradient::unit_points` and
  `Gradient::interpolates_with` serve backends.
- Gradients animate (#330): with a transition, a gradient changing into one
  of the same kind with as many stops animates its colors, positions and
  angle on the web (registered custom properties, named in the transition:
  `transition: all` leaves them out), iOS, tvOS and visionOS (Core Animation
  on the gradient layer) and Android (the box drawable, which now animates
  its background color too).
- Widgets and Live Activities draw gradients (#330): snapshots carry them
  (`gr`); SwiftUI draws them on iOS and macOS (a root gradient fills the
  whole widget, under the system's margins too), CSS on the web, and
  Android's `RemoteViews` a stretched bitmap under the node.
- A popover or dialog can open at the caret of a text field (#328):
  `Overlay::anchor(OverlayAnchor::Caret)`, or `anchor_caret()`, anchors
  it at the caret of the field inside its parent (`text_input`,
  `text_area`, `rich_input`, `rich_text_input`), with the usual side,
  flip and keep-inside-the-window rules. Every backend measures the caret
  when it lays the overlay out and follows it as the user types, selects
  or scrolls, without asking the app; with `take_focus(false)` the field
  keeps the focus and the keys. `OverlayAnchor` (`Parent`, the default,
  or `Caret`) can gain kinds of anchors; `OverlayConfig::anchor`
  (`anchor=caret` in the config string) and `controls::caret_in_field`
  for backends. The counter example's Forms section has a template field with a
  `{{` completion list.
- Chip options for `rich_input` (#329), on the web, macOS, iOS, iPadOS,
  visionOS, Android, Linux and Windows. A chip stays one character:
  - a vector icon: `chip_icon` takes an SVG (`ChipIcon::svg`), a
    `Drawing` or a `Path` as well as a symbol name, one em tall,
    `Color::current()` painted in the chip's text color;
  - styled label runs (`chip_label_runs([span("Ada").bold(), …])`),
    cut as a whole;
  - its box: `chip_ring(width, color)`, `chip_radius`, `chip_height`,
    and `chip_max_width`, which cuts the label with an ellipsis;
  - `tooltip(text)`: `title` on the web, the platform's tool tip on
    native chips and inline icons (none on touch screens: there is no
    hover);
  - state and identity: `chip_state("open")` (`data-state` and the class
    `prism-chip-open` on the web), `chip_test_id(id)` (`data-testid`, an
    accessibility identifier or attribute natively), and `chip_attr` for
    other `data-*` attributes on the web.

  `prism::ChipIcon`, `attributes::CHIP_DATA_ATTRIBUTES`, prism-testing's
  `Host::chip(id, test_id)`, and prism-native's `chips` module (the box
  every native backend draws) are new.
- A popover or dialog anchored to a chip of a `rich_input` (#327):
  `.anchor_chip(field_id, chip_key)` opens it on its `.side(…)` of the
  chip, lined up as `.align(…)` says, flipped without room and kept inside
  the window. Every backend finds the chip in its own text layout (the
  web's DOM range, the Apple text views' glyph boxes, Android's `Layout`,
  GTK's iter locations, RichEdit's `ITextRange::GetRect`) and keeps the
  overlay on it as the text wraps, the field scrolls or the window
  resizes, through the caret anchor's placement (#328). Without the chip
  it is anchored to the field, without the field to its parent. Backends
  receive `OverlayAnchor::Text { field, start, end }` (the chip's
  character) or `OverlayAnchor::Field(field)` (`anchor=text:…` and
  `anchor=field:…` in the config string), and the field's view through
  `Widgets::set_overlay_anchor`; `OverlayAnchor` is `#[non_exhaustive]`.
- `ClickEvent::span_rect` and `span_frame` (#327): a click on a chip of a
  `rich_input` says where the chip is, in the field and in the window's
  layout coordinates. Backends report it as `UiEvent::RunClick`; remote
  hosts add `x`, `y`, `w`, `h` to their `menu` event.
- Windows: a tap on a chip with `on_click` in a `rich_input` reports it,
  as on the other platforms.
- The counter example's rich note opens a menu under a clicked chip
  (open, remove).
- Widgets refresh without the app (#331): `Widget::refresh(|cx| async { … })`
  fetches fresh data (HTTP with `prism::http`, the app's storage) each time
  the system asks for new drawings, then the widget is drawn again with
  `render` and `ahead`, while the app stays closed. `Widget::reload(policy)`
  and `reload_after(delay)` say when the system asks (`WidgetReload::After`,
  `AtEnd`, `Never`; 15 minutes by default for a widget with a hook);
  `refresh_timeout` bounds the hook (10 s by default). A hook that fails or
  times out leaves the previous drawings. On iOS, iPadOS, visionOS and
  macOS the app's binary becomes the WidgetKit extension's executable
  (`prism run`, `build` and `publish` link the extension's Swift into it)
  and runs the hook on WidgetKit's timeline reloads (measured: 17 to 20 MB
  on the iOS simulator, under the extension's ~30 MB); on Android an
  inexact alarm runs it in the app's process, without a screen when the app
  is closed. `Runtime::refresh_widget` and `App::set_widgets_refresh_only`
  are the backends' side. The Counter example has a "Departures" widget
  when built with a feed (`COUNTER_DEPARTURES_URL`).

### Changed

- **Breaking:** a push within the app stays in the selected tab (#332).
  `navigate`, `navigate_to`, `navigate_with` and `replace` to a path
  under another tab's root (`/devices/7` from the Rooms tab) no longer
  switch to that tab: the screen goes on the selected tab's stack, so
  Back returns to the screen the user came from and the tab bar stays,
  as on iOS and Android. Only a tab's root exactly, or a link from
  outside (a deep link at launch, `Runtime::open_link` from a widget or
  a notification), selects another tab. On the web each history entry
  is reopened in the tab it was made in, and a reload gives the screen
  back to that tab; the URL is still the pushed path.
- On the web, a chip with a `bg` keeps the stylesheet's 4 px radius (or
  its `chip_radius`) instead of the 3 px inline radius a highlighted span
  gets; plain chips look as before (#329).
- **Breaking:** `TextRun` has new public fields for the chip options
  (#329): code that builds a `TextRun` literal without
  `..TextRun::default()` adds it.
- `Span::chip_icon` takes `impl Into<ChipIcon>`: a symbol name still
  works as before (#329).
- Android measures a rich input with its runs, its chips as wide and tall
  as they are drawn: a field whose chips wrap to more lines is tall
  enough for them (#329).
- **Breaking:** `ClickEvent` has `span_rect` and `span_frame` (code
  building one with a struct literal adds them, or starts from
  `ClickEvent::default()`), `UiEvent` the `RunClick` variant (backends
  matching it exhaustively add the arm), and `VNode` a `chip_anchor`
  field (#327).
- **Breaking:** `WidgetSnapshot` has `reload` (the widget's reload policy,
  written to the snapshot as `"reload"`) and `refresh` (whether it has a
  hook) (#331).
- macOS apps with widgets keep their data in their App Group
  (`~/Library/Group Containers/<group>/prism-data`), as on iOS, so the
  widget extension shares it; data in `~/Library/Application Support/<bundle
  id>` moves there on the first run (#331).
- macOS widget-button links stay valid for the widgets a refresh did not
  draw again: the link ledger records which widgets each drawing covers
  (#331).

### Fixed

- `rich_input` undo (#338). Web: the field's history survives the app's
  runs that answer the user's edit (runs normalized after a chip was
  deleted): ⌘Z brings the chip back, and the app hears it. Runs with no
  edit in flight are new content and start the history over. ⌘Z, ⌘⇧Z and
  Ctrl+Y are handled on `keydown`, so the browser's own undo no longer runs
  as well, and a key the app consumes (`consume_keys(&["cmd+z", …])`) or a
  menu shortcut never reaches the field's history. Linux: undoing the
  deletion of a chip put back a plain character that the app's runs then
  turned into a chip with an irreversible action, which wiped the undo and
  redo history; the chip's anchor now comes back while the undo applies.
  macOS: runs from the app with other characters left the text view's undo
  steps in place, and ⌘Z then raised `NSRangeException` or garbled the
  text; that history now starts over, as on iOS and Android. Windows: the
  same runs start RichEdit's history over too (with their formats and chip
  pictures). The rich input guide (Forms › Rich inputs) says how an app
  with its own undo consumes the keys.
- `FileStorage`: two keys that differ only after their last dot (`board.today`,
  `board.tomorrow`) no longer share one temp file, and each write has its
  own, so the app and its widget extension (#331) can write at once.
- iOS, tvOS, visionOS: a diagonal linear gradient in a box that is not
  square follows CSS's angle, as on macOS (#330): `CAGradientLayer` spaces
  colors in unit coordinates, so the CSS line is converted rather than
  divided by the size.
- Web: a state that changes the background color of a box with a gradient
  (`.bg_gradient(g).hover(|s| s.bg(c))`) keeps the gradient over the new
  color, as natively; the `background` shorthand of the state's rule used
  to remove it (#330).
- macOS: the `PRISM_TAP` test hook scrolls its target into view and does
  not click a text field that is already editing (the click went to the
  container's key handler instead of the field).
- macOS: a popover anchored at a caret or a chip (#327, #328) no longer
  crashes when it follows an anchor AppKit has not shown it at yet (the
  anchor scrolled out of view): it is shown there instead of moved.

## 0.5.0 (2026-11-25)

The web's history follows the path: browser Back and Forward reopen the
screen each entry showed, a deep link is one entry, and
`Screen::on_back_request` can hold the browser's Back for unsaved changes
(#324). `copy_text` says whether it worked, and `copy_on_click` copies
during the click so Safari takes it (#323). Widgets keep `min_w`, `min_h`
and `nowrap` on every platform (#321), a click anywhere in a
`rich_text_input` places the caret (#322), and iOS bundles require arm64
(ITMS-90502, #325).
**Breaking:** `NavigateEvent` has a `depth`, and `UiEvent` and `Patch`
have new variants.

### Added

- `copy_text` says whether it worked (#323): `cx.copy_text(text).await`
  in a task returns `Result<(), DeviceError>` (every backend answers;
  `Unavailable` on tvOS and watchOS). `copy_on_click(text)` on `div`,
  `button` and `text` copies when clicked: on the web the page writes it
  during the click itself, which Safari requires; `on_copied` hears the
  result (`CopiedEvent`) on every platform.
- `Screen::on_back_request` (#324): on the web, the browser's Back and
  Forward (and the page's Back button) leave the screen in place and ask
  the app, which goes with `cx.back()` once the user agrees (unsaved
  changes).
- `prism publish` checks that the app and its widget extension require
  `arm64` (ITMS-90502, outside watchOS).

### Changed

- **Breaking:** `NavigateEvent` has a `depth` (the browser's history
  entry's depth, #324), `UiEvent` the `Copied` and `History` variants and
  `Patch` the `SetCopyOnClick` and `SetBackRequests` ones (backends and
  hosts matching them exhaustively add the arms).
- The web's history follows the path (#324): each entry keeps its path and
  depth, a sidebar's `navigate_root` adds an entry, `cx.replace` and
  `replace_query` replace it, `cx.back()` returns to the entry it came
  from, and Back or Forward between two entries of the same depth shows
  the entry's screen. A deep link is one entry (none for the screen under
  it).

### Fixed

- iOS, tvOS and visionOS bundles (the app and its widget extension)
  require `arm64` in `UIRequiredDeviceCapabilities`: App Store Connect no
  longer rejects the upload with ITMS-90502 (#325).
- Widgets: a `nowrap` text hugs its content instead of stretching its
  parent across the row, and `min_w` / `min_h` reach WidgetKit, Android
  widgets and Live Activities on the web (#321).
- `rich_text_input`: a click anywhere in the editor focuses it, below the
  text (the caret goes to its end), beside a line or in its padding, as
  in a `<textarea>` (#322).
- The web's `copy` device call and Apple's answer, so `copy_text` no
  longer leaves a task waiting (#323).

## 0.4.0 (2026-11-25)

Store compliance, built in. Every Apple bundle carries a privacy manifest
with the required-reason APIs its binary uses and Prism's reasons for them
(ITMS-91053), binaries reference only the protected APIs the app declares
in `[permissions]` (ITMS-90683), and app icons lose their alpha outside
macOS (ITMS-90717). `[permissions]` covers contacts, calendars, bluetooth,
tracking, health and seven more, on every platform. Android app bundles
carry native debug symbols and are checked for 16 KB pages; `prism
publish` checks all of it before uploading. On the web, shapes in hidden
boxes, `hit_test(true)` in states and moved elements with a transition now
behave as natively (#319).
**Breaking on Apple platforms:** location, the camera, the microphone, the
photo library and Face ID need prismsdk's `location`, `location-always`,
`camera`, `microphone`, `photo-library` and `face-id` features (`prism`
turns them on from `[permissions]`).

### Added

- Privacy manifests (#318): every Apple bundle (the app and its widget
  extension; iOS, iPadOS, tvOS, visionOS, watchOS, macOS) carries a
  `PrivacyInfo.xcprivacy`. It declares the required-reason APIs the
  bundler finds in each binary, with Prism's reasons for its own uses
  (FileTimestamp `C617.1`, SystemBootTime `35F9.1`, DiskSpace `E174.1`),
  so App Store Connect no longer flags ITMS-91053. Prism.toml `[privacy]`
  sets tracking and its domains, `[privacy.apis]` the app's own reasons
  (`user_defaults = ["CA92.1"]`), `[privacy.collected]` the data it
  collects; a `PrivacyInfo.xcprivacy` next to Prism.toml is merged in. App
  Store builds for the iOS family stop on a referenced category without a
  reason, a protected API without its purpose string, or tracking without
  domains or its prompt.
- `[permissions]` knows contacts, calendars, reminders, bluetooth, speech,
  tracking (App Tracking Transparency), health, nfc, media_library,
  homekit, siri and local_network: their Info.plist texts, Android
  permissions (`READ_CONTACTS`, `BLUETOOTH_SCAN`, `AD_ID`…), Mac App Store
  entitlements and Windows capabilities. An unknown name is an error.
- App icons lose their alpha channel outside macOS (ITMS-90717): filled
  with white, the front layers of tvOS and visionOS kept, with a note when
  the icon was transparent.
- Android app bundles carry the library's native debug symbols
  (`BUNDLE-METADATA/com.android.tools.build.debugsymbols`), and ship it
  stripped: Play Console's crashes show function names.
- Android builds check that the library is aligned for 16 KB pages (an
  error for app bundles), and `prism publish android` checks every library
  in the bundle.
- `prism publish` checks the privacy manifests of the app and its widget
  extension, and that the iOS and watchOS app icon is opaque.

### Changed

- **Breaking on Apple platforms:** Prism's code for location, the camera,
  the microphone, the photo library and Face ID is compiled in only with
  prismsdk's `location`, `location-always`, `camera`, `microphone`,
  `photo-library` and `face-id` features, because App Store Connect wants
  a purpose string for every protected API a binary references (ITMS-90683).
  `prism` turns them on from `[permissions]`; a build without `prism` lists
  them in Cargo.toml. Without one, the API fails with a message naming the
  `[permissions]` entry and `capabilities()` reports it unavailable.
  LocalAuthentication is linked on iOS and visionOS only with `face-id`.
- macOS reads « Double-click a window's title bar » through CFPreferences,
  not NSUserDefaults (a required-reason API none of Apple's reasons covers
  for a system setting).

### Fixed

- Web: a `shape()` drawn while its box has no size (inside a `display:
  none` ancestor) waits for one instead of keeping a 0×0 viewBox, so icons
  revealed on hover show (#319).
- Web: `.hit_test(true)` in a state (`hover`, `group_hover`…) takes
  pointer input back (`pointer-events: auto`) where the base has
  `.hit_test(false)` (#319).
- Web: an element with a transition (`.spring()`, `with_animation`) that
  moves among its siblings slides to its new place (FLIP), as natively
  (#319).
- Android: a Maven repository that answers an error (429, 5xx) or doesn't
  answer is asked again (as Retry-After says, else after 1, 3 and 9 s),
  then stops the build with that error. It used to count as not having the
  file, and the build went on to fail in `javac` on the car library's
  classes.

## 0.3.0 (2026-11-25)

In-app purchases (StoreKit 2, Google Play Billing, the app's own store on
the web), frame callbacks for app code (`cx.frames`, `cx.on_next_frame`)
driven by each platform's display link, a `VirtualList` that code can
scroll, and `[app] orientations` to lock an app to portrait. iOS apps start
~1 s sooner: AuthenticationServices, PhotosUI and StoreKit are linked only
when the app asks for them. Android scrolls a long list from code at a
fraction of the CPU, the right distance, and inside its bounds.
**Breaking on Apple platforms:** Sign in with Apple, `oauth` sign-in,
passkeys, the photo picker and in-app purchases need prismsdk's `sign-in`,
`photo-picker` and `purchases` features.

### Added

- In-app purchases (#306): one-time unlocks (non-consumables) through
  the platform's store. `cx.store_products(ids)` gives names and prices,
  localized; `cx.purchase(id)` / `purchase_with(Purchase::new(id).account_token(uuid))`
  answers `Purchased`, `Pending` or `Cancelled`; `cx.restore_purchases()`;
  `cx.owns(id)` and `cx.entitlements()` answer at launch (the store's last
  answer is kept between runs), in render and in widgets;
  `cx.on_entitlements` hears purchases made elsewhere, refunds and
  approved Ask to Buy. Capability: `purchases`.
- Apple platforms: StoreKit 2, with prismsdk's `purchases` feature (verified transactions, finished;
  `Transaction.updates`; `AppStore.sync()` to restore; the purchase sheet
  on the app's scene or window).
- Android: Google Play Billing 9.1.0, bundled with Prism.toml
  `[store] enabled = true` (fetched from Google's Maven, locked in
  `android-deps.lock`). Purchases are acknowledged; the app reads them
  again each time it comes back.
- The app's own store: `trait Store`, set with `Application::store` or
  `cx.set_store`. On the web it sells through the host's payment
  provider: the checkout page opens in a popup and comes back to the
  server (`/_prism/store/return`, a one-time state per session), and
  `prism_web::entitlements_changed(buyer_id)` updates the buyer's live
  sessions from a webhook. `TestStore` sells at once, from code or a
  StoreKit configuration file (`.storekit`), for tests and debug builds.
- Frame callbacks (#311): `cx.on_next_frame(|this, frame, cx| …)` runs
  once on the next frame the display shows; `cx.frames(…)` on every frame
  until `cx.stop_frames(ticker)`, like `requestAnimationFrame` or
  `CADisplayLink`. `FrameInfo` gives the frame's `timestamp`,
  `target_timestamp`, `elapsed` and `delta`. Driven by the display link
  (CADisplayLink on iOS, iPadOS, tvOS and visionOS; the screen's display
  link on macOS; Choreographer on Android; `requestAnimationFrame` on the
  web; the GTK frame clock; `CompositionTarget.Rendering` on Windows), a
  60 Hz timer elsewhere (watchOS). Frames pause in the background.
- `VirtualList::set_id(id, cx)` names its scroll view, so `cx.scroll_to`,
  `scroll_by` and `scroll_offset` drive it; `scroll_to_row(row, animated,
  cx)` scrolls to a row (#309).
- Prism.toml `[app] orientations` (#310): `["portrait"]` locks an iPhone
  app to portrait (`UISupportedInterfaceOrientations`) and an Android app
  (`screenOrientation`); also `portrait_upside_down`, `landscape`,
  `landscape_left`, `landscape_right`. iPad keeps every orientation, as
  multitasking requires.

### Changed

- **Breaking (Apple platforms):** Sign in with Apple, `oauth` sign-in
  (`web_auth`) and passkeys need prismsdk's `sign-in` feature, the photo
  picker (`pick_images`) its `photo-picker` feature, in-app purchases its
  `purchases` feature (#312, #314). Each links a framework
  (AuthenticationServices, PhotosUI, StoreKit) that drags ~1,000 images
  into every launch: in the simulator a one-screen app now reaches `main`
  ~0.4 s after its process starts instead of ~1.45 s. Without a feature,
  its calls fail with a message naming it and its capability is false.
  Other platforms ignore the features.

- Android builds: the Maven resolver (`android-deps.py`) applies version
  constraints as Gradle does: those of the libraries' Gradle module
  metadata (androidx.lifecycle's artifacts at one version) and the Kotlin
  Gradle plugin's (kotlin-stdlib-jdk7/jdk8 at kotlin-stdlib's version).
  Without them, Play Billing next to the car library brought the same
  classes twice and the build failed. Car builds now bundle kotlin-stdlib
  1.8.22 (was 1.7.10), as Gradle would.

### Fixed

- Android: `cx.scroll_to` and `scroll_by` moved the density's multiple of
  the distance asked (#308).
- Android: timers no longer pile up wake-ups: the host keeps one, at the
  earliest time asked (#315). A scroll view's unchanged content size is
  not sent again, and its content is laid out again only when that size
  changes (#316). Together: scrolling a long list from code went from
  65 % to 12.5 % CPU in #313's benchmark, frames over 25 ms from 4 % to
  0.2 %.
- Android: a scroll view clips its content: rows scrolled out of it no
  longer draw over the views above and below it.

## 0.2.0 (2026-11-24)

Layout fixes for native backends. A `flex_1` list in a column sized by
its content now takes its rows' height, as in browsers: a sheet whose list
sat under a search field opened empty on iOS. Sheets whose content is a
navigation stack lay out their screens. A screen's leading scroll view
runs under the side safe area. Also: `timer()` takes the style's font on
Apple platforms, and a quick tap shows `.active()` inside an iOS scroll
view. The minor version goes up because native layouts that relied on
such a list getting no height now show it.

### Fixed

- Native layout: a `scroll().flex_1()` (or any `flex_1` element) in a
  column sized by its content takes its content's height, as in browsers
  (#298). A sheet whose list sat under a search field in a plain column
  showed no rows on iOS (and every native backend) while the web showed
  them. A column that grows (`flex_1`) or has a height still gives the
  list what is left, and the list scrolls.
- Native backends: a sheet, popover or dialog whose content is a
  `stack()` (or tabs) lays its screens out (#298). The sheet showed only
  the stack's title. One whose content is a `scroll()` now gets its
  scroll extent.
- Native backends: a screen's leading `scroll()` runs under the side safe
  area too (#303), as it already did under the bars: it spans the
  screen's width, its scroll indicator at the edge (iPhone Duo's status
  column, landscape cutouts), and its content is inset by the side
  insets.
- iOS, macOS: `timer()` uses the style's font: `font_family`,
  `font_mono()`, italic and weight (#301). It always showed the system
  font. Its digits keep a fixed width.
- iOS: a quick tap shows the `.active(|s| …)` press of an element inside
  a `scroll()` (#302). Scroll views pass touches to their content at once
  and still scroll when a touch that began on a button or field turns
  into a drag. A press shows for at least 100 ms.

## 0.1.4 (2026-11-24)

Fixes to the rich text editor: blocks grow with their text, and links
get a color of their own (the theme's accent, or `link_color`). Also fixes
an abort in iOS debug builds on text fields with `autocomplete`.

### Added

- UI: `RichTextInput::link_color(color)` (#299). The editor's links are
  drawn in the theme's accent by default, not only underlined. The
  document keeps no color for them.
- Spans: `text_color_value()` and `no_text_color()`.

### Fixed

- UI: each block of a `rich_text_input` takes its text's height (#296).
  The editor grows with its content, or scrolls when the app caps its
  height. Before, the blocks shared the editor's minimum height, so a
  long text showed as an empty-looking box.
- iOS: a text field with `autocomplete` no longer aborts debug builds
  (`-[PrismField setTextContentType:]: method not found`, #297). The
  content type is sent unchecked, like the field's other text input
  traits.

## 0.1.3 (2026-11-23)

A rich text editor, keyboard chords an element can keep from its fields,
and two web fixes: the wheel over a horizontal scroll view, and the
typography of islands' hosts.

### Added

- UI: `ui::rich_text_input(doc)`, a rich text editor (#294): paragraphs,
  headings, bulleted and numbered lists, and runs in bold, italic or
  linked, edited without markup on every platform `rich_input` runs on
  (one native field per block, the selection running across them). Its
  value is a `RichText` the app stores, with `to_markdown` /
  `from_markdown` and `to_html` / `from_html` (pasted HTML sanitized to
  the model's subset). Block and inline modes; ⌘B, ⌘I and ⌘K (Ctrl
  elsewhere); commands for a toolbar (`toggle_bold`, `set_block`,
  `set_link`…) and its state (`formatting()`); `on_link_request` for the
  app's link picker; Markdown shortcuts (`- `, `1. `, `## `); copy and
  paste of HTML, Markdown and its own type. Headings and list items are
  named for screen readers with the new `prism.heading`,
  `prism.list_item` and `prism.numbered_item` texts, in Prism's 12
  languages.
- Keys: a `consume_keys` entry can name modifiers, `"cmd+b"`,
  `"cmd+shift+z"`, `"alt+ArrowUp"` (#294): the key is taken from the
  focused field only with exactly those held, `cmd` being ⌘ on Apple
  platforms and Ctrl elsewhere. On iOS, iPadOS and visionOS chords are
  key commands too.
- Spans: `no_bold()`, `no_italic()`, `link_to(path)` and `no_link()`.
- iOS, iPadOS, visionOS and Android: Backspace from the on-screen
  keyboard with the caret at the start of a text area, which deletes
  nothing, is reported as a `Backspace` key to the nodes listening to
  keys (#294), as a hardware press is.

### Fixed

- Web: a horizontal scroll view (`scroll_x()`) no longer stops the mouse
  wheel from scrolling the vertical view around it (#293): each scroll
  view contains overscroll on its own axis only.
- Web: a `web_component` or `island` host now gets its typography
  (`text_size`, `font_weight`, `text_color`, `line_height`…), in its
  `hover`, `focus_within` and breakpoint variants too (#294), so its
  shadow root can inherit the app's text styles.

## 0.1.2 (2026-11-23)

Disclosures for screen readers, a router that tells screens they are on
top, sidebars acting as tabs, and drops that no longer fail silently.

### Added

- Accessibility: `accessibility_expanded(bool)` and
  `accessibility_controls(name)` say whether a disclosure shows its
  content and which element it folds (#291): `aria-expanded` and
  `aria-controls` on the web, `accessibilityExpanded` and linked elements
  on macOS, the expanded status on iOS, iPadOS, tvOS 18 and visionOS 2,
  the state description and expand/collapse actions on Android, the
  expanded state and controls relation on Linux, the ExpandCollapse
  pattern and controlled peers on Windows. Where the platform has no
  state of its own the words come from the new `prism.expanded` and
  `prism.collapsed` texts, in Prism's 12 languages.
- Router: `cx.is_top_screen()` tells a screen whether it is the visible
  one (the screens kept under it for Back and the other tabs' re-render
  too), `cx.on_screen_shown(f)` runs each time it becomes the visible one
  again, and the screen renders again then (#288). `App::top_screen()`
  and `Router::top_entity()` give its entity.
- Router: `cx.navigate_root(path)` and `.link_root(path)` leave `path`
  alone in its stack, without building the root route underneath:
  sidebar entries that act as tabs (#289). `App::stack_depth()` reads
  the stack's depth.
- Drops: `on_drop_error` hears of files dropped on a target that did not
  reach the app, with a `DropError` (`NotAccepted`, `Limits`, `Busy`,
  `Failed`) and the details (#290): on the web a type the target does not
  take, the upload limits, a fifth drop at once, a busy server, an
  unwritable data directory, an upload cut off (a refused drop stops
  uploading at once); natively files that could not be copied. The web
  server says at startup when its data directory is not writable.
- Styles: `group_drop_hover` and `group_drop_hover_named` restyle a drop
  target's descendants while files are over it (#290).

### Fixed

- Web: a WebSocket forwarded by a proxy as `wss`/`ws`
  (`X-Forwarded-Proto`, `Forwarded`) counts as the page's `https`/`http`
  when its origin is checked (#286).
- Bare popovers (`.bare()`) draw no box of their own on every platform,
  like bare dialogs (#287).

### Changed

- Releases publish themselves: pushing a version's tag builds every crate
  on macOS, publishes them to crates.io through trusted publishing, then
  creates the GitHub release; a tag such as `v0.2.0-rc.1` publishes a
  pre-release (RELEASING.md).
- New variants and fields in public types: `Interaction::GroupDropHover`
  (`Interaction::ALL` has 10 states), `UiEvent::DropFailed`,
  `Accessibility::{expanded, controls, controls_node}`,
  `RenderEnvironment::{expanded, collapsed}`, `VNode::{link_root,
  on_drop_error}`. Code that matches these enums exhaustively or builds
  these structs without `..Default::default()` needs the new ones.

## 0.1.1 (2026-11-22)

A fix for the web's first paint and the crates' links.

### Fixed

- Web: shapes (icons drawn with `shape()`) are in the server-rendered first
  paint instead of popping in once `client.js` has loaded (#285). The server
  writes the `<svg>`: in pixels when the shape's width and height are fixed,
  scaled to the box when it has no arcs or insets (strokes keep their width),
  and the client draws it again whenever the box is resized. Paint goes in
  the page's style rules, so a strict `style-src` still allows it.
- The guide and the "Part of Prism" links of `prismsdk-core`, `-push`,
  `-apple-build`, `-build`, `-http`, `-ui` and `-native` point to
  <https://prismsdk.dev/>.

## 0.1.0 (2026-11-22)

The first version of Prism: write a UI once in Rust and ship it as a web app
rendered on the server (HTML and CSS, a small JavaScript client, no
WebAssembly) and as native apps made of each platform's own controls.

**Platforms.** Supported: web, macOS 13+, iOS and iPadOS 17+, Android 10+
(API 29). Previews, to try: Linux (GTK 4), Windows 10 1809+ (WinUI 3, tested
on Windows 11), tvOS 17+, watchOS 10+, visionOS 26+, Wear OS, Android TV and
car screens (CarPlay, Android Auto, Automotive). Rust 1.94 or newer.

**Highlights.**

- One element tree (`div`, `text`, `button`, fields, toggles, sliders,
  pickers, `scroll`, `table`, `VirtualList`, `picture`, `shape`, `scene3d`,
  web views, videos, rich inputs) laid out with flexbox and grid, with
  styles, animations, light and dark appearance, right-to-left languages
  and Dynamic Type.
- Navigation with `Router`: stacks, tabs, sheets, dialogs and popovers, deep
  links and URLs on the web.
- Accessibility on every backend: VoiceOver, TalkBack, Orca, UI Automation
  and screen readers on the web, with labels, roles, headings, live regions
  and tables.
- System services: notifications with actions, remote push (APNs, FCM, Web
  Push, UnifiedPush, WNS) with a server-side sender (`prismsdk-push`),
  widgets and Live Activities, background tasks, secure storage,
  biometrics and passkeys, Sign in with Apple and OAuth/OIDC, device APIs
  (location, motion, camera, photos, share, haptics, clipboard), tray icons
  and app menus.
- `prism-ui`, a component library, plus HTTP (`prismsdk-http`), local
  storage (`prismsdk-db`), typed translations with plurals and
  `prismsdk-island` for interactive islands on web pages.
- The `prism` command (`cargo install prismsdk-cli`): `prism new`, `prism dev`
  with a restart on every save, `prism run` on simulators, emulators and
  devices, `prism build` for each platform (an `.app`, a notarized DMG, an
  APK or App Bundle, a `.deb`, a Windows zip or MSIX, a Docker image) and
  `prism publish` to the App Store and Google Play.

The guide is at <https://prismsdk.dev/>.

### Added

- `scripts/windows/onscreen.ps1`: on-screen proofs of the Windows backend's system UI on a PC someone else may use. It shows nothing unless the session has had no input for 5 minutes, and its captures keep only the app's window and the dialog (the rest painted gray). Cases: a web page's new windows without and with a real click (S41), the same click into Chrome with a throwaway profile, the Share dialog, and Windows' passkey dialog (`-Attended` only: a person must press its Cancel). The counter's Security card gains « Create a passkey » (a demo relying party, nothing kept; `PRISM_PASSKEY_TIMEOUT`), and `PRISM_TRACE` logs when the Share dialog is asked for. Seen on Windows 11 25H2: the Share dialog over the window, and Windows' « Choose where to save your passkey » dialog over it. That dialog runs above the app's integrity level, so UI Automation, keys and clicks from the app never reach it, and neither the request's timeout nor `WebAuthNCancelCurrentOperation` (the app cancels its ceremony when it quits, now logged) closed it: it stayed open after the app quit until a person pressed Cancel (documented under What Windows cannot do).
- A `table` element (#284): `table().column("Time").column(column("Min").align_end()).rows(..)`, columns with headers, then `row()`s of cells holding any element (`cell(..)` for a styled one); per-column alignment, `width` and `min_width`; `separators()` between rows; tabular digits by default (`proportional_digits()` turns them off). On the web it is a real `<table>` with `<thead>` and `<th scope="col">`. Native backends size the columns with the CSS automatic table layout, written once in `prism_native::table` (`column_widths`, a pure function with unit tests): each column's min- and max-content widths measured from its cells, extra room shared in proportion to max-content, and columns that shrink toward min-content when the table is narrow, so a destination wraps and `09:35` never does. Screen readers get table semantics: `NSAccessibilityTableRole` with each cell's column header (macOS), `UIAccessibilityContainerDataTable` (iOS, iPadOS, visionOS), `CollectionInfo` and `CollectionItemInfo` with the column title from API 33 (Android), GTK's `TABLE`, `ROW`, `COLUMN_HEADER` and `CELL` roles with each cell described by its header (Linux), UI Automation's Table, Grid, TableItem and GridItem patterns (Windows), the same data table on tvOS, and header traits with each element's column header in VoiceOver's More Content rotor on watchOS (its SwiftUI host has no data-table API). The counter shows a timetable in its Components tab and at `/components/table`; see the guide's Tables page. `scripts/windows/uia-dump.ps1` prints a table's rows and columns and a cell's row, column and column header.
- `justify_stretch()` (`Justify::Stretch`, #284): a grid's `auto` columns grow to fill its width (CSS `justify-content: stretch`), on the web and natively; a flex container packs its children at the start.
- A native Windows backend, `prismsdk-windows` (`prism_windows`), on WinUI 3: `cargo run` on Windows opens a Win32 window whose content is a XAML island of the Windows App SDK (self-contained: fetched from nuget.org at build time, its 54 MB runtime copied next to the executable, nothing installed), laid out by prism-native's flexbox in `Canvas`es. Prism's controls are WinUI's (`Button`, `TextBox` and `PasswordBox`, `ToggleSwitch`, `CheckBox`, `RadioButton`, `Slider`, `ProgressBar`, `ProgressRing`, `ComboBox`, `CalendarDatePicker`, `TimePicker`, `FontIcon`, `Image`, `Path`), a `SelectorBar` the tab bar, a back button and bar buttons the stack header; buttons and fields the app styles keep the real control with transparent chrome; clickable views hold their children in a chrome-less `Button` (Invoke, keyboard, focus visuals). Styles map to `Border`s (radii, borders, solid, linear, radial and acrylic backgrounds), stroked `Rectangle`s (dashed borders), Composition drop shadows (exact offset, blur, spread), opacity, `CompositeTransform`s and clips; text to `TextBlock`s with `Run`s, measured by XAML itself. Light and dark from the system or the app (`RequestedTheme`), the system accent, Mica; WinUI's UI Automation peers with Prism's labels, hints, heading levels and live regions; Prism key listeners; `MenuFlyout` context menus; per-monitor DPI. `prism_build::link_app()` embeds the manifest that registers the runtime (reg-free WinRT: `windres` for GNU, `/MANIFESTINPUT` for MSVC) and copies the runtime; `prism_build::winappsdk` fetches and caches the packages (`PRISM_WINAPPSDK_DIR`). WinUI renders only in a desktop session: the smoke hooks (`PRISM_PATH`, `PRISM_CLICK` through UI Automation peers, `PRISM_SCROLL`, `PRISM_DELAY`, `PRISM_AFTER`, `PRISM_SNAPSHOT` with `RenderTargetBitmap` and `PrintWindow`, `PRISM_APPEARANCE`, `PRISM_TRACE`) work there, and `scripts/windows-smoke.sh` runs the counter's six routes through `scripts/windows/run-in-session.ps1` when the build has the WinUI runtime (plain SSH otherwise). GNU or MSVC.
- Windows apps with the `prism` command: `prism run windows` and `prism dev windows` build and start the app on a Windows PC (`prism dev` stops it before each rebuild: Windows locks a running executable); `prism build windows` makes `target/windows/<package>-<version>-windows-<arch>.zip`, one folder with the release executable, the Windows App SDK runtime (207 files with `resources.pri`) and the app's assets, nothing else (the counter's: 26.6 MB). It builds on Windows (`--target <triple>` for another target), and from macOS or Linux it cross-compiles for `x86_64-pc-windows-gnu` with MinGW-w64 on `PATH`, or in the new `prism-windows-cross` Docker image (`docker/windows-cross.Dockerfile`) without it. `prism_build::link_app()` now writes the executable's resources itself, as one `.res` file without a resource compiler (`prism_build::winres`): the manifest (now with the app's bundle id and version as its identity, its name, and `asInvoker`), the icon from `[app] icon` (a PNG scaled to 16–256 pixels, or an `.ico`) and `VERSIONINFO` (`ProductName` and `FileDescription` from the name, the version and `[app] build`, `CompanyName` from the new `[windows] publisher`, `Comments` from the description), read from `PRISM_APP_NAME`, `PRISM_VERSION`, `PRISM_BUILD`, `PRISM_BUNDLE_ID`, `PRISM_PUBLISHER`, `PRISM_DESCRIPTION` and `PRISM_ICON` (which `prism` sets) or the `Cargo.toml`. GNU links it through `windres`, MSVC passes it to `link.exe` with `/MANIFEST:NO` (in place of `/MANIFESTINPUT`), `*-gnullvm` to `lld`. Release builds link as Windows programs, so no console window opens (`-Wl,--subsystem,windows`; MSVC `/SUBSYSTEM:WINDOWS /ENTRY:mainCRTStartup`), without `#![windows_subsystem]`; debug builds keep the console for logs; `PRISM_WINDOWS_SUBSYSTEM` chooses. The window takes the executable's icon, and a shipped app finds its pictures in the `assets` folder next to it. `prism new` writes Windows-safe dependency paths in `Cargo.toml` (`C:/…`, no `\\?\` prefix) and a `mise run windows` task. The Windows App SDK packages are fetched safely by two build scripts at once, and the WinUI bindings build without rustfmt (as in Docker images). The `windows` CI job runs `prism build windows` on the counter and checks the zip. arm64 builds and zips with llvm-mingw (`--target aarch64-pc-windows-gnullvm`), not yet run on an arm64 PC. Not yet: an installer; MSVC builds are not run on a PC yet. x86 (`--target i686-pc-windows-gnullvm`, llvm-mingw) builds, zips with the `win-x86` runtime and runs on an x64 PC (WOW64): the smoke's screens and the accessibility check pass. `prism build` links `*-gnullvm` targets with `+crt-static`, so the executable no longer needs llvm-mingw's `libunwind.dll` (it stopped at launch with `0xC0000135`): the flag goes in `CARGO_TARGET_<TRIPLE>_RUSTFLAGS`, after the caller's own value of it, which Cargo joins with the project's `target.<triple>.rustflags` (string or array, tested), so a project's flags are kept; with `RUSTFLAGS` set, nothing is added; the profiler and the tray icon compile for 32-bit x86; `windows-smoke.sh` and `windows-a11y.sh` run a given build (`PRISM_SMOKE_EXE`).
- Windows: animations, overlays, alerts and the rest of the box styles. Transitions of opacity and transforms, enter and exit poses and loops are Composition keyframe animations on each element's facades (`UIElement::StartAnimation`) with the transition's own curve and delay; frames move and resize on each rendered frame; a node with an exit pose stays until it has animated out; nothing animates with « Animation effects » off (`PRISM_REDUCE_MOTION` for one app). Dialogs and sheets have `ContentDialog`'s look and motion (the smoke, a centered panel, its shadow; scale and fade in and out), take the focus (Tab cycles inside) and give it back, and close on Escape or a click on the smoke unless fixed; popovers and placed dialogs are WinUI `Flyout`s on their anchor (light dismiss, Escape, placement from `side` and `align`). `cx.show_alert` is a `ContentDialog`. Inset shadows, conic gradients, borders whose sides differ in color (split at the corners like CSS) and children clipped to rounded corners. The counter's Decorations card shows a conic gradient and a border of four colors; `PRISM_CLICK` takes `scroll:<y>` and `key:Escape|Tab|Enter` steps on Windows.
- The Windows backend's system services: the shell's file, folder and photo pickers and the save dialog (`IFileOpenDialog`, `IFileSaveDialog`), `reveal_path` in Explorer; Windows App SDK app notifications, unpackaged and self-contained (buttons, reply fields, silent, replace and withdraw by id, delays while the app runs), whose clicks reach `on_notification_response` while the app runs or start it; the badge as an overlay on the taskbar button; secure storage in Credential Manager; Windows Hello for `authenticate` and biometric secrets; `prism::oauth` through the default browser and a loopback redirect; tray icons (`Shell_NotifyIcon`, a Segoe Fluent Icons glyph, a popup menu); the app menu as a WinUI `MenuBar` with access keys and keyboard accelerators; alerts as `ContentDialog`s; the clipboard's HTML, RTF, pictures, files and custom types; run-time fonts; background tasks while the app runs. New development hooks: `PRISM_TRAY_MENU`, `PRISM_NOTIFICATIONS_CLEANUP`, `PRISM_BACKGROUND_INTERVAL`.
- Windows: web views, videos, 3D scenes, rich inputs, links and pointer input on WinUI 3. Web views are WinUI's `WebView2` on the Evergreen WebView2 Runtime (part of Windows 11): the app's own environment and data folder, `prismHost` messages both ways, allowed origins, `on_load`, other origins and popups in the default browser, sandboxes without scripts (JavaScript off) or `same-origin` (InPrivate); `prism_build::winappsdk` fetches the `Microsoft.Web.WebView2` package the WinUI package depends on and copies its WinRT DLL next to the app. Videos are `MediaPlayerElement`s (transport controls, poster, autoplay, looping, muted, `on_ended`, the video's ratio). 3D scenes are drawn by the GTK backend's OpenGL renderer, now shared in prism-native (`prism_native::gl3d`, feature `gl3d`), in a WGL OpenGL 3.3 context, frames read back into a XAML `Image`; drag, pan, wheel zoom and taps. Rich inputs are `RichEditBox`es showing the runs as character formats (edits reported as plain text, Return submits). Links in text are `Hyperlink`s (Tab, Enter, UI Automation). Pointer input reaches Prism: `on_pointer_*`, `on_drag` (any button, long press on touch), `draggable` / `on_drop_payload` between views (`locate_drag`, preview, autoscroll), `on_swipe_*`, `on_wheel`, `on_aux_click`, files dropped from Explorer on `on_drop_files`. `PRISM_CLICK` gains `hover:`, `click:`, `drag:`, `swipe:`, `wheel:` and `type:` steps (real mouse and keyboard input), and `PRISM_TRACE_INPUT=1` logs the events sent. The counter gets `/components/media` (a web page talking to the app, a video, a file drop zone). The scene fragment shader declares a default precision for `sampler2DShadow` only on OpenGL ES (desktop GLSL 3.30 allows it for `float` and `int` only; AMD's compiler refused it).
- Windows polish: a clip with only some corners rounded (opposite ones, or three) rounds exactly those (a Direct2D path as the Composition clip), shown by a row of tiles in the counter's Decorations card; run-time fonts load in an app installed in a read-only folder (`C:\Program Files`): they go to `%LOCALAPPDATA%\<app>\prism-fonts`, which XAML reads through the app's resource manager (`Application.ResourceManagerRequested`, with a `ResourceNotFound` fallback); file dialogs name their filters in the app's language (new built-in texts `files`, `images` and `all_files`, overridable with `prism.files`, `prism.images`, `prism.all_files`); the menu bar sits on the window's solid color under the title bar, one band with the tab bar and headers instead of a strip of Mica; a popover closed by Escape gives the focus back to what had it, else to its anchor, so the page no longer scrolls back to its top (opened through UI Automation, nothing had it); one-line text fields are as wide as their placeholder or text (120 at least, like macOS: two short fields share a row) and draw their `leading_icon`. Fixed: with a menu bar, `PRISM_CLICK`'s real clicks, drags between views (hit-testing and the preview) and window-level pop-up menus were 40 DIPs off. `PRISM_CLICK` steps after real input (`click:…|key:Escape`) and the snapshot's `PRISM_AFTER` now wait until that input has played. macOS: `PRISM_APPEARANCE=light|dark` stands in for the system's appearance, and a snapshot with an alert open writes the alert's content to `<snapshot>.alert.png` (macOS shows alerts in a glass window no view draws).
- Windows: a native button (`button(..)`, `ui::button(..).native()`) is no longer wrapped in a second, empty button: no nameless Tab stop before it.
- Windows accessibility: the views Prism draws itself have custom UI Automation peers (COM aggregation of `Button`, `Canvas` and their peers) with their own role, not « button »: links, check boxes, switches (« toggle switch », WinUI's words), tabs and tab bars, lists and list items, radios, sliders, progress bars, combo boxes, groups, images, headings; their states and value through the Toggle, SelectionItem and Value patterns (checked, selected, current, disabled, value), with property-changed events; `Navigation` and `Main` landmarks; containers with a role are elements without a name; the tab bar's items are tab items (WinUI's `SelectorBar` said « SelectorBarItem »); a clickable text is named by its text. Sheets and dialogs are dialog windows (`IsDialog`) named by their label or first heading, and the app's content leaves the UI Automation tree while one is open; a status's new value is announced. High contrast: under a contrast theme the app's colors give way to the theme's system colors as WinUI's controls and Edge's forced colors do (fills, the highlight for colored strong and selected fills, the text color for gray ones, window, button, link and gray text, borders, an outline for cards; no shadows, no Mica), read at start and when the theme changes; `PRISM_HIGH_CONTRAST=0|1|aquatic|desert|dusk|night-sky` stands in for the setting and `PRISM_CLICK=contrast:…` changes it as the app runs. `scripts/windows/uia-dump.ps1 -Json` records what a screen reader reads of each element, and `scripts/windows-a11y.sh` checks the roles, names, states and reading order of the counter's routes, a dialog, a sheet and a popover, with switches and check boxes pressed through UI Automation (`scripts/windows/a11y-expect.json`, `a11y-check.py`).
- Windows: files dragged from Explorer onto `on_drop_files` targets, run on screen (a real Explorer window dragged onto the counter's `/components/media`). As on macOS, the drag's files are read when it enters (under the event's deferral): only a target that accepts one of them (its MIME types or extensions) gets the copy cursor and its `drop_hover` style, a folder never counts; leaves bubbled from the target's children no longer clear it. The drop code moved to its own module (`drops.rs`). The counter's media demo has a second drop zone that takes pictures only.
- Windows: rich inputs, videos, web views and pinches as on macOS and GTK. Rich inputs: chips are pictures embedded in the text (`InsertImage`, drawn with Direct2D and DirectWrite: the Fluent icon and label in a rounded box, at the window's scale), one character the caret jumps over; the editing hooks (#217): copy, cut and paste to the app (`CopyingToClipboard`, `CuttingToClipboard`, `Paste`, with the clipboard's text, HTML and types), Ctrl- and Alt-clicks and every click with the character under the pointer (heard with `handledEventsToo` through a delegate boxed as `IReference<T>`), the selection's rectangle, `write_clipboard` and `insert_text`. Other people's carets (`remote_carets`, #240) and a selection group's highlights (#231) are drawn over text boxes and rich inputs; a mouse drag from one field of a `text_flow` into another selects across them (#264). Videos send their request headers (`Video::header`): range requests of the backend's own through `MediaSource::CreateFromStream`, an `AdaptiveMediaSource` with an `HttpClient` for HLS and DASH. A web view sandboxed without `forms` submits no form: a script in every frame stops submissions and every POST of a document is answered 403 (`WebResourceRequested`). Pinches: `on_magnify` hears two touches (a drag follows the fingers' middle, #172) and Ctrl + wheel (precision touchpads' pinches); 3D scenes zoom with two fingers. The smoke hooks inject touches from a synthetic touch device (`pinch:`, `tswipe:`, `tdrag:`), Ctrl + wheel (`zoom:`), Ctrl-clicks and key chords. The counter: a chip in the rich note, a pinch zoom on the pointer read-out, test hooks for a video behind a header and a sandboxed page (`PRISM_VIDEO_URL`, `PRISM_VIDEO_HEADER`, `PRISM_WEB_URL`, `PRISM_WEB_SANDBOX`). Run on screen: typing, chips, copy, paste, Ctrl-clicks, the selection's rectangle, remote carets, drags and Select all across fields, a video behind a header (and refused without it), forms held back with scripts on and off, touch and Ctrl + wheel pinches, a 3D pinch, touch swipes. Fixes found on screen: `AddHandler` takes `IReference<TDelegate>` under WinRT's `delegate({guid})` id (windows-rs signs delegates otherwise; `boxed.rs` answers the right one); text geometry counts from inside the text host's padding; `on_magnify` elements and orbiting 3D scenes keep their touches (`ManipulationMode::Scale`), swiping rows keep horizontal ones (`System | TranslateX | TranslateRailsX`), so the scroll views around no longer take them. The smoke hooks aim under the window's menu bar (they were a menu bar's height off) and give input only to the app's own window.
- The crates are published as `prismsdk*` on crates.io (`prismsdk`, `prismsdk-cli`, `prismsdk-core`…): `prism`, `prism-cli` and `prism-core` were taken. Code is unchanged (`use prism::prelude::*`, the `prism` command); a dependency reads `prism = { package = "prismsdk", version = "0.1" }`, and `prism new` writes it. The site is <https://prismsdk.dev> and the repository <https://github.com/0xtlt/prismsdk>.
- Windows performance: the counter's components screen (668 nodes) shows its first frame in about 0.55 s in a release build (median 604 ms on a test PC shared with other agents' builds, 551 ms when it was quiet), down from 1.7 s measured the same way (3.2 s in the first measurement, which also created the app's database every run); the UI thread's CPU time until then went from 1404 to 521 ms, the XAML elements from 2685 to 1562, the memory at the first frame from 174 / 192 MB to 105 / 58 MB (working set / private); a debug build from about 2.6 s to 0.65 s. A node is a `Canvas` and its content, with a box, shadows or a children `Canvas` only once it needs them. Text is laid out by XAML once per text and width (its natural size, its narrowest width and a few wrapped sizes kept while its text and font stay the same; a width it fits in is not laid out again; its first baseline read from the same layout): 2675 XAML text layouts became 367. New content of a scroll view outside the window starts `Collapsed` (no templates, layout or drawing) and shows a batch per frame after the first one, at once when scrolled to or focused (`PRISM_DEFER=0` turns it off). OpenGL starts after the first frame. Built-in 3D environments (`Environment::studio()`, `neutral()`, `sunset()`) are prefiltered when a renderer first reads them (`blobs::put_lazy`), not while the app's tree is built: 95 of the tree's 109 ms on Windows, where MinGW's math functions are slow, and most of macOS's too. 3D frames go to a `SwapChainPanel`'s DXGI swap chain on the GPU (the GL context shares Direct3D 11 textures through `WGL_NV_DX_interop2`; the texture kind the driver takes is probed once; a frame costs the UI thread 0.5 to 0.7 ms instead of 2.0 to 2.4 ms reading it back), at XAML's frame rate instead of a 16 ms timer, with the box's background and rounded corners; `PRISM_3D_READBACK=1` or a driver without the extension keeps the read-back. `PRISM_SCALE=1.25` (`1.5`, `2`) runs the app at another scale on any screen (the island's `OverrideScale`) and the `dpi:144` PRISM_CLICK step moves the window to a simulated monitor through `WM_DPICHANGED`: text, borders, icons, pictures and 3D scenes checked at 125, 150 and 200 %. `PRISM_TRACE` now logs each startup phase with the UI thread's CPU time, where the backend's time goes (create, style, measure, text layout, place), XAML's own layout, the first frame (`CompositionTarget::Rendered`), the XAML elements, each update until the frame showing it, 3D frame times and OpenGL's start; `PRISM_PROFILE` samples the UI thread's stacks by phase without administrator rights (`scripts/perf/profile.py` names them); macOS logs the same startup phases. `scripts/windows-perf.sh` and `scripts/macos-perf.sh` time the counter several times and print the medians (`scripts/perf/report.py`); PRISM_CLICK takes `wait:500` between steps.
- Windows: off-screen elements are made after the first frame. The layout is prism-native's, computed from the scene, so a node of a scroll view outside the window (a view, text, control, picture or shape) gets no XAML element until it shows: after the first frame the waiting nodes are made 48 per frame, nearest first, and at once when scrolled to, focused, used as a popover's anchor or reached by a `PRISM_CLICK` step, with their state replayed from the scene (style, text, value, accessibility, pointer input) and their UI Automation peers, so the UI Automation tree is complete once the batches are done (about 1.2 s on the counter's components screen). On that screen, measured in alternating rounds against the build before (16 runs each): XAML elements at the first frame 1562 → 460, styling 30 → 3 ms, the UI thread's CPU time until the first frame 383 → 324 ms, the first frame's median 472 → 460 ms. `PRISM_DEFER=0` makes every element before the first frame. HLS and DASH videos with request headers (`Video::header`, `AdaptiveMediaSource`) and the browser sign-in (`prism::oauth`, loopback redirect) are now run end to end on screen: every playlist, manifest and segment request carries the header and the videos play (and fail with a 401 without it); against a local OpenID Connect provider the app registers, the browser shows the consent page, the app gets the code on its loopback, exchanges it with PKCE and shows the signed-in user. New development hook `PRISM_BROWSER`: a browser's path or command line to open web pages with, in place of the default browser.
- Windows as a test target: a `windows` CI job (MSVC) runs clippy and the workspace tests. `scripts/windows-remote.sh` copies a worktree to a Windows machine over SSH (`PRISM_WINDOWS_HOST`, `PRISM_WINDOWS_DIR`; symbolic links followed, `target` kept) and runs cargo or any command there. In `scripts/windows/`, `run-in-session.ps1` runs a command from SSH in the signed-in user's own desktop (a scheduled task with an interactive logon: no password, no administrator rights) and returns its output and exit code, `uia-dump.ps1` prints a window's UI Automation tree (type, name, automation id, bounds; JSON with `-Json`) or presses an element by name (`-Invoke`), and `capture.ps1` saves a window or the screen as PNG. CONTRIBUTING.md › Windows covers the machine's setup, the repository's symbolic links on Windows, and what works over SSH without a signed-in session.
- Windows finish: a picture's rounded corners clip it, as on macOS (a Composition geometric clip on the `Image`, a Direct2D path when only some corners are rounded). A sticky view is drawn above the content scrolling under it (`Canvas.ZIndex`), so the counter's sticky "Layout" heading no longer overlaps its caption. `tooltip_when_truncated` shows the text's full content as WinUI's tooltip while XAML trims it (`TextBlock::IsTextTrimmed`, followed as the width changes). A view lifted with `z_index` keeps its place in the source for UI Automation and Tab: prism-native tells backends the tree's order (`Widgets::set_reading_order`, after `set_children`'s paint order) and Windows keeps the elements in it, drawing in the paint order through `Canvas.ZIndex` (the counter's overlapping avatars on `/components`). A rich input's text without its own color takes the theme's text color, again when the theme changes: it was RichEdit's automatic color, black on a dark field. `PRISM_CLICK` makes the window the active one before its steps and `chord:` knows `delete` and `backspace`; `scripts/windows-a11y.sh` waits for what a case opens (`waitFor`: the popover's `Popup`) before the dump, so the popover case no longer fails when its flyout opens late.
- Windows: a view with `on_key_down` takes the keyboard focus, as on macOS and Linux: by Tab, by a press and by `cx.focus`. A clickable one through its button (`cx.focus` now reaches it too), any other as its canvas made a Tab stop (`UIElement.IsTabStop`) with the system's focus visual after keyboard input (none after a press), read by UI Automation as a keyboard-focusable group or its own role (`IsKeyboardFocusableCore`).
- Windows: `on_click` gets the click's position in the view, the keys held and its count, as on macOS and Linux: presses on the same view within the system's double-click time (`GetDoubleClickTime`) and rectangle (`SM_CXDOUBLECLK` × `SM_CYDOUBLECLK`) count 2, 3…, read from a clickable view's button press and release (`handledEventsToo`), else from `Tapped` and `DoubleTapped` (a double click's second click was lost there). Enter, Space and UI Automation still click without a position. A rich input's every-click hook (`FieldClick`) counts its clicks the same way.
- Windows: `consume_key_at` gets the caret's edges, as on macOS and Linux: a key declared for an edge is taken from a text field only when the caret is there (it was always taken). Start and end come from the selection, the first and last visual lines (wrapping included) from `TextBox::GetRectFromCharacterIndex` of the caret against the text's start and end, RichEdit's `ITextRange::GetRect` in rich inputs; a one-line field is on both. `KeyEvent::selection` now comes from rich inputs too.
- Windows: key releases match their press, as on macOS and Linux: a release (`on_key_up`) goes to the node its press went to, under the name it went down with (Shift released first no longer turns « A » into « a »), and only when that press was reported; keys still down when the window turns inactive (`WM_ACTIVATE`) are released then, since Windows sends their release to the window that is active.
- Windows: `cx.on_lifecycle` fires, with the phases of macOS and Linux: active while the window is the active one, inactive while another window is (`WM_ACTIVATE`), background while it is minimized (`WM_SIZE`) and when a close is confirmed, before the window goes (the last chance to save).
- Windows: `.cursor(..)` works, and clickable views show a pointing hand, as on macOS and Linux: each view's cursor is a system one (`InputSystemCursor` as its `ProtectedCursor`). Windows has no open or closed hand and no zoom cursor: `Grab` and `Grabbing` show the move arrows, `ZoomIn` and `ZoomOut` the arrow.
- Windows: custom window chrome (`WindowChrome::Custom`), as on macOS: the title bar goes (`AppWindowTitleBar::ExtendsContentIntoTitleBar`) and the content reaches the window's top edge, the system's caption buttons drawn over its top right corner on a transparent background, in the app's theme. The window's top row is its title bar, as in Windows 11's own apps: the menu bar's row, which holds the caption buttons, or without menus a tab bar or navigation header along the top edge, whose trailing end makes room for them; its empty part is a caption region (`InputNonClientPointerSource`: a drag moves the window, a double click maximizes or restores it, a right click opens the window menu) while menus, tabs, Back and bar buttons are passthrough regions and keep their clicks. The regions follow resizes, DPI changes and the bars' items (`SizeChanged`). `.window_drag()` views are caption regions too, what keeps its clicks inside them a passthrough region. `cx.window_controls()` is the caption buttons' frame, `None` with a menu bar (the app's views sit under its row). `WindowControls::insets(width)` gives the room to keep at each end of a title bar on both systems (the traffic lights at the left, the caption buttons at the right). With `PRISM_CHROME=custom` the counter's menu row is its title bar on Windows (macOS: a title bar of its own). `PRISM_CLICK` takes `rclick:` and `#window` (the window's client area: `drag:#window@250,20=-200,120`), and `PRISM_TRACE_INPUT` logs the window's frame; proven on a PC with real input. macOS does the same with its custom chrome: a tab bar or header along the top edge moves the window and takes the title bar's double click (`PRISM_CLICK=dblclick:#window@x,y` in dev builds). There, such a bar keeps room at its end for the traffic lights over it (Back, bar buttons and tabs start after them; none in full screen or lower down, and tabs shrink to fit), and `PRISM_SNAPSHOT` draws the traffic lights over the content. On both systems `cx.window_controls()` is `Some` only while the app's own views are under the window's buttons: `None` while a menu bar, tab bar or navigation header along the top edge holds them, so an app's own title bar never stacks a second one under that bar. It follows navigations, bars shown or hidden, full screen and resizes. On Windows it now follows them in the same turn of the loop, as on macOS (it came one turn later): the layout that moves a bar to or from the caption buttons renders the app again before XAML draws a frame, and at startup the buttons are measured before the window shows; `PRISM_TRACE_CHROME=1` logs each change and the value each first frame after an update shows. On macOS, presses in the title bar strip (its top 28 points) now reach the app's views instead of the invisible title bar, so a `.window_drag()` bar there drags and zooms from its text and empty part alike, and its buttons keep their clicks: Prism's window (a `PrismWindow`, `NSWindow` subclass) sends them to the view under them from `sendEvent:`, with public API only, so Mac App Store builds pass review; AppKit's controls there and the traffic lights keep AppKit's own handling. On the way out of full screen, `window_controls()` is where the lights will be (renders saw them in the full-screen title bar's own window, as tall as the screen).
- Windows catches up with macOS and Linux on 13 gaps. The runtime's layers (toasts) stay above dialogs and sheets. An optional date clears with a ✕ button after the picker, and `date_time()` shows a `CalendarDatePicker` and a `TimePicker` side by side. Text inputs take their keyboard type, capitalization and autocorrect as an input scope with spell checking and text prediction, and their suggestions as a list under the field (the arrows, Enter, Escape). `Router::tint` colors the selected tab's pill and the headers' Back and bar buttons. Closing with unsaved changes asks the app's translated question in a `ContentDialog`. Bundled and embedded fonts register at startup, and each text takes its family's closest file by weight and style. `tabular_nums` sets `Typography.NumeralAlignment`. Colored, thick, double, dotted and dashed underlines are drawn by Prism under each line of the run, since XAML has no underline color. `object_position` places a covering picture around its point. `Scrollbar::Hidden` hides the bars and still scrolls. `device_info().screen` is the monitor showing the window (size, work area, millimeters from its EDID, refresh rate). `tooltip_when_truncated` and text tooltips open again: a restyle no longer turns their hit testing off. `Scroll::on_refresh` is a `RefreshContainer` (a touch pull) plus F5 and Ctrl+R. What Windows cannot do is listed in its page: no all-caps input scope, an Enter key label for Search only, a touch-only pull.
- Widgets drawn ahead (#281): `Widget::ahead(|cx, config| instants)` gives the instants at which a widget's drawing changes (each departure of a « next bus » widget, at most 16), and the render closure is called again for each, with `cx.now()` returning that instant. The system switches drawings on its own while the app sleeps, so a countdown moves on to the next departure instead of stopping at 0:00: WidgetKit timelines with one entry per drawing (`.atEnd`, `.never` without any) on iOS, iPadOS, macOS and visionOS; on Android the latest drawing whose instant has come, and an inexact alarm that draws the widget again at the next one (or when a timer with an `after` text ends). Snapshots of widgets without `ahead` are unchanged; the drawings come in an `"ahead"` list. `App::now`, `WidgetEntry`, `WidgetSnapshot::ahead` / `ahead_for` and `Widget::MAX_AHEAD` are new.
- `timer(end).after(text)` (#281): what a countdown shows once its end has passed, instead of `0:00` (`"Departed"`, `"—"`). In the app on every backend, in iOS and macOS widgets without the app (an entry at the timer's end; timers compare their end with the entry's instant), in Live Activities once they are drawn again, on Android widgets (an alarm draws them again at the end) and Live Updates drawn after the end, the macOS menu bar and the web's live banner. Timers are laid out wide enough for the text. `prism_core::timer_text` and `SNode::placeholder` (`"ph"`) are new.
- macOS: a mouse drag from a text area or rich input of a `text_flow` into another of its fields extends the selection across them (#264), like on the web. Native backends report it with `UiEvent::SelectionAcross` and the runtime answers with the selection and highlights to draw; `Widgets::set_selection_group` tells them which containers are groups.
- `rich_input(..).on_click(..)` and `ClickEvent::text_offset` (#266): every click in a rich input with the character under the pointer, its keys and count, the field still placing its caret, so ⇧-click on an unfocused field can extend a selection from another field of a `text_flow`. The web, macOS and Linux (`edit_events::CLICK`, `UiEvent::FieldClick`).
- `RemoteCaret::label_text_color(color)` (#267): the color of the name tag's text, for a light person color that needs dark text. White by default, as before; every backend.
- Drag and drop between elements (#251): `.draggable(DragPayload::new(kind, data))`, `.on_drop_payload(kind, …)` and `.on_drag_over(kind, …)` (`Enter`, `Move`, `Leave`) on views and scroll views. A snapshot follows the pointer (`DragPreview::None` to leave it out), the innermost target under it that takes the kind gets `drop_hover`, positions are in the target (its content for a scroll view, scrolling included), and a scroll view scrolls when the pointer stays near its edge. The platform's hit-testing finds the target: `elementFromPoint` on the web (pointer events, not HTML5 drag and drop), `hitTest:` on macOS and iOS, `gtk_widget_pick` on Linux, the view tree on Android; nothing on tvOS and watchOS (`Capabilities::drag_and_drop`). `UiEvent::PayloadDrag`, `Patch::SetDragAndDrop`, `prism_native::dnd`, `Runtime::node_named` and `Harness::drag_payload_to` are new.
- One selection across several text fields (#231): `text_flow()` or `div().selection_group()` groups the fields inside into one selection space. ⇧+arrows at a field's edge (and mouse drags on the web) carry the selection into the next or previous field; the field where it started keeps its own selection and the others draw their part highlighted (`Patch::SetSelectionHighlight`: the CSS Custom Highlight API, a layout-manager attribute on macOS, `selectionRects` boxes on iOS, a text tag on GTK). `on_selection_across` hears `SelectionAcross { anchor, head, fields }`; ⌘C / ⌘X go to the first field's `on_copy` / `on_cut` with `CopyEvent::across`; typing, deleting, Return, a cut or a paste over the range go to `on_edit_across` (`EditAcross`) instead of a field; `cx.set_selection_across` and `cx.clear_selection_across` set it. Native backends decide with `Runtime::consumes_key_event`.
- Selection across fields on every native backend (#231, #264). Android: ⇧+arrows, copy and edits over the range (the Java host asks `PrismHost.nativeConsumesKey`), highlights in each field's overlay, `set_selection_across` across fields. Mouse drags from one field into another on Linux, Android (mouse, stylus) and an iPad's pointer, reported like macOS's (`UiEvent::SelectionAcross`); on macOS the drag now keeps the range between two fields, reaches one-line inputs and scrolls. Text from an on-screen keyboard or an input method over the range goes to `on_edit_across` (`Runtime::takes_text_across`, `UiEvent::TypeAcross`), iOS letters from a hardware keyboard included. One-line inputs draw their part on every native backend. `Runtime::selection_group`, `selection_group_fields` and `NativeRenderer::view` are new. Touch selection handles stay inside one field (documented). Fixed: on Android, a text field keeps its focus when a sibling is removed.
- A rich input's editing hooks (#217): `on_selection_rect` (and `track_selection_rect`, `cx.selection_rect(id)`) gives the caret's or the selection's rectangle in the field and in the window, to anchor a toolbar or a link popover above the selected words; `on_copy` / `on_cut` hear ⌘C / ⌘X (keyboard, Edit menu, context menu) with the selected runs and write the app's `ClipboardContent` (plain text plus HTML, Markdown and custom types, one item); `on_paste` hears ⌘V with the clipboard's text, HTML and types, and `prevent_default()` takes it over; `on_span_click` hears modifier-clicks on a span while editing (⌘-click a link). Web, macOS, iOS, Linux and Android (no modifier-clicks on touch screens; nothing on tvOS and watchOS). `Patch::SetEditEvents`, `WriteClipboard`, `InsertText` and `UiEvent::SelectionRect`, `Copy`, `Paste`, `SpanClick` are new; `PRISM_KEYS` on macOS takes modifiers (`cmd+c`).
- `.remote_carets(..)` (#240) on text inputs, text areas and rich inputs: other people's carets and selections drawn inside the field being edited, for collaborative documents. Each `RemoteCaret { anchor, head, color, label, idle }` is a thin caret in its color with a name tag and a tinted selection, laid out with the field's own text (wrapping and scrolling included), above it, never editable nor hit; updating them leaves the user's caret, selection and IME composition alone. `idle` fades the name out. The web draws a layer beside the field, macOS a view from the layout manager, iOS, iPadOS and visionOS from `UITextInput`, Android in the `EditText`'s overlay, Linux over text areas and rich inputs (not a one-line `GtkEntry`); tvOS, watchOS and secure fields show none. `Patch::SetRemoteCarets` and `Widgets::set_remote_carets` are new.
- `div().drag_after_long_press(delay)` (#243): on touch screens an `on_drag` starts only once the press is held still for `delay`, with the platform's lift haptic, then wins over the scroll views around it, so cards in a horizontally scrolling board can be dragged while a swipe still scrolls. Mice, trackpads and an iPad's pointer drag on movement as before. iOS, iPadOS and visionOS (`UILongPressGestureRecognizer`), Android, the web (touch and pen pointers, `touch-action` until the lift) and Linux touchscreens; nothing on macOS, tvOS and watchOS. `Patch::SetDragDelay` and `Widgets::set_drag_delay` are new.
- `.keyboard_toolbar(bar)` (#242) on text inputs, rich inputs, text areas and containers, and `keyboard_toolbar(bar)`: a bar of the app's own above the on-screen keyboard while a field it serves has the focus; a field's own bar wins over its container's. Taps on it keep the field focused. The field's `inputAccessoryView` on iOS and iPadOS, a bar docked above the IME on Android, a bar on the visual viewport's bottom edge on touch-screen browsers; left out on macOS, Linux, tvOS, watchOS and visionOS. `OverlayKind::Keyboard`, `OverlayConfig::field_only`, `Widgets::set_keyboard_bar` and the remote `kbar` op are new.
- Menus (#247): `MenuItem::shortcut_hint("cmd+b")` shows a shortcut without binding it, so the key keeps going to the focused view (⌘B in a field, ⌘= for a view's own zoom): a key equivalent only while the menu is open on macOS, GTK's `accel` attribute on Linux, left out of the page's shortcuts and written in its menus on the web. Pop-up menus: `div().on_click_menu(menu)` and `button(…).on_click_menu(menu)` open the platform's menu below the element, `cx.show_menu_on(id, menu, at)` at a point in an element (or below it), `cx.show_menu(menu, at)` in the window: `NSMenu` pop-ups on macOS, `UIMenu` from a button's primary action on iOS, iPadOS and visionOS (17.4+), `PopupMenu` on Android, `GtkPopoverMenu` on Linux, the page's menu on the web; nothing on tvOS and watchOS. `MenuItemSpec::Action::shortcut_hint`, `Patch::ShowMenu` and `Widgets::show_menu` are new. The web's menus show their items' shortcuts.
- Span underlines and inline icons (#221): `span(..).underline_color(c)` and `.underline_style(UnderlineStyle::Thick | Double | Dotted | Dashed)` underline a run in its own color and line, whatever the text's color, in read-only text and in a rich input. `Span::icon(name, points)` sizes an inline icon; in a `rich_input` an icon is now one character of the value (U+FFFC, like a chip), drawn as its SF Symbol on macOS and its glyph on the web. Web: `text-decoration-color` / `-style` / `-thickness`; Apple: `NSUnderlineColorAttributeName` and `NSUnderlineStyle` (dotted and dashed are patterns); watchOS: `Text.LineStyle` (no thick or double); Android: a span draws the line; Linux: Pango's underline color, double, and its error line for dotted (thick and dashed are single).
- `cx.read_clipboard_item()` and `cx.write_clipboard(ClipboardItem)` (#246): the clipboard in all its types, not only text. `ClipboardItem::new().text(…).html(…).image(bytes, "image/png").custom(type, data)`, read back with `types()`, `get(type)`, `as_text`, `as_html`, `as_image` (`ClipboardImage { bytes, mime }`) and `files()`. macOS turns TIFF (screenshots) and copied image files into images; iOS uses `UIPasteboard`; the web reads with `navigator.clipboard.read()` (uploaded like picked files) and writes text, HTML, PNG and `web …` custom formats; Android puts text, HTML and an image behind a content URI; Linux reads and writes GDK formats. tvOS and watchOS have no clipboard. `DeviceCall::ReadClipboardItem`, `WriteClipboard` and `DeviceValue::Clipboard` are new.
- Drags for shape editors (#223): `DragEvent::modifiers` follows the keys held now (every move, and a `Move` when ⇧, ⌥, ⌘ or ⌃ alone change mid-drag, on the web and macOS); new cursors `ResizeN/S/E/W`, the corners `ResizeNe/Nw/Se/Sw`, the diagonals `ResizeNeSw`, `ResizeNwSe`, and `ZoomIn` / `ZoomOut`. On the web, the click a drag's release makes is swallowed before any handler until the next press (it was cleared after one tick).
- `prism::http` (#250): `Request::secret_header`, `redirects(Redirects::None | SameOrigin | Follow(n))`, `max_body(bytes)`, and streamed bodies (`send_stream_blocking`, `FetchStream::fetch_stream`, with `Progress`). Prism now follows redirects itself (at most 10 by default) and drops `Authorization`, `Proxy-Authorization`, `Cookie` and secret headers when one leaves the request's origin.
- `cx.pick_folder(PickFolder)` (#225): the user picks a folder and the app gets its files with their tree (`PickedFolder { name, path, files }`, each file named by its path in the folder, hidden ones left out). macOS and Linux read them in place; iOS, iPadOS and Android copy them; the web uploads them (`webkitdirectory`). Backends answer the folder first (`FOLDER_MIME`), then its files.
- `cx.set_badge(Option<u32>)` (#239): the app icon's number set on its own, without a notification. The Dock tile on macOS, `setBadgeCount` on iOS, iPadOS, tvOS and visionOS, the Badging API on the web; nothing on Android, Linux and watchOS. Outside an app bundle (`cargo run` on macOS), the notification permission now answers `Unavailable` and notification calls do nothing, instead of throwing.
- An app-level locale (#229): `Application::locale("fr-FR")` and `supported_locales([...])`, and the same on `WebApp` for mounted apps. A one-language app keeps its language whatever the browser's `Accept-Language` or the system's: `<html lang>` and `dir` in the first paint, built-in texts, `cx.locale()`. With supported locales, the user's language when the app has it, else the closest, else the first. `PRISM_LOCALE` still wins. `Locale::best_match` and `App::apply_app_locale` are new.
- `.z_index(n)` (#232): paint order among siblings, like CSS `z-index`. A handle or badge overflowing into the next element is drawn (and clicked) above it. The web writes `z-index`; native backends order the views by each child's value or the highest one inside it, up to a node that sets its own (the layout is unchanged; stacks and tab bars keep their order). `Style::z_index`.
- `text_area().rows(n)` (#235): a text area's minimum height in lines, three by default. `rows(1)` starts like a one-line field and grows with its text (up to `max_h`): a title, a table cell. The web sets `rows` and the minimum height; native backends measure that many lines (`setMinLines` on Android). `InputConfig::rows` and `min_rows` are new.
- `on_focus_change(|e: &FocusEvent, cx| …)` (#224) on text fields, rich inputs, text areas and views: the node took (`e.focused`) or lost the keyboard focus, for « commit on blur » in inspectors and inline renames. The web reports `focusin` / `focusout`; native backends report what their shared focus tracking sees (`NativeRenderer::take_focus_changes`). `UiEvent::FocusChange` is new.
- Auto margins (#228): `m_auto()`, `mx_auto()`, `my_auto()`, `mt_auto()`, `mr_auto()`, `mb_auto()`, `ml_auto()` take the free space like CSS `margin: auto`: a centered column without an `items_center()` parent, the last item of a row pushed to the end without a spacer, a footer at the bottom. `Style::margin_auto`; CSS `auto` on the web, taffy's auto margins natively.
- `button(label).link(path)` and `ui::button(label).link(path)` (#230): a button that navigates. Natively it stays the platform's (or the theme's) button; on the web it is an `<a href>` with a button's look, so Cmd/Ctrl-click, a middle click and « Open in new tab » work, before the script runs too. It is announced as a link.
- `picture(src).object_position(x, y)` (#218) crops a filled picture around a focal point (fractions, the center by default), like CSS `object-position`, on every backend: `object-position` on the web, `contentsRect` on macOS, iOS, iPadOS, tvOS and visionOS, a matrix on Android, a covering paintable on Linux. `prism_core::cover_crop` and `picture_position` compute it.
- Tooltips (#216): `.tooltip(text)` on any element shows the platform's own (`toolTip` on macOS, `UIToolTipInteraction` on iPadOS and visionOS, `title` on the web, GTK tooltips, `setTooltipText` on Android). `.tooltip_when_truncated()` on a text shows its full content only while it is truncated or clamped, checked by the platform (macOS, the web, Linux, Android). They travel in `Accessibility::tooltip` (`Tooltip::Text`, `Tooltip::WhenTruncated`).
- `div().on_aux_click(…)` (#215) hears clicks with the middle, back and forward mouse buttons, and `ClickEvent::button` says which (`PointerButton::Middle`, `BACK`, `FORWARD`; `Primary` for `on_click`). Middle-click a tab to close it, a row to open it in a new tab. Web (`auxclick`, without autoscroll or history navigation over the element), macOS and Linux. `UiEvent::AuxClick` and `pointer_mask::AUX_CLICK` are new.
- Named hover groups (#212): `.group_named("row")` with `.group_hover_named("row", …)`, `.group_active_named` and `.group_focus_within_named`. A refinement follows the ancestor group it names, whatever groups sit in between, so nested hover-revealed controls need no app-side hover state. Backends resolve them like `group_hover`: the web and Linux with `[data-gn]` / class selectors, other platforms with the shared renderer. Layout changes in hover refinements (a width, a padding) apply on every backend. `Style::group_name`, `Style::is_group`, `States::resolved_named` and `Style::interacted_in` are new.
- `div().consume_key_at(key, CaretEdge::FirstLine)` (#211) takes a key from the focused text field only while its caret is at that edge: the start, the end, the first or the last visual line (wrapping included). Every platform checks the caret when the key is pressed, before the field acts, the web included. `KeyEvent::edges` (`CaretEdges`) and `KeyEvent::at(edge)` tell the handler the same, so a grid or a block editor moves between fields at the right moment without tracking the caret. `Runtime::consumes_key` takes the caret's edges.
- `cx.save_file(SaveFile::bytes(data, "report.pdf"))` and `SaveFile::path(…)` (#210) let the user keep a file the app holds, with the platform's UI: `NSSavePanel` on macOS, the GTK save dialog on Linux, "Save to Files" on iOS, iPadOS and visionOS, `ACTION_CREATE_DOCUMENT` on Android, and a download on the web (the server keeps the bytes behind a session-bound URL, so no token reaches the page). It answers `Option<SavedFile>`, with the path where the platform gives it. `cx.reveal_path(path)` shows a file selected in the Finder or the Linux file manager.
- `video(src)` (#207) plays a video with the platform's player and controls: `AVPlayerView` on macOS, `AVPlayerViewController` on iOS, iPadOS, tvOS and visionOS, `VideoView` on Android, `GtkVideo` on Linux and `<video>` on the web (nothing on watchOS). It takes `header` (sent natively except on Linux; on the web the server streams it by byte ranges through `/_prism/media/<id>`), `controls`, `autoplay`, `looping`, `muted`, `poster` (web; native players show the first frame) and `on_ended`. Given a width, its height follows the video's ratio once known (16:9 before).
- `cx.webauthn_autofill(options)` (#203) offers the app's passkeys in the suggestions of a username field (conditional mediation), answering when the user picks one. On the web it uses `mediation: "conditional"`; on iOS, iPadOS and visionOS, `performAutoFillAssistedRequests`. Any other ceremony replaces it. Mark the field with `.attr("autocomplete", "username webauthn")`; on iOS that `autocomplete` now also sets the field's `textContentType` (username, passwords, one-time code, e-mail…). It answers `Unavailable` on macOS and Android.
- `ReorderList::with_options` and `ReorderOptions` (#201). Rows can be as tall as their content: the list measures them after layout (#192) and places the drop slot and the rows making room by their real heights. The app can give its own handle (`.handle(…)`, Prism keeps the role, label, keys and drag) and put it at the leading edge. `ReorderList::new` is unchanged: fixed heights, three bars at the trailing edge.
- Web: `div().on_drop_progress(…)` (#199) follows the upload of files dropped on an `on_drop_files` target. It reports `DropProgress { sent, total }` (`fraction()`) from 0 to the last byte, just before the files arrive, like `pick_files_with_progress` does for picked files. Natively dropped files are on the device already, so it is not called.
- `cx.scroll_by(id, dx, dy)` and `cx.scroll_to(id, x, y, animated)` (#193) move a `scroll()` with that `.id()` from the app, kept within its content: auto-scroll while dragging near an edge, back to the top. `cx.scroll_offset(id)` reads the last offset it reported. Available on the web, macOS, iOS, Linux and Android.
- Element frames (#192): `div().on_layout(…)` hears an element's frame after each layout that moved or resized it, and `cx.frame_of(id)` reads the last one of an element with an `.id()` that reports it (`on_layout` or `track_layout()`). This covers hit-testing rows and drop targets without estimating heights. Frames are `Rect` (with `contains`, `intersects`, `center` and `to_layout`), in points, in the window's layout coordinates before scrolling, so distances inside a `scroll()` hold at any offset. Native backends report them from Prism's layout; the web reports them from the page's.
- `Prism.toml` `[ios] associated_domains = ["webcredentials:example.com", "applinks:example.com"]` (#191): the `com.apple.developer.associated-domains` entitlement that passkeys of the app's own relying party and universal links need. Entries are checked (known services, a host, an optional `?mode=`). It goes into device builds when the provisioning profile grants the capability (with a note otherwise), into simulator builds, and into macOS App Store builds.
- `DeviceError::Platform { name, message }` and `DeviceError::name()` (#189): device calls keep the platform's error name, using the web's `DOMException` names everywhere. Registering a passkey the device already holds gives `Some("InvalidStateError")` on the web, Apple platforms (`matchedExcludedCredential`) and Android (its DOM exception type), so apps no longer search the message. A request no authenticator there can do is `Some("NotSupportedError")` on the web, Android and Windows, never `Unavailable`, which says the device has no passkeys. Backends send a name as `name` U+001F message (`DeviceError::from_message`).
- `Prism.toml` `[network] allow_http_dev` (#187): plain-HTTP hosts for development builds only. `prism run` and `prism dev` add them to `allow_http`'s ATS exceptions and Android cleartext permissions; release builds and store uploads leave them out.
- `consume_keys(&[...])` on an element with `on_key_down` (#181): while the focus is in it, those keys reach its handlers and not the focused text field, the page or the window, so ↑ ↓ ↵ drive a suggestion menu under a field without moving its caret or submitting. Declared at render, it works on the web (a capture-phase `preventDefault`), macOS (the key monitor drops the event), iOS / iPadOS / visionOS (priority `UIKeyCommand`s), Android and Linux.
- Lists the user reorders (#168): `ReorderList::new(cx, keys, row_height, render_row, on_move)` gives each fixed-height row a handle. Dragging the handle lifts the row while the others make room; with the focus on the handle, ↑ / ↓ / Home / End move the row. On drop the list takes its new order and calls `on_move(from, to, app)` so the app reorders its data. The rest of each row keeps its own clicks and scrolling. Handles are buttons for screen readers, with the row's position and a hint in 12 languages (`prism.reorder`, `prism.reorder_hint`, `prism.reorder_position`). Built on Prism's drag and key events, so it works on the web, macOS, iOS, iPadOS, visionOS, Android and Linux (keys only on tvOS).
- Dialogs without Prism's box (#166): `Overlay::bare()` takes away the dialog's background, border, corners, shadow and entry animation, so an app-styled panel (a picker beside its anchor with `.side(…)` and `Backdrop::Clear`) is the whole dialog, still modal. Web: no box styles on the `<dialog>`; macOS: a borderless, transparent panel that takes the keys and holds the presses behind it, instead of a sheet or an `NSPopover`; iOS, iPadOS, visionOS, tvOS: a clear card without cross-fade; Android: a transparent dialog window without animation; Linux: a `GtkPopover` without arrow or background, or an undecorated window. `OverlayConfig::bare` (`bare` in the config string) for backends.
- The whole menu bar, defined by the app (#161): `cx.set_menu_bar(menus)` replaces Prism's English application, Edit and Go menus, and `MenuItem::system(SystemAction::…)` puts the platform's own actions (Undo, Redo, Cut, Copy, Paste, Paste and Match Style, Delete, Select All, Minimize, Zoom, full screen, Close, Hide, Hide Others, Show All, About, Services, Quit) next to the app's items, with their usual shortcuts and a `.title(…)` to localize them. On macOS system items go along the responder chain and follow the focus; app items keep their state (`validateMenuItem:`), take their shortcut even from a focused field (an app's own Undo on ⌘Z), and a menu with Minimize is the Window menu. Prism's default Edit menu gains Redo (⇧⌘Z). Web: the shortcuts of the app's menus are taken before the browser (`preventDefault`) and run the item. Linux: the full bar is the GTK menu bar, without system items. `PlatformRequest::SetMenuBar`, `MenuItemSpec::System` for backends.
- Leaving with unsaved changes (#158): `cx.set_unsaved_changes(true)` makes closing the page or the window ask first (`has_unsaved_changes()` reads it). Web: `beforeunload` (the browser's own question), with the live session's first tree so a reconnect drops a stale state; macOS: the close button's dot, and closing the window or quitting asks « Leave without saving? » (Discard Changes / Cancel); Linux: closing the window asks. Phones, tablets and watches close apps themselves: nothing there. New built-in texts `unsaved_title`, `unsaved_message`, `discard` and `cancel` in the twelve built-in languages. `PlatformRequest::SetUnsavedChanges` for backends.
- Click keys and count, key releases (#159): `ClickEvent::modifiers` (⌘ / Ctrl, ⇧, ⌥ held) and `ClickEvent::count` (2 on a double click, 3 on a triple click, as the system counts them; `is_double()`) for ⌘-click, ⇧-click and double-click; `on_key_up` / `on_global_key_up` hear key releases with the rules of their `on_key_down` twins, for « hold a key » interactions (hold Space to pan). A release comes only for a reported press, and keys still held when the window loses the focus are released then. Web: `MouseEvent` keys and `detail`, `keyup`, `blur`; macOS: `modifierFlags` and `clickCount` on Prism views, `keyUp:` and the key monitor, `windowDidResignKey:`. `UiEvent::ClickWith` / `KeyUp`, `Modifiers::NONE` for backends; other backends report no keys, count 1 and no releases yet.
- How an overlay was closed (#156): `BackEvent::reason` (`DismissReason::Escape`, `Outside`, `Swipe`, `Back`, `Unknown`) tells `on_dismiss` whether the user pressed Escape, clicked or tapped outside, swiped a sheet down or used Back, so a picker can cancel on Escape and keep its choice on a click outside. Reported by the web (the dialog's `cancel`, the backdrop, a popover's light dismiss right after Escape or not), macOS (the event closing a popover, Escape in a sheet), iOS, iPadOS, visionOS and tvOS (Escape or Menu, the backdrop, an iPad popover, a sheet swiped down, the Close button), Android (Escape, a touch outside, Back) and Linux (Escape, a click outside a popover, the close button). `UiEvent::Dismiss` for backends; `BackEvent` and `DismissReason` are in the prelude.
- The app's title (#154): `cx.set_title("Orders")` titles the page or the window for an app that does its own navigation, over the top screen's title; `cx.clear_title()` goes back, `cx.title()` reads it. Web: the tab's « Orders · App », in the first paint, with the live session's first tree (a reconnected page drops a stale one) and on change; macOS and Linux: the window's title; iOS, iPadOS, visionOS, tvOS: the scenes' title; Android: the activity's. `PlatformRequest::SetTitle` for backends.
- Plain-HTTP hosts in `Prism.toml` (#152): `[network] allow_http = ["preview.localhost"]` lets the app reach these hosts and their subdomains over `http://`, such as a development server shown in a `web_view` on an iPhone. App Transport Security exceptions (`NSExceptionDomains`, `NSAllowsLocalNetworking` for IP addresses) in the Apple `Info.plist`, a `network_security_config` with cleartext traffic for those hosts on Android. Hosts are checked (no scheme, port or path).
- Passkeys of the app's own relying party (#94): `cx.webauthn_create(options_json)` and `cx.webauthn_get(options_json)` run a WebAuthn ceremony with the options the app's server produced (`PublicKeyCredentialCreationOptionsJSON` / `RequestOptionsJSON`, a `{"publicKey": …}` wrapper accepted) and answer the credential as JSON (`RegistrationResponseJSON` / `AuthenticationResponseJSON`) for the server to verify: passkey sign-in, adding a passkey to an account. Web: `navigator.credentials` in the page (the JSON forms converted where the browser lacks `parse…FromJSON` / `toJSON`; a « Continue » tap when it wants a fresh gesture); Apple platforms but the watch: `ASAuthorizationPlatformPublicKeyCredentialProvider` (the relying party's `webcredentials:` associated domain); Android 14+: the Credential Manager. `Capabilities::passkeys`, `DeviceCall::WebAuthn` / `WebAuthnRequest` for backends. `cx.authenticate` keeps Prism's own credential.
- Drags with the middle or secondary mouse button (#136): `.drag_buttons(&[PointerButton::Middle])` makes the middle button (wheel pressed) start an `on_drag` instead of the primary one, so a canvas pans with it while the left button selects; `DragEvent::button` (`PointerButton::Primary`, `Middle`, `Secondary`, `Other(n)`) and `DragEvent::modifiers` (keys held at the start) say how it drags. Web: pointer events of that button, with middle-click autoscroll, paste and « open in a new tab » prevented over the element and no context menu after a secondary-button drag; macOS: `otherMouse…` and `rightMouse…` events (a right press that does not move still opens the menu). Other backends drag with the primary button only. `pointer_mask::DRAG_MIDDLE` / `DRAG_SECONDARY`; the remote hosts' `drag` events carry `button` and `mods`.
- Wheel and pinch events on any element (#133), for panning and zooming an infinite canvas: `div().on_wheel(handler)` gets `WheelEvent { delta, position, modifiers, phase, precise }` (points, positive `y` toward the end like the web's `deltaY`; `GesturePhase` `Start`, `Move`, `End`, `Momentum` for trackpads) and `.on_magnify(handler)` gets `MagnifyEvent { scale, position, phase }` (relative to the previous event). The element takes the events (nothing behind it scrolls, the page does not zoom). Web: a non-passive `wheel` listener, one report per animation frame, Ctrl + wheel and Safari's gesture events as pinches; macOS: `scrollWheel:` (precise deltas, phases, momentum) and `magnifyWithEvent:`. Other backends do not report them yet. `pointer_mask::WHEEL` / `MAGNIFY`, `UiEvent::Wheel` / `Magnify`, the `wheel` and `magnify` events of remote hosts for backends.
- Drop targets for files (#111): `div().on_drop_files(types, handler)` receives files dragged from the system and dropped on the element (`FileDropEvent { files }`, only those `types` accept, as for `PickFiles`), and `.drop_hover(|s| …)` styles it while accepted files are over it (`Interaction::DropHover`). Web: `dragover` / `drop`, the files uploaded in chunks like picked files (`drop_start`, `upload_chunk`, `drop_end`; the server takes them only for its drop targets and types they accept, at most four drops at once), and a drop elsewhere on the page no longer opens the file; macOS: `NSDraggingDestination` on Prism views; iOS, iPadOS, visionOS: `UIDropInteraction` (`PrismDrop.swift`); Android: `View.OnDragListener` with the drop's permissions; Linux: `GtkDropTarget` and `:drop(active)`. Dropped files are copied where the app can read them, off the main thread. `Capabilities::drop_files` (none on tvOS and watchOS), `Patch::SetDropTarget`, `UiEvent::DropFiles`, `Widgets::set_drop_target`, the remote `drop` op and event for backends; `Harness::drop_files` in prism-testing.
- Overlay and hidden scroll bars (#132): `scroll().scrollbar(Scrollbar::Overlay)` keeps thin bars over the content that never take room, `Scrollbar::Hidden` removes them (the view still scrolls), `Scrollbar::Auto` is today's behavior (`Style::scrollbar`). macOS: an overlay `NSScrollerStyle` kept when « Show scroll bars » changes or a mouse is plugged in (a `PrismScrollView` subclass), or no scrollers; web: `data-prism-scrollbar` styled by `prism.css` (`scrollbar-width: thin` shown on hover and focus, or `none`); iOS, iPadOS, visionOS, watchOS and Android: indicators off when hidden (theirs are already overlay); Linux: GTK overlay scrolling, `PolicyType::External` when hidden. The remote hosts' style carries `scrollbar`.
- Upload progress for picked files on the web (#112): `cx.pick_files_with_progress(options, |p, cx| …)` and `ui::file_input(…).on_progress(…)` report `UploadProgress { upload, sent, total }` (`fraction()`) while the browser sends the chosen files, and `cx.cancel_upload(p.upload)` stops it (the pick then gives no files and what arrived is deleted). The browser now sends picked files and photos in chunks of 256 KB (`upload_start`, `upload_chunk`), waiting while the socket is busy, instead of one message holding every file: large files no longer hit the socket's message limit, and the server writes each chunk to disk as it arrives. The server takes no more than each file's announced size, and only for a pick call the app made; an unfinished upload is deleted with its session. `DeviceEvent::Upload` for backends. Natively the files are on the device: no progress.
- Pretty and balanced line breaking (#95): `.text_pretty()` keeps a lone word off the last line of a subtitle or a short paragraph, `.text_balance()` gives a heading lines of about the same length (`.text_wrap(TextWrap::Pretty | Balance)`, `Style::text_wrap`, also in responsive refinements). Web: CSS `text-wrap`; macOS, iOS, iPadOS, tvOS, visionOS: `lineBreakStrategy = .pushOut` on the label and its paragraph style for both (Apple has no balanced breaking); Android: `BREAK_STRATEGY_BALANCED` for balance, the high-quality breaker for pretty. Linux, watchOS and widgets break as usual. The remote hosts' style carries `tw`.
- Anchored dialogs without a dim (#79): `dialog(panel).side(PopoverSide::Trailing)` places a modal dialog beside its anchor (the element that contains it; on the opposite side without room, centered when neither side has room, as on phones) and keeps its modality (focus inside, the rest out of reach, Escape and a click outside close it), lined up as `.align(…)` says, as for popovers (#113). `.backdrop(Backdrop::Clear | Dim | Blur)` sets what a sheet or dialog shows over the page (the web's blur is now configurable), and `.size(w, h)` the content area (sheets keep the platform's on native). Web: the `<dialog>` is still `showModal`ed, positioned beside the anchor, `::backdrop` per option. macOS: a placed dialog is an `NSPopover` on that edge (no backdrop on macOS). iOS / iPadOS / visionOS: the dialog's card beside the anchor over a clear, dimmed or blurred backdrop, a clear sheet leaves the window undimmed. Linux: a placed dialog is a `GtkPopover` at that position. Android: the dialog's window beside the anchor, the dim off or with a blur (Android 12+). `OverlayConfig::placed` / `backdrop` / `width` / `height` (`placed`, `backdrop=`, `size=WxH`), `OverlayConfig::anchored_side`, `PopoverSide::is_right` and `controls::place_beside` (the side-and-alignment rule, with `None` when neither side has room).
- Popover side and alignment (#113): `popover(content).side(PopoverSide::Trailing)` opens a popover beside its anchor (`Bottom` by default, `Top`, `Trailing`, `Leading`; trailing is the right in left-to-right layouts), on the opposite side when the window has no room there, and `.align(PopoverAlign::End)` lines it up with the anchor's start (the default), center or end: a submenu beside its item with tops lined up, a panel lined up with the trailing edge of a toolbar button. Nested popovers and `light_dismiss(false)` / `take_focus(false)` keep working. Web: placed by the client; macOS: the `NSPopover`'s preferred edge and the part of the anchor it points at, the `take_focus(false)` panel placed exactly; iPadOS / visionOS: `permittedArrowDirections` and `sourceRect`; Linux: `GtkPopover::set_position` / `set_pointing_to`; Android: the `PopupWindow` placed beside or above its anchor. Arrowed popovers (macOS, iPadOS, Linux) line up exactly when they are no longer than the anchor along that edge, and are centered on it otherwise. `OverlayConfig::side` / `align` (`side=…`, `align=…` in the config string), `PopoverAlign::arrow_span` for backends.
- Cookies from a handler (#91): `cx.set_cookie(Cookie::new(name, value))` sets a cookie in the browser from a handler running over the live socket (signing in or out). `Cookie` defaults to `HttpOnly; Secure; SameSite=Lax; Path=/` (`.path`, `.domain`, `.max_age`, `.http_only`, `.secure`, `.same_site`, `Cookie::remove`), and `Cookie::check` refuses what a browser would drop. The server gives each cookie a one-time ticket (128 bits, 60 s, bound to the browser's `prism_sid`); the page posts it in an `X-Prism-Cookie` header to `POST <base>/_prism/cookie` (same origin only), whose `204` sets it, one cookie after the other. `AsyncApp::set_cookie(..).await` answers once the browser has it, and `cx.request()` shows it from then on; a `Secure` cookie on a plain `http://` page (not `localhost`) or a `Domain` that does not cover the page fails instead of being lost. `cx.reconnect()` opens a new live session at once, without the « Reconnecting… » banner, after the cookies set before it, so the host's middleware sees them. Native apps answer `set_cookie` with `Unavailable` and ignore `reconnect`. `DeviceCall::SetCookie` / `Reconnect`, `WebRequest::apply_cookie` for backends.
- Editable rich text (#115): `rich_input(spans)` edits the runs of a `rich_text` in place, with `on_input` (a `RichInputEvent { value, spans }`: each span keeps the style, link, handler and key of the run it came from), `placeholder`, `on_submit` (Enter; Shift+Enter inserts a line break), `on_select` / `.selection` in characters (#86), `keyboard`, `capitalize`, `autocorrect`, `return_key`, `id`, `autofocus`, `on_key_down` and text styles (`text_size`, `font_family`, `text_color`…) like `text_input`. `chip(key, label).chip_icon(..)` is an atomic inline object, one `OBJECT_CHAR` (U+FFFC) of the value that the caret, the selection and Backspace treat as one character (a read-only `rich_text` shows its label). Typed text takes the style of the run before the caret (not a chip's or a link's), pastes are plain text, the app's runs follow the echo rules of text fields (#73). `split_spans`, `restyle_spans`, `splice_spans`, `normalize_spans`, `spans_text` and `Span` getters (`text()`, `is_bold()`, `chip_key()`…) edit and read runs by characters. Web: a `contenteditable` drawn from a model the client owns (`beforeinput`, IME compositions read back when they end, plain copy and paste, undo/redo); macOS: `NSTextView` attributes with `NSTextAttachment` chips. iOS, GTK, Android and watchOS show a plain multi-line field of the value for now (styles not drawn, a chip as U+FFFC), whose edits still come back as spans keeping the styles around them. New `NodeKind::RichInput`, `Patch::SetRichValue`, `UiEvent::RichInput`, `Widgets::rich_input` / `set_rich_value`, web op `rich` and `input` events with `runs`.
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
- Web views (#62): `web_view(url)` shows another web page inside the app: an `<iframe>` on the web, a `WKWebView` on macOS, iOS, iPadOS and visionOS (through the Swift bridge), an `android.webkit.WebView` on Android. `.on_message(…)` gets what the page posts (`parent.postMessage` in a frame, `prismHost.postMessage` natively; `WebMessageEvent::data::<T>()` and its origin), `cx.post_web_message(id, &value)` sends it a `message` event (queued until the page loads), `cx.reload_web_view(id)` reloads it, `.on_load(…)` reports each loaded page. Only the allowed origins (`.allow_origin(…)`, by default the URL's own) are heard and posted to; native views open links to other origins in the system browser. `.sandbox(WebSandbox::new().scripts()…)` maps to the iframe's `sandbox` and to the native settings. tvOS, watchOS and Linux draw an empty box (`capabilities().web_view` is false). `WebApp::frame_origin(…)` adds an origin to the strict CSP's `frame-src`. Guide: `docs/src/guide/web-views.md`.
- Custom window chrome on macOS (#67): `Application::window_chrome(WindowChrome::Custom { traffic_lights: Some(Point::new(16., 13.)) })` removes the title bar (full-size content view, transparent untitled title bar; the window title still names it in Mission Control, the Window menu and for VoiceOver) and keeps the traffic lights where the app puts them through resizes, full screen and appearance changes. `.window_drag()` on any element moves the window on press and follows the user's « double-click a window's title bar » setting (zoom, minimize, nothing), while buttons and clickable views inside keep their clicks. `cx.window_controls()` (the lights' frame, `None` in full screen) and `cx.is_fullscreen()` re-render on change so the app's title bar can make room for them. Accepted and ignored on the web and other backends; `Runtime::set_window_chrome` for hosts.
- Inline popovers (#70): `popover(menu).light_dismiss(false)` is not closed by a click or tap outside (only by the app, or Escape while the focus is inside), and `.take_focus(false)` leaves the keyboard focus with the anchor's text field, clicks on the popover's items included, for slash-command, mention and autocomplete menus. Web: `popover="manual"`, no focus move, presses inside keep the field focused; macOS: a borderless non-activating child panel instead of an `NSPopover`; iOS / iPadOS: the field stays first responder and taps around the popover reach the window; Linux: a `GtkPopover` without autohide; Android: a non-focusable `PopupWindow`. `OverlayConfig::manual` / `keep_focus` (`manual`, `keepfocus` in the config string).
- `prism::channel()` (#53): an unbounded channel whose `Sender` (`Send + Clone`) works from any thread and whose `Receiver::recv()` is awaited in a task. Sending wakes the UI thread on every backend, so a thread the app owns (a WebSocket client, a file watcher, an SDK callback) feeds the UI without a polling loop or a pool thread held per session. `try_recv`, `drain`, `Sender::is_closed`.
- `Picture::tint(color)` (#51): paints every path of an `svg()` picture in one color (adaptive or `Color::current()` included), for icons that hard-code their color.
- 3D scenes: `scene3d()` with `cube`, `box3d`, `sphere`, `plane`, `cylinder`, `cone`, `torus`, custom meshes and `group`s (`use prism::scene3d::*`), physically based materials (color, metalness, roughness, emission, textures, transparency, unlit, double-sided), ambient, directional, point and spot lights, perspective and orthographic cameras (`docs/src/guide/scene3d.md`). Drawn by SceneKit on Apple platforms (watchOS included), WebGL 2 on the web, OpenGL ES 3 on Android and OpenGL in a `GtkGLArea` on Linux. On the device, without re-rendering the app: an orbiting camera (`Orbit`: drag, pinch, scroll, pan, auto-rotation), `spin`, transitions of every node and of the camera with the scene's `.transition(…)`, and `on_tap` with the named node, the part (label) and instance hit and the point. glTF 2.0 models (`Model::from_glb` / `from_gltf`: node tree, meshes, metallic-roughness materials and textures, skins, animations; prism-core feature `gltf`, on by default) with clips played on the device (`.play(clip, Playback::looping().fade(0.3))`, crossfades, app-built `AnimationClip`s), GPU skinning, instancing (`Instances`, one draw call per set), frustum culling, the full PBR texture set (normal, metal-rough, occlusion, emissive maps, alpha masks), directional shadows, image-based lighting (`Environment::studio()`, `neutral`, `sunset`, `from_hdr`) and ACES tone mapping with `.exposure(…)`. Scenes travel as per-node diffs (`scene3d::wire`); meshes, textures, clips, skins and environments as content-addressed binary blobs fetched once (`scene3d::blobs`, `/_prism/3d/<id>` on the web). `prism_core::scene3d::player::ScenePlayer` is the reference every backend follows (the web, Android and Apple ports are checked against its test vectors and its shaders). Scenes draw only while something changes or moves. The counter's Components tab has a 3D card.
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
- `Runtime::set_capabilities(caps)`: a backend gives the capabilities it finds after startup (a check that answers late), and the app re-renders when they differ, as with `set_device_info`. The web uses it when the browser says whether it has a platform authenticator, which often comes in a second report of the same device and changed nothing on screen before.
- Windows device APIs, all in an unpackaged app (no MSIX): location, motion, taking a photo, the share sheet, passkeys and haptics, which answered « unavailable ». Location is `Geolocator` (`current_location` with the web's accuracy settings, `watch_location` through `PositionChanged`; the permission is Settings › Privacy & security › Location read without asking, `request_permission` asks with `RequestAccessAsync`; refused is `Denied`, no provider `Unavailable`). Motion reads `Windows.Devices.Sensors` (`Accelerometer` with its fused gravity and linear readings, else a low-pass filter, `Gyrometer`, `Inclinometer`) in Apple's units, on tablets and 2-in-1s. `take_photo` opens the Windows Camera app's capture UI for a result (`microsoft.windows.camera.picker:`, the protocol behind `CameraCaptureUI`, whose classes need a UWP window or package identity) and gets a JPEG through a `SharedStorageAccessManager` token; closing it is `Cancelled`. `share` opens Windows' Share dialog for the window (`IDataTransferManagerInterop`): text, a link, files. Passkeys run through `webauthn.dll` (Windows 10 1903 and later, loaded when first used): `webauthn_create` and `webauthn_get` with the relying party, user, challenge, algorithms, exclude and allow lists with transports, user verification, attachment, resident keys, attestation and timeout from the WebAuthn JSON, a browser's client data (`origin` `https://<rpId>`), Windows' own dialog (Windows Hello, security keys, phones), the web's error names (`InvalidStateError` for an excluded credential, `NotSupportedError` for a request Windows refuses) and a ceremony cancelled by the next one; `webauthn_autofill` stays `Unavailable`. `haptic` plays the waveforms of the PC's vibration device where it has one. The capabilities (`location`, `motion`, `camera`, `share`, `passkeys`, `haptics`, and Windows Hello's `biometry` and `device_auth`) say what the PC has: each is checked on its own thread from startup and published when it answers (the app re-renders), without delaying the first frame; a call made before then tries the API itself. New development hook `PRISM_DEVICE_REPORT=<file.json>`: what each device API sees, without a prompt and without where the PC is; `scripts/windows-device-apis.sh` runs it on a Windows machine and checks it (`scripts/windows/device-report-check.py`), and lists what only a person at the unlocked PC can prove (the dialogs). See Windows › Device APIs without a package.
- Rich inputs with styles and chips on iOS, iPadOS and visionOS, Linux and Android, as on macOS and Windows: a `UITextView` (runs as attributes, chips and icons as text attachments drawn from their SF Symbol and label, again when the appearance changes), a `GtkTextView` (runs as text tags, chips and icons as widgets at child anchors on the baseline) and an `EditText` (runs as spans, chips and icons as spans drawn in place of their character). A chip is one character for the caret, the selection and Backspace; the app's echo restyles in place, keeping the caret, the selection and a composition; a tap or click on a chip with `on_click` reports it; copy and cut without the app's hooks write each chip as its label, and a paste is plain text. `prism_native::remote::RemoteWidgets::with_rich_inputs` lets a host say it edits rich text (Android does). Linux's `PRISM_CLICK` takes `type:<placeholder>=<text>`. watchOS and tvOS keep a plain field: SwiftUI's `TextEditor` is unavailable on watchOS and `TextField` binds a `String`; tvOS's `UITextView` cannot be edited in place.
- Pull to refresh (`Scroll::on_refresh`) on macOS and Android. macOS: a trackpad or Magic Mouse pull 60 points past the top, or ⌘R, with a spinner above the content while `refreshing`. Android: a disc follows the finger at the top, like `SwipeRefreshLayout`, and spins until `refreshing` turns off; F5 or Ctrl+R from a hardware keyboard. A tap on a refreshable Android scroll view no longer refreshes it.
- A rich input's paste hook (`on_paste`) gets the clipboard's HTML and the text of its other types on Linux, and Android's copy (`CopyEvent::write`) keeps the app's own types (Markdown, `ClipboardContent::custom`) for its paste to read back, as on macOS and the web.
- Windows: `prism build windows --msix` makes an MSIX package instead of the zip (`target/windows/<package>-<version>-windows-<arch>.msix`, 5.9 MB for the counter against its 26.6 MB zip): the executable and the app's assets, without the Windows App SDK runtime, which the Windows App Runtime framework package provides (a `PackageDependency` on `Microsoft.WindowsAppRuntime.2` 2.7.0.0). The executable is built with `PRISM_WINDOWS_PACKAGED=1`: `prism_build::link_app()` then embeds a manifest without the runtime's `activatableClass` registrations and copies no runtime. The `AppxManifest.xml` comes from `Prism.toml`: the identity (the bundle id, the new `[windows] package_publisher`, else `CN=<publisher>`, the version: `[app] version` with 0 as its fourth part, `1.2.0.0`, as the Microsoft Store requires, so each MSIX release needs a higher `[app] version`; `[app] build` stays in the executable's file version), a desktop `Application` with `runFullTrust`, logos scaled from `[app] icon` (`prism_build::winres::logo_png`, also from an `.ico`), the notification activator's COM server, and `[permissions]` as device capabilities (location, webcam, microphone). No `resources.pri`: a packaged app's XAML resource manager reads WinUI's `Microsoft.UI.Xaml.Controls.pri` from the framework package. Packing is plain Rust on any OS (`msix.rs`: Zip64, 64 KB deflated blocks, `AppxBlockMap.xml` with SHA-256 hashes, `[Content_Types].xml`): `makeappx unpack` opens packages made on a Mac and on Windows, and `makeappx pack` validates their manifests.
- Windows: code signing. With `PRISM_WINDOWS_CERTIFICATE` (a `.pfx`) and `PRISM_WINDOWS_CERTIFICATE_PASS` (never in `Prism.toml`), `prism build windows` signs the executable before zipping or packing it, and the MSIX after packing (SHA-256 Authenticode, `PRISM_WINDOWS_TIMESTAMP_URL` to time-stamp): `osslsigncode` first on every OS (`PRISM_OSSLSIGNCODE` or `PATH`; the password goes through a file in a folder only the user opens), else `signtool` on Windows (`PRISM_SIGNTOOL`, `PATH` or the Windows SDK), which never gets the password nor a key file: Windows PowerShell opens the `.pfx` with the password from its standard input, its key in memory only, and imports that key, non-exportable, into a key container named `prism-sign-<pid>-<random>` in the user's key store (CNG's software key storage provider for RSA and ECDSA, a CryptoAPI provider for RSA if it refuses); signtool signs with it (`/csp`, `/kc`) and the public certificate, and that container alone is deleted right after, also when signing fails. A container a crashed run left is deleted by the next one once its process is gone. Nothing goes into a certificate store, and no other key or file is deleted. The MSIX's publisher is the certificate's subject as Windows writes it, read back from the signed executable's PKCS #7 in plain Rust, so no certificate store is needed; a `[windows] package_publisher` that differs stops the build. Checked with a self-signed certificate made at test time: `osslsigncode verify` on a Mac, `signtool verify` and `Get-AuthenticodeSignature` on a PC.
- Windows: the MSIX's install was tried on the test PC (Windows 11, Windows App Runtime 2.7.0.0 installed): `Add-AppxPackage` refuses it with `0x80073CF0` / `0x800B0109` because the self-signed test certificate's root is not trusted there; trusting its public part for the user only (`Cert:\CurrentUser\TrustedPeople`) was tried and is refused the same way, and machine-wide trust needs an administrator. The package's structure is proven instead: `makeappx unpack` and `pack` succeed, the manifest's publisher equals the signer, `signtool verify /pa` names the test certificate, and the packaged executable registers no runtime class. Microsoft's unsigned route (a publisher with `OID.2.25.311729368913984317654407730594956997722=1`, `Add-AppxPackage -AllowUnsigned`) was tried for the standard user too: Windows refuses it with `0x80073D2B`, an unsigned package may not contain executable activations, so `prism build windows --msix` makes no unsigned test package. The packaged app has not run yet.
- Windows: remote push through Windows Push Notification Services, in an MSIX: `register_for_push` asks the Windows App SDK's `PushNotificationManager` for a channel for the new `[push] wns_remote_id` (a Microsoft Entra app's object id, compiled in as `PRISM_WNS_REMOTE_ID`) and gives its URI as the token (`PushService::Wns`); raw pushes carry Prism's payload and reach `on_push` or `on_background_push`, also when WNS starts the closed app (the manifest's push COM server, from `[push] wns_app_id`): its window then stays hidden, a deferral keeps it alive while the push is handled (25 seconds at most), and it quits. While the app runs, the channel is renewed a day before its `ExpirationTime`, the new URI going to `on_push_token`. The zip build and an MSIX without the remote id keep answering « unavailable ». `prism-push` sends to WNS (`Sender::wns(WnsCredentials::new(tenant, client, secret)?)`: an Entra client-credentials token for `https://wns.windows.com/.default`, cached, then a `wns/raw` `POST` of 5,000 bytes at most to `https://*.notify.windows.com` channels only, always with `X-WNS-Cache-Policy: cache` so a PC that is offline gets the latest one, and `X-WNS-TTL`; `X-WNS-Status: dropped` or `channelthrottled` is an error, and only 404 and a 410 for an expired channel are `PushError::Gone`, not a 410 « Domain Blocked »). Not run end to end: a channel needs an Entra registration that Microsoft maps to the package, and none exists for Prism's tests.
- Repository tasks `mise run build` (every crate and the example), `mise run clean` (the target folder, `docs/book`, and the Docker images and registry volume Prism builds; a `CARGO_TARGET_DIR` outside the checkout, which other checkouts may share, is left alone) and `mise run dev` (the same as `mise run web`).

### Changed

- Platforms in 0.1: web, macOS, iOS and iPadOS, and Android phones and tablets are supported; tvOS, watchOS, visionOS, Linux, Windows, Wear OS, Android TV and car screens are previews, to try, with their gaps listed. The README and the book's introduction list them with the oldest system each runs on: macOS 13, iOS 17, Android 10 (API 29), tvOS 17, watchOS 10, visionOS 26, GTK 4, Windows 10 1809.
- prism-ui: without `ui::set_theme`, components use `Theme::system()` and follow the system's light or dark appearance, as native text and controls do. The default was the light theme, which put the system's white text on light cards wherever the system was dark: in dark mode on tvOS, and always on watchOS.
- Prism needs Rust 1.94 or later (it was 1.88): sqlx 0.9, which brings the patched SQLite, requires it.
- `prism::db` re-exports sqlx 0.9 (it was 0.8). sqlx 0.9 runs a query string only if it is a literal or wrapped in `sqlx::AssertSqlSafe(..)`; literal queries compile as before, and `Db::open`'s migrations still take any `&str`. sqlx's SQLite features are the ones 0.8 had (bundled SQLite; no `deserialize`, `load-extension` or `unlock-notify`).
- `prism-push` is on RustCrypto's 0.14 crates (p256, ecdsa, elliptic-curve, pkcs8, aes-gcm, hmac, sha2): `webpush::ReceiverKeys::secret`, `webpush::decrypt` and `webpush::encrypt_with` take p256 0.14's `SecretKey`. Their keys, messages and signatures are the same bytes as before (RFC 8291, RFC 8292, RFC 7515 and the APNs fixtures pass unchanged).
- The web client no longer carries its own copy of the 3D scenes' GLSL: the server puts prism-core's `scene3d::shaders` in `client.js`, so the web draws with the same text as the OpenGL backends (the copy had drifted: it lacked the `#ifdef GL_ES` that shaders.rs puts around the shadow sampler's precision; WebGL 2 drew the same).
- `Application::run` on a build without a backend panics with the reason (neither `native` nor `web` enabled, the native Windows backend not available yet, or the OS that has none), the native targets (macOS, iOS/iPadOS/tvOS/visionOS, watchOS, Linux, Android), and the `web` feature, which works on Windows too.
- Big trees render, update and lay out faster on every backend (the Performance chapter has the numbers). The reconciler no longer goes quadratic when many entities are notified at once, when keyed or entity children are reordered, or when many nodes have context menus or links. Native backends measure each leaf once per width in a layout pass (80,000 → 30,000 native measures in the first layout of 5,000 rows), find keyboard bars and `z_index` paint order in one walk of the tree, and hash node ids with one multiplication. On 5,000 rows: every row notified 343 → 14 ms, keyed rows reversed 143 → 9 ms, one row changed under a keyboard bar 130 ms → 27 µs, the first render of rows with a `z_index` badge 7.1 s → 0.11 s, laying out again after one change 4.9 → 3.0 ms. `prism_core::IdHasher`, `IdMap` and `IdSet` (hidden, for backends) are new.
- Android is faster to start and to update: text measures are kept in Rust by request (one JNI call per distinct text, style and width, measured with one reused view in Java), so the example's first screen makes 35 measure calls instead of 254 and appears ~165 ms after the process starts instead of ~800 ms; scrolling a 10,000-row `VirtualList` costs ~0.7 ms per scroll event instead of ~5 ms (2 % janky frames instead of 54 %). An endless `repeat` animation pauses while its view is hidden (a screen below, another tab) instead of waking the app on every frame, and only a tab's icon rebuilds its tab bar.
- Linux: nodes with the same style share one CSS rule (class `s<n>`), and the stylesheet, whose reload restyles every widget, is loaded again only when a new rule appears: a click that changes a text no longer reloads it (~0.5 ms instead of ~2 ms), and rows scrolling into a long list reuse their siblings' rules (a 3,000 px scroll of 10,000 rows: ~21 ms instead of ~40 ms).
- Apple backends, faster and quieter (measured on the example app, [Performance](https://prismsdk.dev/performance.html)): on macOS, tabs and the screens under the top of a stack stay in the window, hidden, instead of being detached, so switching back to a tab (29 ms → 5–18 ms) or going Back to a large screen (39 ms → 5 ms) draws nothing again, at the cost of keeping their layers' drawing; countdowns are ticked by a timer that runs only while one is shown, so an idle app wakes up 0.3 times a second instead of 1.4 (macOS) and 0.05 instead of 1 (iOS); `cx.format_number` reuses its Foundation formatter (scrolling a list of numbers −40 % on macOS, −50 % on iOS); the Swift bridge is compiled as one module (`-wmo`), a quarter faster to build.
- The router picks the most specific route that matches a path, whatever the order the routes were added in (#83): segment by segment from the left, a static segment beats a `:param`, which beats a `*rest`, so `/sites/new` wins over `/sites/:site` and `/files` over `/files/*rest`. Routes of the same shape keep their order (the first added wins) and debug builds warn about the one never reached.
- Web deep links no longer build the screen under them (#65): opening `/orders/42` keeps `/` underneath (Back, the browser's history and the header's Back still go there), but that screen is built when Back reaches it, not in the first paint. A deep link no longer loads the root screen's data nor ships its HTML; the page carries an empty hidden placeholder instead. Native backends still build it up front, since their Back shows it (iOS names it in the Back button, the swipe reveals it).
- Bundling asks the app what it declares once (#74): `PRISM_DESCRIBE=all` prints the assets directory, widgets, background tasks, Live Activities, widget buttons and car screen as one JSON object, so `ios-run.sh`, `android-run.sh` and `macos-app.sh` start the host build once instead of up to six times (the app's `init` runs once). An unknown `PRISM_DESCRIBE` question exits with status 2 instead of starting the app, and a failed host build says why the bundler needs it. The `prism` command chapter documents the host build and the `Cargo.toml` layout for apps that enable the native backend only on their device targets (« What the bundlers ask the app »).
- `ui::DateField::value` is an `Option<SystemTime>` (#89); `ChangeEvent::date()` and `minutes()` read an empty value as the epoch and 0.
- Web: the server compresses what it sends.
  - **Socket.** Browsers with `DecompressionStream` ask for compressed frames (`&compress=deflate-raw`). Messages of 1 KB or more then arrive as binary frames of raw DEFLATE, and the client applies them in order. Small patches stay text. Axum's tungstenite has no `permessage-deflate`, so this is done at the application level. `prism_web::Frame` frames messages for other WebSocket servers.
  - **HTTP.** `WebApp::handle` gzips pages per request. It gzips the client, CSS, islands and text assets once, cached by hash, with a `"<hash>-gzip"` ETag. It honors `Accept-Encoding` q-values and sends `Vary: Accept-Encoding`.
  - **Result.** On the counter, a cold load of `/` goes from 283 KB to 63 KB over HTTP and from 390 KB to 37 KB over the socket.
- Web: lighter sessions, first trees and client.
  - **Memory.** A live session no longer keeps its first render's patches and the JSON of its first message: 1.08 MB → 0.71 MB per open session on the counter.
  - **First tree.** Within one message, a style op whose CSS an earlier node got names that node (`"like": id`) instead of repeating it: a 5,000-row list's first tree goes from 6.5 MB to 3.1 MB of JSON (deflated, 253 KB → 243 KB, in half the time), the counter's from 23.1 KB to 19.0 KB.
  - **Client.** `client.js` and `prism.css` are served without comment lines, blank lines or indentation (the sources keep them): 107 KB → 77 KB and 5.6 KB → 4.0 KB gzipped.
  - `cargo test -p prismsdk-web --release --test bench -- --ignored --nocapture` measures a long list's page, first tree and patches.
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
- Linux: tray icons no longer use `ksni`. prism-linux serves the StatusNotifierItem and its `com.canonical.dbusmenu` menu itself, over GDBus (the D-Bus library GTK already links), on a thread of their own. 36 crates leave the lock file, among them zbus 5 and its unmaintained `zvariant`, `zvariant_derive` and `zbus_names` (RUSTSEC-2026-0370, RUSTSEC-2026-0372, RUSTSEC-2026-0371), and none join it: `cargo audit` reports no warnings. A click on the tray icon now brings the app's window forward (it did nothing), with the panel's activation token on Wayland.

### Fixed

- macOS: an app built with Xcode 26 runs on macOS 13 to 15 again, and Prism builds with older Xcode versions. A heading's accessibility role was the `NSAccessibilityHeadingRole` constant, which exists only from macOS 26: older systems failed to load the app and older SDKs failed to link it. It is now that constant's value, `"AXHeading"`, which VoiceOver reads on every version.
- Android: `prism run android` and `prism build android` build against the newest stable platform of the SDK, not a preview one (its `source.properties` names a codename) whose APKs install only on that preview (`INSTALL_FAILED_OLDER_SDK: Requires development platform`). The target SDK is the platform's API level, not the end of its folder name (`android-36-ext17` gave `ext17`). With only previews installed, the build stops and says which platform to install.
- A clone on Windows keeps LF line endings (`.gitattributes`): Git's `core.autocrlf` turned the Swift, Java and shell sources copied into builds into CRLF, which shell scripts do not run with.
- `mise install` adds the `wasm32-unknown-unknown` target, which `prism build web` needs for islands.
- `apple/generate_widgets.py` writes its Swift as UTF-8 with LF line endings on every system; on Windows it wrote CRLF and the ANSI code page.
- Windows: a web view's links and new windows that leave the app open with `PRISM_BROWSER` when it is set, as `open_url` does (they always went to the default browser, so a test PC's logger never saw them), and a refusal is logged with why: `web view N: a new window for <url> not opened: no user gesture` (or a sandbox without popups, not a web, mail or phone link); with `PRISM_TRACE`, each hand-off too (`… opens in the browser (user gesture)`). The decision is a pure function, `links::new_window`, tested on every host. Seen on screen (Windows 11 25H2, `scripts/windows/onscreen.ps1`): a page's `window.open` without a gesture returned `null` and started no process or window; a real click on its `target=_blank` link went to the https handler, and with `PRISM_BROWSER` set to Chrome a Chrome window showed the page.
- `scripts/windows/run-in-session.ps1` no longer opens a Windows Terminal window on the PC's screen while its command runs: with Windows Terminal as the default terminal, the scheduled task's PowerShell got a terminal window despite `-WindowStyle Hidden`. The task now starts it in `conhost.exe --headless`.
- Windows: `PRISM_CLICK` makes its window the active one also when the foreground lock refuses it (a program started by a scheduled task, with the desktop active): it joins the active window's input queue for the call. Windows shows the Share dialog only over the active window, and real clicks need it too.
- The counter shows a local web page by its file name (`Loaded: s41.html`), not its path, which named the user's folder in screenshots.
- macOS: the selected tab's symbol shows. It was drawn in the bar's tint over the selected segment's bezel, which has that tint too, so it vanished; it is now drawn as AppKit draws a selected segment's, like its title (white on the tint, gray in a window that is not key).
- Windows: a rich input is measured from its runs: its bold words and each chip (its symbol, label and padding) take their room. The counter's English rich note was one line tall and its second line, « Roadmap. », was cut.
- Windows: a rich input keeps its chips. RichEdit reported a chip's picture as a space, so the first change the field reported (the picture going in) turned the chip into a space in the app's runs: the counter's rich note lost its « Roadmap » chip, its click and its Markdown. The text is read with the chip's U+FFFC.
- Windows: a text area or rich input that draws its own look (a background or border, as `ui::text_area` gives it) and has no padding keeps its text 10 × 8 from its edges, as on macOS and Linux; its text touched the border. Its measure counts those insets, and a padded one wraps its text at its real width (the padding was taken off twice).
- watchOS: a text area and a rich input show their text on as many lines as they are measured; watchOS drew a text field's text on one line, cut (« through this o… »), in a box as tall as the wrapped text.
- Counter: the last English left in French and Arabic is translated: the widgets' names and descriptions in the gallery (named again when the language changes), the local web page of Web and video (with its `lang` and `dir`), the shared text, the first todo and the « kg » of Layout.
- Counter: the Navigation card and the Report screen it opens are in the app's language (they stayed English in French and Arabic): their headings, buttons, room names, periods (« 30 j ») and the « Saved » notice.
- Windows: a text area is three lines tall at least unless `rows(n)` says otherwise (#235), as on the other platforms; it was measured one line tall.
- Counter: the rich note and the three Blocks paragraphs are in the app's language (they stayed English in French and Arabic), and the note says what works where: ⌘-click a link on macOS, Ctrl-click on Windows and Linux, either on the web; on touch screens, which have no modifier-click, it asks for a tap on its chip.
- A rich input is as tall as its text, as on macOS: a paragraph of one line is one line tall. iOS, Linux and Android show it as a text area, which is three lines at least (#235), so each one-line paragraph of the counter's Blocks was about three lines tall (60 pt on iOS, 70 px on Linux). `rich_input` now sends `rows=1`. On Android and watchOS, a text area's height is now measured with its `rows` too: the host measured three lines whatever `rows(n)` said, and a text area with padding measured its minimum lines in place of a longer text that wraps past them.
- Linux: a focused field keeps the focus and its selection when a sibling comes or goes. Every child was unparented and added again, so selecting words in the counter's rich note (its Bold toolbar comes beside it) moved the focus to the next field, and Ctrl+C copied nothing.
- Linux: a text area or rich input counts its frame when measured. The scrolled window's border (a pixel on each side) was left out of the width and the height, so a text on two lines (a Blocks paragraph, the counter's shared draft) wrapped differently and was cut, scrolling inside its field.
- macOS: a click on a rich input's chip runs its `on_click`. AppKit reports clicks on an attachment's cell to the text view's delegate, and a chip's image attachment has none: the text view now finds the chip under the click itself.
- macOS: a text area or rich input is as tall as its text lays out. An unstyled field was measured 2 pt wider than its bezel leaves its text, and sometimes empty, so a line that wrapped there (the counter's rich note, its chip on the second line) was cut off.
- A chip's `doc.text` icon (and `doc`), the guide's example, draws as a document (📄) where Prism maps SF Symbols to glyphs (Android, the web); it was the unknown symbol's bullet.
- The selected tab follows the route. `navigate()` and `replace()` to a path under another tab's root (`/components/table` while Settings shows) open it in that tab and select it; they pushed it on the selected tab, whose button stayed lit, so the tab bar and Back disagreed with the screen. A path under no tab's root but `/` (a screen several tabs show) still opens in the selected tab. A link opened while the app runs picks its tab as a deep link at launch does: the longest tab root it starts with, `/` included.
- Android: text without a color of its own (a text, a timer, a symbol) takes the theme's primary text color (`textColorPrimary`), in light and dark mode, and again when the theme changes while the app runs. A bare `TextView` took the small text appearance's secondary color: gray. After a theme change while the app runs, native buttons, fields (their text, placeholder and underline), check boxes, radio buttons and switches now take the new mode's looks as well; they kept the old mode's (black text in a dark field, a light button on a dark screen). Buttons, fields and switches are also made in the app's own mode (`set_appearance`), as the other controls already were, and the theme's accent (a native slider, the selected tab) is the app's mode's, not the system's.
- Android, TalkBack: a table cell holding a control (a button, a link, a field) is no longer one element hiding it. TalkBack moves to each control and text in the cell, reads them with the cell's row and column (the cell keeps its `CollectionItemInfo`) and activates the controls; the cell was one focus stop read from its text, repeating its controls, with nothing to activate. A cell without a control still reads as one element. Plain texts are now in the tree TalkBack reads: each accessibility update set a view back to `importantForAccessibility="auto"`, under which a `TextView` is not important, and TalkBack asks for important views only; texts and timers now keep a `TextView`'s own `yes`. A heading (`.heading(n)`) is marked as one (`setAccessibilityHeading`): TalkBack says « heading » and its heading navigation reaches it; it read as plain text. A label that goes away (a cell emptied) is cleared instead of staying.
- Linux: screen readers no longer read table rows, tabs, scroll bars and other elements as "PrismBox", "GtkToggleButton" or "GtkScrollbar" (GTK 4.8 and 4.9 name an element without a label after its type; 4.10 and 4.11 leave it unnamed). Before GTK 4.12, Prism names rows, cells, buttons and tabs after their content, as GTK 4.12 and later do, and gives other elements an empty name; a plain layout box has no name. Drawn controls get their role (button, link, heading with its level, list, image, landmarks, tab, tab panel…), take the keyboard focus and are pressed with Enter or Space, as on the web. Tab now reaches them, a view with `on_key_down` and a clickable text too: a focusable GTK box passed the focus on to its children only, and a label without links refused it; a disabled one is unavailable, `a11y.hidden` hides an element, and only what can be selected is "selectable". The tab bar is a tab list of tabs named by their titles, the selected one selected; drawings and 3D scenes are images. A hint is no longer erased on GTK before 4.12, where clearing a tooltip cleared the description. `scripts/linux-a11y.sh` dumps the AT-SPI tree Orca gets from the counter and checks it, on GTK 4.8 and on the newest GTK (`docker/linux-a11y.Dockerfile`); the Tables guide says what Orca gets from a table: no GTK 4 implements AT-SPI's `Table` and `TableCell` interfaces.
- Apple: VoiceOver reaches and activates the controls in table cells (#284), and tables keep their data-table semantics. iOS, iPadOS and visionOS: a cell of text stays one element, and each control in a cell (a button, a switch, a link, a text field) is an element of its own that reports the cell's row and column (`UIAccessibilityContainerDataTableCell`), so the table's rows, columns and headers still read; activating a cell of a clickable row clicks the row. tvOS: the table is the same data table (it was plain containers), and the remote focuses each control in a cell and the cells of a clickable row, whose Select clicks the row. watchOS: the table is a container of its cells row by row, header cells have the header trait, a cell of text is one element (an empty label no longer hides its text), each control in a cell is its own element, every element gives its column header in the More Content rotor, and a cell of a clickable row has an action that clicks it. `UIAccessibilityContainerDataTable` and `accessibilityContainerType` are unavailable on watchOS and SwiftUI has no row or column API, so the watch reads no row or column numbers. macOS: a cell holding a control has no label of its own, so VoiceOver reads what it holds (`Track, button`), and pressing the cell presses its control; pressing a cell without a control clicks a clickable row; a link or a clickable text is now its own element with its role, label and press (AppKit's text cell read it as plain text). A picker in a cell opens its menu when VoiceOver activates it: on iOS, iPadOS and visionOS through its primary action (iOS 17.4 and later; before, the cell answers no and VoiceOver taps the picker), on macOS through its click on the next run loop turn. The iOS, iPadOS, tvOS and visionOS table answers an element for every cell, never a cell holding a control (a container), so table navigation always lands on something VoiceOver reads; on tvOS a picker in a cell reports its row and column like the other buttons. macOS: an icon is no control (an image view is an `NSControl`), so a cell of an icon and a text reads as text and its press clicks a clickable row; pressing a cell whose control acted (a button, a switch, a checkbox) reports success, where AppKit answers no; VoiceOver reads a control's `accessibility_label` and help, which AppKit's controls take from their cells (a picker read only its value), and skips a hidden one (a decorative image read its symbol's name). watchOS: a cell keeps its views, and their state, when its label, a control in it or its row's click comes or goes (each part is one chain of SwiftUI modifiers whose values change). `PRISM_A11Y_DUMP` lists the elements in such cells (`"parts"`) and, on iOS, tvOS and visionOS, the table's answer for each cell (`"answers"`); the hook `PRISM_A11Y_PRESS` presses elements by label, or a table's cell by row and column (`cell:R,C`), as VoiceOver does, on macOS and now on iOS, tvOS and visionOS, and on macOS `PRISM_AFTER` also waits after it.
- watchOS: SwiftUI draws text and controls in the app's appearance. The host ignored it and used the system's, which is dark on a watch, so a light theme had white text and controls on its light cards. A screen's background now fills the display, under the title and the clock and at both ends of a scroll, where the system's black showed.
- tvOS: buttons, toggles, pickers and text field placeholders read in both appearances. tvOS's tint is white in light and dark mode, and an unfocused system button draws its title in it: white on a pale platter in light mode. Buttons and pickers take the label color as their tint, and a placeholder is drawn in the placeholder color of the app's appearance.
- Tests of prism-web and prism-core no longer leave folders in the system's temp directory (`prism-sec-s10-*` and others piled up in `%TEMP%` on Windows): each temp folder is held by a guard (`TempDir`, shared by a crate's unit and integration tests) that removes it when the test ends, also when it fails.
- Nothing of Prism's is left in the system's temp directory any more, on macOS, Linux and Windows. The tests of every other crate (prism-cli, prism-db, prism-build, prism-native, prism-apple-build, prism-push, prism-macos, prism-linux, prism-windows) hold their temp folders with the same guard, so a test that fails or skips early (the signing tests without `python3`) no longer leaves `prism-signing-*`, `cli-*`, `s24-*`, `prism-db-s38-*` or `prism-db-test-*` behind; prism-db's tests close their databases first, since Windows cannot delete an open file. In the code: prism-db, without a data directory (headless tests), keeps its database in memory instead of a `prism-db-<uid>` folder that was never removed; Apple platforms register run-time fonts from `prism-fonts-<pid>` and stage saved files in `prism-save-<pid>-<n>`, both removed when the app exits, or by the next app when it was killed; Windows copies dropped files to the app's `<data>\picked` (with picked files) instead of `%TEMP%\prism-dropped`; the iOS camera removes its temporary JPEG once copied; `prism publish` retries a refused `hdiutil detach` with `-force` and names the folder when the image stays mounted; the counter writes its web page to its data directory instead of `prism-counter-page.html` in the temp directory.
- Windows: a secure text field leaves no hidden text box behind. Its `TextBox` became a `PasswordBox` but stayed in the node's canvas, under it and still focusable: an extra stop in the tab order and an extra edit field in UI Automation (named after the placeholder, or unnamed when the field was made after the first frame).
- macOS: a wrapping text's min-content width (the width an `items_start` column, a `MinContent` grid track or a table column shrinks to) includes the label's own inset, so the text measured at that width no longer breaks its longest word (#284).
- Windows: a wrapping text's min-content width is its longest word (#284). It came out near 0: XAML clips a measured size to the width it is measured at, so the narrowest width is now found from line counts (`Wrap` against `WrapWholeWords`), and a text that cannot break (`09:35`) is its one-line width.
- Linux: a wrapping text's min-content width is its longest word, as on the other backends, not one character (GTK labels break between characters when a word doesn't fit): a squeezed column keeps `09:35` and `Line` whole (#284).
- Windows hosts: `prism build linux` passes the container `/src/examples/app`, not `/src\examples\app` (the path was joined with the host's separator), and `prism-native` writes file URLs as `file:///C:/…` with `/` separators. The CLI's tests no longer need the `openssl` program for P-256 keys (ring makes them) and skip the RSA ones without it, so `cargo test --workspace` passes on a stock Windows. `prism-http`'s test servers read whole requests before answering: a server that closed with a request body unread reset the connection, and `fetches_from_a_local_server` failed now and then on Windows (os error 10054).
- Android: scroll events report points, as `ScrollEvent` documents, not pixels. A `VirtualList` rendered the rows 2.6 times further down than shown on a typical phone, so a long list went blank after a few flings; `on_scroll` handlers and `cx.scroll_offset` read points like on the other backends.
- Android: views are released when their node is destroyed. Two weak maps of the Java host (views to nodes, blurred backdrops) held each view through their own value, so every view ever created stayed in memory: the example used 174 MB after scrolling its long list, 53 MB now.
- Widgets (iOS, iPadOS, macOS, visionOS) and watchOS: a `timer(..)` follows `.text_center()` and `.text_right()` (#279). A timer takes the width it is offered and lays its digits out by its own alignment, so it sat left-aligned, most visibly in an `AccessoryCircular` widget. In widgets, a centered or right-aligned text or timer also keeps its alignment when it is stretched by its column, fills the width (`w_full()`, `flex_1()`) or is cut on one line (`nowrap()`); it was pushed to the leading edge.
- iOS, iPadOS, macOS, visionOS: a configurable widget whose saved option no longer exists (an option gone since, or the empty placeholder of an instance added before the app wrote its options) draws the default configuration instead of staying a grey placeholder (#277). The generated `EntityQuery.entities(for:)` answers every id it is asked, an unknown one with an entity titled like the default option; it used to leave it out, so the required parameter was missing and WidgetKit never asked for a timeline.
- macOS: a `text_input` with its own look (`.border(0.)`, a padding, a background, a border, a `focus` refinement) no longer draws AppKit's blue focus ring inside the app's own outline (#274); Linux drops the GTK theme's outline on such fields too, as the web already did. An unstyled field keeps the platform's ring. `Style::own_field_look` is the shared rule.
- `group_hover`, `group_active` and `group_focus_within` follow the nearest `group()` ancestor only, as documented (#273): with nested groups, hovering one inner cell no longer lights up the refinements of every cell because the outer group is hovered too. Native backends (prism-native), the web (CSS) and Linux (now resolved by prism-native instead of GTK CSS). Named groups (#212) are unchanged.
- `Runtime::dispatch` no longer returns answers (a selection, a highlight, a clipboard write) aimed at nodes its own re-render destroyed (#265): an `on_edit_across` handler that merges fields can be tested through `dispatch` with `prism-testing`, which panicked on them.
- Linux: `prism-linux` builds again (a rich input is created as a text area, as `prism-native` already asks), and screen-reader states no longer log GTK criticals: the checked and selected states are passed as the ints GTK reads (GTK 4.8 and later). `cargo doc` with `-D warnings` passes again (two `scene3d` links).
- macOS, iOS, iPadOS, visionOS: a child drawn outside its parent (absolutely positioned with a negative offset) gets clicks, hovers and drags in the part outside it, unless the parent clips (#213). AppKit and UIKit only hit-tested inside the parent's bounds; before macOS 14, hover tracking stopped there too.
- Native backends: a `picture` given only a width (`w(px(…))`, `w_full()`, a percentage) takes its height from the image's own ratio at that width, wider than the image included (#208). It used to keep the image's natural height, or the height of the image narrowed to the width. The web already did this.
- macOS, iOS, Linux: a `scroll()` the app moves (`cx.scroll_by`, `scroll_to`, `scroll_into_view`) reports its new offset (#204). `on_scroll` runs and `cx.scroll_offset(id)` follows. The platform's scroll callback came while Prism was applying the patches that moved it, so the event was dropped; it is now dispatched right after.
- Web: a fast drag off a small handle starts (#194). The page now follows the pointer on the window from the press until the release, so a flick whose first move past the slop is already off the element still starts `on_drag`. The pointer is captured only once it is a drag, so a click on a child stays the child's.
- Web: the page title keeps the top screen's name once the live session starts (#185). The first tree built the main stack before the `root` patch that made it the main one, so the tab fell back to the app's name ("Sign in · App" became "App"), with or without a header bar.
- A `take_focus(false)` popover light-dismisses (#182): a press outside it, the anchor's text field included, closes it and calls `on_dismiss` with `Outside`, and the press still reaches what it landed on (the caret moves). Typing does not close it; `light_dismiss(false)` keeps it open as before. On macOS it is shown in a child panel and never closed. The web, Linux, iOS and Android now watch presses around it too; `take_focus(false)` no longer implies `light_dismiss(false)`.
- macOS, iOS, iPadOS, visionOS: a `rich_input` or `text_area` with zero padding lines up with the same text drawn by `text()` (#179), so switching from a text to a field to edit it no longer makes the text jump. The text view's line fragment padding (5 pt) is gone: macOS keeps a label cell's 2 pt and iOS 0, and the default inset absorbs the difference so unstyled fields look the same. The style's `line_height` and the label's baseline offset now apply to the field's text and typing. On macOS the field lays out with a label's typesetter, and on iOS with TextKit 1 like labels.
- macOS: a mouse click no longer leaves a focus ring on clickable views (#177). Like native buttons, a view with `on_click` takes the focus only with Full Keyboard Access (Tab or a click) or from `cx.focus`. A view with `on_key_down` still takes it on click, but a focus from a mouse press draws no ring; Tab and `cx.focus` do.
- Linux, iOS / iPadOS / visionOS and Android: `ClickEvent::modifiers` / `count` and `on_key_up` / `on_global_key_up` (#159) work there too. Clicks carry the keys held and their number in a quick series (GTK `n_press`, UIKit `UITouch.tapCount`, Android's double-tap timeout and slop). Key releases go to where their press went, and keys still held when the window or app loses the focus get their release. On Android, the second tap of a double tap now reaches `on_click` (the gesture detector used to swallow it).
- Linux, iPadOS and Android: `drag_buttons` and `DragEvent::button` / `modifiers` (#136) work there too, so a canvas pans with the middle (or secondary) mouse button. Linux: `GestureDrag` on any button, and with a secondary drag the context menu opens on a release that did not move. iPadOS: the pan's `buttonMask` picks the button. Android: the mouse's `BUTTON_TERTIARY` / `BUTTON_SECONDARY`. They report the keys held when the drag started; they used to report the primary button and no keys.
- Android and Linux: `on_wheel` and `on_magnify` are reported (#133), so a canvas pans with a wheel or touchpad and zooms with a pinch there too. Android: `ACTION_SCROLL`, and a pinch measured from the fingers' spread (from the touch slop on, without `ScaleGestureDetector`'s minimum distance of about 27 mm between the fingers), with a one-finger `on_drag` that follows the fingers' center during a pinch (no jump when a finger joins or leaves). Linux: `EventControllerScroll` (touchpad phases, a wheel notch as 48 points) and `GestureZoom`.
- iOS, iPadOS, visionOS: `on_magnify` and `on_wheel` are reported (#172). A `UIPinchGestureRecognizer` sends `MagnifyEvent` (the scale since the last event, the fingers' center, `Start` / `Move` / `End`), next to a one-finger `on_drag` that keeps following the fingers' center during the pinch, so an infinite canvas pans and zooms with two fingers. An iPad trackpad or mouse scrolling over an element sends `WheelEvent` (a scroll-only pan recognizer; a finger still scrolls what is behind).
- Web: the shortcuts of app menu items (`set_menu_bar`, `set_app_menu`) run their item (#170). Their ids started at 2^62, beyond the 2^53 a JavaScript number holds exactly, so the page sent back a rounded id that named no item. App menu ids now start at 2^52 on every platform (context menus, derived from node ids, stay below).
- macOS: Escape outside a sheet no longer crashes the app (#163). AppKit sends `cancelOperation:` up the responder chain from a focused field; Prism views forwarded it to `NSView`, which has no such method (« invalid message send »). It now goes on up the chain, to whatever handles it; the app's key handlers hear `"Escape"` from the key monitor as before.
- Apple platforms: a variable font given as bytes (`EmbeddedAssets`, `FontFace::bytes`) has all its weights (#150); it drew every weight as regular. Registering a `CGFont` keeps only the default instance, so the bytes now go through a cache file registered by URL, like the bundle's fonts (the `CGFont` remains the fallback without a writable cache).
- Web: a dialog opened in the same update as `.autofocus()` or `cx.focus` keeps that focus (#148); `showModal()` moved it to the dialog's first control.
- Docs: `WebSandbox::popups()` also lets the windows a sandboxed page opens escape the sandbox (`allow-popups-to-escape-sandbox` on the web, the system browser natively), now documented (#146).
- Web: the current entry of a navigation made of `.link()` elements is `aria-current="page"` (#144): `accessibility_current(true)` on an `<a href>` said `"true"` because the link role was dropped before the value was chosen, in the first paint and in the client.
- Web: an element dragged only with the middle or secondary button (`drag_buttons`, #136) is no longer text-selectable (#142): every element with a drag handler gets `prism-draggable` (`user-select: none`, `touch-action: none`), so a left drag over a middle-button canvas no longer selects the whole area.
- Web views load only web pages (#62 follow-up): `web_view(url)` refuses URLs whose scheme is not `http`, `https` or `file` (read as browsers read it: surrounding spaces and control characters, tabs and newlines ignored), and the web client loads only `http:` / `https:` frames. `web_view("javascript:…")` built from a user's link ran in the app's own page on the web, with its cookies and session. `prism_core::web_view::is_web_view_url`.
- macOS: a dialog placed beside its anchor (`dialog(…).side(…)`, #79) is modal again: a press on the window behind it closes it and reaches nothing else. It is shown as a transient `NSPopover`, which closes on an outside click but may also hand that click to what was clicked; presses in the dialog, and in menus or popovers opened from it, go on as usual.
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
- Linux: a tray set while no panel shows trays appears when one starts (it stayed hidden until the app set it again), and registers again when the panel restarts. Underscores in its menu titles show instead of marking access keys. Setting a tray no longer waits for the panel's answer on GTK's thread.

### Security

Fixes from a security review before the first release.

- The workspace's `Cargo.lock` takes `url` 2.5.9 (RUSTSEC-2026-0592: a stack overflow in `Url::origin()` on nested `blob:` URLs, which `cargo audit` reported for 2.5.8).
- WebAuthn ceremonies are bound to the operation the server chose; an enrolled credential can no longer be replaced through a protected read.
- The payload reader refuses JSON nested deeper than 64 levels (stack exhaustion).
- The web server bounds live sessions (`WebApp::max_sessions`, now 1024 by default), message size (`WebApp::max_message_size`, 8 MiB) and the bytes queued per session (`WebApp::max_queued_bytes`, 32 MiB), and closes sockets that stop answering pings.
- The web server serves only portable asset names from inside the assets folder (no hidden files, no links out of it, no Windows drive, stream or device names) and stores uploads under generated names.
- `prism-push` sends Web Push only to public HTTPS endpoints, follows no redirects, ignores proxy environment variables and caps error answers; `Sender::trust_origin` and `Sender::proxy` opt in to others.
- Pictures and videos with headers are fetched by the web server from public addresses (or origins the app trusts) only, without following redirects to other origins (SSRF).
- Android web views take messages from the page's main frame only (an embedded frame could post as the page).
- Links, `open_url` and `web_auth` on the web never use a `javascript:`, `vbscript:`, `data:`, `blob:` or `filesystem:` URL (script injection through link targets).
- Uploaded files are served sandboxed and with `nosniff`; only raster pictures and videos display inline (active content in an upload). The strict CSP no longer replaces the sandbox of uploads, proxied pictures and downloads.
- Over HTTPS the browser's id is a host-only `Secure` cookie (`__Host-prism_sid`) from the system CSPRNG (session fixation, cleartext cookie).
- Signing in on the web gives the browser a new id and moves its data there in one rename, so an id planted before the sign-in opens an empty session (session fixation). OAuth and OpenID Connect sign-ins (`web_auth`) and passkeys of the app's relying party (`webauthn_get`, `webauthn_create`, `webauthn_autofill`) do it before the app sees their outcome; the new `AsyncApp::renew_browser_id` does it for an app's own sign-in (`Ok(())` at once natively). The browser's other live pages, and any page still on the old id, reconnect once the browser has the new one.
- Socket origins are compared whole (scheme, host, port), duplicate `Origin` fields are refused, and the built-in server on a loopback address refuses unknown host names (DNS rebinding). New `WebApp::public_origin` / `PRISM_PUBLIC_ORIGIN` for servers behind a proxy.
- Web messages from an origin the web view does not allow are dropped by the app itself, whatever the platform layer reports.
- Clicks, typing and changes of disabled or unmounted controls, values a control could not produce (an index past its options, a toggle other than 0/1, non-finite numbers) and layout/scroll reports of nodes that do not report them are dropped; slider values are snapped into range and date picker values clamped to their bounds.
- A web sign-in can be cancelled only by the session that started it; waiting sign-ins expire after 10 minutes, are capped per session and server-wide, and end with their session.
- The page can no longer complete a `set_cookie` or `web_auth` call itself; they complete only from the server, and a page reply can only report a failure.
- Web uploads are checked against new limits before anything is stored (files per pick, bytes kept per browser, bytes received server-wide), hold no open files between chunks, bound encoded chunks before decoding, and expire after `keep_uploads` with their path mappings.
- OpenID Connect sign-ins require an ID token and check its issuer, audience (and `azp`), expiry, subject, nonce and signing algorithm; refreshed ID tokens are checked too, and discovery documents must name their issuer.
- OAuth token, refresh, revocation and registration requests follow no redirect, issuer and endpoints must use HTTPS (plain HTTP only to localhost), and metadata and token answers are capped at 1 MiB.
- Widget-action deep links run only with a token signed by the app, so other apps and web pages cannot trigger widget actions.
- Widget-action deep links (macOS widget buttons) run once: each token is bound to one drawing of the widgets, a link opened again does nothing, and a link whose drawing the app replaced over an hour ago has expired. The used links are kept next to the key, both in a `widget-links` folder of the app's data directory, out of the app's storage (`widget-link-ledger`), so a captured link cannot be replayed after a restart either, while the first tap on a widget drawn days ago still works. `App::set_widget_key` takes the store of that ledger.
- Web components reject invalid tag names, and their props can no longer set event handlers, markup, styles or Prism's own attributes and state.
- The web server reads pictures and videos with headers within size limits (`WebApp::max_picture_bytes`), checks partial answers against the range it asked for, and keeps their URLs' queries out of its log.
- Biometric secrets are stored only where the store itself checks the owner; elsewhere `set_with` fails with `Unavailable` and keeps the previous secret, and the new `SecureOptions::confirm_owner` opts into the app's own check.
- Windows: biometric secrets are sealed in Credential Manager with a key only Windows Hello gives back (an app's Hello key signs a random challenge, deterministically; the signature derives the AES-256-GCM key), so another program of the user can no longer read them without a Hello prompt. Storing and reading them ask for Hello; without Hello set up, `set_with` fails with `Unavailable` and keeps the previous secret. `confirm_owner` secrets are unchanged. Linux keeps `Unavailable` for biometric secrets: no desktop store ties a secret to the user's presence (see the security guide).
- `prism build linux` checks the package, bundle ID, build and one-line display strings before building, and the Linux launcher holds only quoted identifiers (shell injection from package metadata).
- Build-time downloads (Windows App SDK packages, Android Maven libraries, bundletool) are pinned by SHA-256 and checked before unpacking and on cache reuse (supply-chain integrity).
- Web `secure_storage` creates its key once for all workers, writes key and secret files private from creation, refuses an invalid `PRISM_SECRET_KEY`, and reports failed writes and deletes (key disclosure, false success).
- `AuthRequest::biometrics_only()` fails with `Unavailable` where the system check may accept a PIN or password instead (web, Windows Hello, watchOS).
- The web push service worker tells only the app's own pages (controlled windows under its mount) about pushes and clicks, gives a click to the window it opened, and opens only links inside the app.
- Web 3D scene blobs (`/_prism/3d/<id>`) go only to the browsers whose pages were sent them, with `Cache-Control: private`, unless the app opts in with `WebApp::public_scenes(true)`; first-paint pins are bounded in count and bytes.
- UnifiedPush receiver keys and the Linux registration token are made once (concurrent first uses share them), private from creation (0600/0700), and a failure to keep them fails `register_for_push` instead of announcing an unusable subscription.
- Native web views: the app's messages reach a page only if it still shows the origin it showed when they were sent.
- Browser sign-in ends only with a redirect to exactly its redirect URI carrying its `state` once; other requests and deep links are refused while it waits.
- Linux and Windows keep one loopback listener for sign-ins: `OAuth::resolve` (and `refresh`, which calls it) no longer leaves a listening socket open until the app quits. The next sign-in takes the waiting listener, and one nobody takes closes after 10 minutes.
- Android: a refused sign-in deep link no longer brings the app forward, which closed the Custom Tab where the user was signing in; the tab stays open and the sign-in goes on.
- Debug output of HTTP requests and responses, OAuth tokens and registrations, Apple credentials and push subscriptions hides credentials; HTTP and push errors leave out URL queries, endpoints and control characters.
- prism-web: pages and the sign-in callback can be framed only by the app's own origin by default (`WebApp::frame_ancestor` adds trusted sites); every response is `nosniff`; the strict CSP is added alongside a response's own policy.
- Android: installing an App Bundle on a device signs its APKs with the debug key, so the store key's passwords are never written to disk; `prism publish --service-account` takes a path only.
- prism-cli: App Store, notarization and device checks keep their temporary files in a private, randomly named directory.
- prism-db: `Db::open` takes a file name only (a path is refused; `Db::open_path` takes paths), and without a data directory uses a directory private to the user.
- Sign in with Apple: the nonce is sent to Apple hashed (SHA-256, hex), as documented, and a credential with another request's `state` is refused.
- `prism publish` (Google Play) and the Android bundle build refuse a bundle with unsigned entries or several signers.
- Android and Windows web views open a new window outside the app only after a tap, and Android only for web, mail and phone links (Windows: seen on screen, see Fixed).
- prism-db bundles SQLite 3.54.1 (sqlx 0.9, libsqlite3-sys 0.40): a database opened with a crafted `-wal` file can no longer return stale process memory (CVE-2026-119375, RUSTSEC-2026-0459). prism-db sets libsqlite3-sys 0.39 as a floor, so an app's lock file cannot keep an older SQLite, and does not build against a SQLite before 3.54.
- prism-push's elliptic-curve arithmetic (Web Push encryption, VAPID and APNs signing) runs on crypto-bigint 0.7.11, whose constant-time operations no longer branch on secrets (RUSTSEC-2026-0387; it was 0.5.5).
