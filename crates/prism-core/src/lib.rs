//! # prism-core
//!
//! The platform-agnostic heart of Prism. An app is a tree of [`Render`]
//! entities; rendering produces [`Element`]s; the [`Reconciler`] diffs them into
//! [`Patch`]es; a [`Backend`] applies the patches to a real host (DOM, AppKit,
//! UIKit, ...) and feeds [`UiEvent`]s back through a [`Runtime`].

mod app;
mod element;
mod reconcile;
mod style;

pub use app::{AnyEntity, App, Context, Entity, EntityId, Render};
pub use element::{
    Button, ClickEvent, Div, Element, FluentBuilder, Handler, InputEvent, IntoElement, NodeKind,
    ParentElement, Text, TextInput, VNode, button, div, text, text_input,
};
pub use reconcile::{NodeId, Patch, Reconciler, UiEvent};
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
        let mut app = App::new();
        init(&mut app);
        Runtime {
            app,
            reconciler: Reconciler::new(),
        }
    }

    /// Renders unconditionally and returns the patches.
    pub fn render(&mut self) -> Vec<Patch> {
        self.app.take_dirty();
        self.reconciler.render(&mut self.app)
    }

    /// Runs the handler for `event`, then re-renders if any entity notified.
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
        }
        if self.app.take_dirty() {
            self.reconciler.render(&mut self.app)
        } else {
            Vec::new()
        }
    }
}

/// Everything an app usually needs.
pub mod prelude {
    pub use crate::{
        App, ClickEvent, Context, Entity, FluentBuilder, InputEvent, IntoElement, ParentElement,
        Render, Styled, button, div, px, relative, rgb, rgba, text, text_input,
    };
}

#[cfg(test)]
mod tests;
