# prism-native

Shared machinery for native Prism backends: patch handling and flexbox layout (taffy).

Shared machinery for Prism's native backends. A platform implements the small
`Widgets` trait (create a view, set its text, measure it, place it);
`NativeRenderer` turns reconciler patches into those calls and runs flexbox
layout with taffy, so AppKit, UIKit, GTK and Android lay out exactly like the
web backend's CSS.

Part of [Prism](https://github.com/0xtlt/prism). Guide: [https://0xtlt.github.io/prism/](https://0xtlt.github.io/prism/) · API: [docs.rs/prism-native](https://docs.rs/prism-native)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
