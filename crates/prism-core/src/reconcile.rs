//! Turns successive element trees into a minimal list of [`Patch`]es.
//!
//! Backends never see elements or closures: they only receive patches and
//! report events by [`NodeId`]. That boundary is what lets the same app run on
//! the DOM (over a WebSocket), AppKit, UIKit, or any future host.

use std::collections::HashMap;

use crate::app::App;
use crate::element::{ClickEvent, Element, Handler, InputEvent, NodeKind, VNode};
use crate::style::Style;

pub type NodeId = u64;

/// A mutation of the host tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Patch {
    /// Create a detached node.
    Create {
        id: NodeId,
        kind: NodeKind,
    },
    SetStyle {
        id: NodeId,
        style: Style,
    },
    /// Text content, button label, or text input value.
    SetText {
        id: NodeId,
        text: String,
    },
    SetPlaceholder {
        id: NodeId,
        text: String,
    },
    /// Whether a `View` reacts to clicks (buttons always do).
    SetClickable {
        id: NodeId,
        clickable: bool,
    },
    /// Replace the ordered child list. Children not listed are detached.
    SetChildren {
        id: NodeId,
        children: Vec<NodeId>,
    },
    /// Mount this node as the root of the window/page.
    SetRoot {
        id: NodeId,
    },
    /// The node and its native resources can be freed.
    Destroy {
        id: NodeId,
    },
}

/// An event reported by a backend.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Click(NodeId),
    Input(NodeId, String),
}

/// A fully resolved node (entities rendered, fragments flattened).
struct Node {
    vnode: VNode,
    children: Vec<Node>,
}

struct Mounted {
    id: NodeId,
    kind: NodeKind,
    key: Option<String>,
    style: Style,
    text: String,
    placeholder: String,
    clickable: bool,
    children: Vec<Mounted>,
}

#[derive(Default)]
pub struct Reconciler {
    tree: Option<Mounted>,
    next_id: NodeId,
    click: HashMap<NodeId, Handler<ClickEvent>>,
    input: HashMap<NodeId, Handler<InputEvent>>,
}

impl Reconciler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn click_handler(&self, id: NodeId) -> Option<Handler<ClickEvent>> {
        self.click.get(&id).cloned()
    }

    pub fn input_handler(&self, id: NodeId) -> Option<Handler<InputEvent>> {
        self.input.get(&id).cloned()
    }

    /// Renders the app's root entity and diffs it against the mounted tree.
    pub fn render(&mut self, app: &mut App) -> Vec<Patch> {
        let Some(root) = app.root() else {
            return Vec::new();
        };
        let element = root.render(app);
        let mut nodes = Vec::new();
        resolve(app, element, &mut nodes);
        let root = match nodes.len() {
            1 => nodes.pop().unwrap(),
            _ => Node {
                vnode: wrapper(),
                children: nodes,
            },
        };
        self.reconcile(root)
    }

    fn reconcile(&mut self, root: Node) -> Vec<Patch> {
        self.click.clear();
        self.input.clear();
        let mut patches = Vec::new();
        let mut destroyed = Vec::new();
        let old = self.tree.take();
        let old_id = old.as_ref().map(|m| m.id);
        let new = self.diff(old, root, &mut patches, &mut destroyed);
        if old_id != Some(new.id) {
            patches.push(Patch::SetRoot { id: new.id });
        }
        self.tree = Some(new);
        patches.extend(destroyed.into_iter().map(|id| Patch::Destroy { id }));
        patches
    }

    fn diff(
        &mut self,
        old: Option<Mounted>,
        node: Node,
        patches: &mut Vec<Patch>,
        destroyed: &mut Vec<NodeId>,
    ) -> Mounted {
        let mut m = match old {
            Some(m) if m.kind == node.vnode.kind && m.key == node.vnode.key => m,
            old => {
                if let Some(old) = old {
                    collect_ids(&old, destroyed);
                }
                self.next_id += 1;
                let id = self.next_id;
                patches.push(Patch::Create {
                    id,
                    kind: node.vnode.kind,
                });
                Mounted {
                    id,
                    kind: node.vnode.kind,
                    key: node.vnode.key.clone(),
                    style: Style {
                        opacity: f32::NAN,
                        ..Style::default()
                    }, // forces SetStyle
                    text: String::new(),
                    placeholder: String::new(),
                    clickable: false,
                    children: Vec::new(),
                }
            }
        };
        let Node { vnode, children } = node;
        let id = m.id;

        if m.style != vnode.style {
            patches.push(Patch::SetStyle {
                id,
                style: vnode.style.clone(),
            });
            m.style = vnode.style;
        }
        if m.text != vnode.text {
            patches.push(Patch::SetText {
                id,
                text: vnode.text.clone(),
            });
            m.text = vnode.text;
        }
        if m.placeholder != vnode.placeholder {
            patches.push(Patch::SetPlaceholder {
                id,
                text: vnode.placeholder.clone(),
            });
            m.placeholder = vnode.placeholder;
        }
        let clickable = vnode.kind == NodeKind::Button || vnode.on_click.is_some();
        if m.clickable != clickable {
            patches.push(Patch::SetClickable { id, clickable });
            m.clickable = clickable;
        }
        if let Some(h) = vnode.on_click {
            self.click.insert(id, h);
        }
        if let Some(h) = vnode.on_input {
            self.input.insert(id, h);
        }

        let old_ids: Vec<NodeId> = m.children.iter().map(|c| c.id).collect();
        let mut old_children: Vec<Option<Mounted>> = std::mem::take(&mut m.children)
            .into_iter()
            .map(Some)
            .collect();
        for (i, child) in children.into_iter().enumerate() {
            let matched = match &child.vnode.key {
                Some(key) => old_children
                    .iter()
                    .position(|o| o.as_ref().is_some_and(|o| o.key.as_ref() == Some(key))),
                None => old_children
                    .get(i)
                    .and_then(|o| o.as_ref())
                    .filter(|o| o.key.is_none())
                    .map(|_| i),
            };
            let previous = matched.and_then(|j| old_children[j].take());
            m.children
                .push(self.diff(previous, child, patches, destroyed));
        }
        for leftover in old_children.into_iter().flatten() {
            collect_ids(&leftover, destroyed);
        }
        let new_ids: Vec<NodeId> = m.children.iter().map(|c| c.id).collect();
        if new_ids != old_ids {
            patches.push(Patch::SetChildren {
                id,
                children: new_ids,
            });
        }
        m
    }
}

fn wrapper() -> VNode {
    use crate::element::IntoElement;
    match crate::element::div().into_element() {
        Element::Node(node) => *node,
        _ => unreachable!(),
    }
}

fn collect_ids(m: &Mounted, out: &mut Vec<NodeId>) {
    for c in &m.children {
        collect_ids(c, out);
    }
    out.push(m.id);
}

fn resolve(app: &mut App, element: Element, out: &mut Vec<Node>) {
    match element {
        Element::Empty => {}
        Element::Entity(entity) => {
            let rendered = entity.render(app);
            resolve(app, rendered, out);
        }
        Element::Fragment(items) => {
            for item in items {
                resolve(app, item, out);
            }
        }
        Element::Node(mut vnode) => {
            let mut children = Vec::new();
            for child in std::mem::take(&mut vnode.children) {
                resolve(app, child, &mut children);
            }
            out.push(Node {
                vnode: *vnode,
                children,
            });
        }
    }
}
