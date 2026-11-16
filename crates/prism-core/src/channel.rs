//! Values from threads the app does not own, delivered on the UI thread.
//!
//! A blocking WebSocket client, a file watcher or a device SDK callback runs
//! on its own thread; [`channel`] hands what it receives to a task, which
//! wakes when a value arrives (no polling, no pool thread held):
//!
//! ```ignore
//! let (tx, mut rx) = prism::channel::<Event>();   // tx: Send + Clone
//! std::thread::spawn(move || for ev in socket { if tx.send(ev).is_err() { break } });
//! cx.spawn(async move |this, cx| {
//!     while let Some(ev) = rx.recv().await {      // resolves on the UI thread
//!         cx.update_entity(&this, |this, _| this.apply(ev));
//!     }
//! });
//! ```

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

struct State<T> {
    queue: VecDeque<T>,
    /// The task waiting in [`Receiver::recv`].
    waker: Option<Waker>,
    senders: usize,
    receiver: bool,
}

struct Shared<T>(Mutex<State<T>>);

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        // A panic while holding the lock leaves a consistent queue.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// An unbounded channel whose [`Sender`] works from any thread and whose
/// [`Receiver`] is awaited in a task (`cx.spawn`): sending wakes the app's
/// UI thread, which polls the task. Values arrive in the order sent.
pub fn channel<T: Send>() -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared(Mutex::new(State {
        queue: VecDeque::new(),
        waker: None,
        senders: 1,
        receiver: true,
    })));
    (
        Sender {
            shared: shared.clone(),
        },
        Receiver { shared },
    )
}

/// The sending half of a [`channel`]: `Send` and `Clone`, usable from any
/// thread. The channel closes when every sender is dropped.
pub struct Sender<T> {
    shared: Arc<Shared<T>>,
}

/// [`Sender::send`] failed: the [`Receiver`] was dropped (its task ended or
/// was cancelled). Holds the value back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendError<T>(pub T);

impl<T> std::fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the receiver was dropped")
    }
}

impl<T: std::fmt::Debug> std::error::Error for SendError<T> {}

impl<T> Sender<T> {
    /// Queues `value` and wakes the task awaiting [`Receiver::recv`]. Never
    /// blocks. Fails when the receiver is gone: a thread can stop then.
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let waker = {
            let mut s = self.shared.lock();
            if !s.receiver {
                return Err(SendError(value));
            }
            s.queue.push_back(value);
            s.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
        Ok(())
    }

    /// Whether the receiver was dropped (sending would fail).
    pub fn is_closed(&self) -> bool {
        !self.shared.lock().receiver
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.shared.lock().senders += 1;
        Sender {
            shared: self.shared.clone(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let waker = {
            let mut s = self.shared.lock();
            s.senders -= 1;
            if s.senders > 0 {
                return;
            }
            s.waker.take()
        };
        // The last sender: `recv` ends with `None`.
        if let Some(w) = waker {
            w.wake();
        }
    }
}

impl<T> std::fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sender").finish_non_exhaustive()
    }
}

/// The receiving half of a [`channel`], awaited on the UI thread.
pub struct Receiver<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Receiver<T> {
    /// The next value; `None` once every [`Sender`] is dropped and the
    /// queue is empty.
    pub fn recv(&mut self) -> Recv<'_, T> {
        Recv { rx: self }
    }

    /// The next value if one is queued, without waiting.
    pub fn try_recv(&mut self) -> Option<T> {
        self.shared.lock().queue.pop_front()
    }

    /// Every value queued so far, without waiting.
    pub fn drain(&mut self) -> Vec<T> {
        self.shared.lock().queue.drain(..).collect()
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut s = self.shared.lock();
        s.receiver = false;
        s.waker = None;
        s.queue.clear();
    }
}

impl<T> std::fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Receiver").finish_non_exhaustive()
    }
}

/// The future of [`Receiver::recv`].
#[must_use = "futures do nothing unless awaited"]
pub struct Recv<'a, T> {
    rx: &'a mut Receiver<T>,
}

impl<T> Future for Recv<'_, T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let mut s = self.rx.shared.lock();
        if let Some(v) = s.queue.pop_front() {
            return Poll::Ready(Some(v));
        }
        if s.senders == 0 {
            return Poll::Ready(None);
        }
        s.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}
