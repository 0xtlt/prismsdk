//! Widgets refreshed without the app (#331): the `refresh` hook a widget
//! extension (or Android's widget alarm) runs on the system's timeline
//! reloads, the reload policy written to the snapshot, and the
//! refresh-only drawing of a runtime started from the app's `init`.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::Duration;

use prism_core::prelude::*;
use prism_core::{AppInit, PlatformRequest, Runtime, WidgetReload};

/// A future that completes once the test opens it (the network call).
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

/// What the "board" widget's server answers next: departures, or an error.
#[derive(Clone)]
struct Server(Rc<RefCell<Result<&'static str, &'static str>>>);

struct Board {
    next: String,
}

/// The kinds and families JSON of the widgets drawn by these requests.
fn drawn(requests: &[PlatformRequest]) -> Vec<(String, String)> {
    requests
        .iter()
        .filter_map(|r| match r {
            PlatformRequest::ReloadWidgets(snaps) => Some(snaps),
            _ => None,
        })
        .flatten()
        .map(|s| (s.kind.clone(), s.to_json()))
        .collect()
}

/// A "board" widget whose hook waits for the gate, then reads the server;
/// a "clock" widget without a hook.
fn init(timeout: Duration) -> AppInit {
    Arc::new(move |cx: &mut App| {
        let board = cx.new_entity(|_| Board {
            next: "nothing yet".into(),
        });
        let gate = Gate::default();
        let server = Server(Rc::new(RefCell::new(Ok("12 Harbour 09:41"))));
        cx.set_global(gate.clone());
        cx.set_global(server.clone());
        cx.set_global(Runs::default());
        cx.reload_widgets_on(&board);
        cx.register_widget(
            Widget::new("board", "Departures")
                .families(&[WidgetFamily::Small])
                .render(move |cx, _| text(board.read(cx).next.clone()))
                .refresh(move |cx| {
                    let (gate, server) = (gate.clone(), server.clone());
                    async move {
                        cx.update(|app| {
                            app.global::<Runs>().0.set(app.global::<Runs>().0.get() + 1)
                        });
                        gate.await;
                        let next = (*server.0.borrow())?;
                        cx.update_entity(&board, |b, _| b.next = next.into());
                        Ok::<_, &str>(())
                    }
                })
                .refresh_timeout(timeout)
                .reload_after(Duration::from_secs(30 * 60)),
        );
        cx.register_widget(
            Widget::new("clock", "Clock")
                .families(&[WidgetFamily::Small])
                .render(|_, _| text("clock")),
        );
    })
}

/// How many times the board's hook started.
#[derive(Clone, Default)]
struct Runs(Rc<Cell<u32>>);

fn runs(rt: &Runtime) -> u32 {
    rt.app.global::<Runs>().0.get()
}

/// A runtime like a widget extension's: started from `init`, drawing only
/// what a refresh brought.
fn extension(timeout: Duration) -> Runtime {
    let mut rt = Runtime::new(&init(timeout));
    rt.app.set_widgets_refresh_only(true);
    rt
}

/// The app's widget `w`, as the snapshot JSON its first drawing writes.
fn first_drawing(w: fn() -> Widget) -> String {
    let init: AppInit = Arc::new(move |cx: &mut App| cx.register_widget(w()));
    let mut rt = Runtime::new(&init);
    drawn(&rt.take_requests()).remove(0).1
}

fn plain() -> Widget {
    Widget::new("w", "W").render(|_, _| text("w"))
}

fn hooked() -> Widget {
    plain().refresh(|_| async { Ok::<_, String>(()) })
}

