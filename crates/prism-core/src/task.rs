//! Async on the UI thread: `cx.spawn`, timers, and background work.
//!
//! Futures run on the main (UI) thread, polled by the [`Runtime`]. They can
//! freely touch app state between `.await`s through [`AsyncApp`]. Blocking
//! work (HTTP, disk, crypto) goes to a thread with [`AsyncApp::background`] and
//! comes back as a future. Backends only have to call `Runtime::poll` when
//! woken (see [`Waker`](crate::Runtime::set_waker)) and when the next timer
//! is due (see `Runtime::next_timer`).
//!
//! ```ignore
//! cx.spawn(async move |this, cx| {
//!     cx.sleep(Duration::from_secs(1)).await;
//!     let body = cx.background(|| std::fs::read_to_string("data.txt")).await;
//!     cx.update_entity(&this, |this, _| this.data = body.ok());
//! });
//! ```

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use crate::app::{App, Context, Entity};

/// Called (from any thread) when a task becomes ready; the backend must then
/// call `Runtime::poll` on the UI thread.
pub type WakeUi = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Shared {
    ready: Mutex<Vec<u64>>,
    wake_ui: Mutex<Option<WakeUi>>,
}

struct TaskWaker {
    id: u64,
    shared: Arc<Shared>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.shared.ready.lock().unwrap().push(self.id);
        let wake = self.shared.wake_ui.lock().unwrap().clone();
        if let Some(wake) = wake {
            wake();
        }
    }
}

type Task = Pin<Box<dyn Future<Output = ()>>>;

/// The UI-thread executor owned by the [`App`].
#[derive(Default)]
pub(crate) struct Executor {
    tasks: RefCell<HashMap<u64, Task>>,
    next_id: Cell<u64>,
    shared: Arc<Shared>,
    timers: RefCell<Vec<(Instant, Waker)>>,
    /// Tasks the platform should keep the app alive for in the background.
    keep_alive: RefCell<std::collections::HashSet<u64>>,
}

impl Executor {
    pub(crate) fn spawn(&self, fut: impl Future<Output = ()> + 'static) -> u64 {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        self.tasks.borrow_mut().insert(id, Box::pin(fut));
        self.shared.ready.lock().unwrap().push(id);
        id
    }

    pub(crate) fn cancel(&self, id: u64) {
        self.tasks.borrow_mut().remove(&id);
        self.keep_alive.borrow_mut().remove(&id);
    }

    pub(crate) fn is_running(&self, id: u64) -> bool {
        self.tasks.borrow().contains_key(&id) || self.shared.ready.lock().unwrap().contains(&id)
    }

    pub(crate) fn wants_background_time(&self) -> bool {
        !self.keep_alive.borrow().is_empty()
    }

    pub(crate) fn set_wake_ui(&self, wake: WakeUi) {
        *self.shared.wake_ui.lock().unwrap() = Some(wake);
    }

    pub(crate) fn has_tasks(&self) -> bool {
        !self.tasks.borrow().is_empty()
    }

    pub(crate) fn next_timer(&self) -> Option<Instant> {
        self.timers.borrow().iter().map(|(t, _)| *t).min()
    }

    /// Fires due timers and returns the ids of ready tasks.
    fn take_ready(&self) -> Vec<u64> {
        let now = Instant::now();
        let mut due = Vec::new();
        self.timers.borrow_mut().retain(|(t, w)| {
            if *t <= now {
                due.push(w.clone());
                false
            } else {
                true
            }
        });
        for w in due {
            w.wake();
        }
        let mut ready = std::mem::take(&mut *self.shared.ready.lock().unwrap());
        ready.sort_unstable();
        ready.dedup();
        ready
    }
}

thread_local! {
    /// The app being polled; valid only while `poll_tasks` runs.
    static CURRENT_APP: Cell<*mut App> = const { Cell::new(std::ptr::null_mut()) };
}

/// Polls every ready task with access to `app`. Returns when nothing is ready.
pub(crate) fn poll_tasks(app: &mut App) {
    let exec = app.executor.clone();
    loop {
        let ready = exec.take_ready();
        if ready.is_empty() {
            break;
        }
        for id in ready {
            let Some(mut task) = exec.tasks.borrow_mut().remove(&id) else {
                continue;
            };
            let waker = Waker::from(Arc::new(TaskWaker {
                id,
                shared: exec.shared.clone(),
            }));
            let previous = CURRENT_APP.with(|c| c.replace(app as *mut App));
            let done = task
                .as_mut()
                .poll(&mut TaskContext::from_waker(&waker))
                .is_ready();
            CURRENT_APP.with(|c| c.set(previous));
            if done {
                exec.keep_alive.borrow_mut().remove(&id);
            } else {
                exec.tasks.borrow_mut().insert(id, task);
            }
            // Deliver notifications and navigation between tasks.
            app.flush_effects();
        }
    }
}

