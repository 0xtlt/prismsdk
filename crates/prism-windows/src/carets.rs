//! Other people's carets and selections in text fields (#240), and a
//! field's part of a selection across a selection group (#231), as GTK and
//! macOS draw them: a `Canvas` over the field (`TextBox` or `RichEditBox`),
//! never hit, clipped to it. Each caret is a thin bar in its color with a
//! name tag above the line (under it on the first line; gone when idle),
//! its selection tinted boxes, one per line; the group's part is boxes in
//! the accent color, like the field's own selection.
//!
//! The geometry is the field's own: `TextBox::GetRectFromCharacterIndex`,
//! RichEdit's `ITextRange::GetRect` (its client coordinates start at its
//! text host). It is measured again on the next turn after the carets, the
//! text, the size or the scroll position change. The text, the caret, the
//! selection and IME composition are never touched.

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::{Color, NodeId, RemoteCaret};
use prism_native::fields::chars_to_utf16;
use windows::Foundation::Rect as WinRect;
use windows_core::{HSTRING, Interface, Result};

use crate::fluent::Palette;
use crate::winui::bindings::Microsoft::UI::Dispatching::{DispatcherQueue, DispatcherQueueHandler};
use crate::winui::bindings::Microsoft::UI::Text::{ITextRange, PointOptions, TextGetOptions};
use crate::winui::xaml::Controls::{
    Border, Canvas, RichEditBox, ScrollViewer, ScrollViewerViewChangedEventArgs, TextBlock, TextBox,
};
use crate::winui::xaml::Media::RectangleGeometry;
use crate::winui::xaml::Shapes::Rectangle;
use crate::winui::xaml::{FrameworkElement, SizeChangedEventHandler, UIElement};
use crate::winui::{brush, h, thickness, ui, uniform};

/// The field drawn over.
#[derive(Clone)]
pub enum Field {
    Text(TextBox),
    Rich(RichEditBox),
}

impl Field {
    /// The field of a node's content, if it is a text field.
    pub fn of(content: &crate::controls::Content) -> Option<Field> {
        match content {
            crate::controls::Content::Field(t) => Some(Field::Text(t.clone())),
            crate::controls::Content::Rich(r) => Some(Field::Rich(r.element.clone())),
            _ => None,
        }
    }

    fn element(&self) -> Result<FrameworkElement> {
        match self {
            Field::Text(t) => t.cast(),
            Field::Rich(r) => r.cast(),
        }
    }
}

struct Layer {
    canvas: Canvas,
    field: Field,
    carets: Vec<RemoteCaret>,
    /// The group's part, in characters (`start == end`: none).
    highlight: (usize, usize),
    /// The field's scroll viewer is listened to.
    scroll_wired: bool,
}

thread_local! {
    static LAYERS: RefCell<HashMap<NodeId, Layer>> = RefCell::new(HashMap::new());
    /// Layers to measure and draw on the next turn.
    static DUE: RefCell<Vec<NodeId>> = const { RefCell::new(Vec::new()) };
    static PALETTE: RefCell<Option<Palette>> = const { RefCell::new(None) };
}

/// Shows `carets` over field `field` of node `id` (in `outer`).
pub fn set_carets(id: NodeId, outer: &Canvas, field: Field, carets: &[RemoteCaret], pal: &Palette) {
    update(id, outer, field, pal, |l| l.carets = carets.to_vec());
}

/// Highlights characters `start..end` of field `field` (`start == end`:
/// none).
pub fn set_highlight(
    id: NodeId,
    outer: &Canvas,
    field: Field,
    start: usize,
    end: usize,
    pal: &Palette,
) {
    update(id, outer, field, pal, |l| l.highlight = (start, end));
}

