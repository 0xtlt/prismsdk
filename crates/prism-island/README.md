# prism-island

Write client-side islands for Prism web apps in Rust: small WASM Web Components with props from the server and events back.

An island is the part of a Prism web page that runs in the browser, such as
a chart following the pointer or a canvas, while the rest of the app stays on
the server. Implement the `Island` trait (`mount`, `update`, `unmount`),
render into the element's shadow root (`Host`), read the server's props
(`Props`, no serde needed) and send events back with `Host::emit`;
`register!` exports it.

An island is a crate of its own (`crate-type = ["cdylib"]`) listed under
`[web] islands` in `Prism.toml`; `prism run|dev|build web` compiles it to
WebAssembly, runs wasm-bindgen and serves it as `<prism-island-NAME>`.

```toml
[dependencies]
prism-island = { package = "prismsdk-island", version = "0.4" }
```

Part of [Prism](https://prismsdk.dev). Guide: [https://prismsdk.dev/](https://prismsdk.dev/) · API: [docs.rs/prismsdk-island](https://docs.rs/prismsdk-island)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
