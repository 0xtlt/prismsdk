//! prism-ui components and charts, rendered through a real runtime into a
//! mirrored host tree (`prism_testing`).

mod charts;
mod components;

use prism_core::prelude::*;
use prism_core::{Element, NodeId};
use prism_testing::{HNode, Harness, mount};
use prism_ui::Theme;

/// A page rendering `render` with some state handlers can change.
pub struct Page {
    pub on: bool,
    pub log: Vec<String>,
    render: fn(&mut Page, &mut Context<Page>) -> Element,
}

impl Render for Page {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        (self.render)(self, cx)
    }
}

/// Renders `render` with `theme` installed.
pub fn show(theme: fn() -> Theme, render: fn(&mut Page, &mut Context<Page>) -> Element) -> Harness {
    Harness::new(move |cx| {
        prism_ui::set_theme(cx, theme());
        let page = cx.new_entity(|_| Page {
            on: false,
            log: Vec::new(),
            render,
        });
        mount(cx, page);
    })
}

/// The mounted node whose accessibility label is `label`.
pub fn labelled(h: &Harness, label: &str) -> NodeId {
    h.host
        .find(|n| n.a11y.label.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("nothing labelled {label:?}\n{}", h.describe()))
}

pub fn node(h: &Harness, id: NodeId) -> &HNode {
    h.host.get(id)
}

/// What assistive technologies see: (role, label) of every node that is not
/// hidden and has a role or a label.
pub fn a11y_tree(h: &Harness) -> Vec<(Option<Role>, String)> {
    fn walk(h: &Harness, id: NodeId, out: &mut Vec<(Option<Role>, String)>) {
        let n = h.host.get(id);
        if n.a11y.hidden {
            return;
        }
        if n.a11y.label.is_some() || n.a11y.role.is_some_and(|r| r != Role::Text) {
            out.push((n.a11y.role, n.a11y.label.clone().unwrap_or_default()));
        }
        for c in &n.children {
            walk(h, *c, out);
        }
    }
    let mut out = Vec::new();
    if let Some(r) = h.host.root {
        walk(h, r, &mut out);
    }
    out
}

pub fn log(h: &Harness) -> Vec<String> {
    h.root::<Page>().read(&h.rt.app).log.clone()
}
