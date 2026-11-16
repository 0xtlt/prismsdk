//! A `Widgets` implementation that records every call, with a deterministic
//! text measure: 8 pt per character, 20 pt per line, wrapping at word
//! boundaries when given a width.

use std::collections::HashMap;
use std::sync::Arc;

use prism_core::{
    Accessibility, App, AppInit, Appearance, Backend, NodeId, NodeKind, Patch, Path, Pose, Render,
    RenderEnvironment, Runtime, Style, Transition, menu::MenuSpec,
};
use prism_native::{Insets, NativeRenderer, Sticky, Viewport, Widgets};

pub const CHAR: f32 = 8.;
pub const LINE: f32 = 20.;
/// Height of the fake navigation bar above a stack's screens.
pub const HEADER: f32 = 44.;

/// One recorded call (frames and styles only where ordering matters).
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Frame(NodeId, (f32, f32, f32, f32)),
    FrameAnimated(NodeId, (f32, f32, f32, f32), Transition),
    Style(NodeId, Style),
    StyleAnimated(NodeId, Style, Transition),
    Exit {
        parent: NodeId,
        view: NodeId,
        to: Style,
        t: Transition,
    },
    Repeat(NodeId, Option<(Pose, Transition)>),
    Destroy(NodeId),
    Drawing(NodeId),
}

#[derive(Default)]
pub struct Fake {
    pub calls: Vec<Call>,
    pub kinds: HashMap<NodeId, NodeKind>,
    pub frames: HashMap<NodeId, (f32, f32, f32, f32)>,
    pub styles: HashMap<NodeId, Style>,
    pub texts: HashMap<NodeId, String>,
    pub placeholders: HashMap<NodeId, String>,
    pub clickable: HashMap<NodeId, bool>,
    pub children: HashMap<NodeId, Vec<NodeId>>,
    pub root: Option<NodeId>,
    pub a11y: HashMap<NodeId, Accessibility>,
    pub values: HashMap<NodeId, f64>,
    pub icons: HashMap<NodeId, String>,
    pub menus: HashMap<NodeId, Option<MenuSpec>>,
    pub swipes: HashMap<NodeId, (bool, bool)>,
    pub scroll_events: HashMap<NodeId, bool>,
    pub hidden: HashMap<NodeId, bool>,
    pub content: HashMap<NodeId, (f32, f32)>,
    pub drawings: HashMap<NodeId, Vec<Path>>,
    pub env: Option<RenderEnvironment>,
    pub appearance: Option<Appearance>,
    /// `(max_width)` of each measure call, by view.
    pub measured: HashMap<NodeId, Vec<Option<f32>>>,
    /// Safe area the fake platform gives stack screens.
    pub screen_insets: Insets,
    /// What each sticky view was last told: its scroll view and range.
    pub sticky: HashMap<NodeId, Option<(NodeId, Sticky)>>,
    /// The interaction states each view was asked to report.
    pub interactions: HashMap<NodeId, prism_core::Interactions>,
    // Pointer, keys, focus, layers (#17, #25).
    pub pointer: HashMap<NodeId, u8>,
    pub keys: HashMap<NodeId, (bool, bool)>,
    pub focused: Vec<NodeId>,
    pub revealed: Vec<NodeId>,
    pub layers: Vec<NodeId>,
    /// Web views (#62): messages posted, reloads.
    pub web_messages: Vec<(NodeId, String)>,
    pub web_reloads: Vec<NodeId>,
}

impl Fake {
    /// The view showing `text` (panics if none or several).
    pub fn by_text(&self, text: &str) -> NodeId {
        let ids: Vec<NodeId> = self
            .texts
            .iter()
            .filter(|(_, t)| t.as_str() == text)
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(ids.len(), 1, "views with text {text:?}: {ids:?}");
        ids[0]
    }

