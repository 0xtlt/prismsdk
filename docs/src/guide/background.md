# Background execution

```rust
cx.on_lifecycle(|phase, app| if phase == Lifecycle::Background { save(app) });

// Finishes even if the user leaves the app (iOS: beginBackgroundTask, ~30 s).
cx.spawn(async move |this, cx| upload(&cx).await).finish_in_background();

// Periodic work, even when the app is closed (the system picks the moment).
cx.register_background_task(
    BackgroundTask::refresh("sync").every(Duration::from_secs(15 * 60)),
    |cx| async move { sync(&cx).await },
);
```

| | iOS | macOS | Web | Android | Linux | Windows |
|---|---|---|---|---|---|---|
| Lifecycle | active / inactive / background | active / inactive / hidden or minimized | page focus / visibility | active / inactive / background | active / inactive (another window focused) / background (minimized) | not yet |
| `finish_in_background` | `beginBackgroundTask` | keeps running | keeps running (server) | runs while the process lives (no extra time is asked) | keeps running | keeps running |
| Periodic tasks | `BGTaskScheduler`; the app is relaunched without UI if needed | `NSBackgroundActivityScheduler` | timer while a page is open | `JobScheduler`; the app is started without UI if needed | timer while the app runs | timer while the app runs |

`prism build/run` declares the tasks in Info.plist (`BGTaskSchedulerPermittedIdentifiers`, `UIBackgroundModes`). The iOS Simulator cannot run `BGTaskScheduler`: use a device, or `PRISM_RUN_BACKGROUND=sync` to run a task at launch and `PRISM_BACKGROUND_INTERVAL=5` to shorten intervals (macOS, web).
