//! `prism_native::Widgets` over the scene: prism-native's calls create,
//! style and place scene nodes; what changed is repainted (only that:
//! the window region each touched node was drawn in and the one it is
//! drawn in now), text fields are moved over their node.

use std::rc::Rc;

use prism_core::{
    Accessibility, Appearance, IdMap, Interactions, NodeId, NodeKind, RenderEnvironment, Style,
};
use prism_native::{Insets, Viewport, Widgets};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDC, HGDIOBJ, MapWindowPoints, ReleaseDC, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect};
use windows::core::{Interface, Result};

use crate::controls::{self, content_inset, is_field};
use crate::fluent::{Accent, Palette};
use crate::gfx::{Gfx, Surface};
use crate::images::Images;
use crate::paint::{self, Ctx, Part, TargetCache};
use crate::scene::{HEADER, Rect, Scene, TAB_BAR};
use crate::text::TextCache;
use crate::win::{hwnd, invalidate, px_rect};
use crate::{fields, system};

/// The window and everything drawn in it.
pub struct Win {
    pub hwnd: HWND,
    pub gfx: Rc<Gfx>,
    pub scene: Scene,
    pub texts: TextCache,
    pub images: Images,
    pub surface: Surface,
    pub cache: TargetCache,
    /// Where each node was last drawn (window, view units).
    pub painted: IdMap<NodeId, Rect>,
    /// Nodes changed since the last repaint request.
    dirty: Vec<NodeId>,
    dirty_all: bool,
    /// Frames or scroll offsets changed: text fields move.
    pub moved: bool,
    pub dpi: u32,
    /// Client area, view units.
    pub size: (f32, f32),
    pub system_dark: bool,
    pub appearance: Appearance,
    pub accent: Accent,
    pub palette: Palette,
    pub env: RenderEnvironment,
    pub locale: prism_core::Locale,
    /// The node with the keyboard focus (a field's own window has the
    /// Win32 focus then).
    pub focus: Option<NodeId>,
    /// Focus moved with the keyboard: its ring shows.
    pub focus_visible: bool,
    pub hover: Option<(NodeId, Part)>,
    pub pressed: Option<(NodeId, Part)>,
    /// `cx.focus(…)` and `dismiss_keyboard` while patches apply: carried
    /// out after (they report to prism-native).
    pub focus_requests: Vec<Option<NodeId>>,
    pub app_title: String,
    /// Fields reporting their selection (`on_select`).
    pub select_events: prism_core::IdSet<NodeId>,
    /// What each field sent and the app has not rendered back yet (#73).
    pub typed: IdMap<NodeId, prism_native::fields::Typed>,
}

