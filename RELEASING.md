# Releasing

Every crate shares one version (`[workspace.package] version` in the root
`Cargo.toml`) and is released together. Pushing the version's tag publishes
it: `.github/workflows/release.yml` checks the tag, builds every crate from
its package on macOS, publishes them to crates.io through trusted publishing
(no token is stored anywhere), then creates the GitHub release.

## Checklist

1. **Start from a green `main`.** CI passes (format, tests, clippy on every
   target, bundles) and the docs workflow builds.
2. **Pick the version.** Before 1.0, a breaking change bumps the minor
   (`0.1.x` → `0.2.0`), anything else the patch. A release candidate takes
   a pre-release suffix: `0.2.0-rc.1` (see [Release candidates](#release-candidates)).
3. **Bump it in one commit**, in this order:
   1. `[workspace.package] version` in `Cargo.toml`;
   2. the `version = "…"` of every `prism*` entry in `[workspace.dependencies]`
      (same file; they are what crates.io resolves);
   3. `cargo update --workspace` to refresh `Cargo.lock`;
   4. `CHANGELOG.md`: rename `Unreleased` to the version and date
      (`## 0.2.0 (2026-12-01)`), start a new empty `Unreleased` section, and
      open the version's section with a short summary (what the release is
      about, its highlights, any breaking change): it is the top of the
      GitHub release.
4. **Check locally** (macOS with Xcode, so the Apple crates build):

   ```sh
   cargo fmt --all --check
   cargo test --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
   cargo publish --workspace --dry-run    # packages and builds every crate
   mise run docs                          # the book
   scripts/docs-links.py docs/book        # its links and anchors
   ```

   `--dry-run` packages each crate, then builds it from its package against
   the others' packages, so a file outside a crate's directory (or a missing
   version on a path dependency) fails here.
5. **Tag** the bump commit once it is on `main`:
   `git tag v0.2.0 && git push origin v0.2.0`. The release workflow then:
   1. refuses a tag that is not the workspace's version, not on `main`, or
      without its `CHANGELOG.md` section;
   2. runs `cargo publish --workspace --dry-run`: nothing is uploaded unless
      every crate packages and builds;
   3. publishes with `scripts/publish.sh` (the crates of that version not on
      crates.io yet, in dependency order);
   4. creates the GitHub release, its notes being the version's section of
      `CHANGELOG.md` (`scripts/release-notes.py 0.2.0` prints them). A
      section longer than GitHub takes gives its summary and each entry's
      first sentence, with a link to the full list.
6. **Check** the run (`gh run watch`), then the docs.rs builds. After a fix
   to the notes on `main`, `gh workflow run release.yml -f tag=v0.2.0`
   updates the release (and publishes nothing).

If the upload stops halfway (crates.io down, a rate limit), re-run the failed
job: `scripts/publish.sh` skips the crates already published.

## Release candidates

A version with a pre-release suffix (`0.2.0-rc.1`, `0.2.0-beta.2`) goes the
same way, from its tag `v0.2.0-rc.1`, with a `## 0.2.0-rc.1 (date)` section
in `CHANGELOG.md`. crates.io marks it a pre-release: `prismsdk = "0.2"`
keeps resolving to the last stable version, and only
`prismsdk = "0.2.0-rc.1"` gets it. The GitHub release is marked a
pre-release too. The final `0.2.0` follows with its own bump and tag.

## Trusted publishing (set up once)

crates.io lets the release workflow publish without a stored token: for each
crate, it trusts the `0xtlt/prismsdk` repository's `release.yml` workflow in
the `release` environment. GitHub signs a statement for the job
(`id-token: write`), `rust-lang/crates-io-auth-action` trades it for a token
that lasts only for the job, and the action revokes it at the end.

- The `release` environment exists in the repository's settings
  (Environments), its deployments limited to `v*` tags.
- Each crate of the workspace has the configuration on crates.io (the
  crate's Settings → Trusted Publishing: owner `0xtlt`, repository
  `prismsdk`, workflow `release.yml`, environment `release`). A new crate
  is published once by hand (trusted publishing configures existing crates),
  then gets it too.

To publish by hand instead (from macOS with Xcode), with a crates.io token
in `CARGO_REGISTRY_TOKEN`: `cargo publish --workspace --dry-run`, then
`scripts/publish.sh`.

## Publish order

Dependencies first. `cargo publish --workspace` computes this order itself;
the table is the same order for publishing crate by crate.

| # | Crate | Depends on (Prism crates) |
|---|---|---|
| 1 | `prismsdk-core` | – |
| 2 | `prismsdk-push` | – |
| 3 | `prismsdk-apple-build` | – |
| 4 | `prismsdk-build` | apple-build |
| 5 | `prismsdk-http` | core |
| 6 | `prismsdk-ui` | core |
| 7 | `prismsdk-native` | core, http |
| 8 | `prismsdk-web` | core, http |
| 9 | `prismsdk-db` | core |
| 10 | `prismsdk-macos` | core, native, apple-build (build script) |
| 11 | `prismsdk-ios` | core, native, apple-build (build script) |
| 12 | `prismsdk-watchos` | core, native, apple-build (build script) |
| 13 | `prismsdk-linux` | core, native, push |
| 14 | `prismsdk-android` | core, native, push |
| 15 | `prismsdk-windows` | core, native, http, build (build script) |
| 16 | `prismsdk` | core, ui, http, web, db and the six backends |
| 17 | `prismsdk-cli` | apple-build, build |
| 18 | `prismsdk-island` | – (islands' WASM side; built for `wasm32-unknown-unknown`) |

Publishing must happen from macOS with Xcode (the release workflow runs on
`macos-15`): verifying `prism-macos`, `prism-ios` and `prism-watchos`
compiles the Swift bridge.

### New versions of existing crates

crates.io accepts 30 updates in a burst, then one a minute: the whole
workspace goes in one command (`scripts/publish.sh`, which the release
workflow runs).

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
         prismsdk-android prismsdk-windows prismsdk prismsdk-cli \
         prismsdk-island; do
  cargo publish -p "$c" || break
done
```

Run the loop again after the wait: remove the crates already published from
the list first (publishing a version twice is an error). The thirteen crates
after the first five take about 130 minutes. Alternatively, ask help@crates.io for a
temporary higher limit before the release.

On crates.io every crate is named `prismsdk*` (`prism`, `prism-core` and
`prism-cli` belong to other projects). The folders, the dependency keys and the
library names stay `prism*`: `crates/prism-core` publishes `prismsdk-core`,
which code imports as `prism_core`. The publish order above uses the
crates.io names.

## After publishing

- docs.rs builds every crate within minutes (<https://docs.rs/crate/prismsdk/latest/builds>).
  The Apple, Android and Windows backends are documented on docs.rs' Linux target (see
  their `[package.metadata.docs.rs]`).
- `.github/workflows/docs.yml` only checks the book and the API docs; it
  deploys nothing (the site will be hosted on prismsdk.dev).
- A broken release can't be deleted, only yanked: `cargo yank --version
  0.2.0 <crate>`, then publish a fixed patch.
