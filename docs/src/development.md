# Development

```sh
mise install        # Rust 1.99 + macOS/iOS targets
mise run test       # unit tests
mise run check      # type-check host, web and iOS-simulator builds
mise run web        # the example, restarting on save (also: macos, ios, android, macos-app, device)
```

- `prism dev` keeps the current screen across restarts (`PRISM_DEV_STATE`); the web client reconnects by itself.
- `PRISM_PORT=8080`, `PRISM_SIM="iPhone 17"` (simulators are slimmed with `simslim` when installed), `PRISM_LOCALE=ar`, `PRISM_TEXT_SCALE=1.5`.
- macOS dev hooks: `PRISM_CLICK="+1|Dark mode" PRISM_SCROLL=600 PRISM_DELAY=2 PRISM_SNAPSHOT=out.png cargo run -p counter` presses controls (tab-bar tabs too) by title or accessibility label, scrolls, waits, writes a PNG of the window, and quits. `PRISM_PATH=/components` opens a screen first; `PRISM_HOVER="<accessibility label>"` puts that view in its hover state before the snapshot.
- `PRISM_A11Y_DUMP=/abs/tree.json` (macOS, iOS) writes the visible native view tree with each view's accessibility label, role and frame, for tests that check what a screen reader gets.
- CI (`.github/workflows/ci.yml`): format, tests, clippy for macOS / iOS simulator / iOS device / web, then release bundles and a web smoke test. The `release-*` jobs build the store packages and run every `prism publish --dry-run` with throwaway keys. The Linux job builds and installs the `.deb`. Smoke runs (`scripts/*-smoke.sh`, `scripts/web-e2e.mjs`) open every tab on Linux (Xvfb), macOS, the iOS simulator, an Android emulator and headless Chrome, and fail on crashes, blank screens, console errors or missing accessibility labels; `node --test crates/prism-web/tests/js/` tests `client.js` without npm.
