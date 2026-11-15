# prism-macos

Prism backend for macOS (AppKit native views).

Prism's AppKit backend: every node is a real `NSView` (labels are
`NSTextField`s, buttons `NSButton`s), laid out by `prism-native`, plus the
menu bar, tray items and WidgetKit widgets through a Swift bridge compiled by
`prism-apple-build` (Xcode required).

Apps get it through the `prism` facade on macOS.

Part of [Prism](https://github.com/0xtlt/prism). Guide: [https://0xtlt.github.io/prism/](https://0xtlt.github.io/prism/) · API: [docs.rs/prism-macos](https://docs.rs/prism-macos)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
