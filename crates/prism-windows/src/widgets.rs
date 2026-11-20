//! `prism_native::Widgets` over WinUI 3: each Prism node is a set of XAML
//! elements (`view.rs`) placed at the frame prism-native's layout gives
//! it, styled from its `Style`. A pure model of the tree (`scene.rs`) is
//! kept alongside: which page a stack or tab bar shows, label search for
//! the smoke hooks, window origins for drags.

use prism_core::{
    Accessibility, Appearance, IdMap, IdSet, Interactions, NodeId, NodeKind, RenderEnvironment,
    Role, Style,
};
use prism_native::{Insets, Viewport, Widgets};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Imaging::IWICImagingFactory;
use windows_core::{Interface, Result};

use crate::controls::{self, Content};
use crate::fluent::{Accent, Palette};
use crate::host::Island;
use crate::images::Images;
use crate::scene::{HEADER, Rect, Scene, TAB_BAR};
use crate::text::Measurer;
use crate::view::{Bar, View, scroll_axis};
use crate::win::log;
use crate::winui::xaml::Automation::AutomationProperties;
use crate::winui::xaml::Automation::Peers::{
    AccessibilityView, AutomationHeadingLevel, AutomationLiveSetting,
};
use crate::winui::xaml::Controls::{Canvas, ToolTipService};
use crate::winui::xaml::{ElementTheme, FocusState, UIElement, Visibility};
use crate::winui::{boxed, h, ui};
use crate::{events, system};

/// The window and its XAML.
pub struct Win {
    pub hwnd: HWND,
    pub scene: Scene,
    pub views: IdMap<NodeId, View>,
    /// The island's content: the app (root view and layers), then the
    /// open overlays above it.
    pub root: Canvas,
    pub app: Canvas,
    pub overlays: Canvas,
    pub island: Option<Island>,
    pub measurer: Measurer,
    pub images: Images,
    pub wic: IWICImagingFactory,
    pub dpi: u32,
    /// Client area, view units (DIPs).
    pub size: (f32, f32),
    pub system_dark: bool,
    pub appearance: Appearance,
    pub accent: Accent,
    pub palette: Palette,
    pub env: RenderEnvironment,
    pub locale: prism_core::Locale,
    /// The node with the keyboard focus.
    pub focus: Option<NodeId>,
    /// Focus moved with the keyboard.
    pub focus_visible: bool,
    /// `cx.focus(…)` and `dismiss_keyboard` while patches apply: carried
    /// out after (they report to prism-native).
    pub focus_requests: Vec<Option<NodeId>>,
    pub app_title: String,
    /// Fields reporting their selection (`on_select`).
    pub select_events: IdSet<NodeId>,
    /// What each field sent and the app has not rendered back yet (#73).
    pub typed: IdMap<NodeId, prism_native::fields::Typed>,
    /// Stacks and tab bars whose bar or shown page changed.
    bars: IdSet<NodeId>,
    /// Which view's children canvas holds each node's element.
    parents: IdMap<NodeId, NodeId>,
    /// Where each overlay was when its content was laid out (the content's
    /// frames are relative to it).
    pub overlay_origins: IdMap<NodeId, (f32, f32)>,
    /// Each overlay's panel or flyout (`overlay.rs`).
    pub layers: IdMap<NodeId, crate::overlay::Overlay>,
    /// Nodes with events already wired: clicks, hover and press.
    clicks: IdSet<NodeId>,
    hovers: IdSet<NodeId>,
}

