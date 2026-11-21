# Decorations and interaction states

```rust
// A card: soft elevation, children clipped to its rounded corners.
div().bg(t.surface).rounded(16.).clip().shadow(shadow_sm()).shadow(shadow_md())

// A list row: a separator under it, feedback under the pointer and while pressed.
div()
    .border_bottom(1., Color::current().with_alpha(0.12))
    .hover(|s| s.bg(Color::current().with_alpha(0.06)))
    .active(|s| s.bg(Color::current().with_alpha(0.14)))
    .on_click(…)

// An "offline" placeholder, a gradient banner, a frosted bar.
div().border(2.).border_color(Color::current().with_alpha(0.35)).border_dashed()
div().bg_gradient(Gradient::linear(135., [t.accent, rgb(0xBF5AF2)]))
div().absolute().bottom(0.).left(0.).right(0.).backdrop_blur(16.).bg(rgba(0xFFFFFF, 0.45))

// A hue wheel (a custom color swatch), a pie-like progress disc at 30 %.
let (red, yellow, green) = (rgb(0xFF0000), rgb(0xFFFF00), rgb(0x00FF00));
let (cyan, blue, magenta) = (rgb(0x00FFFF), rgb(0x0000FF), rgb(0xFF00FF));
div().size(24.).rounded(12.).bg_gradient(Gradient::conic(0., [red, yellow, green, cyan, blue, magenta, red]))
div().size(40.).rounded(20.).bg_gradient(
    Gradient::conic(0., [t.accent, t.border])
        .with_stops([(t.accent, 0.), (t.accent, 0.3), (t.border, 0.3), (t.border, 1.)]),
)

// A search field whose box rings while it is edited, with a Clear button
// shown only then.
div()
    .group()
    .border(1.).border_color(t.border)
    .focus_within(|s| s.border_color(t.accent).shadow(Shadow::ring(3., t.accent.with_alpha(0.25))))
    .child(text_input().on_input(…))
    .child(
        clear_button
            .with_style(|s| s.display_none = true)
            .group_focus_within(|s| s.with_style(|s| s.display_none = false)),
    )
```

The Components tab of `examples/counter` has all of them (the Decorations card).

## Box decorations

| Builder | What it draws |
|---|---|
| `.shadow(Shadow::new(x, y, blur, color))` | a drop shadow; call it again to stack several (the first is on top, like CSS). `.spread(v)` grows it, `.inset()` draws it inside the box, `Shadow::ring(width, color)` is a focus ring; `shadow_sm()`, `shadow_md()`, `shadow_lg()` are ready-made elevations; `.shadow_none()` removes them |
| `.border_top(w, color)`, `.border_bottom(…)`, `.border_left(…)`, `.border_right(…)` | one side's border (left and right are leading and trailing: they swap in right-to-left languages); the other sides keep `.border(w)` / `.border_color(c)` |
| `.border_dashed()`, `.border_dotted()` | dashes (3 × width on, 2 × width off) or round dots |
| `.clip()` | children are clipped to the box, rounded corners included (`overflow: hidden`) |
| `.bg_gradient(Gradient::linear(angle, colors))` | a gradient over the background: `angle` in CSS degrees (`180` top to bottom, `90` left to right); `Gradient::radial(colors)` goes from the center to the corners; `Gradient::conic(from, colors)` turns around the center, clockwise from `from` degrees (`0` up, like CSS `conic-gradient(from …)`; repeat the first color last for a seamless wheel); `.with_stops([(color, 0.2), …])` places the colors |
| `.backdrop_blur(radius)` | blurs what is behind the node; give it a translucent `.bg(…)` to tint it |

`Color::current()` is the text color of the node it is used on (CSS `currentColor`): `Color::current().with_alpha(0.12)` is a 12 % tint that follows dark mode and themes without being recomputed at each call site. It works in backgrounds, borders, shadows and gradients. Natively it resolves against the node's `text_color`, or the platform's label color (black, white in dark mode) when the node has none; on the web, against the node's `text_color` too, else the inherited `currentColor`.

Shadow offsets are physical (positive `x` goes right in every language), like CSS.

## Interaction states

