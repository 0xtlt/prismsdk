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
cx.scroll_by("list", 0., 12.);                                  // auto-scroll a step while dragging near the edge
cx.scroll_to("list", 0., 0., true);                             // back to the top, animated
```

- **Coordinates** are points in the element, from its top-left corner (whatever the layout direction). A drag's `start` is where the pointer went down and `translation` how far it moved since, so moving the element while it is dragged (drag to reorder) does not disturb them; `position()` is their sum.
- **Click and drag** combine: a tap reports its position through `on_click`, a drag starts once the pointer moved past the platform's slop and then does not click. Together they make a custom slider or a « level » bar; add `on_key_down` (arrows) and an `accessibility_role(Role::Slider)` with a value so it works without a pointer.
- **Hover** needs a pointer: a mouse, a trackpad, an iPad pointer, Apple Vision Pro's gaze. Phones report `on_pointer_move` while a finger touches the element, and no hover. Don't hide anything essential behind hover.
- **Keys** use the web's `KeyboardEvent.key` names everywhere: characters (`"a"`, `"A"`, `" "`), `"Escape"`, `"Enter"`, `"Tab"`, `"Backspace"`, `"Delete"`, `"ArrowUp"`…, `"Home"`, `"End"`, `"PageUp"`, `"PageDown"`, `"F1"`… with `e.modifiers` (`shift`, `control`, `alt`, `meta`, `command()` for ⌘ on Apple platforms and Ctrl elsewhere). A view with `on_key_down` can take the focus (Tab, a click, `cx.focus`, the tvOS focus engine, a D-pad). The handlers of the focused element and its ancestors run first, then every `on_global_key_down`, newest first; `cx.stop_propagation()` stops there. While a text field has the focus, global handlers only get keys that do not type (Escape, arrows, Enter, Tab, function keys) or come with Control / Command.
- **TV remotes and D-pads**: the Siri Remote and Android TV / D-pad report their arrows and `"Enter"` (select / center), `"MediaPlayPause"`, and Menu / Back as `"Escape"` when the app listens for keys.
- **Ids**: `.id("name")` names an element for `cx.focus` and `cx.scroll_into_view` (the last element given a name wins). Unknown names are ignored.
- **Scrolling from the app** (#193): `cx.scroll_by(id, dx, dy)` moves a `scroll()` with that `.id()` by points from where it is, at once (call it each frame or timer tick while a drag stays in an edge zone); `cx.scroll_to(id, x, y, animated)` puts its content's point (`x`, `y`) at its top-left corner (`f32::MAX`: the end). Both stay within the content and do nothing on other elements. `cx.scroll_offset(id)` is the last offset it reported, along its axis, once it has `on_scroll`. Web: `scrollBy` / `scrollTo`; macOS: the clip view's origin; iOS: `setContentOffset(_:animated:)`; Linux: the scrolled window's adjustments (not animated); Android: `scrollBy` / `smoothScrollTo`; Windows: `ScrollViewer.ChangeView`. `.autofocus()` focuses an element when it appears (a dialog's first field).
- **Dismissing the keyboard**: `cx.dismiss_keyboard()` takes the focus away from whatever has it, after the next render, and the on-screen keyboard hides: after a submit, or before swapping the root (`cx.set_root`) away from a sign-in form. UIKit: the window ends editing; Android: the focused view clears its focus and the IME hides; macOS: the window's first responder lets go; GTK: the window's focus is unset; Windows: the root view takes the focus; watchOS: SwiftUI's focus state is cleared; web: `document.activeElement.blur()`. On iOS the room made for the keyboard also goes when the root is swapped while it is up.
- **Overflowing children** (#213): a child drawn outside its parent (absolutely positioned with a negative offset, a handle on a grid's edge) takes clicks, hovers and drags everywhere it is drawn, unless the parent clips (`.clip()`), like on the web. macOS and iOS hit-test the overflow themselves (AppKit and UIKit only look inside a view's bounds); before macOS 14, hover tracking covers it with an explicit rect. Android only delivers touches inside a view's parent: keep touch targets inside it there.
- **Hit testing**: `.hit_test(false)` lets clicks, taps and hovers through the element and everything in it (CSS `pointer-events: none`: a label drawn over a bar, a decorative scrim). `.inert(true)` also takes the subtree out of the keyboard focus and the accessibility tree while keeping it visible.

| | Click position | Hover / move | Drag | Keys | Focus, scroll into view |
|---|---|---|---|---|---|
| Web | pointer events | `pointerenter` / `pointermove` (one per frame) | followed on the window from the press (a fast flick off a small handle still starts, #194), pointer capture once it is a drag (one move per frame) | `keydown` | `focus()`, `scrollIntoView` |
| macOS | mouse up | `NSTrackingArea` | mouse dragged | `keyDown`, a local event monitor | first responder, `scrollRectToVisible` |
| iOS, iPadOS, visionOS | tap location | `UIHoverGestureRecognizer`; touches | `UIPanGestureRecognizer` | `pressesBegan` (hardware keyboards) | first responder, `scrollRectToVisible` |
| tvOS | — | — | — | Siri Remote presses | focus engine, `scrollRectToVisible` |
| Linux | `GestureClick` | `EventControllerMotion` | `GestureDrag` | `EventControllerKey` | `grab_focus`, scrolled window adjustment |
| Android | touch up | hover events; touches | touch listener with touch slop | key events, D-pad | `requestFocus`, `requestRectangleOnScreen` |
| Windows | `Tapped` / `DoubleTapped`; a clickable view's button: its press and release (`handledEventsToo`) | `PointerEntered` / `PointerMoved` | pointer capture past the system's drag distance | the message loop's `WM_KEYDOWN`, before XAML | `Focus`, `StartBringIntoView` |
| watchOS | `SpatialTapGesture` | — (moves while touching) | `DragGesture` | — | `@FocusState`, `ScrollViewReader` |

tvOS has no pointer over views, the watch neither hover nor keys: those handlers simply never run there. On tvOS, a focused view with `on_key_down` keeps left / right for the app while up / down still move the focus.

Web events travel over the app's WebSocket: moves and drags are throttled to one per animation frame, so a fast pointer does not flood the connection; clicks, drag starts and ends are never dropped.

## Keys a field around you must not act on

A suggestion menu under a text field (type `@` or `/`, then ↑ ↓ to move the highlight and ↵ to pick) needs those keys for itself: the field must not also move its caret or submit. `consume_keys` declares them at render, on the element whose `on_key_down` handles them (#181):

```rust
div()
    .child(rich_input(self.runs.clone()))
    .child(suggestions(&self.menu))
    .on_key_down(cx.listener(|this, e: &KeyEvent, _| this.menu_key(e)))
    .consume_keys(if self.menu.is_open() { &["ArrowUp", "ArrowDown", "Enter", "Escape"][..] } else { &[] })
