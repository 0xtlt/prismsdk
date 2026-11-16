# Layout: grids, sticky and fixed, scrolling sideways

Every node is a flex container (a column by default, like React Native).
Beyond flexbox, Prism has CSS grid, weighted grow and shrink, per-axis gaps,
baseline alignment, aspect ratios, sticky and fixed positioning, and
horizontal scroll views. The web emits the matching CSS; native backends lay
out with the same algorithms (taffy's flexbox and CSS grid in
`prism-native`), so both agree.

## Grid

```rust
// As many 172 pt columns as fit, sharing what is left (CSS
// `repeat(auto-fill, minmax(172px, 1fr))`): no column count to compute
// from the window, no padding cells.
div().grid().grid_cols([auto_fill(minmax(172., fr(1.)))]).gap(12.)
    .children(cards)

div().grid().cols(3)                                  // 3 equal columns
div().grid().grid_cols([px(240.).into(), fr(1.)])     // sidebar + content
div().grid().grid_rows([px(48.).into(), fr(1.)]).grid_auto_rows(fr(1.))

card().col_span(2)                    // spans two columns
card().col_start(1).row_start(2)      // explicit placement (lines from 1)
```

Tracks: points (`172.`, `px(172.)`), `relative(0.25)`, `fr(1.)`,
`GridTrack::Auto`, `MinContent`, `MaxContent`, `minmax(min, max)`,
`repeat(n, track)`, `auto_fill(track)`, `auto_fit(track)` (empty columns
collapse, so the items stretch). The cells of a row are as tall as the row
(equal-height cards) unless the grid says otherwise (`items_start()`).

## Flex weights, gaps, baselines, aspect ratio

```rust
div().flex_row()
    .child(list.flex_1())
    .child(detail.flex_1().flex_grow(2.))   // twice the list's share
    .child(badge.flex_shrink(0.))           // never shrinks
div().flex_row().flex_wrap().gap_x(8.).gap_y(12.)   // per-axis gaps

// A big figure and its unit on one baseline, an icon beside a label.
div().flex_row().items_baseline()
    .child(text("42").text_size(40.)).child(text("kg"))
text("…").self_baseline()

picture("cover.jpg").w_full().aspect_ratio(16. / 9.)  // height follows width
```

Native backends measure each text leaf's first baseline (the font's ascent
where the control draws its text: labels, buttons, inputs, pickers, timers);
views without text sit on the baseline with their bottom edge, as in CSS.

**Items shrink below their content.** Every node's minimum width is `0`
unless it sets one (the web's `min-width: 0` on every node, and the native
layout's default too), so a row that does not fit shrinks its items
(weighted by `flex_shrink` and their size) rather than overflowing: a text
wraps, a label beside an icon in a narrow cell takes two lines. Keep an item
at its size with `.flex_shrink_0()` or `.min_w(px(…))`. Switches and spinners
keep their own width. Heights keep CSS's automatic minimum (a column's
children are at least as tall as their content). A text's narrowest natural
width (its min-content, used by `items_start` columns and grid tracks) is its
longest word.

## Padding on every node

`.p()`, `.px()`, `.py()`, `.pt()`, `.pr()`, `.pb()` and `.pl()` work on
every kind of node, not only containers: the box (background, border,
rounding) grows by the padding, and the content is drawn inside it.
`pl`/`pr` are leading and trailing, so they swap right to left.

```rust
// A chip: no wrapper needed.
text("Overdue").px(12.).py(4.).bg(red.with_alpha(0.14)).rounded(999.)

// Room for a leading icon, in a field without a border.
text_input().leading_icon("magnifyingglass").pl(36.).py(8.)
    .bg(surface).rounded(10.).border(0.)
```

What padding means per kind:

- **Texts and timers**: the text is inset by it.
- **Buttons, text inputs, text areas**: it replaces the control's own
  inset around its title or text (the default look, with its built-in
  inset, is kept when no padding is set). An explicit zero counts:
  `.p(0.)` (or `.px(0.)`, `.pt(0.)`…) leaves no inset at all, and the
  field is as tall as its line unless `.h(…)` is set, e.g. an inline
  editor that sits exactly where its text was. A field with padding, a
  background, a border or `.border(0.)` draws Prism's box instead of the
  platform's bezel: its own background and border, else a 1-point gray
  border rounded 6, and a default 6 × 8 inset when it sets no padding (as on
  the web). A leading icon keeps at least 32 points before the text.
  A field taller than its line (`.h(36.)`) centers its text vertically,
  on macOS too. Such a field, or one with a `.focus(…)` refinement, does
  not get the web page's default `:focus-visible` outline: its own look
  (and the caret) shows the focus, as natively.
