# tvOS

`prism run tvos` (simulator) / `prism build tvos`: the UIKit backend, adapted to the Siri Remote.

- Clickable views take the focus (raised with a shadow) and react to the select press; buttons use the primary action.
- Toggles are check-mark buttons (`UISwitch` does not exist on tvOS).
- The whole UI is zoomed by the system's body text size ÷ 17, so phone-sized styles read from a sofa. The app sees the viewport in Prism points (1920 ÷ zoom), so breakpoints keep working. `PRISM_ZOOM` overrides it.
- Not on tvOS (the SDK marks them unavailable): notifications with text, widgets, Live Activities, motion, photo/camera pickers, haptics, clipboard, share sheet, pull to refresh, [web views](../guide/web-views.md) (no WebKit: an empty box). Location gives one position at a time. `cx.capabilities()` reports all of this, and `.only(Platform::TV)` / `Platform::Tvos` adapt the UI.
- Rich inputs are plain one-line fields: `UITextView`'s `editable` is `API_UNAVAILABLE(tvos)`, so text is typed in the system's full-screen keyboard, which edits plain text. Edits keep the styles around them ([Rich text input](../guide/forms.md#rich-text-input)).
- The minimum version is tvOS 17 (`TVOS_DEPLOYMENT_TARGET`, or `[tvos] deployment_target` in `Prism.toml`).
- Dev hooks on iOS and tvOS: `PRISM_CLICK="+1|Save"` presses by title or label, `PRISM_SNAPSHOT=/abs/out.png` saves the window and quits.
