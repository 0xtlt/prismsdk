# Responsive

```rust
// Styles, from a width up (like Tailwind): phones get a column, tablets a row.
div().flex_col().gap(12.).md(|s| s.flex_row().gap(24.))
field.order(1).md(|s| s.order(-1))                        // visual order per breakpoint
text("Swipe to delete").hidden_from(Breakpoint::Medium)   // phones only
sidebar().visible_from(Breakpoint::Expanded)              // laptops and up

// Structure: anything else.
if cx.breakpoint() >= Breakpoint::Expanded { two_panes() } else { one_pane() }
cx.on_resize(|(w, h), app| { … });                        // every size change (and density change)
```

| Breakpoint | Width | Typically |
|---|---|---|
| `Compact` | < 600 | phones |
| `Medium` (`md`) | ≥ 600 | tablets portrait, small windows, phones in landscape |
| `Expanded` (`lg`) | ≥ 1024 | iPad landscape / 13", laptops |
| `Large` (`xl`) | ≥ 1440 | desktops |

Rules are resolved in the core for the current window, so every backend agrees and the web needs no media queries: the iOS window (rotation, Split View, iPad), the macOS window (resize) and the browser (resize, plus a cookie so the server's first render already fits) report their size, and the app re-renders when it crosses a breakpoint. On wide browser windows the tab bar becomes a sidebar.

## The content area and the safe area

```rust
let (w, h) = cx.viewport();        // what the root screen lays out in
let window = cx.window_size();     // the whole window, bars included
let safe = cx.safe_area();         // what the system covers, per edge (Edges)
```

`cx.viewport()` is the window minus what the system covers: the status bar
and home indicator, a notch or camera cutout, a foldable's side bar (the
iPhone Duo's trailing tab bar and clock), the room a root tab bar takes from
its screens (a bottom bar, an iPad sidebar). Breakpoints, `.md()`… and
`.at_width(…)` resolve against its width, so a layout computed from it fits
beside a side bar. `cx.safe_area()` gives the insets (top, right, bottom,
left; the on-screen keyboard is not part of them) and `cx.window_size()` the
whole window. They change with rotation, folding, Split View and resizes:
the app re-renders when the insets move, and `on_resize` gets the new
content size.

| Backend | Safe area |
|---|---|
| iOS, iPadOS, tvOS, visionOS | the root's `safeAreaInsets` (without the keyboard), plus what a root `UITabBarController` takes from its selected tab (a bottom bar, a sidebar or side bar) |
| macOS | the content view's `safeAreaInsets` (a full-size content view's title bar and toolbar; zero for a standard window) |
| Android | system bars and display cutouts (`WindowInsets`), the round-watch margins, a TV's overscan margins; the keyboard is not counted |
| watchOS | SwiftUI's safe area around the root (corners, the clock) |
| Web | `env(safe-area-inset-*)`, reported by the page on connect and resize (zero unless the page is laid out under the notch with `viewport-fit=cover`) |
| Linux | none: GTK windows have no system insets |
| Windows | none: desktop windows have no system insets |

## Pixel density

```rust
let (w, h) = cx.viewport();        // points (CSS pixels on the web)
let scale = cx.scale_factor();     // pixels per point: 2 on a Retina Mac, 3 on most iPhones
let png = render_canvas((w * scale).round() as u32, (h * scale).round() as u32);
picture_data(png, "image/png").w(w).h(h)
```

Content the app rasterizes itself (a chart, a canvas, a document drawn with
tiny-skia or resvg) is sharp when it is drawn at `size × cx.scale_factor()`
pixels and shown at `size` points: a Retina screen gets its 2× bitmap, a 1×
screen does not pay for one. It is 1 until the platform reports, and often
fractional (Windows at 125 or 150 %, Android, Linux with fractional scaling,
a zoomed browser). It changes when the window moves to another screen or the
browser zooms: the app re-renders and `on_resize` handlers run once, with the
new size and density (the same size if only the density changed).

| Backend | Scale factor | Changes |
|---|---|---|
| macOS | the window's `backingScaleFactor` | the window moves to another screen (`viewDidChangeBackingProperties`) |
| iOS, iPadOS, tvOS, visionOS | the trait collection's `displayScale` (times the TV zoom on tvOS) | an iPad's external display (trait changes) |
| watchOS | `WKInterfaceDevice.screenScale` (times the watch's zoom) | never |
| Android | `DisplayMetrics.density` (pixels per dp) | the display size setting, another display (configuration changes) |
| Web | `devicePixelRatio`, sent with the socket's handshake, the viewport reports and the `prism_dpr` cookie (the server takes 0.25 to 8) | browser zoom, another screen (a `resolution` media query) |
| Linux | the GDK surface's fractional `scale` on GTK 4.12 and later, else GTK's integer scale factor | another monitor, the desktop's scale (`notify::scale`, `notify::scale-factor`) |
| Windows | the window's DPI ÷ 96: what XAML rasterizes at (`XamlRoot.RasterizationScale`) | another monitor, the display's scale (`WM_DPICHANGED`) |

`cx.device_info().screen.scale` is the screen's own density, rounded up to
1 on the web; `cx.scale_factor()` is the window's, the one to render at.

On the web the viewport and the ratio come from the browser, so a modified
page can send anything: the server clamps each side of `cx.viewport()` to
16384 CSS pixels and takes ratios from 0.25 to 8. A bitmap of `viewport ×
scale_factor` can still be 131072 pixels a side: cap the size you allocate
(a chart rarely needs more than 4096 pixels a side).

## Your own breakpoints, any width

```rust
// The size classes move: md ≥ 480, lg ≥ 900, xl ≥ 1280.
Application::new().breakpoints([480., 900., 1280.]).run(…);   // or cx.set_breakpoints(…)

// Any threshold, without a size class: re-renders when the window crosses 768.
div().flex_col().at_width(768., |s| s.flex_row())
```

The app re-renders whenever the window crosses a breakpoint or a width an
`.at_width` rule uses (no `on_resize` + `refresh()` needed).

## An element's own width (container queries)

```rust
// A row once this card is 420 pt wide, a column below, whatever the window
// (a sidebar or a grid column can make it narrow on a wide screen).
card().when_width_at_least(420., |s| s.flex_row().items_center())
```

The rules are closures, so they run in Rust, but on the element's laid-out
width rather than the window's, and without asking the app to re-render:

- natively the layout pass knows every frame: after laying out, nodes whose
  width crossed one of their thresholds are restyled and laid out again in
  the same pass (at most twice), before anything is drawn;
- on the web the server sends the node's CSS for each threshold, and
  `client.js` applies the right one from a `ResizeObserver` (before paint, no
  round trip).

Trade-offs: a rule should change how the element arranges its content
(direction, gaps, padding, what is shown), not the element's own width,
which would make it flip back and forth (native backends stop after two
extra passes; browsers stop resize-observer loops). The element is first
laid out with no rule applied. There is no `on_layout` callback that hands
measured sizes back to the app: on the web it would cost a socket round trip
and a second render for every resize; structural choices that need the
window stay with `cx.viewport()` / `cx.breakpoint()`.
