# prism-linux

Prism backend for Linux desktops (GTK 4 native widgets).

Prism's GTK 4 backend for Linux desktops: every node is a real GTK widget
(`GtkLabel`, `GtkButton`, `GtkEntry`, `GtkSwitch`…), placed at the frames
`prism-native` computes, plus notifications, a tray (StatusNotifierItem) and
a menu bar. Requires the GTK 4 development files (`libgtk-4-dev`). Web
views use WebKitGTK 6.0 with the `web-view` feature
(`libwebkitgtk-6.0-dev`); without it they are empty boxes.

Apps get it through the `prism` facade on Linux.

Part of [Prism](https://prismsdk.dev). Guide: [https://prismsdk.dev/](https://prismsdk.dev/) · API: [docs.rs/prismsdk-linux](https://docs.rs/prismsdk-linux)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
