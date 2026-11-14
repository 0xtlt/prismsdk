//! Turns entity renders into a minimal list of [`Patch`]es.
//!
//! Backends never see elements or closures: they only receive patches and
//! report events by [`NodeId`]. That boundary is what lets the same app run on
//! the DOM (over a WebSocket), AppKit, UIKit, or any future host.
//!
//! ## Entity-granular updates
//!
//! Every entity renders exactly one root node (fragments are wrapped in a
//! `div`), and the reconciler remembers which node each entity owns. When
//! entities are notified, only those are re-rendered, in place, shallowest
//! first. A clean child entity met during a parent's re-render is not rendered
//! or diffed at all: its mounted subtree is moved over as-is.

use std::collections::{HashMap, HashSet};

use crate::a11y::{Accessibility, Role};
use crate::app::{AnyEntity, App, Appearance, EntityId};
use crate::element::{
    BackEvent, ChangeEvent, ClickEvent, Element, Handler, InputEvent, NavigateEvent, NodeKind,
    VNode,
};
use crate::style::Style;

pub type NodeId = u64;

/// A mutation of the host tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Patch {
    /// Light/dark appearance of native chrome.
    SetAppearance {
        appearance: Appearance,
    },
    /// Toggle state (0/1), selected tab index.
    SetValue {
        id: NodeId,
        value: f64,
    },
    /// Icon name (SF Symbol on Apple platforms).
    SetIcon {
        id: NodeId,
        icon: String,
    },
    /// The effective accessibility description (defaults already applied).
    SetAccessibility {
        id: NodeId,
        a11y: Accessibility,
    },
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
    /// A stack was popped natively down to `depth` screens.
    Back(NodeId, usize),
    /// A stack was asked to show `path` (e.g. browser forward).
    Navigate(NodeId, String),
    /// A toggle was flipped (0/1) or a tab selected (index).
    Change(NodeId, f64),
}

/// A mounted host node.
struct MNode {
    kind: NodeKind,
    key: Option<String>,
    /// The entity whose render produced this node as its root.
    entity: Option<AnyEntity>,
    style: Style,
    text: String,
    placeholder: String,
    icon: String,
    value: Option<f64>,
    a11y: Accessibility,
    clickable: bool,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
}

/// A flattened child, ready to diff.
enum Item {
    Node(Box<VNode>, Option<AnyEntity>),
    /// A clean entity: reuse its mounted subtree.
    Cached(AnyEntity),
}

#[derive(Default)]
pub struct Reconciler {
    nodes: HashMap<NodeId, MNode>,
    root: Option<NodeId>,
    entity_roots: HashMap<EntityId, NodeId>,
    next_id: NodeId,
    click: HashMap<NodeId, Handler<ClickEvent>>,
    input: HashMap<NodeId, Handler<InputEvent>>,
    back: HashMap<NodeId, Handler<BackEvent>>,
    navigate: HashMap<NodeId, Handler<NavigateEvent>>,
    change: HashMap<NodeId, Handler<ChangeEvent>>,
    // Per-frame scratch state.
    dirty: HashSet<EntityId>,
    patches: Vec<Patch>,
    destroyed: Vec<NodeId>,
    rendered: usize,
    appearance: Appearance,
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

    pub fn back_handler(&self, id: NodeId) -> Option<Handler<BackEvent>> {
        self.back.get(&id).cloned()
    }

    pub fn navigate_handler(&self, id: NodeId) -> Option<Handler<NavigateEvent>> {
        self.navigate.get(&id).cloned()
    }

    pub fn change_handler(&self, id: NodeId) -> Option<Handler<ChangeEvent>> {
        self.change.get(&id).cloned()
    }

    /// Number of entity renders performed by the last [`render`](Self::render).
    pub fn last_render_count(&self) -> usize {
        self.rendered
    }