impl Win {
    pub fn new(hwnd: HWND, images: Images, title: String, dpi: u32) -> Result<Win> {
        let system_dark = system::dark_mode();
        let accent = system::accent();
        let root = Canvas::new()?;
        let app = Canvas::new()?;
        let overlays = Canvas::new()?;
        root.Children()?.Append(&app)?;
        root.Children()?.Append(&overlays)?;
        let mut w = Win {
            hwnd,
            scene: Scene::default(),
            views: IdMap::default(),
            root,
            app,
            overlays,
            island: None,
            measurer: Measurer::new()?,
            images,
            wic: crate::wic::factory()?,
            dpi,
            size: (0., 0.),
            system_dark,
            appearance: Appearance::System,
            accent,
            palette: Palette::new(system_dark, accent),
            env: RenderEnvironment {
                text_scale: 1.,
                ..Default::default()
            },
            locale: system::locale(),
            focus: None,
            focus_visible: false,
            focus_requests: Vec::new(),
            app_title: title,
            select_events: IdSet::default(),
            typed: IdMap::default(),
            bars: IdSet::default(),
            parents: IdMap::default(),
            overlay_origins: IdMap::default(),
            layers: IdMap::default(),
            clicks: IdSet::default(),
            hovers: IdSet::default(),
        };
        w.theme_root()?;
        Ok(w)
    }

    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.
    }

    pub fn dark(&self) -> bool {
        match self.appearance {
            Appearance::Dark => true,
            Appearance::Light => false,
            Appearance::System => self.system_dark,
        }
    }

    fn theme_root(&mut self) -> Result<()> {
        self.root.SetRequestedTheme(if self.dark() {
            ElementTheme::Dark
        } else {
            ElementTheme::Light
        })
    }

    /// The window's size changed (DIPs).
    pub fn resize(&mut self, w: f32, h: f32) {
        // The menu bar (`menubar.rs`) takes the top of the window.
        self.size = (w, (h - crate::menubar::height()).max(0.));
        let _ = self.root.SetWidth(w as f64);
        let _ = self.root.SetHeight(h as f64);
        crate::menubar::resize(w);
    }

    /// The theme changed (system, app, accent): every color again.
    pub fn retheme(&mut self) {
        self.palette = Palette::new(self.dark(), self.accent);
        crate::imp::set_dark_title_bar(self.hwnd, self.dark());
        let _ = self.theme_root();
        let ids: Vec<NodeId> = self.views.keys().copied().collect();
        for id in ids {
            self.restyle(id);
            if let Some(n) = self.scene.get(id)
                && matches!(n.kind, NodeKind::Stack | NodeKind::Tabs)
            {
                self.bars.insert(id);
            }
            if let (Some(n), Some(v)) = (self.scene.get(id), self.views.get(&id))
                && n.kind == NodeKind::Shape
            {
                let _ = controls::set_paths(&v.content, &n.paths, &self.palette);
            }
        }
    }

    /// Applies a node's style to its elements again.
    pub fn restyle(&mut self, id: NodeId) {
        let sticky = self.sticky(id);
        let (Some(n), Some(v)) = (self.scene.get(id), self.views.get_mut(&id)) else {
            return;
        };
        if let Err(e) = v.apply_style(n, &self.palette, sticky) {
            log!("style of node {id}: {e}");
        }
    }

    fn sticky(&self, id: NodeId) -> f32 {
        self.scene
            .get(id)
            .map_or(0., |n| self.scene.sticky_offset(n))
    }

    /// The outer element of a node.
    pub fn outer(&self, id: NodeId) -> Option<UIElement> {
        self.views.get(&id).map(View::outer)
    }

    /// Whether node `id` is drawn where it is: not hidden, and the page a
    /// stack or tab bar shows.
    fn shown_in_parent(&self, id: NodeId) -> bool {
        let Some(n) = self.scene.get(id) else {
            return false;
        };
        if n.hidden {
            return false;
        }
        match n
            .parent
            .and_then(|p| self.scene.get(p).map(|pn| (p, pn.kind)))
        {
            Some((p, NodeKind::Stack | NodeKind::Tabs)) => {
                self.scene.shown_children(p).contains(&id)
            }
            _ => true,
        }
    }

    fn update_visibility(&self, id: NodeId) {
        if let Some(v) = self.views.get(&id) {
            let _ = v.set_visible(self.shown_in_parent(id));
        }
    }

    /// After a commit: bars, the pages shown, overlays, sticky views.
    pub fn flush(&mut self) {
        let bars: Vec<NodeId> = self.bars.drain().collect();
        for id in bars {
            let (Some(n), Some(v)) = (self.scene.get(id), self.views.get_mut(&id)) else {
                continue;
            };
            let width = n.frame.w;
            let r = match &mut v.bar {
                Bar::Header(hd) => {
                    let shown = self.scene.header_shown(id);
                    let _ = hd.root.SetVisibility(if shown {
                        Visibility::Visible
                    } else {
                        Visibility::Collapsed
                    });
                    hd.update(&self.scene, id, &self.palette, width)
                }
                Bar::Tabs(t) => t.update(&self.scene, id, &self.palette, width),
                Bar::None => Ok(()),
            };
            if let Err(e) = r {
                log!("bar of node {id}: {e}");
            }
            for c in self
                .scene
                .get(id)
                .map(|n| n.children.clone())
                .unwrap_or_default()
            {
                self.update_visibility(c);
            }
        }
        self.place_overlays();
        let sticky: Vec<NodeId> = self
            .scene
            .nodes
            .iter()
            .filter(|(_, n)| n.sticky.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in sticky {
            let s = self.sticky(id);
            if let (Some(n), Some(v)) = (self.scene.get(id), self.views.get(&id)) {
                let r = Rect::new(0., 0., n.frame.w, n.frame.h);
                let _ = crate::style::apply_transform(&v.outer, &n.style, r, s);
            }
        }
    }

    /// The app's root and layers in the window.
    fn mount(&mut self) {
        let Ok(coll) = self.app.Children() else {
            return;
        };
        let _ = coll.Clear();
        for id in self.scene.roots().collect::<Vec<_>>() {
            if let Some(e) = self.outer(id) {
                let _ = crate::view::detach(&e);
                let _ = coll.Append(&e);
            }
        }
    }

    /// Wires clicks, hover and press once a node needs them; makes the
    /// node hit-testable.
    fn update_hit(&mut self, id: NodeId) {
        let Some(n) = self.scene.get(id) else { return };
        let hover = n.reports.contains(prism_core::Interaction::Hover)
            || n.reports.contains(prism_core::Interaction::Active);
        let pointer = crate::pointer::wants(id);
        let wants = n.clickable || hover || n.menu.is_some() || n.pointer_mask != 0 || pointer;
        let clickable = n.clickable;
        let Some(v) = self.views.get_mut(&id) else {
            return;
        };
        let _ = v.set_hit(wants);
        if clickable && self.clicks.insert(id) {
            match v.make_button(id) {
                Ok(Some(b)) => {
                    if let Ok(e) = ui(&b) {
                        let _ = events::focus(&e, id);
                    }
                    let _ = events::pressed(&b, id);
                    // The name and help text go on the button.
                    if let Some(n) = self.scene.get(id) {
                        let _ = accessibility(v, &n.a11y);
                    }
                }
                // Scroll views and leaves: clicked where nothing else is.
                _ => {
                    let _ = events::clickable(&v.outer, id);
                }
            }
        }
        if hover && self.hovers.insert(id) {
            let _ = events::hover(&v.outer, id);
        }
        if pointer && let Err(e) = crate::pointer::wire(&v.outer, &v.kids, v.button.is_some(), id) {
            log!("pointer input of node {id}: {e}");
        }
    }

    /// A field's text from the app, unless it is the echo of typing.
    fn set_field_text(&mut self, id: NodeId, text: &str) {
        let Some(v) = self.views.get(&id) else { return };
        let current = controls::field_text(&v.content).unwrap_or_default();
        let takes = self
            .typed
            .entry(id)
            .or_default()
            .should_apply(text, &current);
        if takes {
            crate::imp::programmatic_text(id, text);
            if let Err(e) = controls::set_field_text(&v.content, text) {
                log!("text of field {id}: {e}");
            }
        }
    }

    /// A picture's bitmap, once decoded.
    pub fn show_picture(&mut self, id: NodeId) {
        let (Some(n), Some(v)) = (self.scene.get(id), self.views.get(&id)) else {
            return;
        };
        let bitmap = self.images.bitmap(&n.text).map(|(_, b)| b);
        if let Err(e) = controls::set_picture(&v.content, bitmap.as_ref()) {
            log!("picture {:?}: {e}", n.text);
        }
    }

    /// Gives node `id` the keyboard focus (`None`: away from fields).
    pub fn focus_node(&mut self, id: Option<NodeId>) {
        let control = id
            .and_then(|i| self.views.get(&i))
            .and_then(|v| v.content.control());
        match control {
            Some(c) => {
                let state = if self.focus_visible {
                    FocusState::Keyboard
                } else {
                    FocusState::Programmatic
                };
                let _ = c.Focus(state);
            }
            None if id.is_none() => {
                // Out of the field: the root takes the focus.
                if let Some(v) = self.scene.root.and_then(|r| self.views.get(&r)) {
                    let _ = v.outer.Focus(FocusState::Programmatic);
                }
            }
            None => {}
        }
    }

    /// The control showing `value` again (the app kept its own).
    pub fn sync_value(&mut self, id: NodeId) {
        if let (Some(n), Some(v)) = (self.scene.get(id), self.views.get(&id)) {
            let _ = controls::set_value(&v.content, n, n.value);
        }
        if self.scene.get(id).is_some_and(|n| n.kind == NodeKind::Tabs) {
            self.bars.insert(id);
        }
    }

    /// Scrolls a scroll view to `(x, y)`.
    pub fn scroll_view_to(&mut self, id: NodeId, x: f32, y: f32, animated: bool) -> bool {
        let Some(sv) = self.views.get(&id).and_then(|v| v.scroll.clone()) else {
            return false;
        };
        let h = windows::Foundation::PropertyValue::CreateDouble(x.max(0.) as f64)
            .and_then(|v| v.cast::<windows::Foundation::IReference<f64>>());
        let vv = windows::Foundation::PropertyValue::CreateDouble(y.max(0.) as f64)
            .and_then(|v| v.cast::<windows::Foundation::IReference<f64>>());
        let (Ok(h), Ok(vv)) = (h, vv) else {
            return false;
        };
        let moved = sv
            .ChangeViewWithOptionalAnimation(
                &h,
                &vv,
                None::<&windows::Foundation::IReference<f32>>,
                !animated,
            )
            .unwrap_or(false);
        if let Some(n) = self.scene.get_mut(id) {
            let (mx, my) = n.max_scroll();
            n.scroll = (x.clamp(0., mx), y.clamp(0., my));
        }
        moved
    }
}

