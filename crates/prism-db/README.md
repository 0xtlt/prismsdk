# prism-db

SQL for Prism apps (sqlx): SQLite on the device, the server's database on the web.

SQL for Prism apps with [sqlx](https://crates.io/crates/sqlx): SQLite on the
device natively, on the server for the web backend (where it is your
backend, shared by every user). Queries run on a small Tokio runtime and
resolve on the UI thread; migrations are applied once each, in order.

Apps use it as `prism::db` (the `prism` facade's `db` feature). It re-exports
sqlx 0.9, with a bundled SQLite 3.54 or later.

Part of [Prism](https://github.com/0xtlt/prismsdk). Guide: [https://0xtlt.github.io/prismsdk/](https://0xtlt.github.io/prismsdk/) · API: [docs.rs/prismsdk-db](https://docs.rs/prismsdk-db)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
