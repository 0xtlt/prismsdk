//! Focus timer: a Live Activity on the lock screen and in the Dynamic Island,
//! plus a notification when the session ends. On platforms without Live
//! Activities it falls back automatically (menu bar on macOS, live banner on
//! the web). The notification has buttons: five more minutes, or a quick
//! reply that becomes a todo.

use std::time::{Duration, SystemTime};

use prism::prelude::*;
use prism::ui::{self, Theme};
use prism::{ActivityId, Color, TaskHandle};

pub struct Focus {
    minutes: u64,
    running: Option<Session>,
    completed: u32,
}

struct Session {
    activity: ActivityId,
    ends: SystemTime,
    total: Duration,
    /// Ends the session when the countdown reaches zero.
    timer: TaskHandle,
}

impl Default for Focus {
    fn default() -> Self {
        Focus {
            minutes: 25,
            running: None,
            completed: 0,
        }
    }
}

/// Describes the activity for a session ending at `ends`.
fn activity(ends: SystemTime, total: Duration, accent: Color) -> Activity {
    let minutes = total.as_secs() / 60;
    Activity::new("focus")
        .summary(format!("Focus session, {minutes} minutes"))
        .link("/focus")
        .tint(adaptive(rgb(0xFFFFFF), rgb(0x1C1C22)))
        .stale_after(ends)
        // Lock screen / banner.
        .lock_screen(
            div()
                .flex_row()
                .items_center()
                .gap(12.)
                .p(16.)
                .child(
                    image("brain.head.profile")
                        .text_size(28.)
                        .text_color(accent),
                )
                .child(
                    div()
                        .flex_1()
                        .gap(2.)
                        .child(
                            text("Focus")
                                .semibold()
                                .text_color(adaptive(rgb(0x1C1C28), rgb(0xF2F2F7))),
                        )
                        .child(
                            text(format!("{minutes}-minute session"))
                                .text_size(13.)
                                .text_color(adaptive(rgb(0x6E6E80), rgb(0x9A9AA8))),
                        ),
                )
                .child(timer(ends).text_size(28.).bold().text_color(accent)),
        )
        // Dynamic Island: compact, minimal, expanded.
        .compact(
            image("brain.head.profile").text_color(accent),
            timer(ends).semibold().text_color(accent),
        )
        .minimal(image("timer").text_color(accent))
        .expanded(
            image("brain.head.profile")
                .text_size(24.)
                .text_color(accent),
            timer(ends).text_size(24.).bold().text_color(accent),
            text("Focus").semibold().text_color(rgb(0xFFFFFF)),
            text(format!("Stay on task — {minutes} min session"))
                .text_size(13.)
                .text_color(rgb(0xB0B0BC)),
        )
}

/// Creates the Focus tab (shared, so notification actions can reach it) and
/// handles the notification's buttons.
pub fn install(cx: &mut App, store: Entity<crate::Store>) -> Entity<Focus> {
    let focus = cx.new_entity(|_| Focus::default());
    cx.on_notification_response(move |r, app| match r.action.as_deref() {
        Some("again") => focus.update(app, |f, cx| {
            if f.running.is_none() {
                f.minutes = 5;
                f.start(cx);
                cx.notify();
            }
        }),
        Some("note") => {
            let title = r.text.clone().unwrap_or_default();
            if !title.trim().is_empty() {
                store.update(app, |s, cx| s.add(title.trim().to_string(), cx));
            }
        }
        _ => {}
    });
    focus
}

/// "Session complete", delivered after `delay`.
fn done_notification(delay: Duration) -> Notification {
    Notification::new("Focus session complete")
        .id("focus-done")
        .body("Nice work. Time for a short break.")
        .link("/focus")
        .after(delay)
        .action(NotificationAction::new("again", "5 more minutes").link("/focus"))
        .action(NotificationAction::new("note", "Add a todo…").reply("What's next?"))
}

impl Focus {
    fn start(&mut self, cx: &mut Context<Self>) {
        self.start_for(Duration::from_secs(self.minutes * 60), cx);
    }

    fn start_for(&mut self, total: Duration, cx: &mut Context<Self>) {
        let ends = SystemTime::now() + total;
        let accent = Theme::of(cx).accent;
        let id = cx.start_activity(activity(ends, total, accent));
        cx.send_notification(done_notification(total));
        let timer = Self::finish_after(total, cx);
        self.running = Some(Session {
            activity: id,
            ends,
            total,
            timer,
        });
    }

