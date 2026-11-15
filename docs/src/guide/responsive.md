# Responsive

```rust
// Styles, from a width up (like Tailwind): phones get a column, tablets a row.
div().flex_col().gap(12.).md(|s| s.flex_row().gap(24.))
text("Swipe to delete").hidden_from(Breakpoint::Medium)   // phones only
sidebar().visible_from(Breakpoint::Expanded)              // laptops and up

// Structure: anything else.
if cx.breakpoint() >= Breakpoint::Expanded { two_panes() } else { one_pane() }
cx.on_resize(|(w, h), app| { … });                        // every size change
```

| Breakpoint | Width | Typically |
|---|---|---|
| `Compact` | < 600 | phones |
| `Medium` (`md`) | ≥ 600 | tablets portrait, small windows, phones in landscape |
| `Expanded` (`lg`) | ≥ 1024 | iPad landscape / 13", laptops |
| `Large` (`xl`) | ≥ 1440 | desktops |

Rules are resolved in the core for the current window, so every backend agrees and the web needs no media queries: the iOS window (rotation, Split View, iPad), the macOS window (resize) and the browser (resize, plus a cookie so the server's first render already fits) report their size, and the app re-renders when it crosses a breakpoint. On wide browser windows the tab bar becomes a sidebar.
