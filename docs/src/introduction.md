# Prism

Write a UI once in Rust (GPUI-style API), ship it as:

| Target | How it renders | Command |
|---|---|---|
| **Web** | Server-side Rust → HTML/CSS + a small JS client receiving DOM patches over a WebSocket. **No WASM.** | `prism dev web` → http://localhost:3000 |
| **macOS** | Real AppKit views (`NSView`, `NSTextField`, `NSButton`), menu bar, tray | `prism dev macos` |
| **iOS** | Real UIKit views (`UIView`, `UILabel`, `UIButton`, `UITextField`), widgets + Dynamic Island | `prism dev ios` (simulator) / `prism run ios --device "My iPhone"` |
| **Android** | Real Android views (`TextView`, `Button`, `EditText`, `Switch`), home-screen widgets, ongoing notifications | `prism dev android` (emulator or phone) |

Also, as previews: [tvOS](platforms/tvos.md), [watchOS](platforms/watchos.md),
[visionOS](platforms/visionos.md), [Linux (GTK 4)](platforms/linux.md),
[Windows (WinUI 3)](platforms/windows.md), Wear OS, Android TV and car screens ([CarPlay, Android Auto](guide/car.md)).

## Platforms in 0.1

| Platform | In 0.1 | Oldest version |
|---|---|---|
| Web | supported | a current browser (tested in Chrome) |
| macOS | supported | macOS 13 |
| iOS and iPadOS | supported | iOS 17 |
| Android (phones, tablets) | supported | Android 10 (API 29) |
| tvOS | preview | tvOS 17 |
| watchOS | preview | watchOS 10 |
| visionOS | preview | visionOS 26 |
| Linux | preview | GTK 4 |
| Windows | preview | Windows 10 1809 (the Windows App SDK's minimum); tested on Windows 11 |
| Wear OS, Android TV | preview | the Android APK, Android 10 (API 29) |
| Car screens (CarPlay, Android Auto, Automotive) | preview | those of iOS and Android |

**Supported** platforms are the ones Prism 0.1 is made for: they are tested on every change, and their bugs come first. **Preview** platforms work and can be tried, with gaps listed on their page in the Platforms part, and are less tested on real devices.

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

## This guide

- [Getting started](getting-started.md): install the `prism` command, create
  and run a project.
- **Guide**: one chapter per feature area, from [state](guide/state.md) to
  [navigation](guide/navigation.md), widgets and car screens.
- **Platforms**: what each backend uses and what it lacks.
- [Architecture](architecture.md): how the core, the reconciler and the
  backends fit together, and how to add a platform.
- [Shipping](shipping.md): release builds, signing, servers and CDNs.

The API reference for the `main` branch is [here](api/prism/index.html)
(`prism`, the crate apps depend on, links to the others); released versions
are on docs.rs.
