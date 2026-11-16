//! Islands: small parts of a page that run in the browser.
//!
//! The rest of a Prism web app runs on the server. An island is a Web
//! Component that runs client-side: written in Rust and compiled to WASM
//! ([`island`], an island crate built by `prism`), or plain JavaScript
//! ([`web_component`]). The server gives it props (JSON) and handles the
//! events it emits:
//!
//! ```
//! use prism_core::prelude::*;
//! use prism_core::{IslandEvent, island};
//!
//! struct Dashboard { points: Vec<f64>, selected: Option<f64> }
//!
//! impl Render for Dashboard {
//!     fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
//!         island("chart")
//!             .prop("points", &self.points)
//!             .on("select", cx.listener(|this, e: &IslandEvent, _| {
//!                 this.selected = e.detail::<f64>();
//!             }))
//!             .h(240.)
//!             // The fallback: the first paint, pages without JS, native apps.
//!             .child(text(format!("{} points", self.points.len())))
//!     }
//! }
//! ```
//!
//! Every backend sees an island as a container (a `View`) holding its
//! children, so native apps show the fallback. On the web it becomes the
//! custom element (`<prism-island-chart>`), its module is loaded the first
//! time a page shows it, and a prop that changes in a render updates the
//! live island in place.

use crate::a11y::{Accessibility, Accessible};
use crate::element::{
    Element, FluentBuilder, Handler, IntoElement, NodeKind, ParentElement, VNode,
};
use crate::style::{Style, Styled};

/// Where an island's custom element is defined.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IslandSource {
    /// A WASM island crate built by `prism` (`[web] islands` in
    /// `Prism.toml`), by name: `"chart"` defines `<prism-island-chart>`.
    Wasm(String),
    /// A JavaScript module that defines the element: an app asset name
    /// (`"time-ago.js"` → `/assets/time-ago.<hash>.js`) or a URL.
    Module(String),
    /// Defined by the page itself (a script added to the head).
    Page,
}

/// An event emitted by an island (a DOM `CustomEvent` on its element).
#[derive(Clone, Debug, PartialEq)]
pub struct IslandEvent {
    /// The event's name (`"select"`).
    pub name: String,
    /// Its `detail`, as JSON (`null` without one).
    pub detail: String,
}

impl IslandEvent {
    /// The detail as `T`, if it parses as one.
    pub fn detail<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        serde_json::from_str(&self.detail).ok()
    }

    /// The detail as a JSON value (`Null` if it does not parse).
    pub fn detail_value(&self) -> serde_json::Value {
        serde_json::from_str(&self.detail).unwrap_or(serde_json::Value::Null)
    }
}

/// What makes a [`VNode`] an island: its custom element, props and events.
pub struct IslandSpec {
    /// The custom element's tag (`prism-island-chart`, `time-ago`).
    pub tag: String,
    /// Where the element is defined.
    pub source: IslandSource,
    /// Props, by name, as JSON.
    pub props: Vec<(String, String)>,
    /// Event handlers, by event name.
    pub events: Vec<(String, Handler<IslandEvent>)>,
}

/// The custom element of a WASM island: `chart` → `prism-island-chart`.
pub fn island_tag(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("prism-island-{}", name.trim_matches('-'))
}

/// A client-side island (see the [module docs](self)).
pub struct Island {
    node: VNode,
    native: Option<Element>,
}

/// A WASM island by name: the crate `islands/chart` declared in
/// `Prism.toml` (`[web] islands = ["islands/chart"]`) becomes
/// `<prism-island-chart>`. Its children are the fallback: the first paint,
/// what shows until the island mounts (it renders into its shadow root),
/// and what native apps show.
pub fn island(name: impl AsRef<str>) -> Island {
    let name = name.as_ref();
    Island::new(island_tag(name), IslandSource::Wasm(name.to_string()))
}

/// A plain JavaScript Web Component, through the same mechanism as
/// [`island`]: props become properties (and attributes, for strings, numbers
/// and booleans), its `CustomEvent`s reach [`on`](Island::on) handlers. Load
/// its definition with [`module`](Island::module), or define it in the page.
/// The tag must contain a `-` (a custom element name).
pub fn web_component(tag: impl Into<String>) -> Island {
    let tag = tag.into().to_ascii_lowercase();
    debug_assert!(
        tag.contains('-'),
        "custom element names contain a `-`: {tag}"
    );
    Island::new(tag, IslandSource::Page)
}

impl Island {
    fn new(tag: String, source: IslandSource) -> Self {
        let mut node = VNode::new(NodeKind::View);
        node.island = Some(Box::new(IslandSpec {
            tag,
            source,
            props: Vec::new(),
            events: Vec::new(),
        }));
        Island { node, native: None }
    }

    fn spec(&mut self) -> &mut IslandSpec {
        self.node.island.as_mut().expect("an island has a spec")
    }

    /// The module defining a [`web_component`]: an app asset name
    /// (`"time-ago.js"`) or a URL. It is imported the first time a page
    /// shows the component.
    pub fn module(mut self, module: impl Into<String>) -> Self {
        self.spec().source = IslandSource::Module(module.into());
        self
    }

    /// A prop, serialized as JSON. Changing it in a later render updates the
    /// live island (no re-mount). A value that cannot be serialized is `null`.
    pub fn prop<T: serde::Serialize + ?Sized>(self, name: impl Into<String>, value: &T) -> Self {
        let json = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
        self.prop_json(name, json)
    }

    /// A prop already encoded as JSON (not checked).
    pub fn prop_json(mut self, name: impl Into<String>, json: impl Into<String>) -> Self {
        let (name, json) = (name.into(), json.into());
        let props = &mut self.spec().props;
        match props.iter_mut().find(|(n, _)| *n == name) {
            Some(p) => p.1 = json,
            None => props.push((name, json)),
        }
        self
    }

    /// Called when the island emits the event `name` (a `CustomEvent`; its
    /// `detail` is in [`IslandEvent::detail`]).
    pub fn on(mut self, name: impl Into<String>, handler: Handler<IslandEvent>) -> Self {
        let name = name.into();
        let events = &mut self.spec().events;
        events.retain(|(n, _)| *n != name);
        events.push((name, handler));
        self
    }

    /// What native apps show instead of the children (which stay the web's
    /// fallback).
    pub fn native(mut self, element: impl IntoElement) -> Self {
        self.native = Some(element.into_element());
        self
    }

    /// Stable identity among siblings (see `Div::key`).
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.node.key = Some(key.into());
        self
    }

    /// A name to address the element from code (see `Div::id`).
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.node.id = Some(id.into());
        self
    }
}

impl Styled for Island {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.node.style
    }
}

impl Accessible for Island {
    fn a11y_mut(&mut self) -> &mut Accessibility {
        &mut self.node.a11y
    }
}

impl FluentBuilder for Island {}

impl ParentElement for Island {
    fn children_mut(&mut self) -> &mut Vec<Element> {
        &mut self.node.children
    }
}

impl IntoElement for Island {
    fn into_element(self) -> Element {
        let Island { mut node, native } = self;
        if let Some(native) = native {
            let fallback = std::mem::take(&mut node.children);
            node.children.push(crate::element::lazy(move |app| {
                if app.platform() == crate::platform::Platform::Web {
                    Element::Fragment(fallback)
                } else {
                    native
                }
            }));
        }
        Element::Node(Box::new(node))
    }
}
