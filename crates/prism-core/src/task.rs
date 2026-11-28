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
    /// Signalled when a task becomes ready or `background` work ends, for
    /// [`Executor::wait_for_work`].
    changed: std::sync::Condvar,
    wake_ui: Mutex<Option<WakeUi>>,
    /// `background` work running on other threads.
    in_flight: std::sync::atomic::AtomicUsize,
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
        self.shared.changed.notify_all();
        self.shared.wake_ui();
    }
}

impl Shared {
    /// Asks the backend to call `Runtime::poll` on the UI thread.
    fn wake_ui(&self) {
        let wake = self.wake_ui.lock().unwrap().clone();
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
    /// Tasks cancelled when an entity is released (`cancel_with`).
    owned: RefCell<HashMap<crate::app::EntityId, Vec<u64>>>,
    /// The task being polled, and whether it was cancelled meanwhile (it
    /// is then not put back).
    polling: Cell<Option<(u64, bool)>>,
}

impl Executor {
    /// Ties task `id` to `owner`: releasing the entity cancels it.
    fn own(&self, owner: crate::app::EntityId, id: u64) {
        let mut owned = self.owned.borrow_mut();
        let ids = owned.entry(owner).or_default();
        // Forget finished tasks so long-lived entities don't accumulate ids.
        let polling = self.polling.get().map(|(p, _)| p);
        ids.retain(|t| self.tasks.borrow().contains_key(t) || polling == Some(*t));
        ids.push(id);
    }

    /// Cancels the tasks tied to a released entity.
    pub(crate) fn release_owner(&self, owner: crate::app::EntityId) {
        let ids = self.owned.borrow_mut().remove(&owner);
        for id in ids.into_iter().flatten() {
            self.cancel(id);
        }
    }

    pub(crate) fn spawn(&self, fut: impl Future<Output = ()> + 'static) -> u64 {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        self.tasks.borrow_mut().insert(id, Box::pin(fut));
        self.shared.ready.lock().unwrap().push(id);
        // A task spawned outside `poll_tasks` (from `render`, say, which runs
        // after the tasks were polled) would otherwise wait for the next
        // event (#57). Inside it, its loop picks the new task up.
        if self.polling.get().is_none() {
            self.shared.wake_ui();
        }
        id
    }

    pub(crate) fn cancel(&self, id: u64) {
        if self.tasks.borrow_mut().remove(&id).is_none()
            && self.polling.get().is_some_and(|(p, _)| p == id)
        {
            // Being polled right now (it cancelled itself or released its
            // owner): `poll_tasks` drops it instead of putting it back.
            self.polling.set(Some((id, true)));
        }
        self.keep_alive.borrow_mut().remove(&id);
        // Already woken: it would still count as running until the next poll.
        self.shared.ready.lock().unwrap().retain(|r| *r != id);
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

    /// Whether some task waits to be polled.
    pub(crate) fn has_ready(&self) -> bool {
        !self.shared.ready.lock().unwrap().is_empty()
    }

    pub(crate) fn has_tasks(&self) -> bool {
        !self.tasks.borrow().is_empty()
    }

    pub(crate) fn has_background_work(&self) -> bool {
        self.shared
            .in_flight
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0
    }

    /// Blocks until a task is ready (`true`), or until `deadline` passes or
    /// no `background` work is in flight (`false`). Timers are not waited
    /// for: a task sleeping or awaiting a channel does not hold it up.
    pub(crate) fn wait_for_work(&self, deadline: Instant) -> bool {
        let shared = &self.shared;
        let mut ready = shared.ready.lock().unwrap();
        loop {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            if !ready.is_empty() {
                return true;
            }
            if shared.in_flight.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                return false;
            }
            ready = shared
                .changed
                .wait_timeout(ready, deadline - now)
                .unwrap()
                .0;
        }
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
            let outer = exec.polling.replace(Some((id, false)));
            let done = task
                .as_mut()
                .poll(&mut TaskContext::from_waker(&waker))
                .is_ready();
            CURRENT_APP.with(|c| c.set(previous));
            let cancelled = exec.polling.replace(outer).is_some_and(|(_, c)| c);
            if cancelled {
                exec.keep_alive.borrow_mut().remove(&id);
                drop(task);
            } else if done {
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
    ///
    /// Work runs on a process-wide pool of at most [`background_threads`]
    /// threads ([`DEFAULT_BACKGROUND_THREADS`] unless changed with
    /// [`set_background_threads`] or `PRISM_BACKGROUND_THREADS`); more calls
    /// at once wait in line, in order. Threads start on demand and stop
    /// after a while idle. Work whose task is cancelled before it starts is
    /// skipped.
    pub fn background<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
    ) -> Background<R> {
        let (future, job) = self.background_job(work);
        pool::global().submit(job);
        future
    }

    /// The future and the pool job of [`background`](Self::background).
    fn background_job<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
    ) -> (Background<R>, pool::Job) {
        let slot: Arc<Mutex<(Option<R>, Option<Waker>)>> = Arc::new(Mutex::new((None, None)));
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (worker, skip) = (slot.clone(), cancelled.clone());
        let shared = self.exec.shared.clone();
        shared
            .in_flight
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let job: pool::Job = Box::new(move || {
            // Counted until the end, even if `work` panics.
            let _running = InFlight(shared);
            if skip.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let result = work();
            let waker = {
                let mut s = worker.lock().unwrap_or_else(|e| e.into_inner());
                s.0 = Some(result);
                s.1.take()
            };
            if let Some(w) = waker {
                w.wake();
            }
            // `_running` drops after the wake: a backend that sees no work
            // in flight knows its wake signal is already set.
        });
        (Background { slot, cancelled }, job)
    }
}

/// Counts one `background` job as in flight until dropped.
struct InFlight(Arc<Shared>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0
            .in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        // Work whose task is gone wakes nobody: tell `wait_for_work` it
        // may stop waiting. Taking the lock first means a waiter is either
        // asleep (and notified) or has not read the count yet.
        drop(self.0.ready.lock().unwrap_or_else(|e| e.into_inner()));
        self.0.changed.notify_all();
    }
}