impl Win {
    pub fn new(hwnd: HWND, gfx: Rc<Gfx>, images: Images, title: String, dpi: u32) -> Win {
        let system_dark = system::dark_mode();
        let accent = system::accent();
        Win {
            hwnd,
            gfx,
            scene: Scene::default(),
            texts: TextCache::default(),
            images,
            surface: Surface::new(hwnd),
            cache: TargetCache::default(),
            painted: IdMap::default(),
            dirty: Vec::new(),
            dirty_all: true,
            moved: true,
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
            hover: None,
            pressed: None,
            focus_requests: Vec::new(),
            app_title: title,
            select_events: Default::default(),
            typed: IdMap::default(),
        }
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

    /// The theme changed (system, app, accent): every color again.
    pub fn retheme(&mut self) {
        self.palette = Palette::new(self.dark(), self.accent);
        crate::imp::set_dark_title_bar(self.hwnd, self.dark());
        let ids: Vec<NodeId> = self
            .scene
            .nodes
            .iter()
            .filter(|(_, n)| n.field.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.style_field(id);
        }
        self.repaint_all();
    }

    /// A node needs repainting where it was and where it is.
    pub fn mark(&mut self, id: NodeId) {
        if !self.dirty_all {
            self.dirty.push(id);
            if self.dirty.len() > 400 {
                self.dirty_all = true;
                self.dirty.clear();
            }
        }
    }

    pub fn repaint_all(&mut self) {
        self.dirty_all = true;
        self.dirty.clear();
    }

    /// Turns what changed into invalidated window regions.
    pub fn flush(&mut self) {
        let scale = self.scale();
        if self.dirty_all {
            self.dirty_all = false;
            self.dirty.clear();
            invalidate(self.hwnd, None, scale);
        } else {
            let mut ids = std::mem::take(&mut self.dirty);
            ids.sort_unstable();
            ids.dedup();
            for id in ids {
                let mut r = self.painted.get(&id).copied().unwrap_or_default();
                if let Some(now) = self.visual_rect(id) {
                    r = r.union(&now);
                }
                if !r.is_empty() {
                    invalidate(self.hwnd, Some(r), scale);
                }
            }
        }
        if self.moved {
            self.moved = false;
            self.place_fields();
        }
    }

    /// Where a node draws now (window, view units), transforms and
    /// shadows included.
    fn visual_rect(&self, id: NodeId) -> Option<Rect> {
        let n = self.scene.get(id)?;
        if !self.scene.is_shown(id) {
            return None;
        }
        let mut m = windows_numerics::Matrix3x2::identity();
        let mut chain = self.scene.chain(id);
        chain.reverse();
        for (i, a) in chain.iter().enumerate() {
            m = paint::local_matrix(&self.scene, *a) * m;
            let an = self.scene.get(*a)?;
            if an.kind == NodeKind::Scroll && i + 1 < chain.len() {
                m = windows_numerics::Matrix3x2::translation(-an.scroll.0, -an.scroll.1) * m;
            }
        }
        let local = Rect::new(0., 0., n.frame.w, n.frame.h).inflate(paint::overhang(&n.style));
        Some(paint::bounds(&m, local))
    }

    /// A node and everything in it changed place.
    fn mark_subtree(&mut self, id: NodeId) {
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            self.mark(n);
            if self.dirty_all {
                return;
            }
            if let Some(node) = self.scene.get(n) {
                stack.extend(node.children.iter().copied());
            }
        }
    }

    // --- Painting -------------------------------------------------------------

