//! The underlines Prism draws over a text (`underline.rs`): a canvas over
//! the node's `TextBlock`, never hit, clipped to it, with a stroked `Path`
//! per underline. The characters' boxes are XAML's own
//! (`TextPointer::GetCharacterRect`), measured on the next turn after the
//! text, its style or its size change. A rich input's are drawn with its
//! remote carets (`carets.rs`), from RichEdit's boxes.

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::{Color, NodeId};
use prism_native::text::StyledText;
use windows::Foundation::{Point, Rect as WinRect};
use windows_core::{Interface, Result};

use crate::fluent::Palette;
use crate::underline::{self, Mark, Segment};
use crate::winui::bindings::Microsoft::UI::Dispatching::{DispatcherQueue, DispatcherQueueHandler};
use crate::winui::xaml::Controls::{Canvas, TextBlock, UIElementCollection};
use crate::winui::xaml::Documents::{Hyperlink, LogicalDirection, Run};
use crate::winui::xaml::Media::{
    DoubleCollection, LineSegment, PathFigure, PathGeometry, PenLineCap, RectangleGeometry,
};
use crate::winui::xaml::Shapes::Path;
use crate::winui::xaml::SizeChangedEventHandler;
use crate::winui::{brush, ui};

struct Layer {
    canvas: Canvas,
    text: TextBlock,
    /// The underlines and their colors.
    marks: Vec<(Mark, Color)>,
}

thread_local! {
    static LAYERS: RefCell<HashMap<NodeId, Layer>> = RefCell::new(HashMap::new());
    /// Layers to measure and draw on the next turn.
    static DUE: RefCell<Vec<NodeId>> = const { RefCell::new(Vec::new()) };
}

/// Node `id`'s text `tb` (in `outer`) shows `st`, `ink` its color: the
/// underlines Prism draws, if any.
pub fn sync(
    id: NodeId,
    outer: &Canvas,
    tb: &TextBlock,
    st: &StyledText,
    pal: &Palette,
    ink: Color,
) -> Result<()> {
    let marks = colored(underline::marks(st, false), pal, ink);
    if marks.is_empty() {
        forget(id);
        return Ok(());
    }
    let made = LAYERS.with(|l| l.borrow().contains_key(&id));
    if !made {
        let canvas = Canvas::new()?;
        canvas.SetIsHitTestVisible(false)?;
        outer.Children()?.Append(&canvas)?;
        tb.SizeChanged(&SizeChangedEventHandler::new(move |_, _| {
            schedule(id);
            Ok(())
        }))?;
        LAYERS.with(|l| {
            l.borrow_mut().insert(
                id,
                Layer {
                    canvas,
                    text: tb.clone(),
                    marks: Vec::new(),
                },
            )
        });
    }
    LAYERS.with(|l| {
        if let Some(layer) = l.borrow_mut().get_mut(&id) {
            layer.marks = marks;
        }
    });
    schedule(id);
    Ok(())
}

/// Each underline's color: its own, else its run's, else the text's.
pub fn colored(marks: Vec<Mark>, pal: &Palette, ink: Color) -> Vec<(Mark, Color)> {
    marks
        .into_iter()
        .map(|m| {
            let c = m.color.or(m.ink).map_or(ink, |c| pal.ink(c));
            (m, c)
        })
        .collect()
}

/// Node `id` is gone, or its text has no drawn underline.
pub fn forget(id: NodeId) {
    if let Some(layer) = LAYERS.with(|l| l.borrow_mut().remove(&id))
        && let Ok(e) = ui(&layer.canvas)
    {
        let _ = crate::view::detach(&e);
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
                let r = LAYERS.with(|l| match l.borrow().get(&id) {
                    Some(layer) => draw(layer),
                    None => Ok(()),
                });
                if let Err(e) = r {
                    crate::win::log!("underlines of node {id}: {e}");
                }
            }
            Ok(())
        }));
    }
}

