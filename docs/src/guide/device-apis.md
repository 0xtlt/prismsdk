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

`cx.capabilities()` says what exists (`location`, `motion`, `camera`, `photos`, `haptics`, `share`); unsupported calls fail with `DeviceError::Unavailable`.
