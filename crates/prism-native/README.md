# prism-native

Shared machinery for native Prism backends: patch handling and flexbox layout (taffy).

Shared machinery for Prism's native backends. A platform implements the small
`Widgets` trait (create a view, set its text, measure it, place it);
`NativeRenderer` turns reconciler patches into those calls and runs flexbox
layout with taffy, so AppKit, UIKit, GTK, Android and WinUI lay out exactly like the
web backend's CSS.

Part of [Prism](https://github.com/0xtlt/prismsdk). Guide: [https://0xtlt.github.io/prismsdk/](https://0xtlt.github.io/prismsdk/) · API: [docs.rs/prismsdk-native](https://docs.rs/prismsdk-native)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
