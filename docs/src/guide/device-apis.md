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

| | iOS | macOS | Web | Android | Linux | Windows |
|---|---|---|---|---|---|---|
| Permissions | system prompts (location, camera, photos, microphone, motion, notifications) | same (bundled app) | Permissions API, `getUserMedia` | runtime permissions (`requestPermissions`) | — | Settings › Privacy & security, read without asking; a location prompt (Windows 11 24H2 and later) |
| Location | CoreLocation | CoreLocation (bundled app) | `navigator.geolocation` | `LocationManager` | GeoClue2 | `Geolocator` |
| Motion | CoreMotion (device only) | — | `devicemotion` | `SensorManager` | — | `Windows.Devices.Sensors` (tablets, 2-in-1s) |
| Photos / camera | PHPicker (no permission needed) / camera | open panel / — | `<input type=file>`, uploaded to the session | photo picker (`ACTION_PICK_IMAGES`) / `ACTION_IMAGE_CAPTURE` | file chooser / — | `IFileOpenDialog` / the Camera app's capture UI |
| Haptics, clipboard, share, URLs | UIFeedbackGenerator, UIPasteboard, share sheet | trackpad, NSPasteboard, sharing picker | `vibrate`, Clipboard API, Web Share | `VibrationEffect`, `ClipboardManager`, `ACTION_SEND` chooser | —, the display's clipboard, —, the default app | a vibration device (most PCs have none), the Win32 clipboard, the Share dialog, `ShellExecuteW` |

On iOS, iPadOS, visionOS and watchOS the photo picker (`pick_images`) needs prismsdk's `photo-picker` feature (`features = ["photo-picker"]`): it links PhotosUI, which costs ~1 s at every launch in the simulator (#314). Without it `pick_images` fails with a message naming the feature and `capabilities().photos` is false there; the macOS open panel needs nothing. `take_photo` (and `PickImages::camera`) needs `camera` under `[permissions]` (below).

On Windows every one works in an unpackaged app; [Windows › Device APIs without a package](../platforms/windows.md#device-apis-without-a-package) says how, and what `capabilities()` checks at startup.

Usage texts go in `Prism.toml`; `prism` writes them to Info.plist (iOS refuses to ask without them) and the Android manifest:

```toml
[permissions]
location = "Shows todos near you."
camera = "Adds a photo to a todo."
```

On Apple platforms each one also compiles in the code that asks for it: App Store Connect wants a purpose string for every protected API the binary references (ITMS-90683), so Prism's location, camera, microphone, photo library and Face ID code is left out of apps that don't declare them. `prism` turns on prismsdk's matching feature (`location`, `location-always`, `camera`, `microphone`, `photo-library`, `face-id`); a build without `prism` lists them in `Cargo.toml`. Undeclared, the call fails with a message naming the `[permissions]` entry, and `capabilities()` says the resource is unavailable. [`Prism.toml` › Permissions](../cli.md#permissions) lists every name.

## Images and other types on the clipboard

`cx.read_clipboard()` and `cx.copy_text()` move text. In a task, `cx.copy_text(text).await` says whether it worked (#323): `Err(DeviceError::Unavailable)` on tvOS and watchOS, the platform's error otherwise.

On the web, browsers accept a clipboard write only during a click (Safari refuses one that arrives later, which a write from the server does). A button that copies writes in the click itself with `copy_on_click`, on `div`, `button` and `text`; every platform hears the result:

```rust
button("Copy link")
    .copy_on_click(url.clone())
    .on_copied(cx.listener(|this, e: &CopiedEvent, cx| {
        this.notice = if e.result.is_ok() { "Copied" } else { "Could not copy" };
        cx.notify();
    }))
```

The page writes the text before the click reaches the app (`on_click` still runs); elsewhere the app writes it when the click arrives.

 `cx.read_clipboard_item()` and `cx.write_clipboard()` move everything the clipboard holds, in every type at once (#246): a screenshot pasted as an image block, HTML from a browser, the files copied in the file manager, the app's own format next to text for other apps.

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
| Windows | `CF_UNICODETEXT`, `HTML Format`, `Rich Text Format`, `PNG` else a DIB (as PNG), `CF_HDROP` (as `text/uri-list`), formats named like MIME types | every type, the image also as `CF_DIBV5`, file URIs also as `CF_HDROP` |
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
| Windows | `IFileSaveDialog` over the window (Windows asks before replacing) | the saved path | Explorer, file selected (`SHOpenFolderAndSelectItems`) |
| iOS, iPadOS, visionOS | "Save to Files" (`UIDocumentPickerViewController` exporting a copy) | the exported copy's path | nothing |
| Android | the storage picker (`ACTION_CREATE_DOCUMENT`), then the file is copied into the document | the name, no path (a provider's document) | nothing |
| Web | a download (`<a download>`) of `<base>/_prism/download/<id>` | the name, once the download starts | nothing |
| tvOS, watchOS | `DeviceError::Unavailable` | | nothing |

- The name is a file name: path separators and control characters become `_`. The MIME type comes from `.mime(…)`, else from the extension.
- Natively, bytes (or a file to save under another name) are first written to a temporary folder, removed once the platform is done.
- On the web the server keeps the bytes and the page downloads them from its own origin, so no header or token reaches the browser. The URL is random and bound to the session (another browser gets a 404). Files are kept 10 minutes (the last 64, 256 MiB at most). The answer only says the download started: the browser decides the folder, and may ask.

`cx.capabilities()` says what exists (`location`, `motion`, `camera`, `photos`, `haptics`, `share`); unsupported calls fail with `DeviceError::Unavailable`. A failure the platform names is `DeviceError::Platform { name, message }` (#189): match `e.name()`, show `e` (its message).
