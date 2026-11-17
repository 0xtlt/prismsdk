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

## Reversed directions and `order`

```rust
// A box after its label in a narrow inspector, before it from `md`.
div().flex_row().gap(8.)
    .child(text("Show in menu"))
    .child(checkbox.order(1).md(|s| s.order(-1)))

div().flex_row_reverse()      // trailing to leading (`row-reverse`)
div().flex_col_reverse()      // bottom to top (`column-reverse`)
div().flex_row().md(|s| s.flex_row_reverse())
```

`.order(i)` (CSS `order`, `0` by default) sorts a node among its siblings,
lower first, keeping the app's order among equals; it applies to grid items
too. Reversed directions start from the end of the main axis, and
`justify_start` packs the children against that end, as in CSS. Both only
move nodes on screen: the keyboard's focus order and assistive technologies
follow the tree, as on the web. Both work in refinements (`md`, `at_width`,
`when_width_at_least`, interaction states), so a row can change its visual
order per breakpoint without rebuilding it in `render`. Right to left, a
reversed row starts at the left edge.

## Auto margins

`m_auto()`, `mx_auto()`, `my_auto()`, `mt_auto()`, `mr_auto()`, `mb_auto()` and `ml_auto()` give a side an `auto` margin, which takes the free space like CSS `margin: auto` (#228):

```rust
div().w_full().max_w(px(1152.)).mx_auto()                    // a centered page column
div().flex_row().child(title).child(actions.ml_auto())        // pushed to the end, no spacer
div().h_full().child(content).child(footer.mt_auto())         // pinned to the bottom
```

Leading and trailing follow the text direction, like `ml` and `mr`. A margin in points on the same side (`ml(8.)`) replaces `auto`. They work in refinements (`.md(|s| s.mx_auto())`). The web writes `margin-*: auto`; native backends use taffy's auto margins.

## Paint order: `z_index`

`.z_index(n)` draws a node above its siblings, like CSS `z-index` (#232): a handle sticking out of a grid cell over the next cell, a badge over a neighbour. Higher is above, 0 (the default) keeps the tree order, equal values keep theirs. Clicks go to what is drawn on top. It changes the drawing only, never the layout, and works in refinements (`.hover(|s| s.z_index(1))`).

```rust
div().flex_row()
    .child(cell().child(handle().absolute().right(px(-12.)).z_index(1)))   // over the next cell
    .child(cell())
```

The web writes `z-index` (flex and grid items take it as they are). Native backends order each parent's views by the value each child shows: its own, or the highest one inside it up to a node that sets its own, so the cell holding the handle is drawn after the next cell. Stacks and tab bars keep their order.

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

## Scroll bars

```rust
scroll().scrollbar(Scrollbar::Overlay)  // thin, over the content, never takes room
scroll().scrollbar(Scrollbar::Hidden)   // no bar; still scrolls (wheel, trackpad, keys, touch)
```

By default (`Scrollbar::Auto`) each platform shows its usual bars: on macOS
with a mouse, or with « Show scroll bars: Always », they stay and take room
(#132). Areas whose design has no bar (a sidebar, a canvas, a row of tabs)
can keep a thin overlay bar or none. Keep the bars where users need to see
how long the content is.

| | Overlay | Hidden |
|---|---|---|
| Web | `scrollbar-width: thin`, shown while the pointer is over the view or it has the focus (browsers with overlay bars show them while scrolling) | `scrollbar-width: none` (and `::-webkit-scrollbar` for older Safari) |
| macOS | `NSScrollerStyle.overlay`, kept when the setting changes; a row gets a horizontal overlay bar | no scroller |
| iOS, iPadOS, visionOS, watchOS, Android | the usual indicators (already overlay) | indicators off |
| Linux | GTK overlay scrolling | `PolicyType::External` |

## Where an element ended up

`on_layout` hears an element's frame after each layout that moved or resized it, and `cx.frame_of(id)` reads the last one of an element with an `.id()` that reports it (`on_layout`, or `track_layout()` without a handler), #192. Hit-test a rubber-band selection against rows, find the drop slot under the pointer, without estimating heights from fonts:

```rust
div()
    .children(self.rows.iter().map(|r| {
        div().id(format!("row-{}", r.id)).track_layout().child(row_view(r))
    }))
    .on_drag(cx.listener(|this, e: &DragEvent, cx| {
        // The pointer, in the same space as the frames.
        let Some(list) = cx.frame_of("list") else { return };
        let at = list.to_layout(e.position());
        this.hovered = this.rows.iter().position(|r| {
            cx.frame_of(&format!("row-{}", r.id)).is_some_and(|f| f.contains(at))
        });
    }))
```

A frame (`Rect { x, y, width, height }`, with `contains`, `intersects`, `center`) is in points, in the window's layout coordinates **before scrolling**: two elements in the same `scroll()` keep their distance as it scrolls, so frames and a point measured from one of them (`to_layout`) compare at any scroll offset. Add the scroll view's offset (`on_scroll`) for a position on screen. Transforms (`offset`, animations) are not included. Native backends report it from Prism's layout (the frames of the ancestors added up, as soon as a layout moves it); the web from the page's (`getBoundingClientRect` with the scroll offsets added back, after each update, on resize and when the element itself resizes). Frames of different screens or overlays are not in a shared space.

## Backends

| | Web | macOS | iOS · tvOS · visionOS | watchOS | Linux | Android |
|---|---|---|---|---|---|---|
| Grid, weights, gaps, aspect ratio | CSS | ✓ | ✓ | ✓ | ✓ | ✓ |
| Baselines | CSS | ✓ | ✓ | ✓ | ✓ | ✓ |
| Sticky | CSS | ✓ | ✓ | ✓ | ✓ | ✓ |
| Fixed | CSS | ✓ | ✓ | ✓ | ✓ | ✓ |
| Horizontal scroll | CSS | ✓ | ✓ | ✓ | ✓ | ✓ |
| Reversed directions, `order` | CSS | ✓ | ✓ | ✓ | ✓ | ✓ |
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