    /// Re-renders dirty entities (or everything, on first mount) and returns
    /// the patches to apply.
    pub fn render(&mut self, app: &mut App) -> Vec<Patch> {
        self.dirty = app.take_dirty();
        self.rendered = 0;
        let Some(root_entity) = app.root() else {
            return Vec::new();
        };

        let root_owner = self
            .root
            .and_then(|r| self.nodes.get(&r))
            .and_then(|n| n.entity)
            .map(|e| e.id());
        if root_owner != Some(root_entity.id()) {
            // First mount, or the root entity was swapped.
            let vnode = self.render_entity(app, root_entity);
            let id = self.diff(app, self.root, None, vnode, Some(root_entity));
            if self.root != Some(id) {
                self.root = Some(id);
                self.patches.push(Patch::SetRoot { id });
            }
        }

        // Re-render the remaining dirty entities in place, shallowest first, so
        // an ancestor's re-render handles its dirty descendants in one pass.
        loop {
            let next = self
                .dirty
                .iter()
                .filter_map(|e| self.entity_roots.get(e).map(|n| (self.depth(*n), *e, *n)))
                .min();
            let Some((_, _, node)) = next else { break };
            let entity = self.nodes[&node].entity.expect("entity root is tagged");
            let parent = self.nodes[&node].parent;
            let vnode = self.render_entity(app, entity);
            let new = self.diff(app, Some(node), parent, vnode, Some(entity));
            if new != node {
                self.replace_child(parent, node, new);
            }
        }
        self.dirty.clear();

        let mut patches = std::mem::take(&mut self.patches);
        patches.extend(self.destroyed.drain(..).map(|id| Patch::Destroy { id }));
        if app.appearance() != self.appearance {
            self.appearance = app.appearance();
            patches.push(Patch::SetAppearance {
                appearance: self.appearance,
            });
        }
        patches
    }

    fn replace_child(&mut self, parent: Option<NodeId>, old: NodeId, new: NodeId) {
        match parent {
            Some(p) => {
                let node = self.nodes.get_mut(&p).expect("parent is mounted");
                for c in node.children.iter_mut() {
                    if *c == old {
                        *c = new;
                    }
                }
                self.patches.push(Patch::SetChildren {
                    id: p,
                    children: node.children.clone(),
                });
            }
            None => {
                self.root = Some(new);
                self.patches.push(Patch::SetRoot { id: new });
            }
        }
    }

    fn depth(&self, mut id: NodeId) -> usize {
        let mut depth = 0;
        while let Some(parent) = self.nodes.get(&id).and_then(|n| n.parent) {
            depth += 1;
            id = parent;
        }
        depth
    }

    /// Renders an entity to exactly one root node.
    fn render_entity(&mut self, app: &mut App, entity: AnyEntity) -> Box<VNode> {
        self.dirty.remove(&entity.id());
        self.rendered += 1;
        let mut element = entity.render(app);
        while let Element::Lazy(build) = element {
            element = build(app);
        }
        match element {
            Element::Node(vnode) => vnode,
            other => {
                let mut wrapper = VNode::new(NodeKind::View);
                wrapper.children.push(other);
                Box::new(wrapper)
            }
        }
    }

    fn flatten(&mut self, app: &mut App, element: Element, out: &mut Vec<Item>) {
        match element {
            Element::Empty => {}
            Element::Node(vnode) => out.push(Item::Node(vnode, None)),
            Element::Lazy(build) => {
                let element = build(app);
                self.flatten(app, element, out);
            }
            Element::Fragment(items) => {
                for item in items {
                    self.flatten(app, item, out);
                }
            }
            Element::Entity(entity) => {
                let clean = !self.dirty.contains(&entity.id())
                    && self.entity_roots.contains_key(&entity.id());
                if clean {
                    out.push(Item::Cached(entity));
                } else {
                    let vnode = self.render_entity(app, entity);
                    out.push(Item::Node(vnode, Some(entity)));
                }
            }
        }
    }

    fn owner(&self, id: NodeId) -> Option<EntityId> {
        self.nodes.get(&id).and_then(|n| n.entity).map(|e| e.id())
    }

