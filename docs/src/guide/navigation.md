# Navigation

A URL-style router, rendered with each platform's native navigation:

```rust
Application::new().title("Prism").run(|cx| {
    let store = cx.new_entity(|_| Store::new());
    Router::new()
        .route("/", move |cx, _| cx.new_entity(|cx| Home::new(store, cx)))
        .route("/todos/:id", move |cx, p| {
            let id = p.parse("id").unwrap_or(0);
            cx.new_entity(|cx| TodoDetail::new(store, id, cx))
        })
        .build(cx)
});

// A route's view returns a screen; its title goes in the navigation bar.
screen("Details").child(...)

// Anywhere:
cx.navigate("/todos/2");   cx.back();   cx.replace("/login");   cx.reset("/");
div().on_click(navigate_to("/todos/2"))   // handler helpers: navigate_to, go_back
```

| | Stack | Back | Deep links |
|---|---|---|---|
| **iOS** | `UINavigationController` (large titles, push animation) | system button + edge swipe | `PRISM_PATH=/todos/1` |
| **macOS** | header bar (title, Back button) | Back button, Go ▸ Back ⌘[ | `PRISM_PATH=/todos/1` |
| **Web** | header bar | header button + browser Back/Forward (History API) | any URL, server-rendered |

**Tabs**: `.tab("Todos", "checklist", "/")` per tab (SF Symbol name; a glyph on the web). Each tab keeps its own stack. `navigate()` to a tab's root path switches tab. iOS: `UITabBarController`. macOS: segmented control. Web: ARIA tablist, URL follows the tab.

**Scrolling**: `scroll()` (`UIScrollView` / `NSScrollView` / `overflow:auto`). A screen starting with a scroll view lets it run under the bars, so iOS large titles collapse natively.

Patterns: `/`, `/todos`, `/todos/:id`, `/files/*rest`. Unknown paths show a "Not found" screen (or `.not_found(...)`); on the web that page is answered with `404` (`cx.shows_not_found()` tells the app). Opening a deep link puts `/` underneath so Back works.