    /// `WM_PAINT`: repaints `update` (client pixels) and presents it.
    pub fn paint(&mut self, hdc: windows::Win32::Graphics::Gdi::HDC, update: Option<RECT>) {
        let mut client = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut client);
        }
        let size = (client.right.max(0) as u32, client.bottom.max(0) as u32);
        let dpi = self.dpi as f32;
        if self.cache.generation != self.surface.generation {
            self.cache = TargetCache::new(self.surface.generation);
        }
        let Win {
            surface,
            gfx,
            scene,
            texts,
            images,
            cache,
            painted,
            palette,
            focus_visible,
            hover,
            pressed,
            locale,
            ..
        } = self;
        let gfx: &Gfx = gfx;
        let mut draw = |dc: &ID2D1DeviceContext, clip: Option<Rect>| -> Result<()> {
            let brush = unsafe { dc.CreateSolidColorBrush(&D2D1_COLOR_F::default(), None)? };
            let mut ctx = Ctx {
                dc,
                gfx,
                scene,
                pal: palette,
                texts,
                images,
                cache,
                painted,
                clip,
                dpi,
                focus_visible: *focus_visible,
                hover: *hover,
                pressed: *pressed,
                brush,
                locale,
            };
            paint::draw(&mut ctx)
        };
        surface.paint(gfx, hdc, size, dpi, update, &mut draw);
    }

    /// Paints the whole window now, through its DC (outside `WM_PAINT`).
    pub fn paint_now(&mut self) {
        unsafe {
            let hdc = GetDC(Some(self.hwnd));
            self.paint(hdc, None);
            ReleaseDC(Some(self.hwnd), hdc);
        }
    }

    /// The window as drawn, text fields included, as a PNG (smoke tests,
    /// over SSH too: no desktop compositor needed).
    pub fn snapshot(&mut self, path: &str) -> Result<()> {
        let mut client = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut client);
        }
        let (w, h) = (client.right.max(1) as u32, client.bottom.max(1) as u32);
        let dpi = self.dpi as f32;
        let mut cache = TargetCache::default();
        let mut painted = IdMap::default();
        let Win {
            gfx,
            scene,
            texts,
            images,
            palette,
            focus_visible,
            hover,
            pressed,
            locale,
            ..
        } = self;
        let gfx: &Gfx = gfx;
        let bmp = gfx.offscreen(w, h, dpi, |dc| {
            let brush = unsafe { dc.CreateSolidColorBrush(&D2D1_COLOR_F::default(), None)? };
            let mut ctx = Ctx {
                dc,
                gfx,
                scene,
                pal: palette,
                texts,
                images,
                cache: &mut cache,
                painted: &mut painted,
                clip: None,
                dpi,
                focus_visible: *focus_visible,
                hover: *hover,
                pressed: *pressed,
                brush,
                locale,
            };
            paint::draw(&mut ctx)
        })?;
        // The native editors, painted by themselves, where they show
        // (`WM_PRINT` ignores their window region).
        let scale = self.scale();
        for (&id, n) in self.scene.nodes.iter() {
            let Some(f) = n.field.map(hwnd) else { continue };
            if !fields::shown(f) {
                continue;
            }
            let Some(visible) = self.scene.visible_rect(id) else {
                continue;
            };
            let visible = px_rect(visible, scale);
            let holes: Vec<RECT> = self
                .scene
                .covering(id)
                .into_iter()
                .map(|r| px_rect(r, scale))
                .collect();
            let inside =
                |r: &RECT, x: i32, y: i32| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
            if let Some((x, y, cw, ch, px)) = capture_child(self.hwnd, f) {
                self.gfx.blit(&bmp, (x, y), (cw, ch), &px, &|wx, wy| {
                    inside(&visible, wx, wy) && !holes.iter().any(|h| inside(h, wx, wy))
                })?;
            }
        }
        self.gfx.save_png(path, &bmp.cast()?)
    }

    // --- Text fields ----------------------------------------------------------

    /// Font and colors of a field's editor from its node.
    pub fn style_field(&mut self, id: NodeId) {
        let Some(n) = self.scene.get(id) else { return };
        let Some(f) = n.field.map(hwnd) else { return };
        let s = &n.style;
        let pal = self.palette;
        let family = crate::text::family_name(&self.gfx, &s.font_family, s.font_size);
        let text = s.text_color.map_or(pal.text, |c| pal.resolve(c));
        let mut under = controls::backdrop_color(&self.scene, &pal, id);
        if !s.own_field_look() {
            let fill = if n.focused {
                pal.input_active
            } else {
                pal.control
            };
            under = crate::fluent::over(fill, under);
        }
        let config = prism_core::controls::InputConfig::decode(&n.config);
        fields::set_look(
            f,
            &family,
            s.font_size * self.scale(),
            s.font_weight.css(),
            s.italic,
            crate::fluent::over(text, under),
            under,
            crate::fluent::over(pal.text_secondary, under),
            config.secure,
            config.max_length,
        );
    }

    /// Moves every editor over its node (after a layout, a scroll, a
    /// navigation), clipped to what is visible of it.
    pub fn place_fields(&mut self) {
        let scale = self.scale();
        let overlay = self
            .scene
            .open_overlays()
            .last()
            .and_then(|o| self.scene.get(*o))
            .and_then(|o| o.children.first().copied());
        let ids: Vec<NodeId> = self
            .scene
            .nodes
            .iter()
            .filter(|(_, n)| n.field.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let n = &self.scene.nodes[&id];
            let f = hwnd(n.field.expect("field"));
            // Under an open overlay: hidden (drawn overlays cannot cover
            // child windows).
            let covered =
                overlay.is_some_and(|o| id != o && !self.scene.ancestors(id).any(|a| a == o));
            let visible = if covered {
                None
            } else {
                self.scene.visible_rect(id)
            };
            let Some(visible) = visible else {
                fields::place(f, None, None, &[]);
                continue;
            };
            let r = self.scene.window_rect(id);
            let (l, t, rr, b) = content_inset(n);
            let mut inner = Rect::new(
                r.x + l,
                r.y + t,
                (r.w - l - rr).max(1.),
                (r.h - t - b).max(1.),
            );
            if n.kind == NodeKind::TextInput {
                // One line, centered like the drawn text would be.
                let lh = (n.style.font_size * 1.36).ceil() + 2.;
                inner.y = r.y + t + ((r.h - t - b) - lh) / 2.;
                inner.h = lh;
            }
            let px = px_rect(inner, scale);
            let vis = px_rect(visible, scale);
            let holes: Vec<RECT> = self
                .scene
                .covering(id)
                .into_iter()
                .map(|r| px_rect(r, scale))
                .collect();
            fields::place(f, Some(px), Some(vis), &holes);
        }
    }

    pub fn field_of(&self, id: NodeId) -> Option<HWND> {
        self.scene.get(id).and_then(|n| n.field).map(hwnd)
    }

    // --- Scrolling ------------------------------------------------------------

    /// Scrolls a scroll view by `(dx, dy)`; repaints and moves fields.
    pub fn scroll_by(&mut self, id: NodeId, dx: f32, dy: f32) -> bool {
        if !self.scene.scroll_by(id, dx, dy) {
            return false;
        }
        if let Some(v) = self.scene.visible_rect(id) {
            invalidate(self.hwnd, Some(v.inflate(2.)), self.scale());
        } else {
            self.repaint_all();
        }
        self.moved = true;
        true
    }
}

