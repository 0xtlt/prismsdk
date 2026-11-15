# Performance

Measured on the example app (release build, Apple silicon):

| | Idle CPU | Memory |
|---|---|---|
| macOS, 460×720 / 1200×800 window | ~0 % | 42 MB / 49 MB |
| iOS (simulator) | ~0.2 % | 30–35 MB |
| Web server, per open session | ~0 % (sleeps until a message or timer) | ~0.3 MB |

- **Updates are local**: changing one row of a 5,000-row list re-renders that row only (1 patch, ~20 µs) and touches no other native view (`cargo test -p prism-native --release -- --ignored --nocapture bench`).
- **GPU-composited views**: backgrounds, borders, corners and pictures are layer properties, not per-view bitmaps (macOS graphics memory 28 MB → 0.4 MB in a large window).
- **Web**: one database pool per file for the whole server (not per visitor); sessions sleep until a message, a timer or finished background work.
- **Release profile**: thin LTO and stripped symbols (binaries −24 %).
