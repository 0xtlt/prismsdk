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
| Windows | `Slider` | `ComboBox` | `CalendarDatePicker` / `TimePicker` (both side by side for `date_time()`) | multi-line `TextBox` | `ProgressRing` |
| watchOS | SwiftUI `Slider` (Digital Crown) | SwiftUI `Picker` | wheels (`DatePicker`) in a sheet | vertical `TextField` | `ProgressView` |

- **Values**: sliders report `e.value` while dragging (snapped to the step), pickers `e.index()`, date pickers `e.date()` (an instant, shown in the device's time zone).
- **Empty pickers**: `picker(options, None)` shows its `.placeholder(…)` until the user chooses; once a value is picked the list holds only the options, so « nothing » cannot be chosen again (on the web the placeholder `<option>` leaves the `<select>`).
- **Text inputs**: `secure` (password entry), `keyboard` (email, number, decimal, phone, URL), `return_key` (Done, Go, Next, Search, Send), `capitalize`, `autocorrect`, and `on_submit` for the return key. Each maps to the platform's own setting where it has one (no keyboard types on macOS, Linux or the watch (number has no watch equivalent); no return-key label on macOS or Linux, and only Search on Windows, whose touch keyboard has no other; Windows has no scope that capitalizes every letter, and its touch keyboard capitalizes sentences only while spell checking is on, so `capitalize(None)` turns spell checking off; tvOS has no multi-line editing, so text areas are one line there).
- **Accessibility**: sliders are adjustable (VoiceOver swipe up/down, TalkBack, arrow keys) and read their value (`accessibility_value("40 %")` overrides it); pickers read their label and choice; spinners are busy indicators. Toasts are announced politely (`Role::Status`: ARIA live region, VoiceOver and TalkBack announcements, `gtk_accessible_announce`, UI Automation's `RaiseNotificationEvent`).
- **Checkbox and radio group**: the platform's own controls where it has them (`NSButton` checkbox and radio buttons, `GtkCheckButton` and a group, `CheckBox` and `RadioGroup`, WinUI's `CheckBox` and `RadioButton`s, `<input type=checkbox / radio>`, a SwiftUI `Toggle` and a list of buttons on the watch). UIKit has neither: iOS, iPadOS, tvOS and visionOS draw them (SF Symbols) with the toggle-button / selected traits, so VoiceOver reads « checkbox, checked » and « 2 of 3 ». A radio group is one element for the app (`e.index()`), moved through with the arrow keys where the platform does so.
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
  | Windows | the `CalendarDatePicker`'s placeholder; an empty `TimePicker` | opening the calendar or the time flyout | a ✕ button after the picker (named and tooltipped with the `clear` text) |
- **Controlled values**: `.value(..)` is the field's text, focused or not. What the user types comes back as the same value and leaves the field alone (caret, selection, IME composition); a different value is the app's and replaces the field's text, the caret keeping its distance from the end. So an `on_input` handler can rewrite what was typed (strip a Markdown « # » prefix, uppercase, clear after a command) and the field follows. During an IME composition, the web applies it once the composition ends.
- **Text input extras**: `max_length` (`maxlength`, a length filter natively), `leading_icon` (an SF Symbol inside the field: `leftView`, a GTK primary icon, a compound drawable, a glyph on the web), `suggestions` (web `<datalist>`, AppKit's completion list, `AutoCompleteTextView`, `GtkEntryCompletion` on Linux, a bar of matching suggestions above the keyboard on iOS, which has no API for app-supplied completions; a list under the field on Windows, like `AutoSuggestBox`'s, the arrows and Enter choosing; none on tvOS, visionOS and watchOS), `autofocus` and `.id` with `cx.focus` (see [Pointer, keyboard, focus](input.md)).
- **Files**: `ui::file_input` (or `cx.pick_files`) opens `<input type=file>`, `UIDocumentPickerViewController`, `NSOpenPanel`, `GtkFileChooserNative` (the desktop portal), `ACTION_OPEN_DOCUMENT` or `IFileOpenDialog`; `PickedFile::read()` returns the bytes. On the web the browser uploads them over the app's connection and `read()` reads the server's copy. There is no file picker on tvOS and watchOS (`DeviceError::Unavailable`).
- **Folders** (#225): `cx.pick_folder(PickFolder::new().title("Import")).await?` gives `Option<PickedFolder { name, path, files }>`: every file under the chosen folder, sub-folders included (hidden ones left out), each `PickedFile::name` being its path in the folder (`notes/2024/a.md`). `NSOpenPanel` on macOS, GTK's folder chooser on Linux and `IFileOpenDialog` (`FOS_PICKFOLDERS`) on Windows read the files in place (`path` is the folder's); the Files app's folder picker on iOS and iPadOS and `ACTION_OPEN_DOCUMENT_TREE` on Android copy them for the app; the web uploads them like picked files (`<input webkitdirectory>`). `DeviceError::Unavailable` on tvOS and watchOS.
- **Drop targets** (#111): `div().on_drop_files(["image/*", ".pdf"], cx.listener(|this, e: &FileDropEvent, cx| …))` receives files dragged from the system (the desktop, the Files app, another app in split screen) and dropped on the element; `e.files` are the dropped files it accepts, readable like picked ones, and the handler is not called when none is. `.drop_hover(|s| s.bg(…))` styles the zone while accepted files are over it. Types are those of `PickFiles::types` (empty for any file); the innermost target under the pointer gets the drop. Keep a chooser next to it (`ui::file_input`): phones rarely drag files. `cx.capabilities().drop_files` is `false` on tvOS and watchOS. On the web, `.on_drop_progress(cx.listener(|this, e: &DropProgress, cx| …))` follows the upload of what was dropped (#199): `e.sent` of `e.total` bytes (`e.fraction()`), from 0 as the upload starts to the last byte, just before `on_drop_files`, like `pick_files_with_progress` for picked files. Natively the files are on the device already, and it is not called.
- **Refused drops** (#290): `.on_drop_error(cx.listener(|this, e: &DropErrorEvent, cx| …))` hears of files dropped on the target that did not reach the app, so it can tell the user. `e.error` says why: `DropError::NotAccepted` (none is of a type it takes), `Limits` (more files or bytes than the web app's `upload_limits`, or more than four drops uploading at once), `Busy` (the server receives too many files from every session right now), `Failed` (the files could not be read or stored: the data directory is not writable, a file vanished, the upload was cut off); `e.reason` has the details in English, for a log. When only some files failed, `on_drop_files` gets the others. On the web, a refused drop stops uploading at once, and the server says on stderr at startup when its data directory (`PRISM_DATA_DIR`, `WebApp::data_dir`) is not writable.
- **The zone's children while files are over it** (#290): mark the drop target `.group()` (or `.group_named("zone")`), and its descendants follow its drop hover with `.group_drop_hover(|s| …)` (or `.group_drop_hover_named("zone", |s| …)`), like `group_hover`: a label that says « Release to upload », an icon that lights up.

```rust
div()
    .group()
    .on_drop_files(["image/*"], cx.listener(|this, e: &FileDropEvent, _| this.add(&e.files)))
    .on_drop_error(cx.listener(|this, e: &DropErrorEvent, _| {
        this.notice = Some(match e.error {
            DropError::NotAccepted => "Only images, please",
            DropError::Limits => "Too many files at once",
            _ => "The upload failed: try again",
        })
    }))
    .drop_hover(|s| s.bg(rgb(0xEEF2FF)))
    .child(text("Drop images here").opacity(0.6).group_drop_hover(|s| s.opacity(1.).bold()))
```

  | | Drop | Highlight | Files |
  |---|---|---|---|
  | Web | `dragover` / `drop` (a drop elsewhere on a page with drop targets does not open the file) | `[data-prism-drop]` | uploaded over the app's connection like picked files; the server takes only drops on its drop targets, of types they accept |
  | macOS | `NSDraggingDestination` on the view | while the drag holds accepted files | copied to the cache, off the main thread |
  | iOS, iPadOS, visionOS | `UIDropInteraction` | `sessionDidEnter` / `Exit` | loaded with `loadFileRepresentation`, copied to the cache |
  | Android | `View.OnDragListener` (with `requestDragAndDropPermissions`) | `ACTION_DRAG_ENTERED` / `EXITED` | copied to the cache |
  | Linux | `GtkDropTarget` of a file list | `:drop(active)` | copied to the cache |
  | Windows | XAML's drop target (`AllowDrop`, OLE: Explorer, any app) | while the drag holds accepted files (read under the event's deferral) | copied to the app's `<data>\picked` (with picked files), off the UI thread |
- **Upload progress** (#112): on the web the files reach the app before `on_pick` runs, which takes a while for a large photo on a phone. `cx.pick_files_with_progress(options, |p, cx| …)` and `ui::file_input(…).on_progress(|p, cx| …)` are called once the user has chosen (`p.sent == 0`) and as the bytes arrive (`p.sent` of `p.total`, every file included; `p.fraction()`). `cx.cancel_upload(p.upload)` stops sending: the pick then gives no files and what arrived is deleted. The browser sends files in chunks of 256 KB and waits while the connection is busy, so the progress follows the network; the server writes each chunk as it comes, takes no more than the size the browser announced, and only for a pick the app asked for. The server also caps how many files one pick takes, the bytes one browser keeps and the bytes all sessions receive at once, and deletes uploads after 24 hours ([web limits](../platforms/web.md)): keep what the app needs with `PickedFile::read`. Natively the files are already on the device: no progress is reported, and `cancel_upload` does nothing.
- **Toasts**: `ui::show_toast` works from any handler or task: the runtime mounts the toaster as a layer above the router, tabs, sheets and dialogs, so a toast shown right before `cx.back()` stays. `ui::toaster()` is deprecated and does nothing (remove it).
- **What handlers receive**: only what the control could report, whatever the platform (or a modified web page) sends. A control marked `accessibility_disabled(true)` gets no clicks, typing, submits or changes. A slider's value is within its range and on its steps; a date picker's within its `min`/`max` and years 1 to 9999. An index past the options of a picker, radio group or tab bar, a toggle or checkbox value other than 0 or 1, or a number that is not finite is dropped.
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

It is the node's own focus (for a group, style with `focus_within`). Every backend reports it: the web from `focusin` / `focusout` (switching to another window or tab counts as leaving), macOS from the window's first responder, iOS, Linux, Android and Windows from their focus states, all through the same focus tracking as `.focus(|s| …)`. Natively, switching to another app keeps the field focused.

## A bar above the keyboard

`.keyboard_toolbar(bar)` puts a bar of the app's own (bold, link, insert a block, Done) right above the on-screen keyboard while a field has the focus (#242). On a text input, rich input or text area it serves that field; on a container (`div().keyboard_toolbar(bar)`, or a `keyboard_toolbar(bar)` child) it serves every field inside. A field's own bar wins over its container's, and a nearer container's over a farther one's.

```rust
rich_input(self.runs())
    .keyboard_toolbar(
        div().flex_row().gap(8.).px(12.).py(6.).bg(theme.surface_alt)
            .child(ui::button("B").on_click(cx.listener(|this, _, cx| this.toggle_bold(cx))))
            .child(ui::button("Done").on_click(cx.listener(|_, _, cx| cx.dismiss_keyboard()))),
    )
```

The bar is an ordinary element: its handlers run like any other, and tapping its buttons keeps the field focused and the keyboard up. It is laid out at the window's width (at most half its height); give it its height, padding and background.

- **iOS, iPadOS**: the field's `inputAccessoryView`, an input view in the keyboard's style (above a hardware keyboard's shortcut bar too). A field's suggestion bar (`suggestions`) takes its place while it shows.
- **Android**: docked to the bottom of the window above the IME (the window insets), while a field it serves has the focus; the app's content is laid out above it.
- **Web**: on touch screens (`pointer: coarse`), fixed to the bottom of the visual viewport (`VisualViewport`), which the keyboard shrinks; presses on it do not move the focus. It is not in the first paint.
- **macOS, Linux, Windows, tvOS, watchOS, visionOS, desktop browsers**: no on-screen keyboard bar. Native desktops leave it out of the tree (its content is not built); desktop browsers keep it hidden.

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
- **Backends**: web (`selectionchange`, `select`, keys, pointer), AppKit (the field editor's and text view's `NSTextViewDidChangeSelectionNotification`), UIKit (`textFieldDidChangeSelection:`, `textViewDidChangeSelection:`), GTK (`notify::cursor-position` / `selection-bound`, the text buffer's marks), Android (`onSelectionChanged`), WinUI (`SelectionChanged` of the `TextBox` and the `RichEditBox`). On macOS and iOS, a single-line input that is not being edited takes the app's selection when the app focuses it (`cx.focus`); a click or tap places the caret where the user put it. watchOS has no selection API (`on_select` never runs, `KeyEvent::selection` is `None`, `.selection` is ignored).

## Other people's carets

In a collaborative document, `.remote_carets(..)` draws the carets and selections of the other people editing a text input, a text area or a rich input, inside the field being edited: a thin caret in each person's color with their name in a small tag, and their selection tinted with that color. The field lays them out with its own text (wrapping and scrolling included) and draws them above it; they are never editable and take no clicks, and updating them leaves the user's caret, selection and IME composition alone.

```rust
rich_input(self.spans.clone())
    .on_input(cx.listener(|this, e: &RichInputEvent, _| this.spans = e.spans.clone()))
    .remote_carets(self.peers.iter().map(|p| {
        RemoteCaret::new(p.anchor, p.head, p.color)   // a selection; `RemoteCaret::caret(at, color)` for a caret alone
            .label(p.name.clone())
            .idle(p.last_seen.elapsed() > Duration::from_secs(3))
    }))
```

- **Offsets** are characters, like [`SelectionEvent`](#selection-and-caret) (a chip or an icon of a rich input counts as one). The app moves them when the text changes under them (its collaboration engine already does: CRDT or OT positions mapped to characters); offsets past the end are clamped. An empty list removes them.
- **Label text**: white by default; `.label_text_color(color)` sets it, for a light person color that needs dark text (#267), on every backend.
- **Idle**: `idle(true)` fades the name out and keeps the caret and the selection. The app decides when, from its presence data (a few seconds without an update is usual), so every backend agrees.
- **Secure fields** show no one's caret.
- **Backends**: the web draws a layer next to the field (ranges of a rich input's DOM, a hidden copy of a textarea or input for the others), redrawn after updates, typing, scrolling and resizing. macOS lays a view over the field from its layout manager (the field editor's while a text input is edited); iOS, iPadOS and visionOS use the field's `UITextInput` geometry (`caretRectForPosition:`, `selectionRectsForRange:`); Android draws in the `EditText`'s overlay from its `Layout`; Linux draws over the `GtkTextView` of text areas and rich inputs. The name sits above the caret's line, under it on the first line of a text area that clips its content. It fades on the web, macOS, iOS and Android, and disappears at once on Linux. Windows lays a canvas over the field (`TextBox::GetRectFromCharacterIndex`, RichEdit's `ITextRange::GetRect`), measured again after typing, resizing and scrolling.
- **Limits**: a one-line text input on Linux (`GtkEntry`) shows none, and neither do tvOS (it edits text in its full-screen keyboard) and watchOS.

## Rich text input

`rich_input(spans)` edits the runs of a [`rich_text`](typography.md#rich-text) in place: bold, italic, underline, strikethrough, inline code (`font_mono()` with a `bg`) and links stay styled while the user types, and a `chip(key, label)` (a mention of a page, a person, a date: an optional `chip_icon`, then its label) is one character the caret jumps over and one Backspace deletes. So is an `icon(name)` (#221): one character, drawn as its symbol. Underline colors and styles (`underline_color`, `underline_style`) are drawn too. It is controlled like `text_input`: render the spans you store, store the spans `on_input` gives back.

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

- **The value**: `RichInputEvent { value, spans }`. `value` is the plain text, a chip or an icon being `prism::OBJECT_CHAR` (U+FFFC); selections (`on_select`, `.selection`, `KeyEvent::selection`) count its characters, a chip as one. Each span keeps the style, link, `on_click` and chip key of the run it came from; read them with `text()`, `is_bold()`, `is_italic()`, `is_underlined()`, `underline_color_value()`, `underline_style_value()`, `is_strikethrough()`, `is_mono()`, `href()`, `chip_key()`, `chip_label()`, `icon_name()` to map them to your own model. Spans come back normalized (no empty run, equal neighbours merged).
- **Editing helpers**, by characters: `split_spans(&spans, at)` (split a block at the caret), `restyle_spans(&spans, range, f)` (format a selection), `splice_spans(&spans, range, insert)` (insert or replace, chips included), `normalize_spans`, `spans_text`.
- **Typing** takes the style of the run before the caret (as editors do), except after a chip, an icon or a link: then the run after it, or plain text. Over a selection, the style of its first character. The app may restyle what comes back (a « pending bold » after ⌘B on a caret, Markdown shortcuts).
- **Paste** is plain text, in the style of where it lands (object characters dropped: only the app makes chips); copy and cut give plain text, a chip as its label (on the web, an icon as its glyph). Formatting commands of the platform (the browser's ⌘B, the font panel) do nothing.
- **Enter** calls `on_submit` and inserts nothing; **Shift+Enter** inserts a line break (`'\n'`). The field wraps like a paragraph and grows with its text (one line at least; cap it with `max_h`).
- **Click to edit**: with no padding, border or background (`.p(0.).border(0.).bg(transparent)`), a `rich_input` or `text_area` with the same typography as a `text()` draws its text exactly where the text was: same start, same line height (`line_height`), same baseline, on every line (#179). Swap one for the other and nothing moves.
- **App values and echoes** follow the text field rules: runs the user just typed, rendered back, leave the field alone (caret, IME composition); other runs are the app's and replace the field's, the caret keeping its distance from the end, after the composition on the web. `.selection(..)` after them places the caret.

| | Rich editing |
|---|---|
| Web | a `contenteditable` drawn from runs the client owns: `beforeinput` edits them (typing, deletion by character, word or line, a chip as one character), an IME composition is the browser's and is read back when it ends, undo/redo step through the field's own edits |
| macOS | `NSTextView` with the runs as attributes and each chip an `NSTextAttachment` (its icon and label drawn in a rounded box), each icon an attachment of its SF Symbol; a click on a chip with an `on_click` reports it; undo is AppKit's |
| Windows | `RichEditBox` with the runs as RichEdit character formats and each chip a picture embedded in the text (`InsertImage`: its Segoe Fluent icon and label drawn in a rounded box with Direct2D, at the window's scale); undo is RichEdit's |
| iOS, iPadOS, visionOS | `UITextView` with the runs as attributes and each chip or icon an `NSTextAttachment` (its SF Symbol and label drawn in a rounded box, again when the appearance changes); restyling the text the field shows changes its attributes in place, so the caret, the selection and a composition stay; a tap on a chip with an `on_click` reports it; undo is UIKit's |
| Linux | `GtkTextView` with the runs as text tags and each chip or icon a widget at a child anchor (its symbol and label in a rounded box, on the text's baseline), which the caret, the selection and Backspace treat as one character; a click on a chip with an `on_click` reports it; undo is GTK's |
| Android | `EditText` with the runs as spans, and each chip or icon a span drawn in place of its character (its glyph and label in a rounded box, on the baseline); restyling the text the field shows changes its spans in place, caret and IME composition untouched; a tap on a chip with an `on_click` reports it |
| watchOS | a plain multi-line field of `value` (a chip or an icon shows as U+FFFC): SwiftUI's `TextEditor`, its only view that edits an `AttributedString`, is `@available(watchOS, unavailable)`, and `TextField` binds a plain `String`. Edits still come back as spans that keep the styles around them, a deleted U+FFFC removes its chip, and Enter (a line break typed alone) calls `on_submit` when the input has one |
| tvOS | the same plain field, on one line: `UITextView`'s `editable` is `API_UNAVAILABLE(tvos)`, so text is typed in the system's full-screen keyboard, which edits plain text |

### Toolbars, the clipboard and modifier-clicks

A document editor places UI at the selection, owns copy and paste, and opens links with ⌘-click (#217):

```rust
div() // laid out at the field's origin: no padding
    .child(
        rich_input(self.spans.clone())
            .on_selection_rect(cx.listener(|this, e: &SelectionRectEvent, _| {
                this.toolbar = (!e.selection.is_caret()).then_some(e.rect)
            }))
            .on_copy(cx.listener(|_, e: &CopyEvent, _| {
                e.write(ClipboardContent::text(&e.text).markdown(to_markdown(&e.spans)))
            }))
            .on_paste(cx.listener(|this, e: &PasteEvent, _| {
                if e.text.contains('\n') {
                    e.prevent_default();
                    this.insert_blocks(e.selection, &e.text);
                }
            }))
            .on_span_click(cx.listener(|this, e: &SpanClickEvent, cx| this.open_link(e.span, cx))),
    )
    .children(self.toolbar.map(|r| {
        div().absolute().left(r.x).top(r.y).w(r.width).h(r.height).child(
            popover(toolbar).open(true).side(PopoverSide::Top).take_focus(false),
        )
    }))
```

- **The selection's rectangle**: `on_selection_rect` hears `SelectionRectEvent { selection, rect, frame }` whenever the caret or the selection moves or the text under it changes. `rect` is in the field's own coordinates (the caret one point wide; a selection on several lines, the box around them), so an absolutely positioned element at `rect`, in a container laid out at the field's origin, anchors a popover above the selected words. `frame` is the same rectangle in the window's layout coordinates (like `cx.frame_of`), once the field has been laid out. `.track_selection_rect()` keeps it for `cx.selection_rect(id)` without a handler.
- **Copy and cut**: `on_copy` / `on_cut` hear ⌘C / ⌘X (the keyboard, the Edit menu, the context menu) with text selected: `CopyEvent { selection, text, spans, cut }`, `spans` being the selected runs. `e.write(ClipboardContent)` puts the app's content on the clipboard as one item: plain text plus `html`, `markdown` and types of its own (`custom("application/x-blocks", json)`), which the app reads back on paste. Without `write`, the plain text is copied. A cut then deletes the selection (`on_input` reports it).
- **Paste**: `on_paste` hears ⌘V with `PasteEvent { selection, text, html, types, data }` (`data`: Markdown and the apps' own text types, where the platform hands them over; `e.get(mime)`, `e.has(mime)`). `e.prevent_default()` takes it over (split lines into blocks, upload an image the clipboard holds); otherwise the plain text lands over the selection.
- **Modifier-clicks**: with `on_span_click`, a click with a modifier key on the field while it is edited is the app's: `SpanClickEvent { span, offset, modifiers }`, `span` indexing the spans you rendered. A plain click still places the caret.
- **Every click**: `on_click` hears each click in the field with the character under the pointer (`ClickEvent::text_offset`, #266), its keys and count; the field still places its caret. A ⇧-click on a field that is not focused can extend a selection to that point from another field of a `text_flow`. The web, macOS, Linux and Windows; touch screens report none.

| | Selection rectangle | Copy, cut | Paste | Modifier-click |
|---|---|---|---|---|
| Web | the selection's client rectangles | the `copy` / `cut` event: plain text at once, the app's content then through the async Clipboard API (custom types as `web …` formats where the browser has them; Safari may refuse the late write and keep the plain text) | the `paste` event's `clipboardData` (text, HTML, every text type, file types) | ⌘, Ctrl or ⌥ |
| macOS | the layout manager's glyph box, the caret's rectangle | `copy:` / `cut:`: `NSPasteboard`, every type (Markdown as `net.daringfireball.markdown`) | `paste:`: text, HTML and every type's text from the general pasteboard | ⌘ or ⌥ (⌃-click stays the context menu) |
| iOS, iPadOS, visionOS | `caretRect` and `selectionRects` | `copy:` / `cut:`: `UIPasteboard`, every type | `paste:`: text, HTML, types from `UIPasteboard` | none (touch) |
| Linux | the text view's iter locations | `copy-clipboard` / `cut-clipboard`: every type | `paste-clipboard`: text, HTML and the text of the clipboard's types | Ctrl or Alt |
| Windows | RichEdit's `ITextRange::GetRect` | `CopyingToClipboard` / `CuttingToClipboard`: every type (`HTML Format`, MIME-named formats) | `Paste`: text, HTML, the clipboard's types and their text | Ctrl or Alt |
| Android | the `Layout`'s line bounds | the field's copy / cut actions: plain text and HTML, the other types listed in the clip's description with their text in its extras | the paste action: text, HTML, the app's own types and their text (another app's `text/…` item as its text) | none (touch) |
| tvOS, watchOS | none | none: no clipboard | none | none |

### One selection across several fields

A document made of blocks (one `rich_input` or `text_area` per paragraph) can be selected like one text (#231). `text_flow()` (a column; or any container with `.selection_group()`) groups the text fields inside it, in the order they appear, into one selection; name each with `.id(…)`, the name the app hears:

```rust
text_flow()
    .children(self.blocks.iter().map(|b| rich_input(b.spans()).id(b.id.clone())))
    .on_selection_across(cx.listener(|this, e: &SelectionAcross, _| this.range = e.clone()))
    .on_edit_across(cx.listener(|this, e: &EditAcross, cx| {
        // Keep the first field's start and the last one's end, drop the
        // blocks in between, insert `e.text` (a line break: split there).
        this.replace(&e.selection, &e.text, e.cause, cx);
    }))
```

- **Extending**: ⇧↓ or ⇧→ at the end of a field, ⇧↑ or ⇧← at its start (on its first or last line, wrapping included) carry the selection into the next or the previous field. Then ⇧→ and ⇧← move it a character at a time (the step between two fields counts as one, like a line break), ⇧↓ and ⇧↑ a field at a time (to the end of the next one, the start of the previous one). A mouse drag from one field into another does it too (#264; a pen on the web and Android, an iPad's pointer): natively the backend reports where the drag is (`UiEvent::SelectionAcross`) and the runtime draws the selection and the highlights; back over the first field, the drag selects there again.
- **Drawing**: the field where it started keeps its own, focused selection; the other fields draw their part highlighted, the fields in between whole.
- **`on_selection_across`** hears every change: `SelectionAcross { anchor, head, fields }`, each end a `SelectionPoint { field, offset }`, `fields` every field's part (`field`, `start`, `end`, `text`) in order, `text()` all of it joined by line breaks, `start()` / `end()` the ends in document order. It is called once more when the selection comes back inside one field (`is_across()` false).
- **Ending it**: an arrow without ⇧ puts the caret at that end of the range, Escape at the moving end; a click or a new selection in a field ends it there.
- **Copy and cut**: ⌘C / ⌘X (Ctrl elsewhere) go to the `on_copy` / `on_cut` of the field where it started, with `CopyEvent::across` set and `text` the whole range's, so the app writes its own content for all the blocks. Without a handler, the fields' texts joined by line breaks are copied.
- **Editing**: typing (a hardware keyboard, the on-screen keyboard, an input method), Backspace, Delete, Return, a cut or a paste over the range is applied by no field: `on_edit_across` hears `EditAcross { selection, text, cause }` (`text` the typed or pasted text, `"\n"` for Return, empty otherwise) and the app rewrites its blocks, then places the caret (`cx.set_selection_across(("p2", 4), ("p2", 4))` or `.selection(…)` and `cx.focus(…)`). Without `on_edit_across`, such a key first brings the selection back into the field where it started, which then acts on its own part.
- **From the app**: `cx.set_selection_across(("p1", 4), ("p3", 2))` selects across fields (the anchor's field takes the focus), `cx.clear_selection_across()` ends it.

| | Extends with | Draws the other fields' parts | Typing over it |
|---|---|---|---|
| Web | ⇧+arrows, mouse and pen drags, in the page (the server hears the result) | rich inputs: the CSS Custom Highlight API; a text area or input only when it is covered whole | in the page |
| macOS | ⇧+arrows; mouse drags starting in a text area or rich input (a plain press; Prism tracks the drag instead of AppKit, with the scroll views around following it; ⇧-, ⌘- and double-clicks keep AppKit's own behaviour) | text areas and rich inputs: a temporary attribute of the layout manager in the selection colour; one-line inputs: a box blended over their text | the key-down monitor |
| iOS, iPadOS, visionOS | ⇧+arrows on a hardware keyboard (key commands: arrows, Escape, Return, Backspace, ⌘C, ⌘X, ⌘V); an iPad pointer's drags (a pan recognizer on the group, for the pointer only) | tinted boxes from `selectionRects` (behind the text of text areas and rich inputs, over one-line inputs') | `textView:shouldChangeTextInRange:` / `textField:shouldChangeCharactersInRange:`: hardware and on-screen keyboards |
| Linux | ⇧+arrows; mouse drags starting in a text area or rich input (a capture-phase drag gesture takes over from the view's) | text areas and rich inputs: a text tag; one-line inputs: a Pango background attribute | the window's key controller |
| Android | ⇧+arrows on a hardware keyboard (the Java host asks the runtime as the key is pressed); mouse and stylus drags starting in any text field | a drawable in the field's overlay, from its own layout | hardware keys; an input filter for the on-screen keyboard and input methods |
| Windows | ⇧+arrows; mouse drags starting in any text field (heard with `handledEventsToo`: the field keeps its own selection until the drag reaches another field) | a canvas over the field, from its own geometry (`GetRectFromCharacterIndex`, RichEdit's `GetRect`), in the accent color | the window's key handling |
| tvOS, watchOS | none | none | none |

Touch selection handles stay inside one field on every platform: the system draws and drives them for one text view (UIKit's text interaction, Android's editor, the browser's) and cannot hand them to another. On a touch screen, extend from the app instead: the counter's Blocks demo has a "Select all blocks" button (`cx.set_selection_across` from the first block's start to the last one's end); typing, the on-screen keyboard's delete key and copy then act on the whole range.

## Rich text editor

`ui::rich_text_input(doc)` is a ready editor for formatted text (#294): paragraphs, headings (levels 1 to 3), bulleted and numbered lists, and runs in bold, italic or linked, written without seeing markup. It is built from the pieces above: one `rich_input` per block in a `text_flow`, so it edits natively wherever `rich_input` does and the selection runs across blocks. It is an entity: create it once, keep it, and read or command it from your toolbar.

```rust
use prism::ui::{self, BlockKind, LinkRequest, RichText, RichTextChange, RichTextInput};

struct Note { body: Entity<RichTextInput>, markdown: String }

// Once, when the screen is built (`cx` is the screen's `Context`).
let on_change = cx.listener(|this: &mut Note, e: &RichTextChange, _| {
    this.markdown = e.value.to_markdown()
});
let on_link = cx.listener(|this: &mut Note, e: &LinkRequest, cx| {
    // Open your link picker with `e.href` and `e.text`; then answer:
    this.body.update(cx, |ed, cx| ed.set_link("https://prismsdk.dev", cx));
});
let body = cx.new(|_| {
    ui::rich_text_input(RichText::from_markdown(&saved))
        .label("Body")
        .placeholder("Write something…")
        .on_change(on_change)
        .on_link_request(on_link)
});
cx.observe(&body, |_, _, _| {}); // re-render the toolbar when the selection moves

// In `render`: a toolbar showing and changing the selection's formatting.
let f = self.body.read(cx).formatting();
let pressed = |on| if on { ui::Variant::Primary } else { ui::Variant::Ghost };
let body = self.body;
div()
    .child(ui::button("B").variant(pressed(f.bold))
        .on_click(cx.listener(move |_, _, cx| body.update(cx, |ed, cx| ed.toggle_bold(cx)))))
    .child(ui::button("H2").variant(pressed(f.block == BlockKind::Heading(2)))
        .on_click(cx.listener(move |_, _, cx| body.update(cx, |ed, cx| ed.toggle_block(BlockKind::Heading(2), cx)))))
    .child(body)
```

- **The value**: `RichText { blocks }`, each `RichBlock { kind, spans }` with `BlockKind::Paragraph`, `Heading(level)`, `Bullet` or `Numbered`, its spans the [rich text](typography.md#rich-text) runs (`is_bold()`, `is_italic()`, `href()`). `value()` reads it, `set_value(doc, cx)` replaces it, `on_change` hears each edit once the event is handled (so the handler may update the entity that holds the editor), `revision()` counts them.
- **Markdown and HTML**: `to_markdown()` / `from_markdown(&str)` write and read CommonMark (headings, `-` and `1.` lists, `**`, `*`, `[text](url)`, backslash escapes; other syntax is read as its text: a quote as a paragraph, an image as its alt text, code as plain text). `to_html()` / `from_html(&str)` do the same with HTML, reading pasted HTML through a sanitizer: only the model's subset is kept (scripts, styles and `javascript:` links never are), Google Docs' bold wrappers included.
- **Modes**: block by default; `.inline()` keeps one line (a title, a caption): bold, italic and links, Enter does nothing, line breaks typed or pasted become spaces.
- **Typing**: Enter splits the block (an empty list item leaves the list, a heading is followed by a paragraph), Backspace at a block's start turns a list item into a paragraph, then joins it to the block before; Delete at its end joins the next one; ← → ↑ ↓ at a block's edges move into the next one. `- `, `* `, `1. `, `# `, `## ` and `### ` typed at a paragraph's start make it a list item or a heading (`.markdown_shortcuts(false)` turns that off).
- **Commands**: `toggle_bold`, `toggle_italic` (on a caret: what is typed next), `set_block(kind)` and `toggle_block(kind)` for the selected blocks, `request_link`, `set_link(href)` (a caret in a link changes its target; a caret elsewhere inserts `href` as a linked text), `remove_link`, `select(RichSelection)`, `focus()`. ⌘B, ⌘I and ⌘K (Ctrl elsewhere) do the first three: the editor declares them with `consume_keys(&["cmd+b", "cmd+i", "cmd+k"])`, so the browser's or the platform's own shortcuts never run.
- **Links**: ⌘K or `request_link` calls `on_link_request` with `LinkRequest { href, text, selection }` (`href` is the link the caret or the selection is in). Open your picker; answer with `set_link` or `remove_link`, at once or later: the request's selection is kept, so the picker may take the focus meanwhile. Without `on_link_request`, ⌘K does nothing. Links are drawn in the theme's accent, or in `.link_color(color)`; the document keeps no color for them.
- **A toolbar's state**: `formatting()` gives `Formatting { bold, italic, link, block }` for the selection (bold when all of it is), with the caret's pending marks; `in_link()`. `selection()` gives the `RichSelection { anchor, head }` in `RichPosition { block, offset }`, `is_focused()` whether a block has the focus.
- **Clipboard**: a copy writes plain text, HTML, Markdown and the editor's own type (`RICH_TEXT_TYPE`), so a paste into another editor keeps the blocks exactly. A paste reads the richest type there is (the editor's own, then HTML, then Markdown, then text: lines become blocks); `.paste(PasteMode::Plain)` keeps only the text, in the style of where it lands.
- **Looks**: the frame of `ui::text_area` (surface, border, the accent around the focused editor); `.style(|s| …)` refines it (`.style(|s| s.min_h(80.))` for a multi-line field).
- **Clicks anywhere**: as in a `<textarea>`, every point of the editor focuses it (#322). Its inset and the space between blocks are the blocks' own padding, and the last block takes the height the text leaves, so a click below the text puts the caret at its end and a click beside a line at that line's nearest point; list markers let clicks through to their item. Headings are drawn 1.6, 1.35 and 1.15 times the theme's text size, list markers beside their items (hidden from screen readers).
- **Accessibility**: each block is a text field (`role=textbox`, `aria-multiline` on the web) labelled with the editor's label and, for headings and items, the block's kind (« Heading 2 », « List item », « Item 3 », in Prism's 12 languages); the editor is a group with its label.
- **On-screen keyboards**: Backspace with the caret at a block's start deletes nothing, so iOS and Android report no key for it; their text areas report it as a `Backspace` key there, and the editor joins the blocks as with a hardware keyboard.

**Sheets, popovers, dialogs** that a screen opens without a route: see [Dialogs, menus, tray](dialogs-menus.md#sheets-popovers-and-dialogs-without-a-route).

**Absolute positioning**: `div().absolute().bottom(16.).left(16.).right(16.)` takes an element out of the flex flow and places it at its insets from the parent (`.top()`, `.inset(0.)`…), like CSS `position: absolute` (taffy on native). Later siblings draw on top; `left`/`right` are the leading and trailing edges.
