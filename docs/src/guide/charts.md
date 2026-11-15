# Charts and vector drawing

`prism::ui` has charts, drawn natively with vector shapes: UIKit Bézier paths (iOS, tvOS, visionOS), Core Animation shape layers (macOS), `Canvas` (Android), SwiftUI (watchOS), SVG (web), cairo (Linux).

```rust
ui::line_chart()
    .series("This week", [1.2, 1.9, 1.6, 2.4, 2.1, 3.1, 2.8])
    .series("Last week", [1.0, 1.4, 1.5, 1.7, 1.9, 2.2, 2.0])
    .labels(["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"])
    .smooth().area().points().unit(" k€")

ui::bar_chart()
    .bars([("Mon", 12.), ("Tue", 18.), ("Wed", 9.)])
    .highlight(self.day)                                        // the others fade
    .on_select(cx.listener(|this, e, _| this.day = Some(e.index())))

ui::pie_chart().slice("Rent", 900.).slice("Food", 420.).donut("1 320 €")
ui::ring(0.72).label("72%").caption("Goal")                     // circular progress
ui::stat("Revenue", "12 480 €").delta("+12%", true).trend([8., 9.5, 9., 11.])
ui::sparkline([3., 5., 4., 8.])
```

- **Themed**: series use `Theme::chart_color(i)` (the accent first), adaptive in dark mode; axis labels use the app's number format.
- **Axes** pick round ticks (1, 2, 2.5, 5 × 10ⁿ). Curves never overshoot the data (monotone interpolation).
- **Bars** are native elements: tappable, read one by one by screen readers, and animated when values change. Other charts read as one element with their values.
- Charts mirror in right-to-left languages, like the layout around them.

Draw your own with `shape()`: coordinates are fractions of the frame, so drawings follow the layout size.

```rust
shape()
    .inset(2., 2.)                                              // points kept free for strokes
    .path(Path::smooth([(0., 1.), (0.5, 0.2), (1., 0.6)]).stroke(theme.accent, 2.).round())
    .path(Path::sector(0.5, 0.5, 0.5, 0.3, 0., 120.).fill(theme.success))   // donut slice, degrees
    .size(px(120.))
    .label("Trend: up")                                         // decorative unless labelled
```

`Path` also has `line`, `arc`, `circle`, `rect`, `move_to`/`line_to`/`curve_to`/`arc_to`/`close`, `.fill`, `.stroke`, `.dashed`.
