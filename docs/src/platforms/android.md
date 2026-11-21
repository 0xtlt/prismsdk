# Android

`prism run android` / `prism dev android` / `prism build android`: an APK built with the SDK's own tools (`javac`, `d8`, `aapt2`, `zipalign`, `apksigner`), without Gradle, then installed with `adb` on the running emulator or a USB phone (`--device <serial>`; `prism devices` lists them).

The app runs in Rust, and a small Java host (`crates/prism-cli/platform/android`) creates real Android views and places them at the frames Rust computes.

- **Layout:** prism-native's flexbox in dp; the zoom is the screen density, and Java measures text. Navigation stacks get a header with Back (also the system back gesture), tabs a bottom bar, and modals a full-screen dialog.
- **Look:** light/dark follows the system or `cx.set_appearance` (the header, bars and system bars included), also when it changes while the app runs. Text without a color of its own takes the theme's primary text color (`textColorPrimary`). Right-to-left languages mirror the layout, the header and the tab bar.
- **Setup (mise):** Java 21, the Android SDK (`ANDROID_HOME`), `cargo-ndk`, the `aarch64-linux-android` target. Install the SDK packages and create an emulator:
  ```sh
  sdkmanager "platform-tools" "emulator" "platforms;android-36" "build-tools;37.0.0" "ndk;30.0.16248370" "system-images;android-36;google_apis;arm64-v8a"
  avdmanager create avd -n Pixel_9 -k "system-images;android-36;google_apis;arm64-v8a" -d pixel_9
  emulator -avd Pixel_9 &
  ```
- **The app crate:** its code lives in its library, and `main.rs` calls it. Android loads a shared library, through `prism::android_main!(run)` in `lib.rs`.
- **Device APIs:**
  - location (`LocationManager`), motion (rotation vector, gyroscope);
  - photos (system photo picker), camera (`ACTION_IMAGE_CAPTURE`);
  - haptics (`VibrationEffect`), clipboard;
  - share sheet (files through a small content provider), links;
  - secrets (Keystore), `BiometricPrompt`, sign-in in a Custom Tab, `cloud_storage` backed up (see [Security](../guide/security.md)).
  Permissions come from `Prism.toml`'s `[permissions]`. Deep links and sign-in redirects arrive through `PrismLinkActivity`, which hands them to the running app.
- **Notifications:** actions and text replies; the first one asks for the permission (Android 13+), as on iOS. Alerts are `AlertDialog`s. Context menus (`PopupMenu`) open on press and hold. Also supported: swipes, scroll events, deep links (`prism-<package>:/path`) and the lifecycle.
- **Background tasks:** JobScheduler, even with the app closed; the library then loads without the Activity and runs the task without a screen.
- **Live Activities:** an ongoing notification drawing the lock-screen content, with timers counting natively (`Chronometer`).
- **Widgets:** home-screen widgets (`AppWidgetProvider`), one per `register_widget`, with small/medium layouts chosen by size (Android 12+). They are redrawn from the last snapshot while the app is closed, charts as bitmaps (see [Charts in widgets](../guide/charts.md#charts-in-widgets-live-activities-and-car-screens)). Both surfaces render `SNode` snapshots with `RemoteViews` (`PrismSurface.java`).
- **Release:** `PRISM_RELEASE=1` or `prism build android`. Sign with `PRISM_ANDROID_KEYSTORE`, `PRISM_ANDROID_KEYSTORE_PASS` and `PRISM_ANDROID_KEY_ALIAS`; the default is the SDK's debug key. Libraries are aligned for 16 KB pages. `prism build android --aab` makes an App Bundle for Google Play, signed with that key, and `prism publish android` uploads it (see [Shipping to stores](../shipping.md#shipping-to-stores)). Installing a bundle on a device signs the device's APKs with the debug key, so the store key's passwords never reach bundletool (which reads them only from its command line or from files); uninstall a copy signed with another key first.
- **Wear OS and Android TV:** the same APK runs there; `cx.device_info().kind` says `Watch` or `Tv`.
  - *Watches:* dark, compact header, icon-only tabs, margins inside a round screen. Back is the swipe to the right. The app is standalone (no phone needed).
  - *TVs:* the remote moves the focus; the focused view rises with a ring. Select clicks, and holding it opens the context menu. Android TV's safe area applies, and the app gets a launcher banner made from the icon.
  - *Emulators:* `system-images;android-34;android-wear;arm64-v8a` (device `wearos_large_round`) and `system-images;android-34;android-tv;arm64-v8a` (device `tv_1080p`).
- **Not on Android:** tray and menu bar (desktop surfaces).