impl Widgets for Win {
    type View = NodeId;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> NodeId {
        self.scene.create(id, kind);
        if kind == NodeKind::DatePicker
            && let Some(n) = self.scene.get_mut(id)
        {
            n.value = f64::NAN;
        }
        match View::new(id, kind) {
            Ok(v) => {
                if let Some(c) = v.content.control()
                    && let Ok(e) = ui(&c)
                {
                    let _ = events::focus(&e, id);
                }
                if matches!(kind, NodeKind::Scroll | NodeKind::Stack | NodeKind::Tabs) {
                    self.bars.insert(id);
                }
                self.views.insert(id, v);
                if kind == NodeKind::Overlay {
                    self.new_overlay(id);
                }
            }
            Err(e) => log!("node {id} ({kind:?}): {e}"),
        }
        id
    }

    fn set_style(&mut self, view: &NodeId, style: &Style) {
        let id = *view;
        // Shown at once: a running animation stops.
        crate::motion::settle(id);
        let Some(n) = self.scene.get_mut(id) else {
            return;
        };
        let axis = n.style.scroll_axis != style.scroll_axis;
        n.style = style.clone();
        self.measurer.forget(id);
        if (axis || n.kind == NodeKind::Scroll)
            && let Some(sv) = self.views.get(&id).and_then(|v| v.scroll.as_ref())
        {
            let _ = scroll_axis(sv, style.scroll_axis == prism_core::ScrollAxis::Vertical);
        }
        self.restyle(id);
    }

