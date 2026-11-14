//! `Widgets` for the SwiftUI host: the same flexbox layout as iOS and macOS
//! (prism-native's taffy), computed in Rust. SwiftUI only measures text and
//! draws each node at the frame it is given, so layouts match the other
//! platforms. Changes are queued as JSON and sent once per commit.

use std::collections::HashMap;
use std::ffi::{CString, c_char};

use prism_core::{
    Accessibility, Color, FontWeight, NodeId, NodeKind, RenderEnvironment, Style, TextAlign,
};
use prism_native::{Insets, Viewport, Widgets};
use serde_json::{Value, json};

unsafe extern "C" {
    /// Natural size of a leaf, measured by UIKit/SwiftUI.
    fn prism_watch_measure(
        kind: *const c_char,
        text: *const c_char,
        font: f64,
        weight: i32,
        max_width: f64,
        out_w: *mut f64,
        out_h: *mut f64,
    );
}

#[derive(Default)]
pub(crate) struct WatchWidgets {
    /// Changes for the host, in order.
    pub ops: Vec<Value>,
    kinds: HashMap<NodeId, NodeKind>,
    texts: HashMap<NodeId, String>,
    styles: HashMap<NodeId, Style>,
    /// Content size of each stack or tab container, reported by the host.
    pub viewports: HashMap<NodeId, (f32, f32)>,
    frames: HashMap<NodeId, (f32, f32)>,
}

fn color(c: Option<Color>) -> Value {
    match c {
        None => Value::Null,
        Some(c) => {
            let (l, d) = (c.light_variant(), c.dark_variant());
            json!([l.r, l.g, l.b, l.a, d.r, d.g, d.b, d.a])
        }
    }
}

fn weight(w: FontWeight) -> i32 {
    match w {
        FontWeight::Regular => 400,
        FontWeight::Medium => 500,
        FontWeight::Semibold => 600,
        FontWeight::Bold => 700,
    }
}

impl Widgets for WatchWidgets {
    type View = NodeId;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> NodeId {
        self.kinds.insert(id, kind);
        self.ops
            .push(json!({ "op": "create", "id": id, "kind": kind.name() }));
        id
    }

    /// Only what SwiftUI draws: layout is ours (frames).
    fn set_style(&mut self, id: &NodeId, s: &Style) {
        self.styles.insert(*id, s.clone());
        self.ops.push(json!({ "op": "style", "id": id, "s": {
            "bg": color(s.background),
            "radius": s.corner_radius,
            "bw": s.border_width,
            "bc": if s.border_width > 0. { color(s.border_color) } else { Value::Null },
            "opacity": s.opacity,
            "color": color(s.text_color),
            "font": s.font_size,
            "weight": weight(s.font_weight),
            "talign": match s.text_align { TextAlign::Left => "left", TextAlign::Center => "center", TextAlign::Right => "right" },
        }}));
    }

    fn set_text(&mut self, id: &NodeId, text: &str) {
        self.texts.insert(*id, text.to_string());
        self.ops
            .push(json!({ "op": "text", "id": id, "text": text }));
    }

    fn set_placeholder(&mut self, id: &NodeId, text: &str) {
        self.ops
            .push(json!({ "op": "placeholder", "id": id, "text": text }));
    }

    fn set_clickable(&mut self, id: &NodeId, clickable: bool) {
        self.ops
            .push(json!({ "op": "clickable", "id": id, "on": clickable }));
    }

    fn set_children(&mut self, parent: &NodeId, children: &[NodeId]) {
        self.ops
            .push(json!({ "op": "children", "id": parent, "children": children }));
    }

    fn set_root(&mut self, id: &NodeId) {
        self.ops.push(json!({ "op": "root", "id": id }));
    }

    fn destroy(&mut self, id: &NodeId) {
        self.kinds.remove(id);
        self.texts.remove(id);
        self.styles.remove(id);
        self.viewports.remove(id);
        self.frames.remove(id);
        self.ops.push(json!({ "op": "destroy", "id": id }));
    }

