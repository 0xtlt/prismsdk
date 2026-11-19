# Performance

Measured on the example app (release build, Apple silicon):

| | Idle CPU | Memory |
|---|---|---|
| macOS, 460×720 / 1200×800 window | ~0 % | 42 MB / 49 MB |
| iOS (simulator) | ~0.2 % | 30–35 MB |
| Web server, per open session | ~0 % (sleeps until a message or timer) | ~0.7 MB |

- **Updates are local**: changing one row of a 5,000-row list re-renders that row only (1 patch, ~20 µs) and touches no other native view (`cargo test -p prismsdk-native --release -- --ignored --nocapture bench`).
- **GPU-composited views**: backgrounds, borders, corners and pictures are layer properties, not per-view bitmaps (macOS graphics memory 28 MB → 0.4 MB in a large window).
- **Web**: one database pool per file for the whole server (not per visitor); sessions sleep until a message, a timer or finished background work.
- **Lazy tabs**: a tab renders on its first selection ([navigation](guide/navigation.md)), so a page ships only the visible tab: the example's `/` is 44 nodes instead of 715, and 17 KB of HTML instead of 150 KB.
- **Release profile**: thin LTO and stripped symbols (binaries −24 %).

## Web

The example's `/` (release build, served on localhost; 100 sessions opened on a fresh server for the memory):

| | Raw | Compressed |
|---|---|---|
| Page (first paint) | 17 KB | 2.7 KB gzip |
| First WebSocket update | 19 KB of JSON | 2.8 KB deflated |
| A click on « +1 » | 203-byte patch, ~0.6 ms there and back | – |
| `client.js` | 262 KB | 77 KB gzip, 65 KB brotli |
| `prism.css` | 17 KB | 4.0 KB gzip |
| The chart island (JS + WASM, without `wasm-opt`) | 62 KB | 22 KB gzip, 19 KB brotli |

- **Lean messages**: within a message, a node whose style another node got first names that node instead of repeating the CSS, and messages of 1 KB or more are deflated for browsers that can inflate them ([Web](platforms/web.md#transport-and-compression)).
- **Lean client**: `client.js` and `prism.css` are served without their comment lines and indentation, gzipped once and cached `immutable`.
- **A 5,000-row list** (`cargo test -p prismsdk-web --release --test bench -- --ignored --nocapture`): a 991 KB page (59 KB gzipped) and a 3.1 MB first tree (243 KB deflated), each built in 60–90 ms, about half of it the core's render; changing one row sends a 60-byte patch, about 20 µs after the click reaches the session.
