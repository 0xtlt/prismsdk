# Dialogs, menus, tray

```rust
cx.show_alert(Alert::new("Delete “Milk”?").message("This cannot be undone.")
    .destructive("Delete", on_delete).cancel("Cancel"));          // UIAlertController, NSAlert, <dialog>
cx.present("/todos/new");                                          // sheet: page sheet, NSWindow sheet, <dialog>
div().context_menu(Menu::new().item(MenuItem::new("Open").icon("arrow.up.forward.square")))
cx.set_tray(Tray::new("focus").title("25:00").menu(menu));         // macOS menu bar (NSStatusItem)
cx.set_app_menu(vec![Menu::titled("File").item(MenuItem::new("New").shortcut("cmd+n"))]);
```

Context menus open with a long press (iOS, Android), right click or ⌃-click (macOS), right click or a long press (Linux), right click, Shift+F10 or a long press (Windows), right click / Shift+F10 (web), and from VoiceOver.

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
- Web: no menu bar, but the shortcuts of the app's menus (`set_app_menu` or `set_menu_bar`) are the app's: the page takes them before the browser (⌘Z in a field, ⌘S, ⌘K; `cmd` is ⌘ on Apple devices and Ctrl elsewhere) and runs the item. Linux: `set_menu_bar` is the GTK menu bar, without the system items (GTK fields have their own). Windows: a WinUI `MenuBar` under the title bar; the system items Windows has no equivalent for (Hide, Show All, About, Services, full screen) are left out. Phones, tablets and watches show no menu bar.

## Shortcuts a menu only shows

`.shortcut(keys)` both shows a key in the menu and binds it: an app item takes
it from everything. A shortcut the focused view handles itself (⌘B in a text
field, ⌘D on a canvas, ⌘= for a zoom the view does) is documented with
`.shortcut_hint(keys)` instead (#247): the menu lists it, the key keeps going
to the focused view, and choosing the item runs its handler as usual.

```rust
Menu::titled("Format")
    .item(MenuItem::new("Bold").shortcut_hint("cmd+b").on_select(bold))   // the field's ⌘B
    .item(MenuItem::new("Find").shortcut("cmd+f").on_select(find))        // the app's ⌘F
```

| | Shortcut hint |
|---|---|
| macOS | the item's key equivalent only while its menu is open (an `NSMenuDelegate`), so a closed menu never takes the key |
| Linux | the item's `accel` attribute: shown by the `GtkPopoverMenu`, no accelerator |
| Windows | the item's `KeyboardAcceleratorTextOverride`: shown, no `KeyboardAccelerator` |
| Web | left out of the shortcuts the page takes; the page's menus write it (⌘B on Apple devices, Ctrl+B elsewhere, `aria-keyshortcuts`) |
| iOS, iPadOS, Android, watchOS | menus show no shortcuts there: nothing to show |

## Pop-up menus: from a button, at the pointer

The platform's own menu, opened by the app rather than by a right-click (#247):

```rust
button("+").on_click_menu(Menu::new()                     // below the button when pressed
    .item(MenuItem::new("Note").on_select(add_note))
    .item(MenuItem::new("Folder").on_select(add_folder)))
div().id("canvas").on_click(cx.listener(|this, e: &ClickEvent, cx| {
    cx.show_menu_on("canvas", this.menu(), e.position)   // at the pointer, in the canvas
}))
cx.show_menu(menu, Point::new(120., 80.))                // in the window's content
```

- `div().on_click_menu(menu)` (and `button(…).on_click_menu(menu)`) opens the menu below the element when it is clicked, tapped or activated from the keyboard, in place of `on_click`.
- `cx.show_menu_on(id, menu, at)` opens it over the element with that `.id(…)`: at `at` in the element's coordinates (a `ClickEvent::position`), or below it with `None`. `cx.show_menu(menu, at)` opens it at a point of the window's content. They open after the next render; showing another pop-up menu replaces this one's items.
- Items run their handlers like a context menu's.

| | Pop-up menu |
|---|---|
| macOS | `NSMenu.popUp(positioning:at:in:)` |
| iOS, iPadOS, visionOS | a `UIMenu` from a transparent one-point `UIButton` at that place (`showsMenuAsPrimaryAction`, `performPrimaryAction`): iOS 17.4 and later, nothing before |
| Android | `PopupMenu` anchored to a transparent view laid over that place |
| Linux | `GtkPopoverMenu` pointing at the place (or the element) |
| Windows | a `MenuFlyout` at the place (or below the element) |
| Web | the page's menu (the one context menus use), placed at the point or below the element |
| tvOS, watchOS | nothing |

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
| Windows | Escape | a click on the smoke, a popover's light dismiss | – | – |

| | Sheet | Popover | Dialog |
|---|---|---|---|
| Web | modal `<dialog>` (bottom sheet on phones), blurred backdrop | popover API, placed under its anchor | modal `<dialog>`, blurred backdrop |
| macOS | `NSWindow` sheet | `NSPopover` | `NSWindow` sheet |
| iOS, iPadOS, visionOS | `UISheetPresentationController` (medium / large detents) | `UIPopoverPresentationController` (a sheet on iPhone) | centered card over a dimmed window |
| tvOS | centered card over a blurred screen (Menu closes it) | same | same |
| Linux | modal window (up to 560 pt wide; no bottom sheet without libadwaita) | `GtkPopover` | modal window |
| Android | dialog pinned to the bottom, sliding up | `PopupWindow` | centered `Dialog` |
| Windows | a larger dialog (Windows has no sheet) | WinUI `Flyout` | `ContentDialog`'s look: a centered panel over the smoke |
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
            .take_focus(false)      // the field keeps the keyboard focus
            .on_dismiss(…)          // a press outside it, in the field too, closes it
    }))
