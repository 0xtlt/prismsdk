//! Background task scheduling and the app lifecycle, without real time: tasks
//! wait on a [`Gate`] the test opens.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::Duration;

use prism_core::prelude::*;
use prism_core::{BackgroundKind, PlatformRequest};

use prism_testing::{Harness, mount};

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
    fn wait(&self) -> GateWait {
        GateWait(self.clone())
    }
}

struct GateWait(Gate);

impl Future for GateWait {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<()> {
        if self.0.open.get() {
            Poll::Ready(())
        } else {
            *self.0.waker.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[derive(Default)]
struct Status {
    runs: u32,
    phases: Vec<Lifecycle>,
}

impl Render for Status {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        text(format!("runs {} phases {}", self.runs, self.phases.len()))
    }
}

fn gate(h: &Harness) -> Gate {
    h.rt.app.global::<Gate>().clone()
}

/// An app with a "sync" task that waits for the gate, then counts a run.
fn app() -> Harness {
    Harness::new(|cx| {
        let status = cx.new_entity(|_| Status::default());
        mount(cx, status);
        let gate = Gate::default();
        cx.set_global(gate.clone());
        cx.on_lifecycle(move |phase, app| {
            status.update(app, |s, cx| {
                s.phases.push(phase);
                cx.notify();
            })
        });
        cx.register_background_task(BackgroundTask::refresh("sync"), move |cx| {
            let wait = gate.wait();
            async move {
                wait.await;
                cx.update_entity(&status, |s, _| s.runs += 1);
            }
        });
    })
}

fn scheduled(requests: &[PlatformRequest]) -> Vec<BackgroundTask> {
    requests
        .iter()
        .filter_map(|r| match r {
            PlatformRequest::ScheduleBackgroundTask(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn task_definitions() {
    let r = BackgroundTask::refresh("a");
    assert_eq!(r.every, Duration::from_secs(900));
    assert_eq!(r.kind, BackgroundKind::Refresh);
    // Requirements only apply to processing tasks.
    assert_eq!(r.clone().requires_network().requires_power(), r);
    let p = BackgroundTask::processing("b")
        .requires_power()
        .every(Duration::from_secs(3600));
    assert_eq!(
        p.kind,
        BackgroundKind::Processing {
            requires_network: false,
            requires_power: true
        }
    );
    assert_eq!(p.every, Duration::from_secs(3600));
    assert_eq!(
        BackgroundTask::processing("c").every,
        Duration::from_secs(86_400)
    );
}

#[test]
fn registering_the_same_id_replaces_the_task_and_reschedules() {
    let init: prism_core::AppInit = std::sync::Arc::new(|app: &mut App| {
        app.register_background_task(BackgroundTask::refresh("sync"), |_| async {});
        app.register_background_task(BackgroundTask::processing("clean"), |_| async {});
        app.register_background_task(
            BackgroundTask::processing("sync").requires_network(),
            |_| async {},
        );
    });
    let mut rt = prism_core::Runtime::new(&init);
    let tasks = rt.app.background_tasks();
    assert_eq!(
        tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["clean", "sync"]
    );
    assert!(matches!(
        tasks[1].kind,
        BackgroundKind::Processing {
            requires_network: true,
            ..
        }
    ));
    assert_eq!(scheduled(&rt.take_requests()).len(), 3);
    // Bundlers read this to declare identifiers (ids are JSON-escaped).
    rt.app
        .register_background_task(BackgroundTask::refresh("we\"ird"), |_| async {});
    assert_eq!(
        rt.app.describe_background(),
        r#"[{"id":"clean","kind":"processing"},{"id":"sync","kind":"processing"},{"id":"we\"ird","kind":"refresh"}]"#
    );
}

#[test]
fn lifecycle_handlers_run_on_changes_only() {
    let mut h = app();
    assert_eq!(h.rt.app.lifecycle(), Lifecycle::Active);
    // Same phase: nothing.
    assert!(h.rt.set_lifecycle(Lifecycle::Active).is_empty());
    let p = h.rt.set_lifecycle(Lifecycle::Inactive);
    h.apply(p);
    h.rt.set_lifecycle(Lifecycle::Inactive);
    let p = h.rt.set_lifecycle(Lifecycle::Background);
    h.apply(p);
    let status = h.root::<Status>();
    assert_eq!(
        status.read(&h.rt.app).phases,
        [Lifecycle::Inactive, Lifecycle::Background]
    );
    h.host.by_text("runs 0 phases 2");
}

#[test]
fn a_background_run_keeps_the_app_alive_until_it_finishes() {
    let mut h = app();
    h.rt.take_requests();
    let id = h.rt.run_background_task("sync").expect("registered");
    // The platform is asked for the next run right away.
    assert_eq!(scheduled(&h.rt.take_requests()).len(), 1);
    assert!(h.rt.is_task_running(id));
    assert!(h.rt.wants_background_time());
    assert!(h.rt.poll().is_empty());
    gate(&h).open();
    let p = h.rt.poll();
    h.apply(p);
    assert!(!h.rt.is_task_running(id));
    assert!(!h.rt.wants_background_time());
    h.host.by_text("runs 1 phases 0");
    // Unknown ids are refused.
    assert!(h.rt.run_background_task("nope").is_none());
}

#[test]
fn a_cancelled_background_run_never_completes() {
    let mut h = app();
    let id = h.rt.run_background_task("sync").unwrap();
    // The platform's time expired after the task was woken, before it ran.
    gate(&h).open();
    h.rt.cancel_task(id);
    assert!(!h.rt.is_task_running(id));
    assert!(!h.rt.wants_background_time());
    assert!(h.rt.poll().is_empty());
    assert_eq!(h.root::<Status>().read(&h.rt.app).runs, 0);
}

#[test]
fn a_task_that_needs_no_waiting_finishes_at_once() {
    let init: prism_core::AppInit = std::sync::Arc::new(|cx: &mut App| {
        cx.register_background_task(BackgroundTask::refresh("now"), |_| async {});
    });
    let mut rt = prism_core::Runtime::new(&init);
    let id = rt.run_background_task("now").unwrap();
    assert!(!rt.is_task_running(id));
    assert!(!rt.wants_background_time());
    // Without a UI (launched in the background) polling works too.
    rt.poll_headless();
}
