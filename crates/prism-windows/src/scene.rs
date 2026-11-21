//! The model of a window's tree, kept beside its XAML: one node per Prism
//! view with its kind, frame, style and state. `widgets.rs` drives it from
//! prism-native's `Widgets` calls; it tells which page a stack or tab bar
//! shows, where a node is in the window (drags, overlays), and finds nodes
//! by label for the smoke hooks.
//!
//! No Win32 types: the pages shown, hit testing and the label search are
//! tested on every host.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::{Accessibility, IdMap, Interactions, NodeId, NodeKind, Style};

/// Height of a navigation stack's header bar, view units (DIPs).
pub const HEADER: f32 = 48.;
/// Height of a tab bar (Fluent `NavigationView` in top mode), DIPs.
pub const TAB_BAR: f32 = 48.;

/// A rectangle in view units (DIPs).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
}

/// One view.
#[derive(Clone, Debug)]
pub struct Node {
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    /// In paint order (prism-native sorts them by `z_index`).
    pub children: Vec<NodeId>,
    /// In the parent's coordinates (its content's, for a scroll view).
    pub frame: Rect,
    /// The effective style from prism-native (zoomed, leading = left,
    /// `Color::current` resolved), in the node's interaction states.
    pub style: Style,
    pub text: String,
    pub placeholder: String,
    /// The raw config: a text's runs, a control's options.
    pub config: String,
    pub value: f64,
    pub icon: String,
    /// `display: none`.
    pub hidden: bool,
    pub clickable: bool,
    pub a11y: Accessibility,
    /// A scroll view's content size and offset.
    pub content: (f32, f32),
    pub scroll: (f32, f32),
    /// Sticky in that scroll view.
    pub sticky: Option<(NodeId, prism_native::Sticky)>,
    /// States prism-native wants reported (`Widgets::set_interactions`).
    pub reports: Interactions,
    /// Has the keyboard focus.
    pub focused: bool,
    /// `UiEvent::Key` while focused (`local`) or always (`global`).
    pub keys_local: bool,
    pub keys_global: bool,
    /// `prism_core::events::pointer_mask` bits.
    pub pointer_mask: u8,
    pub menu: Option<prism_core::menu::MenuSpec>,
    /// A shape's paths, in its own points.
    pub paths: Vec<prism_core::Path>,
    pub scroll_events: bool,
}

impl Node {
    pub fn new(kind: NodeKind) -> Node {
        Node {
            kind,
            parent: None,
            children: Vec::new(),
            frame: Rect::default(),
            style: Style::default(),
            text: String::new(),
            placeholder: String::new(),
            config: String::new(),
            value: 0.,
            icon: String::new(),
            hidden: false,
            clickable: false,
            a11y: Accessibility::default(),
            content: (0., 0.),
            scroll: (0., 0.),
            sticky: None,
            reports: Interactions::NONE,
            focused: false,
            keys_local: false,
            keys_global: false,
            pointer_mask: 0,
            menu: None,
            paths: Vec::new(),
            scroll_events: false,
        }
    }

    pub fn is_text_field(&self) -> bool {
        matches!(
            self.kind,
            NodeKind::TextInput | NodeKind::TextArea | NodeKind::RichInput
        )
    }

    /// Whether the scroll view scrolls this way.
    pub fn scrolls(&self, vertical: bool) -> bool {
        self.kind == NodeKind::Scroll
            && (self.style.scroll_axis == prism_core::ScrollAxis::Vertical) == vertical
    }

    /// Children are clipped to the box (`overflow: hidden`, scroll views,
    /// a stack's screens).
    pub fn clips(&self) -> bool {
        self.style.clip
            || matches!(
                self.kind,
                NodeKind::Scroll | NodeKind::Stack | NodeKind::Tabs | NodeKind::TextArea
            )
    }

    pub fn max_scroll(&self) -> (f32, f32) {
        (
            (self.content.0 - self.frame.w).max(0.),
            (self.content.1 - self.frame.h).max(0.),
        )
    }

