//! A mirror of the host tree built from patches only, like a backend would.
//!
//! Applying patches checks the protocol's invariants (nodes are created
//! before use, never used after `Destroy`, have at most one parent), so every
//! test that goes through [`Host`] also checks the reconciler's output is
//! well-formed. [`Host::describe`] prints the mounted tree for golden
//! comparisons.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use prism_core::{
    Accessibility, AppInit, Appearance, Backend, NodeId, NodeKind, Patch, RenderEnvironment,
    Runtime, Style, UiEvent,
};

#[derive(Clone, Debug)]
pub struct HNode {
    pub kind: NodeKind,
    pub text: String,
    pub placeholder: String,
    pub icon: String,
    pub value: Option<f64>,
    pub style: Option<Style>,
    pub a11y: Accessibility,
    pub clickable: bool,
    pub children: Vec<NodeId>,
    pub has_menu: bool,
    pub swipe: (bool, bool),
    pub scroll_events: bool,
    /// Control configuration (`Patch::SetConfig`).
    pub config: String,
}

#[derive(Default)]
pub struct Host {
    pub nodes: HashMap<NodeId, HNode>,
    pub root: Option<NodeId>,
    pub destroyed: HashSet<NodeId>,
    pub env: Option<RenderEnvironment>,
    pub appearance: Option<Appearance>,
}

impl Host {
    fn node(&mut self, id: NodeId, what: &str) -> &mut HNode {
        assert!(
            !self.destroyed.contains(&id),
            "{what} on destroyed node {id}"
        );
        self.nodes
            .get_mut(&id)
            .unwrap_or_else(|| panic!("{what} on unknown node {id}"))
    }

    /// Every node reachable from the root exists, and has one parent.
    pub fn check(&self) {
        let Some(root) = self.root else { return };
        let mut seen = HashSet::new();
        let mut todo = vec![root];
        while let Some(id) = todo.pop() {
            assert!(seen.insert(id), "node {id} is mounted twice");
            let n = self
                .nodes
                .get(&id)
                .unwrap_or_else(|| panic!("mounted node {id} does not exist"));
            todo.extend(n.children.iter().copied());
        }
    }

    pub fn get(&self, id: NodeId) -> &HNode {
        &self.nodes[&id]
    }

    /// Mounted node ids, depth first.
    pub fn mounted(&self) -> Vec<NodeId> {
        fn walk(h: &Host, id: NodeId, out: &mut Vec<NodeId>) {
            out.push(id);
            for c in &h.nodes[&id].children {
                walk(h, *c, out);
            }
        }
        let mut out = Vec::new();
        if let Some(r) = self.root {
            walk(self, r, &mut out);
        }
        out
    }

    /// The first mounted node (depth first) matching `f`.
    pub fn find(&self, f: impl Fn(&HNode) -> bool) -> Option<NodeId> {
        self.mounted().into_iter().find(|id| f(&self.nodes[id]))
    }

    /// The mounted node with this text (panics if none).
    pub fn by_text(&self, text: &str) -> NodeId {
        self.find(|n| n.text == text)
            .unwrap_or_else(|| panic!("no mounted node with text {text:?}\n{}", self.describe()))
    }

    /// The mounted tree, e.g. `view[text"a" button"+"]`.
    pub fn describe(&self) -> String {
        fn walk(h: &Host, id: NodeId, out: &mut String) {
            let n = &h.nodes[&id];
            out.push_str(n.kind.name());
            if !n.text.is_empty() {
                out.push_str(&format!("{:?}", n.text));
            }
            if let Some(v) = n.value {
                out.push_str(&format!("={v}"));
            }
            if !n.children.is_empty() {
                out.push('[');
                for (i, c) in n.children.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    walk(h, *c, out);
                }
                out.push(']');
            }
        }
        let mut out = String::new();
        if let Some(r) = self.root {
            walk(self, r, &mut out);
        }
        out
    }

    /// Texts of the children of `id`.
    pub fn child_texts(&self, id: NodeId) -> Vec<String> {
        self.nodes[&id]
            .children
            .iter()
            .map(|c| self.nodes[c].text.clone())
            .collect()
    }
}

