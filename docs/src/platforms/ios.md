# iOS and iPadOS

`prism dev ios` (the simulator, restarting on save), `prism run ios --device
"My iPhone"` (a real device), `prism build ios`.

Every node is a real UIKit view (`UIView`, `UILabel`, `UIButton`,
`UITextField`), laid out by prism-native's flexbox. The app uses the UIScene
lifecycle; `prism` writes the Info.plist (scene delegate, usage texts,
languages, background tasks) and bundles the app.

- **Simulators:** `--sim <name|udid>`, `PRISM_SIM` or `[ios] simulator` in
  `Prism.toml`. Simulators are slimmed with `simslim` when it is installed.
- **Devices:** signed with your Apple Development identity and a matching
  provisioning profile (found automatically, or `PRISM_PROFILE`), then
  installed with `devicectl`. Set `[ios] team` in `Prism.toml`.
- **Windows:** rotation, Split View, Stage Manager and the iPad sizes re-render
  across [breakpoints](../guide/responsive.md); screens shrink above the
  keyboard and the focused field stays visible.
- **Navigation:** `UINavigationController` (large titles, edge swipe) and
  `UITabBarController` ([Navigation](../guide/navigation.md)).
- **System surfaces:** WidgetKit widgets, Live Activities with the Dynamic
  Island, notifications with actions, and CarPlay
  ([Widgets, Live Activities, notifications](../guide/system-surfaces.md),
  [In the car](../guide/car.md)). The widget extension is generated and signed
  with the App Group by `prism`, for apps that use it (see below).
- **Device APIs:** CoreLocation, CoreMotion (device only), PHPicker and the
  camera, haptics, the share sheet ([Device APIs](../guide/device-apis.md)).
- **Text:** Dynamic Type is followed live ([Languages, text size](../guide/i18n.md)).
- **Background:** `beginBackgroundTask` and `BGTaskScheduler`; the simulator
  cannot run `BGTaskScheduler` ([Background execution](../guide/background.md)).

The Swift bridge is compiled by `prism-ios`'s build script (Xcode required).
The same backend serves [tvOS](tvos.md) and [visionOS](visionos.md).

**Deployment target.** Apps are built for iOS and iPadOS 17 and later. `prism`
sets `IPHONEOS_DEPLOYMENT_TARGET` for every `cargo` it runs, and writes it as
the bundle's `MinimumOSVersion`. To change it, set it in `Prism.toml`:

```toml
[ios]
deployment_target = "18.0"
```

The environment wins when it sets `IPHONEOS_DEPLOYMENT_TARGET` itself. The
other platforms work the same way: `[tvos]` (default 17.0, `TVOS_…`),
`[watchos]` (10.0, `WATCHOS_…`), `[visionos]` (26.0, `XROS_…`) and `[macos]`
(13.0, `MACOSX_…`).

Plain `cargo build --target aarch64-apple-ios-sim` (without `prism`) gets the
same target from `prism_build::link_app()` in the app's `build.rs` (which
`prism new` writes), unless `IPHONEOS_DEPLOYMENT_TARGET` is set: put it in
`.cargo/config.toml` under `[env]` to use another one. `link_app` also adds
`-rpath /usr/lib/swift` to the app's binary. Cargo applies a dependency's
link arguments only to that dependency, so Prism's backends cannot add the
rpath themselves. Without a deployment target, rustc links for iOS 14, where
Swift Concurrency is an `@rpath` library the app does not ship, and the app
stops at launch with `Library not loaded: @rpath/libswift_Concurrency.dylib`.

**Widget extension.** `prism` builds, signs and embeds the extension
(`PlugIns/PrismWidgets.appex`), with its provisioning profile
(`<bundle id>.widgets`) and the App Group, only for apps that register a
widget or declare Live Activities (`cx.enable_live_activities()`, or a
push-to-start handler `cx.on_activity_push_token`). The others are built
without them and keep their data in the app's own container.

Development hooks: `PRISM_CLICK="+1|Save"` presses by title or label,
`PRISM_SNAPSHOT=/abs/out.png` saves the window and quits.
