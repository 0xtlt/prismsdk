//! # prism-core
//!
//! The platform-agnostic heart of Prism. An app is a tree of [`Render`]
//! entities; rendering produces [`Element`]s; the [`Reconciler`] diffs them into
//! [`Patch`]es; a [`Backend`] applies the patches to a real host (DOM, AppKit,
//! UIKit, ...) and feeds [`UiEvent`]s back through a [`Runtime`].

mod a11y;
pub mod animation;
mod app;
mod apple_models;
pub mod background;
pub mod car;
pub mod device;
pub mod device_info;
mod element;
pub mod glyphs;
pub mod i18n;
pub mod list;
pub mod menu;
pub mod platform;
mod reconcile;
mod router;
pub mod storage;
mod style;
pub mod surfaces;
mod task;
mod window;

pub use a11y::{Accessibility, Accessible, Role};
pub use animation::{Animations, Pose, Transform};
pub use app::{AnyEntity, App, Appearance, Context, Entity, EntityId, Environment, Render};
pub use background::{BackgroundKind, BackgroundTask, Lifecycle};
pub use car::{CarButton, CarButtonStyle, CarItem, CarSection, CarTab, CarTemplate};
pub use device::{
    Accuracy, DeviceCall, DeviceError, DeviceEvent, DeviceValue, Haptic, Location, Motion,
    Permission, PermissionState, PickImages, PickedFile, Share, Watch,
};
pub use device_info::{
    Browser, DeviceInfo, DeviceKind, Engine, Fold, Orientation, Os, OsName, Posture, Rotation,
    ScreenInfo, Version, WindowMode,
};
pub use element::{
    BackEvent, Button, ChangeEvent, ClickEvent, Div, Element, FluentBuilder, Handler, Image,
    InputEvent, IntoElement, Modal, NavigateEvent, NodeKind, ParentElement, Picture, Progress,
    Screen, Scroll, ScrollEvent, Stack, SwipeDirection, Tab, Tabs, Text, TextInput, Timer, Toggle,
    VNode, button, div, format_remaining, image, lazy, modal, picture, progress, screen, scroll,
    stack, tab, tabs, text, text_input, timer, toggle,
};
pub use i18n::{BasicFormatter, DateStyle, Direction, Formatter, Locale, Translations};
pub use list::VirtualList;
pub use menu::{ActionRole, Alert, AlertStyle, Menu, MenuItem, Tray};
pub use platform::{Platform, Platforms};
pub use reconcile::{NodeId, Patch, Reconciler, RenderEnvironment, UiEvent};
pub use router::{Params, Router, RouterBuilder, go_back, navigate_to, normalize_path};
pub use storage::{FileStorage, MemoryStorage, Storage};
pub use style::*;
pub use surfaces::{
    Activity, ActivityId, Capabilities, Notification, NotificationAction, NotificationResponse,
    PlatformRequest, PlatformServices, SNode, Widget, WidgetFamily,
};
pub use task::{AsyncApp, Background, Sleep, TaskHandle, WakeUi};

use std::sync::Arc;

/// Builds the initial app state. Called once per window (native) or once per
/// browser session (web), so it must be shareable across threads.
pub type AppInit = Arc<dyn Fn(&mut App) + Send + Sync>;

/// What a backend provides to the app at startup.
#[derive(Default)]
pub struct RuntimeOptions {
    /// Initial path (deep link, request URL).
    pub path: String,
    pub capabilities: Capabilities,
    /// Where `cx.storage()` persists data (in memory if `None`).
    pub storage: Option<Box<dyn Storage>>,
    /// Called from any thread when async work is ready; the backend must then
    /// call [`Runtime::poll`] on the UI thread.
    pub wake_ui: Option<WakeUi>,
    /// System locale, text size, reduce motion.
    pub environment: Environment,
    /// Platform number/date formatting (portable fallback if `None`).
    pub formatter: Option<Box<dyn Formatter>>,
    /// Storage synced between devices (local storage semantics if `None`).
    pub cloud_storage: Option<Box<dyn Storage>>,
    /// Per-user data directory (web: per browser).
    pub data_dir: Option<std::path::PathBuf>,
    /// Data shared by every user (web: the server's directory).
    pub shared_data_dir: Option<std::path::PathBuf>,
    /// Window or viewport size at startup (a phone if unknown).
    pub viewport: Option<(f32, f32)>,
    /// Where the app runs (the compilation target if `None`; the web backend
    /// says `Web`).
    pub platform: Option<Platform>,
    /// What the device is (guessed from the platform and viewport if `None`).
    pub device_info: Option<DeviceInfo>,
}