impl Backend for Host {
    fn apply(&mut self, patches: &[Patch]) {
        for p in patches {
            match p {
                Patch::Create { id, kind } => {
                    assert!(!self.nodes.contains_key(id), "node {id} created twice");
                    assert!(!self.destroyed.contains(id), "node id {id} reused");
                    self.nodes.insert(
                        *id,
                        HNode {
                            kind: *kind,
                            text: String::new(),
                            placeholder: String::new(),
                            icon: String::new(),
                            value: None,
                            style: None,
                            a11y: Accessibility::default(),
                            clickable: false,
                            children: Vec::new(),
                            has_menu: false,
                            swipe: (false, false),
                            scroll_events: false,
                            config: String::new(),
                        },
                    );
                }
                Patch::SetStyle { id, style } => {
                    self.node(*id, "SetStyle").style = Some(style.clone())
                }
                Patch::SetText { id, text } => self.node(*id, "SetText").text = text.clone(),
                Patch::SetPlaceholder { id, text } => {
                    self.node(*id, "SetPlaceholder").placeholder = text.clone()
                }
                Patch::SetIcon { id, icon } => self.node(*id, "SetIcon").icon = icon.clone(),
                Patch::SetValue { id, value } => self.node(*id, "SetValue").value = Some(*value),
                Patch::SetClickable { id, clickable } => {
                    self.node(*id, "SetClickable").clickable = *clickable
                }
                Patch::SetAccessibility { id, a11y } => {
                    self.node(*id, "SetAccessibility").a11y = a11y.clone()
                }
                Patch::SetMenu { id, menu } => self.node(*id, "SetMenu").has_menu = menu.is_some(),
                Patch::SetSwipe { id, left, right } => {
                    self.node(*id, "SetSwipe").swipe = (*left, *right)
                }
                Patch::SetScrollEvents { id, on } => {
                    self.node(*id, "SetScrollEvents").scroll_events = *on
                }
                Patch::SetConfig { id, config } => {
                    self.node(*id, "SetConfig").config = config.clone()
                }
                Patch::SetChildren { id, children } => {
                    let unique: HashSet<_> = children.iter().collect();
                    assert_eq!(unique.len(), children.len(), "duplicate child of {id}");
                    for c in children {
                        self.node(*c, "child of SetChildren");
                    }
                    self.node(*id, "SetChildren").children = children.clone();
                }
                Patch::SetRoot { id } => {
                    self.node(*id, "SetRoot");
                    self.root = Some(*id);
                }
                Patch::Destroy { id } => {
                    assert!(self.nodes.remove(id).is_some(), "destroy of unknown {id}");
                    self.destroyed.insert(*id);
                }
                Patch::SetEnvironment { env } => self.env = Some(env.clone()),
                Patch::SetAppearance { appearance } => self.appearance = Some(*appearance),
            }
        }
        // Destroyed nodes are gone from the mounted tree once a frame is applied.
        for id in self.mounted() {
            assert!(
                !self.destroyed.contains(&id),
                "destroyed node {id} is mounted"
            );
        }
        self.check();
    }
}

/// A runtime plus its mirrored host tree.
pub struct Harness {
    pub rt: Runtime,
    pub host: Host,
    /// Patches of the last frame.
    pub last: Vec<Patch>,
}

impl Harness {
    pub fn new(init: impl Fn(&mut prism_core::App) + Send + Sync + 'static) -> Self {
        Self::with_runtime(|init| Runtime::new(init), init)
    }

    pub fn at(path: &str, init: impl Fn(&mut prism_core::App) + Send + Sync + 'static) -> Self {
        let path = path.to_string();
        Self::with_runtime(move |init| Runtime::with_path(init, &path), init)
    }

    pub fn with_runtime(
        make: impl FnOnce(&AppInit) -> Runtime,
        init: impl Fn(&mut prism_core::App) + Send + Sync + 'static,
    ) -> Self {
        let init: AppInit = Arc::new(init);
        let mut rt = make(&init);
        let first = rt.render();
        let mut host = Host::default();
        host.apply(&first);
        Harness {
            rt,
            host,
            last: first,
        }
    }

    /// Applies a frame of patches the runtime returned.
    pub fn apply(&mut self, patches: Vec<Patch>) -> &[Patch] {
        self.host.apply(&patches);
        self.last = patches;
        &self.last
    }

    pub fn render(&mut self) -> &[Patch] {
        let p = self.rt.render();
        self.apply(p)
    }

    pub fn dispatch(&mut self, event: UiEvent) -> &[Patch] {
        let p = self.rt.dispatch(event);
        self.apply(p)
    }

    pub fn click(&mut self, text: &str) -> &[Patch] {
        let id = self.host.by_text(text);
        self.dispatch(UiEvent::Click(id))
    }

    pub fn open_link(&mut self, path: &str) -> &[Patch] {
        let p = self.rt.open_link(path);
        self.apply(p)
    }

    pub fn describe(&self) -> String {
        self.host.describe()
    }

    /// The entity given to [`mount`].
    pub fn root<T: 'static>(&self) -> prism_core::Entity<T> {
        *self.rt.app.global::<prism_core::Entity<T>>()
    }

    /// Updates the root entity's state and renders.
    pub fn update<T: 'static>(&mut self, f: impl FnOnce(&mut T)) -> &[Patch] {
        let root = self.root::<T>();
        self.rt.app.update(&root, |this, cx| {
            f(this);
            cx.notify();
        });
        self.render()
    }
}

/// Makes `entity` the root, and remembers it for [`Harness::root`].
pub fn mount<T: prism_core::Render>(cx: &mut prism_core::App, entity: prism_core::Entity<T>) {
    cx.set_global(entity);
    cx.set_root(entity);
}

pub fn count(patches: &[Patch], f: impl Fn(&Patch) -> bool) -> usize {
    patches.iter().filter(|p| f(p)).count()
}

pub fn creates(patches: &[Patch]) -> usize {
    count(patches, |p| matches!(p, Patch::Create { .. }))
}

pub fn destroys(patches: &[Patch]) -> Vec<NodeId> {
    patches
        .iter()
        .filter_map(|p| match p {
            Patch::Destroy { id } => Some(*id),
            _ => None,
        })
        .collect()
}

/// A deterministic pseudo-random sequence (no dependency, same on every run).
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}