fn update(id: NodeId, outer: &Canvas, field: Field, pal: &Palette, f: impl FnOnce(&mut Layer)) {
    PALETTE.with(|p| *p.borrow_mut() = Some(*pal));
    let made = LAYERS.with(|l| {
        let mut l = l.borrow_mut();
        if let Some(layer) = l.get_mut(&id) {
            f(layer);
            return Ok(());
        }
        let mut layer = new_layer(id, outer, field)?;
        f(&mut layer);
        l.insert(id, layer);
        Ok::<_, windows_core::Error>(())
    });
    if let Err(e) = made {
        crate::win::log!("carets of {id}: {e}");
    }
    schedule(id);
}

/// Field `id` is gone.
pub fn forget(id: NodeId) {
    if let Some(layer) = LAYERS.with(|l| l.borrow_mut().remove(&id))
        && let Ok(e) = ui(&layer.canvas)
    {
        let _ = crate::view::detach(&e);
    }
}

fn new_layer(id: NodeId, outer: &Canvas, field: Field) -> Result<Layer> {
    let canvas = Canvas::new()?;
    canvas.SetIsHitTestVisible(false)?;
    outer.Children()?.Append(&canvas)?;
    let fe = field.element()?;
    fe.SizeChanged(&SizeChangedEventHandler::new(move |_, _| {
        schedule(id);
        Ok(())
    }))?;
    Ok(Layer {
        canvas,
        field,
        carets: Vec::new(),
        highlight: (0, 0),
        scroll_wired: false,
    })
}

/// The field's text changed (or anything moving its text): drawn again.
pub fn changed(id: NodeId) {
    if LAYERS.with(|l| l.borrow().contains_key(&id)) {
        schedule(id);
    }
}

/// Measures and draws layer `id` on the next turn (after XAML's layout).
fn schedule(id: NodeId) {
    let first = DUE.with(|d| {
        let mut d = d.borrow_mut();
        let first = d.is_empty();
        if !d.contains(&id) {
            d.push(id);
        }
        first
    });
    if !first {
        return;
    }
    if let Ok(q) = DispatcherQueue::GetForCurrentThread() {
        let _ = q.TryEnqueue(&DispatcherQueueHandler::new(|| {
            for id in DUE.with(|d| std::mem::take(&mut *d.borrow_mut())) {
                draw(id);
            }
            Ok(())
        }));
    }
}

/// The text shown and a way to measure character edges in it.
struct Measure {
    field: Field,
    text: String,
    /// RichEdit's client coordinates → the field's.
    origin: (f32, f32),
}

impl Measure {
    fn new(field: &Field) -> Result<Measure> {
        let (text, origin) = match field {
            Field::Text(t) => (t.Text()?.to_string(), (0., 0.)),
            Field::Rich(r) => {
                let mut s = HSTRING::new();
                r.Document()?.GetText(TextGetOptions::UseLf, &mut s)?;
                (
                    s.to_string(),
                    host_origin(&r.cast()?, r.GetTemplateChild(&h("ContentElement")).ok()),
                )
            }
        };
        Ok(Measure {
            field: field.clone(),
            text,
            origin,
        })
    }

    /// UTF-16 units in the text.
    fn len(&self) -> i32 {
        self.text.encode_utf16().count() as i32
    }

    /// The box of the character at UTF-16 `i` (x, y, width, height) in
    /// the field.
    fn char_box(&self, i: i32) -> Option<(f32, f32, f32, f32)> {
        match &self.field {
            Field::Text(t) => {
                let a = t.GetRectFromCharacterIndex(i, false).ok()?;
                let b = t.GetRectFromCharacterIndex(i, true).ok()?;
                Some((a.X, a.Y, (b.X - a.X).max(0.), a.Height))
            }
            Field::Rich(r) => {
                let range: ITextRange = r.Document().ok()?.GetRange(i, i + 1).ok()?.cast().ok()?;
                let mut rect = WinRect::default();
                let mut hit = 0;
                range
                    .GetRect(PointOptions::ClientCoordinates, &mut rect, &mut hit)
                    .ok()?;
                Some((
                    rect.X + self.origin.0,
                    rect.Y + self.origin.1,
                    rect.Width,
                    rect.Height,
                ))
            }
        }
    }