```

While the focus is in the element, those keys reach its `on_key_down` (and its ancestors', then the global ones, as usual) and nothing else: not the focused field, not the page, not the window. Other keys type as before. The declaration is read as the key is pressed, so a handler that closes the menu on ↵ still keeps that ↵ from the field. `cx.stop_propagation()` only stops other Prism handlers; it never reaches the platform, which is why the keys are declared rather than consumed from the handler: on the web the handler runs on the server, after the browser has acted.

| | How the field is kept out |
|---|---|
| Web | a capture-phase `keydown`: reported, then `preventDefault()` and stopped |
| macOS | the key monitor returns no event |
| iOS, iPadOS, visionOS | `UIKeyCommand`s with priority over the system's behaviour on the root controller (named keys: arrows, Enter, Tab, Escape, Space, Home, End, Page Up / Down) |
| Android | `dispatchKeyEvent` consumes the press and its release |
| Linux (GTK) | the window's capture-phase key controller stops it |
| Windows | the message loop drops the key message before XAML sees it |

### Leaving a field at its edges

A grid of cells or a document of blocks moves to the next field only when the caret is at the field's edge: ← at its start, → at its end, ↑ on its first visual line, ↓ on its last (wrapped lines count). Elsewhere the field moves its caret. `consume_key_at` declares a key with its edge, and the platform checks the caret when the key is pressed, before the field acts (#211):

```rust
div()
    .child(text_area().value(block.text.clone()).id(format!("block-{i}")))
    .consume_key_at("ArrowUp", CaretEdge::FirstLine)
    .consume_key_at("ArrowDown", CaretEdge::LastLine)
    .on_key_down(cx.listener(move |this, e: &KeyEvent, cx| {
        if e.is("ArrowUp") && e.at(CaretEdge::FirstLine) { this.focus_block(i - 1, cx) }
        if e.is("ArrowDown") && e.at(CaretEdge::LastLine) { this.focus_block(i + 1, cx) }
    }))
