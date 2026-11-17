# Images, long lists, gestures

```rust
picture("https://…/photo.jpg").alt("A river").fill()   // cached on disk; or picture("logo.png") from assets/
picture_data(png_bytes, "image/png").alt("Avatar")      // bytes in memory (a data: URL)
picture(avatar_url).header("Authorization", format!("Bearer {token}"))  // behind a token
svg(server_made_svg).alt("Pairing code").w(px(160.))    // SVG markup, drawn as vectors
ui::qr_code("https://example.com/pair/42").size(180.)    // a QR code, no image involved
VirtualList::new(10_000, 56., |i| row(i))               // only the visible rows exist
ReorderList::new(cx, keys, 52., row, on_move)           // rows the user puts in another order
scroll().on_refresh(handler).refreshing(self.loading)   // pull to refresh (UIRefreshControl)
div().on_swipe(SwipeDirection::Left, handler)           // swipe gestures
```

`picture("logo.png")` names an app asset: a file of the assets folder
(`Application::assets`, copied into native bundles), or one compiled into the
binary with `Application::embedded_assets`, found first (web, macOS, iOS and
Linux; Android and watchOS read the bundle's folder only). See
[Assets compiled into the binary](../platforms/web.md#assets-compiled-into-the-binary).

## Lists the user reorders

`ReorderList` is a column of rows of a fixed height that the user puts in another order (#168): a task list, a playlist, the widgets of a dashboard. Each row gets a handle (three bars) at its trailing edge. Dragging the handle lifts the row and the others make room. With the focus on the handle (Tab, a click), ↑ and ↓ move the row by one, and Home and End move it to the top or the bottom. The rest of the row is the app's own: its clicks, its scrolling.

```rust
let list = ReorderList::new(
    cx,
    tasks.iter().map(|t| t.id.to_string()), // one unique key per row
    52.,                                    // each row's height, in points
    move |i, app| task_row(&store.read(app).tasks[i]),
    move |from, to, app| store.update(app, |s, cx| {
        let task = s.tasks.remove(from);    // the list already shows the new order:
        s.tasks.insert(to, task);           // reorder the data the same way
        cx.notify();
    }),
);
div().child(list)                                  // in render
list.update(cx, |l, cx| l.set_keys(new_keys, cx)); // rows added or removed
```

- `on_move(from, to, app)` runs once the row is dropped, or after each arrow key. A drag the platform takes over (a scroll, a system gesture) moves nothing.
- While a row is dragged, `dragging()` gives `(from, to)`, where it would go if dropped now, and `handle_id(i)` names row `i`'s handle for `cx.focus`.
- The dragged row is drawn as a lifted copy with a shadow, so give rows a background.
- Screen readers hear each handle as a "Reorder" button with the row's position ("2 of 5") and a hint about the arrow keys, in the app's language (`prism.reorder`, `prism.reorder_hint` and `prism.reorder_position` in the app's translations override them).
- It is built on Prism's drag and key events, so it works wherever they do: the web, macOS, iOS, iPadOS, visionOS, Android and Linux. tvOS has no drag, but the keys still move rows there.

`ReorderList::with_options` lays it out differently (#201): rows as tall as their content, and the app's own handle at either edge.

```rust
ReorderList::with_options(
    cx,
    keys,
    ReorderOptions::default()                                   // rows follow their content
        .handle_leading()                                       // the handle before the row
        .handle(|_, _| icon("line.3.horizontal").text_color(rgb(0x8e8e93))),
    render_row,
    on_move,
)
```

- `ReorderOptions::row_height(h)` gives every row the same height, like `ReorderList::new`. Without it, each row is as tall as its content: the list measures the rows after layout (`on_layout`, #192) and places the drop slot and the rows making room by their real heights. A row is passed once its leading edge (its bottom going down, its top going up) reaches the row's middle.
- `.handle(|i, app| …)` replaces the three bars with the app's content (at least 44 points wide, for touch). Prism keeps its button role, its label, position and hint, the arrow keys and the drag around it. `.handle_leading()` puts it at the leading edge.

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