    /// Every live view of `kind`, in creation order.
    pub fn of_kind(&self, kind: NodeKind) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self
            .kinds
            .iter()
            .filter(|(_, k)| **k == kind)
            .map(|(id, _)| *id)
            .collect();
        ids.sort();
        ids
    }

    pub fn frame(&self, id: NodeId) -> (f32, f32, f32, f32) {
        *self
            .frames
            .get(&id)
            .unwrap_or_else(|| panic!("{id} has no frame"))
    }

    /// Lines a text needs at `max_width`.
    pub fn wrap(text: &str, max_width: Option<f32>) -> (f32, f32) {
        let full = text.chars().count() as f32 * CHAR;
        let Some(max) = max_width.filter(|m| full > *m) else {
            return (full, LINE);
        };
        let mut lines = 1;
        let mut line = 0f32;
        let mut widest = 0f32;
        for word in text.split_whitespace() {
            let w = word.chars().count() as f32 * CHAR;
            let with_space = if line > 0. { line + CHAR + w } else { w };
            if line > 0. && with_space > max {
                widest = widest.max(line);
                lines += 1;
                line = w;
            } else {
                line = with_space;
            }
        }
        widest = widest.max(line);
        (widest, lines as f32 * LINE)
    }

    pub fn take_calls(&mut self) -> Vec<Call> {
        std::mem::take(&mut self.calls)
    }
}

