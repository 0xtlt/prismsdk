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

## Charts in widgets, Live Activities and car screens

The same charts and shapes work in `Widget::render`, `Activity::lock_screen`/`compact`/`expanded` and car screens:

```rust
Widget::new("todos", "Todos").families(&[WidgetFamily::Medium, WidgetFamily::AccessoryCircular])
    .render(move |cx, family| ui::ring(done).label("2/3").size(56.).thickness(6.))
Activity::new("focus").lock_screen(div().flex_row().child(ui::ring(today / 4.).size(48.)).child(timer(ends)))
CarItem::new("Today").drawing(ui::ring(0.6).drawing(cx))       // also ui::sparkline(…).drawing(cx), any Drawing
```

| Surface | iOS / macOS / visionOS | Android |
|---|---|---|
| Widgets (all sizes) | SwiftUI `Path`s at the node's frame (`PrismRenderer.swift`) | a bitmap per drawing in `RemoteViews` (`PrismSurface.java`) |
| Lock-screen widgets (`Accessory*`) | `widgetRenderingMode`: in vibrant and accented modes colored paths are white and `widgetAccentable`, gray ones (grids, tracks) recede | — (no lock-screen widgets) |
| Live Activity, Dynamic Island | SwiftUI paths, every region | ongoing notification: bitmaps |
| Car screens | CarPlay list/grid images (`UIImage` with day and night variants) | Android Auto / Automotive row and grid images (`CarIcon` bitmap, day or night) |

- **Snapshots** carry the drawing unresolved (fractions of the frame), so each renderer draws it at the size it lays out. Numbers are short (`.25`, opaque colors without alpha): a sparkline snapshot is ~0.5 KB, a ring with its label ~0.3 KB, a 7-point smooth line chart with its axes ~1.8 KB.
- **Static variant**: while a surface renders, `cx.in_snapshot()` is true. The bar chart then draws its bars in its shape (nothing is tappable there), and overlays (a ring's label) use negative margins, which snapshots now keep.
- **Light/dark and right to left** follow the host: adaptive colors resolve with the widget's color scheme (Android 12+: day and night bitmaps via `RemoteViews.setIcon(…, night)`), and drawings flip with the layout direction.
- **Limits**, enforced:
  - ActivityKit: 4 KB for a Live Activity's static and dynamic data together. Content over it has its drawings rounded to two decimals, then left out, with a log line (`ActivityContent::payload_size`).
  - WidgetKit timelines read the snapshot from the App Group container, so there is no payload limit there; the extension's memory budget is the constraint, and vector paths cost almost nothing.
  - Android widgets: the bitmaps and icons of one `RemoteViews` may not exceed screen width × height × 4 × 1.5 bytes (`AppWidgetManager.updateAppWidget`, an error from Android 17), shared by every size of the widget.
  - Android notifications: the system warns above 2 000 000 bytes of custom views and strips them above 5 000 000 (`config_notificationWarnRemoteViewSizeBytes`, `…Strip…`). Prism keeps the collapsed and expanded views under 2 MB together.
  - Past a budget, bitmaps are drawn at a lower resolution (and left out below a quarter of it). RemoteViews do not report their size: a drawing's bitmap is made for the size estimated from the host's bounds, fixed sizes and measured text, then stretched to its real frame.
- **Watch complications**: this repository has no watchOS widget extension yet, so there are none; the watch app itself draws charts with SwiftUI.
