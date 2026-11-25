//! Frame callbacks (#311): once and every frame, from a display link or
//! the timer standing in for one, paused in the background.

use std::time::{Duration, Instant};

use prism_core::Lifecycle;
use prism_core::prelude::*;

use prism_testing::{Harness, mount};

#[derive(Default)]
struct Ticks {
    frames: Vec<FrameInfo>,
    once: usize,
    ticker: Option<FrameTicker>,
}

impl Render for Ticks {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        text(format!("{} frames", self.frames.len()))
    }
}

fn app() -> Harness {
    Harness::new(|cx| {
        let root = cx.new_entity(|_| Ticks::default());
        mount(cx, root);
    })
}

fn native(h: &mut Harness) {
    h.rt.set_native_frames(true);
}

fn start(h: &mut Harness) {
    let root = h.root::<Ticks>();
    h.rt.app.update(&root, |_, cx| {
        let t = cx.frames(|this, frame, _| this.frames.push(*frame));
        cx.on_next_frame(|this, _, _| this.once += 1);
        // `update` runs with the entity taken out: store it afterwards.
        cx.on_next_frame(move |this, _, _| this.ticker = Some(t));
    });
}

fn ticks(h: &Harness) -> &Ticks {
    h.root::<Ticks>().read(&h.rt.app)
}

const MS: Duration = Duration::from_millis(1);

#[test]
fn a_display_link_drives_the_callbacks_and_re_renders() {
    let mut h = app();
    native(&mut h);
    assert!(!h.rt.wants_frames());
    start(&mut h);
    assert!(h.rt.wants_frames());
    // No timer stands in for the display link.
    assert_eq!(h.rt.next_timer(), None);

    let t0 = Instant::now();
    let p = h.rt.frame(t0, t0 + 16 * MS);
    h.apply(p);
    h.host.by_text("1 frames");
    let p = h.rt.frame(t0 + 16 * MS, t0 + 33 * MS);
    h.apply(p);
    h.rt.frame(t0 + 33 * MS, t0 + 50 * MS);
    let t = ticks(&h);
    assert_eq!(t.once, 1, "on_next_frame runs once");
    assert_eq!(t.frames.len(), 3);
    assert_eq!(t.frames[0].elapsed, Duration::ZERO);
    assert_eq!(t.frames[0].delta, Duration::ZERO);
    assert_eq!(t.frames[0].target_timestamp, t0 + 16 * MS);
    assert_eq!(t.frames[1].delta, 16 * MS);
    assert_eq!(t.frames[2].elapsed, 33 * MS);
    assert_eq!(t.frames[2].delta, 17 * MS);

    // Stopped: the display link may stop.
    let ticker = t.ticker.unwrap();
    h.rt.app.stop_frames(ticker);
    assert!(!h.rt.wants_frames());
    h.rt.frame(t0 + 50 * MS, t0 + 66 * MS);
    assert_eq!(ticks(&h).frames.len(), 3);
}

#[test]
fn frames_pause_in_the_background() {
    let mut h = app();
    native(&mut h);
    start(&mut h);
    h.rt.set_lifecycle(Lifecycle::Background);
    assert!(!h.rt.wants_frames());
    let t0 = Instant::now();
    h.rt.frame(t0, t0 + 16 * MS);
    assert_eq!(ticks(&h).frames.len(), 0);
    h.rt.set_lifecycle(Lifecycle::Active);
    assert!(h.rt.wants_frames());
    h.rt.frame(t0 + 16 * MS, t0 + 33 * MS);
    assert_eq!(ticks(&h).frames.len(), 1);
}

#[test]
fn a_released_entity_stops_its_ticker() {
    let mut h = app();
    native(&mut h);
    start(&mut h);
    let t0 = Instant::now();
    h.rt.frame(t0, t0 + 16 * MS);
    let root = h.root::<Ticks>();
    h.rt.app.release(root);
    h.rt.frame(t0 + 16 * MS, t0 + 33 * MS);
    assert!(!h.rt.wants_frames());
}

#[test]
fn a_timer_stands_in_without_a_display_link() {
    let mut h = app();
    start(&mut h);
    // The timer's task starts at the next poll (a backend's commit).
    h.rt.poll();
    let started = Instant::now();
    let due = h.rt.next_timer().expect("the 60 Hz timer");
    assert!(due <= started + 17 * MS, "{:?}", due - started);
    // Three refreshes.
    while ticks(&h).frames.len() < 3 {
        assert!(started.elapsed() < Duration::from_secs(5), "no frames");
        if let Some(at) = h.rt.next_timer() {
            std::thread::sleep(at.saturating_duration_since(Instant::now()));
        }
        let p = h.rt.poll();
        h.apply(p);
    }
    let t = ticks(&h);
    assert_eq!(t.once, 1);
    // A 60 Hz grid, however late the wake-ups.
    assert!(
        t.frames[1].delta >= 16 * MS && t.frames[1].delta.as_nanos().is_multiple_of(16_666_667),
        "{:?}",
        t.frames[1].delta
    );
    assert!(t.frames[2].elapsed >= t.frames[1].elapsed);
    // Stopped, the timer stops too.
    h.rt.app.stop_frames(t.ticker.unwrap());
    if let Some(at) = h.rt.next_timer() {
        std::thread::sleep(at.saturating_duration_since(Instant::now()));
    }
    h.rt.poll();
    assert_eq!(h.rt.next_timer(), None);
    assert!(!h.rt.wants_frames());
}
