//! Frame callbacks for app code (#311): run something once per display
//! refresh, in step with the screen, like the web's `requestAnimationFrame`
//! or `CADisplayLink`.
//!
//! ```ignore
//! // Once, on the next frame (time to first frame, a measurement after layout).
//! cx.on_next_frame(|this, frame, cx| this.shown_at = Some(frame.timestamp));
//! // Every frame until stopped: an auto-scroll, a game loop.
//! self.ticker = Some(cx.frames(|this, frame, cx| {
//!     this.offset = 1.2 * frame.elapsed.as_secs_f32() * 1000.;
//!     cx.scroll_to("list", 0., this.offset, false);
//! }));
//! cx.stop_frames(ticker);
//! ```
//!
//! Backends with a display link drive the callbacks from it (iOS, iPadOS,
//! tvOS, visionOS: `CADisplayLink`; macOS: the screen's display link;
//! Android: `Choreographer`; the web: `requestAnimationFrame`; Linux: the
//! GTK frame clock; Windows: `CompositionTarget.Rendering`). Elsewhere a
//! timer at 60 Hz stands in. Frames pause while the app is in the
//! background ([`Lifecycle::Background`](crate::Lifecycle)) and go on when
//! it comes back.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::app::{App, Context};

/// One refresh of the display, handed to frame callbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    /// When this frame began: the display's refresh (vsync) the callbacks
    /// run for.
    pub timestamp: Instant,
    /// When the frame being prepared now reaches the screen: what the
    /// callbacks change shows then.
    pub target_timestamp: Instant,
    /// Time since this callback's first frame (zero on that frame).
    pub elapsed: Duration,
    /// Time since the previous frame (zero on the first one after the
    /// clock started).
    pub delta: Duration,
}

/// A callback started with [`App::frames`] or [`Context::frames`], to stop
/// with [`App::stop_frames`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameTicker(u64);

/// Returns whether to keep it (false: its entity is gone).
type EveryFrame = Rc<RefCell<dyn FnMut(&FrameInfo, &mut App) -> bool>>;
type NextFrame = Box<dyn FnOnce(&FrameInfo, &mut App)>;

/// The refresh rate of the timer that stands in for a display link.
const FALLBACK_PERIOD: Duration = Duration::from_nanos(16_666_667);

#[derive(Default)]
pub(crate) struct FrameClock {
    next_id: u64,
    once: Vec<NextFrame>,
    /// Each ticker, with its first frame's timestamp once it had one.
    every: Vec<(u64, EveryFrame, Option<Instant>)>,
    /// The previous frame's timestamp, while the clock runs.
    last: Option<Instant>,
    /// The backend calls [`Runtime::frame`](crate::Runtime::frame) from a
    /// display link: no timer stands in.
    native: bool,
    /// The timer standing in for a display link is running.
    fallback: bool,
}

impl FrameClock {
    fn has_callbacks(&self) -> bool {
        !self.once.is_empty() || !self.every.is_empty()
    }
}

impl App {
    /// Runs `f` once, on the next frame the display shows (#311): a
    /// measurement once the first frame is on its way, a step that must
    /// follow the screen. Callbacks added while frames run go to the frame
    /// after.
    pub fn on_next_frame(&mut self, f: impl FnOnce(&FrameInfo, &mut App) + 'static) {
        self.frame_clock.once.push(Box::new(f));
        self.ensure_frame_timer();
    }

    /// Runs `f` on every frame the display shows (#311), until
    /// [`stop_frames`](App::stop_frames): an auto-scroll, a game loop, an
    /// animation [`Transition`](crate::Transition) cannot express. Use
    /// [`FrameInfo::elapsed`] (or `delta`) rather than counting frames:
    /// refresh rates differ (60, 120 Hz) and frames can be skipped.
    pub fn frames(&mut self, mut f: impl FnMut(&FrameInfo, &mut App) + 'static) -> FrameTicker {
        self.add_ticker(Rc::new(RefCell::new(
            move |frame: &FrameInfo, app: &mut App| {
                f(frame, app);
                true
            },
        )))
    }

    /// Stops a callback started with [`frames`](App::frames). The display
    /// link stops with the last one.
    pub fn stop_frames(&mut self, ticker: FrameTicker) {
        self.frame_clock.every.retain(|(id, ..)| *id != ticker.0);
        if !self.frame_clock.has_callbacks() {
            self.frame_clock.last = None;
        }
    }

