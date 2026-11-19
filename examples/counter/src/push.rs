//! Remote push notifications: register, show the subscription to give a
//! server, and list what arrived (in the foreground, tapped, or silent).
//!
//! Try it in the iOS Simulator (no server needed):
//!
//! ```sh
//! xcrun simctl push booted dev.prism.counter examples/counter/push/hello.apns
//! xcrun simctl push booted dev.prism.counter examples/counter/push/silent.apns
//! ```
//!
//! With a server: `cargo run -p prismsdk-push --example push -- send '<subscription>' '{"title":"Hi","link":"/push"}'`.

use std::time::Duration;

use prism::prelude::*;
use prism::ui::{self, Theme};

use crate::strings;

/// What the screen shows (a global: every screen sees it).
#[derive(Clone, Default)]
pub struct PushStatus {
    pub subscription: Option<PushSubscription>,
    pub error: Option<String>,
    pub registering: bool,
    /// Latest first: (event, text).
    pub received: Vec<(PushEvent, String)>,
    /// The running Live Activity's push token (iOS).
    pub activity_token: Option<String>,
}

pub fn install(cx: &mut App) {
    cx.set_global(PushStatus::default());
    cx.on_push_token(|sub, app| {
        // A real app sends `sub.to_json()` to its server here.
        eprintln!("counter: push subscription {}", sub.to_json());
        app.update_global::<PushStatus, _>(|s| {
            s.subscription = Some(sub.clone());
            s.error = None;
            s.registering = false;
        });
        app.storage().set("push", "on");
    });
    cx.on_push(|push, app| {
        eprintln!(
            "counter: push {:?} {:?} {:?}",
            push.event, push.title, push.data
        );
        let line = match (&push.title, &push.body) {
            (Some(t), Some(b)) => format!("{t} — {b}"),
            (Some(t), None) => t.clone(),
            (None, Some(b)) => b.clone(),
            (None, None) => push
                .data
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", "),
        };
        app.update_global::<PushStatus, _>(|s| {
            s.received.insert(0, (push.event, line));
            s.received.truncate(5);
        });
    });
    // Live Activities started with `push_updates()` (focus.rs): a server
    // updates them with `prism_push::Push::live_activity` and this token.
    // (A token handler alone brings the widget extension along.)
    if !crate::widgets::no_widgets() {
        cx.on_activity_push_token(|t, app| {
            eprintln!(
                "counter: Live Activity push token {:?} {}",
                t.activity, t.token
            );
            if t.activity.is_some() {
                app.update_global::<PushStatus, _>(|s| s.activity_token = Some(t.token.clone()));
            }
        });
    }
    // A silent push: the system keeps the app awake until this returns.
    cx.on_background_push(|push, cx| async move {
        // Stands in for fetching what the push announced.
        cx.sleep(Duration::from_millis(500)).await;
        eprintln!("counter: background push handled ({:?})", push.get("sync"));
    });
    // Apple and Google recommend registering at every launch: tokens change.
    if cx.storage().get("push").is_some() && cx.capabilities().push.is_some() {
        cx.register_for_push();
    }
}

fn register(app: &mut App) {
    app.update_global::<PushStatus, _>(|s| s.registering = true);
    app.spawn(async move |cx| {
        let result = cx.register_for_push().await;
        cx.update(|app| {
            app.update_global::<PushStatus, _>(|s| {
                s.registering = false;
                if let Err(e) = &result {
                    s.error = Some(e.to_string());
                }
            })
        });
    });
}

fn service_name(s: PushService) -> &'static str {
    match s {
        PushService::Apns => "APNs",
        PushService::Fcm => "Firebase Cloud Messaging",
        PushService::WebPush => "Web Push",
        PushService::UnifiedPush => "UnifiedPush",
    }
}

/// The settings card.
pub fn card(cx: &App) -> ui::Card {
    let t = Theme::of(cx);
    let status = cx.global::<PushStatus>().clone();
    let muted = |s: String| text(s).text_size(13.).text_color(t.muted);
    let mut card = ui::card().child(ui::heading(strings::push_title(cx)));
    let Some(service) = cx.capabilities().push else {
        return card.child(muted(strings::push_unavailable(cx)));
    };
    card = card.child(muted(strings::push_service(cx, service_name(service))));
    match &status.subscription {
        Some(sub) => {
            let json = sub.to_json();
            // The token is long: its start is enough to recognize it.
            let short: String = sub.token.chars().take(24).collect();
            card = card
                .child(text(strings::push_registered(cx, format!("{short}…"))).text_size(14.))
                .child(
                    div()
                        .flex_row()
                        .flex_wrap()
                        .gap(8.)
                        .child(ui::button(strings::push_copy(cx)).small().on_click(
                            std::rc::Rc::new(move |_: &ClickEvent, app: &mut App| {
                                app.copy_text(json.clone())
                            }),
                        ))
                        .child(
                            ui::button(strings::push_disable(cx))
                                .small()
                                .variant(ui::Variant::Secondary)
                                .on_click(std::rc::Rc::new(|_: &ClickEvent, app: &mut App| {
                                    app.unregister_for_push();
                                    app.storage().remove("push");
                                    app.update_global::<PushStatus, _>(|s| s.subscription = None);
                                })),
                        ),
                );
        }
        None => {
            card = card.child(
                ui::button(if status.registering {
                    strings::push_registering(cx)
                } else {
                    strings::push_enable(cx)
                })
                .on_click(std::rc::Rc::new(|_: &ClickEvent, app: &mut App| {
                    register(app)
                })),
            );
        }
    }
    if let Some(t) = &status.activity_token {
        let short: String = t.chars().take(24).collect();
        card = card.child(muted(strings::push_activity(cx, format!("{short}…"))));
    }
    if let Some(e) = &status.error {
        card = card.child(muted(strings::push_error(cx, e)));
    }
    if status.received.is_empty() {
        card = card.child(muted(strings::push_none(cx)));
    } else {
        card = card.child(ui::divider());
        for (event, line) in &status.received {
            let kind = match event {
                PushEvent::Received => strings::push_received(cx),
                PushEvent::Opened => strings::push_opened(cx),
                PushEvent::Background => strings::push_background(cx),
            };
            card = card.child(
                div()
                    .gap(4.)
                    .items_start()
                    .child(ui::badge(kind))
                    .child(text(line.clone()).text_size(14.)),
            );
        }
    }
    card
}

/// `/push`: where pushes with `"link": "/push"` open.
pub struct PushScreen;

impl Render for PushScreen {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let (screen, body) = crate::page(strings::push_title(cx), cx);
        screen.child(
            body.child(card(cx)).child(
                text(strings::push_hint(cx))
                    .text_size(13.)
                    .text_color(t.muted),
            ),
        )
    }
}
