# Widgets, Live Activities, Dynamic Island, notifications

System surfaces are described with the **same elements as the UI**, plus `image("sf.symbol")`, `progress(0.4)`, `timer(until)` (counts down by itself, even when the app is not running) and `div().link("/path")` (deep link everywhere):

```rust
// A home-screen widget, re-rendered whenever `store` changes.
cx.register_widget(
    Widget::new("todos", "Todos")
        .families(&[WidgetFamily::Small, WidgetFamily::Medium])
        .link("/")
        .render(move |cx, family| text(format!("{} open", store.read(cx).open()))),
);
cx.reload_widgets_on(&store);

// A Live Activity: lock screen + Dynamic Island (compact, minimal, expanded).
let id = cx.start_activity(
    Activity::new("focus")
        .summary("Focus session, 25 minutes")
        .compact(image("brain.head.profile"), timer(ends))
        .minimal(image("timer"))
        .lock_screen(div().flex_row().child(text("Focus")).child(timer(ends)))
        .link("/focus"),
);
cx.update_activity(id, …);  cx.end_activity(id, None, true);

// A local notification that opens a screen when tapped, with action buttons.
cx.send_notification(
    Notification::new("Done").body("Take a break").link("/focus").after(total)
        .action(NotificationAction::new("again", "5 more minutes").link("/focus"))
        .action(NotificationAction::new("note", "Add a todo…").reply("What's next?")),
);
cx.on_notification_response(|r, app| match r.action.as_deref() { Some("note") => add(app, r.text.clone()), _ => {} });
```

| | iOS | macOS | Web | Android (planned) |
|---|---|---|---|---|
| Widgets | WidgetKit | WidgetKit (`mise run macos-app`) | — | Glance |
| Live Activities | lock screen + **Dynamic Island** | menu bar item with live countdown | in-page live banner (`aria-live`) | Live Updates notification |
| Notifications | UserNotifications | UserNotifications (bundled app) | Notification API / toast | NotificationCompat |

How it works: Prism resolves the elements into a static snapshot (compact JSON, under ActivityKit's 4 KB limit). A **generic SwiftUI renderer** shipped with Prism (`crates/prism-apple-build/apple/Extension`) draws it in a WidgetKit extension. `crates/prism-cli/platform/scripts/ios-run.sh` generates that extension from the app's registered widgets, then builds and signs it with the App Group. The Rust app talks to WidgetKit/ActivityKit/UserNotifications through a small Swift bridge linked automatically (`crates/prism-apple-build`). Platforms implement `PlatformServices`; apps query `cx.capabilities()` instead of checking the OS.

**Light & dark**: colors can be adaptive, `adaptive(rgb(0xFFFFFF), rgb(0x1C1C22))`, resolved natively everywhere: CSS `light-dark()`, dynamic `UIColor`/`NSColor`, SwiftUI color scheme in widgets and Live Activities. `Theme::system()` makes the whole component library follow the system appearance live.
