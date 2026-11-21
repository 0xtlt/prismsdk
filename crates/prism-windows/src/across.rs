//! One selection across several text fields (#231) on Windows: a mouse
//! drag from a text field (`TextBox`, `RichEditBox`) into the other fields
//! of its selection group (#264), as on macOS and GTK. While the drag
//! stays in its field, the field selects by itself; once it is over
//! another field of the group, every move goes to the runtime
//! (`UiEvent::SelectionAcross`), back over the first field too, and the
//! runtime answers with the selection and the other fields' highlights
//! (`carets.rs` draws them).
//!
//! The keys (Shift+arrows at a field's edge) go through the window's key
//! handling (`Runtime::consumes_key_event`, `keyboard.rs`).
//!
//! The fields take their presses (they mark them handled): they are heard
//! with `handledEventsToo`.

use std::cell::RefCell;

use prism_core::{NodeId, UiEvent};
use windows_core::{Interface, Result};

use crate::imp::{dispatch, with_state};
use crate::winui::bindings::Microsoft::UI::Input::PointerDeviceType;
use crate::winui::xaml::Controls::{RichEditBox, TextBox};
use crate::winui::xaml::Input::{PointerEventHandler, PointerRoutedEventArgs};
use crate::winui::xaml::{FrameworkElement, UIElement};

/// A mouse drag that started in a field of a group.
#[derive(Clone, Copy)]
struct Drag {
    pointer: u32,
    field: NodeId,
    group: NodeId,
    /// Where it started in the field (characters).
    at: usize,
    /// It went over another field: the runtime has it now.
    taken: bool,
}

thread_local! {
    static DRAG: RefCell<Option<Drag>> = const { RefCell::new(None) };
}

/// Listens to the drags of field `id` (element `e`).
pub fn connect(id: NodeId, e: &UIElement) -> Result<()> {
    let handler = |f: fn(NodeId, &UIElement, &PointerRoutedEventArgs) -> Result<()>| {
        crate::boxed::boxed(PointerEventHandler::new(move |s, a| {
            if let (Some(s), Some(a)) = (s.as_ref(), a.as_ref()) {
                f(id, &s.cast()?, a)?;
            }
            Ok(())
        }))
    };
    e.AddHandler(&UIElement::PointerPressedEvent()?, &handler(pressed), true)?;
    e.AddHandler(&UIElement::PointerMovedEvent()?, &handler(moved), true)?;
    e.AddHandler(
        &UIElement::PointerReleasedEvent()?,
        &handler(released),
        true,
    )?;
    e.AddHandler(
        &UIElement::PointerCaptureLostEvent()?,
        &handler(released),
        true,
    )?;
    Ok(())
}

/// The character of field element `e` (a text box or a rich edit box)
/// at `(x, y)`, in its own coordinates.
fn offset_in(e: &UIElement, x: f32, y: f32) -> Option<usize> {
    if let Ok(r) = e.cast::<RichEditBox>() {
        return crate::edit::offset_at(&r, x, y);
    }
    let t = e.cast::<TextBox>().ok()?;
    let text = t.Text().ok()?.to_string();
    let len = text.encode_utf16().count() as i32;
    // The nearest edge on the line under the point.
    let mut best: Option<(f32, i32)> = None;
    let mut below = true;
    for i in 0..len.min(8192) {
        let Ok(r) = t.GetRectFromCharacterIndex(i, false) else {
            continue;
        };
        if y >= r.Y {
            below = false;
        }
        if y < r.Y || y >= r.Y + r.Height {
            continue;
        }
        let trailing = t.GetRectFromCharacterIndex(i, true).map_or(r.X, |t| t.X);
        for (edge, at) in [(r.X, i), (trailing, i + 1)] {
            let d = (edge - x).abs();
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, at));
            }
        }
    }
    let at = match best {
        Some((_, at)) => at,
        None if below => 0,
        None => len,
    };
    Some(prism_native::fields::utf16_to_chars(
        &text,
        at.max(0) as usize,
    ))
}

fn pressed(id: NodeId, e: &UIElement, a: &PointerRoutedEventArgs) -> Result<()> {
    let p = a.GetCurrentPoint(e)?;
    if p.PointerDeviceType()? != PointerDeviceType::Mouse
        || !p.Properties()?.IsLeftButtonPressed()?
    {
        return Ok(());
    }
    let Some(group) = with_state(|s| s.runtime.selection_group(id)).flatten() else {
        return Ok(());
    };
    let (x, y) = p.Position().map(|p| (p.X, p.Y))?;
    let Some(at) = offset_in(e, x, y) else {
        return Ok(());
    };
    DRAG.with(|d| {
        *d.borrow_mut() = Some(Drag {
            pointer: p.PointerId()?,
            field: id,
            group,
            at,
            taken: false,
        });
        Ok(())
    })
}

/// The field of `group` under the pointer of `a`, and the character there.
fn field_at(group: NodeId, a: &PointerRoutedEventArgs) -> Option<(NodeId, usize)> {
    let fields: Vec<(NodeId, UIElement)> = with_state(|s| {
        s.runtime
            .selection_group_fields(group)
            .into_iter()
            .filter_map(|f| {
                let v = s.renderer.widgets.views.get(&f)?;
                Some((f, v.content.element()?))
            })
            .collect()
    })?;
    fields.into_iter().find_map(|(f, e)| {
        let p = a.GetCurrentPoint(&e).ok()?.Position().ok()?;
        let fe: FrameworkElement = e.cast().ok()?;
        let (w, h) = (
            fe.ActualWidth().ok()? as f32,
            fe.ActualHeight().ok()? as f32,
        );
        let inside = p.X >= 0. && p.Y >= 0. && p.X < w && p.Y < h;
        inside.then(|| offset_in(&e, p.X, p.Y).map(|at| (f, at)))?
    })
}

fn moved(_: NodeId, _: &UIElement, a: &PointerRoutedEventArgs) -> Result<()> {
    let pointer = a.Pointer()?.PointerId()?;
    let Some(mut drag) = DRAG.with(|d| *d.borrow()).filter(|d| d.pointer == pointer) else {
        return Ok(());
    };
    let Some((over, to)) = field_at(drag.group, a) else {
        return Ok(());
    };
    if over == drag.field && !drag.taken {
        return Ok(());
    }
    drag.taken = true;
    DRAG.with(|d| *d.borrow_mut() = Some(drag));
    let e = UiEvent::SelectionAcross(drag.group, Some(((drag.field, drag.at), (over, to))));
    if crate::win::env("PRISM_TRACE_INPUT").is_some() {
        crate::win::log!("input: {e:?}");
    }
    dispatch(e);
    Ok(())
}

fn released(_: NodeId, _: &UIElement, a: &PointerRoutedEventArgs) -> Result<()> {
    let pointer = a.Pointer()?.PointerId()?;
    DRAG.with(|d| {
        let mut d = d.borrow_mut();
        if d.is_some_and(|d| d.pointer == pointer) {
            *d = None;
        }
    });
    Ok(())
}
