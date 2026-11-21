//! Tooltips (#216): WinUI's own (`ToolTipService`), with its delay and
//! look. `tooltip_when_truncated` shows a text's full content only while
//! XAML cuts it short (an ellipsis, a line clamp, a clipped line):
//! `TextBlock::IsTextTrimmed`, followed through `IsTextTrimmedChanged`
//! (macOS asks the label as the pointer rests).
//!
//! A text's `TextBlock` takes no pointer input (its node's canvas does):
//! it takes it while it has a tooltip, so the pointer resting on it opens
//! the tooltip; clicks still reach its node (routed events bubble).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use prism_core::{NodeId, Tooltip};
use windows::Foundation::TypedEventHandler;
use windows_core::{IInspectable, Result};

use crate::controls::Content;
use crate::view::View;
use crate::winui::boxed;
use crate::winui::xaml::Controls::{IsTextTrimmedChangedEventArgs, TextBlock, ToolTipService};
use crate::winui::xaml::UIElement;

/// A text with `tooltip_when_truncated`.
struct Watched {
    tb: TextBlock,
    /// Its full text.
    text: String,
    /// Its `IsTextTrimmedChanged` handler.
    token: i64,
}

thread_local! {
    static WATCHED: RefCell<HashMap<NodeId, Watched>> = RefCell::default();
    /// Nodes given a tooltip, to take it off.
    static TIPPED: RefCell<HashSet<NodeId>> = RefCell::default();
}

/// Node `id`'s tooltip on `target` (its button, control, text or canvas);
/// `text` is the node's own.
pub fn apply(
    v: &View,
    id: NodeId,
    target: &UIElement,
    tip: Option<&Tooltip>,
    text: &str,
) -> Result<()> {
    let label = match &v.content {
        Content::Text(tb) => Some(tb),
        _ => None,
    };
    match tip {
        Some(Tooltip::Text(t)) => {
            unwatch(id);
            ToolTipService::SetToolTip(target, &boxed(t)?)?;
            if let Some(tb) = label {
                tb.SetIsHitTestVisible(true)?;
            }
            TIPPED.with(|s| s.borrow_mut().insert(id));
        }
        // A text's (macOS, GTK and Android show it on texts only).
        Some(Tooltip::WhenTruncated) => {
            // A fixed tooltip before: it goes.
            let watched = WATCHED.with(|w| w.borrow().contains_key(&id));
            if !watched && TIPPED.with(|s| s.borrow_mut().remove(&id)) {
                ToolTipService::SetToolTip(target, None::<&IInspectable>)?;
            }
            if let Some(tb) = label {
                watch(tb, id, text)?;
                TIPPED.with(|s| s.borrow_mut().insert(id));
            }
        }
        None => {
            if TIPPED.with(|s| s.borrow_mut().remove(&id)) {
                unwatch(id);
                ToolTipService::SetToolTip(target, None::<&IInspectable>)?;
                if let Some(tb) = label {
                    tb.SetIsHitTestVisible(false)?;
                }
            }
        }
    }
    Ok(())
}

/// Shows `text` as `tb`'s tooltip while it is trimmed.
fn watch(tb: &TextBlock, id: NodeId, text: &str) -> Result<()> {
    let known = WATCHED.with(|w| {
        let mut w = w.borrow_mut();
        match w.get_mut(&id) {
            Some(it) => {
                it.text = text.to_string();
                true
            }
            None => false,
        }
    });
    if !known {
        let token = tb.IsTextTrimmedChanged(&TypedEventHandler::<
            TextBlock,
            IsTextTrimmedChangedEventArgs,
        >::new(move |_, _| {
            sync(id);
            Ok(())
        }))?;
        WATCHED.with(|w| {
            w.borrow_mut().insert(
                id,
                Watched {
                    tb: tb.clone(),
                    text: text.to_string(),
                    token,
                },
            )
        });
    }
    sync(id);
    Ok(())
}

/// The tooltip as the text is trimmed now.
fn sync(id: NodeId) {
    let Some((tb, text)) = WATCHED.with(|w| {
        w.borrow()
            .get(&id)
            .map(|it| (it.tb.clone(), it.text.clone()))
    }) else {
        return;
    };
    let r = (|| -> Result<()> {
        if tb.IsTextTrimmed()? && !text.is_empty() {
            ToolTipService::SetToolTip(&tb, &boxed(&text)?)?;
            tb.SetIsHitTestVisible(true)
        } else {
            ToolTipService::SetToolTip(&tb, None::<&IInspectable>)?;
            tb.SetIsHitTestVisible(false)
        }
    })();
    if let Err(e) = r {
        crate::win::log!("tooltip of node {id}: {e}");
    }
}

/// Node `id`'s text is `text` now.
pub fn retext(id: NodeId, text: &str) {
    let known = WATCHED.with(|w| match w.borrow_mut().get_mut(&id) {
        Some(it) => {
            it.text = text.to_string();
            true
        }
        None => false,
    });
    if known {
        sync(id);
    }
}

fn unwatch(id: NodeId) {
    if let Some(it) = WATCHED.with(|w| w.borrow_mut().remove(&id)) {
        let _ = it.tb.RemoveIsTextTrimmedChanged(it.token);
        let _ = ToolTipService::SetToolTip(&it.tb, None::<&IInspectable>);
    }
}

/// Node `id` is gone.
pub fn forget(id: NodeId) {
    unwatch(id);
    TIPPED.with(|s| s.borrow_mut().remove(&id));
}
