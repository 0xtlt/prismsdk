# The `prism` command

`prism` creates, runs, bundles and live-restarts Prism apps. Install it with
`cargo install prism-cli` (or `mise run cli` in a checkout).

```text
prism new <name> [--prism <path>]       Create a project
prism run <target> [options]            Build and run
prism dev <target> [options]            Run, rebuild and restart on save (keeps the screen)
prism build <target> [options]          Build release artifacts
prism devices                           List simulators, emulators and devices
```

Targets: `web`, `macos`, `ios`, `tvos`, `watchos`, `visionos`, `android`.

| Option | |
|---|---|
| `--device <name\|udid>` | iOS: a real iPhone or iPad (signed with your team); Android: an `adb` serial (`emulator-5554`, a phone) |
| `--sim <name\|udid>` | iOS, tvOS, watchOS, visionOS: a simulator |
| `--release` | optimized build (`prism build` always is) |
| `--path </route>` | start on this screen |
| `--bundle` | macOS: run as a full `.app` (widgets, notifications) |
| `--port <port>` | web: listen on this port |

`prism new` uses path dependencies when it finds a Prism checkout (`--prism`,
or the one the command was installed from), and the crates.io release of the
same version otherwise.

The bundling scripts and the platform sources (the Swift bridge and widget
extension, the Android Java host) are embedded in the binary and written under
`<target dir>/prism/platform` when they change, so projects need nothing else.

## `Prism.toml`

Next to the project's `Cargo.toml`:

```toml
[app]
name = "Counter"              # display name
package = "counter"           # cargo package (default: the Cargo.toml's)
bundle_id = "com.example.counter"
version = "1.0.0"             # default: the Cargo.toml's
icon = "icon.png"             # 1024×1024 PNG
# locales = "locales"         # translations directory

[permissions]                 # why the app asks (shown by the system)
location = "Shows todos near you."
camera = "Takes a photo for a todo."

[ios]
team = "ABCDE12345"           # Apple team for real devices
simulator = "iPhone 17"       # default simulator
device = "My iPhone"          # default device (instead of a simulator)
icloud = true                 # iCloud key-value storage (cloud_storage)
deployment_target = "17.0"    # oldest iOS/iPadOS (the default)

[macos]                       # likewise [tvos] (17.0), [watchos] (10.0), [visionos] (26.0)
deployment_target = "13.0"
```

`prism` sets the deployment targets (`IPHONEOS_DEPLOYMENT_TARGET`,
`TVOS_DEPLOYMENT_TARGET`, `WATCHOS_DEPLOYMENT_TARGET`, `XROS_DEPLOYMENT_TARGET`,
`MACOSX_DEPLOYMENT_TARGET`) for every `cargo` it runs, from these keys or the
defaults; a variable already set in the environment wins. The app's
`build.rs` calls `prism_build::link_app()` (`prism new` writes it), so builds
without `prism` link the same way ([iOS](platforms/ios.md#deployment-target)).

The widget extension (iOS, iPadOS, visionOS, macOS) is built, signed and
embedded, with its provisioning profile and App Group, only when the app
registers a widget or (iOS) declares Live Activities with
`cx.enable_live_activities()`. Android generates one widget receiver per
registered widget, so none without widgets.

## Environment

| Variable | |
|---|---|
| `PRISM_PORT`, `PRISM_HOST` | web server address |
| `PRISM_SIM` | simulator (name or UDID); simulators are slimmed with `simslim` when installed |
| `PRISM_PROFILE` | provisioning profile for real Apple devices (found automatically otherwise) |
| `PRISM_PATH` | start screen (`/todos/1`) |
| `PRISM_LOCALE`, `PRISM_TEXT_SCALE` | language and text size, for testing |
| `PRISM_RELEASE=1` | release build |
| `PRISM_ANDROID_AUTOMOTIVE=1` | build for a car running Android |
| `PRISM_ANDROID_KEYSTORE`, `PRISM_ANDROID_KEYSTORE_PASS`, `PRISM_ANDROID_KEY_ALIAS` | Android release signing |

Web servers also read `PRISM_BASE_PATH`, `PRISM_ASSET_URL` and
`PRISM_DATA_DIR` ([Web](platforms/web.md)). The development hooks
(`PRISM_CLICK`, `PRISM_SNAPSHOT`…) are in [Development](development.md).
