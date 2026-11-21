//! Widget buttons (#47): `widget_action` nodes in widget and Live Activity
//! snapshots, the app's `on_widget_action` job, the reload after it, and the
//! link fallback that runs an action when the app opens on it.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context as TaskContext, Poll, Waker};

use prism_core::prelude::*;
use prism_core::{PlatformRequest, SNode, WidgetAction};

use prism_testing::{Harness, mount};

#[derive(Default)]
struct Lights {
    on: bool,
    actions: Vec<WidgetAction>,
}

impl Render for Lights {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        text(if self.on { "on" } else { "off" })
    }
}

/// A future that completes once the test opens it.
#[derive(Clone, Default)]
struct Gate {
    open: Rc<Cell<bool>>,
    waker: Rc<RefCell<Option<Waker>>>,
}

impl Gate {
    fn open(&self) {
        self.open.set(true);
        if let Some(w) = self.waker.borrow_mut().take() {
            w.wake();
        }
    }
}

impl Future for Gate {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<()> {
        if self.open.get() {
            Poll::Ready(())
        } else {
            *self.waker.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// A "lights" widget with a button, and a handler that waits for the gate
/// (a network call) before switching the lights.
fn app() -> Harness {
    Harness::new(|cx| {
        let lights = cx.new_entity(|_| Lights::default());
        mount(cx, lights);
        let gate = Gate::default();
        cx.set_global(gate.clone());
        cx.register_widget(
            Widget::new("lights", "Lights")
                .families(&[WidgetFamily::Small])
                .link("/")
                .render(move |cx, _| {
                    let on = lights.read(cx).on;
                    div()
                        .link("/lights")
                        .child(text(if on { "Lights on" } else { "Lights off" }))
                        .child(
                            div()
                                .widget_action("toggle", "kitchen")
                                .child(text("Toggle")),
                        )
                }),
        );
        cx.on_widget_action(move |action, cx| {
            let gate = gate.clone();
            async move {
                gate.await;
                cx.update_entity(&lights, |l, cx| {
                    l.on = !l.on;
                    l.actions.push(action);
                    cx.notify();
                });
            }
        });
    })
}

fn reloads(requests: &[PlatformRequest]) -> Vec<String> {
    requests
        .iter()
        .filter_map(|r| match r {
            PlatformRequest::ReloadWidgets(snaps) => Some(snaps[0].families[0].1.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn snapshots_carry_the_action_with_the_widget_kind() {
    let mut h = app();
    let json = reloads(&h.rt.take_requests()).remove(0);
    assert!(
        json.contains(r#""wa":{"k":"lights","a":"toggle","p":"kitchen"}"#),
        "{json}"
    );
    // The link stays on the container; the button has none.
    assert!(json.contains(r#""l":"/lights""#));
}

#[test]
fn an_action_runs_the_job_then_reloads_the_widgets() {
    let mut h = app();
    h.rt.take_requests();
    assert!(h.rt.app.handles_widget_actions());
    let task =
        h.rt.run_widget_action(WidgetAction::new("lights", "toggle", "kitchen"));
    // The job waits (a network call): the platform keeps the app alive.
    assert!(h.rt.is_task_running(task));
    assert!(h.rt.wants_background_time());
    assert!(reloads(&h.rt.take_requests()).is_empty());
    h.rt.app.global::<Gate>().clone().open();
    let patches = h.rt.poll();
    h.apply(patches);
    assert!(!h.rt.is_task_running(task));
    // The state changed, and the widgets were drawn from it.
    let json = reloads(&h.rt.take_requests());
    assert_eq!(json.len(), 1);
    assert!(json[0].contains("Lights on"));
    let lights = h.root::<Lights>();
    let actions = lights.read(&h.rt.app).actions.clone();
    assert_eq!(actions, [WidgetAction::new("lights", "toggle", "kitchen")]);
    h.render();
    h.host.by_text("on");
}

#[test]
fn without_a_handler_an_action_only_reloads() {
    let init: prism_core::AppInit = std::sync::Arc::new(|app: &mut App| {
        app.register_widget(Widget::new("w", "W").render(|_, _| text("w")));
    });
    let mut rt = prism_core::Runtime::new(&init);
    rt.take_requests();
    assert!(!rt.app.handles_widget_actions());
    let task = rt.run_widget_action(WidgetAction::new("w", "go", ""));
    assert!(!rt.is_task_running(task));
    assert_eq!(reloads(&rt.take_requests()).len(), 1);
}

/// The key a backend keeps for the app ([`App::set_widget_key`]).
const KEY: [u8; 32] = [7; 32];

/// The token the widget's button carries in its snapshot (`wa.t`).
fn snapshot_token(json: &str) -> String {
    let at = json.find(r#""t":""#).expect("a token") + 5;
    json[at..at + 32].to_string()
}

#[test]
fn the_action_link_runs_the_action_instead_of_navigating() {
    let action = WidgetAction::new("lights", "set level", "a&b=c/50%");
    let link = action.to_link();
    assert!(link.starts_with("/__prism/widget-action?"), "{link}");
    assert_eq!(WidgetAction::from_link(&link), Some(action.clone()));
    assert_eq!(WidgetAction::from_link("/lights?action=x"), None);
    assert_eq!(
        WidgetAction::from_link("/__prism/widget-action?kind=k"),
        None
    );

    let mut h = app();
    h.rt.app.set_widget_key(KEY);
    h.rt.take_requests();
    h.rt.app.global::<Gate>().clone().open();
    let token =
        h.rt.app
            .widget_action_link(&WidgetAction::new("lights", "toggle", "kitchen light"))
            .unwrap()
            .rsplit_once("&token=")
            .unwrap()
            .1
            .to_string();
    // How the macOS widget extension writes it (Swift percent-encoding).
    let patches = h.rt.open_link(&format!(
        "/__prism/widget-action?kind=lights&action=toggle&payload=kitchen%20light&token={token}"
    ));
    h.apply(patches);
    let lights = h.root::<Lights>();
    assert_eq!(
        lights.read(&h.rt.app).actions,
        [WidgetAction::new("lights", "toggle", "kitchen light")]
    );
    assert!(
        !h.rt
            .app
            .current_path()
            .is_some_and(|p| p.starts_with("/__prism")),
        "no navigation"
    );
    assert_eq!(reloads(&h.rt.take_requests()).len(), 1);
}

#[test]
fn queued_actions_parse_from_the_extension_json() {
    let list = WidgetAction::list_from_json(
        r#"[{"a":"toggle","k":"todo","p":"2"},{"k":"x"},"junk",{"a":"go"}]"#,
    );
    assert_eq!(
        list,
        [
            WidgetAction::new("todo", "toggle", "2"),
            WidgetAction::new("", "go", ""),
        ]
    );
    assert!(WidgetAction::list_from_json("nope").is_empty());
}

#[test]
fn live_activity_buttons_carry_the_activity_kind() {
    let init: prism_core::AppInit = std::sync::Arc::new(|app: &mut App| {
        app.start_activity(
            Activity::new("timer")
                .summary("Timer")
                .lock_screen(div().widget_action("pause", "").child(text("Pause"))),
        );
    });
    let mut rt = prism_core::Runtime::new(&init);
    let content = rt
        .take_requests()
        .into_iter()
        .find_map(|r| match r {
            PlatformRequest::StartActivity { content, .. } => Some(content),
            _ => None,
        })
        .expect("started");
    let ls: SNode = content.lock_screen.clone().expect("lock screen");
    let action = ls.action.expect("action");
    assert_eq!(action, WidgetAction::new("timer", "pause", ""));
    assert!(
        content
            .to_json()
            .contains(r#""wa":{"k":"timer","a":"pause","p":""}"#)
    );
}

/// S19: any app or web page can open the app's links; only the app's own
/// widgets know the tokens that run an action.
#[test]
fn s19_only_links_with_the_apps_token_run_an_action() {
    let mut h = app();
    h.rt.take_requests();
    h.rt.app.global::<Gate>().clone().open();
    let toggle = WidgetAction::new("lights", "toggle", "kitchen");
    let plain = "/__prism/widget-action?kind=lights&action=toggle&payload=kitchen";
    let ran = |h: &mut Harness| h.root::<Lights>().read(&h.rt.app).actions.len();

    // Without a key (iOS, Android, the web), no link runs an action.
    assert_eq!(h.rt.app.widget_action_link(&toggle), None);
    let patches = h.rt.open_link(plain);
    h.apply(patches);
    assert_eq!(ran(&mut h), 0, "an outside link ran the action");

    // The widget's snapshot carries the token; the renderer adds it to the
    // link it opens.
    h.rt.app.set_widget_key(KEY);
    let json = reloads(&h.rt.take_requests()).remove(0);
    let token = snapshot_token(&json);
    assert!(
        json.contains(&format!(
            r#""wa":{{"k":"lights","a":"toggle","p":"kitchen","t":"{token}"}}"#
        )),
        "{json}"
    );
    let signed = h.rt.app.widget_action_link(&toggle).unwrap();
    assert_eq!(signed, format!("{plain}&token={token}"));

    let forged = [
        // No token, or an empty one.
        plain.to_string(),
        format!("{plain}&token="),
        // Another action, payload or widget with this token.
        format!("/__prism/widget-action?kind=lights&action=toggle&payload=garage&token={token}"),
        format!("/__prism/widget-action?kind=lights&action=reset&payload=kitchen&token={token}"),
        format!("/__prism/widget-action?kind=locks&action=toggle&payload=kitchen&token={token}"),
        // Fields moved between parts.
        format!("/__prism/widget-action?kind=lights&action=togglekitchen&payload=&token={token}"),
        // Part of the token (fewer guesses), or one digit off.
        format!("{plain}&token={}", &token[..2]),
        format!("{plain}&token={}", &token[..30]),
        format!("{plain}&token={}0", &token[..31]),
        format!("{plain}&token={token}00"),
        // A token of another key.
        {
            let mut other = app();
            other.rt.app.set_widget_key([8; 32]);
            other.rt.app.widget_action_link(&toggle).unwrap()
        },
    ];
    for link in &forged {
        let patches = h.rt.open_link(link);
        h.apply(patches);
        assert_eq!(ran(&mut h), 0, "{link} ran the action");
        assert!(
            !h.rt
                .app
                .current_path()
                .is_some_and(|p| p.starts_with("/__prism")),
            "{link} navigated"
        );
    }

    let patches = h.rt.open_link(&signed);
    h.apply(patches);
    assert_eq!(ran(&mut h), 1, "the widget's own link runs the action");
}