/// Default size of the [`AsyncApp::background`] thread pool.
pub const DEFAULT_BACKGROUND_THREADS: usize = 16;

/// Caps the threads [`AsyncApp::background`] work runs on (process-wide, at
/// least 1). Extra threads stop once the jobs already queued are done. The
/// `PRISM_BACKGROUND_THREADS` environment variable sets it at startup.
pub fn set_background_threads(threads: usize) {
    pool::global().set_max(threads);
}

/// The most threads [`AsyncApp::background`] work runs on at once.
pub fn background_threads() -> usize {
    pool::global().max()
}

/// Runs `job` on the [`AsyncApp::background`] pool, outside any task
/// (answers to an app scheme's requests, #343). A panic in it is caught.
pub(crate) fn run_in_background(job: impl FnOnce() + Send + 'static) {
    pool::global().submit(Box::new(job));
}

/// The bounded, elastic thread pool behind [`AsyncApp::background`].
mod pool {
    use std::collections::VecDeque;
    use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
    use std::time::Duration;

    pub(super) type Job = Box<dyn FnOnce() + Send>;

    struct State {
        queue: VecDeque<Job>,
        /// Threads alive, busy or idle.
        threads: usize,
        idle: usize,
        max: usize,
    }

    pub(super) struct Pool {
        state: Mutex<State>,
        ready: Condvar,
        /// An idle thread waits this long for work, then stops.
        idle_timeout: Duration,
    }

