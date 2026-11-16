# Images, long lists, gestures

```rust
picture("https://…/photo.jpg").alt("A river").fill()   // cached on disk; or picture("logo.png") from assets/
picture_data(png_bytes, "image/png").alt("Avatar")      // bytes in memory (a data: URL)
picture(avatar_url).header("Authorization", format!("Bearer {token}"))  // behind a token
svg(server_made_svg).alt("Pairing code").w(px(160.))    // SVG markup, drawn as vectors
ui::qr_code("https://example.com/pair/42").size(180.)    // a QR code, no image involved
VirtualList::new(10_000, 56., |i| row(i))               // only the visible rows exist
scroll().on_refresh(handler).refreshing(self.loading)   // pull to refresh (UIRefreshControl)
div().on_swipe(SwipeDirection::Left, handler)           // swipe gestures
```

## Pictures behind a token

`picture(url).header(name, value)` (any number of headers) loads a remote image that needs credentials, such as a per-user rendering behind the API's bearer token:

| | Loaded by | Cache |
|---|---|---|
| iOS, iPadOS, tvOS, visionOS, macOS | Prism's HTTP client (rustls), off the main thread | memory only (never the on-disk image cache) |
| watchOS | `URLRequest` with the headers on an ephemeral `URLSession` | memory only |
| Android | `HttpURLConnection.setRequestProperty`, off the main thread | `setUseCaches(false)` |
| Linux | Prism's HTTP client on a GIO worker thread, decoded by `GdkTexture` | none |
| Web | **the server**: the page gets `<base>/_prism/picture/<id>` | `private, max-age=300` |

On the web the app runs on the server, so the server fetches the image with the headers and streams it to the page; the token never reaches the browser (not in the HTML, the socket or JavaScript). The `<id>` is a keyed hash (random key per server process) of the session and the source: it reveals nothing about the token, and it is bound to the session that rendered it, so another browser gets a 404 even with the URL. Only `image/*` answers are passed on, with `X-Content-Type-Options: nosniff` and a sandboxing `Content-Security-Policy`. The server remembers the last 4096 such sources (an older one loads again once its page re-renders). Headers never appear in logs. They are ignored for assets, files, `data:` URLs and `svg()`; line breaks in names or values are dropped. Widgets show no remote pictures.

## Pictures from memory, SVG and QR codes

- **Loading background**: on the web a picture shows a light gray box while it loads, and drops it once loaded, so a transparent PNG or logo is drawn over its parent, as natively.
- **`picture_data(bytes, mime)`** shows image bytes the app already has (from a database, an HTTP response, a camera). They travel as a `data:` URL and are decoded by the platform: `<img>` on the web, `UIImage(data:)` (iOS, tvOS, visionOS, watchOS), `NSImage(data:)` (macOS), `BitmapFactory.decodeByteArray` (Android), `GdkTexture` (Linux). Formats are what each platform decodes: PNG, JPEG and GIF everywhere; WebP and HEIC where the OS reads them. Keep them small (they are resent when they change, and kept in memory).
- **`svg(markup)`** draws an SVG document with Prism's vector shapes (the same machinery as `shape()` and the charts), so it is native on every backend and in widgets, and needs no SVG renderer on the device. Neither `UIImage(data:)` nor `NSImage(data:)` documents SVG support (the SDK headers never mention it; Android and GTK need extra libraries), so this is what works on iOS 17, macOS 13 and everywhere else. `picture_data(bytes, "image/svg+xml")` takes this path too. The picture is as wide as the SVG's `width` (or `viewBox`) and keeps its proportions (`aspect_ratio`): set `.w(…)` and the height follows.
  - Supported: `path` (every command, arcs included), `rect` (rounded), `circle`, `ellipse`, `line`, `polyline`, `polygon`, `g`, `transform`, fills and strokes with opacity, round caps and joins, one dash pattern, `fill-rule="evenodd"`, `display`/`visibility`, presentation attributes and `style="…"`, hex / `rgb()` / named colors, `currentColor`.
  - **Tinting.** `currentColor` (with no `color` attribute on the SVG) is [`Color::current()`](decorations.md): the text color of the node the SVG is drawn in, so `svg(icon).text_color(t.muted)` tints a monochrome icon and an adaptive text color (`adaptive(light, dark)`) follows dark mode, on every backend and in widgets. Without a `text_color` it is the platform's label color natively (black, white in dark mode) and the inherited text color on the web. `svg(icon).tint(color)` recolors every painted path of an SVG that hard-codes its color, each path keeping its own opacity (`Color::current()` works there too).
  - Not supported (skipped): `<text>`, gradients and patterns (their fallback color is used, if any), filters, masks, clip paths, `<use>`, `<style>` sheets. Stroke widths are converted at the SVG's own size.
- **`ui::qr_code(text)`** encodes `text` (byte mode, UTF-8; versions 1 to 40, the smallest that fits) and draws the modules as one shape, merged into rectangles. `.ecc(QrEcc::Low | Medium | Quartile | High)` (default `Medium`), `.size(pt)` (default 160, quiet zone included), `.quiet_zone(modules)` (default 4), `.colors(dark, light)` (default black on white, in dark mode too: scanners read dark on light), `.alt("Pairing code")`. Text over a version 40 code's capacity (2 953 bytes at `Low`) draws an empty square. The encoder alone is `prism_ui::qr::QrCode` (`dark(x, y)`, `dark_runs()`), checked by decoding its output with a QR reader in the tests, across every version and level.
