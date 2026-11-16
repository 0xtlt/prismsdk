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

**Lazy tabs**: a tab renders when it is first selected (a tap, `navigate()` to its root, a deep link), then stays rendered: switching away keeps its screens, their state and their views (a half-typed field is still there when you come back). Until then it shows only its tab-bar entry (title and icon) over an empty panel, and its screens are built but not rendered. A page then ships only the visible tab: on the example app, `/` went from 715 nodes, 150 KB of HTML and a 384 KB first WebSocket update to 44 nodes, 16 KB and 24 KB. A deep link (`/components`, `/components/list`) renders its tab on the server, in the first paint. `cx.reset(..)` makes every tab lazy again. Opt a tab out with `.eager()` right after it, to render it up front:

```rust
Router::new()
    .tab("Todos", "checklist", "/")
    .tab("Player", "play.circle", "/player")
    .eager() // rendered from the start, not on first selection
```

Make a tab eager when its views must exist while hidden, or when switching to it must not wait for a render. On the web the first switch to a lazy tab takes one round trip to the server: the tab is selected at once and the panel shown stays up until the new one arrives, so no empty panel flashes (a timeout shows it after a second if no answer comes). Native backends render in the app's process, with no round trip. watchOS pages through tabs with the Digital Crown, which shows the next page while it scrolls, so the tabs next to the selected one render too. `router.tab_rendered(i)` tells whether a tab is rendered.

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

## Bar buttons and the large title

A screen puts its actions in the navigation bar rather than in its content, and chooses its title size:

```rust
screen("Todos")
    .large_title(false)                                   // a small title, even on iOS
    .leading(bar_button("Edit").on_click(cx.listener(|this, _, _| this.editing = !this.editing)))
    .trailing(
        bar_button("New todo")                            // announced by assistive technologies
            .icon("plus")                                 // an SF Symbol; a glyph elsewhere
            .on_click(Rc::new(|_: &ClickEvent, cx: &mut App| cx.present("/todos/new"))),
    )
```

Leading buttons come after Back (which stays), trailing ones at the trailing edge, each side in the order they were added; right to left, the sides swap. A button with an icon shows the icon where the platform has symbols and is announced by its title (or `.accessibility_label(..)`). A screen marked `.bare()` hides them with its bar.

| | Bar buttons | `large_title(false)` / `(true)` |
|---|---|---|
| **iOS, iPadOS, visionOS** | `UIBarButtonItem`s (`leftBarButtonItems` with `leftItemsSupplementBackButton`, `rightBarButtonItems`); a sheet's Close stays first | `largeTitleDisplayMode` `.never` / `.always` (default `.automatic`) |
| **tvOS** | `UIBarButtonItem`s | no large titles on tvOS: ignored |
| **macOS** | borderless `NSButton`s in the header bar (SF Symbol image or title) | one title size: ignored |
| **Web** | `<button class="prism-bar-button">` in `.prism-header` (server-rendered too): the icon's glyph, else the title | ignored |
| **Linux** | flat `GtkButton`s in the header: the icon's glyph, else the title | ignored |
| **Android, Wear OS** | borderless buttons in the header: the icon's glyph, else the title | ignored |
| **watchOS** | `ToolbarItemGroup`s (`.topBarLeading`, `.topBarTrailing`) with SF Symbols | ignored |

A tap reaches the screen as a change event of the button's index (`UiEvent::Change(screen, i)`); the handler you gave runs. The options travel as the screen's config (`prism_core::navbar::ScreenConfig`).

## Tinting the tab bar and the header bars

The selected tab and the header bars' buttons (Back included) take the app's accent color: `ui::set_theme` sets it from the theme's accent (`cx.set_accent_color(Some(color))` without prism-ui). A router can have its own:

```rust
Router::new().tint(rgb(0xFF9500)) // wins over the accent color
```

| | Tab bar | Header bars |
|---|---|---|
| **iOS, iPadOS, tvOS, visionOS** | `UITabBar.tintColor` | `UINavigationBar.tintColor` (Back; bar buttons where the system tints them) |
| **macOS** | the selected segment's symbol (and its bezel where the appearance draws one) | Back's and the bar buttons' `contentTintColor` |
| **Web** | `--prism-tint` on `.prism-tabs`: the selected tab's color | `--prism-tint` on `.prism-stack`: Back and the bar buttons |
| **Linux** | CSS: the checked tab button | CSS: the header's buttons |
| **Android, Wear OS** | the selected tab's color | Back and the bar buttons |
| **watchOS** | `.tint` on the `TabView` | `.tint` on the `NavigationStack` |

## Closing the router (signing out)

An app that signs out replaces its root (the tab router) with a sign-in screen. Release the router and everything it holds goes with it:

```rust
fn sign_out(cx: &mut App, router: Entity<Router>) {
    cx.release(router);                       // every screen of every tab, and the sheet
    let page = cx.new_entity(|_| SignIn);
    cx.set_root(page);
}
```

Releasing the router releases each screen it built (`Router::close` does it without releasing the router; `cx.reset(path)` opens screens again), and releasing an entity releases what it created with `cx.new`, drops the observers it registered with `cx.observe` and cancels its tasks spawned with `.cancel_on_release()`. A screen a route hands out without creating it (`.route("/focus", move |_, _| focus)`) is the app's: the router never releases it. Once the app's router is released, `cx.current_path()` is `None`; signing in again builds a new router, which becomes the navigator. `cx.entity_count()` counts live entities, for tests that check nothing is left behind.