fn draw(layer: &Layer) -> Result<()> {
    let kids = layer.canvas.Children()?;
    kids.Clear()?;
    let tb = &layer.text;
    let (w, h) = (tb.ActualWidth()?, tb.ActualHeight()?);
    // Where the text is in the canvas (both in the node's outer canvas).
    let at = tb
        .TransformToVisual(&layer.canvas)?
        .TransformPoint(Point { X: 0., Y: 0. })?;
    let clip = RectangleGeometry::new()?;
    clip.SetRect(WinRect {
        X: at.X,
        Y: at.Y,
        Width: w as f32,
        Height: h as f32,
    })?;
    layer.canvas.SetClip(&clip)?;
    // Lines share the first line's baseline offset.
    let first_top = tb
        .ContentStart()?
        .GetCharacterRect(LogicalDirection::Forward)
        .map_or(0., |r| r.Y);
    let baseline = tb.BaselineOffset()? as f32 - first_top;
    let inlines = tb.Inlines()?;
    for (m, color) in &layer.marks {
        let Some(run) = run_at(&inlines, m.run) else {
            continue;
        };
        let start = run.ContentStart()?;
        let len = run.Text()?.len() as i32;
        let boxes = (0..len.min(4096)).filter_map(|i| {
            let r = start
                .GetPositionAtOffset(i, LogicalDirection::Forward)
                .ok()?
                .GetCharacterRect(LogicalDirection::Forward)
                .ok()?;
            Some((r.X + at.X, r.Y + at.Y, r.Width, r.Height))
        });
        let lines = underline::segments(boxes);
        add_path(&kids, &lines, m, *color, |s| s.top + baseline)?;
    }
    Ok(())
}

/// Run `i` of a text's inlines (a link's run inside its `Hyperlink`).
fn run_at(inlines: &crate::winui::xaml::Documents::InlineCollection, i: usize) -> Option<Run> {
    let inline = inlines.GetAt(i as u32).ok()?;
    if let Ok(r) = inline.cast::<Run>() {
        return Some(r);
    }
    inline
        .cast::<Hyperlink>()
        .ok()?
        .Inlines()
        .ok()?
        .GetAt(0)
        .ok()?
        .cast()
        .ok()
}

/// Mark `m`'s lines under `lines` (each line's baseline from `baseline`),
/// one stroked path, in `kids`.
pub fn add_path(
    kids: &UIElementCollection,
    lines: &[Segment],
    m: &Mark,
    color: Color,
    baseline: impl Fn(&Segment) -> f32,
) -> Result<()> {
    if lines.is_empty() {
        return Ok(());
    }
    let stroke = underline::stroke(m.style, m.size);
    let geometry = PathGeometry::new()?;
    let figures = geometry.Figures()?;
    for s in lines {
        for off in &stroke.offsets {
            let y = underline::snap(baseline(s) + off, stroke.thickness);
            let f = PathFigure::new()?;
            f.SetStartPoint(Point { X: s.x0, Y: y })?;
            f.SetIsClosed(false)?;
            f.SetIsFilled(false)?;
            let to = LineSegment::new()?;
            to.SetPoint(Point { X: s.x1, Y: y })?;
            f.Segments()?.Append(&to)?;
            figures.Append(&f)?;
        }
    }
    let path = Path::new()?;
    path.SetData(&geometry)?;
    path.SetStroke(&brush(color)?)?;
    path.SetStrokeThickness(stroke.thickness as f64)?;
    if let Some((on, off)) = stroke.dash {
        let dashes = DoubleCollection::new()?;
        dashes.Append(on as f64)?;
        dashes.Append(off as f64)?;
        path.SetStrokeDashArray(&dashes)?;
    }
    if stroke.round {
        path.SetStrokeDashCap(PenLineCap::Round)?;
        path.SetStrokeStartLineCap(PenLineCap::Round)?;
        path.SetStrokeEndLineCap(PenLineCap::Round)?;
    }
    path.SetIsHitTestVisible(false)?;
    kids.Append(&ui(&path)?)
}