#[test]
fn the_snapshot_carries_the_reload_policy_and_the_refresh_flag() {
    // No policy and no hook: the extension's own default, as before.
    let j = first_drawing(plain);
    assert!(!j.contains("reload") && !j.contains("refresh"), "{j}");
    let j = first_drawing(|| plain().reload(WidgetReload::Never));
    assert!(j.contains(r#""reload":"never""#), "{j}");
    let j = first_drawing(|| plain().reload(WidgetReload::AtEnd));
    assert!(j.contains(r#""reload":"end""#), "{j}");
    let j = first_drawing(|| plain().reload_after(Duration::from_millis(90_500)));
    assert!(j.contains(r#""reload":90.5"#), "{j}");
    // A hook without a policy asks every 15 minutes.
    assert!(hooked().has_refresh() && !plain().has_refresh());
    assert_eq!(
        hooked().reload_policy(),
        Some(WidgetReload::After(Widget::DEFAULT_RELOAD))
    );
    let j = first_drawing(hooked);
    assert!(j.contains(r#""reload":900,"refresh":true"#), "{j}");
    // The bundler learns which widgets have a hook.
    let init: AppInit = Arc::new(|cx: &mut App| cx.register_widget(hooked()));
    let rt = Runtime::new(&init);
    assert!(rt.app.describe_widgets().contains(r#""refresh":true"#));
    let init: AppInit = Arc::new(|cx: &mut App| cx.register_widget(plain()));
    let rt = Runtime::new(&init);
    assert!(rt.app.describe_widgets().contains(r#""refresh":false"#));
}

#[test]
fn a_refresh_runs_the_hook_then_draws_only_that_widget() {
    let mut rt = extension(Widget::REFRESH_TIMEOUT);
    // `init` registered both widgets: drawing them from its fresh state
    // would replace the app's drawings.
    assert!(drawn(&rt.take_requests()).is_empty());
    let task = rt.refresh_widget("board").expect("a hook to wait for");
    assert!(rt.is_task_running(task));
    assert!(rt.wants_background_time());
    assert!(drawn(&rt.take_requests()).is_empty());
    rt.app.global::<Gate>().clone().open();
    rt.poll();
    assert!(!rt.is_task_running(task));
    let drawn = drawn(&rt.take_requests());
    assert_eq!(drawn.len(), 1, "{drawn:?}");
    assert_eq!(drawn[0].0, "board");
    assert!(drawn[0].1.contains("12 Harbour 09:41"), "{}", drawn[0].1);
    assert!(drawn[0].1.contains(r#""reload":1800"#));
}

#[test]
fn a_failed_refresh_keeps_the_previous_drawings() {
    let mut rt = extension(Widget::REFRESH_TIMEOUT);
    rt.take_requests();
    *rt.app.global::<Server>().0.borrow_mut() = Err("503 Service Unavailable");
    let task = rt.refresh_widget("board").unwrap();
    rt.app.global::<Gate>().clone().open();
    rt.poll();
    assert!(!rt.is_task_running(task));
    assert!(drawn(&rt.take_requests()).is_empty());
}

#[test]
fn a_refresh_past_its_timeout_is_given_up() {
    let mut rt = extension(Duration::from_millis(20));
    rt.take_requests();
    let task = rt.refresh_widget("board").unwrap();
    std::thread::sleep(Duration::from_millis(40));
    rt.poll();
    assert!(
        !rt.is_task_running(task),
        "the hook is dropped at its timeout"
    );
    // The gate opening afterwards changes nothing.
    rt.app.global::<Gate>().clone().open();
    rt.poll();
    assert!(drawn(&rt.take_requests()).is_empty());
}

#[test]
fn requests_close_together_share_one_run() {
    let mut rt = extension(Widget::REFRESH_TIMEOUT);
    rt.take_requests();
    let first = rt.refresh_widget("board").unwrap();
    // Another instance of the widget asks while the hook runs.
    assert_eq!(rt.refresh_widget("board"), Some(first));
    assert_eq!(runs(&rt), 1);
    rt.app.global::<Gate>().clone().open();
    rt.poll();
    // Just refreshed: nothing to wait for, the drawing is on its way.
    assert_eq!(rt.refresh_widget("board"), None);
    assert_eq!(runs(&rt), 1);
    assert_eq!(drawn(&rt.take_requests()).len(), 1);
}

#[test]
fn widgets_without_a_hook_have_nothing_to_refresh() {
    let mut rt = extension(Widget::REFRESH_TIMEOUT);
    assert_eq!(rt.refresh_widget("clock"), None);
    assert_eq!(rt.refresh_widget("unknown"), None);
    assert!(drawn(&rt.take_requests()).is_empty());
}

#[test]
fn in_the_app_a_refresh_redraws_every_widget() {
    // Android runs the hook in the app's process when the app is open:
    // its state is the real one, every widget is drawn from it.
    let mut rt = Runtime::new(&init(Widget::REFRESH_TIMEOUT));
    assert_eq!(drawn(&rt.take_requests()).len(), 2);
    rt.refresh_widget("board").unwrap();
    rt.app.global::<Gate>().clone().open();
    rt.poll();
    let drawn = drawn(&rt.take_requests());
    let kinds: Vec<&str> = drawn.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, ["board", "clock"]);
}