```

- `.light_dismiss(false)`: only the app closes it (`open(false)`, or by no longer rendering it), or Escape while the focus is inside it (then `on_dismiss` runs). `dismissible(false)` also refuses Escape.
- `.take_focus(false)`: showing it moves no focus, and clicking or tapping its items runs their handlers without taking the focus from the field. Keys go to the field: handle Escape, the arrows and Enter there (`on_key_down`, with `consume_keys` so the field does not also act on them, #181). A press outside it, the field included, closes it (`on_dismiss`, reason `Outside`) and still reaches what it landed on: the caret moves, a drag starts (#182). Typing does not close it. Add `.light_dismiss(false)` to keep it open until the app closes it. Fields inside such a popover cannot be typed in.

| | `light_dismiss(false)` | `take_focus(false)` |
|---|---|---|
| Web | `popover="manual"` | no focus move; presses inside keep the field focused; a capture-phase `pointerdown` around it closes it without holding the press |
| macOS | `NSPopover`, application-defined behavior | a borderless, non-activating child panel (it never becomes key); a local mouse-down monitor on the window closes it and returns the event |
| iOS, iPadOS, visionOS | taps around it reach the window (passthrough views) | the field stays first responder, the keyboard stays up (if UIKit moves the first responder while presenting, the field takes it back); a passive touch watcher on the window closes it once a touch outside ends |
| Linux | `GtkPopover` without autohide | a `GtkPopover` without autohide that cannot take the focus; an unclaimed capture-phase click gesture on the window closes it |
| Android | `PopupWindow` ignoring outside touches | a non-focusable `PopupWindow` above the keyboard; outside touches close it and reach the window below |
| Windows | a `Flyout` letting presses through to the page (`OverlayInputPassThroughElement`) | a transient `Flyout` that takes no focus; presses outside reach the page |

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
| Windows | the `Flyout`'s placement (WinUI flips it) | the placement's edge alignment (`BottomEdgeAlignedLeft`…) |

Phones that show popovers as sheets, tvOS and watchOS ignore both options, as do sheets. On a dialog, `.side(…)` places it beside its anchor (below).

### At the caret: completion lists

An autocomplete list (template fields after `{{`, mentions after `@`) opens where the user types, not under the whole field. `.anchor_caret()` (or `.anchor(OverlayAnchor::Caret)`) anchors a popover, or a dialog, at the caret of the text field inside the overlay's parent: `text_input`, `text_area`, `rich_input` or `rich_text_input`.

```rust
div()
    .flex_row()
    // The field keeps the keys: the list's arrows, Enter and Escape are the app's.
    .consume_keys(if open { &["ArrowDown", "ArrowUp", "Enter", "Tab", "Escape"][..] } else { &[] })
    .on_key_down(cx.listener(|this, e: &KeyEvent, _| this.completion_key(e)))
    .child(text_input().value(text).selection(sel).on_input(…).on_select(…).flex_1())
    .children(open.then(|| {
        popover(completions)
            .open(true)
            .anchor_caret()         // below the caret, its leading edge at the caret
            .take_focus(false)      // the field keeps the focus while the list is open
            .bare()
            .size(200., 0.)
            .on_dismiss(…)
    }))
```

- The field is the one with the keyboard focus inside the parent, else the first one there. A field without the focus has no caret: the overlay goes against the whole field. Without a field, against the parent, as usual.
- The usual placement applies against the caret: `.side(…)` (below by default, flipped above without room), `.align(…)`, kept inside the window.
- The backend measures the caret when it lays the overlay out and places it again whenever the caret moves: typing, the arrows, a click, a selection, the field or the page scrolling, the field moving. The app is not asked, so the list moves in the same frame as the caret.
- With `.take_focus(false)`, typing keeps going to the field and does not close the list; handle the arrows, Enter and Escape on the field (`consume_keys`, `on_key_down`).

| | Caret | Follows it |
|---|---|---|
| Web | a range in a rich input; a hidden copy of an `<input>` or `<textarea>` up to the caret | on input, selection changes, scrolling, resizing and each update |
| macOS | `firstRectForCharacterRange` from the text view or the field editor | the `NSPopover`'s positioning rect or the panel, on text and selection changes, scrolling and layout |
| iOS, iPadOS, visionOS | `caretRectForPosition` | the popover's `sourceRect` or the dialog's card, on editing, selection changes, scrolling and layout |
| Linux | `gtk::Text::compute_cursor_extents`, or the text view's iter location | the `GtkPopover`'s pointing rectangle, each frame while shown |
| Android | the `EditText`'s layout at the cursor | the `PopupWindow` or the dialog's window, before each frame while shown |
| Windows | `TextBox::GetRectFromCharacterIndex`, a `RichEditBox`'s selection rectangle | the `Flyout`'s position and exclusion rectangle, on text and selection changes and scrolling |

tvOS (centered cards), watchOS, sheets and centered dialogs ignore it. The counter example's Forms section has a template field: type `{{` to see the list.

### At a chip

`.anchor_chip(field, key)` anchors a popover, or a dialog, to the chip `key` of the `rich_input` with id `field` instead, and it follows the chip as the text moves: see [a menu under a clicked chip](forms.md#a-menu-under-a-clicked-chip).

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
| Windows | a `Flyout` on that side | the smoke: clear, a dim (the default) or an acrylic blur | popovers and dialogs |
| watchOS | `.sheet` | – | – |

### A dialog or popover without Prism's box

A picker or menu the app styles itself (its own background, corners, border and shadow) would otherwise sit inside Prism's dialog or popover box. `.bare()` takes that box away: no background, border, corners, arrow, shadow or entry animation, so the content is the whole panel. A dialog stays modal: the focus stays inside, Escape and a click outside close it, with their `DismissReason`. A popover keeps its place beside the anchor and closes like any popover.

```rust
dialog(div().bg(rgb(0x1c1c1e)).rounded(12.).shadow_lg().child(swatches))
    .open(self.picking)
    .side(PopoverSide::Trailing)
    .backdrop(Backdrop::Clear)
    .bare()
    .on_dismiss(cx.listener(|this, _, _| this.picking = false))
```

| | Bare dialog | Bare popover |
|---|---|---|
| Web | the `<dialog>` without Prism's background, border, radius, shadow or animation | the popover without them |
| macOS | a borderless, transparent panel that takes the keys, instead of a sheet or an `NSPopover` | the same panel beside the anchor (it takes the keys unless the popover leaves the focus to its anchor) |
| iOS, iPadOS, visionOS | a clear card without corners or edge, shown without a cross-fade | the popover without its background, arrow or shadow |
| tvOS | a clear card without corners or edge, shown without a cross-fade | the same |
| Linux | a `GtkPopover` without arrow or background, or an undecorated window | a `GtkPopover` without arrow or background |
| Android | a transparent dialog window, without animation | a transparent popup, without elevation or animation |
| Windows | the content without the panel, its edge, shadow or animation | a `Flyout` without its background, border, shadow or animation |

Sheets keep their platform's box; `.bare()` does nothing on them.

Toasts (and any `cx.add_layer` entity) stay above overlays on the web, macOS (a transparent child window), iOS, iPadOS, tvOS and visionOS (a pass-through window) and Windows (the layers' canvas above the panels). On Linux and Android, sheets and dialogs are separate windows, and on watchOS sheets cover the app: layers stay under them there. On the web, layers are in the server-rendered first paint too (see [Web](../platforms/web.md#layers-in-the-first-paint)).

`hit_test(false)` and `inert(true)` (see [Pointer, keyboard, focus](input.md)) let a screen draw its own scrim or decoration and keep the content under it out of reach.
