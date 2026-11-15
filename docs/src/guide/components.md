# Components (`prism::ui`)

A themed, accessible component library: `button`, `switch`, `checkbox`, `text_field`, `card`, `heading`, `badge`, `list_row`, `divider`, and charts (see [Charts](charts.md)).

Three levels of customization:

1. **Theme**: `ui::set_theme(cx, Theme::dark().accent(rgb(0x0A84FF)).radius(0.))`. It also switches the native chrome (bars, native controls) to dark.
2. **Per instance**: `ui::button("Delete").danger().large()`, plus `.style(|s| s.corner_radius = 0.)` for anything else.
3. **Replace a component app-wide**: `Theme::light().override_component(|b: ui::Button, cx| my_button(b, cx))`, or implement `ui::Component` for your own and register it with `prism_ui::component!(MyThing)`.

Custom or native: `ui::button("Save").native()` / `ui::switch(..).native()` per instance, or `Theme::prefer_native(true)` everywhere (UIButton/UISwitch, NSButton/NSSwitch, `<button>`/`<input role=switch>`). Raw native primitives remain available: `button()`, `toggle()`, `text_input()`.
