# The `prism` command

`prism` creates, runs, bundles and live-restarts Prism apps. Install it with
`cargo install prismsdk-cli` (or `mise run cli` in a checkout).

```text
prism new <name> [--prism <path>]       Create a project
prism run <target> [options]            Build and run
prism dev <target> [options]            Run, rebuild and restart on save (keeps the screen)
prism build <target> [options]          Build release artifacts
prism devices                           List simulators, emulators and devices
```

Targets: `web`, `macos`, `ios`, `tvos`, `watchos`, `visionos`, `android`,
`linux`, `windows`.

| Option | |
|---|---|
| `--device <name\|udid>` | iOS: a real iPhone or iPad (signed with your team); Android: an `adb` serial (`emulator-5554`, a phone) |
| `--sim <name\|udid>` | iOS, tvOS, watchOS, visionOS: a simulator |
| `--release` | optimized build (`prism build` always is) |
| `--path </route>` | start on this screen |
| `--bundle` | macOS: run as a full `.app` (widgets, notifications) |
| `--port <port>` | web: listen on this port |
| `--target <triple>` | Windows: the Rust target of `prism build windows` (default: this PC's toolchain; off Windows `x86_64-pc-windows-gnu`) |

`prism new` uses path dependencies when it finds a Prism checkout (`--prism`,
or the one the command was installed from), and the crates.io release of the
same version otherwise.

On Windows, `prism run windows` and `prism dev windows` build and start the
app (`prism dev` stops it before each rebuild: Windows locks a running
program). `prism build windows` makes a zip of one folder: the release
executable (icon, version information and manifest inside, no console
window), the Windows App SDK runtime and the app's assets. It builds on
Windows, and from macOS or Linux it cross-compiles with MinGW-w64, or in
Docker without it ([Windows › Running and
shipping](platforms/windows.md#running-and-shipping)).

The bundling scripts and the platform sources (the Swift bridge and widget
extension, the Android Java host) are embedded in the binary and written under
`<target dir>/prism/platform` when they change, so projects need nothing else.

## `Prism.toml`

Next to the project's `Cargo.toml`:

```toml
[app]
name = "Counter"              # display name
package = "counter"           # cargo package (default: the Cargo.toml's)
bundle_id = "com.example.counter"  # dot-separated words: letters, digits, _ and -
version = "1.0.0"             # default: the Cargo.toml's
icon = "icon.png"             # 1024×1024 PNG
# locales = "locales"         # translations directory
orientations = ["portrait"]   # iPhone and Android (default: every one), see below

[permissions]                 # why the app asks (shown by the system), see below
location = "Shows todos near you."
camera = "Takes a photo for a todo."

[privacy]                     # the App Store privacy manifest, see below
tracking = false              # NSPrivacyTracking
# tracking_domains = ["ads.example.com"]  # needed when tracking = true
# manifest = "PrivacyInfo.xcprivacy"      # merged in (the default when it exists)

[privacy.apis]                # required-reason APIs of the app's own code
user_defaults = ["CA92.1"]

[privacy.collected]           # data the app or its server collects
email_address = ["app_functionality", "linked"]

[store]                       # in-app purchases (see In-app purchases)
enabled = true                # Android: bundles Google Play Billing

[ios]
team = "ABCDE12345"           # Apple team for real devices
simulator = "iPhone 17"       # default simulator
device = "My iPhone"          # default device (instead of a simulator)
icloud = true                 # iCloud key-value storage (cloud_storage)
associated_domains = ["webcredentials:example.com", "applinks:example.com"]  # passkeys, universal links
deployment_target = "17.0"    # oldest iOS/iPadOS (the default)

[macos]                       # likewise [tvos] (17.0), [watchos] (10.0), [visionos] (26.0)
deployment_target = "13.0"

[windows]
publisher = "Example Ltd"     # the executable's CompanyName (default: Cargo.toml's first author)

[network]
allow_http = ["preview.localhost"]  # plain-HTTP hosts and their subdomains, on one line
allow_http_dev = ["192.168.1.20"]   # the same, in development builds only

[web]
islands = ["islands/chart"]   # WASM island crates, on one line (see Islands)
island_budget_kb = 40         # gzip per island: `prism build web` fails above it
```

`[app] orientations` locks the app to some orientations (#310): any of
`"portrait"`, `"portrait_upside_down"`, `"landscape"` (both),
`"landscape_left"` and `"landscape_right"` (the device's top on the right or on
the left: iOS's `UIInterfaceOrientationLandscapeLeft` and `…Right`). On iPhone
they become `UISupportedInterfaceOrientations` (default: portrait and both
landscapes); iPad keeps every orientation, which its multitasking needs. On
Android they become the activity's `android:screenOrientation`, following the
user's rotation lock: `portrait`, `userPortrait` (both portraits),
`userLandscape`, `landscape`, `reverseLandscape`, or `fullUser` for all four;
portrait and landscape mixed otherwise leaves the system's default. Android 16
ignores the lock on large screens (600 dp wide and more), as it does for every
app targeting it.

`[network] allow_http` lists the hosts the app may reach over plain HTTP
(#152), such as a development server shown in a `web_view`: Apple platforms
refuse `http://` by default (App Transport Security) and so does Android
(cleartext traffic, since API 28). Each host and its subdomains get an
exception: `NSAppTransportSecurity` › `NSExceptionDomains` with
`NSExceptionAllowsInsecureHTTPLoads` in the iOS, iPadOS, tvOS, visionOS,
watchOS and macOS `Info.plist`, and a `network_security_config` with
`cleartextTrafficPermitted` on Android. An IP address (which ATS exceptions
cannot name) turns on `NSAllowsLocalNetworking` instead. Give hosts only,
without a scheme, port or path. App Review asks why a shipped app loads plain
HTTP: keep it to development hosts.

`[network] allow_http_dev` takes the same hosts, checked the same way, for
development builds only (#187): `prism run` and `prism dev` without `--release`
add them; `prism build`, `--release` and `prism publish` leave them out, so a
store build carries no exception for a laptop's address. A host in both lists
is allowed everywhere.

### Permissions

`[permissions]` names the protected resources the app asks for, each with the
text the system shows when it asks. `prism` knows these names and refuses
others:

| Name | Apple (Info.plist) | Android | Mac App Store sandbox, Windows |
|---|---|---|---|
| `location` | `NSLocationWhenInUseUsageDescription` | `ACCESS_FINE_LOCATION`, `ACCESS_COARSE_LOCATION` | location; `location` |
| `location_always` | `NSLocationAlwaysAndWhenInUseUsageDescription` | `ACCESS_BACKGROUND_LOCATION` | location; `location` |
| `camera` | `NSCameraUsageDescription` | `CAMERA` | camera; `webcam` |
| `microphone` | `NSMicrophoneUsageDescription` | `RECORD_AUDIO` | audio input; `microphone` |
| `photos` | `NSPhotoLibraryUsageDescription`, `NSPhotoLibraryAddUsageDescription` | — (the photo picker asks none) | photos library |
| `motion` | `NSMotionUsageDescription` | — | — |
| `face_id` | `NSFaceIDUsageDescription` | — | — |
| `contacts` | `NSContactsUsageDescription` | `READ_CONTACTS` | address book |
| `calendars`, `reminders` | `NSCalendarsFullAccessUsageDescription`, `NSRemindersFullAccessUsageDescription` (and the older keys) | `READ_CALENDAR`, `WRITE_CALENDAR` (calendars) | calendars |
| `bluetooth` | `NSBluetoothAlwaysUsageDescription` | `BLUETOOTH_SCAN`, `BLUETOOTH_CONNECT` | bluetooth; `bluetooth` |
| `speech` | `NSSpeechRecognitionUsageDescription` | `RECORD_AUDIO` | audio input |
| `tracking` | `NSUserTrackingUsageDescription` (App Tracking Transparency) | `com.google.android.gms.permission.AD_ID` | — |
| `health` | `NSHealthShareUsageDescription`, `NSHealthUpdateUsageDescription` | — | — |
| `nfc` | `NFCReaderUsageDescription` | `NFC` | —; `proximity` |
| `media_library` | `NSAppleMusicUsageDescription` | `READ_MEDIA_AUDIO` | — |
| `homekit`, `siri`, `local_network` | `NSHomeKitUsageDescription`, `NSSiriUsageDescription`, `NSLocalNetworkUsageDescription` | — | — |

App Store Connect wants a purpose string for every protected API the binary
references, whether the app calls it or not (ITMS-90683). So on Apple
platforms Prism's code for location, the camera, the microphone, the photo
library and Face ID is compiled in only when `[permissions]` declares it:
`prism` turns on prismsdk's feature for each (`location`, `location-always`,
`camera`, `microphone`, `photo-library`, `face-id`) through the key the app
depends on prismsdk under (`--features prism/location,…`). An app built
without `prism` lists them in its `Cargo.toml`
(`prism = { …, features = ["location"] }`). Without one, the API fails with a
message naming the `[permissions]` entry, and `capabilities()` reports the
resource unavailable. The Apple bundlers also look for protected APIs a crate
brings without its text (`CLLocationManager`, `CNContactStore`,
`HKHealthStore`…): App Store builds stop on one, other builds warn.

### Privacy manifest

Every Apple bundle Prism makes carries a `PrivacyInfo.xcprivacy` (#318):
the app and its widget extension, on iOS, iPadOS, tvOS, visionOS, watchOS
and macOS. It declares the [required-reason APIs](https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api)
each binary references, which the bundler finds in the binary itself (the
functions, constants and selectors Apple lists), and Prism gives the reasons
of its own uses:

| Category | Reason | Prism's use |
|---|---|---|
| `FileTimestamp` | `C617.1` | metadata of files in the app's containers (`std::fs`, assets, caches) |
| `SystemBootTime` | `35F9.1` | time between the app's events (animations, frames) |
| `DiskSpace` | `E174.1` | SQLite checks the volume before it writes (`db`) |

The other categories, `UserDefaults` and `ActiveKeyboards`, are the app's own
(a crate it depends on): it gives their reasons under `[privacy.apis]`
(`user_defaults`, `active_keyboards`, also `file_timestamp`, `system_boot_time`
and `disk_space` for reasons of its own), checked against Apple's list. An App
Store build for the iOS family stops when a category the binary references has
no reason, since App Store Connect rejects it (ITMS-91053); other builds warn.

`[privacy]` fills the rest of the app's manifest: `tracking` and
`tracking_domains` (Apple wants at least one domain when the app tracks, and
`tracking` under `[permissions]` for the prompt), and `[privacy.collected]`,
one line per [collected data type](https://developer.apple.com/documentation/bundleresources/describing-data-use-in-privacy-manifests)
(`email_address`, `name`, `precise_location`, `crash_data`,
`product_interaction`…) with its purposes (`third_party_advertising`,
`developer_advertising`, `analytics`, `product_personalization`,
`app_functionality`, `other`) and the flags `linked` (to the user's identity)
and `tracking`. A `PrivacyInfo.xcprivacy` next to `Prism.toml` (or the file
`[privacy] manifest` names) is merged in: its entries join Prism's, which
lets an app declare a reason or key Prism doesn't know yet.

`prism run|dev|build web` builds the [islands](guide/islands.md) for
`wasm32-unknown-unknown` with a size profile, runs wasm-bindgen as a library
(no CLI to install), runs `wasm-opt -Oz` when it is on `PATH`, prints each
island's raw / gzip / brotli size, and passes them to the server
(`PRISM_ISLANDS_DIR`). `prism dev` rebuilds them on save.

`prism` sets the deployment targets (`IPHONEOS_DEPLOYMENT_TARGET`,
`TVOS_DEPLOYMENT_TARGET`, `WATCHOS_DEPLOYMENT_TARGET`, `XROS_DEPLOYMENT_TARGET`,
`MACOSX_DEPLOYMENT_TARGET`) for every `cargo` it runs, from these keys or the
defaults; a variable already set in the environment wins. The app's
`build.rs` calls `prism_build::link_app()` (`prism new` writes it), so builds
without `prism` link the same way ([iOS](platforms/ios.md#deployment-target)).
On Windows it also embeds the app's icon, version information and manifest
and links release builds without a console; `prism` passes it `Prism.toml`'s
name, version, build, bundle id, publisher, description and icon
(`PRISM_APP_NAME`, `PRISM_VERSION`, `PRISM_BUILD`, `PRISM_BUNDLE_ID`,
`PRISM_PUBLISHER`, `PRISM_DESCRIPTION`, `PRISM_ICON`), and a plain `cargo
build` takes the `Cargo.toml`'s ([Windows › What the executable
says](platforms/windows.md#what-the-executable-says)).

The widget extension (iOS, iPadOS, visionOS, macOS) is built, signed and
embedded, with its provisioning profile and App Group, only when the app
registers a widget or (iOS) declares Live Activities with
`cx.enable_live_activities()`. Android generates one widget receiver per
registered widget, so none without widgets.

## What the bundlers ask the app

What goes into a bundle depends on what the app declares when it starts: its
assets directory, widgets, background tasks, Live Activities, widget buttons
and car screen. Before building for iOS, tvOS, watchOS, visionOS or Android,
`prism` builds the app for the computer it runs on and starts it once with
`PRISM_DESCRIBE=all`. `Application::run` then runs the app's `init`, prints
one JSON object and exits, without opening a window or starting a backend:

```json
{"assets": "/…/assets", "widgets": [{"kind": "todos", …}], "background": [{"id": "sync", "kind": "refresh"}],
 "live_activities": true, "widget_actions": true, "car": false}
```

The questions can also be asked one at a time (`PRISM_DESCRIBE=assets`,
`widgets`, `background`, `live_activities`, `widget_actions`, `car`). An
unknown question exits with status 2 instead of starting the app.

So the app must compile and reach `Application::run` on the computer that
bundles it, but it does not need Prism's native backend there. An app that
enables the backend only for its device targets keeps `prism` on every
target and turns `native` on per target:

```toml
[dependencies]
prism = { package = "prismsdk", version = "0.5", default-features = false }

[target.'cfg(target_os = "ios")'.dependencies]
prism = { package = "prismsdk", version = "0.5", features = ["native"] }
```

Its Mac build then compiles without AppKit and answers the bundler from the
core; started without `PRISM_DESCRIBE`, it stops with « no backend for this
target ». Dependencies the app takes only on the device go under the same
`[target.…]` table, and the code that uses them under the same `cfg`.

## Environment

| Variable | |
|---|---|
| `PRISM_PORT`, `PRISM_HOST` | web server address |
| `PRISM_PUBLIC_ORIGIN` | the web app's public origin behind a proxy (`https://app.example.com`) |
| `PRISM_SIM` | simulator (name or UDID); simulators are slimmed with `simslim` when installed |
| `PRISM_PROFILE` | provisioning profile for real Apple devices (found automatically otherwise) |
| `PRISM_PATH` | start screen (`/todos/1`) |
| `PRISM_LOCALE`, `PRISM_TEXT_SCALE` | language and text size, for testing |
| `PRISM_RELEASE=1` | release build |
| `PRISM_WINDOWS_SUBSYSTEM` | Windows: `windows` (no console) or `console`, whatever the profile (default: `windows` for release builds) |
| `PRISM_ANDROID_AUTOMOTIVE=1` | build for a car running Android |
| `PRISM_ANDROID_KEYSTORE`, `PRISM_ANDROID_KEYSTORE_PASS`, `PRISM_ANDROID_KEY_ALIAS` | Android release signing |

Web servers also read `PRISM_BASE_PATH`, `PRISM_ASSET_URL`,
`PRISM_DATA_DIR` ([Web](platforms/web.md)), `PRISM_ISLANDS_DIR` and
`PRISM_STRICT_CSP=1` ([Islands](guide/islands.md)). The development hooks
(`PRISM_CLICK`, `PRISM_SNAPSHOT`…) are in [Development](development.md).
