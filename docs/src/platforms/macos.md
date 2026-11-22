# macOS

`prism dev macos` / `prism run macos` (a plain binary, fastest to iterate),
`prism run macos --bundle` (a full `.app`), `prism build macos` (a release
`.app` with widgets and icon).

Every node is a real AppKit view (`NSView`; labels are `NSTextField`s,
buttons `NSButton`s), laid out by prism-native's flexbox. Backgrounds,
borders, corners and pictures are layer properties ([Performance](../performance.md)).

- **Window:** resizing re-renders across [breakpoints](../guide/responsive.md);
  the window title follows the screen title. Apps can draw their own title
  bar ([Custom window chrome](#custom-window-chrome)).
- **Navigation:** a header bar with a Back button and Go ▸ Back ⌘[; tabs are a
  segmented control ([Navigation](../guide/navigation.md)).
- **Menus:** `cx.set_app_menu` (the app's menus in Prism's bar) or
  `cx.set_menu_bar` (the whole bar, with system items such as Copy or
  Minimize, #161), context menus on
  right click or ⌃-click, and a menu bar item with `cx.set_tray`
  ([Dialogs, menus, tray](../guide/dialogs-menus.md)).
- **System surfaces:** WidgetKit widgets, notifications (UserNotifications) and
  Live Activities as a menu bar item with a live countdown need the bundled app
  (`--bundle` or `prism build macos`); see
  [Widgets, Live Activities, notifications](../guide/system-surfaces.md).
- **Device APIs:** CoreLocation, the open panel for photos, the trackpad's
  haptics, `NSPasteboard`, the sharing picker ([Device APIs](../guide/device-apis.md)).
  Location needs the bundled app.
- **Storage:** Application Support; iCloud key-value storage needs a Mac
  provisioning profile ([Data](../guide/data.md)).
- **Accessibility:** `NSAccessibility` roles and labels, Full Keyboard Access,
  focus ring ([Accessibility](../guide/accessibility.md)). Like native
  buttons, a clickable view takes the focus (a click included) only with
  Full Keyboard Access on, or from `cx.focus`; a view with `on_key_down`
  takes it on click but shows the ring only when focused from the keyboard
  or by the app (#177).
- **Background:** `NSBackgroundActivityScheduler` ([Background execution](../guide/background.md)).
- **Pull to refresh:** a trackpad or Magic Mouse gesture that pulls the
  content 60 points past the top (elastic scrolling), or ⌘R while the scroll
  view is in the key window; a spinner sits above the content while
  `refreshing` is on.

The Swift bridge (`crates/prism-apple-build/apple/Bridge`) is compiled by
`prism-macos`'s build script, so Xcode is required. The deployment target is
macOS 13 (`MACOSX_DEPLOYMENT_TARGET`, set by `prism`; change it with
`[macos] deployment_target` in `Prism.toml`, see
[iOS](ios.md#deployment-target)); the WidgetKit parts need macOS 14. The
bundled app embeds the widget extension and its App Group only when the app
registers a widget.

Development hooks: `PRISM_CLICK="+1|Dark mode" PRISM_SCROLL=600 PRISM_DELAY=2
PRISM_SNAPSHOT=out.png cargo run -p counter` ([Development](../development.md)).

## Custom window chrome

Apps with their own title bar (tabs at the top, a sidebar reaching the top
edge) remove the system's one:

```rust
Application::new()
    .title("Notes")
    .window_chrome(WindowChrome::Custom {
        // The traffic lights' top-left corner, in points from the window's
        // top-left corner (`None`: where macOS puts them).
        traffic_lights: Some(Point::new(16., 13.)),
    })
    .run(|cx| cx.new_entity(|_| Notes::default()));

impl Render for Notes {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Room for the window's buttons, dropped in full screen (they hide).
        let (left, right) = cx
            .window_controls()
            .map_or((0., 0.), |c| c.insets(cx.window_size().0));
        div().size_full().flex_col()
            .child(div().h(40.).pl(left + 12.).pr(right + 12.).flex_row().items_center()
                .window_drag()                    // moves and zooms the window
                .child(tabs(cx)))                 // clicks still reach the tabs
            .child(body(cx))
    }
}
```

- The window has a full-size content view with a transparent, untitled title
  bar: the content is laid out from the window's top edge, and
  `cx.viewport()` / `cx.safe_area()` report the whole window (no title bar
  inset). The title (`.title(…)`, then the screen's) still names the window in
  Mission Control, the Window menu and for VoiceOver.
- The traffic lights stay where `traffic_lights` puts them after resizes,
  live resizes, full screen and appearance changes. Their invisible bar grows
  to keep them vertically centered in it, so they keep responding to the
  pointer; the rest of that strip is the app's content and takes clicks.
- `cx.window_controls()` is the lights' frame from the window's top-left
  corner (`x`, `y`, `width`, `height`, `right()`, `bottom()`, and
  `insets(width)`: the room to keep at each end of a title bar), `None` in full
  screen or with the standard chrome. `cx.is_fullscreen()` says whether the
  window fills the screen. Both re-render the app when they change (on the
  way into full screen, before the animation ends).
- `.window_drag()` (on any element, usually the title bar's container) makes
  a press move the window (`performWindowDragWithEvent:`) and a double click
  do what System Settings › Desktop & Dock › « Double-click a window's title
  bar » says: zoom (the default, and « Fill », which has no public API),
  minimize, or nothing. Buttons, `on_click` views and draggable views inside
  keep their own clicks; text and plain containers inside move the window.
  On a clickable element itself the click wins: put it on the container.
- Containers outside a `.window_drag()` region don't move the window, even in
  the title bar strip. A tab bar or a navigation header along the window's
  top edge is the exception, as Safari's and Finder's tab bars are: its
  empty part moves the window and takes the double click, while its tabs,
  Back and bar buttons keep their clicks. The same bars do so on Windows.
  `PRISM_CLICK=dblclick:#window@x,y` (dev builds) double-clicks at that
  point of the window, and the log gives the window's frame before and
  after, zoomed or minimized.
- Such a bar keeps room for the traffic lights over it, as Windows' bars do
  for the caption buttons: Back, the bar buttons and the tabs start after
  them, at whichever end they are (the left, unless the system runs right to
  left), and the title stays centered in the window. A right-to-left app
  keeps its Back at the right and its trailing buttons clear of the lights.
  A bar lower down (a header under a tab bar) and every bar in full screen,
  where the lights hide, keep no room. Tabs too wide for what is left
  shrink, their titles truncated.
- `PRISM_SNAPSHOT` (dev builds) draws the traffic lights over the content
  with a custom chrome, so a snapshot shows what they cover.

Windows has it too, with its caption buttons at the top right
([Windows › Custom window chrome](windows.md#custom-window-chrome)). Other
platforms accept both and ignore them: the web (the browser owns the
window), iOS, iPadOS, visionOS, Android, and Linux, where GTK keeps its own
header bar for now. `window_controls()` is `None` and `is_fullscreen()` is
`false` there.
