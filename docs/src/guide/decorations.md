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
| `.bg_gradient(Gradient::linear(angle, colors))` | a gradient over the background: `angle` in CSS degrees (`180` top to bottom, `90` left to right); `Gradient::radial(colors)` goes from the center to the corners; `.with_stops([(color, 0.2), …])` places the colors |
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
| `.group_hover(…)`, `.group_active(…)`, `.group_focus_within(…)` | an ancestor marked `.group()` is in that state (a row's actions under the pointer, a field's Clear button) |
| `.cursor(Cursor::Text)` | the pointer's shape over the node; clickable nodes show a pointing hand by default |

Refinements are written like the [responsive](responsive.md) ones and apply on top of them; when several states hold at once, they apply in this order (later wins): group hover, group focus-within, group press, hover, focus-within, focus, press, disabled. With `.animate(ms)` (see [Motion](motion.md)) the change animates. A refinement may change anything, layout included (`display_none`, sizes); native backends lay out again when it does.

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

## Platform notes

| | Shadows | Borders | Clip | Gradients | Backdrop blur |
|---|---|---|---|---|---|
| Web | `box-shadow` | `border-*` (logical sides) | `overflow: hidden` | `linear-gradient()`, `radial-gradient()` | `backdrop-filter` |
| macOS, iOS, tvOS, visionOS | Core Animation layers; drop shadows live behind the view so `clip` doesn't cut them | shape layers (per-side borders are straight between the corners) | `masksToBounds`, per-corner `maskedCorners` | `CAGradientLayer` | `NSVisualEffectView`, `UIVisualEffectView`: the radius picks a material (tvOS has Regular and Prominent only) |
| watchOS | SwiftUI shadows of the box's shape (spread and inset drawn by hand) | stroked paths | `clipShape` | `LinearGradient`, `EllipticalGradient` | system materials |
| Linux | GTK CSS `box-shadow` | GTK CSS | `Overflow::Hidden` | GTK CSS gradients | none in GTK 4: the translucent background is drawn without blur |
| Android | drawn with `BlurMaskFilter` on the box's path | `DashPathEffect` | outline, or a path clip | `LinearGradient`, `RadialGradient` | no per-view backdrop API: Prism draws what is behind into a bitmap and blurs it (`RenderEffect` on Android 12+, a downscale before) |

Drop shadows don't follow animated frame changes yet on Apple platforms (they jump to the end). Widgets and Live Activities don't draw the new decorations.