/// Window/page settings shared by every backend.
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub title: String,
    pub width: f32,
    pub height: f32,
    /// TCP port used by the web backend (`PRISM_PORT` overrides it).
    pub port: u16,
    /// Directory of app assets (`picture("logo.png")`), used in development;
    /// bundles copy it into their resources.
    pub assets: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            title: "Prism".into(),
            width: 480.,
            height: 640.,
            port: 3000,
            assets: None,
        }
    }
}

/// A host that can display a tree described by patches.
pub trait Backend {
    fn apply(&mut self, patches: &[Patch]);
}

/// The app plus its reconciler: everything a backend needs to drive a UI.
pub struct Runtime {
    pub app: App,
    reconciler: Reconciler,
    /// `prism dev`: where the current route is saved, so a restart reopens it.
    dev_state: Option<(std::path::PathBuf, String)>,
}

impl Runtime {
    pub fn new(init: &AppInit) -> Self {
        Self::with_path(init, "/")
    }

    /// Starts the app on `path` (a deep link / the requested URL).
    pub fn with_path(init: &AppInit, path: &str) -> Self {
        Self::with_capabilities(init, path, Capabilities::default())
    }

    /// Like [`Runtime::with_path`], telling the app what the platform supports
    /// before it initializes.
    pub fn with_capabilities(init: &AppInit, path: &str, caps: Capabilities) -> Self {
        Self::with_options(
            init,
            RuntimeOptions {
                path: path.into(),
                capabilities: caps,
                ..Default::default()
            },
        )
    }

    /// Full control over the environment a backend gives the app.
    pub fn with_options(init: &AppInit, options: RuntimeOptions) -> Self {
        let mut app = App::new();
        app.set_initial_path(&options.path);
        app.set_capabilities(options.capabilities);
        if let Some(storage) = options.storage {
            app.set_storage(storage);
        }
        if let Some(wake) = options.wake_ui {
            app.executor.set_wake_ui(wake);
        }
        app.set_environment(options.environment);
        if let Some(formatter) = options.formatter {
            app.set_formatter(formatter);
        }
        if let Some(cloud) = options.cloud_storage {
            app.set_cloud_storage(cloud);
        }
        app.set_data_dirs(options.data_dir, options.shared_data_dir);
        if let Some(platform) = options.platform {
            app.window.platform = platform;
        }
        if let Some(size) = options.viewport {
            app.set_viewport(size);
        }
        if let Some(info) = options.device_info {
            app.set_device_info(info);
        }
        init(&mut app);
        Runtime {
            app,
            reconciler: Reconciler::new(),
            dev_state: std::env::var_os("PRISM_DEV_STATE").map(|p| (p.into(), String::new())),
        }
    }

