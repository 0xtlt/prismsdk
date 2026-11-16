//! `<prism-island-chart>`: a line chart drawn on a canvas, in the browser.
//!
//! The crosshair and read-out follow the pointer at the display's rate with
//! no round trip (the server never hears about pointer moves). A click
//! emits `select` with `[index, value]`; the server shows the selection in
//! normal Prism UI and pushes new points as props.
//!
//! Props: `points` (numbers), `color` (CSS color of the line), `selected`
//! (an index, or `null`).

use std::cell::RefCell;
use std::rc::Rc;

use prism_island::wasm_bindgen::closure::Closure;
use prism_island::wasm_bindgen::{JsCast, UnwrapThrowExt};
use prism_island::{Host, Island, JsValue, Props, js_sys, web_sys};

/// Space around the plot (CSS px): room for the value labels on the left.
const LEFT: f64 = 40.;
const RIGHT: f64 = 12.;
const TOP: f64 = 14.;
const BOTTOM: f64 = 14.;

struct State {
    host: Host,
    canvas: web_sys::HtmlCanvasElement,
    ctx: web_sys::CanvasRenderingContext2d,
    points: Vec<f64>,
    color: String,
    selected: Option<usize>,
    /// The pointer over the plot (CSS px), if it is.
    pointer: Option<f64>,
}

struct Chart {
    state: Rc<RefCell<State>>,
    /// Kept alive while mounted (the canvas's listeners).
    _listeners: Vec<Closure<dyn FnMut(web_sys::MouseEvent)>>,
    resize: Closure<dyn FnMut()>,
}

impl Island for Chart {
    fn mount(host: Host, props: &Props) -> Self {
        let doc = host.document();
        let canvas: web_sys::HtmlCanvasElement =
            doc.create_element("canvas").unwrap_throw().unchecked_into();
        // CSSOM, not a `style` attribute: a strict CSP allows it.
        let style = canvas.style();
        let _ = style.set_property("display", "block");
        let _ = style.set_property("width", "100%");
        let _ = style.set_property("height", "100%");
        let _ = style.set_property("cursor", "crosshair");
        let _ = style.set_property("touch-action", "none");
        let _ = host.root().append_child(&canvas);
        let ctx: web_sys::CanvasRenderingContext2d = canvas
            .get_context("2d")
            .ok()
            .flatten()
            .unwrap_throw()
            .unchecked_into();
        let state = Rc::new(RefCell::new(State {
            host,
            canvas: canvas.clone(),
            ctx,
            points: Vec::new(),
            color: "#5856d6".into(),
            selected: None,
            pointer: None,
        }));
        state.borrow_mut().apply(props);

        let on = |name: &str, f: Box<dyn FnMut(web_sys::MouseEvent)>| {
            let c = Closure::wrap(f);
            let _ = canvas.add_event_listener_with_callback(name, c.as_ref().unchecked_ref());
            c
        };
        let s = state.clone();
        let moved = on(
            "pointermove",
            Box::new(move |e| {
                let mut s = s.borrow_mut();
                s.pointer = Some(e.offset_x() as f64);
                s.draw();
            }),
        );
        let s = state.clone();
        let left = on(
            "pointerleave",
            Box::new(move |_| {
                let mut s = s.borrow_mut();
                s.pointer = None;
                s.draw();
            }),
        );
        let s = state.clone();
        let clicked = on(
            "click",
            Box::new(move |e| {
                let mut s = s.borrow_mut();
                let Some(i) = s.nearest(e.offset_x() as f64) else {
                    return;
                };
                s.selected = Some(i);
                s.draw();
                let detail = prism_island::array([JsValue::from(i as f64), s.points[i].into()]);
                let host = s.host.clone();
                drop(s);
                host.emit("select", detail);
            }),
        );
        let s = state.clone();
        let resize = Closure::<dyn FnMut()>::new(move || s.borrow_mut().draw());
        if let Some(w) = web_sys::window() {
            let _ = w.add_event_listener_with_callback("resize", resize.as_ref().unchecked_ref());
        }
        // Once laid out (the element was just connected).
        let s = state.clone();
        let first = Closure::once_into_js(move || s.borrow_mut().draw());
        if let Some(w) = web_sys::window() {
            let _ = w.request_animation_frame(first.unchecked_ref());
        }
        state.borrow_mut().draw();
        Chart {
            state,
            _listeners: vec![moved, left, clicked],
            resize,
        }
    }

