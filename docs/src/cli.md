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

[network]
allow_http = ["preview.localhost"]  # plain-HTTP hosts and their subdomains, on one line
allow_http_dev = ["192.168.1.20"]   # the same, in development builds only

[web]
islands = ["islands/chart"]   # WASM island crates, on one line (see Islands)
island_budget_kb = 40         # gzip per island: `prism build web` fails above it
```

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
prism = { version = "0.1", default-features = false }

[target.'cfg(target_os = "ios")'.dependencies]
prism = { version = "0.1", features = ["native"] }
```

Its Mac build then compiles without AppKit and answers the bundler from the
core; started without `PRISM_DESCRIBE`, it stops with « no backend for this
target ». Dependencies the app takes only on the device go under the same
`[target.…]` table, and the code that uses them under the same `cfg`.

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

Web servers also read `PRISM_BASE_PATH`, `PRISM_ASSET_URL`,
`PRISM_DATA_DIR` ([Web](platforms/web.md)), `PRISM_ISLANDS_DIR` and
`PRISM_STRICT_CSP=1` ([Islands](guide/islands.md)). The development hooks
(`PRISM_CLICK`, `PRISM_SNAPSHOT`…) are in [Development](development.md).
