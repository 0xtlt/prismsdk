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

An overlay can be created already open, in the same update as its parent: it is shown as soon as it is placed.

### Inline popovers: suggestion menus under a field

A slash-command, mention or autocomplete menu shows under the field the user is typing in, and the field must keep the keys. Two options make a popover fit for it:

```rust
div()
    .child(text_input().value(text).on_input(…).on_key(…))   // Escape, arrows: the field's keys
    .children(text.starts_with('/').then(|| {
        popover(menu(items))
            .open(true)
            .light_dismiss(false)   // a click outside (in the field) does not close it
            .take_focus(false)      // the field keeps the keyboard focus
    }))
```

- `.light_dismiss(false)`: only the app closes it (`open(false)`, or by no longer rendering it), or Escape while the focus is inside it (then `on_dismiss` runs). `dismissible(false)` also refuses Escape.
- `.take_focus(false)`: showing it moves no focus, and clicking or tapping its items runs their handlers without taking the focus from the field. Keys go to the field: handle Escape, the arrows and Enter there (`on_key`). It implies `light_dismiss(false)` (a click in the field would otherwise close it). Fields inside such a popover cannot be typed in.

| | `light_dismiss(false)` | `take_focus(false)` |
|---|---|---|
| Web | `popover="manual"` | no focus move; presses inside keep the field focused |
| macOS | `NSPopover`, application-defined behavior | a borderless, non-activating child panel (it never becomes key) |
| iOS, iPadOS, visionOS | taps around it reach the window (passthrough views) | the field stays first responder, the keyboard stays up (if UIKit moves the first responder while presenting, the field takes it back) |
| Linux | `GtkPopover` without autohide | a `GtkPopover` without autohide that cannot take the focus |
| Android | `PopupWindow` ignoring outside touches | a non-focusable `PopupWindow` above the keyboard |

Sheets and dialogs ignore both options. On macOS the panel follows the anchor at each layout, not while a scroll view scrolls under it.

### Placing a popover: side and alignment

A popover opens below its anchor, its leading edge lined up with the anchor's. `.side(…)` and `.align(…)` place it elsewhere:

```rust
// A submenu: beside the item that opens it, tops lined up.
div()
    .child(button("Turn into ›").on_click(…))
    .child(popover(block_kinds).open(self.turn_into).side(PopoverSide::Trailing))

// A panel opened from a button at the end of a toolbar: lined up with the
// button's trailing edge, it grows towards the middle of the window.
div()
    .child(button("More").on_click(…))
    .child(popover(settings).open(self.more).align(PopoverAlign::End))
```

- `PopoverSide`: `Bottom` (the default), `Top`, `Trailing` (after the anchor: its right in left-to-right layouts, its left in right-to-left ones) or `Leading`. Without room on that side of the window, the popover opens on the opposite one: a submenu near the window's right edge opens to the left of its item.
- `PopoverAlign`: `Start` (the default: leading edges below or above the anchor, top edges beside it), `Center` or `End` (trailing edges, or bottom edges beside). The popover is then moved, if needed, to stay inside the window.

A popover can open from an item of another open popover (a submenu of a menu): each is placed against its own anchor and both stay open. Both options combine with `light_dismiss(false)` and `take_focus(false)`.

| | Side | Alignment |
|---|---|---|
| Web | placed by the client, flipped without room | exact |
| macOS | `NSPopover` preferred edge (AppKit flips it); a `take_focus(false)` panel is placed by Prism, flipped without room | panel: exact; `NSPopover`: its arrow points at the part of the anchor it lines up with, exact when the popover is no longer than the anchor along that edge, centered on the anchor otherwise |
| iPadOS, visionOS | `permittedArrowDirections`: the side's, the opposite one without room | like the `NSPopover` |
| Linux | `GtkPopover::set_position` (GTK flips it) | `set_pointing_to`, like the `NSPopover` |
| Android | the `PopupWindow` placed by Prism, flipped without room | exact |

Phones that show popovers as sheets, tvOS and watchOS ignore both options, as do sheets and dialogs.

Toasts (and any `cx.add_layer` entity) stay above overlays on the web, macOS (a transparent child window), iOS, iPadOS, tvOS and visionOS (a pass-through window). On Linux and Android, sheets and dialogs are separate windows, and on watchOS sheets cover the app: layers stay under them there. On the web, layers are in the server-rendered first paint too (see [Web](../platforms/web.md#layers-in-the-first-paint)).

`hit_test(false)` and `inert(true)` (see [Pointer, keyboard, focus](input.md)) let a screen draw its own scrim or decoration and keep the content under it out of reach.
