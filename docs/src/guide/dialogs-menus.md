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

## The whole menu bar

`cx.set_app_menu(menus)` adds the app's menus between Prism's own (an English
application menu, Edit with Undo and Redo, Go). `cx.set_menu_bar(menus)` makes
the app define the whole bar (#161): localized, with its own Undo and Redo,
and the system's actions where it wants them.

```rust
cx.set_menu_bar(vec![
    Menu::titled("App")                                    // the application menu: macOS titles it
        .item(MenuItem::system(SystemAction::About).title("À propos"))
        .item(MenuItem::new("Réglages…").shortcut("cmd+,").on_select(settings))
        .separator()
        .item(MenuItem::system(SystemAction::Hide).title("Masquer"))
        .item(MenuItem::system(SystemAction::HideOthers).title("Masquer les autres"))
        .item(MenuItem::system(SystemAction::Quit).title("Quitter")),
    Menu::titled("Édition")
        .item(MenuItem::new("Annuler").shortcut("cmd+z").on_select(undo))         // the app's history
        .item(MenuItem::new("Rétablir").shortcut("cmd+shift+z").on_select(redo))
        .separator()
        .item(MenuItem::system(SystemAction::Cut).title("Couper"))
        .item(MenuItem::system(SystemAction::Copy).title("Copier"))
        .item(MenuItem::system(SystemAction::Paste).title("Coller"))
        .item(MenuItem::system(SystemAction::SelectAll).title("Tout sélectionner")),
    Menu::titled("Fenêtre")
        .item(MenuItem::system(SystemAction::Minimize).title("Réduire"))
        .item(MenuItem::system(SystemAction::Zoom)),
]);
```

- `MenuItem::system(SystemAction::…)` is an action the platform performs, sent like the system's own items: Cut, Copy, Paste, Select All and Undo / Redo of a text field go to the focused view (and are enabled when it can), Minimize, Zoom, full screen and Close to the window, Hide, Hide Others, Show All, About and Services to the app; Quit asks first with unsaved changes. Each has the usual shortcut unless given one, and an English title unless given `.title(…)`. A menu with Minimize is the Window menu (macOS lists the windows there).
- An app item with a shortcut takes it from everything, focused text fields included: an `Annuler` item on ⌘Z runs the app's undo, not the field's. Use `MenuItem::system(SystemAction::Undo)` instead to keep the fields' own.
- Web: no menu bar, but the shortcuts of the app's menus (`set_app_menu` or `set_menu_bar`) are the app's: the page takes them before the browser (⌘Z in a field, ⌘S, ⌘K; `cmd` is ⌘ on Apple devices and Ctrl elsewhere) and runs the item. Linux: `set_menu_bar` is the GTK menu bar, without the system items (GTK fields have their own). Phones, tablets and watches show no menu bar.

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

`e.reason` says how (#156): `DismissReason::Escape`, `Outside` (a click or tap on the backdrop or around a popover), `Swipe` (a sheet swiped down), `Back` (the system's back, a sheet's Close button) or `Unknown`. A color picker cancels on Escape and keeps the color on a click outside:

```rust
dialog(picker).open(self.picking).on_dismiss(cx.listener(|this, e: &BackEvent, _| {
    if e.reason == DismissReason::Escape { this.color = this.color_before; }
    this.picking = false;
}))
```

| | Escape | Outside | Swipe | Back |
|---|---|---|---|---|
| Web | `cancel`; a popover closed right after Escape | the backdrop, a popover's light dismiss | – | – |
| macOS | Escape in a sheet or popover | a click outside a popover | – | – |
| iOS, iPadOS, visionOS | a hardware keyboard's Escape (tvOS: Menu) | the backdrop, around an iPad popover | a sheet | a sheet's Close button |
| Android | a hardware keyboard's Escape | a touch outside | – | the system's back |
| Linux | Escape | a click outside a popover | – | the window's close button |

| | Sheet | Popover | Dialog |
|---|---|---|---|
| Web | modal `<dialog>` (bottom sheet on phones), blurred backdrop | popover API, placed under its anchor | modal `<dialog>`, blurred backdrop |
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

Phones that show popovers as sheets, tvOS and watchOS ignore both options, as do sheets. On a dialog, `.side(…)` places it beside its anchor (below).

### Beside the anchor, without a dim

An editor opens small choice panels (a list of options, a colour picker) beside the column that asked for them, over the edge of the page being edited, without dimming it: the user keeps seeing what they change. A dialog with a side does that and stays a modal dialog: the focus stays inside, the rest is out of reach, and Escape or a click outside closes it.

```rust
div()
    .child(row("Colour", value).on_click(cx.listener(|this, _, _| this.picking = true)))
    .child(dialog(color_picker)
        .open(self.picking)
        .side(PopoverSide::Trailing)      // after the column (left of it in right-to-left languages)
        .backdrop(Backdrop::Clear)        // the page stays in sight
        .size(320., 0.)                   // 320 points wide, as high as its content
        .on_dismiss(cx.listener(|this, _, _| this.picking = false)))
```

- `.side(PopoverSide::…)` on a dialog: the side of its anchor (the element that contains the overlay) it opens on, lined up as `.align(…)` says, like a popover. It goes on the opposite side when its own has no room; a dialog with room on neither side (a phone) is centered as usual. A dialog without `.side(…)` is centered.
- `.backdrop(Backdrop::Clear | Dim | Blur)`: what a sheet or dialog shows over the page. A clear backdrop still takes the click outside that closes the overlay. Popovers have none.
- `.size(width, height)`: the content area in points, instead of the platform's default width (`0`: keep it) and the content's height (`0`: fit it), kept inside the window. The content's own `.w()` / `.h()` still apply inside it.

| | Placed dialog | Backdrop | Size |
|---|---|---|---|
| Web | the modal `<dialog>`, positioned beside the anchor | `::backdrop`: none, a dim, or a dim and a blur (the default) | the box's size; a sheet on a phone stays full width |
| macOS | an `NSPopover` on that edge | none (sheets are windows) | the popover's or window's content size |
| iOS, iPadOS, visionOS | the card beside the anchor | the card's: clear, a dim (the default) or a blur; a clear sheet leaves the window undimmed | popovers and dialogs |
| tvOS | centered cards (focus moves by direction) | the card's: a blur (the default), a dim or clear | popovers and dialogs |
| Linux | a `GtkPopover` at that position | none (sheets and dialogs are windows) | popovers and dialogs |
| Android | the dialog's window beside the anchor | the dialog's dim, off, or with a blur (Android 12+) | popovers and dialogs |
| watchOS | `.sheet` | – | – |

### A dialog without Prism's box

A picker the app styles itself (its own background, corners, border and shadow) would otherwise sit inside Prism's dialog box. `.bare()` takes that box away: no background, border, corners, shadow or entry animation, so the content is the whole panel. The dialog stays modal: the focus stays inside, Escape and a click outside close it, with their `DismissReason`.

```rust
dialog(div().bg(rgb(0x1c1c1e)).rounded(12.).shadow_lg().child(swatches))
    .open(self.picking)
    .side(PopoverSide::Trailing)
    .backdrop(Backdrop::Clear)
    .bare()
    .on_dismiss(cx.listener(|this, _, _| this.picking = false))
```

| | Bare dialog |
|---|---|
| Web | the `<dialog>` without Prism's background, border, radius, shadow or animation |
| macOS | a borderless, transparent panel that takes the keys, instead of a sheet or an `NSPopover` |
| iOS, iPadOS, visionOS, tvOS | a clear card without corners or edge, shown without a cross-fade |
| Linux | a `GtkPopover` without arrow or background, or an undecorated window |
| Android | a transparent dialog window, without animation |

Sheets and popovers keep their platform's box; `.bare()` does nothing on them.

Toasts (and any `cx.add_layer` entity) stay above overlays on the web, macOS (a transparent child window), iOS, iPadOS, tvOS and visionOS (a pass-through window). On Linux and Android, sheets and dialogs are separate windows, and on watchOS sheets cover the app: layers stay under them there. On the web, layers are in the server-rendered first paint too (see [Web](../platforms/web.md#layers-in-the-first-paint)).

`hit_test(false)` and `inert(true)` (see [Pointer, keyboard, focus](input.md)) let a screen draw its own scrim or decoration and keep the content under it out of reach.
