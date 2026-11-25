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
// A `.bg(..)` on the widget's root fills the whole widget (iOS and macOS:
// its container background), not just the area inside the system's margins.
// Sizes work as in the app: a `nowrap` text hugs its content (cut where its
// parent ends) and `min_w` / `min_h` keep a pill at least that size (#321;
// on Android, a box's `min_h` applies to texts only).
cx.reload_widgets_on(&store);

// A Live Activity: lock screen + Dynamic Island (compact, minimal, expanded).
// Declared at startup, so the iOS bundle gets the extension that draws it.
cx.enable_live_activities();
let id = cx.start_activity(
    Activity::new("focus")
        .summary("Focus session, 25 minutes")
        .compact(image("brain.head.profile"), timer(ends))
        .minimal(image("timer"))
        .lock_screen(div().flex_row().child(text("Focus")).child(timer(ends)))
        .link("/focus"),
);
cx.update_activity(id, …);  cx.end_activity(id, None, true);
// Activities started by a push (`prism_push::Push::start_live_activity`) or by
// an earlier run are picked up under their id too: see Remote push.

// A local notification that opens a screen when tapped, with action buttons.
cx.send_notification(
    Notification::new("Done").body("Take a break").link("/focus").after(total)
        .action(NotificationAction::new("again", "5 more minutes").link("/focus"))
        .action(NotificationAction::new("note", "Add a todo…").reply("What's next?")),
);
cx.on_notification_response(|r, app| match r.action.as_deref() { Some("note") => add(app, r.text.clone()), _ => {} });
cx.set_badge(Some(unread));   // the app icon's number, without a notification (#239); None clears it
```

**The app badge** (#239): `cx.set_badge(Some(n))` sets the number on the app icon on its own, when the app's unread count changes (`None` or 0 clears it): the Dock tile on macOS, the home screen on iOS, iPadOS, tvOS and visionOS (with the notification permission, badges included), the Badging API on the web where the browser has it (installed web apps), an overlay on the taskbar button on Windows. Android launchers and Linux docks badge from notifications only, and watchOS has none: nothing happens there. **Outside an app bundle** (`cargo run` on macOS), notification calls do nothing and the notification permission answers `Unavailable` instead of throwing.

| | iOS | macOS | Web | Android | Linux | Windows |
|---|---|---|---|---|---|---|
| Widgets | WidgetKit | WidgetKit (`mise run macos-app`) | — | app widgets (`RemoteViews`) | — | — |
| Live Activities | lock screen + **Dynamic Island** | menu bar item with live countdown | in-page live banner (`aria-live`) | ongoing notification | — | — |
| Notifications | UserNotifications | UserNotifications (bundled app) | Notification API / toast | `Notification.Builder` | `GNotification` | Windows App SDK app notifications |

How it works: Prism resolves the elements into a static snapshot (compact JSON, under ActivityKit's 4 KB limit). A **generic SwiftUI renderer** shipped with Prism (`crates/prism-apple-build/apple/Extension`) draws it in a WidgetKit extension. `crates/prism-cli/platform/scripts/ios-run.sh` generates that extension from the app's registered widgets, then builds and signs it with the App Group. Apps that register no widget and declare no Live Activity (`cx.enable_live_activities()` at startup, or a push-to-start handler `cx.on_activity_push_token`) are bundled without the extension, its provisioning profile and the App Group: on iOS, `start_activity` then fails with a log line saying so. macOS draws Live Activities in the app (a menu bar item), so there only widgets need the extension. Android adds one widget receiver per registered widget. The Rust app talks to WidgetKit/ActivityKit/UserNotifications through a small Swift bridge linked automatically (`crates/prism-apple-build`). Platforms implement `PlatformServices`; apps query `cx.capabilities()` instead of checking the OS.

## Drawing a widget ahead

A widget is drawn when the app reloads it, and the system keeps that drawing until the next reload. A widget whose content changes at known instants (the next bus, the next meeting) can be **drawn ahead** for them: the system switches to each drawing at its instant on its own, while the app sleeps.

```rust
cx.register_widget(
    Widget::new("next", "Next departure")
        .families(&[WidgetFamily::Small])
        // Called once per instant, with `cx.now()` returning it.
        .render(move |cx, _| {
            let now = cx.now();
            match buses.read(cx).departures.iter().find(|t| **t > now) {
                Some(t) => timer(*t).after("Departed").into_element(),
                None => text("No more buses today").into_element(),
            }
        })
        // The instants at which the drawing changes: each departure.
        .ahead(move |cx, _config| buses.read(cx).departures.clone()),
);
```

- `ahead(|cx, config| instants)` is called on every reload, once per configuration (an empty `WidgetConfig` for a widget without parameters, `render_configured` for one with). Instants already past are dropped and the 16 soonest are kept (`Widget::MAX_AHEAD`).
- The render closure is called again for each instant. **`cx.now()`** returns the instant being drawn (the reload's time for the first drawing, the clock outside widgets), so a drawing for 15:40 leaves out what has gone by then. Read it instead of `SystemTime::now()` in what a widget shows.
- Each reload draws `1 + instants` times per family and configuration: keep the render closure quick.

| | How the drawings ahead are shown |
|---|---|
| iOS, iPadOS, macOS, visionOS | one timeline entry per drawing (`Timeline(entries:policy: .atEnd)`, `.never` when nothing is ahead), so WidgetKit switches drawings by itself; after the last one, it asks again and the last drawing stays |
| Android | the latest drawing whose instant has come, each time the widget is drawn, and an inexact alarm draws it again at the next instant |
| Web, Linux, Windows, watchOS, tvOS | no home-screen widgets |

The snapshot keeps its format for widgets that don't draw ahead: the drawings come in an `"ahead": [{"at": …, "families": …}]` list next to `"families"` (per configuration for configurable widgets), only when there are some. `WidgetSnapshot::ahead_for(config)` lists them, as `WidgetEntry { at, families }`.

## A timer that has ended

`timer(end)` counts down by itself, then stops at `0:00`. `.after(text)` says what the app means once the end has passed:

```rust
timer(departure).after("Departed")
```

The text replaces the countdown when it reaches `0:00`, wherever the timer is drawn, and the timer is laid out wide enough for it:

| | Once the end has passed |
|---|---|
| In the app (web, macOS, iOS, iPadOS, tvOS, visionOS, Android, Linux, Windows, watchOS) | the text, on the next tick (watchOS: drawn again at the end) |
| Widgets on iOS and macOS | the text, without the app: the widget's timeline gets an entry at the timer's end, and the timer compares its end with the entry's instant (WidgetKit draws entries ahead of time) |
| Live Activities on iOS | the text once ActivityKit draws the activity again: an update, or its `stale_after(end)` |
| Android widgets | the text, without the app: an alarm draws the widget again at the timer's end (inexact: Doze may delay it a little) |
| Android Live Updates | the text when the notification is drawn after the end |
| macOS menu bar, the web's live banner | the text, on the next tick |

## Configurable widgets

A widget can have **parameters** the user picks per instance in the system's « Edit widget » sheet (long-press the widget), like the city of a weather widget. Several instances of one kind then show different things:

```rust
cx.register_widget(
    Widget::new("reading", "Measure")
        .families(&[WidgetFamily::Small, WidgetFamily::Medium])
        // One row of the edit sheet. The options come from app state and are
        // read again on every widget reload: the sheet lists the current ones.
        .parameter(
            WidgetParameter::new("place", "Place")
                .options(move |cx| rooms.read(cx).iter().map(|r| WidgetOption::new(r.id.to_string(), &r.name)).collect())
                .default("all"), // otherwise the first option
        )
        .parameter(WidgetParameter::new("metric", "Measure").options(|_| vec![
            WidgetOption::new("co2", "CO2"),
            WidgetOption::new("temp", "Temperature"),
        ]))
        // The instance's configuration: every parameter has a value (the
        // default where the user picked nothing, or picked an option that is gone).
        .render_configured(move |cx, family, config| {
            let place = config.get("place").unwrap_or("all");
            text(format!("{} · {}", place, config.get("metric").unwrap_or("co2")))
        }),
);
cx.reload_widgets_on(&rooms); // new rooms show up in the sheet, every configuration is redrawn
```

`render(|cx, family| …)` keeps working (every instance then looks the same); `render_configured` adds the configuration. Widgets without parameters are unchanged (a `StaticConfiguration` on Apple platforms).

**Apple (iOS 17+, iPadOS, macOS 14+, visionOS 26)**: the generated extension declares an `AppIntentConfiguration` for a kind with parameters: a `WidgetConfigurationIntent` with one `@Parameter` per parameter, each an `AppEntity` whose `EntityQuery` lists the options the app last wrote to the App Group (`suggestedEntities`, `entities(for:)`, and `defaultResult` for the default). The parameters are required, not optional: WidgetKit fills a required parameter with the query's `defaultResult()` when the widget is added, so the « Edit widget » sheet shows the default option (« Everywhere »), like system widgets do. An optional one would start empty, the sheet showing the parameter's title as a grey placeholder while the widget draws the default (#49). Before the app has written its options, the default is an empty placeholder entity (titled like the parameter), which the extension reads as « the default ». An instance whose option is gone (a room deleted) draws the default configuration too, until the user picks another option: the query still answers its id, titled like the default option, so WidgetKit keeps asking for timelines (#277). Give a parameter whose options come from app state a `.default(..)` that is always among them (« Everywhere »): without one, the default is the first option, which changes with the list, and an instance added before the app had any option keeps the empty placeholder, drawn as whatever option comes first. The bundler also runs the App Intents metadata extraction Xcode would (`appintentsmetadataprocessor`), which WidgetKit needs to show the sheet. On a simulator slimmed with `simslim`, `prism run ios` keeps `linkd` and `siriactionsd` running for such apps: without them the widget stays a grey placeholder with no « Edit widget » (re-slim a simulator slimmed before with `simslim on <udid> --except widgets --keep com.apple.linkd,com.apple.siriactionsd`).

**Which configurations are drawn**: the extension cannot run the app, so the app renders snapshots ahead of time, on every reload, at most `Widget::MAX_CONFIGURATIONS` (16) per kind, in this order:

1. the default configuration;
2. the configurations of the instances on screen: each time WidgetKit asks the extension for a timeline, it records them (the one asked for first, then `WidgetCenter`'s current configurations) in the App Group, and the app reads them back on its next reload;
3. when every combination of options fits in the limit (here 3 rooms × 2 measures = 6), all of them, so any choice in the sheet shows at once.

A choice that was not drawn yet (many options, freshly picked in the sheet) shows « Open the app to set up this widget. » rather than another configuration's content. The app redraws as soon as it becomes active again when the instances on screen changed while it was away, and on every `reload_widgets`. Options are a list in the edit sheet: keep them to what a person can scroll through.

**Android** has no edit sheet for app widgets yet (a configuration activity is not generated): configurable widgets show their default configuration. Web, Linux and Windows have no home-screen widgets.

## Widget buttons: actions without opening the app

`div().link(..)` in a widget opens the app. `widget_action(action, payload)` (on `div`, `text` and `button`) makes a **button that runs app code in place**: the home screen stays, the app's handler runs (the app is launched in the background when it is not running), then the widgets are redrawn from the new state. Typical: « Lights off », « Mark as done », « Start the coffee machine ».

```rust
cx.register_widget(
    Widget::new("shortcuts", "Shortcuts")
        .families(&[WidgetFamily::Small, WidgetFamily::Medium])
        .link("/shortcuts") // the rest of the widget still opens the app
        .render(move |cx, _| {
            div().gap(6.).children(shortcuts(cx).into_iter().map(|(id, name)| {
                div().widget_action("run-shortcut", id).child(text(name))
            }))
        }),
);

