# prism-windows

Prism backend for Windows (WinUI 3, Windows App SDK).

Prism's native Windows backend: one Win32 window per app window whose
client area is a XAML island of the Windows App SDK. Prism's nodes become
real WinUI 3 controls (`Button`, `TextBox`, `ToggleSwitch`, `Slider`,
`ComboBox`, `CalendarDatePicker`, a `SelectorBar` for tabs…), `TextBlock`s
and `Border`s, laid out by `prism-native`'s flexbox like on the other native
platforms; WinUI's UI Automation peers expose them to screen readers.

The Windows App SDK is self-contained: the build script fetches it from
nuget.org once and generates its bindings; the app's `build.rs`
(`prism_build::link_app()`) copies its runtime next to the executable and
embeds the manifest that registers it. Nothing is installed on the user's
machine. Builds with the MSVC and the GNU (MinGW) toolchains. WinUI renders
in a desktop session only (not over SSH).

Apps get it through the `prism` facade on Windows.

Part of [Prism](https://github.com/0xtlt/prismsdk). Guide: [https://0xtlt.github.io/prismsdk/](https://0xtlt.github.io/prismsdk/) · API: [docs.rs/prismsdk-windows](https://docs.rs/prismsdk-windows)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
