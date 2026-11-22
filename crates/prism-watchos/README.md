# prism-watchos

Prism backend for watchOS: the app runs in Rust, a SwiftUI host draws it.

Prism's watchOS backend. watchOS has no UIKit views, so the app runs in Rust
and a SwiftUI host (compiled by `prism-apple-build`, Xcode required) draws the
tree at the frames `prism-native` computes.

Apps get it through the `prism` facade on watchOS; `prism run watchos` builds
and bundles the watch app (see the
[watchOS chapter](https://prismsdk.dev/platforms/watchos.html)).

Part of [Prism](https://prismsdk.dev). Guide: [https://prismsdk.dev/](https://prismsdk.dev/) · API: [docs.rs/prismsdk-watchos](https://docs.rs/prismsdk-watchos)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
