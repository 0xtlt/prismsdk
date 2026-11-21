# Development

```sh
mise install        # Rust 1.99 + macOS/iOS targets
mise run test       # unit tests
mise run check      # type-check host, web and iOS-simulator builds
mise run web        # the example, restarting on save (also: macos, ios, android, macos-app, device)
```

- `prism dev` keeps the current screen across restarts (`PRISM_DEV_STATE`); the web client reconnects by itself.
- `PRISM_PORT=8080`, `PRISM_SIM="iPhone 17"` (simulators are slimmed with `simslim` when installed), `PRISM_LOCALE=ar`, `PRISM_TEXT_SCALE=1.5`.
- macOS dev hooks: `PRISM_CLICK="+1|Dark mode" PRISM_SCROLL=600 PRISM_DELAY=2 PRISM_SNAPSHOT=out.png cargo run -p counter` presses controls (tab-bar tabs too) by title or accessibility label, scrolls, waits, writes a PNG of the window, and quits. `PRISM_PATH=/components` opens a screen first; `PRISM_HOVER="<accessibility label>"` puts that view in its hover state before the snapshot; `PRISM_APPEARANCE=light|dark` stands in for the system's appearance (as on Windows; the app's own choice still wins). An alert open at the snapshot (`cx.show_alert`, which macOS shows in a glass window no view draws) also writes its content (title, message, buttons) to `out.png.alert.png`.
- `COUNTER_NO_WIDGETS=1 prism run ios` (from `examples/counter`) builds the example without widgets or Live Activities, so its bundle has no widget extension and no App Group, like most apps.
- `PRISM_A11Y_DUMP=/abs/tree.json` (macOS, iOS) writes the visible native view tree with each view's accessibility label, role and frame, for tests that check what a screen reader gets. A table cell holding a control also lists its `"parts"`: the elements VoiceOver reaches in it.
- `PRISM_A11Y_PRESS="Rebook|Boarding"` (macOS) presses each accessibility element with that label as VoiceOver does (`accessibilityPerformPress`) and prints what it returned, to check that a table cell's press reaches its control or its row.
- CI (`.github/workflows/ci.yml`): format, tests, clippy for macOS / iOS simulator / iOS device / web, then release bundles and a web smoke test. The `release-*` jobs build the store packages and run every `prism publish --dry-run` with throwaway keys. The Linux job builds and installs the `.deb`. Smoke runs (`scripts/*-smoke.sh`, `scripts/web-e2e.mjs`) open every tab on Linux (Xvfb), macOS, the iOS simulator, an Android emulator and headless Chrome (`scripts/windows-smoke.sh` runs on a Windows machine over SSH, in the signed-in user's desktop session for WinUI, outside CI), and fail on crashes, blank screens, console errors or missing accessibility labels; `node --test crates/prism-web/tests/js/` tests `client.js` without npm.
- Islands end to end: `scripts/islands-e2e.mjs` drives the counter's `/components/islands` page in headless Chrome. Start the server with `PRISM_STRICT_CSP=1` after `prism build web` (it needs the `wasm32-unknown-unknown` target), then run `node scripts/islands-e2e.mjs --url http://127.0.0.1:3000 --out /tmp/shots`. It checks that a page without islands loads no island file, that the island page loads its JS and WASM once, that pointer moves send nothing over the socket, that a click sends one event and the server's text updates, that "Add a point" updates the same mounted island, that nothing violates the strict CSP, and that the fallback shows with JavaScript off.
