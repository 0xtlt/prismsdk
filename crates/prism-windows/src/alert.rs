//! `cx.show_alert(…)` as a WinUI `ContentDialog` (Windows 11's style:
//! `DefaultContentDialogStyle`): the title, the message, and the dialog's
//! three buttons: the app's first two actions as the primary (the accent
//! one when it is a regular action) and secondary buttons, its cancel
//! action as the close button (Escape presses it). Actions beyond those
//! are buttons under the message; an alert without actions has an OK
//! button. Only one `ContentDialog` shows at a time: alerts wait for the
//! one before to close.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use prism_core::UiEvent;
use prism_core::menu::{ActionRole, AlertSpec};
use windows_core::{Interface, Result};

use crate::widgets::Win;
use crate::winui::xaml::Controls::{
    Button, ContentDialog, ContentDialogButton, ContentDialogResult, Orientation, StackPanel,
    TextBlock,
};
use crate::winui::xaml::{
    Application, ElementTheme, HorizontalAlignment, RoutedEventHandler, Style, TextWrapping,
    XamlRoot,
};
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
    /// Actions beyond the three buttons, listed under the message.
    pub more: Vec<usize>,
}

/// The app's actions on `ContentDialog`'s three buttons: the cancel one
/// closes, the first two others are primary and secondary, later ones go
/// under the message.
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
            _ => b.more.push(i),
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
    let b = buttons(&spec.actions);
    let content = StackPanel::new()?;
    content.SetOrientation(Orientation::Vertical)?;
    content.SetSpacing(12.)?;
    if let Some(m) = spec.message.as_deref().filter(|m| !m.is_empty()) {
        let t = TextBlock::new()?;
        t.SetText(&h(m))?;
        t.SetTextWrapping(TextWrapping::Wrap)?;
        content.Children()?.Append(&t)?;
    }
    // A listed action reports itself, then closes the dialog.
    let listed = Arc::new(AtomicBool::new(false));
    for &i in &b.more {
        let button = Button::new()?;
        button.SetContent(&boxed(&spec.actions[i].0)?)?;
        button.SetHorizontalAlignment(HorizontalAlignment::Stretch)?;
        let (d2, id, listed) = (d.clone(), spec.id, listed.clone());
        button.Click(&RoutedEventHandler::new(move |_, _| {
            listed.store(true, Ordering::SeqCst);
            crate::imp::dispatch(UiEvent::AlertAction(id, i));
            d2.Hide()
        }))?;
        content.Children()?.Append(&button)?;
    }
    d.SetContent(&content)?;
    let title = |i: Option<usize>| i.map_or(String::new(), |i| spec.actions[i].0.clone());
    d.SetPrimaryButtonText(&h(&title(b.primary)))?;
    d.SetSecondaryButtonText(&h(&title(b.secondary)))?;
    // No action at all: an OK button that reports nothing.
    let close = if spec.actions.is_empty() {
        "OK".to_string()
    } else {
        title(b.close)
    };
    d.SetCloseButtonText(&h(&close))?;
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
                // A listed action, already reported.
                _ if listed.load(Ordering::SeqCst) => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_follow_roles() {
        let a = |t: &str, r| (t.to_string(), r);
        let b = buttons(&[
            a("Delete", ActionRole::Destructive),
            a("Cancel", ActionRole::Cancel),
            a("Keep", ActionRole::Normal),
            a("Later", ActionRole::Normal),
        ]);
        assert_eq!(
            b,
            Buttons {
                primary: Some(0),
                secondary: Some(2),
                close: Some(1),
                accent: false,
                more: vec![3],
            }
        );
    }
}
