# prism-build

Build-time helpers for Prism apps: checked, typed translations from locales/*.json, and the app binary's link settings.

Build-time helpers for Prism apps. `translations` turns `locales/*.json` (one
file per language, CLDR plurals) into one typed function per key, checked at
build time: a missing key, an unknown placeholder or a plural without `other`
is a cargo warning (or an error with `.strict()`).

`link_app` gives the app's own binary what Prism's backends cannot (Cargo
keeps a build script's link arguments to its package): on Apple targets the
Swift runtime's `-rpath /usr/lib/swift` and, when the environment sets none,
Prism's deployment target (iOS 17, tvOS 17, watchOS 10, visionOS 26, macOS
13); on Windows the WinUI 3 runtime (Windows App SDK, self-contained, fetched
from nuget.org once) next to the executable and the manifest that registers
it inside (`winappsdk`).

```rust,ignore
// build.rs
fn main() {
    prism_build::link_app();
    prism_build::translations("locales").fallback("en").generate();
}
```

Part of [Prism](https://prismsdk.dev). Guide: [https://prismsdk.dev/](https://prismsdk.dev/) · API: [docs.rs/prismsdk-build](https://docs.rs/prismsdk-build)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
