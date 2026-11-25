# Getting started

## Install the `prism` command

From crates.io:

```sh
cargo install prismsdk-cli   # installs `prism`
```

On crates.io Prism's crates are named `prismsdk*`; code still imports them as
`prism` (`use prism::prelude::*`), and `prism new` writes the dependency:

```toml
prism = { package = "prismsdk", version = "0.3" }
```

Or from a checkout of the repository (this also makes `prism new` point new
projects at the checkout instead of crates.io):

```sh
git clone https://github.com/0xtlt/prismsdk && cd prismsdk
mise install                 # Rust 1.99 and the platform targets
mise run cli                 # cargo install --path crates/prism-cli --locked
```

Prism needs Rust 1.94 or later. Each target has its own tools: Xcode for the
Apple platforms, the GTK 4 development files on Linux, Java and the Android
SDK for Android (see [Android](platforms/android.md)), MSVC or MinGW-w64 for
Windows (see [Windows](platforms/windows.md)).

## Quick start

```sh
prism new my-app && cd my-app
prism dev web                # rebuilds and restarts on every save, on the same screen
prism dev ios                # same, in the iOS simulator
prism run ios --device "My iPhone"
prism build macos            # release .app with widgets and icon
prism publish ios --dry-run  # App Store build, checked (see Shipping to stores)
prism dev android            # the running emulator or a USB phone
prism dev windows            # on Windows: a WinUI window
prism build windows          # a zip: the app and its WinUI runtime (from any OS)
prism devices                # simulators, paired iPhones, Android devices
```

`Prism.toml` configures the app: name, `bundle_id`, `version`, `icon` (one 1024×1024 PNG, compiled with `actool` for iOS and macOS), and `[ios] team` / `device` / `simulator`. Real devices are signed with your Apple Development identity and a matching provisioning profile (found automatically, or `PRISM_PROFILE`), then installed with `devicectl`. The bundling scripts and Apple sources are embedded in the CLI, so projects don't need this repository's scripts.

```rust
use prism::prelude::*;

struct Counter { count: i32 }

impl Render for Counter {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().p(24.).gap(12.)
            .child(text(format!("Count: {}", self.count)).text_size(32.).bold())
            .child(button("+1").on_click(cx.listener(|this, _, _| this.count += 1)))
    }
}

fn main() {
    Application::new().title("Counter").run(|cx| cx.new_entity(|_| Counter { count: 0 }));
}
```

## What `prism new` creates

| File | |
|---|---|
| `Cargo.toml` | depends on `prism` (features `native`, default, and `web`) and `prism-build` |
| `Prism.toml` | name, bundle id, version, icon, Apple team, Windows publisher ([the `prism` command](cli.md#prismtoml)) |
| `src/lib.rs` | the app: a `run()` function, plus `prism::android_main!(run)` for Android |
| `src/main.rs` | calls `run()` on every other platform |
| `build.rs` | the app binary's link settings (`prism_build::link_app()`: Swift's rpath and deployment target on Apple platforms, see [iOS](platforms/ios.md#deployment-target); on Windows the WinUI runtime next to the executable, its manifest, icon and version information, and no console for release builds, see [Windows](platforms/windows.md#running-and-shipping)); turns `locales/*.json` into typed `strings::…` functions ([Languages](guide/i18n.md)) |
| `locales/en.json`, `fr.json` | the strings |
| `assets/` | pictures shown with `picture("logo.png")` |
| `mise.toml` | `mise run web` / `macos` / `ios` / `android` / `windows` |

The app lives in its library because Android loads it as a shared library.
Next: [Updating state](guide/state.md).