    fn update(&mut self, changed: &Props) {
        let mut s = self.state.borrow_mut();
        s.apply(changed);
        s.draw();
    }

    fn unmount(&mut self) {
        if let Some(w) = web_sys::window() {
            let _ = w.remove_event_listener_with_callback(
                "resize",
                self.resize.as_ref().unchecked_ref(),
            );
        }
    }
}

prism_island::register!(Chart);

/// `v` with `digits` decimals, formatted by JS (Rust's float formatting
/// would add ~10 KB to the bundle).
fn fixed(v: f64, digits: u8) -> String {
    js_sys::Number::from(v)
        .to_fixed(digits)
        .map(String::from)
        .unwrap_or_default()
}

impl State {
    fn apply(&mut self, props: &Props) {
        if let Some(points) = props.f64s("points") {
            self.points = points;
        }
        if let Some(color) = props.str("color") {
            self.color = color;
        }
        if let Some(selected) = props.get("selected") {
            self.selected = selected.as_f64().map(|i| i as usize);
        }
        if self.selected.is_some_and(|i| i >= self.points.len()) {
            self.selected = None;
        }
    }

    /// The plot's size in CSS px.
    fn size(&self) -> (f64, f64) {
        (
            self.canvas.client_width() as f64,
            self.canvas.client_height() as f64,
        )
    }

    /// Lowest and highest value shown (a little headroom).
    fn range(&self) -> (f64, f64) {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for &p in &self.points {
            lo = lo.min(p);
            hi = hi.max(p);
        }
        if !lo.is_finite() {
            return (0., 1.);
        }
        let pad = ((hi - lo) * 0.15).max(1.);
        (lo - pad, hi + pad)
    }

    fn x(&self, i: usize, w: f64) -> f64 {
        let n = self.points.len().max(2) - 1;
        LEFT + (w - LEFT - RIGHT) * i as f64 / n as f64
    }

    fn y(&self, v: f64, h: f64) -> f64 {
        let (lo, hi) = self.range();
        TOP + (h - TOP - BOTTOM) * (1. - (v - lo) / (hi - lo))
    }

    /// The point closest to `x` (CSS px).
    fn nearest(&self, x: f64) -> Option<usize> {
        let n = self.points.len();
        if n == 0 {
            return None;
        }
        let (w, _) = self.size();
        let t = ((x - LEFT) / (w - LEFT - RIGHT).max(1.)).clamp(0., 1.);
        Some(((t * (n.max(2) - 1) as f64).round() as usize).min(n - 1))
    }

