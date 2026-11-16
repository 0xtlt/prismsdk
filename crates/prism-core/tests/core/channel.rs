//! `channel`: values sent from threads the app does not own wake the UI
//! thread and reach a task (#53).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use prism_core::prelude::*;
use prism_core::{AppInit, Runtime, RuntimeOptions, channel};

#[derive(Default)]
struct Log {
    events: Vec<u32>,
    closed: bool,
}

impl Render for Log {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        text(format!("{:?} {}", self.events, self.closed))
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

#[test]
fn a_foreign_thread_wakes_the_app_through_a_channel() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let tx_slot: Arc<Mutex<Option<prism_core::Sender<u32>>>> = Default::default();
    let log: Arc<Mutex<Option<Entity<Log>>>> = Default::default();
    let (tx_out, log_out) = (tx_slot.clone(), log.clone());
    let init: AppInit = Arc::new(move |cx: &mut App| {
        let (tx, mut rx) = channel::<u32>();
        *tx_out.lock().unwrap() = Some(tx);
        let entity = cx.new_entity(|cx: &mut Context<Log>| {
            cx.spawn(async move |this, cx| {
                while let Some(v) = rx.recv().await {
                    cx.update_entity(&this, |l, _| l.events.push(v));
                }
                cx.update_entity(&this, |l, _| l.closed = true);
            });
            Log::default()
        });
        *log_out.lock().unwrap() = Some(entity);
        cx.set_root(entity);
    });
    let w = wakes.clone();
    let mut rt = Runtime::with_options(
        &init,
        RuntimeOptions {
            path: "/".into(),
            wake_ui: Some(Arc::new(move || {
                w.fetch_add(1, Ordering::SeqCst);
            })),
            ..Default::default()
        },
    );
    rt.render();
    rt.poll();
    let tx = tx_slot.lock().unwrap().take().unwrap();
    let before = wakes.load(Ordering::SeqCst);
    // Sent from another thread: the UI thread is woken, no polling needed.
    let worker = std::thread::spawn(move || {
        for v in [1, 2, 3] {
            tx.send(v).unwrap();
        }
        // Dropping the last sender ends `recv` with `None`.
    });
    worker.join().unwrap();
    wait_for("the UI wake", || wakes.load(Ordering::SeqCst) > before);
    rt.poll();
    let entity = log.lock().unwrap().unwrap();
    let state = entity.read(&rt.app);
    assert_eq!(state.events, [1, 2, 3]);
    assert!(state.closed);
}

#[test]
fn sending_fails_once_the_receiver_is_gone() {
    let (tx, rx) = channel::<&str>();
    let tx2 = tx.clone();
    assert!(!tx.is_closed());
    drop(rx);
    assert!(tx2.is_closed());
    assert_eq!(tx.send("late"), Err(prism_core::channel::SendError("late")));
}

#[test]
fn queued_values_can_be_taken_without_waiting() {
    let (tx, mut rx) = channel::<u8>();
    assert_eq!(rx.try_recv(), None);
    tx.send(1).unwrap();
    tx.send(2).unwrap();
    tx.send(3).unwrap();
    assert_eq!(rx.try_recv(), Some(1));
    assert_eq!(rx.drain(), [2, 3]);
}
