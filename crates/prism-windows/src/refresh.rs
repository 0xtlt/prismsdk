//! Pull to refresh (`Scroll::on_refresh`): a refreshable scroll view's
//! `ScrollViewer` goes into a `RefreshContainer`, whose pull (touch) and
//! spinner are WinUI's; F5 and Ctrl+R refresh too, like Linux's. A
//! refresh reports a click on the scroll view (how `on_refresh` hears
//! it). The spinner turns while the app says it is refreshing
//! (`Scroll::refreshing`, the node's value), and stops after the click if
//! the app never says so. The app turning `refreshing` on by itself shows
//! the spinner without another click.

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::{NodeId, UiEvent};
use windows::Foundation::{Deferral, TypedEventHandler};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_F5, VK_MENU};
use windows_core::{IInspectable, Interface, Result};

use crate::winui::xaml::Controls::{
    Canvas, RefreshContainer, RefreshRequestedEventArgs, ScrollViewer,
};
use crate::winui::xaml::UIElement;

struct Refreshable {
    container: RefreshContainer,
    /// The refresh going on: done when completed.
    deferral: Option<Deferral>,
    /// The next refresh is the app's own (`refreshing` on): no click.
    quiet: bool,
}

thread_local! {
    static VIEWS: RefCell<HashMap<NodeId, Refreshable>> = RefCell::new(HashMap::new());
}

/// Scroll view `id` (`sv`, in `outer`) refreshes (`on_refresh`), or not.
pub fn set(id: NodeId, outer: &Canvas, sv: &ScrollViewer, on: bool) -> Result<()> {
    let known = VIEWS.with(|v| v.borrow().contains_key(&id));
    if on == known {
        return Ok(());
    }
    let kids = outer.Children()?;
    if on {
        let mut at = 0u32;
        let sv_ui: UIElement = sv.cast()?;
        if !kids.IndexOf(&sv_ui, &mut at)? {
            return Ok(());
        }
        let container = RefreshContainer::new()?;
        kids.RemoveAt(at)?;
        container.SetContent(sv)?;
        kids.InsertAt(at, &container)?;
        container.RefreshRequested(&TypedEventHandler::<
            RefreshContainer,
            RefreshRequestedEventArgs,
        >::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                requested(id, args.GetDeferral()?);
            }
            Ok(())
        }))?;
        VIEWS.with(|v| {
            v.borrow_mut().insert(
                id,
                Refreshable {
                    container,
                    deferral: None,
                    quiet: false,
                },
            )
        });
    } else if let Some(r) = VIEWS.with(|v| v.borrow_mut().remove(&id)) {
        if let Some(d) = r.deferral {
            let _ = d.Complete();
        }
        let mut at = 0u32;
        let c: UIElement = r.container.cast()?;
        if kids.IndexOf(&c, &mut at)? {
            kids.RemoveAt(at)?;
            r.container.SetContent(None::<&IInspectable>)?;
            kids.InsertAt(at, sv)?;
        }
    }
    Ok(())
}

/// A pull (or F5): the click, the spinner until the app is done.
fn requested(id: NodeId, deferral: Deferral) {
    let quiet = VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        let Some(r) = v.get_mut(&id) else {
            return true;
        };
        // A refresh still going on: done, the new one takes over.
        if let Some(d) = r.deferral.replace(deferral) {
            let _ = d.Complete();
        }
        std::mem::take(&mut r.quiet)
    });
    if quiet {
        return;
    }
    crate::imp::dispatch(UiEvent::Click(id));
    // Once the click is handled: done unless the app is refreshing.
    crate::imp::defer(Box::new(move |_| {
        crate::imp::defer(Box::new(move |s| {
            let on = s
                .renderer
                .widgets
                .scene
                .get(id)
                .is_some_and(|n| n.value >= 0.5);
            if !on {
                done(id);
            }
        }))
    }));
}

/// The app's `refreshing` for scroll view `id`.
pub fn refreshing(id: NodeId, on: bool) {
    if !on {
        done(id);
        return;
    }
    // On by itself: the spinner, without a click.
    let container = VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        let r = v.get_mut(&id).filter(|r| r.deferral.is_none())?;
        r.quiet = true;
        Some(r.container.clone())
    });
    if let Some(c) = container
        && c.IsLoaded().unwrap_or(false)
        && c.RequestRefresh().is_err()
    {
        VIEWS.with(|v| {
            if let Some(r) = v.borrow_mut().get_mut(&id) {
                r.quiet = false;
            }
        });
    }
}

/// The spinner stops.
fn done(id: NodeId) {
    if let Some(d) = VIEWS.with(|v| v.borrow_mut().get_mut(&id).and_then(|r| r.deferral.take())) {
        let _ = d.Complete();
    }
}

/// F5 or Ctrl+R (without Alt) refreshes the refreshable scroll views:
/// whether the key was theirs.
pub fn key(vk: u16) -> bool {
    let down = |k: u16| unsafe { GetKeyState(k as i32) } < 0;
    let refresh = vk == VK_F5.0 || (vk == u16::from(b'R') && down(VK_CONTROL.0));
    if !refresh || down(VK_MENU.0) {
        return false;
    }
    let shown: Vec<(NodeId, RefreshContainer)> = VIEWS.with(|v| {
        v.borrow()
            .iter()
            .map(|(id, r)| (*id, r.container.clone()))
            .collect()
    });
    if shown.is_empty() {
        return false;
    }
    for (id, c) in shown {
        // On screen: through the container (its spinner); else the click.
        let loaded = c.IsLoaded().unwrap_or(false);
        if !loaded || c.RequestRefresh().is_err() {
            crate::imp::dispatch(UiEvent::Click(id));
        }
    }
    true
}

/// Node `id` is gone.
pub fn forget(id: NodeId) {
    if let Some(r) = VIEWS.with(|v| v.borrow_mut().remove(&id))
        && let Some(d) = r.deferral
    {
        let _ = d.Complete();
    }
}
