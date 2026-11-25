# Performance

Measured on the example app (release build, Apple silicon):

| | Idle CPU | Memory |
|---|---|---|
| macOS, 460×720 window | ~0 % (0.3 wakeups/s) | 35 MB |
| iOS (simulator) | ~0 % (0.05 wakeups/s) | 31 MB |
| Web server, per open session | ~0 % (sleeps until a message or timer) | ~0.7 MB |

Time from the event to the frame committed to Core Animation, median (release
build, the example app):

| | macOS | iOS (simulator) |
|---|---|---|
| Launch to first frame | 190 ms | 300 ms after `main` |
| `+1` (one text changes) | 0.4 ms | 0.8 ms |
| First visit of the Components tab (~2,450 patches) | 180 ms | 240 ms |
| Switch back to that tab | 5–18 ms | 17 ms |
| Back to that screen from a pushed one | 5 ms | — |
| Push the 10,000-row list | 14 ms | 55 ms |
| Scroll it by 37 points | 2.0 ms | 1.1 ms |

- **Updates are local**: changing one row of a 5,000-row list re-renders that row only (1 patch, 20–50 µs) and touches no other native view (see [Big trees](#big-trees) below).
- **GPU-composited views**: backgrounds, borders, corners and pictures are layer properties, not per-view bitmaps (macOS graphics memory 28 MB → 0.4 MB in a large window).
- **Web**: one database pool per file for the whole server (not per visitor); sessions sleep until a message, a timer or finished background work.
- **Lazy tabs**: a tab renders on its first selection ([navigation](guide/navigation.md)), so a page ships only the visible tab: the example's `/` is 44 nodes instead of 715, and 17 KB of HTML instead of 150 KB.
- **Release profile**: thin LTO and stripped symbols (binaries −24 %).
- **macOS tabs and stacks**: tabs and the screens under the top one stay in the window, hidden, so going back to them draws nothing again (a warm tab switch 29 ms → 5–18 ms, Back to a large screen 39 ms → 5 ms). Their layers keep their drawing: having visited every tab of the example costs ~12 MB more.
- **No idle timer**: countdowns tick every second only while one is shown; an app without one does not wake up for them.
- **Launch on iOS**: the bridge links AuthenticationServices, PhotosUI and StoreKit only with prismsdk's `sign-in`, `photo-picker` and `purchases` features (#312, #314). Any one of them drags ~1,000 more images into the launch, which `dyld` walks before `main`: in the simulator a one-screen app without them reaches `main` ~0.4 s after its process starts instead of ~1.45 s (a SwiftUI app: ~0.24 s). The example app turns all three on.
- **Android wake-ups and scroll content** (#315, #316): the runtime keeps one queued wake-up, at its next timer, instead of one per event; a scroll view's unchanged content size is not sent again, so Android does not lay its content out again. In #313's benchmark (a 1,000-row list scrolled from code every frame, Android emulator): 65 % → 12.5 % CPU, frames over 25 ms 4 % → 0.2 % (Compose's `LazyColumn`: 15 %).
- **Apple number formatting**: `cx.format_number` reuses one `NSNumberFormatter` per locale (scrolling a list of numbers: −40 % on macOS, −50 % in the iOS simulator).

## Big trees

What every backend shares (rendering entities, diffing them into patches,
applying patches, flexbox layout) is measured by

```sh
cargo test -p prismsdk-native --release -- --ignored --nocapture bench
```

on a native backend that does nothing but count, so the times are Prism's
own and not the platform's. Each case runs on fresh state five times and
keeps its fastest run, with the heap allocations it made. A row is a view
with two texts, so 5,000 rows are 15,001 nodes. Before and after the
November 2026 pass (release build, Apple silicon; the machine was busy, so
absolute times are high, but each pair ran back to back):

| Case | 1,000 rows | 5,000 rows |
|---|---|---|
| First render (core) | 5.5 → 3.8 ms | 30.3 → 24.9 ms |
| First apply (native) | 4.7 → 2.5 ms | 28.3 → 17.6 ms |
| First layout | 3.8 → 2.9 ms | 26.2 → 22.5 ms |
| — native measures in it | 16,000 → 6,000 | 80,000 → 30,000 |
| One row changed: render and apply | 26 → 20 µs | 57 → 46 µs |
| One row changed: layout again | 0.43 → 0.28 ms | 4.9 → 3.0 ms |
| — allocations in it | 2,071 → 62 | 10,086 → 74 |
| The list re-rendered, rows unchanged | 0.41 → 0.05 ms | 7.2 → 0.18 ms |
| A row inserted at the top | 0.46 → 0.09 ms | 7.8 → 0.43 ms |
| Every row changed (the list not) | 14.0 → 2.1 ms | 343 → 13.8 ms |
| The window resized: layout again | 2.9 → 2.4 ms | 20.8 → 18.2 ms |
| Keyed rows reversed | 7.0 → 1.3 ms | 143 → 8.6 ms |
| Rows with a context menu and a hover style: first render and apply | 16.2 → 9.3 ms | 135 → 57 ms |
| — all removed | 6.1 → 2.7 ms | 85.6 → 15.8 ms |
| Rows with a text field under a keyboard bar: one row changed | 5.5 ms → 20 µs | 130 ms → 27 µs |
| Rows with a `z_index` badge: first render and apply | 274 → 18.5 ms | 7.13 s → 106 ms |
| Rows under a sticky header every 50: one row changed, layout again | 0.53 → 0.35 ms | 6.5 → 3.0 ms |
| 100 nested views: first render, apply and layout | 0.48 → 0.33 ms | |

What the numbers rest on:

- **No quadratic loops** in the reconciler: the entities notified since the
  last frame are sorted once; children are matched to the mounted ones by
  key or entity through an index (the first free one is tried first, so a
  list in order needs none); context menu and link handlers are kept by node.
- **Each leaf measured once per width in a layout pass**: taffy asks for a
  leaf's size under several constraints that come down to the same width for
  the platform. Native measures are the platform laying text out (AppKit,
  UIKit, GTK, a JNI call into Android's views), the costly part
  of a native layout.
- **Native bookkeeping in one walk**: which keyboard bar serves each field,
  and paint order with `z_index`, are found from the tree once a batch, not
  for each field or each change.
- **Ids hashed with one multiplication**: the maps keyed by node, entity and
  layout ids use `IdHasher` (FxHash's step) instead of SipHash; maps keyed by
  strings keep the standard hasher.
- **Fewer copies and allocations**: the first render keeps the environment
  patch's place instead of moving every patch for it, native maps reserve a
  batch's nodes at once, a style change draws the previous style again only
  for loop animations, and placing children reuses its lists.

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
