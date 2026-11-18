# Device APIs

```rust
cx.spawn(async move |this, cx| {
    if cx.request_permission(Permission::Location).await.is_granted() {
        let here = cx.current_location().await?;            // Location { latitude, longitude, accuracy, … }
    }
    let photos = cx.pick_images(PickImages::new().limit(3)).await?;  // picture(photo.path) shows them
    let photo = cx.take_photo().await?;
    let text = cx.read_clipboard().await;
});
let gyro = cx.watch_motion(Duration::from_millis(50), |m, app| { /* m.rotation_rate, m.attitude */ });
let follow = cx.watch_location(Accuracy::Best, |l, app| { … });
cx.stop_watching(gyro);
cx.haptic(Haptic::Success);   cx.copy_text("Hi");   cx.share(Share::text("Look").url(url));   cx.open_url(url);
```

On the web `open_url` keeps a URL of the page's own origin in the tab (the host's sign-out page, a download) and opens other origins in a new tab; `cx.open_url_in(url, UrlTarget::SameTab)` / `NewTab` chooses. Natively the system opens it either way.

| | iOS | macOS | Web |
|---|---|---|---|
| Permissions | system prompts (location, camera, photos, microphone, motion, notifications) | same (bundled app) | Permissions API, `getUserMedia` |
| Location | CoreLocation | CoreLocation (bundled app) | `navigator.geolocation` |
| Motion | CoreMotion (device only) | — | `devicemotion` |
| Photos / camera | PHPicker (no permission needed) / camera | open panel / — | `<input type=file>`, uploaded to the session |
| Haptics, clipboard, share, URLs | UIFeedbackGenerator, UIPasteboard, share sheet | trackpad, NSPasteboard, sharing picker | `vibrate`, Clipboard API, Web Share |

Usage texts go in `Prism.toml`; `prism` writes them to Info.plist (iOS refuses to ask without them):

```toml
[permissions]
location = "Shows todos near you."
camera = "Adds a photo to a todo."
```

## Images and other types on the clipboard

`cx.read_clipboard()` and `cx.copy_text()` move text. `cx.read_clipboard_item()` and `cx.write_clipboard()` move everything the clipboard holds, in every type at once (#246): a screenshot pasted as an image block, HTML from a browser, the files copied in the file manager, the app's own format next to text for other apps.

```rust
cx.spawn(async move |this, cx| {
    let Some(item) = cx.read_clipboard_item().await else { return };
    if let Some(img) = item.as_image() {
        upload(img.bytes, img.mime);                    // image/png
    } else if let Some(block) = item.get("application/x-notes-block") {
        insert_block(block);                            // copied from this app
    } else if let Some(text) = item.as_text() { … }
    // item.types(), item.as_html(), item.files() (text/uri-list)
});

cx.write_clipboard(
    ClipboardItem::new()
        .text("Q3 revenue")                              // every app pastes this
        .html("<b>Q3</b> revenue")
        .image(png_bytes, "image/png")
        .custom("application/x-notes-block", json),     // only apps that know it
);
```

Types are MIME types. A type set twice keeps its last data; `as_image()` gives the PNG when there is one.

| | Reads | Writes |
|---|---|---|
| macOS | `NSPasteboard`: TIFF (screenshots) becomes PNG, a copied image file also gives its image | every type, a TIFF copy of the image for older apps |
| iOS, iPadOS, visionOS | `UIPasteboard` (the system may ask to allow pasting) | every type |
| Web | `navigator.clipboard.read()`, uploaded to the server like picked files; else the text | `text/plain`, `text/html`, `image/png` (other images converted), other types as the browser's `web …` custom formats where it has them; the text alone when it refuses |
| Android | the primary clip: text, HTML, an image behind a content URI | text, HTML and one image (through Prism's file provider) |
| Linux | GDK formats: text, the image as PNG, other MIME types | every type, the image also as a texture |
| tvOS, watchOS | `None` (no clipboard) | nothing |

On macOS and iOS custom types travel as dynamic UTIs that keep the MIME type, so they come back under their name. Browsers read clipboards only after a user gesture and may prompt.

## Saving a file the app holds

`cx.save_file` hands a file to the user with the platform's own UI (#210): bytes the app has (a PDF fetched with a bearer token, an export it generated) or a file of its own (in its cache). `cx.reveal_path` then shows it in the Finder or the file manager.

```rust
cx.spawn(async move |this, cx| {
    let pdf = fetch_report(&token).await?;                                 // bytes behind a token
    let saved = cx.save_file(SaveFile::bytes(pdf, "report.pdf")).await?;   // None: cancelled
    // or: SaveFile::path(cache_dir.join("q3.csv")).name("Q3.csv").mime("text/csv")
    if let Some(path) = saved.and_then(|s| s.path) {
        cx.update(|app| app.reveal_path(path));                            // "Show in Finder"
    }
});
```

| | `save_file` | Answers | `reveal_path` |
|---|---|---|---|
| macOS | `NSSavePanel` (a sheet on the key window): the user picks the name and folder | the saved path | the Finder, file selected |
| Linux | `GtkFileChooserNative` in save mode (the desktop portal) | the saved path | `org.freedesktop.FileManager1.ShowItems`, else the folder opens |
| iOS, iPadOS, visionOS | "Save to Files" (`UIDocumentPickerViewController` exporting a copy) | the exported copy's path | nothing |
| Android | the storage picker (`ACTION_CREATE_DOCUMENT`), then the file is copied into the document | the name, no path (a provider's document) | nothing |
| Web | a download (`<a download>`) of `<base>/_prism/download/<id>` | the name, once the download starts | nothing |
| tvOS, watchOS | `DeviceError::Unavailable` | | nothing |

- The name is a file name: path separators and control characters become `_`. The MIME type comes from `.mime(…)`, else from the extension.
- Natively, bytes (or a file to save under another name) are first written to a temporary folder, removed once the platform is done.
- On the web the server keeps the bytes and the page downloads them from its own origin, so no header or token reaches the browser. The URL is random and bound to the session (another browser gets a 404). Files are kept 10 minutes (the last 64, 256 MiB at most). The answer only says the download started: the browser decides the folder, and may ask.

`cx.capabilities()` says what exists (`location`, `motion`, `camera`, `photos`, `haptics`, `share`); unsupported calls fail with `DeviceError::Unavailable`. A failure the platform names is `DeviceError::Platform { name, message }` (#189): match `e.name()`, show `e` (its message).
