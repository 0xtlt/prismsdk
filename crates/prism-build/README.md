# prism-build

Build-time helpers for Prism apps: checked, typed translations from locales/*.json, and the app binary's link settings.

Build-time helpers for Prism apps. `translations` turns `locales/*.json` (one
file per language, CLDR plurals) into one typed function per key, checked at
build time: a missing key, an unknown placeholder or a plural without `other`
is a cargo warning (or an error with `.strict()`).

`link_app` gives the app's own binary what Prism's Apple backends cannot
(Cargo keeps a build script's link arguments to its package): the Swift
runtime's `-rpath /usr/lib/swift` and, when the environment sets none, Prism's
deployment target (iOS 17, tvOS 17, watchOS 10, visionOS 26, macOS 13).

```rust,ignore
// build.rs
fn main() {
    prism_build::link_app();
    prism_build::translations("locales").fallback("en").generate();
}
```

Part of [Prism](https://github.com/0xtlt/prismsdk). Guide: [https://0xtlt.github.io/prismsdk/](https://0xtlt.github.io/prismsdk/) · API: [docs.rs/prismsdk-build](https://docs.rs/prismsdk-build)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
