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
  with the App Group by `prism`.
- **Device APIs:** CoreLocation, CoreMotion (device only), PHPicker and the
  camera, haptics, the share sheet ([Device APIs](../guide/device-apis.md)).
- **Text:** Dynamic Type is followed live ([Languages, text size](../guide/i18n.md)).
- **Background:** `beginBackgroundTask` and `BGTaskScheduler`; the simulator
  cannot run `BGTaskScheduler` ([Background execution](../guide/background.md)).

The Swift bridge is compiled by `prism-ios`'s build script (Xcode required).
The deployment target is iOS 17 (`IPHONEOS_DEPLOYMENT_TARGET`). The same
backend serves [tvOS](tvos.md) and [visionOS](visionos.md).

Development hooks: `PRISM_CLICK="+1|Save"` presses by title or label,
`PRISM_SNAPSHOT=/abs/out.png` saves the window and quits.
