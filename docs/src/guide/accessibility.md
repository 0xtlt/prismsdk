# Accessibility

Accessibility is built in, not opt-in. The core resolves one description per node (role, label, hint, value, checked, disabled, hidden) and each backend maps it to the platform API:

| | Web | iOS | macOS |
|---|---|---|---|
| API | ARIA + semantic HTML (`<main>`, `<h1>`, `<button>`, `role=switch`, `tablist`) | `UIAccessibility` traits, labels, values | `NSAccessibility` roles, labels, press action |
| Keyboard | Tab/Enter/Space on custom buttons, arrow keys in tab bars and radio groups, visible focus | — | Full Keyboard Access, Space/Return, focus ring |
| Navigation | focus moves to the new screen's title; document title follows | native | window title follows |

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

The same works for `Role::Checkbox` and `Role::Switch` (`accessibility_checked` for their state). Prefer [`radio_group`](forms.md) when its look fits: it is the platform's own control.
