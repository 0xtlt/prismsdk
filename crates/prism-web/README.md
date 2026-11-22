# prism-web

Prism web backend: server-side rendering + DOM patches over a WebSocket. No WASM. Embeddable in Axum or any HTTP server.

Prism's web backend, LiveView-style: pages are rendered on the server as
plain HTML, then a small JS client applies DOM patches streamed over a
WebSocket. No WASM: the app logic stays in native Rust on the server.

It runs as a standalone server or mounts in Axum next to your API
(`WebApp::router`). Without the default `server` feature it has no server
dependency: `WebApp::handle` and `WebApp::connect` plug into any HTTP and
WebSocket stack. Hashed static files can be exported for a CDN.

Apps use it through the `prism` facade's `web` feature.

Part of [Prism](https://prismsdk.dev). Guide: [https://prismsdk.dev/](https://prismsdk.dev/) · API: [docs.rs/prismsdk-web](https://docs.rs/prismsdk-web)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