// Register it at startup: the app may be launched in the background for it.
cx.on_widget_action(move |action, cx| async move {
    // action.kind: "shortcuts" (the widget, or Live Activity, tapped),
    // action.action: "run-shortcut", action.payload: the id.
    if action.action == "run-shortcut" {
        let ok = run_shortcut(&cx, &action.payload).await; // may call the network
        cx.update_entity(&store, |s, cx| s.last_run(ok, cx));
    }
    // When the job returns, Prism reloads the widgets (no `reload_widgets` needed).
});
```

The payload is a string: an id, or JSON the handler parses. The same buttons work in **Live Activities** (lock screen, Dynamic Island), where `action.kind` is the activity's kind. In the app's own UI a `widget_action` does nothing: use `on_click` there.

| | How a tap reaches the app | Where the handler runs |
|---|---|---|
| iOS / iPadOS 17+ | `Button(intent:)` with one shared `LiveActivityIntent` (`PrismWidgetActionIntent`: kind, action, payload) | the app's process: iOS launches it in the background if needed; the intent's `perform()` waits for the job, WidgetKit then redraws |
| visionOS 26 | the same intent | the same (built, not verified on a device) |
| Android | `setOnClickPendingIntent` → a broadcast to `PrismWidget$Action` (`goAsync`) | the app's process, without an Activity when it is closed (like background jobs) |
| macOS | a link to `/__prism/widget-action?kind=…&action=…&payload=…&token=…` | **degraded**: the tap opens the app, which runs the action instead of navigating (`Runtime::open_link`). macOS widgets run intents in the widget extension, which cannot run the app's Rust code |
| Web, Linux, Windows, watchOS, tvOS | — | no home-screen widgets |

How it works on Apple platforms: `crates/prism-apple-build/apple/Shared/PrismWidgetAction.swift` declares the intent, compiled into the widget extension (the renderer turns each `wa` node of a snapshot into a `Button(intent:)`) and into the app with the Swift bridge. Because it is a `LiveActivityIntent` that the app declares as well, iOS runs `perform()` in the app: the bundler extracts the App Intents metadata of the app too (`Metadata.appintents` in the `.app`, step 6b of `ios-run.sh`) when the app calls `on_widget_action` (`PRISM_DESCRIBE=widget_actions`). In the app, `perform()` hands the action to the Rust runtime (the window's, or a headless one when iOS launched the app just for it) and returns when the job is done; the job's reload writes the new snapshots to the App Group and reloads the timelines. If `perform()` ever runs in the extension instead, it queues the action in the App Group (`prism-widgets/actions.json`) and the app runs it the next time it becomes active.

**Action links are signed and run once.** Any app or web page can open an app's URL scheme, and a link opened once can be captured (a log, another app that claimed the scheme) and opened again. So a link runs a widget action only with its `token`, and only once. The token is the id of the drawing of the widgets it belongs to (each reload draws them anew) and an HMAC-SHA256, under a key only the app knows, of that id, the widget kind, the action and the payload. The app keeps a ledger of the drawings that may still be on screen and of the links already used: a link runs if its drawing is the current one, or one replaced less than an hour ago (WidgetKit shows a new drawing a moment after the app writes it), and if it was not used before. The action's job then reloads the widgets, which gives every button a fresh link. On macOS, Prism creates the key on first launch (32 random bytes in `widget-links/widget-link-key` in the app's data directory, readable by the user only) and keeps the ledger next to it (`widget-links/widget-link-ledger`), out of the app's storage, because a widget shows its links long after the app drew them and a tap may launch the app: the first tap on a widget drawn days ago works. Each button in a widget snapshot carries its token (`"t"`), and the renderer adds it to the link. A link without a valid token does nothing: no action and no navigation. The app's own link opened a second time, or from a drawing replaced over an hour ago, runs nothing either and redraws the widgets, so a button a slow redraw left on screen works at the next tap. A quick second tap on the same button before the widget is redrawn is refused like a replay. iOS, visionOS and Android do not use links for widget buttons (an intent and a private broadcast that only the system's widget host can send), so there an action link never runs an action. A backend that draws widget links itself sets the key and the ledger's store with `App::set_widget_key` and builds them with `App::widget_action_link`. The design and the alternatives are in [Security](security.md#widget-buttons).

Keep jobs short: iOS gives an intent a few seconds of background time, and Android a broadcast about ten. A link that wraps a button (`div().link(..)` around it) keeps working for the rest of its content: the renderer moves the link onto the button's siblings, because WidgetKit gives a tap to the outer `Link`.

**Light & dark**: colors can be adaptive, `adaptive(rgb(0xFFFFFF), rgb(0x1C1C22))`, resolved natively everywhere: CSS `light-dark()`, dynamic `UIColor`/`NSColor`, SwiftUI color scheme in widgets and Live Activities. `Theme::system()` makes the whole component library follow the system appearance live.
