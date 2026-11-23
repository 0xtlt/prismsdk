# Accessibility

Accessibility is built in, not opt-in. The core resolves one description per node (role, label, hint, value, checked, selected, current, disabled, hidden) and each backend maps it to the platform API:

| | Web | iOS | macOS | Windows | Android | Linux |
|---|---|---|---|---|---|---|
| API | ARIA + semantic HTML (`<main>`, `<h1>`, `<button>`, `role=switch`, `tablist`) | `UIAccessibility` traits, labels, values | `NSAccessibility` roles, labels, press action | UI Automation: WinUI's peers for its controls, custom automation peers for drawn views (control type, Toggle, SelectionItem, Value and ExpandCollapse patterns, landmarks, dialogs) | native views (`Button`, `Switch`…), `contentDescription`, `setAccessibilityHeading` | GTK accessible roles, labels and states |
| Keyboard | Tab/Enter/Space on custom buttons, arrow keys in tab bars and radio groups, visible focus | — | Full Keyboard Access, Space/Return, focus ring | Tab, Space/Enter on clickable views, focus visuals | native controls | Tab, Space/Enter on clickable views |
| Navigation | focus moves to the new screen's title; document title follows | native | window title follows | window title follows | activity title follows (what TalkBack reads) | window title follows |

Defaults are automatic: a clickable `div` becomes a button named after its text ("Try Prism on the web, Done"), inputs use their placeholder, toggles expose their state. Refine with:

```rust
text("Settings").heading(1)
div().on_click(..).accessibility_label("Open profile").accessibility_hint("Shows your account")
text("›").accessibility_hidden()
```

## Drawn choices

When the platform's control does not fit (option cards with a title, a description and a dot, a segmented look), draw the choices yourself and say what they are: `Role::Radio` on each choice with `accessibility_checked`, `Role::RadioGroup` (and a label) on their container. Screen readers then say « radio button, checked » instead of « button », and give the position in the group where the platform does (« 2 of 3 » in browsers).

```rust
div()
    .accessibility_role(Role::RadioGroup)
    .accessibility_label("Plan")
    .children(PLANS.iter().enumerate().map(|(i, plan)| {
        div()
            .child(text(plan.title))
            .child(text(plan.note))
            .accessibility_role(Role::Radio)
            .accessibility_checked(self.plan == i)
            .on_click(cx.listener(move |this, _, _| this.plan = i))
    }))
```

| | |
|---|---|
| Web | `role=radio` with `aria-checked` inside `role=radiogroup`; one tab stop per group (the checked choice, else the first), the arrow keys move the focus and click the choice they land on (left and right follow the reading direction), Space and Enter click it |
| iOS, iPadOS, tvOS, visionOS | a button with the selected trait when checked; the group is a semantic group, its label read when the focus enters it |
| watchOS | a button, selected when checked |
| macOS | `NSAccessibilityRadioButtonRole` with its value, in an `NSAccessibilityRadioGroupRole` |
| Android | `RadioButton` (checkable, checked) in a `RadioGroup` for TalkBack |
| Linux | `GTK_ACCESSIBLE_ROLE_RADIO` with the checked state, in a `RADIO_GROUP` |
| Windows | `RadioButton` with the SelectionItem pattern (selected when checked), in a `Group` |

The same works for `Role::Checkbox` and `Role::Switch` (`accessibility_checked` for their state). Prefer [`radio_group`](forms.md) when its look fits: it is the platform's own control.

## Tables

`table()` gives screen readers a table, rows, column headers and cells on every platform that has them: a reader says a cell with its column's header and moves by row and column. See [Tables](tables.md#screen-readers) for what each platform gets.

## Selected and current items

A navigation highlights where the user is; say it to screen readers too. `accessibility_current(true)` marks the destination being shown (a sidebar entry, a link of a tab bar, the last crumb), `accessibility_selected(bool)` the chosen item among several (a tab, an option, a row):

```rust
div()
    .link("/settings")
    .accessibility_current(page == Page::Settings)
    .child(text("Settings"))
```