    fn set_text(&mut self, view: &NodeId, text: &str) {
        let id = *view;
        let Some(n) = self.scene.get_mut(id) else {
            return;
        };
        let kind = n.kind;
        if n.is_text_field() {
            n.text = text.to_string();
            self.set_field_text(id, text);
            return;
        }
        if n.text == text {
            return;
        }
        n.text = text.to_string();
        self.measurer.forget(id);
        match kind {
            NodeKind::Picture => {
                let wic = self.wic.clone();
                self.images.request(&wic, text, self.hwnd);
                self.show_picture(id);
            }
            NodeKind::WebView => self.show_web(id),
            NodeKind::Video => self.show_video(id),
            // A screen's title is its stack's header; a tab's, the bar's.
            NodeKind::Screen | NodeKind::Tab => {
                if let Some(p) = self.scene.get(id).and_then(|n| n.parent) {
                    self.bars.insert(p);
                }
            }
            _ => self.restyle(id),
        }
    }

    fn set_placeholder(&mut self, view: &NodeId, text: &str) {
        let id = *view;
        let Some(n) = self.scene.get_mut(id) else {
            return;
        };
        n.placeholder = text.to_string();
        if n.kind == NodeKind::Timer {
            n.text = prism_core::timer_text(n.value, text);
            self.measurer.forget(id);
        }
        let value = n.value;
        self.restyle(id);
        if let (Some(n), Some(v)) = (self.scene.get(id), self.views.get(&id))
            && matches!(n.kind, NodeKind::Picker | NodeKind::DatePicker)
        {
            let _ = controls::set_value(&v.content, n, value);
        }
    }