    fn diff(
        &mut self,
        app: &mut App,
        old: Option<NodeId>,
        parent: Option<NodeId>,
        vnode: Box<VNode>,
        entity: Option<AnyEntity>,
    ) -> NodeId {
        let reusable = old.filter(|o| {
            let m = &self.nodes[o];
            m.kind == vnode.kind && m.key == vnode.key
        });
        if let (Some(old), None) = (old, reusable) {
            self.destroy(old);
        }
        let id = match reusable {
            Some(id) => id,
            None => self.create(vnode.kind, vnode.key.clone()),
        };

        // Ownership: which entity's root this node is.
        let previous_owner = self.owner(id);
        let new_owner = entity.map(|e| e.id());
        if let Some(prev) = previous_owner.filter(|p| Some(*p) != new_owner)
            && self.entity_roots.get(&prev) == Some(&id)
        {
            self.entity_roots.remove(&prev);
        }
        if let Some(e) = new_owner {
            self.entity_roots.insert(e, id);
        }

        let VNode {
            kind,
            style,
            text,
            placeholder,
            icon,
            value,
            a11y,
            children,
            on_click,
            on_input,
            on_back,
            on_navigate,
            on_change,
            ..
        } = *vnode;
        let clickable = kind == NodeKind::Button || on_click.is_some();
        {
            let m = self.nodes.get_mut(&id).expect("node exists");
            m.entity = entity;
            m.parent = parent;
            if m.style != style {
                self.patches.push(Patch::SetStyle {
                    id,
                    style: style.clone(),
                });
                m.style = style;
            }
            if m.text != text {
                self.patches.push(Patch::SetText {
                    id,
                    text: text.clone(),
                });
                m.text = text;
            }
            if m.placeholder != placeholder {
                self.patches.push(Patch::SetPlaceholder {
                    id,
                    text: placeholder.clone(),
                });
                m.placeholder = placeholder;
            }
            if m.clickable != clickable {
                self.patches.push(Patch::SetClickable { id, clickable });
                m.clickable = clickable;
            }
            if m.icon != icon {
                self.patches.push(Patch::SetIcon {
                    id,
                    icon: icon.clone(),
                });
                m.icon = icon;
            }
            if let Some(v) = value
                && m.value != Some(v)
            {
                self.patches.push(Patch::SetValue { id, value: v });
                m.value = Some(v);
            }
        }
        match on_click {
            Some(h) => self.click.insert(id, h),
            None => self.click.remove(&id),
        };
        match on_input {
            Some(h) => self.input.insert(id, h),
            None => self.input.remove(&id),
        };
        match on_back {
            Some(h) => self.back.insert(id, h),
            None => self.back.remove(&id),
        };
        match on_navigate {
            Some(h) => self.navigate.insert(id, h),
            None => self.navigate.remove(&id),
        };
        match on_change {
            Some(h) => self.change.insert(id, h),
            None => self.change.remove(&id),
        };

        // Children.
        let mut items = Vec::new();
        for child in children {
            self.flatten(app, child, &mut items);
        }
        let old_children = std::mem::take(&mut self.nodes.get_mut(&id).unwrap().children);
        let mut slots: Vec<Option<NodeId>> = old_children.iter().copied().map(Some).collect();
        let mut new_children = Vec::with_capacity(items.len());
        for (i, item) in items.into_iter().enumerate() {
            let child = match item {
                Item::Cached(e) => {
                    let found = slots
                        .iter()
                        .position(|s| s.is_some_and(|n| self.owner(n) == Some(e.id())));
                    match found {
                        Some(j) => slots[j].take().unwrap(),
                        None => {
                            // Moved to another parent: render it fresh here.
                            let v = self.render_entity(app, e);
                            self.diff(app, None, Some(id), v, Some(e))
                        }
                    }
                }
                Item::Node(v, e) => {
                    let found = match (&v.key, e) {
                        (Some(key), _) => slots.iter().position(|s| {
                            s.is_some_and(|n| self.nodes[&n].key.as_ref() == Some(key))
                        }),
                        (None, Some(e)) => slots
                            .iter()
                            .position(|s| s.is_some_and(|n| self.owner(n) == Some(e.id()))),
                        (None, None) => None,
                    }
                    .or_else(|| {
                        // Positional match among unkeyed, unowned nodes.
                        slots
                            .get(i)
                            .copied()
                            .flatten()
                            .filter(|n| {
                                let m = &self.nodes[n];
                                m.key.is_none() && m.entity.is_none()
                            })
                            .map(|_| i)
                    });
                    let previous = found.and_then(|j| slots[j].take());
                    self.diff(app, previous, Some(id), v, e)
                }
            };
            new_children.push(child);
        }
        for leftover in slots.into_iter().flatten() {
            self.destroy(leftover);
        }
        if new_children != old_children {
            self.patches.push(Patch::SetChildren {
                id,
                children: new_children.clone(),
            });
        }
        self.nodes.get_mut(&id).unwrap().children = new_children;

        // Accessibility, with defaults resolved here once for every backend.
        let effective = self.effective_a11y(id, a11y, clickable, value);
        let m = self.nodes.get_mut(&id).unwrap();
        if m.a11y != effective {
            self.patches.push(Patch::SetAccessibility {
                id,
                a11y: effective.clone(),
            });
            m.a11y = effective;
        }
        id
    }

