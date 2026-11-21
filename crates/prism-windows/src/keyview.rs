//! Views listening to keys (`on_key_down`) take the keyboard focus, as on
//! macOS and Linux. A clickable view's button and a control take it by
//! themselves; any other key view's canvas becomes a focus stop: reached
//! with Tab (the system's focus visual shows), focused when pressed (no
//! visual, as WinUI's controls) and by `cx.focus`. Its focus is reported
//! like a control's (`imp::focus_changed`), so its keys go to its
//! listeners (`keyboard.rs`); UI Automation reads it as a focusable
//! element (`access::key_view`, `peers.rs`).

use std::cell::RefCell;

use prism_core::{IdSet, NodeId};
use windows_core::{IInspectable, IUnknown, Interface, Result};

use crate::winui::xaml::Controls::Canvas;
use crate::winui::xaml::Input::{PointerEventHandler, PointerRoutedEventArgs};
use crate::winui::xaml::{FocusState, RoutedEventArgs, RoutedEventHandler, UIElement};

thread_local! {
    /// The views whose canvas is a focus stop.
    static VIEWS: RefCell<IdSet<NodeId>> = RefCell::new(IdSet::default());
    /// The canvases with their handlers (they read [`VIEWS`]).
    static WIRED: RefCell<IdSet<NodeId>> = RefCell::new(IdSet::default());
    /// The last press a key view took (pointer, timestamp): the key views
    /// around it leave it.
    static PRESS: RefCell<Option<(u32, u64)>> = const { RefCell::new(None) };
}

/// Whether node `id`'s canvas is a focus stop.
pub fn is(id: NodeId) -> bool {
    VIEWS.with(|v| {
        let v = v.borrow();
        !v.is_empty() && v.contains(&id)
    })
}

/// Makes node `id`'s canvas a focus stop (`on`) or not; returns whether
/// that changed.
pub fn set(outer: &Canvas, id: NodeId, on: bool) -> Result<bool> {
    let changed = VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        if on { v.insert(id) } else { v.remove(&id) }
    });
    if !changed {
        return Ok(false);
    }
    outer.SetIsTabStop(on)?;
    outer.SetUseSystemFocusVisuals(on)?;
    if on && WIRED.with(|w| w.borrow_mut().insert(id)) {
        wire(outer, id)?;
    }
    Ok(true)
}

/// Node `id` is gone.
pub fn forget(id: NodeId) {
    VIEWS.with(|v| v.borrow_mut().remove(&id));
    WIRED.with(|w| w.borrow_mut().remove(&id));
}

/// Focus events are routed: those of an element inside bubble up. Only
/// the canvas's own count.
fn own(sender: Option<&IInspectable>, a: Option<&RoutedEventArgs>) -> bool {
    let (Some(s), Some(a)) = (sender, a) else {
        return false;
    };
    let (Ok(s), Ok(src)) = (s.cast::<IUnknown>(), a.OriginalSource()) else {
        return false;
    };
    src.cast::<IUnknown>()
        .is_ok_and(|src| src.as_raw() == s.as_raw())
}

fn press_key(a: &PointerRoutedEventArgs) -> Option<(u32, u64)> {
    let p = a.GetCurrentPoint(None::<&UIElement>).ok()?;
    Some((p.PointerId().ok()?, p.Timestamp().ok()?))
}

fn wire(outer: &Canvas, id: NodeId) -> Result<()> {
    outer.GotFocus(&RoutedEventHandler::new(move |s, a| {
        if is(id) && own(s.as_ref(), a.as_ref()) {
            crate::imp::focus_changed(id, true);
        }
        Ok(())
    }))?;
    outer.LostFocus(&RoutedEventHandler::new(move |s, a| {
        if own(s.as_ref(), a.as_ref()) {
            crate::imp::focus_changed(id, false);
        }
        Ok(())
    }))?;
    // A press anywhere in it focuses it, unless something inside took the
    // press (a button, a field: they mark it handled) or a key view
    // inside took the focus first.
    outer.PointerPressed(&PointerEventHandler::new(move |s, a| {
        let (Some(s), Some(a)) = (s.as_ref(), a.as_ref()) else {
            return Ok(());
        };
        if !is(id) || a.Handled()? {
            return Ok(());
        }
        let key = press_key(a);
        if key.is_some() && PRESS.with(|p| p.replace(key)) == key {
            return Ok(());
        }
        s.cast::<UIElement>()?.Focus(FocusState::Pointer)?;
        Ok(())
    }))?;
    Ok(())
}
