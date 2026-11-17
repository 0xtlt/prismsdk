# Forms and feedback

Native form controls everywhere: dragging, menus, calendars and keyboards behave like the platform's own.

```rust
slider(self.volume).range(0., 100.).step(1.)
    .accessibility_label("Volume")
    .on_change(cx.listener(|this, e, _| this.volume = e.value))
picker(["Small", "Medium", "Large"], self.size).placeholder("Choose…")
    .on_change(cx.listener(|this, e, _| this.size = Some(e.index())))
date_picker(self.due).min(SystemTime::now())                  // .time(), .date_time()
    .on_change(cx.listener(|this, e, _| this.due = e.date()))
date_picker_opt(self.published).placeholder("Not published")  // may be empty, clearable
    .on_change(cx.listener(|this, e, _| this.published = e.date_opt()))
text_input().secure(true).return_key(ReturnKey::Go)
    .on_submit(cx.listener(|this, _, cx| this.sign_in(cx)))
text_input().keyboard(Keyboard::Email).capitalize(Capitalize::None).autocorrect(false)
text_area().value(&self.notes).max_h(px(160.))                // grows, then scrolls
text_area().value(&self.title).rows(1)                         // starts on one line (#235)
spinner().large()

// Themed (label, value, accent), with `.native()` for the bare control
ui::slider("Volume", v).range(0., 100.).step(1.).format(|v| format!("{v:.0} %"))
ui::select("Size", sizes, self.size)
ui::date_field("Due", self.due)
ui::date_field_opt("Published", self.published).placeholder("Not yet")
ui::text_field("Password", &self.password).secure().on_submit(…)
ui::text_area("Notes", &self.notes).max_lines(5)
ui::spinner().label("Syncing…")

// Toasts: from anywhere; the runtime keeps the toaster above the whole app
ui::show_toast(cx, ui::toast("Saved").action("Undo", undo));  // .tone(Variant::Danger), .duration(…)

// Choices, a time of day, files
checkbox(self.remember).label("Remember me")
    .on_change(cx.listener(|this, e, _| this.remember = e.checked()))
radio_group(["Daily", "Weekly", "Monthly"], self.every).horizontal()
    .accessibility_label("Frequency")
    .on_change(cx.listener(|this, e, _| this.every = Some(e.index())))
time_picker(self.alarm)                                       // minutes since midnight, no time zone
    .on_change(cx.listener(|this, e, _| this.alarm = e.minutes()))
time_picker_opt(self.opens).placeholder("Closed")             // Option<u32>: e.minutes_opt()
text_input().leading_icon("magnifyingglass").max_length(24)
    .suggestions(["Paris", "Lyon", "Marseille"])
// Your own field: padding replaces its inset, `.border(0.)` drops its
// default border (see Layout → Padding on every node). Text and
// placeholder start inside the padding (and the border) on every backend.
text_input().pl(36.).py(8.).bg(surface).rounded(10.).border(0.)
text_input().border(1.).border_color(line).rounded(10.).h(44.).px(12.)
ui::file_input("Photo").accept(["image/*"])
    .on_pick(|files, cx| { let bytes = files[0].read(); /* … */ })
let files = cx.pick_files(PickFiles::new().types([".csv"]).multiple()).await?;   // the same, from a task
let files = cx.pick_files_with_progress(PickFiles::new(), |p, cx| {             // web uploads: sent / total
    /* show p.fraction(); cx.cancel_upload(p.upload) stops it */
}).await?;
```

