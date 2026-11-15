# prism-ios

Prism backend for iOS (UIKit native views).

Prism's UIKit backend for iOS, iPadOS, tvOS and visionOS: every node is a
real `UIView` (`UILabel`, `UIButton`, `UITextField`…), laid out by
`prism-native`. Widgets, Live Activities, notifications and device APIs go
through a Swift bridge compiled by `prism-apple-build` (Xcode required).

Apps get it through the `prism` facade on Apple mobile targets; the `prism`
command (`prism-cli`) bundles, signs and installs the app.

Part of [Prism](https://github.com/0xtlt/prism). Guide: [https://0xtlt.github.io/prism/](https://0xtlt.github.io/prism/) · API: [docs.rs/prism-ios](https://docs.rs/prism-ios)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
