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

**Scrolling**: `scroll()` (`UIScrollView` / `NSScrollView` / `overflow:auto`) fills the space left in its parent (`flex: 1 1 0`) and scrolls the rest. A screen starting with a scroll view lets it run under the bars, so iOS large titles collapse natively. For a list as tall as its content up to a maximum (in a sheet, a card, a popover), use `scroll().fit().max_h(px(320.))`: it sizes to its rows (`flex: 0 1 auto`), stops at `max_h`, then scrolls. Same layout on every backend (taffy natively, CSS on the web).

**Window-sized screens (web)**: the root stack and its visible screen fill the browser window, like a native app: a frame (your own header, a sidebar) stays put and `scroll().flex_1()` scrolls inside it. A screen taller than the window without a scroll view scrolls by itself (its header stays). A tab bar root already worked this way. A root that is not a stack or tabs (a plain view) still scrolls the page.

Patterns: `/`, `/todos`, `/todos/:id`, `/files/*rest`. Unknown paths show a "Not found" screen (or `.not_found(...)`); on the web that page is answered with `404` (`cx.shows_not_found()` tells the app). Opening a deep link puts `/` underneath so Back works.

## Query strings

Paths keep their query string. A route reads it; the URL (web), deep links and `prism dev` restarts keep it:

```rust
.route("/analytics", |cx, p| {
    let period = p.query("period").unwrap_or("7d").to_string();       // ?period=30d
    let rooms: Vec<u32> = p.query_all("room").iter().filter_map(|r| r.parse().ok()).collect();  // ?room=2&room=5
    cx.new_entity(|cx| Analytics::new(period, rooms, cx))
})

// A filter changed on the screen: rewrite its URL, keep the screen (no history entry).
cx.replace_query([("period", "30d"), ("room", "2")]);
// Another query is another screen: Back (browser Back, swipe) returns to the previous one.
cx.navigate("/analytics?period=90d");
// Building links: values are escaped.
format!("/search?{}", encode_query([("q", "a & b")]))     // /search?q=a%20%26%20b
```

Values are decoded (`%20`, `+`); `p.parse_query::<u32>("room")` parses the first one, `p.query_pairs()` lists them all. Tabs match their root without the query: `navigate("/settings?section=privacy")` switches to the Settings tab, then opens that path on it. `cx.current_query()` reads the visible screen's.

## Handing a value to the next screen

For what does not belong in the URL (a "Saved" notice after a form, an object already loaded), pass a typed value along:

```rust
struct Saved(String);
cx.replace_with("/orders/42", Saved("Order saved".into()));   // or navigate_with(path, value)

.route("/orders/:id", |cx, p| {
    let notice = p.state::<Saved>().map(|s| s.0.clone());        // only this navigation has it
    ...
})
```

Only the screen built by that navigation receives it: a reload, a deep link or Back opens the path without it. It is dropped if no screen is built (the path is already on top, or a tab's root).

## Hiding the header bar

An app with its own chrome (a sidebar, its own titles) turns the stack's header off, everywhere or per screen:

```rust
Router::new().header(false)        // every stack, sheets included
screen("Photo").bare()             // just while this screen is on top
```

The screen then gets the whole area and draws its own Back where it wants one (`button("‹ Back").on_click(go_back())`); its title still names the window, the browser tab and the page for screen readers. Back stays available natively: the swipe back (iOS, iPadOS, visionOS, watchOS), the system Back button and gesture (Android, Wear OS), browser Back, ⌘[ on macOS. A sheet keeps its Close action in `cx.dismiss()` and swipe-down / Escape.

| | Hidden with |
|---|---|
| **iOS, iPadOS, tvOS, visionOS** | `UINavigationController.setNavigationBarHidden` as each screen appears (animated with pushes and pops). The swipe back keeps working without the bar (iOS, iPadOS, visionOS); tvOS goes back with the remote's Menu button. |
| **macOS** | the header view is removed; the screen fills the stack |
| **Web** | `.prism-header` gets `hidden` (server-rendered too) |
| **Linux** | the header box is hidden |
| **Android, Wear OS** | the header is `GONE`; the larger area is reported to the layout |
| **watchOS** | `.toolbar(.hidden, for: .navigationBar)` (watchOS 9+) |