| | Slider | Picker | Date / time | Text area | Spinner |
|---|---|---|---|---|---|
| iOS, visionOS | `UISlider` | `UIButton` + `UIMenu` | `UIDatePicker` (compact) | `UITextView` | `UIActivityIndicatorView` |
| tvOS | focusable track, ◀ ▶ on the remote | `UIButton` + `UIMenu` | focusable field, ◀ ▶ by day / 15 min / hour | `UITextView` | `UIActivityIndicatorView` |
| macOS | `NSSlider` | `NSPopUpButton` | `NSDatePicker` | `NSTextView` | spinning `NSProgressIndicator` |
| Web | `<input type=range>` | `<select>` | `<input type=date / time / datetime-local>` | `<textarea>` | CSS |
| Linux | `GtkScale` | `GtkDropDown` | `GtkCalendar` in a popover | `GtkTextView` | `GtkSpinner` |
| Android | `SeekBar` | `Spinner` | `DatePickerDialog` / `TimePickerDialog` | multi-line `EditText` | indeterminate `ProgressBar` |
| watchOS | SwiftUI `Slider` (Digital Crown) | SwiftUI `Picker` | wheels (`DatePicker`) in a sheet | vertical `TextField` | `ProgressView` |

- **Values**: sliders report `e.value` while dragging (snapped to the step), pickers `e.index()`, date pickers `e.date()` (an instant, shown in the device's time zone).
- **Empty pickers**: `picker(options, None)` shows its `.placeholder(…)` until the user chooses; once a value is picked the list holds only the options, so « nothing » cannot be chosen again (on the web the placeholder `<option>` leaves the `<select>`).
- **Text inputs**: `secure` (password entry), `keyboard` (email, number, decimal, phone, URL), `return_key` (Done, Go, Next, Search, Send), `capitalize`, `autocorrect`, and `on_submit` for the return key. Each maps to the platform's own setting where it has one (no keyboard types on macOS, Linux or the watch (number has no watch equivalent); no return-key label on macOS or Linux; tvOS has no multi-line editing, so text areas are one line there).
- **Accessibility**: sliders are adjustable (VoiceOver swipe up/down, TalkBack, arrow keys) and read their value (`accessibility_value("40 %")` overrides it); pickers read their label and choice; spinners are busy indicators. Toasts are announced politely (`Role::Status`: ARIA live region, VoiceOver and TalkBack announcements, `gtk_accessible_announce`).
- **Checkbox and radio group**: the platform's own controls where it has them (`NSButton` checkbox and radio buttons, `GtkCheckButton` and a group, `CheckBox` and `RadioGroup`, `<input type=checkbox / radio>`, a SwiftUI `Toggle` and a list of buttons on the watch). UIKit has neither: iOS, iPadOS, tvOS and visionOS draw them (SF Symbols) with the toggle-button / selected traits, so VoiceOver reads « checkbox, checked » and « 2 of 3 ». A radio group is one element for the app (`e.index()`), moved through with the arrow keys where the platform does so.
- **Time of day**: `time_picker(minutes)` edits a time that means the same in every time zone (an alarm, opening hours); backends show it in UTC, so « 07:30 » stays 07:30 when the device travels. `date_picker(…).time()` still edits an instant.
- **Optional dates** (#89): `date_picker_opt(Option<SystemTime>)` and `time_picker_opt(Option<u32>)` may be empty (a post not published yet, a day without opening hours). While empty they show their `.placeholder(…)` (also their default accessibility label), and the user can clear a date; read the new value with `e.date_opt()` / `e.minutes_opt()` (`None`: cleared). Starting to fill an empty picker gives it the current moment, to the minute and within the bounds (`DateConfig::start`), which the user then adjusts. On the wire the empty value is `NaN` (`null` in JSON); only optional pickers accept it, so `date_picker` never reports an empty value. The clear action's label is the built-in `clear` text (`prism.clear` to override).

  | | Empty | Filling it | Clearing it |
  |---|---|---|---|
  | Web | an empty `<input type=date…>` (the browser's own format hint) | the browser's picker | the browser's: Clear in Chrome's and Safari's pickers, Firefox's ✕, or erasing every part (non-optional inputs are `required`, which hides those) |
  | iOS, iPadOS, visionOS | a gray pill with the placeholder over the compact picker | a tap on the pill (then the compact picker) | an `xmark.circle.fill` button after the picker |
  | macOS | the placeholder inside the picker's bezel | a click on it, typing, or the stepper | an `xmark.circle.fill` button inside the bezel |
  | Linux | the placeholder, dimmed, on the button | opening the popover | « Clear » under the calendar |
  | Android | the placeholder, dimmed, in the pill | the dialog (OK) | « Clear » in the date or time dialog |
  | watchOS | the placeholder, dimmed, on the button | opening the wheels | « Clear » under the wheels |
  | tvOS | the placeholder, dimmed | the first left / right press | Play/Pause |
- **Controlled values**: `.value(..)` is the field's text, focused or not. What the user types comes back as the same value and leaves the field alone (caret, selection, IME composition); a different value is the app's and replaces the field's text, the caret keeping its distance from the end. So an `on_input` handler can rewrite what was typed (strip a Markdown « # » prefix, uppercase, clear after a command) and the field follows. During an IME composition, the web applies it once the composition ends.
- **Text input extras**: `max_length` (`maxlength`, a length filter natively), `leading_icon` (an SF Symbol inside the field: `leftView`, a GTK primary icon, a compound drawable, a glyph on the web), `suggestions` (web `<datalist>`, AppKit's completion list, `AutoCompleteTextView`, `GtkEntryCompletion` on Linux, a bar of matching suggestions above the keyboard on iOS, which has no API for app-supplied completions; none on tvOS, visionOS and watchOS), `autofocus` and `.id` with `cx.focus` (see [Pointer, keyboard, focus](input.md)).
- **Files**: `ui::file_input` (or `cx.pick_files`) opens `<input type=file>`, `UIDocumentPickerViewController`, `NSOpenPanel`, `GtkFileChooserNative` (the desktop portal) or `ACTION_OPEN_DOCUMENT`; `PickedFile::read()` returns the bytes. On the web the browser uploads them over the app's connection and `read()` reads the server's copy. There is no file picker on tvOS and watchOS (`DeviceError::Unavailable`).
- **Folders** (#225): `cx.pick_folder(PickFolder::new().title("Import")).await?` gives `Option<PickedFolder { name, path, files }>`: every file under the chosen folder, sub-folders included (hidden ones left out), each `PickedFile::name` being its path in the folder (`notes/2024/a.md`). `NSOpenPanel` on macOS and GTK's folder chooser on Linux read the files in place (`path` is the folder's); the Files app's folder picker on iOS and iPadOS and `ACTION_OPEN_DOCUMENT_TREE` on Android copy them for the app; the web uploads them like picked files (`<input webkitdirectory>`). `DeviceError::Unavailable` on tvOS and watchOS.
- **Drop targets** (#111): `div().on_drop_files(["image/*", ".pdf"], cx.listener(|this, e: &FileDropEvent, cx| …))` receives files dragged from the system (the desktop, the Files app, another app in split screen) and dropped on the element; `e.files` are the dropped files it accepts, readable like picked ones, and the handler is not called when none is. `.drop_hover(|s| s.bg(…))` styles the zone while accepted files are over it. Types are those of `PickFiles::types` (empty for any file); the innermost target under the pointer gets the drop. Keep a chooser next to it (`ui::file_input`): phones rarely drag files. `cx.capabilities().drop_files` is `false` on tvOS and watchOS. On the web, `.on_drop_progress(cx.listener(|this, e: &DropProgress, cx| …))` follows the upload of what was dropped (#199): `e.sent` of `e.total` bytes (`e.fraction()`), from 0 as the upload starts to the last byte, just before `on_drop_files`, like `pick_files_with_progress` for picked files. Natively the files are on the device already, and it is not called.

  | | Drop | Highlight | Files |
  |---|---|---|---|
  | Web | `dragover` / `drop` (a drop elsewhere on a page with drop targets does not open the file) | `[data-prism-drop]` | uploaded over the app's connection like picked files; the server takes only drops on its drop targets, of types they accept |
  | macOS | `NSDraggingDestination` on the view | while the drag holds accepted files | copied to the cache, off the main thread |
  | iOS, iPadOS, visionOS | `UIDropInteraction` | `sessionDidEnter` / `Exit` | loaded with `loadFileRepresentation`, copied to the cache |
  | Android | `View.OnDragListener` (with `requestDragAndDropPermissions`) | `ACTION_DRAG_ENTERED` / `EXITED` | copied to the cache |
  | Linux | `GtkDropTarget` of a file list | `:drop(active)` | copied to the cache |
- **Upload progress** (#112): on the web the files reach the app before `on_pick` runs, which takes a while for a large photo on a phone. `cx.pick_files_with_progress(options, |p, cx| …)` and `ui::file_input(…).on_progress(|p, cx| …)` are called once the user has chosen (`p.sent == 0`) and as the bytes arrive (`p.sent` of `p.total`, every file included; `p.fraction()`). `cx.cancel_upload(p.upload)` stops sending: the pick then gives no files and what arrived is deleted. The browser sends files in chunks of 256 KB and waits while the connection is busy, so the progress follows the network; the server writes each chunk as it comes, takes no more than the size the browser announced, and only for a pick the app asked for. Natively the files are already on the device: no progress is reported, and `cancel_upload` does nothing.
- **Toasts**: `ui::show_toast` works from any handler or task: the runtime mounts the toaster as a layer above the router, tabs, sheets and dialogs, so a toast shown right before `cx.back()` stays. `ui::toaster()` is deprecated and does nothing (remove it).
- **Right to left**: sliders fill from the right, menus and calendars follow the language.
- **Tint**: `text_color` tints a control (`ui::*` uses the theme's accent).

- **Platform notes**: tvOS has no `UISlider`, `UIDatePicker` or editable `UITextView` (`API_UNAVAILABLE(tvos)`), hence the focus-driven stand-ins. watchOS sliders always step (a continuous one moves by a tenth of its range), and dates open the system wheels in a sheet. `GtkCalendar` has no min/max, so out-of-range days snap back. Before GTK 4.14, Linux has no `gtk_accessible_announce`: the status text changes but may not be spoken. Android announces through a polite live region (`announceForAccessibility` is deprecated in API 36), and the date dialog's header follows the device language, not the app's.
- **Dev hooks for forms**: macOS `PRISM_TAP=<label>`, `PRISM_TYPE=<text>` (`\n` is Return), `PRISM_INCREMENT=<label>` (VoiceOver increment), `PRISM_A11Y_LOG=1`; Linux `PRISM_CLICK` opens pickers and date pickers, `fill:<placeholder>=<text>`, `fill:@slider=40`, `submit:<placeholder>`, open popovers saved as `out-N.png`; watchOS `PRISM_SCROLL=<pt>`, `PRISM_PRESENT=<label>` (opens a date picker), `PRISM_DEBUG_A11Y=1`. The watch's `ImageRenderer` snapshot can't draw native controls: use `xcrun simctl io <udid> screenshot`.

## Leaving a field

`on_focus_change` hears a field (or a view focusable through `on_key_down`) take and lose the keyboard focus (#224): commit an inspector's value or an inline rename when the user tabs away or clicks elsewhere, not on every keystroke and not only on Enter.

```rust
text_input().value(self.draft.clone())
    .on_input(cx.listener(|this, e: &InputEvent, _| this.draft = e.value.clone()))
    .on_focus_change(cx.listener(|this, e: &FocusEvent, cx| {
        if !e.focused { this.commit_rename(cx) }
    }))
```

It is the node's own focus (for a group, style with `focus_within`). Every backend reports it: the web from `focusin` / `focusout` (switching to another window or tab counts as leaving), macOS from the window's first responder, iOS, Linux and Android from their focus states, all through the same focus tracking as `.focus(|s| …)`. Natively, switching to another app keeps the field focused.

## Selection and caret

A text input or area reports where its selection is with `on_select`, and takes one from the app with `.selection(range)`. Offsets are **characters** (Rust `char`s), not UTF-8 bytes nor UTF-16 units, on every backend; `start == end` is a caret. `e.byte_range(&text)` (and `prism::char_to_byte` / `byte_to_char`) converts them to slice a `&str`. Key presses in a focused field carry its selection in `KeyEvent::selection`, read before the key acts, so `on_key_down` can split or merge blocks:

```rust
struct Block { text: String, sel: SelectionEvent }

text_input()
    .value(self.text.clone())
    .on_input(cx.listener(|this, e: &InputEvent, _| this.text = e.value.clone()))
    // The selection or caret moved: selecting, clicking, arrows, typing.
    .on_select(cx.listener(|this, e: &SelectionEvent, _| this.sel = *e))
    // Rendered back as is: an echo, the field is left alone.
    .selection(self.sel)
    .on_key_down(cx.listener(|this, e: &KeyEvent, cx| {
        let Some(sel) = e.selection else { return };
        if e.is("b") && e.modifiers.command() && !sel.is_caret() {
            // ⌘B / Ctrl+B: bold the selection, keep it selected.
            let r = sel.byte_range(&this.text);
            this.text.replace_range(r.clone(), &format!("**{}**", &this.text[r]));
            this.sel = (sel.start + 2..sel.end + 2).into();
            cx.stop_propagation();
        } else if e.is("Enter") {
            // Split the block at the caret: the new block starts with the rest.
            let r = sel.byte_range(&this.text);
            let rest = this.text.split_off(r.end);
            this.text.truncate(r.start);
            this.split_off(rest, cx);
        } else if e.is("Backspace") && sel.start == 0 && sel.end == 0 {
            this.merge_with_previous(cx);
        }
    }))
```

- **When `on_select` runs**: after every move of the caret or selection, typing included (after that edit's `on_input`), and only when it moved. The web sends at most one per animation frame; native backends one per run-loop turn. A key press first delivers a pending selection, then the key with `selection` as it was before the key.
- **`.selection(range)`** is applied when the app's value changes, after the field's text, focused or not. A selection the user made and the app renders back is an echo and leaves the field alone, and a value that did not change is not applied again, so keeping it in sync with `on_select` never fights the user. On the web, a selection computed while the user has typed further (values not yet echoed) is dropped, like a stale text echo; during an IME composition it waits for the composition to end. Offsets past the end are clamped.
- **Backends**: web (`selectionchange`, `select`, keys, pointer), AppKit (the field editor's and text view's `NSTextViewDidChangeSelectionNotification`), UIKit (`textFieldDidChangeSelection:`, `textViewDidChangeSelection:`), GTK (`notify::cursor-position` / `selection-bound`, the text buffer's marks), Android (`onSelectionChanged`). On macOS and iOS, a single-line input that is not being edited takes the app's selection when the app focuses it (`cx.focus`); a click or tap places the caret where the user put it. watchOS has no selection API (`on_select` never runs, `KeyEvent::selection` is `None`, `.selection` is ignored).

## Rich text input

`rich_input(spans)` edits the runs of a [`rich_text`](typography.md#rich-text) in place: bold, italic, underline, strikethrough, inline code (`font_mono()` with a `bg`) and links stay styled while the user types, and a `chip(key, label)` (a mention of a page, a person, a date: an optional `chip_icon`, then its label) is one character the caret jumps over and one Backspace deletes. It is controlled like `text_input`: render the spans you store, store the spans `on_input` gives back.

```rust
struct Note { spans: Vec<Span>, sel: SelectionEvent }

rich_input(self.spans.clone())
    .placeholder("Write something…")
    .on_input(cx.listener(|this, e: &RichInputEvent, _| this.spans = e.spans.clone()))
    .on_select(cx.listener(|this, e: &SelectionEvent, _| this.sel = *e))
    .selection(self.sel)
    .on_submit(cx.listener(|this, _, cx| this.split_at_caret(cx)))
    .on_key_down(cx.listener(|this, e: &KeyEvent, _| {
        // ⌘B / Ctrl+B: the field has no formatting commands of its own.
        if e.is("b") && e.modifiers.command() {
            this.spans = restyle_spans(&this.spans, this.sel.range(), Span::bold);
        }
    }))

// A mention picker replaces the typed "@ro" with a chip.
let at = self.sel.start;
self.spans = splice_spans(&self.spans, at - 3..at, [chip("page:42", "Roadmap").chip_icon("doc.text")]);
self.sel = SelectionEvent::caret(at - 2);
```

- **The value**: `RichInputEvent { value, spans }`. `value` is the plain text, a chip being `prism::OBJECT_CHAR` (U+FFFC); selections (`on_select`, `.selection`, `KeyEvent::selection`) count its characters, a chip as one. Each span keeps the style, link, `on_click` and chip key of the run it came from; read them with `text()`, `is_bold()`, `is_italic()`, `is_underlined()`, `is_strikethrough()`, `is_mono()`, `href()`, `chip_key()`, `chip_label()` to map them to your own model. Spans come back normalized (no empty run, equal neighbours merged).
- **Editing helpers**, by characters: `split_spans(&spans, at)` (split a block at the caret), `restyle_spans(&spans, range, f)` (format a selection), `splice_spans(&spans, range, insert)` (insert or replace, chips included), `normalize_spans`, `spans_text`.
- **Typing** takes the style of the run before the caret (as editors do), except after a chip or a link: then the run after it, or plain text. Over a selection, the style of its first character. The app may restyle what comes back (a « pending bold » after ⌘B on a caret, Markdown shortcuts).
- **Paste** is plain text, in the style of where it lands (object characters dropped: only the app makes chips); copy and cut give plain text, a chip as its label. Formatting commands of the platform (the browser's ⌘B, the font panel) do nothing.
- **Enter** calls `on_submit` and inserts nothing; **Shift+Enter** inserts a line break (`'\n'`). The field wraps like a paragraph and grows with its text (one line at least; cap it with `max_h`).
- **Click to edit**: with no padding, border or background (`.p(0.).border(0.).bg(transparent)`), a `rich_input` or `text_area` with the same typography as a `text()` draws its text exactly where the text was: same start, same line height (`line_height`), same baseline, on every line (#179). Swap one for the other and nothing moves.
- **App values and echoes** follow the text field rules: runs the user just typed, rendered back, leave the field alone (caret, IME composition); other runs are the app's and replace the field's, the caret keeping its distance from the end, after the composition on the web. `.selection(..)` after them places the caret.

| | Rich editing |
|---|---|
| Web | a `contenteditable` drawn from runs the client owns: `beforeinput` edits them (typing, deletion by character, word or line, a chip as one character), an IME composition is the browser's and is read back when it ends, undo/redo step through the field's own edits |
| macOS | `NSTextView` with the runs as attributes and each chip an `NSTextAttachment` (its icon and label drawn in a rounded box); undo is AppKit's |
| iOS, iPadOS, visionOS, GTK, Android, watchOS | **not yet**: a plain multi-line field of `value` (no styles drawn, a chip shows as U+FFFC). Edits still come back as spans that keep the styles around them, a deleted U+FFFC removes its chip, and Enter (a line break typed alone) calls `on_submit` when the input has one; tvOS shows one line |

**Sheets, popovers, dialogs** that a screen opens without a route: see [Dialogs, menus, tray](dialogs-menus.md#sheets-popovers-and-dialogs-without-a-route).

**Absolute positioning**: `div().absolute().bottom(16.).left(16.).right(16.)` takes an element out of the flex flow and places it at its insets from the parent (`.top()`, `.inset(0.)`…), like CSS `position: absolute` (taffy on native). Later siblings draw on top; `left`/`right` are the leading and trailing edges.
