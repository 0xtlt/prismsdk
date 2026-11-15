# Accessibility

Accessibility is built in, not opt-in. The core resolves one description per node (role, label, hint, value, checked, disabled, hidden) and each backend maps it to the platform API:

| | Web | iOS | macOS |
|---|---|---|---|
| API | ARIA + semantic HTML (`<main>`, `<h1>`, `<button>`, `role=switch`, `tablist`) | `UIAccessibility` traits, labels, values | `NSAccessibility` roles, labels, press action |
| Keyboard | Tab/Enter/Space on custom buttons, arrow keys in tab bars, visible focus | — | Full Keyboard Access, Space/Return, focus ring |
| Navigation | focus moves to the new screen's title; document title follows | native | window title follows |

Defaults are automatic: a clickable `div` becomes a button named after its text ("Try Prism on the web, Done"), inputs use their placeholder, toggles expose their state. Refine with:

```rust
text("Settings").heading(1)
div().on_click(..).accessibility_label("Open profile").accessibility_hint("Shows your account")
text("›").accessibility_hidden()
```