    /// The tab bar's selected index (a `Tabs` node's value).
    pub fn selected(&self) -> usize {
        self.value.max(0.) as usize
    }
}

/// What `PRISM_CLICK` (and UI Automation's Invoke) found by its label.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// A button, a clickable view, a control: clicked like with the mouse.
    Node(NodeId),
    /// Tab `index` of a tab bar.
    Tab(NodeId, usize),
    /// The Back button of a stack's header.
    Back(NodeId),
    /// Bar button `index` of a screen (#30).
    BarItem(NodeId, usize),
}

/// Every node of a window, by id.
#[derive(Default)]
pub struct Scene {
    pub nodes: IdMap<NodeId, Node>,
    pub root: Option<NodeId>,
    /// Runtime layers (toasts…), bottom first: above the root.
    pub layers: Vec<NodeId>,
}

impl Scene {
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    pub fn create(&mut self, id: NodeId, kind: NodeKind) {
        self.nodes.insert(id, Node::new(kind));
    }

    /// Forgets a node (its children are destroyed by their own calls).
    pub fn remove(&mut self, id: NodeId) -> Option<Node> {
        let node = self.nodes.remove(&id)?;
        if let Some(p) = node.parent
            && let Some(parent) = self.nodes.get_mut(&p)
        {
            parent.children.retain(|&c| c != id);
        }
        for c in &node.children {
            if let Some(child) = self.nodes.get_mut(c)
                && child.parent == Some(id)
            {
                child.parent = None;
            }
        }
        if self.root == Some(id) {
            self.root = None;
        }
        self.layers.retain(|&l| l != id);
        Some(node)
    }

    pub fn set_children(&mut self, parent: NodeId, children: &[NodeId]) {
        let old = match self.nodes.get(&parent) {
            Some(p) => p.children.clone(),
            None => return,
        };
        for c in old {
            if !children.contains(&c)
                && let Some(n) = self.nodes.get_mut(&c)
                && n.parent == Some(parent)
            {
                n.parent = None;
            }
        }
        for &c in children {
            // Moved from another parent: leaves it.
            let previous = self.nodes.get(&c).and_then(|n| n.parent);
            if let Some(p) = previous
                && p != parent
                && let Some(pn) = self.nodes.get_mut(&p)
            {
                pn.children.retain(|&x| x != c);
            }
            if let Some(n) = self.nodes.get_mut(&c) {
                n.parent = Some(parent);
            }
        }
        if let Some(p) = self.nodes.get_mut(&parent) {
            p.children = children.to_vec();
        }
    }