    fn set_clickable(&mut self, view: &NodeId, clickable: bool) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.clickable = clickable;
        }
        self.update_hit(*view);
    }

    fn set_children(&mut self, parent: &NodeId, children: &[NodeId]) {
        let id = *parent;
        // A closing panel keeps its content while it animates out.
        self.overlay_children(id, children);
        self.scene.set_children(id, children);
        let list: Vec<(NodeId, UIElement)> = children
            .iter()
            .filter_map(|c| self.outer(*c).map(|e| (*c, e)))
            .collect();
        // Children moving here from another parent (a fixed node hoisted to
        // its screen) leave it first; those leaving go nowhere yet.
        for (c, e) in &list {
            if let Some(old) = self.parents.insert(*c, id)
                && old != id
                && let Some(v) = self.views.get_mut(&old)
            {
                let _ = v.remove_child(*c, e);
            }
        }
        let gone: Vec<NodeId> = self
            .views
            .get(&id)
            .map(|v| {
                v.children
                    .iter()
                    .filter(|c| !children.contains(c))
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        for c in gone {
            if self.parents.get(&c) == Some(&id) {
                self.parents.remove(&c);
            }
        }
        if let Some(v) = self.views.get_mut(&id)
            && let Err(e) = v.set_children(&list)
        {
            log!("children of node {id}: {e}");
        }
        for c in children {
            self.update_visibility(*c);
        }
        if self
            .scene
            .get(id)
            .is_some_and(|n| matches!(n.kind, NodeKind::Stack | NodeKind::Tabs))
        {
            self.bars.insert(id);
        }
    }

    fn set_root(&mut self, view: &NodeId) {
        self.scene.root = Some(*view);
        self.mount();
    }

    fn destroy(&mut self, view: &NodeId) {
        let id = *view;
        let parent = self.scene.get(id).and_then(|n| n.parent);
        self.scene.remove(id);
        self.drop_overlay(id);
        if let Some(v) = self.views.remove(&id) {
            // A leaving node (or what it holds) stays until its exit animation ends.
            if !crate::motion::take_ghost(id) {
                let _ = crate::view::detach(&v.outer());
            }
        }
        if let Some(p) = self
            .parents
            .remove(&id)
            .or(parent)
            .and_then(|p| self.views.get_mut(&p))
        {
            p.children.retain(|c| *c != id);
        }
        self.measurer.forget(id);
        crate::pointer::forget(id);
        crate::drops::forget(id);
        crate::edit::forget(id);
        crate::carets::forget(id);
        crate::media::forget(id);
        crate::web::forget(id);
        self.typed.remove(&id);
        self.select_events.remove(&id);
        self.clicks.remove(&id);
        self.hovers.remove(&id);
        self.overlay_origins.remove(&id);
        crate::motion::forget(id);
        if self.focus == Some(id) {
            self.focus = None;
        }
    }

    fn measure(&mut self, view: &NodeId, max_width: Option<f32>) -> (f32, f32) {
        let Some(n) = self.scene.get(*view) else {
            return (0., 0.);
        };
        controls::measure(
            &mut self.measurer,
            &self.images,
            n,
            *view,
            max_width,
            &self.locale,
        )
    }

    fn measure_min_content(&mut self, view: &NodeId) -> f32 {
        let id = *view;
        if let Some(n) = self.scene.get(id)
            && matches!(n.kind, NodeKind::Text | NodeKind::Timer)
        {
            let st = prism_native::text::StyledText::new(&n.text, &n.config, &n.style);
            if st.no_wrap || st.max_lines.is_some() {
                return 0.;
            }
            return self.measurer.min_width(&st);
        }
        self.measure(view, None).0
    }

    fn set_frame(&mut self, view: &NodeId, x: f32, y: f32, w: f32, h: f32) {
        let id = *view;
        let frame = Rect::new(x, y, w, h);
        let Some(n) = self.scene.get_mut(id) else {
            return;
        };
        if n.frame == frame {
            return;
        }
        crate::motion::stop_frame(id);
        let resized = (n.frame.w, n.frame.h) != (w, h);
        n.frame = frame;
        let (mx, my) = n.max_scroll();
        n.scroll = (n.scroll.0.min(mx), n.scroll.1.min(my));
        let kind = n.kind;
        let sticky = self.sticky(id);
        if let (Some(n), Some(v)) = (self.scene.get(id), self.views.get_mut(&id))
            && let Err(e) = v.place(n, &self.palette, sticky)
        {
            log!("frame of node {id}: {e}");
        }
        if resized && matches!(kind, NodeKind::Stack | NodeKind::Tabs) {
            self.bars.insert(id);
        }
        if resized && kind == NodeKind::Scene3d {
            crate::scene3d::queue(id);
        }
    }

    fn screen_viewport(&mut self, container: &NodeId, child: &NodeId) -> Viewport {
        let (w, h) = self
            .scene
            .get(*container)
            .map_or(self.size, |n| (n.frame.w, n.frame.h));
        let modal = self
            .scene
            .get(*child)
            .is_some_and(|c| c.kind == NodeKind::Modal);
        if modal {
            // A sheet: centered, at most 560 wide, inset from the window.
            let sw = (w - 48.).clamp(200., 560.).min(w);
            let sh = (h - 48.).max(200.).min(h);
            return Viewport {
                x: (w - sw) / 2.,
                y: (h - sh) / 2.,
                width: sw,
                height: sh,
                insets: Insets::default(),
            };
        }
        let kind = self.scene.get(*container).map(|n| n.kind);
        let top = match kind {
            Some(NodeKind::Tabs) => TAB_BAR,
            Some(NodeKind::Stack) if self.scene.header_shown(*container) => HEADER,
            _ => 0.,
        };
        Viewport {
            x: 0.,
            y: top,
            width: w,
            height: (h - top).max(0.),
            insets: Insets::default(),
        }
    }

    fn overlay_viewport(&mut self, overlay: &NodeId, _content: &NodeId) -> Viewport {
        use prism_core::OverlayKind;
        let (w, h) = self.size;
        let config = self
            .scene
            .get(*overlay)
            .map(|n| prism_core::controls::OverlayConfig::decode(&n.config))
            .unwrap_or_default();
        let (width, height, x, y) = match config.kind {
            OverlayKind::Popover => {
                // Below its anchor (the overlay's parent).
                let anchor = self
                    .scene
                    .get(*overlay)
                    .and_then(|n| n.parent)
                    .map(|p| self.scene.window_rect(p))
                    .unwrap_or_default();
                let pw = (w - 32.).clamp(120., 360.);
                let x = anchor.x.clamp(8., (w - pw - 8.).max(8.));
                (pw, (h * 0.8).max(120.), x, anchor.bottom() + 4.)
            }
            OverlayKind::Dialog => {
                let dw = (w - 48.).clamp(200., 480.);
                (dw, (h - 80.).max(160.), (w - dw) / 2., 40.)
            }
            OverlayKind::Sheet => {
                let sw = (w - 48.).clamp(200., 560.).min(w);
                let sh = if config.medium {
                    (h * 0.5).max(200.)
                } else {
                    (h - 48.).max(200.)
                };
                // Windows has no sheet: a large dialog, centered.
                (sw, sh, (w - sw) / 2., (h - sh) / 2.)
            }
            OverlayKind::Keyboard => (w, 0., 0., h),
        };
        // The overlay's content canvas sits at the overlay's window origin
        // as it is now (`place_overlays`).
        let (ox, oy) = self.scene.origin(*overlay);
        self.overlay_origins.insert(*overlay, (ox, oy));
        Viewport {
            x: x - ox,
            y: y - oy,
            width,
            height,
            insets: Insets::default(),
        }
    }

    fn layer_insets(&mut self, safe_area: Insets) -> Insets {
        let tabs = self
            .scene
            .root
            .and_then(|r| self.scene.get(r))
            .is_some_and(|r| r.kind == NodeKind::Tabs);
        Insets {
            top: safe_area.top + if tabs { TAB_BAR } else { 0. },
            ..safe_area
        }
    }

    fn set_accessibility(&mut self, view: &NodeId, a11y: &Accessibility) {
        let id = *view;
        let Some(n) = self.scene.get_mut(id) else {
            return;
        };
        let enabled_changed = n.a11y.disabled != a11y.disabled;
        n.a11y = a11y.clone();
        if let Some(v) = self.views.get(&id)
            && let Err(e) = accessibility(v, a11y)
        {
            log!("accessibility of node {id}: {e}");
        }
        if enabled_changed {
            self.restyle(id);
        }
    }

    fn set_value(&mut self, view: &NodeId, value: f64) {
        let id = *view;
        let Some(n) = self.scene.get_mut(id) else {
            return;
        };
        if n.value.to_bits() == value.to_bits() {
            return;
        }
        let kind = n.kind;
        n.value = value;
        if kind == NodeKind::Timer {
            n.text = prism_core::timer_text(value, &n.placeholder);
            self.measurer.forget(id);
            self.restyle(id);
        }
        match kind {
            NodeKind::Tabs | NodeKind::Stack => {
                self.bars.insert(id);
            }
            NodeKind::Screen => {
                if let Some(p) = self.scene.get(id).and_then(|n| n.parent) {
                    self.bars.insert(p);
                }
            }
            _ => {
                if let (Some(n), Some(v)) = (self.scene.get(id), self.views.get(&id))
                    && let Err(e) = controls::set_value(&v.content, n, value)
                {
                    log!("value of node {id}: {e}");
                }
            }
        }
    }

    fn set_scene(&mut self, view: &NodeId, update: &prism_core::scene3d::SceneUpdate, _json: &str) {
        if let Some(Content::Scene(scene)) = self.views.get(view).map(|v| &v.content) {
            scene.set_scene(update, &self.images.sources);
        }
    }

    fn set_drawing(&mut self, view: &NodeId, paths: &[prism_core::Path]) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.paths = paths.to_vec();
        }
        if let Some(v) = self.views.get(view)
            && let Err(e) = controls::set_paths(&v.content, paths, &self.palette)
        {
            log!("drawing of node {view}: {e}");
        }
    }

    fn set_config(&mut self, view: &NodeId, kind: NodeKind, config: &str) {
        let id = *view;
        let Some(n) = self.scene.get_mut(id) else {
            return;
        };
        if n.config == config {
            return;
        }
        n.config = config.to_string();
        let field = n.is_text_field();
        self.measurer.forget(id);
        if field {
            let swap = self.views.get(&id).and_then(|v| {
                controls::configure_field(&v.content, id, config)
                    .ok()
                    .flatten()
            });
            if let Some(content) = swap {
                self.replace_content(id, content);
            }
        } else if let Some(v) = self.views.get_mut(&id) {
            let was_time = matches!(v.content, Content::Time(_));
            let old = v.content.element();
            if let Err(e) = controls::configure(&mut v.content, id, kind, config) {
                log!("config of node {id}: {e}");
            }
            let is_time = matches!(v.content, Content::Time(_));
            if was_time != is_time && kind == NodeKind::DatePicker {
                // A time picker in place of the date picker.
                if let Some(old) = old {
                    let _ = crate::view::detach(&old);
                    if let Ok(coll) = v.outer.Children() {
                        let mut at = 0u32;
                        if coll.IndexOf(&old, &mut at).unwrap_or(false) {
                            let _ = coll.RemoveAt(at);
                        }
                    }
                }
                self.remount_content(id);
            }
        }
        if kind == NodeKind::Video {
            // The poster.
            self.show_video(id);
        }
        let value = self.scene.get(id).map_or(0., |n| n.value);
        if let (Some(n), Some(v)) = (self.scene.get(id), self.views.get(&id)) {
            let _ = controls::set_value(&v.content, n, value);
        }
        if self
            .scene
            .get(id)
            .is_some_and(|n| n.kind == NodeKind::Screen)
            && let Some(p) = self.scene.get(id).and_then(|n| n.parent)
        {
            self.bars.insert(p);
        }
        self.restyle(id);
    }

    fn set_icon(&mut self, view: &NodeId, icon: &str) {
        let id = *view;
        let Some(n) = self.scene.get_mut(id) else {
            return;
        };
        n.icon = icon.to_string();
        let parent = n.parent;
        let kind = n.kind;
        if let Some(v) = self.views.get(&id) {
            let _ = controls::set_icon(&v.content, icon);
        }
        if kind == NodeKind::Tab
            && let Some(p) = parent
        {
            self.bars.insert(p);
        }
    }

    fn set_menu(&mut self, view: &NodeId, menu: Option<&prism_core::menu::MenuSpec>) {
        let id = *view;
        if let Some(n) = self.scene.get_mut(id) {
            n.menu = menu.cloned();
        }
        if let Some(v) = self.views.get(&id) {
            let r = match menu {
                Some(spec) => events::menu(spec).and_then(|m| v.outer.SetContextFlyout(&m)),
                None => v.outer.SetContextFlyout(
                    None::<&crate::winui::xaml::Controls::Primitives::FlyoutBase>,
                ),
            };
            if let Err(e) = r {
                log!("menu of node {id}: {e}");
            }
        }
        self.update_hit(id);
    }

    fn show_menu(
        &mut self,
        view: Option<&NodeId>,
        menu: &prism_core::menu::MenuSpec,
        at: Option<(f32, f32)>,
    ) {
        let (target, x, y) = match view.and_then(|v| self.outer(*v).map(|e| (*v, e))) {
            Some((v, e)) => {
                let (x, y) = at.unwrap_or_else(|| {
                    let h = self.scene.get(v).map_or(0., |n| n.frame.h);
                    (0., h)
                });
                (e, x, y)
            }
            None => {
                let (x, y) = at.unwrap_or((self.size.0 / 2., self.size.1 / 2.));
                (ui(&self.root).expect("root"), x, y)
            }
        };
        if let Err(e) = events::show_menu(&target, menu, x, y) {
            log!("menu: {e}");
        }
    }

    fn set_scroll_events(&mut self, view: &NodeId, on: bool) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.scroll_events = on;
        }
    }

    fn set_appearance(&mut self, appearance: Appearance) {
        if self.appearance != appearance {
            self.appearance = appearance;
            self.retheme();
        }
    }

    fn set_hidden(&mut self, view: &NodeId, hidden: bool) {
        let Some(n) = self.scene.get_mut(*view) else {
            return;
        };
        if n.hidden == hidden {
            return;
        }
        n.hidden = hidden;
        self.update_visibility(*view);
    }

    fn set_content_size(&mut self, view: &NodeId, width: f32, height: f32) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.content = (width, height);
            let (mx, my) = n.max_scroll();
            n.scroll = (n.scroll.0.min(mx), n.scroll.1.min(my));
        }
        if let (Some(n), Some(v)) = (self.scene.get(*view), self.views.get(view)) {
            let _ = v.content_size(n);
        }
    }

    fn set_environment(&mut self, env: &RenderEnvironment) {
        self.env = env.clone();
    }

    fn baseline(&mut self, view: &NodeId, width: f32, height: f32) -> Option<f32> {
        let n = self.scene.get(*view)?;
        controls::baseline(&self.measurer, n, width, height)
    }

    fn set_sticky(&mut self, view: &NodeId, sticky: Option<(&NodeId, prism_native::Sticky)>) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.sticky = sticky.map(|(s, st)| (*s, st));
        }
        self.restyle(*view);
    }

    fn set_interactions(&mut self, view: &NodeId, states: Interactions) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.reports = states;
        }
        self.update_hit(*view);
    }

    fn set_pointer_events(&mut self, view: &NodeId, mask: u8) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.pointer_mask = mask;
        }
        crate::pointer::set_mask(*view, mask);
        self.update_hit(*view);
    }

    fn set_swipe(&mut self, view: &NodeId, left: bool, right: bool) {
        crate::pointer::set_swipe(*view, left, right);
        self.update_hit(*view);
    }

    fn set_drag_delay(&mut self, view: &NodeId, ms: u32) {
        crate::pointer::set_delay(*view, ms);
    }

    fn set_drop_target(&mut self, view: &NodeId, types: Option<&[String]>) {
        if let Some(v) = self.views.get(view)
            && let Err(e) = crate::drops::set_target(&v.outer, *view, types)
        {
            log!("drop target {view}: {e}");
        }
        self.update_hit(*view);
    }

    fn set_drag_preview(&mut self, view: &NodeId, at: Option<(f32, f32)>) {
        self.drag_preview(*view, at);
    }

    fn rich_input(&self) -> bool {
        true
    }

    fn set_rich_value(&mut self, view: &NodeId, runs: &str) {
        if let (Some(n), Some(v)) = (self.scene.get(*view), self.views.get(view))
            && let Content::Rich(r) = &v.content
            && let Err(e) = r.set_runs(runs, &n.style, &self.palette, self.scale())
        {
            log!("rich input {view}: {e}");
        }
    }

    fn set_edit_events(&mut self, view: &NodeId, mask: u8) {
        if let Some(Content::Rich(r)) = self.views.get(view).map(|v| &v.content) {
            crate::edit::set_mask(*view, &r.element, mask);
        }
    }

    fn write_clipboard(&mut self, _view: &NodeId, content: &prism_core::ClipboardContent) {
        crate::edit::write_clipboard(content);
    }

    fn insert_text(&mut self, view: &NodeId, text: &str) {
        if let Some(Content::Rich(r)) = self.views.get(view).map(|v| &v.content) {
            crate::edit::insert_text(&r.element, text);
        }
    }

    fn set_remote_carets(&mut self, view: &NodeId, carets: &[prism_core::RemoteCaret]) {
        if let Some(v) = self.views.get(view)
            && let Some(f) = crate::carets::Field::of(&v.content)
        {
            crate::carets::set_carets(*view, &v.outer, f, carets, &self.palette);
        }
    }

    fn set_selection_highlight(&mut self, view: &NodeId, start: usize, end: usize) {
        if let Some(v) = self.views.get(view)
            && let Some(f) = crate::carets::Field::of(&v.content)
        {
            crate::carets::set_highlight(*view, &v.outer, f, start, end, &self.palette);
        }
    }

    fn post_web_message(&mut self, view: &NodeId, data: &str) {
        if let Some(Content::Web(w)) = self.views.get(view).map(|v| &v.content) {
            crate::web::post(w, data);
        }
    }

    fn reload_web_view(&mut self, view: &NodeId) {
        if let Some(Content::Web(w)) = self.views.get(view).map(|v| &v.content) {
            crate::web::reload(w);
        }
    }

    fn set_key_events(&mut self, view: &NodeId, local: bool, global: bool) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.keys_local = local;
            n.keys_global = global;
        }
    }

    fn set_selection(&mut self, view: &NodeId, start: usize, end: usize) {
        match self.views.get(view).map(|v| &v.content) {
            Some(Content::Field(t)) => {
                let text = t.Text().map(|s| s.to_string()).unwrap_or_default();
                let a = prism_native::fields::chars_to_utf16(&text, start);
                let b = prism_native::fields::chars_to_utf16(&text, end);
                let _ = t.Select(a as i32, b.saturating_sub(a) as i32);
            }
            Some(Content::Rich(r)) => {
                let text = r.text().unwrap_or_default();
                let a = prism_native::fields::chars_to_utf16(&text, start);
                let b = prism_native::fields::chars_to_utf16(&text, end);
                let _ = r.select(a, b);
            }
            _ => {}
        }
    }

    fn set_select_events(&mut self, view: &NodeId, on: bool) {
        if on {
            self.select_events.insert(*view);
        } else {
            self.select_events.remove(view);
        }
    }

    fn focus(&mut self, view: &NodeId) {
        self.focus_requests.push(Some(*view));
    }

    fn dismiss_keyboard(&mut self) {
        self.focus_requests.push(None);
    }

    fn scroll_into_view(&mut self, view: &NodeId) {
        if let Some(e) = self.outer(*view) {
            let _ = e.StartBringIntoView();
        }
    }

    fn scroll_to(&mut self, view: &NodeId, x: f32, y: f32, relative: bool, animated: bool) {
        let Some(n) = self.scene.get(*view) else {
            return;
        };
        let (tx, ty) = if relative {
            (n.scroll.0 + x, n.scroll.1 + y)
        } else {
            (x, y)
        };
        self.scroll_view_to(*view, tx, ty, animated);
    }

    fn scroll_offset(&mut self, view: &NodeId) -> (f32, f32) {
        self.scene.get(*view).map_or((0., 0.), |n| n.scroll)
    }

    fn window_origin(&mut self, view: &NodeId) -> Option<(f32, f32)> {
        self.scene.get(*view).map(|_| self.scene.origin(*view))
    }

    fn node_at(&mut self, x: f32, y: f32) -> Option<NodeId> {
        self.scene.hit_test(x, y)
    }

    fn set_layers(&mut self, layers: &[NodeId]) {
        self.scene.layers = layers.to_vec();
        self.mount();
    }

    // Animations: `motion.rs`.

    fn set_frame_animated(
        &mut self,
        view: &NodeId,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        t: prism_core::Transition,
    ) {
        self.frame_animated(*view, Rect::new(x, y, w, h), t);
    }

    fn set_style_animated(&mut self, view: &NodeId, style: &Style, t: prism_core::Transition) {
        self.style_animated(*view, style, t);
    }

    fn animate_exit(
        &mut self,
        parent: &NodeId,
        view: &NodeId,
        to: &Style,
        t: prism_core::Transition,
    ) {
        self.exit_animated(*parent, *view, to, t);
    }

    fn set_repeat(
        &mut self,
        view: &NodeId,
        style: &Style,
        repeat: Option<(prism_core::Pose, prism_core::Transition)>,
    ) {
        self.repeat_style(*view, style, repeat);
    }
}