/// Paints a child window (a field) with `WM_PRINT` into memory: what
/// works in session 0, where `PrintWindow` and `BitBlt` do not.
fn capture_child(parent: HWND, child: HWND) -> Option<(i32, i32, i32, i32, Vec<u8>)> {
    unsafe {
        let mut wr = RECT::default();
        GetWindowRect(child, &mut wr).ok()?;
        let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut pts = [POINT {
            x: wr.left,
            y: wr.top,
        }];
        MapWindowPoints(None, Some(parent), &mut pts);
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(mem), &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let old = SelectObject(mem, HGDIOBJ(dib.0));
        fields::print(child, mem);
        let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec();
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        Some((pts[0].x, pts[0].y, w, h, px))
    }
}

impl Widgets for Win {
    type View = NodeId;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> NodeId {
        self.scene.create(id, kind);
        if is_field(kind)
            && let Some(f) = fields::create(self.hwnd, id, kind != NodeKind::TextInput)
        {
            self.scene.get_mut(id).expect("node").field = Some(f.0 as isize);
        }
        if kind == NodeKind::DatePicker
            && let Some(n) = self.scene.get_mut(id)
        {
            n.value = f64::NAN;
        }
        id
    }

    fn set_style(&mut self, view: &NodeId, style: &Style) {
        let id = *view;
        self.mark(id);
        let Some(n) = self.scene.get_mut(id) else {
            return;
        };
        let moved = n.style.transform != style.transform
            || n.style.translate_percent != style.translate_percent;
        n.style = style.clone();
        self.texts.forget(id);
        if moved {
            self.mark_subtree(id);
            self.moved = true;
        } else {
            self.mark(id);
        }
        self.style_field(id);
    }

    fn set_text(&mut self, view: &NodeId, text: &str) {
        let id = *view;
        let Some(n) = self.scene.get_mut(id) else {
            return;
        };
        let kind = n.kind;
        if let Some(f) = n.field.map(hwnd) {
            n.text = text.to_string();
            let current = fields::text(f);
            let takes = self
                .typed
                .entry(id)
                .or_default()
                .should_apply(text, &current);
            if takes {
                fields::set_text(f, text);
            }
            return;
        }
        if n.text == text {
            return;
        }
        n.text = text.to_string();
        self.texts.forget(id);
        match kind {
            NodeKind::Picture => {
                let wic = self.gfx.wic.clone();
                self.images.request(&wic, text, self.hwnd);
                self.cache.forget(id);
            }
            // A screen's title is its stack's header; a tab's, the bar's.
            NodeKind::Screen | NodeKind::Tab => {
                if let Some(p) = self.scene.get(id).and_then(|n| n.parent) {
                    self.mark(p);
                }
            }
            _ => {}
        }
        self.mark(id);
    }

    fn set_placeholder(&mut self, view: &NodeId, text: &str) {
        let Some(n) = self.scene.get_mut(*view) else {
            return;
        };
        n.placeholder = text.to_string();
        if let Some(f) = n.field.map(hwnd) {
            fields::set_placeholder(f, text);
        }
        if n.kind == NodeKind::Timer {
            n.text = prism_core::timer_text(n.value, text);
            self.texts.forget(*view);
        }
        self.mark(*view);
    }

