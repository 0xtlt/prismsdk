# macOS

`prism dev macos` / `prism run macos` (a plain binary, fastest to iterate),
`prism run macos --bundle` (a full `.app`), `prism build macos` (a release
`.app` with widgets and icon).

Every node is a real AppKit view (`NSView`; labels are `NSTextField`s,
buttons `NSButton`s), laid out by prism-native's flexbox. Backgrounds,
borders, corners and pictures are layer properties ([Performance](../performance.md)).

- **Window:** resizing re-renders across [breakpoints](../guide/responsive.md);
  the window title follows the screen title.
- **Navigation:** a header bar with a Back button and Go ▸ Back ⌘[; tabs are a
  segmented control ([Navigation](../guide/navigation.md)).
- **Menus:** `cx.set_app_menu` (the menu bar, with shortcuts), context menus on
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
  focus ring ([Accessibility](../guide/accessibility.md)).
- **Background:** `NSBackgroundActivityScheduler` ([Background execution](../guide/background.md)).

The Swift bridge (`crates/prism-apple-build/apple/Bridge`) is compiled by
`prism-macos`'s build script, so Xcode is required. The deployment target is
macOS 13 (`MACOSX_DEPLOYMENT_TARGET`, set by `prism`; change it with
`[macos] deployment_target` in `Prism.toml`, see
[iOS](ios.md#deployment-target)); the WidgetKit parts need macOS 14. The
bundled app embeds the widget extension and its App Group only when the app
registers a widget.

Development hooks: `PRISM_CLICK="+1|Dark mode" PRISM_SCROLL=600 PRISM_DELAY=2
PRISM_SNAPSHOT=out.png cargo run -p counter` ([Development](../development.md)).
