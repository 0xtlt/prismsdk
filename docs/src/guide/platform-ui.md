# Platform-specific UI

```rust
text("Swipe left to delete").only(Platform::MOBILE)       // iOS, Android
text("Right-click for more").except(Platform::MOBILE)     // macOS, Windows, Linux, web
apple_pay_button().only(Platform::APPLE)
div().p(16.).on(Platform::Web, |s| s.p(24.)).md(|s| s.flex_row())   // combine with breakpoints

match cx.platform() {
    Platform::Ios | Platform::Android => mobile_onboarding(),
    Platform::Web => landing_page(),
    _ => desktop_welcome(),
}
```

Platforms: `Ios`, `Android`, `Macos`, `Windows`, `Linux`, `Web`; sets with `|` or `Platform::MOBILE`, `DESKTOP`, `APPLE`, `NATIVE` (all but web). Excluded elements are left out of the tree (no hidden view). `cx.platform()` is the runtime platform (`Web` for the web backend, even on a phone browser: use breakpoints for screen size); `prism::PLATFORM` is the same at compile time. For code that only compiles on one platform, use `cfg`: `#[cfg(target_os = "ios")]`, `#[cfg(feature = "web")]` (your app's feature), or per-platform files with `#[cfg_attr(target_os = "ios", path = "share_ios.rs")]`.

## Desktop window chrome

`Application::window_chrome(WindowChrome::Custom { traffic_lights })` removes
the macOS title bar so the app draws its own; `.window_drag()` marks the
regions that move the window, and `cx.window_controls()` / `cx.is_fullscreen()`
say how much room the traffic lights need. Other platforms ignore them. See
[macOS › Custom window chrome](../platforms/macos.md#custom-window-chrome).