```

- `KeyEvent::edges` says where the focused field's caret was (`CaretEdges { start, end, first_line, last_line }`), and `e.at(edge)` reads it with the same rule as `consume_key_at`: the handler moves the focus exactly when the key was taken from the field. With text selected, the caret is at no edge (the key acts on the selection). With no field focused, `at` is `true` and the key is taken.
- `consume_key_at` adds to `consume_keys`: call it after.
- Edges `Start`, `End`, `FirstLine` and `LastLine`. A single-line field's caret is on its first and last line.

| | First and last visual lines |
|---|---|
| Web | `<textarea>`: a hidden copy laid out the same, with marks at the start, the caret and the end; rich inputs: the selection's line box against the content's first and last ones. Line breaks only when the page has no layout. |
| macOS | `firstRectForCharacterRange` of the caret against the start and end of the text |
| iOS, iPadOS, visionOS | `caretRect(for:)` of the caret against `beginningOfDocument` and `endOfDocument` |
| Android | `Layout.getLineForOffset` against `getLineCount` |
| Linux (GTK) | `GtkTextView` iterator locations of the caret, the start and the end |
| Windows | `TextBox::GetRectFromCharacterIndex` of the caret against the start and end of the text; rich inputs: RichEdit's `ITextRange::GetRect` |

## Middle, back and forward clicks

`on_aux_click` hears clicks with the middle button (pressing the wheel) and a mouse's back and forward buttons (#215): close a tab with the middle button, open a row in a new tab.

```rust
tab.on_aux_click(cx.listener(move |this, e: &ClickEvent, cx| {
    if e.button == PointerButton::Middle { this.close_tab(i, cx) }
    if e.button == PointerButton::BACK { this.go_back(cx) }
}))
```

- `ClickEvent::button` says which: `Middle`, `PointerButton::BACK` (`Other(3)`) or `FORWARD` (`Other(4)`), numbered like the web on every platform. `on_click` stays the primary button's (its `button` is `Primary`), the context menu the secondary one's.
- A press that moves into a drag (`drag_buttons(&[Middle])`) is a drag, not a click.
- Web: `auxclick`; the middle button's autoscroll and the back and forward buttons' navigation are kept from starting over the element. macOS: `otherMouseDown` / `otherMouseUp` (button numbers 2, 3, 4). Linux: `GtkGestureClick` (buttons 2, 8, 9). Windows: `PointerReleased` (middle, `XButton1`, `XButton2`). Touch screens have no such buttons; iPadOS and Android mice are not reported yet.

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
| Linux (GTK) | `GestureClick`'s `n_press` and the event's state | `EventControllerKey` releases, matched to their press by hardware key; the window turning inactive releases held keys |
| iOS, iPadOS, visionOS | `UITouch.tapCount`, the tap's `modifierFlags` (a hardware keyboard, an iPad pointer) | `pressesEnded:` / `pressesCancelled:`; the app turning inactive releases held keys |
| Android | taps counted with the system's double-tap timeout and slop; the event's meta state | `KeyEvent.ACTION_UP`; the window losing the focus releases held keys |
| Windows | the keys held; presses counted with `GetDoubleClickTime` and the double-click rectangle (`SM_CXDOUBLECLK`) | `WM_KEYUP`, matched to its press by virtual key and scan code; the window turning inactive (`WM_ACTIVATE`) releases held keys |

tvOS counts presses of the remote's select button as single clicks; watchOS reports count 1 and no keys.

## Keys during a drag, and no click after it

A shape editor's ⇧ (keep proportions) and ⌥ (from the centre, duplicate) act live (#223): `DragEvent::modifiers` holds the keys held now, updated on every move, and pressing or releasing a modifier alone mid-drag sends a `Move` where the drag is, with the new keys (the web and macOS; Linux and Windows update them as the pointer moves). Once a drag has started, the release that ends it makes no click on the element or around it: on the web the click is swallowed before any handler or link until the next press; natively the drag takes the press.

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
| Linux (GTK) | `GestureDrag` on any button (`BUTTON_MIDDLE`), claimed past the drag threshold | `BUTTON_SECONDARY`; the menu opens on a release that did not move |
| iPadOS (pointer) | the pan's `buttonMask` (button 3) | `buttonMask` `.secondary` |
| Android (mouse) | `BUTTON_TERTIARY` | `BUTTON_SECONDARY`; a drag cancels the long press's menu |
| Windows | `MiddleButtonPressed`, the pointer captured past the drag distance | `RightButtonPressed` |

tvOS and watchOS have no pointer buttons. Fingers and pens drag with the primary button everywhere.

## Long press to drag on touch screens

```rust
// A kanban board: columns in a horizontal scroll, cards that move.
scroll_x().child(columns.iter().map(|c| {
    column(c).children(c.cards.iter().map(|card| {
        card_view(card)
            .on_drag(cx.listener(move |this, e: &DragEvent, cx| this.move_card(card.id, e, cx)))
            .drag_after_long_press(Duration::from_millis(400))
    }))
}))
```

On a touch screen, a draggable element inside a scroll view competes with
it for the finger: a quick swipe should scroll the board, not pick up the
card. `.drag_after_long_press(delay)` (#243) makes fingers and pens wait:
the drag starts only once the press is held still for `delay`, with the
platform's « lift » haptic, and from then on it wins over every scroll view
around the element (the finger moves the card, the board no longer
scrolls). A swipe before the delay scrolls as usual, and a tap still clicks.
With a mouse, a trackpad or an iPad's pointer the drag starts on movement,
as without it. `DragEvent::start` is where the finger went down;
`translation` counts from the lift.

| | Long press | Then |
|---|---|---|
| iOS, iPadOS, visionOS | a `UILongPressGestureRecognizer` for fingers and the Pencil (`minimumPressDuration`, 10 points of slop); the pan keeps the iPad pointer | a medium `UIImpactFeedbackGenerator` impact (not on visionOS); the scroll views around it stop and its taps are cancelled |
| Android | a timer from `ACTION_DOWN`, cancelled past the touch slop (`ViewConfiguration`); a mouse drags at once | `HapticFeedbackConstants.LONG_PRESS`; `requestDisallowInterceptTouchEvent` keeps the scroll view off |
| Web | touch and pen pointers: a timer from `pointerdown`, cancelled past 8 px; meanwhile `touch-action: pan-x pan-y` lets the browser scroll | pointer capture, `navigator.vibrate` where there is one, and non-passive `touchmove`s prevented so the page stops scrolling; no callout or long-touch menu |
| Linux (GTK) | touchscreen and pen devices: the `GestureDrag` waits (denied past the drag threshold) | the gesture is claimed: the `GtkScrolledWindow` lets go |
| Windows | touches: a timer from the press, cancelled past the slop | the pointer is captured, so the scroll view around stops panning; no haptic |

macOS has no touch screen (its drags start on movement); tvOS and watchOS
have no drags on views. A long touch can only be one thing: don't give the
same element a context menu (on the web the drag wins and the menu opens
with the mouse or the keyboard only).

## Drag and drop between elements

```rust
// A card carries its id; columns take cards.
card.draggable(DragPayload::new("card", task.id.to_string()))

