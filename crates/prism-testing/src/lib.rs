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
    /// Pointer events reported (`Patch::SetPointerEvents`).
    pub pointer_mask: u8,
    /// Touch hold before a drag, in ms (`Patch::SetDragDelay`, #243).
    pub drag_delay_ms: u32,
    /// File types it accepts dropped (`Patch::SetDropTarget`, #111).
    pub drop_types: Option<Vec<String>>,
    /// The payload kind it carries when dragged, and the kinds it takes
    /// dropped (`Patch::SetDragAndDrop`, #251).
    pub dnd: (Option<String>, Vec<String>),
    /// Key events reported: (local, global).
    pub keys: (bool, bool),
    /// Keys taken from the focused control (`consume_keys`, #181).
    pub consumed_keys: Vec<String>,
    /// It reports its frame (`on_layout`, `track_layout`, #192).
    pub reports_layout: bool,
    /// The selection last set by the app (`Patch::SetSelection`, #86).
    pub selection: Option<(usize, usize)>,
    /// Selection changes reported (`Patch::SetSelectEvents`).
    pub select_events: bool,
    /// A screen asks before the browser leaves it (`Patch::SetBackRequests`, #324).
    pub back_requests: bool,
    /// Other people's carets drawn in it (`Patch::SetRemoteCarets`, #240).
    pub remote_carets: Vec<prism_core::RemoteCaret>,
    /// An island's tag (`Patch::SetIsland`).
    pub island: Option<String>,
    /// An island's props, as JSON (`Patch::SetIslandProps`).
    pub island_props: std::collections::BTreeMap<String, String>,
    /// Where a link leads (`Patch::SetLink`).
    pub link: Option<String>,
    /// What a click copies (`Patch::SetCopyOnClick`, #323).
    pub copy_on_click: Option<String>,
    /// App-defined attributes (`Patch::SetAttributes`).
    pub attributes: Vec<(String, String)>,
    /// A rich input's encoded runs (`Patch::SetRichValue`, #115).
    pub runs: String,
    /// The editing events it reports (`Patch::SetEditEvents`, #217).
    pub edit_events: u8,
    /// Its highlighted part of a selection across fields
    /// (`Patch::SetSelectionHighlight`, #231).
    pub highlight: Option<(usize, usize)>,
}

#[derive(Default)]
pub struct Host {
    pub nodes: HashMap<NodeId, HNode>,
    pub root: Option<NodeId>,
    pub destroyed: HashSet<NodeId>,
    pub env: Option<RenderEnvironment>,
    pub appearance: Option<Appearance>,
    /// Runtime-mounted layers (`Patch::SetLayers`).
    pub layers: Vec<NodeId>,
    /// Nodes given the focus, in order (`Patch::Focus`).
    pub focused: Vec<NodeId>,
    /// Nodes scrolled into view, in order (`Patch::ScrollIntoView`).
    pub scrolled_to: Vec<NodeId>,
    /// How many times the keyboard was dismissed (`Patch::DismissKeyboard`).
    pub keyboard_dismissals: usize,
    /// Messages posted to web views, in order: (node, JSON) (`Patch::PostWebMessage`).
    pub web_messages: Vec<(NodeId, String)>,
    /// Web views reloaded, in order (`Patch::ReloadWebView`).
    pub web_reloads: Vec<NodeId>,
    /// Pop-up menus opened, in order: (node, menu, point) (`Patch::ShowMenu`, #247).
    pub popups: Vec<(
        Option<NodeId>,
        prism_core::menu::MenuSpec,
        Option<prism_core::Point>,
    )>,
    /// What was written to the clipboard, in order (`Patch::WriteClipboard`, #217).
    pub clipboard: Vec<(NodeId, prism_core::ClipboardContent)>,
    /// Texts inserted in fields, in order (`Patch::InsertText`, #217).
    pub inserted: Vec<(NodeId, String)>,
    /// The selection across fields last sent (`Patch::SetSelectionAcross`,
    /// #231).
    pub across: Option<((NodeId, usize), (NodeId, usize))>,
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
        todo.extend(self.layers.iter().copied());
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