    fn measure(&mut self, id: &NodeId, max_width: Option<f32>) -> (f32, f32) {
        let kind = self.kinds.get(id).copied().unwrap_or(NodeKind::View);
        let style = self.styles.get(id).cloned().unwrap_or_default();
        let text = match kind {
            NodeKind::Timer => "00:00".to_string(),
            _ => self.texts.get(id).cloned().unwrap_or_default(),
        };
        let kind = CString::new(kind.name()).unwrap_or_default();
        let text = CString::new(text).unwrap_or_default();
        let (mut w, mut h) = (0f64, 0f64);
        unsafe {
            prism_watch_measure(
                kind.as_ptr(),
                text.as_ptr(),
                style.font_size as f64,
                weight(style.font_weight),
                max_width.map_or(f64::INFINITY, f64::from),
                &mut w,
                &mut h,
            )
        };
        (w as f32, h as f32)
    }

    /// The longest word: texts wrap down to it, like CSS min-content.
    fn measure_min_content(&mut self, id: &NodeId) -> f32 {
        let kind = self.kinds.get(id).copied().unwrap_or(NodeKind::View);
        let text = self.texts.get(id).cloned().unwrap_or_default();
        if !matches!(kind, NodeKind::Text | NodeKind::Button) || text.is_empty() {
            return self.measure(id, None).0;
        }
        let words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
        let full = self.texts.insert(*id, String::new());
        let mut widest = 0f32;
        for word in words {
            self.texts.insert(*id, word);
            widest = widest.max(self.measure(id, None).0);
        }
        if let Some(full) = full {
            self.texts.insert(*id, full);
        }
        widest
    }

    fn set_frame(&mut self, id: &NodeId, x: f32, y: f32, w: f32, h: f32) {
        self.frames.insert(*id, (w, h));
        self.ops
            .push(json!({ "op": "frame", "id": id, "f": [x, y, w, h] }));
    }

    /// Stacks and tabs: the area SwiftUI gives their content (below the
    /// navigation bar), as the host reported it.
    fn screen_viewport(&mut self, container: &NodeId, _child: &NodeId) -> Viewport {
        let (width, height) = self
            .viewports
            .get(container)
            .or_else(|| self.frames.get(container))
            .copied()
            .unwrap_or((0., 0.));
        Viewport {
            x: 0.,
            y: 0.,
            width,
            height,
            insets: Insets::default(),
        }
    }

    fn set_accessibility(&mut self, id: &NodeId, a11y: &Accessibility) {
        self.ops.push(json!({
            "op": "a11y", "id": id, "label": a11y.label, "hidden": a11y.hidden,
        }));
    }

    fn set_value(&mut self, id: &NodeId, value: f64) {
        self.ops
            .push(json!({ "op": "value", "id": id, "value": value }));
    }

    fn set_icon(&mut self, id: &NodeId, icon: &str) {
        self.ops
            .push(json!({ "op": "icon", "id": id, "icon": icon }));
    }

    fn set_hidden(&mut self, id: &NodeId, hidden: bool) {
        self.ops
            .push(json!({ "op": "display", "id": id, "hidden": hidden }));
    }

    fn set_content_size(&mut self, id: &NodeId, width: f32, height: f32) {
        self.ops
            .push(json!({ "op": "content", "id": id, "size": [width, height] }));
    }

    fn set_menu(&mut self, id: &NodeId, menu: Option<&prism_core::menu::MenuSpec>) {
        let menu = menu.and_then(|m| serde_json::from_str::<Value>(&m.to_json()).ok());
        self.ops
            .push(json!({ "op": "menu", "id": id, "menu": menu }));
    }

    fn set_swipe(&mut self, id: &NodeId, left: bool, right: bool) {
        self.ops
            .push(json!({ "op": "swipe", "id": id, "left": left, "right": right }));
    }

    fn set_scroll_events(&mut self, id: &NodeId, on: bool) {
        self.ops
            .push(json!({ "op": "scrollev", "id": id, "on": on }));
    }

    fn set_environment(&mut self, _env: &RenderEnvironment) {}
}
