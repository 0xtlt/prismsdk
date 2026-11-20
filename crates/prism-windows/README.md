# prism-windows

Prism backend for Windows (Win32 + Direct2D/DirectWrite).

Prism's native Windows backend: one Win32 window per app window, whose scene
the backend keeps and draws itself with Direct2D and DirectWrite, in a
Windows 11 (Fluent) look. Text fields are RichEdit child windows (IME,
selection, undo, clipboard). Layout is `prism-native`'s flexbox, like on the
other native platforms; UI Automation exposes the tree to screen readers.

Nothing to install or ship: the backend only uses system DLLs, and builds
with the MSVC and the GNU (MinGW) toolchains.

Apps get it through the `prism` facade on Windows.

Part of [Prism](https://github.com/0xtlt/prismsdk). Guide: [https://0xtlt.github.io/prismsdk/](https://0xtlt.github.io/prismsdk/) · API: [docs.rs/prismsdk-windows](https://docs.rs/prismsdk-windows)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
