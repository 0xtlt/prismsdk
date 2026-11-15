# prism-android

Prism backend for Android: the app runs in Rust, a small Java host shows native Android views.

Prism's Android backend: the app runs in Rust (a `cdylib`), and a small Java
host shipped with `prism-cli` creates real Android views (`TextView`,
`Button`, `EditText`, `Switch`…) at the frames `prism-native` computes. Phones,
tablets, Wear OS, Android TV, Android Auto and Automotive.

Apps get it through the `prism` facade on Android; `prism dev android` builds
the APK without Gradle.

Part of [Prism](https://github.com/0xtlt/prism). Guide: [https://0xtlt.github.io/prism/](https://0xtlt.github.io/prism/) · API: [docs.rs/prism-android](https://docs.rs/prism-android)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
