# prism-http

HTTP for Prism apps: `cx.fetch(Request::get(url)).await`, off the UI thread, same code on every platform.

HTTP for Prism apps: `cx.fetch(Request::get(url)).await` runs the request off
the UI thread (rustls, no system TLS dependency) and resolves on it, with the
same code on every platform.

Apps use it as `prism::http`.

Part of [Prism](https://github.com/0xtlt/prismsdk). Guide: [https://0xtlt.github.io/prismsdk/](https://0xtlt.github.io/prismsdk/) · API: [docs.rs/prismsdk-http](https://docs.rs/prismsdk-http)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