    /// The process-wide pool.
    pub(super) fn global() -> &'static Pool {
        static POOL: OnceLock<Pool> = OnceLock::new();
        POOL.get_or_init(|| {
            let max = std::env::var("PRISM_BACKGROUND_THREADS")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(super::DEFAULT_BACKGROUND_THREADS);
            Pool::new(max, Duration::from_secs(20))
        })
    }

    impl Pool {
        pub(super) fn new(max: usize, idle_timeout: Duration) -> Self {
            Pool {
                state: Mutex::new(State {
                    queue: VecDeque::new(),
                    threads: 0,
                    idle: 0,
                    max: max.max(1),
                }),
                ready: Condvar::new(),
                idle_timeout,
            }
        }

        fn lock(&self) -> MutexGuard<'_, State> {
            self.state.lock().unwrap_or_else(|e| e.into_inner())
        }

        pub(super) fn set_max(&self, max: usize) {
            self.lock().max = max.max(1);
        }

        pub(super) fn max(&self) -> usize {
            self.lock().max
        }

        /// Threads alive (busy or idle).
        #[cfg(test)]
        pub(super) fn threads(&self) -> usize {
            self.lock().threads
        }

        pub(super) fn submit(&'static self, job: Job) {
            let mut s = self.lock();
            s.queue.push_back(job);
            if s.idle > 0 {
                self.ready.notify_one();
            } else if s.threads < s.max {
                s.threads += 1;
                drop(s);
                let spawned = std::thread::Builder::new()
                    .name("prism-background".into())
                    .spawn(move || self.run());
                if spawned.is_err() {
                    self.lock().threads -= 1;
                }
            }
            // Otherwise every thread is busy: the job waits its turn.
        }

        fn run(&self) {
            let mut s = self.lock();
            loop {
                if let Some(job) = s.queue.pop_front() {
                    drop(s);
                    // A panicking job must not take the thread (and the
                    // jobs queued behind it) down; the panic hook reports it.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                    s = self.lock();
                    continue;
                }
                if s.threads > s.max {
                    // The cap was lowered: stop once nothing is queued.
                    break;
                }
                s.idle += 1;
                let (next, wait) = self
                    .ready
                    .wait_timeout(s, self.idle_timeout)
                    .unwrap_or_else(|e| e.into_inner());
                s = next;
                s.idle -= 1;
                if wait.timed_out() && s.queue.is_empty() {
                    break;
                }
            }
            s.threads -= 1;
        }
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
    /// Set when the future is dropped (its task cancelled): work that has
    /// not started yet is skipped.
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl<R> Drop for Background<R> {
    fn drop(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl<R> Future for Background<R> {
    type Output = R;
    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<R> {
        let mut s = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        match s.0.take() {
            Some(r) => Poll::Ready(r),
            None => {
                s.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// A spawned task. Dropping it lets the task run on; `cancel` stops it,
/// and [`cancel_on_release`](Self::cancel_on_release) ties it to its view.
pub struct TaskHandle {
    id: u64,
    exec: std::rc::Weak<Executor>,
    /// The entity that spawned it (`cx.spawn` in a view).
    owner: Option<crate::app::EntityId>,
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

    /// Cancels the task when the entity that spawned it (`cx.spawn` in a
    /// view) is released, e.g. when its screen is closed: a request or a
    /// polling loop then stops instead of noticing at its next
    /// `update_entity`. Background work it waits for that has not started
    /// yet is skipped. No effect on [`App::spawn`] tasks; see
    /// [`cancel_with`](Self::cancel_with). Returns the handle.
    pub fn cancel_on_release(self) -> Self {
        if let (Some(owner), Some(exec)) = (self.owner, self.exec.upgrade()) {
            exec.own(owner, self.id);
        }
        self
    }

    /// Cancels the task when `entity` is released. Returns the handle.
    pub fn cancel_with<T: 'static>(self, entity: &Entity<T>) -> Self {
        if let Some(exec) = self.exec.upgrade() {
            exec.own(entity.id(), self.id);
        }
        self
    }

    /// Stops the task: its future is dropped and never polled again.
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
            owner: None,
        }
    }
}

impl<T: 'static> Context<'_, T> {
    /// Spawns a task bound to this entity:
    /// `cx.spawn(async move |this, cx| { … cx.update_entity(&this, |t, _| …); })`.
    /// It runs on after the entity is released (and `update_entity` then
    /// returns `None`) unless you add
    /// [`.cancel_on_release()`](TaskHandle::cancel_on_release).
    pub fn spawn<F: Future<Output = ()> + 'static>(
        &mut self,
        f: impl FnOnce(Entity<T>, AsyncApp) -> F,
    ) -> TaskHandle {
        let this = self.entity();
        let mut handle = self.app.spawn(|cx| f(this, cx));
        handle.owner = Some(this.id());
        handle
    }
}

#[cfg(test)]
mod pool_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;

    fn local_pool(max: usize) -> &'static pool::Pool {
        Box::leak(Box::new(pool::Pool::new(max, Duration::from_millis(50))))
    }

    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "timed out: {what}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn the_pool_never_runs_more_than_its_cap_and_idle_threads_stop() {
        let pool = local_pool(3);
        let (running, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (tx, rx) = mpsc::channel();
        for _ in 0..12 {
            let (running, peak, tx) = (running.clone(), peak.clone(), tx.clone());
            pool.submit(Box::new(move || {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(10));
                running.fetch_sub(1, Ordering::SeqCst);
                tx.send(()).unwrap();
            }));
            assert!(pool.threads() <= 3);
        }
        for _ in 0..12 {
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert_eq!(
            peak.load(Ordering::SeqCst),
            3,
            "queued jobs wait for a thread"
        );
        wait_until("idle threads stop", || pool.threads() == 0);
        // And it starts again on demand.
        let (tx, rx) = mpsc::channel();
        pool.submit(Box::new(move || tx.send(()).unwrap()));
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn a_panicking_job_does_not_take_the_pool_down() {
        let pool = local_pool(1);
        pool.submit(Box::new(|| panic!("boom (expected by the test)")));
        let (tx, rx) = mpsc::channel();
        pool.submit(Box::new(move || tx.send(7).unwrap()));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 7);
        assert_eq!(pool.threads(), 1);
    }

    #[test]
    fn work_of_a_cancelled_task_that_has_not_started_is_skipped() {
        let app = App::new();
        let cx = AsyncApp {
            exec: app.executor.clone(),
        };
        let pool = local_pool(1);
        // Keep the only thread busy so the next job waits in line.
        let (release, blocked) = mpsc::channel::<()>();
        pool.submit(Box::new(move || {
            let _ = blocked.recv();
        }));
        let ran = Arc::new(AtomicBool::new(false));
        let r = ran.clone();
        let (future, job) = cx.background_job(move || r.store(true, Ordering::SeqCst));
        pool.submit(job);
        assert!(app.executor.has_background_work());
        drop(future); // its task was cancelled
        release.send(()).unwrap();
        wait_until("the skipped job is no longer in flight", || {
            !app.executor.has_background_work()
        });
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[test]
    fn the_global_cap_is_configurable() {
        assert!(background_threads() >= 1);
        let before = background_threads();
        set_background_threads(0);
        assert_eq!(background_threads(), 1, "at least one thread");
        set_background_threads(before);
        assert_eq!(background_threads(), before);
    }
}