/// A handle to the app usable across `.await` points.
#[derive(Clone)]
pub struct AsyncApp {
    exec: Rc<Executor>,
}

impl AsyncApp {
    /// Runs `f` with the app. Only valid inside a spawned task.
    pub fn update<R>(&self, f: impl FnOnce(&mut App) -> R) -> R {
        let ptr = CURRENT_APP.with(Cell::get);
        assert!(
            !ptr.is_null(),
            "AsyncApp::update called outside of a Prism task"
        );
        // SAFETY: set by `poll_tasks` for the duration of the poll, on this
        // thread, while no other borrow of the app is live.
        f(unsafe { &mut *ptr })
    }

    /// Updates an entity and re-renders it (like a listener). Returns `None`
    /// if the entity was released (e.g. its screen was closed).
    pub fn update_entity<T: 'static, R>(
        &self,
        entity: &Entity<T>,
        f: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> Option<R> {
        self.update(|app| {
            if !app.contains(entity.id()) {
                return None;
            }
            Some(app.update(entity, |this, cx| {
                let r = f(this, cx);
                cx.notify();
                r
            }))
        })
    }

    /// Completes after `duration`.
    pub fn sleep(&self, duration: Duration) -> Sleep {
        Sleep {
            deadline: Instant::now() + duration,
            exec: self.exec.clone(),
        }
    }

    /// Runs blocking work on a background thread; resolves on the UI thread.
    pub fn background<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
    ) -> Background<R> {
        let slot: Arc<Mutex<(Option<R>, Option<Waker>)>> = Arc::new(Mutex::new((None, None)));
        let worker = slot.clone();
        std::thread::spawn(move || {
            let result = work();
            let waker = {
                let mut s = worker.lock().unwrap();
                s.0 = Some(result);
                s.1.take()
            };
            if let Some(w) = waker {
                w.wake();
            }
        });
        Background { slot }
    }
}

/// See [`AsyncApp::sleep`].
pub struct Sleep {
    deadline: Instant,
    exec: Rc<Executor>,
}

impl Future for Sleep {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<()> {
        if Instant::now() >= self.deadline {
            return Poll::Ready(());
        }
        self.exec
            .timers
            .borrow_mut()
            .push((self.deadline, cx.waker().clone()));
        Poll::Pending
    }
}

/// See [`AsyncApp::background`].
pub struct Background<R> {
    slot: Arc<Mutex<(Option<R>, Option<Waker>)>>,
}

impl<R> Future for Background<R> {
    type Output = R;
    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<R> {
        let mut s = self.slot.lock().unwrap();
        match s.0.take() {
            Some(r) => Poll::Ready(r),
            None => {
                s.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// A spawned task. Dropping it lets the task run on; `cancel` stops it.
pub struct TaskHandle {
    id: u64,
    exec: std::rc::Weak<Executor>,
}

impl TaskHandle {
    /// Identifies the task (see `Runtime::is_task_running`).
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Keeps the app running in the background until this task completes
    /// (iOS: `beginBackgroundTask`, about 30 seconds; elsewhere the app
    /// keeps running anyway). Returns the handle.
    pub fn finish_in_background(self) -> Self {
        if let Some(exec) = self.exec.upgrade()
            && exec.tasks.borrow().contains_key(&self.id)
        {
            exec.keep_alive.borrow_mut().insert(self.id);
        }
        self
    }

    pub fn cancel(self) {
        if let Some(exec) = self.exec.upgrade() {
            exec.cancel(self.id);
        }
    }
}

impl App {
    /// Spawns a task on the UI thread.
    pub fn spawn<F: Future<Output = ()> + 'static>(
        &mut self,
        f: impl FnOnce(AsyncApp) -> F,
    ) -> TaskHandle {
        let handle = AsyncApp {
            exec: self.executor.clone(),
        };
        let id = self.executor.spawn(f(handle));
        TaskHandle {
            id,
            exec: Rc::downgrade(&self.executor),
        }
    }
}

impl<T: 'static> Context<'_, T> {
    /// Spawns a task bound to this entity:
    /// `cx.spawn(async move |this, cx| { … cx.update_entity(&this, |t, _| …); })`.
    pub fn spawn<F: Future<Output = ()> + 'static>(
        &mut self,
        f: impl FnOnce(Entity<T>, AsyncApp) -> F,
    ) -> TaskHandle {
        let this = self.entity();
        self.app.spawn(|cx| f(this, cx))
    }
}