    fn set_clickable(&mut self, view: &NodeId, clickable: bool) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.clickable = clickable;
        }
    }

    fn set_children(&mut self, parent: &NodeId, children: &[NodeId]) {
        // Where the old children were and where the new ones go.
        self.mark_subtree(*parent);
        self.scene.set_children(*parent, children);
        self.mark_subtree(*parent);
        self.moved = true;
    }

    fn set_root(&mut self, view: &NodeId) {
        self.scene.root = Some(*view);
        self.repaint_all();
        self.moved = true;
    }

    fn destroy(&mut self, view: &NodeId) {
        let id = *view;
        if let Some(r) = self.painted.remove(&id) {
            invalidate(self.hwnd, Some(r), self.scale());
        }
        if let Some(n) = self.scene.remove(id)
            && let Some(f) = n.field
        {
            fields::destroy(hwnd(f));
        }
        self.texts.forget(id);
        self.cache.forget(id);
        self.typed.remove(&id);
        self.select_events.remove(&id);
        if self.focus == Some(id) {
            self.focus = None;
        }
        if self.hover.is_some_and(|(h, _)| h == id) {
            self.hover = None;
        }
        if self.pressed.is_some_and(|(p, _)| p == id) {
            self.pressed = None;
        }
    }

    fn measure(&mut self, view: &NodeId, max_width: Option<f32>) -> (f32, f32) {
        controls::measure(
            &self.gfx,
            &mut self.texts,
            &self.images,
            &self.scene,
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
            let st = controls::styled(n);
            if st.no_wrap || st.max_lines.is_some() {
                return 0.;
            }
            return self
                .texts
                .get(&self.gfx, id, &st, Some(1e5))
                .map_or(0., |l| l.min_width());
        }
        self.measure(view, None).0
    }

    fn set_frame(&mut self, view: &NodeId, x: f32, y: f32, w: f32, h: f32) {
        let id = *view;
        let Some(n) = self.scene.get(id) else { return };
        let frame = Rect::new(x, y, w, h);
        if n.frame == frame {
            return;
        }
        let has_children = !n.children.is_empty();
        if has_children {
            self.mark_subtree(id);
        } else {
            self.mark(id);
        }
        if let Some(n) = self.scene.get_mut(id) {
            n.frame = frame;
            let (mx, my) = n.max_scroll();
            n.scroll = (n.scroll.0.min(mx), n.scroll.1.min(my));
        }
        if has_children {
            self.mark_subtree(id);
        } else {
            self.mark(id);
        }
        self.moved = true;
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
        let (ox, oy) = self.scene.origin(*overlay);
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
                let sw = w.min(560.);
                let sh = if config.medium {
                    (h * 0.5).max(200.)
                } else {
                    (h - 48.).max(200.)
                };
                (sw, sh, (w - sw) / 2., h - sh)
            }
            OverlayKind::Keyboard => (w, 0., 0., h),
        };
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
        if let Some(n) = self.scene.get_mut(*view) {
            if n.a11y.disabled != a11y.disabled {
                self.mark(*view);
            }
            if let Some(n) = self.scene.get_mut(*view) {
                n.a11y = a11y.clone();
            }
        }
        crate::uia::changed(self.hwnd, *view);
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
            self.texts.forget(id);
        }
        match kind {
            // What is shown changes: the tab, the header, a sheet.
            NodeKind::Tabs | NodeKind::Stack | NodeKind::Overlay => {
                self.repaint_all();
                self.moved = true;
            }
            NodeKind::Screen => {
                if let Some(p) = self.scene.get(id).and_then(|n| n.parent) {
                    self.mark(p);
                }
            }
            _ => self.mark(id),
        }
    }

    fn set_drawing(&mut self, view: &NodeId, paths: &[prism_core::Path]) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.paths = paths.to_vec();
        }
        self.mark(*view);
    }

    fn set_config(&mut self, view: &NodeId, _kind: NodeKind, config: &str) {
        let Some(n) = self.scene.get_mut(*view) else {
            return;
        };
        if n.config == config {
            return;
        }
        n.config = config.to_string();
        let field = n.field.is_some();
        self.texts.forget(*view);
        if field {
            self.style_field(*view);
        }
        self.mark(*view);
        if self
            .scene
            .get(*view)
            .is_some_and(|n| n.kind == NodeKind::Screen)
            && let Some(p) = self.scene.get(*view).and_then(|n| n.parent)
        {
            self.mark(p);
        }
    }

    fn set_icon(&mut self, view: &NodeId, icon: &str) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.icon = icon.to_string();
            let parent = n.parent;
            self.mark(*view);
            if let Some(p) = parent {
                self.mark(p);
            }
        }
    }

    fn set_menu(&mut self, view: &NodeId, menu: Option<&prism_core::menu::MenuSpec>) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.menu = menu.cloned();
        }
    }

    fn show_menu(
        &mut self,
        view: Option<&NodeId>,
        menu: &prism_core::menu::MenuSpec,
        at: Option<(f32, f32)>,
    ) {
        let (x, y) = match (view, at) {
            (Some(v), Some((x, y))) => {
                let (ox, oy) = self.scene.origin(*v);
                (ox + x, oy + y)
            }
            (Some(v), None) => {
                let r = self.scene.window_rect(*v);
                (r.x, r.bottom())
            }
            (None, Some(p)) => p,
            (None, None) => (self.size.0 / 2., self.size.1 / 2.),
        };
        crate::input::post_menu(menu.clone(), x, y);
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
        self.mark_subtree(*view);
        self.moved = true;
    }

    fn set_content_size(&mut self, view: &NodeId, width: f32, height: f32) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.content = (width, height);
            let (mx, my) = n.max_scroll();
            n.scroll = (n.scroll.0.min(mx), n.scroll.1.min(my));
            self.mark(*view);
        }
    }

    fn set_environment(&mut self, env: &RenderEnvironment) {
        self.env = env.clone();
    }

    fn baseline(&mut self, view: &NodeId, width: f32, height: f32) -> Option<f32> {
        controls::baseline(
            &self.gfx,
            &mut self.texts,
            &self.scene,
            *view,
            width,
            height,
        )
    }

    fn set_sticky(&mut self, view: &NodeId, sticky: Option<(&NodeId, prism_native::Sticky)>) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.sticky = sticky.map(|(s, st)| (*s, st));
        }
        self.mark(*view);
    }

    fn set_interactions(&mut self, view: &NodeId, states: Interactions) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.reports = states;
        }
    }

    fn set_pointer_events(&mut self, view: &NodeId, mask: u8) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.pointer_mask = mask;
        }
    }

    fn set_key_events(&mut self, view: &NodeId, local: bool, global: bool) {
        if let Some(n) = self.scene.get_mut(*view) {
            n.keys_local = local;
            n.keys_global = global;
        }
    }

    fn set_selection(&mut self, view: &NodeId, start: usize, end: usize) {
        if let Some(f) = self.field_of(*view) {
            let text = fields::text(f);
            let a = prism_native::fields::chars_to_utf16(&text, start);
            let b = prism_native::fields::chars_to_utf16(&text, end);
            fields::set_selection(f, a, b);
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
        let id = *view;
        for s in self
            .scene
            .scrollers(id, true)
            .into_iter()
            .filter(|s| *s != id)
        {
            let r = self.scene.window_rect(id);
            let v = self.scene.window_rect(s);
            let dy = if r.y < v.y {
                r.y - v.y
            } else if r.bottom() > v.bottom() {
                (r.bottom() - v.bottom()).min(r.y - v.y)
            } else {
                0.
            };
            if dy != 0. {
                self.scroll_by(s, 0., dy);
            }
        }
    }

    fn scroll_to(&mut self, view: &NodeId, x: f32, y: f32, relative: bool, _animated: bool) {
        let Some(n) = self.scene.get(*view) else {
            return;
        };
        let (dx, dy) = if relative {
            (x, y)
        } else {
            (x - n.scroll.0, y - n.scroll.1)
        };
        self.scroll_by(*view, dx, dy);
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
        self.repaint_all();
    }
}
