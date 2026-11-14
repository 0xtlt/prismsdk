//! # prism-core
//!
//! The platform-agnostic heart of Prism. An app is a tree of [`Render`]
//! entities; rendering produces [`Element`]s; the [`Reconciler`] diffs them into
//! [`Patch`]es; a [`Backend`] applies the patches to a real host (DOM, AppKit,
//! UIKit, ...) and feeds [`UiEvent`]s back through a [`Runtime`].

mod a11y;
mod app;
mod element;
pub mod i18n;
pub mod list;
pub mod menu;
mod reconcile;
mod router;
pub mod storage;
mod style;
pub mod surfaces;
mod task;

pub use a11y::{Accessibility, Accessible, Role};
pub use app::{AnyEntity, App, Appearance, Context, Entity, EntityId, Environment, Render};
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
        init(&mut app);
        Runtime {
            app,
            reconciler: Reconciler::new(),
        }
    }

    /// Renders what is dirty (everything on first call) and returns the patches.
    pub fn render(&mut self) -> Vec<Patch> {
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        self.reconciler.render(&mut self.app)
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
            self.reconciler.render(&mut self.app)
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
            self.reconciler.render(&mut self.app)
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
        self.reconciler.render(&mut self.app)
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
        self.reconciler.render(&mut self.app)
    }

    /// Entity renders performed by the last render (for tests and profiling).
    pub fn last_render_count(&self) -> usize {
        self.reconciler.last_render_count()
    }
}

/// Everything an app usually needs.
pub mod prelude {
    pub use crate::{
        Accessible, Activity, Alert, App, AsyncApp, ChangeEvent, ClickEvent, Context, DateStyle,
        Entity, FluentBuilder, Handler, InputEvent, IntoElement, Locale, Menu, MenuItem,
        Notification, NotificationAction, NotificationResponse, Params, ParentElement, Render,
        Role, Router, ScrollEvent, Styled, SwipeDirection, Transition, Translations, Tray,
        VirtualList, Widget, WidgetFamily, adaptive, button, div, go_back, image, lazy,
        navigate_to, picture, progress, px, relative, rgb, rgba, screen, scroll, text, text_input,
        timer, toggle,
    };
}

#[cfg(test)]
mod tests;