    fn effective_a11y(
        &self,
        id: NodeId,
        mut a: Accessibility,
        clickable: bool,
        value: Option<f64>,
    ) -> Accessibility {
        let m = &self.nodes[&id];
        if a.role.is_none() {
            a.role = match m.kind {
                NodeKind::View if clickable => Some(Role::Button),
                kind => kind.default_role(),
            };
        }
        if m.kind == NodeKind::Toggle && a.checked.is_none() {
            a.checked = value.map(|v| v != 0.);
        }
        if a.label.is_none() {
            a.label = match m.kind {
                // A clickable container reads as one button named by its text.
                NodeKind::View if clickable => {
                    Some(self.collect_text(id)).filter(|t| !t.is_empty())
                }
                NodeKind::TextInput if !m.placeholder.is_empty() => Some(m.placeholder.clone()),
                NodeKind::Tab | NodeKind::Screen if !m.text.is_empty() => Some(m.text.clone()),
                _ => None,
            };
        }
        a
    }

    /// The visible text of a subtree, for automatic labels.
    fn collect_text(&self, id: NodeId) -> String {
        fn walk(r: &Reconciler, id: NodeId, out: &mut Vec<String>) {
            let Some(m) = r.nodes.get(&id) else { return };
            if m.a11y.hidden {
                return;
            }
            match m.kind {
                NodeKind::Text | NodeKind::Button if !m.text.trim().is_empty() => out.push(
                    m.a11y
                        .label
                        .clone()
                        .unwrap_or_else(|| m.text.trim().to_string()),
                ),
                _ => m.children.iter().for_each(|c| walk(r, *c, out)),
            }
        }
        let mut parts = Vec::new();
        for c in &self.nodes[&id].children {
            walk(self, *c, &mut parts);
        }
        parts.join(", ")
    }

    fn create(&mut self, kind: NodeKind, key: Option<String>) -> NodeId {
        self.next_id += 1;
        let id = self.next_id;
        self.patches.push(Patch::Create { id, kind });
        self.nodes.insert(
            id,
            MNode {
                kind,
                key,
                entity: None,
                // NaN never compares equal, which forces the first SetStyle.
                style: Style {
                    opacity: f32::NAN,
                    ..Style::default()
                },
                text: String::new(),
                placeholder: String::new(),
                icon: String::new(),
                value: None,
                a11y: Accessibility::default(),
                clickable: false,
                parent: None,
                children: Vec::new(),
            },
        );
        id
    }

    fn destroy(&mut self, id: NodeId) {
        let Some(node) = self.nodes.remove(&id) else {
            return;
        };
        for child in node.children {
            self.destroy(child);
        }
        if let Some(e) = node.entity
            && self.entity_roots.get(&e.id()) == Some(&id)
        {
            self.entity_roots.remove(&e.id());
        }
        self.click.remove(&id);
        self.input.remove(&id);
        self.back.remove(&id);
        self.navigate.remove(&id);
        self.change.remove(&id);
        self.destroyed.push(id);
    }
}
