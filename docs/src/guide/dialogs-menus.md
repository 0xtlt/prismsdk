# Dialogs, menus, tray

```rust
cx.show_alert(Alert::new("Delete “Milk”?").message("This cannot be undone.")
    .destructive("Delete", on_delete).cancel("Cancel"));          // UIAlertController, NSAlert, <dialog>
cx.present("/todos/new");                                          // sheet: page sheet, NSWindow sheet, <dialog>
div().context_menu(Menu::new().item(MenuItem::new("Open").icon("arrow.up.forward.square")))
cx.set_tray(Tray::new("focus").title("25:00").menu(menu));         // macOS menu bar (NSStatusItem)
cx.set_app_menu(vec![Menu::titled("File").item(MenuItem::new("New").shortcut("cmd+n"))]);
```

Context menus open with a long press (iOS), right click or ⌃-click (macOS), right click / Shift+F10 (web), and from VoiceOver.