    fn draw(&mut self) {
        let (w, h) = self.size();
        if w <= 0. || h <= 0. {
            return;
        }
        let dpr = web_sys::window().map_or(1., |w| w.device_pixel_ratio());
        let (pw, ph) = ((w * dpr).round() as u32, (h * dpr).round() as u32);
        if self.canvas.width() != pw || self.canvas.height() != ph {
            self.canvas.set_width(pw);
            self.canvas.set_height(ph);
        }
        let c = &self.ctx;
        let _ = c.set_transform(dpr, 0., 0., dpr, 0., 0.);
        c.clear_rect(0., 0., w, h);
        // Text and grid in the page's text color (light and dark themes).
        let ink = web_sys::window()
            .and_then(|win| win.get_computed_style(self.host.element()).ok().flatten())
            .and_then(|s| s.get_property_value("color").ok())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "#888".into());
        c.set_font("11px system-ui, sans-serif");
        c.set_text_baseline("middle");
        c.set_text_align("right");
        let (lo, hi) = self.range();
        for k in 0..=3 {
            let v = lo + (hi - lo) * k as f64 / 3.;
            let y = self.y(v, h);
            c.set_global_alpha(0.12);
            c.set_stroke_style_str(&ink);
            c.set_line_width(1.);
            c.begin_path();
            c.move_to(LEFT, y.round() + 0.5);
            c.line_to(w - RIGHT, y.round() + 0.5);
            c.stroke();
            c.set_global_alpha(0.55);
            c.set_fill_style_str(&ink);
            let _ = c.fill_text(&fixed(v, 0), LEFT - 8., y);
        }
        c.set_global_alpha(1.);
        let n = self.points.len();
        if n == 0 {
            return;
        }
        // The area, then the line.
        c.begin_path();
        c.move_to(self.x(0, w), h - BOTTOM);
        for (i, &v) in self.points.iter().enumerate() {
            c.line_to(self.x(i, w), self.y(v, h));
        }
        c.line_to(self.x(n - 1, w), h - BOTTOM);
        c.close_path();
        c.set_global_alpha(0.14);
        c.set_fill_style_str(&self.color);
        c.fill();
        c.set_global_alpha(1.);
        c.begin_path();
        for (i, &v) in self.points.iter().enumerate() {
            let (x, y) = (self.x(i, w), self.y(v, h));
            if i == 0 {
                c.move_to(x, y);
            } else {
                c.line_to(x, y);
            }
        }
        c.set_stroke_style_str(&self.color);
        c.set_line_width(2.);
        c.stroke();
        for i in 0..n {
            let r = if self.selected == Some(i) { 5.5 } else { 2.5 };
            self.dot(i, r, w, h);
        }
        // The crosshair and its read-out.
        let Some(i) = self.pointer.and_then(|x| self.nearest(x)) else {
            return;
        };
        let (x, y) = (self.x(i, w), self.y(self.points[i], h));
        let dash = js_sys::Array::of2(&JsValue::from(3.), &JsValue::from(3.));
        let _ = c.set_line_dash(&dash);
        c.set_global_alpha(0.6);
        c.set_stroke_style_str(&ink);
        c.set_line_width(1.);
        c.begin_path();
        c.move_to(x.round() + 0.5, TOP);
        c.line_to(x.round() + 0.5, h - BOTTOM);
        c.move_to(LEFT, y.round() + 0.5);
        c.line_to(w - RIGHT, y.round() + 0.5);
        c.stroke();
        let _ = c.set_line_dash(&js_sys::Array::new());
        c.set_global_alpha(1.);
        self.dot(i, 4.5, w, h);
        let label = [
            "#",
            &fixed((i + 1) as f64, 0),
            "  ",
            &fixed(self.points[i], 1),
        ]
        .concat();
        let (bw, bh) = (label.len() as f64 * 7. + 16., 22.);
        let bx = (x + 10.).min(w - RIGHT - bw).max(LEFT);
        let by = (y - bh - 10.).max(2.);
        c.set_fill_style_str(&self.color);
        c.begin_path();
        let _ = c.round_rect_with_f64(bx, by, bw, bh, 6.);
        c.fill();
        c.set_fill_style_str("#fff");
        c.set_text_align("center");
        c.set_font("600 12px system-ui, sans-serif");
        let _ = c.fill_text(&label, bx + bw / 2., by + bh / 2.);
    }

    fn dot(&self, i: usize, r: f64, w: f64, h: f64) {
        let c = &self.ctx;
        let (x, y) = (self.x(i, w), self.y(self.points[i], h));
        c.begin_path();
        let _ = c.arc(x, y, r, 0., std::f64::consts::TAU);
        c.set_fill_style_str(&self.color);
        c.fill();
        if r > 3. {
            c.set_line_width(2.);
            c.set_stroke_style_str("#fff");
            c.stroke();
        }
    }
}
