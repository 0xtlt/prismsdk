# Performance

Measured on the example app (release build, Apple silicon):

| | Idle CPU | Memory |
|---|---|---|
| macOS, 460×720 window | ~0 % (0.3 wakeups/s) | 35 MB |
| iOS (simulator) | ~0 % (0.05 wakeups/s) | 31 MB |
| Web server, per open session | ~0 % (sleeps until a message or timer) | ~0.3 MB |

Time from the event to the frame committed to Core Animation, median (release
build, the example app):

| | macOS | iOS (simulator) |
|---|---|---|
| Launch to first frame | 190 ms | 300 ms after `main` (the simulator adds ~1.3 s before it) |
| `+1` (one text changes) | 0.4 ms | 0.8 ms |
| First visit of the Components tab (~2,450 patches) | 180 ms | 240 ms |
| Switch back to that tab | 5–18 ms | 17 ms |
| Back to that screen from a pushed one | 5 ms | — |
| Push the 10,000-row list | 14 ms | 55 ms |
| Scroll it by 37 points | 2.0 ms | 1.1 ms |

- **Updates are local**: changing one row of a 5,000-row list re-renders that row only (1 patch, ~20 µs) and touches no other native view (`cargo test -p prismsdk-native --release -- --ignored --nocapture bench`).
- **GPU-composited views**: backgrounds, borders, corners and pictures are layer properties, not per-view bitmaps (macOS graphics memory 28 MB → 0.4 MB in a large window).
- **Web**: one database pool per file for the whole server (not per visitor); sessions sleep until a message, a timer or finished background work.
- **Lazy tabs**: a tab renders on its first selection ([navigation](guide/navigation.md)), so a page ships only the visible tab: the example's `/` is 44 nodes instead of 715, 16 KB of HTML instead of 150 KB, and a 24 KB first WebSocket update instead of 384 KB.
- **Release profile**: thin LTO and stripped symbols (binaries −24 %).
- **macOS tabs and stacks**: tabs and the screens under the top one stay in the window, hidden, so going back to them draws nothing again (a warm tab switch 29 ms → 5–18 ms, Back to a large screen 39 ms → 5 ms). Their layers keep their drawing: having visited every tab of the example costs ~12 MB more.
- **No idle timer**: countdowns tick every second only while one is shown; an app without one does not wake up for them.
- **Apple number formatting**: `cx.format_number` reuses one `NSNumberFormatter` per locale (scrolling a list of numbers: −40 % on macOS, −50 % in the iOS simulator).