    /// Mounted node ids, depth first (the root, then the layers).
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
        for l in &self.layers {
            walk(self, *l, &mut out);
        }
        out
    }

    /// The first mounted node (depth first) matching `f`.
    pub fn find(&self, f: impl Fn(&HNode) -> bool) -> Option<NodeId> {
        self.mounted().into_iter().find(|id| f(&self.nodes[id]))
    }

    /// The chip of rich input `id` whose test id is `test_id`
    /// (`Span::chip_test_id`, #329): its character offset in the value
    /// and its run. Finds a chip without counting its place.
    pub fn chip(&self, id: NodeId, test_id: &str) -> Option<(usize, prism_core::TextRun)> {
        let mut at = 0;
        for r in prism_core::decode_runs(&self.get(id).runs) {
            if r.chip_test_id.as_deref() == Some(test_id) {
                return Some((at, r));
            }
            at += r.text.chars().count();
        }
        None
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
                            pointer_mask: 0,
                            drag_delay_ms: 0,
                            drop_types: None,
                            highlight: None,
                            dnd: (None, Vec::new()),
                            keys: (false, false),
                            consumed_keys: Vec::new(),
                            reports_layout: false,
                            selection: None,
                            select_events: false,
                            back_requests: false,
                            remote_carets: Vec::new(),
                            island: None,
                            island_props: Default::default(),
                            link: None,
                            copy_on_click: None,
                            attributes: Vec::new(),
                            runs: String::new(),
                            edit_events: 0,
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
                Patch::SetPointerEvents { id, mask } => {
                    self.node(*id, "SetPointerEvents").pointer_mask = *mask
                }
                Patch::SetDragDelay { id, ms } => {
                    self.node(*id, "SetDragDelay").drag_delay_ms = *ms
                }
                Patch::SetDropTarget { id, types } => {
                    self.node(*id, "SetDropTarget").drop_types = types.clone()
                }
                Patch::SetDragAndDrop {
                    id, kind, accepts, ..
                } => self.node(*id, "SetDragAndDrop").dnd = (kind.clone(), accepts.clone()),
                Patch::SetKeyEvents { id, local, global } => {
                    self.node(*id, "SetKeyEvents").keys = (*local, *global)
                }
                Patch::SetConsumedKeys { id, keys } => {
                    self.node(*id, "SetConsumedKeys").consumed_keys = keys.clone()
                }
                Patch::SetLayoutEvents { id, on } => {
                    self.node(*id, "SetLayoutEvents").reports_layout = *on
                }
                Patch::ScrollTo { id, .. } => {
                    self.node(*id, "ScrollTo");
                }
                Patch::SetSelection { id, start, end } => {
                    self.node(*id, "SetSelection").selection = Some((*start, *end))
                }
                Patch::SetBackRequests { id, on } => {
                    self.node(*id, "SetBackRequests").back_requests = *on
                }
                Patch::SetSelectEvents { id, on } => {
                    self.node(*id, "SetSelectEvents").select_events = *on
                }
                Patch::SetRemoteCarets { id, carets } => {
                    self.node(*id, "SetRemoteCarets").remote_carets = carets.clone()
                }
                Patch::Focus { id } => {
                    self.node(*id, "Focus");
                    self.focused.push(*id);
                }
                Patch::ScrollIntoView { id } => {
                    self.node(*id, "ScrollIntoView");
                    self.scrolled_to.push(*id);
                }
                Patch::DismissKeyboard => self.keyboard_dismissals += 1,
                Patch::ShowMenu { id, menu, at } => {
                    if let Some(id) = id {
                        self.node(*id, "ShowMenu");
                    }
                    self.popups.push((*id, menu.clone(), *at));
                }
                Patch::SetLayers { ids } => {
                    for id in ids {
                        self.node(*id, "SetLayers");
                    }
                    self.layers = ids.clone();
                }
                Patch::SetIsland { id, tag, .. } => {
                    self.node(*id, "SetIsland").island = Some(tag.clone())
                }
                Patch::SetScene { id, .. } => {
                    self.node(*id, "SetScene");
                }
                Patch::SetLink { id, href } => self.node(*id, "SetLink").link = href.clone(),
                Patch::SetCopyOnClick { id, text } => {
                    self.node(*id, "SetCopyOnClick").copy_on_click = text.clone()
                }
                Patch::PostWebMessage { id, data } => {
                    self.node(*id, "PostWebMessage");
                    self.web_messages.push((*id, data.clone()));
                }
                Patch::ReloadWebView { id } => {
                    self.node(*id, "ReloadWebView");
                    self.web_reloads.push(*id);
                }
                Patch::SetIslandProps { id, props } => {
                    let n = self.node(*id, "SetIslandProps");
                    for (k, v) in props {
                        if v == "null" {
                            n.island_props.remove(k);
                        } else {
                            n.island_props.insert(k.clone(), v.clone());
                        }
                    }
                }
                Patch::SetAttributes { id, attributes } => {
                    self.node(*id, "SetAttributes").attributes = attributes.clone()
                }
                Patch::SetRichValue { id, runs } => {
                    self.node(*id, "SetRichValue").runs = runs.clone()
                }
                Patch::SetEditEvents { id, mask } => {
                    self.node(*id, "SetEditEvents").edit_events = *mask
                }
                Patch::WriteClipboard { id, content } => {
                    self.clipboard.push((*id, content.clone()))
                }
                Patch::InsertText { id, text } => self.inserted.push((*id, text.clone())),
                Patch::SetSelectionHighlight { id, start, end } => {
                    self.node(*id, "SetSelectionHighlight").highlight =
                        (start != end).then_some((*start, *end))
                }
                Patch::SetSelectionAcross { range, .. } => self.across = *range,
                Patch::SetSelectionGroup { id, .. } => {
                    self.node(*id, "SetSelectionGroup");
                }
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

    /// The mounted node named `name` (`.id(name)`).
    pub fn by_name(&self, name: &str) -> NodeId {
        self.rt
            .node_named(name)
            .unwrap_or_else(|| panic!("no node named {name:?}"))
    }

    /// Drags draggable `source` over `target` and releases it there at `at`
    /// (in the target), like a backend would (#251): `on_drag_over` hears
    /// it enter and leave, `on_drop_payload` the drop.
    pub fn drag_payload_to(
        &mut self,
        source: NodeId,
        target: NodeId,
        at: prism_core::Point,
    ) -> &[Patch] {
        assert!(
            self.host.nodes[&source].dnd.0.is_some(),
            "node {source} is not draggable"
        );
        let event = |phase| prism_core::DragEvent {
            phase,
            start: prism_core::Point::new(0., 0.),
            translation: prism_core::Point::new(10., 10.),
            button: prism_core::PointerButton::Primary,
            modifiers: Default::default(),
        };
        let over = Some(prism_core::DropPoint {
            target,
            position: at,
        });
        for phase in [prism_core::DragPhase::Start, prism_core::DragPhase::Move] {
            self.dispatch(UiEvent::PayloadDrag(source, event(phase), over));
        }
        self.dispatch(UiEvent::PayloadDrag(
            source,
            event(prism_core::DragPhase::End),
            over,
        ))
    }

    /// Drops `files` on drop target `id`, like the platform would (#111).
    pub fn drop_files(&mut self, id: NodeId, files: Vec<prism_core::PickedFile>) -> &[Patch] {
        assert!(
            self.host.nodes[&id].drop_types.is_some(),
            "node {id} is not a drop target"
        );
        self.dispatch(UiEvent::DropFiles(id, files))
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