| | Web | iOS, tvOS, visionOS | watchOS | macOS | Android | Linux | Windows |
|---|---|---|---|---|---|---|---|
| `current` | `aria-current="page"` on a link, `"true"` otherwise | selected trait | `.isSelected` | `accessibilitySelected` | `View.setSelected` | `GTK_ACCESSIBLE_STATE_SELECTED` | SelectionItem's `IsSelected` |
| `selected` | `aria-selected` | selected trait | `.isSelected` | `accessibilitySelected` | `View.setSelected` | `GTK_ACCESSIBLE_STATE_SELECTED` | SelectionItem's `IsSelected` |

Only the web tells « current page » and « selected » apart; the native platforms have one selected state for both. A checked tab, radio or toggle button also reads as selected (`Accessibility::is_selected`).

## Disclosures

A row that folds its options, a « Show more », a button opening a panel: say whether its content is shown with `accessibility_expanded(bool)`, and which element it shows with `accessibility_controls(name)`, the name given to that element with `.id(name)`:

```rust
div()
    .child(text("Options"))
    .on_click(cx.listener(|this, _, _| this.open = !this.open))
    .accessibility_expanded(self.open)
    .accessibility_controls("options")
// …
self.open.then(|| div().id("options").children(options))
```

Screen readers then say « Options, button, collapsed », and « expanded » once it is open.

| | `expanded` | `controls` |
|---|---|---|
| Web | `aria-expanded` | `aria-controls` |
| iOS, iPadOS, tvOS, visionOS | the expanded status (iOS, iPadOS and tvOS 18, visionOS 2 and later); before, « expanded » or « collapsed » before its value | — |
| watchOS | « expanded » or « collapsed » before its value | — |
| macOS | `accessibilityExpanded`; a drawn view is a disclosure triangle | its linked element |
| Android | the state description (Android 11 and later), the expand or collapse action | — |
| Linux | `GTK_ACCESSIBLE_STATE_EXPANDED` | `GTK_ACCESSIBLE_RELATION_CONTROLS` |
| Windows | the ExpandCollapse pattern | its controlled peer |

The words « expanded » and « collapsed » come in Prism's languages; override them with the `prism.expanded` and `prism.collapsed` keys of your [translations](i18n.md).

## Tooltips

`.tooltip(text)` gives any element the platform's own tooltip, with its own delay (#216). `.tooltip_when_truncated()` on a text shows its full content only while it is cut short (`truncate()`, `line_clamp(n)`): the full title of a tab or a sidebar row, without measuring it.

```rust
icon_button("xmark").tooltip("Close tab")
text(tab.title.clone()).truncate().tooltip_when_truncated()
```

| | Tooltip | Truncated text |
|---|---|---|
| macOS | `NSView.toolTip` | the label answers `view:stringForToolTip:…` as the pointer rests, with its text only when it overflows its box or its line clamp |
| iPadOS, visionOS | `UIToolTipInteraction` (pointer) | none |
| Web | the `title` attribute | `title` set as the pointer comes over it when `scrollWidth` / `scrollHeight` exceed the box |
| Linux (GTK) | `gtk_widget_set_tooltip_text` | `query-tooltip`, while the label's layout is ellipsized |
| Android | `setTooltipText` (a long press, or a mouse hover) | set after each layout while the last line is ellipsized |
| Windows | `ToolTipService.ToolTip` | set while the `TextBlock` is trimmed (`IsTextTrimmed`, followed through `IsTextTrimmedChanged`) |

Screen readers read a tooltip as a description where the platform does (the web's `title`). Touch screens without a pointer show none.

## High contrast

Contrast themes replace an app's colors with a few system colors chosen by
the user. On the web the browser does it (`forced-colors`); on Windows,
under a contrast theme (Settings › Accessibility › Contrast themes), Prism
does it for the views it draws, as WinUI does for its controls: fills
become the window's color (the button face for a clickable view); colored
strong fills (an accent, a badge) and selected items the highlight, gray
ones (a knob) the text's color; text the window text (the highlight text
on a highlight, the link color in a link, the gray text when disabled),
borders the text's color; shadows go (a card keeps an outline), pictures
keep their colors. Nothing to do in the app; give state to
assistive technologies (`accessibility_selected`, `accessibility_disabled`)
and it shows in the theme too. `PRISM_HIGH_CONTRAST=night-sky` (or
`aquatic`, `desert`, `dusk`) tries a theme on one app without changing the
PC's.