| Builder | Applies while |
|---|---|
| `.hover(\|s\| …)` | the pointer is over the node or a descendant |
| `.active(\|s\| …)` | the node is pressed |
| `.focus(\|s\| …)` | the node has the keyboard focus (a field being edited, a control reached with Tab or the TV remote) |
| `.focus_within(\|s\| …)` | the node or a descendant has the keyboard focus |
| `.disabled(\|s\| …)` | the node is disabled (`accessibility_disabled(true)`); a disabled node ignores its hover, press and focus refinements |
| `.group_hover(…)`, `.group_active(…)`, `.group_focus_within(…)` | the nearest ancestor marked `.group()` is in that state (a row's actions under the pointer, a field's Clear button); a nested `.group()` stops an outer one's states (#273) |
| `.group_hover_named("row", …)`, `.group_active_named(…)`, `.group_focus_within_named(…)` | the ancestor marked `.group_named("row")` is in that state, whatever groups sit in between (#212) |
| `.cursor(Cursor::Text)` | the pointer's shape over the node; clickable nodes show a pointing hand by default. Resize handles have `ResizeN`, `ResizeS`, `ResizeE`, `ResizeW`, the corners `ResizeNe`, `ResizeNw`, `ResizeSe`, `ResizeSw`, the diagonals `ResizeNeSw`, `ResizeNwSe`, and `ZoomIn` / `ZoomOut` (#223): CSS names on the web and GTK, macOS 15's frame-resize and zoom cursors (two-way arrows, a crosshair at corners before), Android's double arrows, Windows' system cursors (two-way arrows; no zoom cursor: the arrow) |

Refinements are written like the [responsive](responsive.md) ones and apply on top of them; when several states hold at once, they apply in this order (later wins): group hover, group focus-within, group press, hover, focus-within, focus, press, disabled. With `.animate(ms)` (see [Motion](motion.md)) the change animates. A refinement may change anything, layout included (`display_none`, sizes); native backends lay out again when it does.

**Named groups** (#212) say which ancestor a refinement follows when groups nest: a grip in a cell of a table, inside a block that reveals its controls on hover. Like CSS `.group/name` and `group-hover/name:`, plain `group_hover` refinements don't follow a named group, and named ones follow only theirs.

```rust
div().group_named("block").child(
    table_row(cells).group().child(
        grip().opacity(0.).w(px(8.)).group_hover_named("block", |s| s.opacity(1.).w(px(16.))),
    ),
)
```

Each state is resolved natively: the platform says when the pointer enters, a press starts or the focus moves, and only that node is restyled (no re-render of the app).

| | Hover | Press | Focus | Cursor |
|---|---|---|---|---|
| Web | `:hover` (pointers only: `@media (hover: hover)`) | `:active` | `:focus-visible`, `:focus-within` | CSS `cursor` |
| macOS | `NSTrackingArea` | mouse down/up | first responder (clicking a clickable view focuses it, unlike the web's `:focus-visible`) | cursor rects (`NSCursor`; no move, help or progress cursor: open hand, arrow) |
| iOS, iPadOS | `UIHoverGestureRecognizer` (pointer, Apple Pencil hover) | touch down/up/cancel | text fields and text areas | iPad pointer: hover effect on clickable nodes, beam for `Text`; other shapes keep the system pointer |
| visionOS | gaze hover | pinch down/up | text fields and text areas | automatic hover highlight |
| tvOS | the focused view (focus engine) | – | the focus engine | – |
| watchOS | – (no pointer) | the button's pressed state | text fields, when SwiftUI reports it | – |
| Linux (GTK) | `:hover` | `:active` | `:focus-visible`, `:focus-within` (GTK CSS) | `set_cursor_from_name` |
| Android | hover listener (mouse, stylus) | touch down/up/cancel | focus listener | `PointerIcon` |
| Windows | `PointerEntered` / `PointerExited` | `PointerPressed` / `PointerReleased`, a button's `IsPressed` | `GotFocus` / `LostFocus` | `ProtectedCursor` with an `InputSystemCursor` (no open or closed hand: the move arrows; no zoom: the arrow) |

## Platform notes

| | Shadows | Borders | Clip | Gradients | Backdrop blur |
|---|---|---|---|---|---|
| Web | `box-shadow` | `border-*` (logical sides) | `overflow: hidden` | `linear-gradient()`, `radial-gradient()`, `conic-gradient()` | `backdrop-filter` |
| macOS, iOS, tvOS, visionOS | Core Animation layers; drop shadows live behind the view so `clip` doesn't cut them | shape layers (per-side borders are straight between the corners) | `masksToBounds`, per-corner `maskedCorners` | `CAGradientLayer` (axial, radial, conic) | `NSVisualEffectView`, `UIVisualEffectView`: the radius picks a material (tvOS has Regular and Prominent only) |
| watchOS | SwiftUI shadows of the box's shape (spread and inset drawn by hand) | stroked paths | `clipShape` | `LinearGradient`, `EllipticalGradient`, `AngularGradient` | system materials |
| Linux | GTK CSS `box-shadow` | GTK CSS | `Overflow::Hidden` | GTK CSS gradients | none in GTK 4: the translucent background is drawn without blur |
| Android | drawn with `BlurMaskFilter` on the box's path | `DashPathEffect` | outline, or a path clip | `LinearGradient`, `RadialGradient`, `SweepGradient` | no per-view backdrop API: Prism draws what is behind into a bitmap and blurs it (`RenderEffect` on Android 12+, a downscale before) |
| Windows | Composition `DropShadow` | XAML `Border`; a `Rectangle` with a dash array for dashed and dotted; one `Path` per side when the colors differ | a Composition geometric clip (rounded corners) | `LinearGradientBrush`, `RadialGradientBrush`; conic drawn as pixels | `AcrylicBrush` |

Drop shadows don't follow animated frame changes yet on Apple platforms (they jump to the end). Widgets and Live Activities don't draw the new decorations.

**Adaptive colors on layers.** Core Animation keeps plain `CGColor`s, so
borders, border sides, shadows, gradient stops and shape strokes of an
`adaptive(light, dark)` color are resolved again when the appearance changes
(the system's, or `cx.set_appearance`): on iOS, tvOS and visionOS from the
root controller's new traits (`traitCollectionDidChange:`), on macOS in
`updateLayer` (views redraw on `viewDidChangeEffectiveAppearance`). GTK
rebuilds the CSS rules of styles with an adaptive color; Android restyles
every node; the web uses `light-dark()`.