    /// Renders what is dirty (everything on first call) and returns the patches.
    pub fn render(&mut self) -> Vec<Patch> {
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// Runs the handler for `event`, then re-renders the entities it touched.
    pub fn dispatch(&mut self, event: UiEvent) -> Vec<Patch> {
        match event {
            UiEvent::Click(id) => {
                if let Some(h) = self.reconciler.click_handler(id) {
                    h(&ClickEvent, &mut self.app);
                }
            }
            UiEvent::Input(id, value) => {
                if let Some(h) = self.reconciler.input_handler(id) {
                    h(&InputEvent { value }, &mut self.app);
                }
            }
            UiEvent::Back(id, depth) => {
                if let Some(h) = self.reconciler.back_handler(id) {
                    h(&BackEvent { depth }, &mut self.app);
                }
            }
            UiEvent::Change(id, value) => {
                if let Some(h) = self.reconciler.change_handler(id) {
                    h(&ChangeEvent { value }, &mut self.app);
                }
            }
            UiEvent::MenuSelect(item) => {
                if item >= menu::APP_MENU_BASE {
                    self.app.select_menu(item);
                } else if let Some(h) = self.reconciler.menu_handler(item) {
                    h(&ClickEvent, &mut self.app);
                }
            }
            UiEvent::AlertAction(alert, index) => self.app.alert_action(alert, index),
            UiEvent::Scroll(id, offset, viewport) => {
                if let Some(h) = self.reconciler.scroll_handler(id) {
                    h(&ScrollEvent { offset, viewport }, &mut self.app);
                }
            }
            UiEvent::Swipe(id, dir) => {
                if let Some(h) = self.reconciler.swipe_handler(id, dir) {
                    h(&ClickEvent, &mut self.app);
                }
            }
            UiEvent::Navigate(id, path) => {
                if let Some(h) = self.reconciler.navigate_handler(id) {
                    h(&NavigateEvent { path }, &mut self.app);
                }
            }
        }
        // Tasks spawned by the handler start right away.
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        if self.app.has_dirty() {
            self.render_dirty()
        } else {
            Vec::new()
        }
    }

    /// Runs ready async tasks and due timers, then re-renders what changed.
    /// Backends call it when woken (see `RuntimeOptions::wake_ui`) and at
    /// [`next_timer`](Runtime::next_timer).
    pub fn poll(&mut self) -> Vec<Patch> {
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        if self.app.has_dirty() {
            self.render_dirty()
        } else {
            Vec::new()
        }
    }

    /// When the earliest pending timer is due, if any.
    pub fn next_timer(&self) -> Option<std::time::Instant> {
        self.app.executor.next_timer()
    }

    /// Whether async tasks are alive (e.g. waiting on the network).
    pub fn has_tasks(&self) -> bool {
        self.app.executor.has_tasks()
    }

    /// Whether `background` work (HTTP, database, files) is running on
    /// other threads: backends that cannot be woken directly poll meanwhile.
    pub fn has_background_work(&self) -> bool {
        self.app.executor.has_background_work()
    }

    /// Platform work requested by the app (widgets, Live Activities,
    /// notifications). Backends call this after each render or dispatch.
    pub fn take_requests(&mut self) -> Vec<PlatformRequest> {
        self.app.take_requests()
    }

    /// Opens a deep link (from a widget, Live Activity or notification tap)
    /// and returns the resulting patches.
    pub fn open_link(&mut self, path: &str) -> Vec<Patch> {
        self.app.navigate(path);
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// A car screen (CarPlay, Android Auto) shows the app, or stopped.
    pub fn car_connected(&mut self, connected: bool) -> Vec<Patch> {
        self.app.set_car_connected(connected);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// An item or button of the car's templates was selected (its `id`).
    pub fn car_select(&mut self, id: u64) -> Vec<Patch> {
        self.app.car_select(id);
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// The car went back to screen `depth` (0: the root).
    pub fn car_back(&mut self, depth: usize) -> Vec<Patch> {
        self.app.car_back(depth);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// A notification was tapped or one of its actions chosen (`link`: the
    /// action's or notification's deep link). Returns the resulting patches.
    pub fn notification_response(
        &mut self,
        response: surfaces::NotificationResponse,
        link: Option<String>,
    ) -> Vec<Patch> {
        self.app.notification_response(response, link);
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// Renders dirty entities, then remembers the route for `prism dev`.
    fn render_dirty(&mut self) -> Vec<Patch> {
        let patches = self.reconciler.render(&mut self.app);
        if let Some((file, saved)) = &mut self.dev_state
            && let Some(path) = self.app.current_path()
            && path != saved
        {
            *saved = path.to_string();
            let _ = std::fs::write(file, path);
        }
        patches
    }

    /// A backend's answer to device call `id`. Returns the resulting patches.
    pub fn device_reply(
        &mut self,
        id: u64,
        result: Result<device::DeviceValue, device::DeviceError>,
    ) -> Vec<Patch> {
        self.app.device_reply(id, result);
        self.poll()
    }

    /// A sensor event for stream `id`. Returns the resulting patches.
    pub fn device_event(&mut self, id: u64, event: device::DeviceEvent) -> Vec<Patch> {
        self.app.device_event(id, event);
        self.poll()
    }

    /// The window was resized (re-renders everything when it crosses a
    /// breakpoint). Returns the resulting patches.
    pub fn set_viewport(&mut self, width: f32, height: f32) -> Vec<Patch> {
        self.app.set_viewport((width, height));
        self.poll()
    }

    /// The device changed (rotation, fold, split view, browser report).
    /// Re-renders everything if anything differs. Returns the resulting patches.
    pub fn set_device_info(&mut self, info: DeviceInfo) -> Vec<Patch> {
        self.app.set_device_info(info);
        self.poll()
    }

    /// Another device changed these synced keys. Returns the resulting patches.
    pub fn cloud_changed(&mut self, keys: &[String]) -> Vec<Patch> {
        self.app.cloud_changed(keys);
        self.poll()
    }

    /// Runs ready tasks without rendering: for apps launched in the
    /// background with no UI yet (the first `render` draws everything).
    pub fn poll_headless(&mut self) {
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
    }

    /// The app moved on or off screen. Returns the resulting patches.
    pub fn set_lifecycle(&mut self, phase: Lifecycle) -> Vec<Patch> {
        self.app.set_lifecycle(phase);
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// The platform runs background task `id`. Returns the running task's id
    /// (watch it with [`is_task_running`](Self::is_task_running) and tell the
    /// platform when it is done), or `None` if no such task is registered.
    pub fn run_background_task(&mut self, id: &str) -> Option<u64> {
        let handle = self.app.start_background_task(id)?;
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        Some(handle.id())
    }

    pub fn is_task_running(&self, id: u64) -> bool {
        self.app.executor.is_running(id)
    }

    /// Whether a task asked to finish in the background.
    pub fn wants_background_time(&self) -> bool {
        self.app.executor.wants_background_time()
    }

    /// Stops a task (e.g. the platform's background time expired).
    pub fn cancel_task(&mut self, id: u64) {
        self.app.executor.cancel(id);
    }

    /// Entity renders performed by the last render (for tests and profiling).
    pub fn last_render_count(&self) -> usize {
        self.reconciler.last_render_count()
    }
}

/// Everything an app usually needs.
pub mod prelude {
    pub use crate::{
        Accessible, Accuracy, Activity, Alert, Animations, App, AsyncApp, BackgroundTask,
        Breakpoint, CarButton, CarButtonStyle, CarItem, CarSection, CarTab, CarTemplate,
        ChangeEvent, ClickEvent, Context, DateStyle, DeviceInfo, DeviceKind, Entity, FluentBuilder,
        Handler, Haptic, InputEvent, IntoElement, Lifecycle, Locale, Location, Menu, MenuItem,
        Motion, Notification, NotificationAction, NotificationResponse, Orientation, Params,
        ParentElement, Permission, PermissionState, PickImages, PickedFile, Platform, Pose,
        Posture, Render, Role, Router, ScrollEvent, Share, Styled, SwipeDirection, Transition,
        Translations, Tray, VirtualList, Watch, Widget, WidgetFamily, adaptive, button, div,
        go_back, image, lazy, navigate_to, picture, progress, px, relative, rgb, rgba, screen,
        scroll, text, text_input, timer, toggle,
    };
}

#[cfg(test)]
mod tests;
