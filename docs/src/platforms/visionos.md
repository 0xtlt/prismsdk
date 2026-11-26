# visionOS

> **Preview in 0.1:** it works and can be tried, but has gaps (listed on this page) and is less tested on real devices than web, macOS, iOS and Android. See [Platforms in 0.1](../introduction.md#platforms-in-01).

`prism run visionos` (simulator) / `prism build visionos`: the UIKit backend in a window, like an iPad app. `cx.platform()` is `Platform::Visionos` and `cx.device_info().kind` is `Headset`; `Platform::SPATIAL` selects it in `.only()`.

- Eyes and pinches arrive as touches, so controls, gestures and menus work as on iOS. Rich inputs are `UITextView`s with styles and chips, as on iOS.
- Widgets (WidgetKit, visionOS 26+): the minimum version is 26 (`XROS_DEPLOYMENT_TARGET`, or `[visionos] deployment_target` in `Prism.toml`). Apps that register no widget are built without the extension and its App Group; widgets with a refresh hook run it in the extension as on iOS.
- Not on visionOS (unavailable in the SDK): `UIScreen` (so the window gives the size and scale), haptics, the camera for apps, Live Activities (no ActivityKit), lock-screen widget families, and "always" location.
- The simulator runtime comes from Xcode › Settings › Components, or `xcodebuild -downloadPlatform visionOS`.
