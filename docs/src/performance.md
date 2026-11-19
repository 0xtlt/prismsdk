# Performance

Measured on the example app (release build, Apple silicon):

| | Idle CPU | Memory |
|---|---|---|
| macOS, 460×720 / 1200×800 window | ~0 % | 42 MB / 49 MB |
| iOS (simulator) | ~0.2 % | 30–35 MB |
| Web server, per open session | ~0 % (sleeps until a message or timer) | ~0.3 MB |

- **Updates are local**: changing one row of a 5,000-row list re-renders that row only (1 patch, ~20 µs) and touches no other native view (`cargo test -p prismsdk-native --release -- --ignored --nocapture bench`).
- **GPU-composited views**: backgrounds, borders, corners and pictures are layer properties, not per-view bitmaps (macOS graphics memory 28 MB → 0.4 MB in a large window).
- **Web**: one database pool per file for the whole server (not per visitor); sessions sleep until a message, a timer or finished background work.
- **Lazy tabs**: a tab renders on its first selection ([navigation](guide/navigation.md)), so a page ships only the visible tab: the example's `/` is 44 nodes instead of 715, 16 KB of HTML instead of 150 KB, and a 24 KB first WebSocket update instead of 384 KB.
- **Release profile**: thin LTO and stripped symbols (binaries −24 %).

## Android and Linux

Measured on the example app, release builds: Android on the `Pixel_9`
emulator (Android 16, arm64, Apple silicon host), Linux in the dev image
(`docker/linux-dev.Dockerfile`: GTK 4 under Xvfb, Cairo renderer).

| | Idle CPU | Memory (PSS) |
|---|---|---|
| Android, first screen / 10,000-row list after 12 flings | ~0 % / ~0 % | 33 MB / 53 MB |
| Linux, first screen / 10,000-row list | ~0 % / ~0 % | 79 MB / 101 MB |

- **Android measures each text once**: layout asks the same leaf several times, and each answer is a JNI call that styles a view in Java; Rust keeps the answers by request (until a font loads or the language changes), and Java measures with one reused view. Launch: 35 measure calls instead of 254, the first screen drawn ~165 ms after the process starts instead of ~800 ms. A `+1` tap: 2 measure calls and one `apply`, ~0.6 ms.
- **Android long lists**: scrolling the 10,000-row `VirtualList` costs ~0.7 ms of the UI thread per scroll event (2 measures, 17 ops), and 2 % of frames miss their deadline during flings (`dumpsys gfxinfo`).
- **Android animations**: an endless `repeat` (a pulse, a spinner) pauses while its view is hidden (a screen below, another tab) instead of waking the app on every frame.
- **GTK stylesheet**: nodes with the same style share one CSS rule (class `s<n>`), and the sheet is reloaded (which restyles every widget) only when a new rule appears. A `+1` click: ~0.5 ms instead of ~2 ms; a 3,000 px scroll of the long list (441 patches): ~21 ms instead of ~40–49 ms, half of which was reloading a 68 KB sheet.
- **Size**: the example's APK is 12.3 MB (`libcounter.so` 9.0 MB, `classes.dex` 6.6 MB with the car library); the Linux binary is 12.9 MB. A cold release build of the example takes about a minute in the Linux image (14 cores).
