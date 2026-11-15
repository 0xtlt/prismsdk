# Dialogs, menus, tray

```rust
cx.show_alert(Alert::new("Delete “Milk”?").message("This cannot be undone.")
    .destructive("Delete", on_delete).cancel("Cancel"));          // UIAlertController, NSAlert, <dialog>
cx.present("/todos/new");                                          // sheet: page sheet, NSWindow sheet, <dialog>
div().context_menu(Menu::new().item(MenuItem::new("Open").icon("arrow.up.forward.square")))
cx.set_tray(Tray::new("focus").title("25:00").menu(menu));         // macOS menu bar (NSStatusItem)
cx.set_app_menu(vec![Menu::titled("File").item(MenuItem::new("New").shortcut("cmd+n"))]);
```

Context menus open with a long press (iOS), right click or ⌃-click (macOS), right click / Shift+F10 (web), and from VoiceOver.

## Sheets, popovers and dialogs without a route

A screen can show its own content over itself, without a route (`cx.present` needs one):

```rust
sheet(gallery).open(self.picking).half()                  // a sheet, starting at half height
    .on_dismiss(cx.listener(|this, _, _| this.picking = false))
div().child(button("Filters").on_click(…))
    .child(popover(filters).open(self.filters)         // anchored to its parent: the wrapper
        .on_dismiss(cx.listener(|this, _, _| this.filters = false)))
dialog(confirm).open(self.confirming).dismissible(false)  // Escape / tap outside do nothing
```

The overlay takes no room where it is placed; its content is only built while it is open, laid out on its own and presented by the platform above the whole window (bars, tabs and the host page's frame included). Modal ones keep the keyboard focus inside and make the rest unreachable. Escape, Back, a tap outside or a swipe down close it and call `on_dismiss`: set your state to closed there (an overlay the app keeps open is shown again).

| | Sheet | Popover | Dialog |
|---|---|---|---|
| Web | modal `<dialog>` (bottom sheet on phones), blurred backdrop | popover API, placed under its anchor | modal `<dialog>` |
| macOS | `NSWindow` sheet | `NSPopover` | `NSWindow` sheet |
| iOS, iPadOS, visionOS | `UISheetPresentationController` (medium / large detents) | `UIPopoverPresentationController` (a sheet on iPhone) | centered card over a dimmed window |
| tvOS | centered card over a blurred screen (Menu closes it) | same | same |
| Linux | modal window (up to 560 pt wide; no bottom sheet without libadwaita) | `GtkPopover` | modal window |
| Android | dialog pinned to the bottom, sliding up | `PopupWindow` | centered `Dialog` |
| watchOS | `.sheet` | `.sheet` | `.sheet` |

Toasts (and any `cx.add_layer` entity) stay above overlays on the web, macOS (a transparent child window), iOS, iPadOS, tvOS and visionOS (a pass-through window). On Linux and Android, sheets and dialogs are separate windows, and on watchOS sheets cover the app: layers stay under them there.

`hit_test(false)` and `inert(true)` (see [Pointer, keyboard, focus](input.md)) let a screen draw its own scrim or decoration and keep the content under it out of reach.
