//! `cx.show_alert(…)` as a WinUI `ContentDialog` (Windows 11's style:
//! `DefaultContentDialogStyle`): the title, the message, and up to three
//! buttons: the app's first two actions as the primary (the accent one
//! when it is a regular action) and secondary buttons, its cancel action
//! as the close button (Escape presses it). Only one `ContentDialog` shows
//! at a time: alerts wait for the one before to close.

use std::cell::RefCell;
use std::collections::VecDeque;

use prism_core::UiEvent;
use prism_core::menu::{ActionRole, AlertSpec};
use windows_core::{Interface, Result};

use crate::widgets::Win;
use crate::winui::xaml::Controls::{
    ContentDialog, ContentDialogButton, ContentDialogResult, TextBlock,
};
use crate::winui::xaml::{Application, ElementTheme, Style, TextWrapping, XamlRoot};
use crate::winui::{boxed, h};

thread_local! {
    /// Alerts waiting, and whether one shows.
    static QUEUE: RefCell<(VecDeque<AlertSpec>, bool)> = const { RefCell::new((VecDeque::new(), false)) };
    static ROOT: RefCell<Option<(XamlRoot, bool)>> = const { RefCell::new(None) };
}

/// Shows `spec` now, or after the alerts before it.
pub fn show(w: &Win, spec: AlertSpec) {
    let Ok(root) = w.root.XamlRoot() else {
        crate::win::log!("alert {:?}: no window to show it in", spec.title);
        return;
    };
    ROOT.with(|r| *r.borrow_mut() = Some((root, w.dark())));
    QUEUE.with(|q| q.borrow_mut().0.push_back(spec));
    next();
}

fn next() {
    let spec = QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if q.1 {
            return None;
        }
        let s = q.0.pop_front()?;
        q.1 = true;
        Some(s)
    });
    let Some(spec) = spec else { return };
    let Some((root, dark)) = ROOT.with(|r| r.borrow().clone()) else {
        return;
    };
    if let Err(e) = present(&root, dark, &spec) {
        crate::win::log!("alert {:?}: {e}", spec.title);
        QUEUE.with(|q| q.borrow_mut().1 = false);
        next();
    }
}

/// Which action each button stands for: primary, secondary, close.
#[derive(Debug, Default, PartialEq)]
pub struct Buttons {
    pub primary: Option<usize>,
    pub secondary: Option<usize>,
    pub close: Option<usize>,
    /// The primary button is the accent one (a regular action).
    pub accent: bool,
}

/// The app's actions on `ContentDialog`'s three buttons: the cancel one
/// closes, the first two others are primary and secondary (later ones do
/// not fit and are left out).
pub fn buttons(actions: &[(String, ActionRole)]) -> Buttons {
    let mut b = Buttons::default();
    for (i, (_, role)) in actions.iter().enumerate() {
        match role {
            ActionRole::Cancel if b.close.is_none() => b.close = Some(i),
            ActionRole::Cancel => {}
            _ if b.primary.is_none() => {
                b.primary = Some(i);
                b.accent = *role == ActionRole::Normal;
            }
            _ if b.secondary.is_none() => b.secondary = Some(i),
            _ => crate::win::log!("alert: a ContentDialog has three buttons; action {i} left out"),
        }
    }
    b
}

fn present(root: &XamlRoot, dark: bool, spec: &AlertSpec) -> Result<()> {
    let d = ContentDialog::new()?;
    d.SetXamlRoot(root)?;
    // Created in code, a ContentDialog takes the old style unless given
    // Windows 11's.
    if let Ok(s) = Application::Current()
        .and_then(|a| a.Resources())
        .and_then(|r| r.Lookup(&boxed("DefaultContentDialogStyle")?))
        .and_then(|s| s.cast::<Style>())
    {
        d.SetStyle(&s)?;
    }
    d.SetRequestedTheme(if dark {
        ElementTheme::Dark
    } else {
        ElementTheme::Light
    })?;
    d.SetTitle(&boxed(&spec.title)?)?;
    if let Some(m) = &spec.message {
        let t = TextBlock::new()?;
        t.SetText(&h(m))?;
        t.SetTextWrapping(TextWrapping::Wrap)?;
        d.SetContent(&t)?;
    }
    let b = buttons(&spec.actions);
    let title = |i: Option<usize>| i.map_or(String::new(), |i| spec.actions[i].0.clone());
    d.SetPrimaryButtonText(&h(&title(b.primary)))?;
    d.SetSecondaryButtonText(&h(&title(b.secondary)))?;
    d.SetCloseButtonText(&h(&title(b.close)))?;
    d.SetDefaultButton(if b.accent {
        ContentDialogButton::Primary
    } else {
        ContentDialogButton::None
    })?;
    let id = spec.id;
    let op = d.ShowAsync()?;
    op.SetCompleted(&windows_future::AsyncOperationCompletedHandler::new(
        move |op, _| {
            let result = op
                .as_ref()
                .and_then(|o| o.GetResults().ok())
                .unwrap_or(ContentDialogResult::None);
            // Escape and the close button: the cancel action, if any.
            let chosen = match result {
                ContentDialogResult::Primary => b.primary,
                ContentDialogResult::Secondary => b.secondary,
                _ => b.close,
            };
            if let Some(i) = chosen {
                crate::imp::dispatch(UiEvent::AlertAction(id, i));
            }
            QUEUE.with(|q| q.borrow_mut().1 = false);
            next();
            Ok(())
        },
    ))?;
    Ok(())
}
