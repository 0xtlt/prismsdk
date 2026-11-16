# Pointer, keyboard, focus

Where the pointer is, drags, key presses, and moving the focus or the scroll position from code.

```rust
div()
    .on_click(cx.listener(|this, e: &ClickEvent, _| {
        if let Some(p) = e.position { this.tap_at(p.x, p.y) }   // None from the keyboard / VoiceOver
    }))
    .on_hover(cx.listener(|this, e: &HoverEvent, _| this.hot = e.hovered))
    .on_pointer_move(cx.listener(|this, e: &PointerEvent, _| this.readout = Some(e.position)))
    .on_drag(cx.listener(|this, e: &DragEvent, _| match e.phase {
        DragPhase::Start => this.from = this.level,
        DragPhase::Move | DragPhase::End => this.level = e.position().x / WIDTH,
        DragPhase::Cancel => this.level = this.from,
    }))

div().id("pad").on_key_down(cx.listener(|this, e: &KeyEvent, cx| {
    if e.is("ArrowUp") { this.y -= 1; cx.stop_propagation(); }
}))
div().on_global_key_down(cx.listener(|this, e: &KeyEvent, _| {
    if e.is("Escape") { this.menu_open = false }               // anywhere in the window
}))

text_input().id("email").autofocus()
cx.focus("email");                                              // after the next render
cx.dismiss_keyboard();                                          // the field lets go, the keyboard hides
cx.scroll_into_view("form");                                    // « jump to the form »
```

