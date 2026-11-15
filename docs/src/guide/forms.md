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
text_input().secure(true).return_key(ReturnKey::Go)
    .on_submit(cx.listener(|this, _, cx| this.sign_in(cx)))
text_input().keyboard(Keyboard::Email).capitalize(Capitalize::None).autocorrect(false)
text_area().value(&self.notes).max_h(px(160.))                // grows, then scrolls
spinner().large()

// Themed (label, value, accent), with `.native()` for the bare control
ui::slider("Volume", v).range(0., 100.).step(1.).format(|v| format!("{v:.0} %"))
ui::select("Size", sizes, self.size)
ui::date_field("Due", self.due)
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
text_input().leading_icon("magnifyingglass").max_length(24)
    .suggestions(["Paris", "Lyon", "Marseille"])
ui::file_input("Photo").accept(["image/*"])
    .on_pick(|files, cx| { let bytes = files[0].read(); /* … */ })
let files = cx.pick_files(PickFiles::new().types([".csv"]).multiple()).await?;   // the same, from a task
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
- **Text inputs**: `secure` (password entry), `keyboard` (email, number, decimal, phone, URL), `return_key` (Done, Go, Next, Search, Send), `capitalize`, `autocorrect`, and `on_submit` for the return key. Each maps to the platform's own setting where it has one (no keyboard types on macOS, Linux or the watch (number has no watch equivalent); no return-key label on macOS or Linux; tvOS has no multi-line editing, so text areas are one line there).
- **Accessibility**: sliders are adjustable (VoiceOver swipe up/down, TalkBack, arrow keys) and read their value (`accessibility_value("40 %")` overrides it); pickers read their label and choice; spinners are busy indicators. Toasts are announced politely (`Role::Status`: ARIA live region, VoiceOver and TalkBack announcements, `gtk_accessible_announce`).
- **Checkbox and radio group**: the platform's own controls where it has them (`NSButton` checkbox and radio buttons, `GtkCheckButton` and a group, `CheckBox` and `RadioGroup`, `<input type=checkbox / radio>`, a SwiftUI `Toggle` and a list of buttons on the watch). UIKit has neither: iOS, iPadOS, tvOS and visionOS draw them (SF Symbols) with the toggle-button / selected traits, so VoiceOver reads « checkbox, checked » and « 2 of 3 ». A radio group is one element for the app (`e.index()`), moved through with the arrow keys where the platform does so.
- **Time of day**: `time_picker(minutes)` edits a time that means the same in every time zone (an alarm, opening hours); backends show it in UTC, so « 07:30 » stays 07:30 when the device travels. `date_picker(…).time()` still edits an instant.
- **Text input extras**: `max_length` (`maxlength`, a length filter natively), `leading_icon` (an SF Symbol inside the field: `leftView`, a GTK primary icon, a compound drawable, a glyph on the web), `suggestions` (web `<datalist>`, AppKit's completion list, `AutoCompleteTextView`, `GtkEntryCompletion` on Linux, a bar of matching suggestions above the keyboard on iOS, which has no API for app-supplied completions; none on tvOS, visionOS and watchOS), `autofocus` and `.id` with `cx.focus` (see [Pointer, keyboard, focus](input.md)).
- **Files**: `ui::file_input` (or `cx.pick_files`) opens `<input type=file>`, `UIDocumentPickerViewController`, `NSOpenPanel`, `GtkFileChooserNative` (the desktop portal) or `ACTION_OPEN_DOCUMENT`; `PickedFile::read()` returns the bytes. On the web the browser uploads them over the app's connection and `read()` reads the server's copy. There is no file picker on tvOS and watchOS (`DeviceError::Unavailable`).
- **Toasts**: `ui::show_toast` works from any handler or task: the runtime mounts the toaster as a layer above the router, tabs, sheets and dialogs, so a toast shown right before `cx.back()` stays. `ui::toaster()` is deprecated and does nothing (remove it).
- **Right to left**: sliders fill from the right, menus and calendars follow the language.
- **Tint**: `text_color` tints a control (`ui::*` uses the theme's accent).

- **Platform notes**: tvOS has no `UISlider`, `UIDatePicker` or editable `UITextView` (`API_UNAVAILABLE(tvos)`), hence the focus-driven stand-ins. watchOS sliders always step (a continuous one moves by a tenth of its range), and dates open the system wheels in a sheet. `GtkCalendar` has no min/max, so out-of-range days snap back. Before GTK 4.14, Linux has no `gtk_accessible_announce`: the status text changes but may not be spoken. Android announces through a polite live region (`announceForAccessibility` is deprecated in API 36), and the date dialog's header follows the device language, not the app's.
- **Dev hooks for forms**: macOS `PRISM_TAP=<label>`, `PRISM_TYPE=<text>` (`\n` is Return), `PRISM_INCREMENT=<label>` (VoiceOver increment), `PRISM_A11Y_LOG=1`; Linux `PRISM_CLICK` opens pickers and date pickers, `fill:<placeholder>=<text>`, `fill:@slider=40`, `submit:<placeholder>`, open popovers saved as `out-N.png`; watchOS `PRISM_SCROLL=<pt>`, `PRISM_PRESENT=<label>` (opens a date picker), `PRISM_DEBUG_A11Y=1`. The watch's `ImageRenderer` snapshot can't draw native controls: use `xcrun simctl io <udid> screenshot`.

**Sheets, popovers, dialogs** that a screen opens without a route: see [Dialogs, menus, tray](dialogs-menus.md#sheets-popovers-and-dialogs-without-a-route).

**Absolute positioning**: `div().absolute().bottom(16.).left(16.).right(16.)` takes an element out of the flex flow and places it at its insets from the parent (`.top()`, `.inset(0.)`…), like CSS `position: absolute` (taffy on native). Later siblings draw on top; `left`/`right` are the leading and trailing edges.
