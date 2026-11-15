# prism-apple-build

Build-script helper: compiles Prism's Swift bridge (WidgetKit, ActivityKit, UserNotifications) and links it.

Build-script helper for Prism's Apple backends. It compiles the Swift sources
shipped in this crate (the bridge to WidgetKit, ActivityKit,
UserNotifications, CarPlay and the device APIs, and the watchOS SwiftUI host)
for the cargo target and links them. Requires Xcode; a no-op elsewhere.

It also exposes those sources (and the widget extension) to the `prism` CLI.

Part of [Prism](https://github.com/0xtlt/prism). Guide: [https://0xtlt.github.io/prism/](https://0xtlt.github.io/prism/) · API: [docs.rs/prism-apple-build](https://docs.rs/prism-apple-build)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
