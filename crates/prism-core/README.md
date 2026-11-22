# prism-core

Platform-agnostic core of Prism: elements, styles, entities and the reconciler.

The platform-agnostic heart of Prism: element builders and styles, entities
and `Context`, the reconciler that diffs element trees into patches, the
`Backend` trait and the `Runtime`. No platform code: every backend
(`prism-web`, `prism-native` and the native backends) builds on it.

Apps use it through the `prism` facade, which re-exports everything here.

Part of [Prism](https://prismsdk.dev). Guide: [https://prismsdk.dev/](https://prismsdk.dev/) · API: [docs.rs/prismsdk-core](https://docs.rs/prismsdk-core)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
