//! Running while the app is not on screen.
//!
//! ```ignore
//! // 1. Know when the app goes to the background (save, pause, …).
//! cx.on_lifecycle(|phase, app| if phase == Lifecycle::Background { save(app) });
//!
//! // 2. Let a task finish after the user leaves the app (iOS gives ~30 s).
//! cx.spawn(async move |this, cx| upload(cx).await).finish_in_background();
//!
//! // 3. Periodic work, even when the app is closed (iOS decides when).
//! cx.register_background_task(
//!     BackgroundTask::refresh("sync").every(Duration::from_secs(15 * 60)),
//!     |cx| async move { sync(&cx).await },
//! );
//! ```
//!
//! | | iOS | macOS | Web | Android | Linux | Windows |
//! |---|---|---|---|---|---|---|
//! | Lifecycle | scene active/inactive/background | app active/inactive/hidden | page visibility | active/inactive/background | window active/inactive/minimized | not yet |
//! | Finish in background | `beginBackgroundTask` | keeps running | keeps running (server) | while the process lives | keeps running | keeps running |
//! | Periodic tasks | `BGTaskScheduler` (app relaunched in the background) | `NSBackgroundActivityScheduler` | timer while a page is open | `JobScheduler` (app started in the background) | timer while the app runs | timer while the app runs |

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

use crate::app::App;
use crate::task::{AsyncApp, TaskHandle};

/// Where the app is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lifecycle {
    /// On screen, receiving input.
    #[default]
    Active,
    /// Visible but interrupted (app switcher, call, another window focused).
    Inactive,
    /// Not visible. On iOS the app is suspended soon after, unless a task asked
    /// to [`finish_in_background`](TaskHandle::finish_in_background).
    Background,
}

/// What kind of background work a task does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundKind {
    /// Short (iOS gives about 30 s): refresh content, sync a little data.
    Refresh,
    /// Longer maintenance (iOS runs it while the device is idle, often
    /// charging): clean-ups, big syncs.
    Processing {
        /// Run only when the network is available.
        requires_network: bool,
        /// Run only while the device is charging.
        requires_power: bool,
    },
}

/// A periodic task the platform runs when it sees fit, even if the app was
/// not running.
#[derive(Clone, Debug, PartialEq)]
pub struct BackgroundTask {
    /// Short id (`sync`). On iOS the full identifier is `<bundle id>.<id>`.
    pub id: String,
    /// Short refresh or longer processing work.
    pub kind: BackgroundKind,
    /// The earliest delay between runs (the platform may wait longer).
    pub every: Duration,
}

impl BackgroundTask {
    /// A short refresh task (default: at most every 15 minutes).
    pub fn refresh(id: impl Into<String>) -> Self {
        BackgroundTask {
            id: id.into(),
            kind: BackgroundKind::Refresh,
            every: Duration::from_secs(15 * 60),
        }
    }

    /// A longer processing task (default: at most daily).
    pub fn processing(id: impl Into<String>) -> Self {
        BackgroundTask {
            id: id.into(),
            kind: BackgroundKind::Processing {
                requires_network: false,
                requires_power: false,
            },
            every: Duration::from_secs(24 * 3600),
        }
    }

    /// The earliest delay between runs (the platform may wait longer).
    pub fn every(mut self, every: Duration) -> Self {
        self.every = every;
        self
    }

    /// Processing tasks only: run when the network is available.
    pub fn requires_network(mut self) -> Self {
        if let BackgroundKind::Processing {
            requires_network, ..
        } = &mut self.kind
        {
            *requires_network = true;
        }
        self
    }

    /// Processing tasks only: run while charging.
    pub fn requires_power(mut self) -> Self {
        if let BackgroundKind::Processing { requires_power, .. } = &mut self.kind {
            *requires_power = true;
        }
        self
    }
}

type Job = Rc<dyn Fn(AsyncApp) -> Pin<Box<dyn Future<Output = ()>>>>;
type LifecycleHandler = Rc<dyn Fn(Lifecycle, &mut App)>;

/// Background state kept by the [`App`].
#[derive(Default)]
pub(crate) struct Background {
    pub lifecycle: Lifecycle,
    pub handlers: Vec<LifecycleHandler>,
    pub tasks: Vec<(BackgroundTask, Job)>,
}

impl App {
    /// Where the app is (on screen, interrupted, in the background).
    pub fn lifecycle(&self) -> Lifecycle {
        self.background.lifecycle
    }

    /// Runs `f` whenever the app changes [`Lifecycle`] phase.
    pub fn on_lifecycle(&mut self, f: impl Fn(Lifecycle, &mut App) + 'static) {
        self.background.handlers.push(Rc::new(f));
    }

    /// Registers periodic background work (call it at startup: the app may
    /// be launched in the background just to run it). `job` gets an
    /// [`AsyncApp`]; the platform is told the task finished when it returns.
    pub fn register_background_task<F: Future<Output = ()> + 'static>(
        &mut self,
        task: BackgroundTask,
        job: impl Fn(AsyncApp) -> F + 'static,
    ) {
        let job: Job = Rc::new(move |cx| Box::pin(job(cx)));
        self.background.tasks.retain(|(t, _)| t.id != task.id);
        self.background.tasks.push((task.clone(), job));
        self.surfaces
            .requests
            .push(crate::PlatformRequest::ScheduleBackgroundTask(task));
    }

    /// The registered background tasks.
    pub fn background_tasks(&self) -> Vec<BackgroundTask> {
        self.background
            .tasks
            .iter()
            .map(|(t, _)| t.clone())
            .collect()
    }

    /// JSON for bundlers (`PRISM_DESCRIBE=background`): the identifiers to
    /// declare in Info.plist.
    pub fn describe_background(&self) -> String {
        let items: Vec<String> = self
            .background
            .tasks
            .iter()
            .map(|(t, _)| {
                format!(
                    r#"{{"id":{},"kind":"{}"}}"#,
                    crate::surfaces::json_str(&t.id),
                    match t.kind {
                        BackgroundKind::Refresh => "refresh",
                        BackgroundKind::Processing { .. } => "processing",
                    }
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    pub(crate) fn set_lifecycle(&mut self, phase: Lifecycle) {
        if self.background.lifecycle == phase {
            return;
        }
        self.background.lifecycle = phase;
        if phase == Lifecycle::Active {
            self.check_widget_configurations();
        }
        for handler in self.background.handlers.clone() {
            handler(phase, self);
        }
    }

    /// Starts background task `id` (the platform decided to run it). Returns
    /// the running task, or `None` if no such task is registered.
    pub(crate) fn start_background_task(&mut self, id: &str) -> Option<TaskHandle> {
        let (task, job) = self
            .background
            .tasks
            .iter()
            .find(|(t, _)| t.id == id)
            .map(|(t, j)| (t.clone(), j.clone()))?;
        // Ask for the next run right away, like platforms expect.
        self.surfaces
            .requests
            .push(crate::PlatformRequest::ScheduleBackgroundTask(task));
        Some(self.spawn(move |cx| job(cx)).finish_in_background())
    }
}