scroll()                                       // a column, scrolling
    .children(cards)
    .drop_hover(|s| s.bg(theme.accent.with_alpha(0.12)))
    .on_drag_over("card", cx.listener(|this, e: &DragOverEvent, cx| {
        // The insertion line: `e.position` is in the column's content.
        this.slot = match e.phase {
            DragOverPhase::Leave => None,
            _ => Some(slot_at(e.position.y)),
        };
    }))
    .on_drop_payload("card", cx.listener(move |this, e: &DropEvent, cx| {
        this.move_card(&e.payload.data, column, slot_at(e.position.y));
    }))
```

`.draggable(payload)` lets an element be dragged onto drop targets inside
the app (#251): a card into another column, a library item onto a canvas, a
row into a tree. A `DragPayload` has a `kind` (which targets take it) and the
app's `data` (an id, JSON); nothing goes through the system's pasteboard.
While it is dragged:

- a see-through snapshot of the element follows the pointer, held where it
  was grabbed (`.preview(DragPreview::None)` when the app moves the element
  itself with `on_drag`, which still runs alongside);
- the innermost element under the pointer that takes the kind (with
  `.on_drop_payload(kind, …)` or `.on_drag_over(kind, …)`, the dragged
  element and what is inside it left out) is the target: it gets the
  `drop_hover` style and `on_drag_over` with `Enter`, `Move` and `Leave`;
- positions (`DragOverEvent::position`, `DropEvent::position`) are in the
  target's coordinates, in its content for a scroll view: scrolling is
  included, the app does no geometry;
- a scroll view whose edge the pointer stays near (44 pt) scrolls, faster
  nearer the edge, and the drag is reported again over what moved under it;
- the release over a target calls its `on_drop_payload` (with the keys held:
  ⌥ to copy), then `on_drag_over` hears `Leave`. A release over nothing, or
  a cancelled drag, drops nothing.

It starts like `on_drag`: past the platform's slop, with the buttons of
`drag_buttons`. The platform's own hit-testing finds what is under the
pointer, so clipping, scrolling and what is drawn above what are the
platform's. Offer the move another way too (a menu, keys): not every user
can drag. `cx.capabilities().drag_and_drop` is `false` on tvOS and watchOS,
which have no pointer drags. `prism_testing::Harness::drag_payload_to`
drags one in tests.

| | Hit-testing | Preview | Autoscroll |
|---|---|---|---|
| Web | `elementFromPoint` during the pointer events of the drag (not HTML5 drag and drop: the same drag as `on_drag`, touch included, keys live, no click after it) | a copy of the element (`.prism-drag-preview`), `pointer-events: none` | the nearest scrollable ancestor, every animation frame |
| macOS | `hitTest:` in the window | a cached image of the view (`cacheDisplayInRect:`) | `NSClipView`, every 16 ms |
| iOS, iPadOS, visionOS | `hitTest:withEvent:` | `snapshotViewAfterScreenUpdates:` | `UIScrollView.contentOffset` |
| Linux (GTK) | `gtk_widget_pick` | a `GtkWidgetPaintable` of the widget | the `ScrolledWindow`'s adjustments |
| Android | the view tree, children drawn last first | a bitmap of the view in the window's decor | `ScrollView` / `HorizontalScrollView.scrollBy` |
| Windows | the backend's model of the tree (frames, clips, paint order) | a `RenderTargetBitmap` of the element | `ScrollViewer.ChangeView` |

Native backends share the work in `prism_native::dnd`: they report the drag
as any drag and the renderer finds the target, sets its state and the
preview. Targets in sheets and popovers shown in their own window (macOS)
are not reached from the main window. A drag inside a scroll view on a touch
screen may be taken by the scroll view: see the long-press drags of #243.

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
| iOS, iPadOS, visionOS | an iPad trackpad or mouse: a scroll-only `UIPanGestureRecognizer` (`Start`, `Move`, `End`, no momentum; always `precise`) | `UIPinchGestureRecognizer`: two fingers, an iPad trackpad pinch |
| Android | `ACTION_SCROLL` (a mouse wheel, a touchpad's fingers: `precise`), in the system's scroll factor; phase `None` | the fingers' spread, from the touch slop on: two fingers, a touchpad pinch |
| Linux (GTK) | `EventControllerScroll`: a touchpad between `scroll-begin` and `scroll-end` (`Start`, `Move`, `End`, `precise`), a wheel notch counting 48 points (three lines) | `GestureZoom`: a touchscreen, a touchpad pinch |
| Windows | `PointerWheelChanged`: a notch counts 100 points, a precision touchpad sends smaller steps (`precise`); phase `None` | two touches on the element (the scroll views around stop), and Ctrl + wheel (about ×1.2 a notch), the way precision touchpads send their pinches to apps |

tvOS and watchOS have neither (their handlers never run).

On a touch screen, one finger drags and two fingers pinch (#172). With `on_drag` and `on_magnify` on the same element, both run during a pinch: on iOS, Android and Windows the drag follows the fingers' center (on Linux, the first finger), so a two-finger pan while pinching moves the canvas through `on_drag`, and zooming around `MagnifyEvent::position` keeps the content under the fingers. Fingers never send `on_wheel`; on an iPad, a finger still scrolls a `scroll()` around an element with `on_wheel`, while the trackpad's scrolling goes to the element.