impl Widgets for Fake {
    type View = NodeId;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> NodeId {
        self.kinds.insert(id, kind);
        id
    }
    fn set_style(&mut self, v: &NodeId, style: &Style) {
        self.styles.insert(*v, style.clone());
        self.calls.push(Call::Style(*v, style.clone()));
    }
    fn set_style_animated(&mut self, v: &NodeId, style: &Style, t: Transition) {
        self.styles.insert(*v, style.clone());
        self.calls.push(Call::StyleAnimated(*v, style.clone(), t));
    }
    fn set_text(&mut self, v: &NodeId, text: &str) {
        self.texts.insert(*v, text.into());
    }
    fn set_placeholder(&mut self, v: &NodeId, text: &str) {
        self.placeholders.insert(*v, text.into());
    }
    fn set_clickable(&mut self, v: &NodeId, on: bool) {
        self.clickable.insert(*v, on);
    }
    fn set_children(&mut self, parent: &NodeId, children: &[NodeId]) {
        self.children.insert(*parent, children.to_vec());
    }
    fn set_root(&mut self, v: &NodeId) {
        self.root = Some(*v);
    }
    fn destroy(&mut self, v: &NodeId) {
        self.kinds.remove(v);
        self.texts.remove(v);
        self.frames.remove(v);
        self.calls.push(Call::Destroy(*v));
    }
    fn measure(&mut self, v: &NodeId, max_width: Option<f32>) -> (f32, f32) {
        self.measured.entry(*v).or_default().push(max_width);
        match self.kinds.get(v) {
            Some(NodeKind::Text | NodeKind::Button) => {
                Self::wrap(self.texts.get(v).map_or("", String::as_str), max_width)
            }
            Some(NodeKind::TextInput) => (120., 30.),
            Some(NodeKind::Toggle) => (50., 30.),
            Some(NodeKind::Progress) => (100., 4.),
            _ => (0., 0.),
        }
    }
    fn measure_min_content(&mut self, v: &NodeId) -> f32 {
        let text = self.texts.get(v).cloned().unwrap_or_default();
        text.split_whitespace()
            .map(|w| w.chars().count() as f32 * CHAR)
            .fold(0., f32::max)
    }
    fn set_frame(&mut self, v: &NodeId, x: f32, y: f32, w: f32, h: f32) {
        self.frames.insert(*v, (x, y, w, h));
        self.calls.push(Call::Frame(*v, (x, y, w, h)));
    }
    fn set_frame_animated(&mut self, v: &NodeId, x: f32, y: f32, w: f32, h: f32, t: Transition) {
        self.frames.insert(*v, (x, y, w, h));
        self.calls.push(Call::FrameAnimated(*v, (x, y, w, h), t));
    }
    /// Stack screens sit below a 44 pt bar; a sheet covers its container.
    fn screen_viewport(&mut self, container: &NodeId, child: &NodeId) -> Viewport {
        let (_, _, w, h) = self.frames[container];
        if self.kinds.get(child) == Some(&NodeKind::Modal) {
            return Viewport {
                x: 0.,
                y: 0.,
                width: w,
                height: h,
                insets: Insets::default(),
            };
        }
        let header = if self.kinds.get(container) == Some(&NodeKind::Tabs) {
            0.
        } else {
            HEADER
        };
        Viewport {
            x: 0.,
            y: header,
            width: w,
            height: h - header,
            insets: self.screen_insets,
        }
    }
    fn set_accessibility(&mut self, v: &NodeId, a11y: &Accessibility) {
        self.a11y.insert(*v, a11y.clone());
    }
    fn set_value(&mut self, v: &NodeId, value: f64) {
        self.values.insert(*v, value);
    }
    fn set_drawing(&mut self, v: &NodeId, paths: &[Path]) {
        self.drawings.insert(*v, paths.to_vec());
        self.calls.push(Call::Drawing(*v));
    }
    fn set_icon(&mut self, v: &NodeId, icon: &str) {
        self.icons.insert(*v, icon.into());
    }
    fn set_menu(&mut self, v: &NodeId, menu: Option<&MenuSpec>) {
        self.menus.insert(*v, menu.cloned());
    }
    fn set_swipe(&mut self, v: &NodeId, left: bool, right: bool) {
        self.swipes.insert(*v, (left, right));
    }
    fn set_scroll_events(&mut self, v: &NodeId, on: bool) {
        self.scroll_events.insert(*v, on);
    }
    fn set_appearance(&mut self, appearance: Appearance) {
        self.appearance = Some(appearance);
    }
    fn set_hidden(&mut self, v: &NodeId, hidden: bool) {
        self.hidden.insert(*v, hidden);
    }
    fn set_content_size(&mut self, v: &NodeId, w: f32, h: f32) {
        self.content.insert(*v, (w, h));
    }
    fn set_environment(&mut self, env: &RenderEnvironment) {
        self.env = Some(env.clone());
    }
    fn animate_exit(&mut self, parent: &NodeId, view: &NodeId, to: &Style, t: Transition) {
        self.calls.push(Call::Exit {
            parent: *parent,
            view: *view,
            to: to.clone(),
            t,
        });
    }
    fn set_repeat(&mut self, v: &NodeId, _style: &Style, repeat: Option<(Pose, Transition)>) {
        self.calls.push(Call::Repeat(*v, repeat));
    }
    fn set_sticky(&mut self, v: &NodeId, sticky: Option<(&NodeId, Sticky)>) {
        self.sticky.insert(*v, sticky.map(|(s, st)| (*s, st)));
    }
    fn set_interactions(&mut self, v: &NodeId, states: prism_core::Interactions) {
        self.interactions.insert(*v, states);
    }
    fn set_pointer_events(&mut self, v: &NodeId, mask: u8) {
        self.pointer.insert(*v, mask);
    }
    fn set_key_events(&mut self, v: &NodeId, local: bool, global: bool) {
        self.keys.insert(*v, (local, global));
    }
    fn focus(&mut self, v: &NodeId) {
        self.focused.push(*v);
    }
    fn scroll_into_view(&mut self, v: &NodeId) {
        self.revealed.push(*v);
    }
    fn set_layers(&mut self, layers: &[NodeId]) {
        self.layers = layers.to_vec();
    }
    fn post_web_message(&mut self, v: &NodeId, data: &str) {
        self.web_messages.push((*v, data.to_string()));
    }
    fn reload_web_view(&mut self, v: &NodeId) {
        self.web_reloads.push(*v);
    }
    /// Overlays get a 300 x 400 area at (10, 20).
    fn overlay_viewport(&mut self, _: &NodeId, _: &NodeId) -> Viewport {
        Viewport {
            x: 10.,
            y: 20.,
            width: 300.,
            height: 400.,
            insets: Insets::default(),
        }
    }
}

/// A runtime plus a renderer that has applied its first render.
pub struct Harness {
    pub rt: Runtime,
    pub r: NativeRenderer<Fake>,
}

impl Harness {
    pub fn new(init: impl Fn(&mut App) + Send + Sync + 'static) -> Self {
        let init: AppInit = Arc::new(init);
        let mut rt = Runtime::new(&init);
        let mut r = NativeRenderer::new(Fake::default());
        r.apply(&rt.render());
        Harness { rt, r }
    }

    /// An app whose root entity is `make()`.
    pub fn root<R: Render + 'static>(make: impl Fn() -> R + Send + Sync + 'static) -> Self {
        Self::new(move |cx| {
            let e = cx.new_entity(|_| make());
            cx.set_root(e);
        })
    }

    pub fn layout(&mut self, w: f32, h: f32) -> &Fake {
        self.r.layout(w, h, Insets::default());
        &self.r.widgets
    }

    /// Applies whatever changed since the last render.
    pub fn rerender(&mut self) -> Vec<Patch> {
        let patches = self.rt.render();
        self.r.apply(&patches);
        patches
    }

    pub fn w(&self) -> &Fake {
        &self.r.widgets
    }
}
