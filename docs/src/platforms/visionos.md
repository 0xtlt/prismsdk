# visionOS

`prism run visionos` (simulator) / `prism build visionos`: the UIKit backend in a window, like an iPad app. `cx.platform()` is `Platform::Visionos` and `cx.device_info().kind` is `Headset`; `Platform::SPATIAL` selects it in `.only()`.

- Eyes and pinches arrive as touches, so controls, gestures and menus work as on iOS.
- Widgets (WidgetKit, visionOS 26+): the minimum version is 26 (`XROS_DEPLOYMENT_TARGET`).
- Not on visionOS (unavailable in the SDK): `UIScreen` (so the window gives the size and scale), haptics, the camera for apps, Live Activities (no ActivityKit), lock-screen widget families, and "always" location.
- The simulator runtime comes from Xcode › Settings › Components, or `xcodebuild -downloadPlatform visionOS`.
