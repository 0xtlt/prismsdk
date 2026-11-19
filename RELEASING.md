# Releasing

Every crate shares one version (`[workspace.package] version` in the root
`Cargo.toml`) and is released together. Only maintainers with a crates.io
token for the crates can publish.

## Checklist

1. **Start from a green `main`.** CI passes (format, tests, clippy on every
   target, bundles) and the docs workflow builds.
2. **Pick the version.** Before 1.0, a breaking change bumps the minor
   (`0.1.x` → `0.2.0`), anything else the patch.
3. **Bump it in one commit**, in this order:
   1. `[workspace.package] version` in `Cargo.toml`;
   2. the `version = "…"` of every `prism*` entry in `[workspace.dependencies]`
      (same file; they are what crates.io resolves);
   3. `cargo update --workspace` to refresh `Cargo.lock`;
   4. `CHANGELOG.md`: rename `Unreleased` to the version and date, start a new
      empty `Unreleased` section.
4. **Check locally** (macOS with Xcode, so the Apple crates build):

   ```sh
   cargo fmt --all --check
   cargo test --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
   cargo publish --workspace --dry-run    # packages and builds every crate
   mise run docs                          # the book
   ```

   `--dry-run` packages each crate, then builds it from its package against
   the others' packages, so a file outside a crate's directory (or a missing
   version on a path dependency) fails here.
5. **Tag** the bump commit: `git tag v0.2.0 && git push origin main v0.2.0`.
6. **Publish** (below), then check the docs.rs builds and create the GitHub
   release from the changelog section.

## Publish order

Dependencies first. `cargo publish --workspace` computes this order itself;
the table is the same order for publishing crate by crate.

| # | Crate | Depends on (Prism crates) |
|---|---|---|
| 1 | `prism-core` | – |
| 2 | `prism-push` | – |
| 3 | `prism-apple-build` | – |
| 4 | `prism-build` | – |
| 5 | `prism-http` | core |
| 6 | `prism-ui` | core |
| 7 | `prism-native` | core, http |
| 8 | `prism-web` | core |
| 9 | `prism-db` | core |
| 10 | `prism-macos` | core, native, apple-build |
| 11 | `prism-ios` | core, native, apple-build |
| 12 | `prism-watchos` | core, native, apple-build |
| 13 | `prism-linux` | core, native, push |
| 14 | `prism-android` | core, native, push |
| 15 | `prism` | core, ui, http, web, db and the five backends |
| 16 | `prism-cli` | apple-build |
| 17 | `prism-island` | – (islands' WASM side; built for `wasm32-unknown-unknown`) |

Publishing must happen from macOS with Xcode: verifying `prism-macos`,
`prism-ios` and `prism-watchos` compiles the Swift bridge.

### New versions of existing crates

crates.io accepts 30 updates in a burst, then one a minute: the whole
workspace goes in one command.

```sh
cargo publish --workspace
```

### The first release (new crates)

crates.io limits **new** crates to a burst of 5, then one every 10 minutes
(`PublishNew` in crates.io's `rate_limiter.rs`). One `cargo publish
--workspace` would stop at the sixth crate with `429 Too Many Requests`. Publish
crate by crate instead, in the order above, and wait when crates.io asks to
(the error says until when):

```sh
for c in prismsdk-core prismsdk-push prismsdk-apple-build prismsdk-build \
         prismsdk-http prismsdk-ui prismsdk-native prismsdk-web prismsdk-db \
         prismsdk-macos prismsdk-ios prismsdk-watchos prismsdk-linux \
         prismsdk-android prismsdk prismsdk-cli prismsdk-island; do
  cargo publish -p "$c" || break
done
```

Run the loop again after the wait: remove the crates already published from
the list first (publishing a version twice is an error). The ten crates after
the first five take about 100 minutes. Alternatively, ask help@crates.io for a
temporary higher limit before the release.

On crates.io every crate is named `prismsdk*` (`prism`, `prism-core` and
`prism-cli` belong to other projects). The folders, the dependency keys and the
library names stay `prism*`: `crates/prism-core` publishes `prismsdk-core`,
which code imports as `prism_core`. The publish order above uses the
crates.io names.

## After publishing

- docs.rs builds every crate within minutes (<https://docs.rs/crate/prismsdk/latest/builds>).
  The Apple and Android backends are documented on docs.rs' Linux target (see
  their `[package.metadata.docs.rs]`).
- The book and the `main` API docs are deployed by `.github/workflows/docs.yml`
  on every push to `main`.
- A broken release can't be deleted, only yanked: `cargo yank --version
  0.2.0 <crate>`, then publish a fixed patch.