    /// The children drawn and hit: a stack's top screen, a tab bar's
    /// selected tab (and the modal sheets they present), an open
    /// overlay's content; every child otherwise.
    pub fn shown_children(&self, id: NodeId) -> std::borrow::Cow<'_, [NodeId]> {
        use std::borrow::Cow;
        let Some(n) = self.nodes.get(&id) else {
            return Cow::Borrowed(&[]);
        };
        match n.kind {
            NodeKind::Stack | NodeKind::Tabs => {
                let pages = self.pages(id);
                let page = if n.kind == NodeKind::Stack {
                    pages.last().copied()
                } else {
                    pages.get(n.selected()).or(pages.last()).copied()
                };
                let modals = n
                    .children
                    .iter()
                    .copied()
                    .filter(|c| self.nodes.get(c).is_some_and(|c| c.kind == NodeKind::Modal));
                Cow::Owned(page.into_iter().chain(modals).collect())
            }
            // Drawn above everything by `open_overlays()`, not in place.
            NodeKind::Overlay => Cow::Borrowed(&[]),
            _ => Cow::Borrowed(&n.children),
        }
    }

    /// A stack's screens or a tab bar's tabs (not the sheets they present).
    pub fn pages(&self, id: NodeId) -> Vec<NodeId> {
        self.nodes.get(&id).map_or_else(Vec::new, |n| {
            n.children
                .iter()
                .copied()
                .filter(|c| self.nodes.get(c).is_some_and(|c| c.kind != NodeKind::Modal))
                .collect()
        })
    }

    /// Whether a stack shows its header bar: not turned off
    /// (`STACK_NO_HEADER`), and its top screen is not bare (value 1).
    pub fn header_shown(&self, stack: NodeId) -> bool {
        let Some(n) = self.nodes.get(&stack) else {
            return false;
        };
        if (n.value as u32) & prism_core::STACK_NO_HEADER != 0 {
            return false;
        }
        !self
            .pages(stack)
            .last()
            .and_then(|t| self.nodes.get(t))
            .is_some_and(|s| s.value == 1.)
    }

    /// Whether a stack's header has a Back (or, on the first screen of a
    /// closable stack, a Close) button.
    pub fn has_back(&self, stack: NodeId) -> bool {
        let depth = self.pages(stack).len();
        self.nodes.get(&stack).is_some_and(|n| {
            depth > 1 || ((n.value as u32) & prism_core::STACK_CLOSABLE != 0 && depth > 0)
        })
    }

    /// Open overlays, in tree order: drawn last, above the window.
    pub fn open_overlays(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = self.roots().collect();
        stack.reverse();
        while let Some(id) = stack.pop() {
            let Some(n) = self.nodes.get(&id) else {
                continue;
            };
            if n.hidden {
                continue;
            }
            if n.kind == NodeKind::Overlay && n.value == 1. {
                out.push(id);
                for &c in n.children.iter().rev() {
                    stack.push(c);
                }
                continue;
            }
            for &c in self.shown_children(id).iter().rev() {
                stack.push(c);
            }
        }
        out
    }

    /// The root, then the layers.
    pub fn roots(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.root.into_iter().chain(self.layers.iter().copied())
    }

    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.nodes.get(&id).and_then(|n| n.parent), move |p| {
            self.nodes.get(p).and_then(|n| n.parent)
        })
    }

    /// How far a sticky node is moved down now (#see `Widgets::set_sticky`).
    pub fn sticky_offset(&self, n: &Node) -> f32 {
        match n.sticky {
            Some((scroll, s)) => self
                .nodes
                .get(&scroll)
                .map_or(0., |sv| s.offset(sv.scroll.1)),
            None => 0.,
        }
    }

    /// Where the node's top-left corner is in the window (scrolling and
    /// sticky offsets included, transforms not).
    pub fn origin(&self, id: NodeId) -> (f32, f32) {
        let Some(n) = self.nodes.get(&id) else {
            return (0., 0.);
        };
        let (mut x, mut y) = (n.frame.x, n.frame.y + self.sticky_offset(n));
        let mut parent = n.parent;
        while let Some(p) = parent {
            let Some(pn) = self.nodes.get(&p) else { break };
            x += pn.frame.x - pn.scroll.0;
            y += pn.frame.y - pn.scroll.1 + self.sticky_offset(pn);
            parent = pn.parent;
        }
        (x, y)
    }

    pub fn window_rect(&self, id: NodeId) -> Rect {
        let (x, y) = self.origin(id);
        let f = self.nodes.get(&id).map(|n| n.frame).unwrap_or_default();
        Rect::new(x, y, f.w, f.h)
    }

    /// The innermost node under a window point that takes pointer input.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<NodeId> {
        // The layers first (above sheets and dialogs too), then the open
        // overlays (topmost last), then the root.
        for r in self.layers.iter().rev() {
            if let Some(hit) = self.hit_node(*r, x, y) {
                return Some(hit);
            }
        }
        for o in self.open_overlays().into_iter().rev() {
            if let Some(content) = self.nodes[&o].children.first()
                && let Some(hit) = self.hit_node(*content, x, y)
            {
                return Some(hit);
            }
        }
        self.root.and_then(|r| self.hit_node(r, x, y))
    }

    fn hit_node(&self, id: NodeId, x: f32, y: f32) -> Option<NodeId> {
        let n = self.nodes.get(&id)?;
        if n.hidden || !n.style.hit_test || n.style.inert {
            return None;
        }
        let r = self.window_rect(id);
        let inside = r.contains(x, y);
        if n.clips() && !inside {
            return None;
        }
        // Topmost first; sticky children are drawn above their siblings.
        let shown = self.shown_children(id);
        for &c in shown
            .iter()
            .filter(|c| self.nodes.get(c).is_some_and(|n| n.sticky.is_some()))
            .chain(shown.iter().rev())
        {
            if let Some(hit) = self.hit_node(c, x, y) {
                return Some(hit);
            }
        }
        // Layers let input through where they have no child.
        let layer = self.layers.contains(&id);
        (inside && !layer).then_some(id)
    }

    /// The nearest ancestor-or-self matching `f`.
    pub fn find_up(&self, id: NodeId, f: impl Fn(&Node) -> bool) -> Option<NodeId> {
        std::iter::once(id)
            .chain(self.ancestors(id))
            .find(|i| self.nodes.get(i).is_some_and(&f))
    }

    /// Nodes in tree order (depth first) that are shown.
    pub fn shown_in_order(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = self.roots().collect();
        stack.reverse();
        let mut overlays = Vec::new();
        while let Some(id) = stack.pop() {
            let Some(n) = self.nodes.get(&id) else {
                continue;
            };
            if n.hidden {
                continue;
            }
            out.push(id);
            if n.kind == NodeKind::Overlay {
                if n.value == 1. {
                    overlays.push(id);
                }
                continue;
            }
            for &c in self.shown_children(id).iter().rev() {
                stack.push(c);
            }
        }
        // An open overlay's content comes after the page (it is modal).
        for o in overlays {
            let mut stack: Vec<NodeId> = self.nodes[&o].children.iter().rev().copied().collect();
            while let Some(id) = stack.pop() {
                let Some(n) = self.nodes.get(&id) else {
                    continue;
                };
                if n.hidden {
                    continue;
                }
                out.push(id);
                for &c in self.shown_children(id).iter().rev() {
                    stack.push(c);
                }
            }
        }
        out
    }

    /// The text a node shows itself (a text's, a button's title).
    pub fn own_text(&self, id: NodeId) -> Option<String> {
        let n = self.nodes.get(&id)?;
        match n.kind {
            NodeKind::Text | NodeKind::Button | NodeKind::Checkbox | NodeKind::Timer => {
                let t = if n.text.is_empty() && !n.config.is_empty() {
                    prism_core::decode_runs(&n.config)
                        .iter()
                        .map(|r| r.text.as_str())
                        .collect()
                } else {
                    n.text.clone()
                };
                Some(t)
            }
            _ => None,
        }
    }

    /// Whether a shown descendant (or the node) reads `label`.
    fn shows_label(&self, id: NodeId, label: &str) -> bool {
        if self.own_text(id).is_some_and(|t| t == label)
            || self
                .nodes
                .get(&id)
                .is_some_and(|n| n.a11y.label.as_deref() == Some(label))
        {
            return true;
        }
        self.shown_children(id)
            .iter()
            .any(|&c| self.nodes.get(&c).is_some_and(|n| !n.hidden) && self.shows_label(c, label))
    }

    /// What `PRISM_CLICK=label` presses: a button or control titled
    /// `label`, a tab, a header's Back or bar button, else the innermost
    /// clickable view showing it.
    pub fn find_by_label(&self, label: &str) -> Option<Target> {
        let order = self.shown_in_order();
        for &id in &order {
            let n = &self.nodes[&id];
            match n.kind {
                NodeKind::Button | NodeKind::Checkbox | NodeKind::Toggle
                    if self.own_text(id).as_deref() == Some(label)
                        || n.a11y.label.as_deref() == Some(label) =>
                {
                    return Some(Target::Node(id));
                }
                NodeKind::Tabs => {
                    for (i, t) in self.pages(id).into_iter().enumerate() {
                        if self.nodes.get(&t).is_some_and(|t| t.text == label) {
                            return Some(Target::Tab(id, i));
                        }
                    }
                }
                NodeKind::Stack if self.has_back(id) && label == "Back" => {
                    return Some(Target::Back(id));
                }
                NodeKind::Screen => {
                    let config = prism_core::navbar::ScreenConfig::decode(&n.config);
                    if let Some(i) = config
                        .items
                        .iter()
                        .position(|b| b.title == label || b.spoken() == label)
                    {
                        return Some(Target::BarItem(id, i));
                    }
                }
                _ => {}
            }
        }
        // Clickable views showing the label: the innermost one. A
        // clickable scroll view refreshes (`refresh.rs`): it is no control,
        // and would take the label of each field and text in the page.
        order
            .iter()
            .rev()
            .copied()
            .find(|&id| {
                let n = &self.nodes[&id];
                let control = n.clickable && n.kind != NodeKind::Scroll;
                (control || matches!(n.kind, NodeKind::Picker | NodeKind::DatePicker))
                    && self.shows_label(id, label)
            })
            .map(Target::Node)
    }

    /// The shown text field whose placeholder (or label) is `placeholder`.
    pub fn find_field(&self, placeholder: &str) -> Option<NodeId> {
        self.shown_in_order().into_iter().find(|&id| {
            let n = &self.nodes[&id];
            n.is_text_field()
                && (n.placeholder == placeholder || n.a11y.label.as_deref() == Some(placeholder))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(scene: &mut Scene, id: NodeId, kind: NodeKind, frame: Rect) {
        scene.create(id, kind);
        scene.nodes.get_mut(&id).unwrap().frame = frame;
    }

    /// root(0,0 400×600) ⊃ scroll(0,100 400×200, content 400×1000)
    /// ⊃ button(10,150 100×40); root ⊃ text(10,10).
    fn sample() -> Scene {
        let mut s = Scene::default();
        node(&mut s, 1, NodeKind::View, Rect::new(0., 0., 400., 600.));
        node(&mut s, 2, NodeKind::Scroll, Rect::new(0., 100., 400., 200.));
        node(&mut s, 3, NodeKind::Button, Rect::new(10., 150., 100., 40.));
        node(&mut s, 4, NodeKind::Text, Rect::new(10., 10., 100., 20.));
        s.nodes.get_mut(&2).unwrap().content = (400., 1000.);
        s.nodes.get_mut(&3).unwrap().text = "+1".into();
        s.nodes.get_mut(&4).unwrap().text = "Hello".into();
        s.set_children(1, &[2, 4]);
        s.set_children(2, &[3]);
        s.root = Some(1);
        s
    }

    #[test]
    fn origins_follow_scrolling() {
        let mut s = sample();
        assert_eq!(s.origin(3), (10., 250.));
        s.nodes.get_mut(&2).unwrap().scroll = (0., 100.);
        assert_eq!(s.origin(3), (10., 150.));
        // At most the content's overflow.
        assert_eq!(s.nodes[&2].max_scroll(), (0., 800.));
    }

    #[test]
    fn hit_testing_respects_clipping_and_order() {
        let mut s = sample();
        assert_eq!(s.hit_test(20., 260.), Some(3));
        assert_eq!(s.hit_test(20., 15.), Some(4));
        assert_eq!(s.hit_test(300., 500.), Some(1));
        // Scrolled out of the scroll view: clipped.
        s.nodes.get_mut(&2).unwrap().scroll = (0., 200.);
        assert_eq!(s.hit_test(20., 60.), Some(1));
        // pointer-events: none.
        s.nodes.get_mut(&2).unwrap().scroll = (0., 0.);
        s.nodes.get_mut(&3).unwrap().style.hit_test = false;
        assert_eq!(s.hit_test(20., 260.), Some(2));
    }

    /// A layer (a toast) is above an open sheet, and lets input through
    /// where it has no child.
    #[test]
    fn layers_are_above_open_overlays() {
        let mut s = sample();
        node(&mut s, 5, NodeKind::Overlay, Rect::new(0., 0., 0., 0.));
        node(&mut s, 6, NodeKind::View, Rect::new(0., 0., 400., 600.));
        node(&mut s, 7, NodeKind::View, Rect::new(0., 0., 400., 600.));
        node(&mut s, 8, NodeKind::Text, Rect::new(100., 500., 200., 40.));
        s.set_children(1, &[2, 4, 5]);
        s.set_children(5, &[6]);
        s.set_children(7, &[8]);
        s.nodes.get_mut(&5).unwrap().value = 1.;
        s.layers = vec![7];
        assert_eq!(s.hit_test(150., 520.), Some(8));
        // Elsewhere the sheet's content, not the layer, takes it.
        assert_eq!(s.hit_test(20., 15.), Some(6));
    }

    #[test]
    fn stacks_show_their_top_screen_and_tabs_their_selection() {
        let mut s = Scene::default();
        node(&mut s, 1, NodeKind::Tabs, Rect::new(0., 0., 400., 600.));
        node(&mut s, 2, NodeKind::Tab, Rect::new(0., TAB_BAR, 400., 552.));
        node(&mut s, 3, NodeKind::Tab, Rect::new(0., TAB_BAR, 400., 552.));
        node(&mut s, 4, NodeKind::Stack, Rect::new(0., 0., 400., 552.));
        node(
            &mut s,
            5,
            NodeKind::Screen,
            Rect::new(0., HEADER, 400., 504.),
        );
        node(
            &mut s,
            6,
            NodeKind::Screen,
            Rect::new(0., HEADER, 400., 504.),
        );
        node(&mut s, 7, NodeKind::Button, Rect::new(0., 0., 50., 20.));
        node(&mut s, 8, NodeKind::Button, Rect::new(0., 0., 50., 20.));
        s.set_children(1, &[2, 3]);
        s.set_children(2, &[4]);
        s.set_children(4, &[5, 6]);
        s.set_children(5, &[7]);
        s.set_children(6, &[8]);
        s.root = Some(1);
        s.nodes.get_mut(&2).unwrap().text = "Todos".into();
        s.nodes.get_mut(&3).unwrap().text = "Settings".into();
        assert_eq!(s.shown_children(4).as_ref(), &[6]);
        assert_eq!(s.shown_children(1).as_ref(), &[2]);
        assert_eq!(s.origin(8), (0., TAB_BAR + HEADER));
        assert_eq!(s.hit_test(10., TAB_BAR + HEADER + 10.), Some(8));
        assert_eq!(s.find_by_label("Settings"), Some(Target::Tab(1, 1)));
        assert_eq!(s.find_by_label("Back"), Some(Target::Back(4)));
        s.nodes.get_mut(&1).unwrap().value = 1.;
        assert_eq!(s.shown_children(1).as_ref(), &[3]);
    }

    #[test]
    fn labels_find_buttons_then_clickable_views() {
        let mut s = sample();
        assert_eq!(s.find_by_label("+1"), Some(Target::Node(3)));
        assert_eq!(s.find_by_label("Hello"), None);
        s.nodes.get_mut(&1).unwrap().clickable = true;
        assert_eq!(s.find_by_label("Hello"), Some(Target::Node(1)));
        assert_eq!(s.find_by_label("nothing"), None);
    }

    #[test]
    fn a_refreshing_scroll_view_takes_no_label() {
        let mut s = sample();
        node(&mut s, 5, NodeKind::Text, Rect::new(10., 200., 100., 20.));
        s.nodes.get_mut(&5).unwrap().text = "Inside".into();
        s.set_children(2, &[3, 5]);
        // Clickable: it refreshes on a pull, F5 or Ctrl+R.
        s.nodes.get_mut(&2).unwrap().clickable = true;
        assert_eq!(s.find_by_label("Inside"), None);
        assert_eq!(s.find_by_label("+1"), Some(Target::Node(3)));
    }

    #[test]
    fn reparenting_and_removal_keep_links_consistent() {
        let mut s = sample();
        s.set_children(1, &[3, 4]);
        assert_eq!(s.nodes[&3].parent, Some(1));
        assert_eq!(s.nodes[&2].parent, None);
        assert!(s.nodes[&2].children.is_empty());
        s.remove(1);
        assert_eq!(s.nodes[&3].parent, None);
        assert_eq!(s.root, None);
    }
}
