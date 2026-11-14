//! # prism-core
//!
//! The platform-agnostic heart of Prism. An app is a tree of [`Render`]
//! entities; rendering produces [`Element`]s; the [`Reconciler`] diffs them into
//! [`Patch`]es; a [`Backend`] applies the patches to a real host (DOM, AppKit,
//! UIKit, ...) and feeds [`UiEvent`]s back through a [`Runtime`].

mod app;
mod element;
mod reconcile;
mod router;
mod style;

pub use app::{AnyEntity, App, Context, Entity, EntityId, Render};
pub use element::{
    BackEvent, Button, ClickEvent, Div, Element, FluentBuilder, Handler, InputEvent, IntoElement,
    NavigateEvent, NodeKind, ParentElement, Screen, Stack, Text, TextInput, VNode, button, div,
    screen, stack, text, text_input,
};
pub use reconcile::{NodeId, Patch, Reconciler, UiEvent};
pub use router::{Params, Router, RouterBuilder, go_back, navigate_to, normalize_path};
pub use style::*;

use std::sync::Arc;

/// Builds the initial app state. Called once per window (native) or once per
/// browser session (web), so it must be shareable across threads.
pub type AppInit = Arc<dyn Fn(&mut App) + Send + Sync>;

/// Window/page settings shared by every backend.
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub title: String,
    pub width: f32,
    pub height: f32,
    /// TCP port used by the web backend (`PRISM_PORT` overrides it).
    pub port: u16,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            title: "Prism".into(),
            width: 480.,
            height: 640.,
            port: 3000,
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
        let mut app = App::new();
        app.set_initial_path(path);
        init(&mut app);
        Runtime {
            app,
            reconciler: Reconciler::new(),
        }
    }

    /// Renders what is dirty (everything on first call) and returns the patches.
    pub fn render(&mut self) -> Vec<Patch> {
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
            UiEvent::Navigate(id, path) => {
                if let Some(h) = self.reconciler.navigate_handler(id) {
                    h(&NavigateEvent { path }, &mut self.app);
                }
            }
        }
        self.app.flush_effects();
        if self.app.has_dirty() {
            self.reconciler.render(&mut self.app)
        } else {
            Vec::new()
        }
    }

    /// Entity renders performed by the last render (for tests and profiling).
    pub fn last_render_count(&self) -> usize {
        self.reconciler.last_render_count()
    }
}

/// Everything an app usually needs.
pub mod prelude {
    pub use crate::{
        App, ClickEvent, Context, Entity, FluentBuilder, InputEvent, IntoElement, Params,
        ParentElement, Render, Router, Styled, button, div, go_back, navigate_to, px, relative,
        rgb, rgba, screen, text, text_input,
    };
}

#[cfg(test)]
mod tests;
