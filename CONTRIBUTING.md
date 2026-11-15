# Contributing to Prism

Thanks for helping. Bug reports, platform gaps and pull requests are welcome
on [GitHub](https://github.com/0xtlt/prism/issues).

## Setup

Development happens on macOS (the Apple backends link Swift, so Xcode is
required); the Linux backend is developed in Docker.

```sh
mise install        # Rust 1.99 and the platform targets, Java, the Android SDK, mdBook
mise run test       # unit tests
mise run check      # type-check every crate on every supported target
mise run web        # the example app, restarting on save (also: macos, ios, android, macos-app, device)
mise run docs       # build the book into docs/book
```

Linux: `docker build -f docker/linux-dev.Dockerfile -t prism-linux-dev docker`,
then run `cargo` inside it as the CI does (`.github/workflows/ci.yml`).

## Where things are

| Path | |
|---|---|
| `crates/prism-core` | elements, styles, entities, reconciler, platform services: no platform code |
| `crates/prism-native` | patches → native views, flexbox layout (taffy) |
| `crates/prism-web` | the web backend (server, client JS and CSS in `src/`) |
| `crates/prism-{macos,ios,watchos,linux,android}` | the native backends |
| `crates/prism-ui` | the component library |
| `crates/prism` | the facade apps depend on |
| `crates/prism-apple-build/apple` | Swift: the bridge, the widget extension, the watchOS host |
| `crates/prism-cli/platform` | the bundling scripts and the Android Java host |
| `docs/` | this book (mdBook) |
| `examples/` | the counter app and an Axum server |

Files a crate needs at build or run time must live inside that crate's
directory: crates.io packages can't reach outside it. The CLI and
`prism-apple-build` embed their directories with generated lists, so a new
Swift, Java or script file only has to be added there.

## Before a pull request

```sh
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

- `prism`, `prism-core` and `prism-ui` deny undocumented public items
  (`missing_docs`, a warning that clippy's `-D warnings` turns into an error):
  give new public items a `///` line.
- Prefer the platform's real control or API, and report what a platform lacks
  through `cx.capabilities()` instead of failing silently.
- A feature that users see gets a section in the matching chapter of `docs/`,
  and a line in `CHANGELOG.md` under `Unreleased`.

Releases are described in [RELEASING.md](https://github.com/0xtlt/prism/blob/main/RELEASING.md).

## License

Prism is licensed under either of the Apache License, Version 2.0 or the MIT
license, at your option. Unless you explicitly state otherwise, any
contribution intentionally submitted for inclusion in Prism by you, as
defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