    /// The caret's place before character `chars` (x, y, height).
    fn caret(&self, chars: usize) -> Option<(f32, f32, f32)> {
        let i = chars_to_utf16(&self.text, chars) as i32;
        let len = self.len();
        if i < len {
            let (x, y, _, h) = self.char_box(i)?;
            return Some((x, y, h));
        }
        if len == 0 {
            return self.empty_caret();
        }
        // After the last character (on the next line after a break).
        let last = self.text.chars().last();
        let (x, y, w, h) = self.char_box(len - 1)?;
        if matches!(last, Some('\n' | '\r')) {
            let (x0, _, _) = self.empty_caret()?;
            Some((x0, y + h, h))
        } else {
            Some((x + w, y, h))
        }
    }

    /// Where the caret of an empty field is.
    fn empty_caret(&self) -> Option<(f32, f32, f32)> {
        match &self.field {
            Field::Text(t) => {
                let p = t.Padding().ok()?;
                let b = t.BorderThickness().ok()?;
                let size = t.FontSize().ok()? as f32;
                Some((
                    (p.Left + b.Left) as f32,
                    (p.Top + b.Top) as f32,
                    size * 1.33,
                ))
            }
            Field::Rich(r) => {
                let range: ITextRange = r.Document().ok()?.GetRange(0, 0).ok()?.cast().ok()?;
                let mut rect = WinRect::default();
                let mut hit = 0;
                range
                    .GetRect(PointOptions::ClientCoordinates, &mut rect, &mut hit)
                    .ok()?;
                Some((
                    rect.X + self.origin.0,
                    rect.Y + self.origin.1,
                    rect.Height.max(1.),
                ))
            }
        }
    }

    /// One box per line of characters `start..end`.
    fn line_boxes(&self, start: usize, end: usize) -> Vec<(f32, f32, f32, f32)> {
        let (a, b) = (
            chars_to_utf16(&self.text, start) as i32,
            chars_to_utf16(&self.text, end).min(self.len() as usize) as i32,
        );
        let mut lines: Vec<(f32, f32, f32, f32)> = Vec::new();
        // Long selections: measured up to a point.
        for i in a..b.min(a + 4096) {
            let Some((x, y, w, hh)) = self.char_box(i) else {
                continue;
            };
            match lines.last_mut() {
                Some(l) if (l.1 - y).abs() < 0.5 => {
                    let right = (l.0 + l.2).max(x + w);
                    l.0 = l.0.min(x);
                    l.2 = right - l.0;
                    l.3 = l.3.max(hh);
                }
                _ => lines.push((x, y, w.max(4.), hh)),
            }
        }
        lines
    }
}

/// Where RichEdit's text host starts in the box.
fn host_origin(me: &UIElement, host: Option<crate::winui::xaml::DependencyObject>) -> (f32, f32) {
    host.and_then(|h| h.cast::<UIElement>().ok())
        .and_then(|h| h.TransformToVisual(me).ok())
        .and_then(|t| {
            t.TransformPoint(windows::Foundation::Point { X: 0., Y: 0. })
                .ok()
        })
        .map_or((0., 0.), |p| (p.X, p.Y))
}

/// The field's inner scroll viewer, listened to once it exists.
fn wire_scroll(id: NodeId, field: &Field) -> bool {
    let host = match field {
        Field::Text(t) => t.GetTemplateChild(&h("ContentElement")),
        Field::Rich(r) => r.GetTemplateChild(&h("ContentElement")),
    };
    let Some(sv) = host.ok().and_then(|h| h.cast::<ScrollViewer>().ok()) else {
        return false;
    };
    sv.ViewChanged(&windows::Foundation::EventHandler::<
        ScrollViewerViewChangedEventArgs,
    >::new(move |_, _| {
        schedule(id);
        Ok(())
    }))
    .is_ok()
}

fn color(c: Color, alpha: f32, pal: &Palette) -> Color {
    let c = pal.resolve(c);
    c.with_alpha(c.a * alpha)
}