    fn add_ticker(&mut self, f: EveryFrame) -> FrameTicker {
        let clock = &mut self.frame_clock;
        clock.next_id += 1;
        let id = clock.next_id;
        clock.every.push((id, f, None));
        self.ensure_frame_timer();
        FrameTicker(id)
    }

    /// Whether frame callbacks are waiting and the app is not in the
    /// background.
    pub(crate) fn wants_frames(&self) -> bool {
        self.frame_clock.has_callbacks() && self.lifecycle() != crate::Lifecycle::Background
    }

    pub(crate) fn set_native_frames(&mut self, native: bool) {
        self.frame_clock.native = native;
        self.ensure_frame_timer();
    }

    /// Starts the timer that stands in for a display link when the backend
    /// has none, callbacks wait and the app is on screen. It stops by
    /// itself when they are gone or the app goes to the background.
    pub(crate) fn ensure_frame_timer(&mut self) {
        if self.frame_clock.native || self.frame_clock.fallback || !self.wants_frames() {
            return;
        }
        self.frame_clock.fallback = true;
        self.spawn(async move |cx| {
            let mut next = Instant::now();
            loop {
                next += FALLBACK_PERIOD;
                let now = Instant::now();
                // Late (a long frame, a stalled loop): skip the ticks
                // missed, on the same grid.
                while next < now {
                    next += FALLBACK_PERIOD;
                }
                cx.sleep(next.saturating_duration_since(now)).await;
                // The tick's own time, like a vsync: steady deltas however
                // late the wake-up.
                let at = next;
                let go_on = cx.update(|app| {
                    if app.frame_clock.native || !app.wants_frames() {
                        app.frame_clock.fallback = false;
                        app.frame_clock.last = None;
                        return false;
                    }
                    app.run_frame(at, at + FALLBACK_PERIOD);
                    true
                });
                if !go_on {
                    break;
                }
            }
        });
    }

    /// Runs the callbacks of the frame that began at `timestamp`.
    pub(crate) fn run_frame(&mut self, timestamp: Instant, target_timestamp: Instant) {
        if !self.wants_frames() {
            return;
        }
        let clock = &mut self.frame_clock;
        let delta = clock
            .last
            .map_or(Duration::ZERO, |l| timestamp.saturating_duration_since(l));
        clock.last = Some(timestamp);
        let once = std::mem::take(&mut clock.once);
        let every: Vec<(u64, EveryFrame, Instant)> = clock
            .every
            .iter_mut()
            .map(|(id, f, start)| (*id, f.clone(), *start.get_or_insert(timestamp)))
            .collect();
        for f in once {
            let frame = FrameInfo {
                timestamp,
                target_timestamp,
                elapsed: Duration::ZERO,
                delta,
            };
            f(&frame, self);
        }
        for (id, f, start) in every {
            // Stopped by an earlier callback of this frame.
            if !self.frame_clock.every.iter().any(|(i, ..)| *i == id) {
                continue;
            }
            let frame = FrameInfo {
                timestamp,
                target_timestamp,
                elapsed: timestamp.saturating_duration_since(start),
                delta,
            };
            let keep = (f.borrow_mut())(&frame, self);
            if !keep {
                self.frame_clock.every.retain(|(i, ..)| *i != id);
            }
        }
        if !self.frame_clock.has_callbacks() {
            self.frame_clock.last = None;
        }
    }
}

impl<T: 'static> Context<'_, T> {
    /// Runs `f` with this entity once, on the next frame the display shows
    /// (#311), then re-renders it. Nothing if the entity is released by
    /// then. See [`App::on_next_frame`].
    pub fn on_next_frame(&mut self, f: impl FnOnce(&mut T, &FrameInfo, &mut Context<T>) + 'static) {
        let entity = self.entity();
        self.app.on_next_frame(move |frame, app| {
            if app.contains(entity.id()) {
                app.update(&entity, |this, cx| {
                    f(this, frame, cx);
                    cx.notify();
                });
            }
        });
    }

    /// Runs `f` with this entity on every frame the display shows (#311),
    /// then re-renders it, until [`stop_frames`](App::stop_frames) or the
    /// entity's release. See [`App::frames`].
    pub fn frames(
        &mut self,
        mut f: impl FnMut(&mut T, &FrameInfo, &mut Context<T>) + 'static,
    ) -> FrameTicker {
        let entity = self.entity();
        self.app.add_ticker(Rc::new(RefCell::new(
            move |frame: &FrameInfo, app: &mut App| {
                if !app.contains(entity.id()) {
                    return false;
                }
                app.update(&entity, |this, cx| {
                    f(this, frame, cx);
                    cx.notify();
                });
                true
            },
        )))
    }
}
