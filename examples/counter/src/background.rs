//! Background execution: a periodic "sync" (iOS may relaunch the app in the
//! background to run it) and what the app does when it leaves the screen.

use std::time::Duration;

use prism::prelude::*;

use crate::strings;

/// Last background refresh (Unix seconds) and how many ran, persisted.
#[derive(Clone, Copy, Default)]
pub struct BackgroundStatus {
    pub last: Option<f64>,
    pub runs: u32,
}

pub fn install(cx: &mut App) {
    let saved = cx.storage().get("background").unwrap_or_default();
    let mut parts = saved.split(';');
    let status = BackgroundStatus {
        last: parts.next().and_then(|s| s.parse().ok()),
        runs: parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
    };
    cx.set_global(status);

    // At most every 15 minutes; the system picks the moment.
    cx.register_background_task(
        BackgroundTask::refresh("sync").every(Duration::from_secs(15 * 60)),
        |cx| async move {
            // Stands in for a network sync.
            cx.sleep(Duration::from_secs(1)).await;
            cx.update(|app| {
                let mut s = *app.global::<BackgroundStatus>();
                s.last = Some(strings::now());
                s.runs += 1;
                let saved = format!("{};{}", s.last.unwrap_or(0.), s.runs);
                app.storage().set("background", &saved);
                app.set_global(s);
                app.reload_widgets();
            });
        },
    );

    cx.on_lifecycle(|phase, _| eprintln!("counter: app is now {phase:?}"));
}

/// "Background refresh: 14:50 (3×)" for the home screen.
pub fn summary(cx: &App) -> String {
    let s = cx.global::<BackgroundStatus>();
    match s.last {
        Some(t) => strings::home_background(cx, cx.format_time(t), s.runs),
        None => strings::home_background_never(cx),
    }
}
