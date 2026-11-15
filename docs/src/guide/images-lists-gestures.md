# Images, long lists, gestures

```rust
picture("https://…/photo.jpg").alt("A river").fill()   // cached on disk; or picture("logo.png") from assets/
picture_data(png_bytes, "image/png").alt("Avatar")      // bytes in memory (a data: URL)
svg(server_made_svg).alt("Pairing code").w(px(160.))    // SVG markup, drawn as vectors
ui::qr_code("https://example.com/pair/42").size(180.)    // a QR code, no image involved
VirtualList::new(10_000, 56., |i| row(i))               // only the visible rows exist
scroll().on_refresh(handler).refreshing(self.loading)   // pull to refresh (UIRefreshControl)
div().on_swipe(SwipeDirection::Left, handler)           // swipe gestures
```

## Pictures from memory, SVG and QR codes

- **`picture_data(bytes, mime)`** shows image bytes the app already has (from a database, an HTTP response, a camera). They travel as a `data:` URL and are decoded by the platform: `<img>` on the web, `UIImage(data:)` (iOS, tvOS, visionOS, watchOS), `NSImage(data:)` (macOS), `BitmapFactory.decodeByteArray` (Android), `GdkTexture` (Linux). Formats are what each platform decodes: PNG, JPEG and GIF everywhere; WebP and HEIC where the OS reads them. Keep them small (they are resent when they change, and kept in memory).
- **`svg(markup)`** draws an SVG document with Prism's vector shapes (the same machinery as `shape()` and the charts), so it is native on every backend and in widgets, and needs no SVG renderer on the device. Neither `UIImage(data:)` nor `NSImage(data:)` documents SVG support (the SDK headers never mention it; Android and GTK need extra libraries), so this is what works on iOS 17, macOS 13 and everywhere else. `picture_data(bytes, "image/svg+xml")` takes this path too. The picture is as wide as the SVG's `width` (or `viewBox`) and keeps its proportions (`aspect_ratio`): set `.w(…)` and the height follows.
  - Supported: `path` (every command, arcs included), `rect` (rounded), `circle`, `ellipse`, `line`, `polyline`, `polygon`, `g`, `transform`, fills and strokes with opacity, round caps and joins, one dash pattern, `fill-rule="evenodd"`, `display`/`visibility`, presentation attributes and `style="…"`, hex / `rgb()` / named colors, `currentColor`.
  - Not supported (skipped): `<text>`, gradients and patterns (their fallback color is used, if any), filters, masks, clip paths, `<use>`, `<style>` sheets. Stroke widths are converted at the SVG's own size.
- **`ui::qr_code(text)`** encodes `text` (byte mode, UTF-8; versions 1 to 40, the smallest that fits) and draws the modules as one shape, merged into rectangles. `.ecc(QrEcc::Low | Medium | Quartile | High)` (default `Medium`), `.size(pt)` (default 160, quiet zone included), `.quiet_zone(modules)` (default 4), `.colors(dark, light)` (default black on white, in dark mode too: scanners read dark on light), `.alt("Pairing code")`. Text over a version 40 code's capacity (2 953 bytes at `Low`) draws an empty square. The encoder alone is `prism_ui::qr::QrCode` (`dark(x, y)`, `dark_runs()`), checked by decoding its output with a QR reader in the tests, across every version and level.