- **Coordinates** are points in the element, from its top-left corner (whatever the layout direction). A drag's `start` is where the pointer went down and `translation` how far it moved since, so moving the element while it is dragged (drag to reorder) does not disturb them; `position()` is their sum.
- **Click and drag** combine: a tap reports its position through `on_click`, a drag starts once the pointer moved past the platform's slop and then does not click. Together they make a custom slider or a « level » bar; add `on_key_down` (arrows) and an `accessibility_role(Role::Slider)` with a value so it works without a pointer.
- **Hover** needs a pointer: a mouse, a trackpad, an iPad pointer, Apple Vision Pro's gaze. Phones report `on_pointer_move` while a finger touches the element, and no hover. Don't hide anything essential behind hover.
- **Keys** use the web's `KeyboardEvent.key` names everywhere: characters (`"a"`, `"A"`, `" "`), `"Escape"`, `"Enter"`, `"Tab"`, `"Backspace"`, `"Delete"`, `"ArrowUp"`…, `"Home"`, `"End"`, `"PageUp"`, `"PageDown"`, `"F1"`… with `e.modifiers` (`shift`, `control`, `alt`, `meta`, `command()` for ⌘ on Apple platforms and Ctrl elsewhere). A view with `on_key_down` can take the focus (Tab, a click, `cx.focus`, the tvOS focus engine, a D-pad). The handlers of the focused element and its ancestors run first, then every `on_global_key_down`, newest first; `cx.stop_propagation()` stops there. While a text field has the focus, global handlers only get keys that do not type (Escape, arrows, Enter, Tab, function keys) or come with Control / Command.
- **TV remotes and D-pads**: the Siri Remote and Android TV / D-pad report their arrows and `"Enter"` (select / center), `"MediaPlayPause"`, and Menu / Back as `"Escape"` when the app listens for keys.
- **Ids**: `.id("name")` names an element for `cx.focus` and `cx.scroll_into_view` (the last element given a name wins). Unknown names are ignored. `.autofocus()` focuses an element when it appears (a dialog's first field).
- **Dismissing the keyboard**: `cx.dismiss_keyboard()` takes the focus away from whatever has it, after the next render, and the on-screen keyboard hides: after a submit, or before swapping the root (`cx.set_root`) away from a sign-in form. UIKit: the window ends editing; Android: the focused view clears its focus and the IME hides; macOS: the window's first responder lets go; GTK: the window's focus is unset; watchOS: SwiftUI's focus state is cleared; web: `document.activeElement.blur()`. On iOS the room made for the keyboard also goes when the root is swapped while it is up.
- **Hit testing**: `.hit_test(false)` lets clicks, taps and hovers through the element and everything in it (CSS `pointer-events: none`: a label drawn over a bar, a decorative scrim). `.inert(true)` also takes the subtree out of the keyboard focus and the accessibility tree while keeping it visible.

| | Click position | Hover / move | Drag | Keys | Focus, scroll into view |
|---|---|---|---|---|---|
| Web | pointer events | `pointerenter` / `pointermove` (one per frame) | pointer capture (one move per frame) | `keydown` | `focus()`, `scrollIntoView` |
| macOS | mouse up | `NSTrackingArea` | mouse dragged | `keyDown`, a local event monitor | first responder, `scrollRectToVisible` |
| iOS, iPadOS, visionOS | tap location | `UIHoverGestureRecognizer`; touches | `UIPanGestureRecognizer` | `pressesBegan` (hardware keyboards) | first responder, `scrollRectToVisible` |
| tvOS | — | — | — | Siri Remote presses | focus engine, `scrollRectToVisible` |
| Linux | `GestureClick` | `EventControllerMotion` | `GestureDrag` | `EventControllerKey` | `grab_focus`, scrolled window adjustment |
| Android | touch up | hover events; touches | touch listener with touch slop | key events, D-pad | `requestFocus`, `requestRectangleOnScreen` |
| watchOS | `SpatialTapGesture` | — (moves while touching) | `DragGesture` | — | `@FocusState`, `ScrollViewReader` |

tvOS has no pointer over views, the watch neither hover nor keys: those handlers simply never run there. On tvOS, a focused view with `on_key_down` keeps left / right for the app while up / down still move the focus.

Web events travel over the app's WebSocket: moves and drags are throttled to one per animation frame, so a fast pointer does not flood the connection; clicks, drag starts and ends are never dropped.

## Click keys and count, key releases

```rust
row.on_click(cx.listener(|this, e: &ClickEvent, cx| {
    if e.is_double() { this.rename(); }                      // e.count: 2, 3…
    else if e.modifiers.shift { this.extend_selection(); }   // ⇧-click
    else if e.modifiers.command() { this.open_in_new_tab(); } // ⌘-click (Ctrl elsewhere)
    else { this.select(); }
}))
canvas
    .on_global_key_down(cx.listener(|this, e: &KeyEvent, _| if e.key == " " { this.panning = true }))
    .on_global_key_up(cx.listener(|this, e: &KeyEvent, _| if e.key == " " { this.panning = false }))
```

`ClickEvent::modifiers` are the keys held and `ClickEvent::count` the
click's number in a quick series, as the system counts it with the user's
double-click speed (#159): a double click calls the handler twice, with
`count` 1 then 2. Keyboard activations have `count` 1 and no keys.
`on_key_up` and `on_global_key_up` hear key releases with the same rules as
`on_key_down` and `on_global_key_down`; a release comes only for a press
that was reported, and keys still held when the window loses the focus are
released then (no key-up arrives otherwise), so a « hold Space to pan »
never sticks.

| | Click keys and count | Key releases |
|---|---|---|
| Web | `MouseEvent` keys and `detail` | `keyup`; `blur` releases held keys |
| macOS | `modifierFlags`, `clickCount` on Prism views | `keyUp:` and the window's key monitor; resigning key releases held keys |

Other backends report count 1 and no keys, and no releases, for now.

## Dragging with other buttons

```rust
// Design tools: the middle button (wheel pressed) pans, the left one selects.
canvas
    .drag_buttons(&[PointerButton::Middle])
    .on_drag(cx.listener(|this, e: &DragEvent, cx| { /* e.button, e.modifiers */ }))
```

`on_drag` starts with the primary button; `.drag_buttons(&[…])` chooses
which of `Primary`, `Middle` and `Secondary` start it instead (#136), and
`DragEvent::button` says which one drags, `DragEvent::modifiers` the keys
held when it started (Shift to constrain, Option to duplicate). Over such an
element the platform's own use of the button is left out: the web's
middle-click autoscroll, paste and « open in a new tab », and the context
menu after a secondary-button drag (a press that does not move still opens
it). Fingers and pens are the primary button.

| | Middle | Secondary |
|---|---|---|
| Web | `pointerdown` with `button === 1` (`mousedown` and `auxclick` prevented) | `button === 2`, no `contextmenu` after a drag |
| macOS | `otherMouseDown` / `Dragged` / `Up` (`buttonNumber == 2`) | `rightMouseDown` / `Dragged` / `Up`; the menu opens on a press that did not move |

Other backends drag with the primary button only for now.

## Wheel and pinch

```rust
// An infinite canvas: two-finger scroll or the wheel pans, a pinch zooms.
div()
    .on_wheel(cx.listener(|this, e: &WheelEvent, cx| {
        if e.modifiers.command() {
            this.zoom_around(e.position, (-e.delta.y / 200.).exp());   // ⌘ / Ctrl + wheel
        } else {
            this.pan -= e.delta;                                       // points, like the web's deltas
        }
        cx.notify();
    }))
    .on_magnify(cx.listener(|this, e: &MagnifyEvent, cx| {
        this.zoom_around(e.position, e.scale);                         // relative to the last event
        cx.notify();
    }))
```

`on_wheel` and `on_magnify` work on any element, not only `scroll()` (#133). The element takes the events: nothing behind it scrolls and the page does not zoom. `delta` is in points, positive `y` scrolling toward the end (after the user's « natural scrolling »), like the web's `deltaY`; a coarse wheel counts 16 points per line and has `precise: false`. `phase` follows a trackpad gesture (`Start`, `Move`, `End`, then `Momentum` while it coasts, and `End` again when it stops); a mouse wheel and the web report `None`. A pinch's `scale` is the change since the previous event (multiply the zoom by it).

| | Wheel | Pinch |
|---|---|---|
| Web | `wheel` (non-passive, one report per animation frame, lines and pages in points) | Ctrl + `wheel` (how browsers report a trackpad pinch), Safari's `gesturechange`; with `on_wheel` alone, Ctrl + wheel is a wheel event with Ctrl held |
| macOS | `scrollWheel:` with `hasPreciseScrollingDeltas`, phases and momentum | `magnifyWithEvent:` |

Other backends do not report them yet: the handlers simply never run there.