impl Win {
    /// A field's content became another control (secure ↔ plain).
    fn replace_content(&mut self, id: NodeId, content: Content) {
        let Some(v) = self.views.get_mut(&id) else {
            return;
        };
        if let Some(old) = v.content.element() {
            let _ = crate::view::detach(&old);
        }
        v.content = content;
        if let Some(c) = v.content.control()
            && let Ok(e) = ui(&c)
        {
            let _ = events::focus(&e, id);
        }
        self.remount_content(id);
    }

    /// Puts a node's content element back in its outer canvas (after it
    /// was replaced).
    fn remount_content(&mut self, id: NodeId) {
        let sticky = self.sticky(id);
        let (Some(n), Some(v)) = (self.scene.get(id), self.views.get_mut(&id)) else {
            return;
        };
        if let Some(e) = v.content.element() {
            let _ = crate::view::detach(&e);
            if let Ok(coll) = v.outer.Children() {
                let _ = coll.Append(&e);
            }
            if let Some(c) = v.content.control()
                && let Ok(ce) = ui(&c)
            {
                let _ = events::focus(&ce, id);
            }
        }
        v.frame = Rect::default();
        let _ = v.place(n, &self.palette, sticky);
    }
}

/// Prism's accessibility on the node's element: name, help text, heading
/// level, live region, hidden; WinUI's own automation peers do the rest.
fn accessibility(v: &View, a: &Accessibility) -> Result<()> {
    let target: UIElement = match (&v.button, v.content.element()) {
        (Some(b), _) => b.cast()?,
        (None, Some(e)) => e,
        (None, None) => v.outer(),
    };
    let name = a.label.clone().unwrap_or_default();
    AutomationProperties::SetName(&target, &h(&name))?;
    AutomationProperties::SetHelpText(&target, &h(a.hint.as_deref().unwrap_or("")))?;
    AutomationProperties::SetHeadingLevel(
        &target,
        match a.role {
            Some(Role::Heading(l)) => AutomationHeadingLevel(l.clamp(1, 9) as i32),
            _ => AutomationHeadingLevel::None,
        },
    )?;
    AutomationProperties::SetLiveSetting(
        &target,
        match a.role {
            Some(Role::Alert) => AutomationLiveSetting::Assertive,
            Some(Role::Status) => AutomationLiveSetting::Polite,
            _ => AutomationLiveSetting::Off,
        },
    )?;
    AutomationProperties::SetAccessibilityView(
        &target,
        if a.hidden {
            AccessibilityView::Raw
        } else {
            AccessibilityView::Content
        },
    )?;
    if let Some(prism_core::Tooltip::Text(t)) = &a.tooltip {
        ToolTipService::SetToolTip(&target, &boxed(t)?)?;
    }
    Ok(())
}
