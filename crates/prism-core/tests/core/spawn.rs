//! Tasks spawned from `render` wake the UI thread (#57): `Runtime::render`
//! polls tasks before it renders, so without a wake they would wait for the
//! next event.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use prism_core::prelude::*;
use prism_core::{AppInit, Runtime, RuntimeOptions};

/// Starts loading the first time it renders, like a lazily loaded screen.
#[derive(Default)]
struct Lazy {
    started: bool,
    value: Option<u32>,
}

impl Render for Lazy {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.started {
            self.started = true;
            cx.spawn(async move |this, cx| {
                let value = cx.background(|| 42).await;
                cx.update_entity(&this, |l, _| l.value = Some(value));
            });
        }
        text(match self.value {
            Some(v) => format!("loaded {v}"),
            None => "loading".into(),
        })
    }
}

/// Spawns a task that does nothing on every render.
struct Idle;

impl Render for Idle {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        cx.spawn(async move |_, _| {});
        text("idle")
    }
}

fn wait_for(what: &str, done: impl Fn() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn runtime(init: AppInit, wakes: &Arc<AtomicUsize>) -> Runtime {
    let w = wakes.clone();
    Runtime::with_options(
        &init,
        RuntimeOptions {
            path: "/".into(),
            wake_ui: Some(Arc::new(move || {
                w.fetch_add(1, Ordering::SeqCst);
            })),
            ..Default::default()
        },
    )
}

#[test]
fn a_task_spawned_during_render_wakes_the_ui() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let lazy: Arc<Mutex<Option<Entity<Lazy>>>> = Default::default();
    let out = lazy.clone();
    let init: AppInit = Arc::new(move |cx: &mut App| {
        let entity = cx.new_entity(|_| Lazy::default());
        *out.lock().unwrap() = Some(entity);
        cx.set_root(entity);
    });
    let mut rt = runtime(init, &wakes);
    rt.render();
    let entity = lazy.lock().unwrap().unwrap();
    assert!(entity.read(&rt.app).started);
    // No event follows: only the wake tells the backend to poll.
    assert!(wakes.load(Ordering::SeqCst) > 0, "spawning woke the UI");
    let woken = wakes.load(Ordering::SeqCst);
    rt.poll();
    // The background work runs, then wakes the UI again with its result.
    wait_for("the background result", || {
        wakes.load(Ordering::SeqCst) > woken
    });
    rt.poll();
    assert_eq!(entity.read(&rt.app).value, Some(42));
    assert!(!rt.has_tasks());
}

#[test]
fn a_task_spawned_during_render_does_not_loop() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Idle);
        cx.set_root(root);
    });
    let mut rt = runtime(init, &wakes);
    rt.render();
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    // The task finishes without touching anything: nothing re-renders, so
    // nothing spawns or wakes again.
    assert!(rt.poll().is_empty());
    assert!(!rt.has_tasks());
    assert!(rt.poll().is_empty());
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
}