fn add_rect(
    kids: &crate::winui::xaml::Controls::UIElementCollection,
    r: (f32, f32, f32, f32),
    c: Color,
) -> Result<()> {
    let e = Rectangle::new()?;
    e.SetWidth(r.2.max(0.) as f64)?;
    e.SetHeight(r.3.max(0.) as f64)?;
    e.SetFill(&brush(c)?)?;
    let u = ui(&e)?;
    Canvas::SetLeft(&u, r.0 as f64)?;
    Canvas::SetTop(&u, r.1 as f64)?;
    kids.Append(&u)
}

/// Measures and draws layer `id`.
fn draw(id: NodeId) {
    let Some(pal) = PALETTE.with(|p| *p.borrow()) else {
        return;
    };
    let r = LAYERS.with(|l| {
        let mut l = l.borrow_mut();
        let Some(layer) = l.get_mut(&id) else {
            return Ok(());
        };
        if !layer.scroll_wired {
            layer.scroll_wired = wire_scroll(id, &layer.field);
        }
        paint(layer, &pal)
    });
    if let Err(e) = r {
        crate::win::log!("carets of {id}: {e}");
    }
}

fn paint(layer: &Layer, pal: &Palette) -> Result<()> {
    let kids = layer.canvas.Children()?;
    kids.Clear()?;
    let fe = layer.field.element()?;
    let (w, hh) = (fe.ActualWidth()? as f32, fe.ActualHeight()? as f32);
    layer.canvas.SetWidth(w as f64)?;
    layer.canvas.SetHeight(hh as f64)?;
    // Inside the field's border.
    let clip = RectangleGeometry::new()?;
    clip.SetRect(WinRect {
        X: 1.,
        Y: 1.,
        Width: (w - 2.).max(0.),
        Height: (hh - 2.).max(0.),
    })?;
    layer.canvas.SetClip(&clip)?;
    if layer.carets.is_empty() && layer.highlight.0 == layer.highlight.1 {
        return Ok(());
    }
    let m = Measure::new(&layer.field)?;
    // The group's part, like a selection.
    let (a, b) = layer.highlight;
    if a < b {
        let c = pal.accent.with_alpha(0.35);
        for r in m.line_boxes(a, b) {
            add_rect(&kids, r, c)?;
        }
    }
    for c in &layer.carets {
        let range = c.range();
        for r in m.line_boxes(range.start, range.end) {
            add_rect(&kids, r, color(c.color, 0.25, pal))?;
        }
        let Some((x, y, ch)) = m.caret(c.head) else {
            continue;
        };
        add_rect(&kids, (x - 1., y, 2., ch), color(c.color, 1., pal))?;
        // Its name, above the line (under it at the top); gone when idle.
        let Some(label) = c.label.as_deref().filter(|_| !c.idle) else {
            continue;
        };
        let tag = Border::new()?;
        tag.SetBackground(&brush(color(c.color, 1., pal))?)?;
        tag.SetCornerRadius(crate::winui::radius(3., 3., 3., 3.))?;
        tag.SetPadding(thickness(4., 0., 4., 1.))?;
        let t = TextBlock::new()?;
        t.SetText(&h(label))?;
        t.SetFontSize(10.)?;
        t.SetFontWeight(crate::text::weight(prism_core::FontWeight::Semibold))?;
        let fg = c
            .label_text_color
            .map_or(prism_core::rgb(0xFFFFFF), |t| pal.resolve(t));
        t.SetForeground(&brush(fg)?)?;
        tag.SetChild(&t)?;
        tag.SetBorderThickness(uniform(0.))?;
        let e = ui(&tag)?;
        e.Measure(windows::Foundation::Size {
            Width: f32::INFINITY,
            Height: f32::INFINITY,
        })?;
        let size = e.DesiredSize()?;
        let ty = if y - size.Height >= 1. {
            y - size.Height
        } else {
            y + ch
        };
        let tx = (x - 1.).min(w - size.Width - 1.).max(1.);
        Canvas::SetLeft(&e, tx as f64)?;
        Canvas::SetTop(&e, ty as f64)?;
        kids.Append(&e)?;
    }
    Ok(())
}