    /// Ends the session (and its Live Activity) when the time is up.
    fn finish_after(delay: Duration, cx: &mut Context<Self>) -> TaskHandle {
        cx.spawn(async move |this, cx| {
            cx.sleep(delay).await;
            cx.update_entity(&this, |focus, cx| {
                if let Some(s) = focus.running.take() {
                    // Leave a final "done" state on the lock screen for a while.
                    let done = Activity::new("focus")
                        .summary("Focus session complete")
                        .link("/focus")
                        .lock_screen(
                            div()
                                .flex_row()
                                .items_center()
                                .gap(12.)
                                .p(16.)
                                .child(
                                    image("checkmark.circle.fill")
                                        .text_size(28.)
                                        .text_color(rgb(0x2E9E5B)),
                                )
                                .child(text("Focus session complete").semibold()),
                        )
                        .compact(image("checkmark.circle.fill"), text("Done"))
                        .minimal(image("checkmark"));
                    cx.end_activity(s.activity, Some(done), false);
                    focus.completed += 1;
                }
            });
        })
    }

    fn extend(&mut self, cx: &mut Context<Self>) {
        let accent = Theme::of(cx).accent;
        if let Some(mut s) = self.running.take() {
            s.ends += Duration::from_secs(5 * 60);
            s.total += Duration::from_secs(5 * 60);
            s.timer.cancel();
            let left = s.ends.duration_since(SystemTime::now()).unwrap_or_default();
            s.timer = Self::finish_after(left, cx);
            cx.update_activity(s.activity, activity(s.ends, s.total, accent));
            cx.send_notification(done_notification(left));
            self.running = Some(s);
        }
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.running.take() {
            s.timer.cancel();
            cx.end_activity(s.activity, None, true);
            cx.cancel_notification("focus-done");
        }
    }
}

impl Render for Focus {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let caps = cx.capabilities();
        let where_ = if caps.dynamic_island {
            "Lock screen and Dynamic Island"
        } else if caps.live_activities {
            "Outside the app (menu bar or live banner)"
        } else {
            "In the app"
        };
        let body = scroll().p(20.).gap(20.);
        let body = match &self.running {
            Some(s) => body.child(
                ui::card()
                    .child(ui::heading("Focusing"))
                    .child(timer(s.ends).text_size(56.).bold().text_color(t.accent))
                    .child(
                        text(format!("Shown live: {where_}"))
                            .text_size(13.)
                            .text_color(t.muted),
                    )
                    .child(
                        div()
                            .flex_row()
                            .gap(8.)
                            .child(
                                ui::button("+5 min")
                                    .secondary()
                                    .on_click(cx.listener(|this, _, cx| this.extend(cx))),
                            )
                            .child(
                                ui::button("Stop")
                                    .danger()
                                    .on_click(cx.listener(|this, _, cx| this.stop(cx))),
                            ),
                    ),
            ),
            None => body.child(
                ui::card()
                    .child(ui::heading("New session"))
                    .child(
                        div()
                            .flex_row()
                            .gap(8.)
                            .accessibility_role(Role::Group)
                            .accessibility_label("Duration")
                            .children([1u64, 5, 25, 50].into_iter().map(|m| {
                                ui::button(format!("{m} min"))
                                    .small()
                                    .variant(if m == self.minutes {
                                        ui::Variant::Primary
                                    } else {
                                        ui::Variant::Secondary
                                    })
                                    .on_click(cx.listener(move |this, _, _| this.minutes = m))
                            })),
                    )
                    .child(
                        ui::button("Start focus")
                            .large()
                            .full_width()
                            .on_click(cx.listener(|this, _, cx| this.start(cx))),
                    )
                    .child(
                        ui::button("Try the notification (10 s)")
                            .ghost()
                            .small()
                            .hint("Starts a 10-second session; its notification has buttons")
                            .on_click(cx.listener(|this, _, cx| {
                                this.start_for(Duration::from_secs(10), cx)
                            })),
                    )
                    .when(self.completed > 0, |d| {
                        d.child(
                            ui::badge(format!("{} session(s) completed", self.completed))
                                .tone(ui::Variant::Primary),
                        )
                    })
                    .child(
                        text(format!(
                            "Live Activity: {where_}. A notification arrives when it ends."
                        ))
                        .text_size(13.)
                        .text_color(t.muted),
                    ),
            ),
        };
        screen("Focus").bg(t.background).child(body)
    }
}