- **Other controls** (pickers, date pickers, switches, sliders, progress
  bars, spinners, checkboxes, radio groups, images, pictures, shapes):
  natively they are placed inside their padding, keeping their own inset;
  on the web the padding is the element's CSS padding (a select keeps room
  for its arrow after it).

`.border(0.)` removes every border, the default one of text inputs, text
areas, pickers and date pickers included (`.border(w)` with `w > 0` brings
borders back). Padding works in interaction states
(`.hover(|s| s.px(16.))`), responsive and own-width rules, and in the
server-rendered first paint.

## Sticky and fixed

```rust
scroll().child(
    div().child(section_header.sticky().top(0.)).children(rows)
)
button("+").fixed().bottom(24.).right(24.)   // a floating action button
```

- `.sticky()` stays in the flow, then stays `.top(…)` points from the top of
  its scroll view while its parent is in view, and leaves with it (CSS
  `position: sticky`). It is drawn above the content scrolling under it:
  give it a background.
- `.fixed()` is placed at its insets from the **screen** it is on (the
  stack screen, tab or sheet; the window when there is none), never
  scrolls, and ignores its parents' positions: declare it anywhere.
  Natively it is shown by the screen itself, inside the safe area, above
  the rest; on the web the area holding the screen (a stack's body, the tab
  panels) is the containing block (`contain: layout`), so `position: fixed`
  sits below the navigation header and above the tab bar too, and stays put
  when a screen taller than the window scrolls by itself. `.absolute()` stays relative to the parent.

Natively the layout pass works out where each sticky node sits in its scroll
view; the backend moves it as the content scrolls without a round trip
through the app.

## Scrolling sideways

```rust
scroll_x().gap(8.).children(chips)      // or scroll().horizontal()
scroll_x().child(wide_table.w(900.))
```

A horizontal scroll view is a row, as tall as its content; its children keep
their size (they never shrink to the view) and the view scrolls when they
are wider. Right to left its content is mirrored; the browser opens it at
its leading (right) edge, native scroll views at offset 0.

## Backends

| | Web | macOS | iOS · tvOS · visionOS | watchOS | Linux | Android |
|---|---|---|---|---|---|---|
| Grid, weights, gaps, aspect ratio | CSS | ✓ | ✓ | ✓ | ✓ | ✓ |
| Baselines | CSS | ✓ | ✓ | ✓ | ✓ | ✓ |
| Sticky | CSS | ✓ | ✓ | ✓ | ✓ | ✓ |
| Fixed | CSS | ✓ | ✓ | ✓ | ✓ | ✓ |
| Horizontal scroll | CSS | ✓ | ✓ | ✓ | ✓ | ✓ |
| Padding on leaves | CSS | ✓ | ✓ | ✓ | ✓ | ✓ |
| `.border(0.)` on fields | CSS | ✓ | ✓ | ✓ | ✓ | ✓ |

Notes per platform:

- Baselines come from the platform's text metrics: `firstBaselineOffsetFromTop`
  (AppKit), the font's ascender where UIKit and SwiftUI place the text,
  `getBaseline()` (Android), GTK's measured baseline. Views without text
  align their bottom edge.
- Sticky views move with the scroll view natively (UIKit's
  `scrollViewDidScroll`, AppKit's clip-view bounds, GTK's vertical
  adjustment, Android's scroll listener, the watch's scroll offset) and are
  drawn above their siblings. On macOS that means they are last among their
  siblings, so VoiceOver reaches them last.
- macOS horizontal scroll views have no visible scroller (legacy scrollers
  would take height from the row): trackpad swipes and shift + wheel scroll
  them. Scroll events (`on_scroll`) stay vertical.
- Sticky only follows vertical scroll views.
- Padding on leaves: the layout adds it around the measured content on
  every native backend; then each draws the content inside it. macOS
  labels draw their box and inset their text (`NSCell`), padded buttons get
  a bezel as tall as their frame (`NSBezelStyleFlexiblePush`) with the title
  centered in it, so uneven leading and trailing padding is not honored on
  native macOS buttons. UIKit labels inset their text rect, buttons take it
  as `contentEdgeInsets`, fields as their text rect (a `UITextField`
  subclass) and text areas as `textContainerInset`. GTK applies it as the
  widget's CSS padding (text views: their margins), Android with
  `setPadding`, the watch with SwiftUI's `.padding` (fields lose their
  platter). Android pickers keep their arrow background with `.border(0.)`.
