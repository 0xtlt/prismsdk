//! `cx.show_alert(…)` as a WinUI `ContentDialog` (Windows 11's style:
//! `DefaultContentDialogStyle`): the title, the message, and the dialog's
//! three buttons: the app's first two actions as the primary (the accent
//! one when it is a regular action) and secondary buttons, its cancel
//! action as the close button (Escape presses it). Actions beyond those
//! are buttons under the message; an alert without actions has an OK
//! button. Only one `ContentDialog` shows at a time: alerts wait for the
//! one before to close.
//!
//! Closing the window with unsaved changes (`cx.set_unsaved_changes`,
//! #158) asks the same way, in the app's language (`prism.unsaved_*`).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use prism_core::UiEvent;
use prism_core::menu::{ActionRole, AlertSpec};
use windows_core::{Interface, Result};

use crate::widgets::Win;
use crate::win::{hwnd, log};
use crate::winui::xaml::Controls::{
    Button, ContentDialog, ContentDialogButton, ContentDialogResult, Orientation, StackPanel,
    TextBlock,
};
use crate::winui::xaml::{
    Application, ElementTheme, HorizontalAlignment, RoutedEventHandler, Style, TextWrapping,
    XamlRoot,
};
use crate::winui::{boxed, h};

/// Who answers an alert.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Asker {
    /// The app (`UiEvent::AlertAction`).
    App,
    /// Leaving window `.0` with unsaved changes: its first action
    /// discards them.
    Leave(isize),
}

thread_local! {
    /// Alerts waiting, and whether one shows.
    static QUEUE: RefCell<(VecDeque<(AlertSpec, Asker)>, bool)> = const { RefCell::new((VecDeque::new(), false)) };
    static ROOT: RefCell<Option<(XamlRoot, bool)>> = const { RefCell::new(None) };
    /// « Leave without saving? » is asked (waiting or shown).
    static LEAVING: Cell<bool> = const { Cell::new(false) };
}

/// Shows `spec` now, or after the alerts before it.
pub fn show(w: &Win, spec: AlertSpec) {
    let Ok(root) = w.root.XamlRoot() else {
        log!("alert {:?}: no window to show it in", spec.title);
        return;
    };
    ROOT.with(|r| *r.borrow_mut() = Some((root, w.dark())));
    QUEUE.with(|q| q.borrow_mut().0.push_back((spec, Asker::App)));
    next();
}

/// « Leave without saving? » before window `main` closes with unsaved
/// changes (#158), like Linux's and macOS's: Discard closes it
/// (`imp::discard_and_close`), Cancel or Escape keeps it. Without XAML to
/// show it in (no desktop session), a message box asks.
pub fn ask_leave(main: isize) {
    if LEAVING.get() {
        return;
    }
    match crate::imp::with_state(|s| leave_dialog(s, main)) {
        Some(Ok(())) => {}
        Some(Err([title, message])) => ask_plainly(main, &title, &message),
        // Busy: once the current work is done.
        None => crate::imp::defer(Box::new(move |s| {
            if let Err([title, message]) = leave_dialog(s, main) {
                ask_plainly(main, &title, &message);
            }
        })),
    }
}

/// Queues the question as a `ContentDialog`; its title and message back
/// when there is no XAML to show it in.
fn leave_dialog(s: &mut crate::imp::State, main: isize) -> std::result::Result<(), [String; 2]> {
    let [title, message, discard, cancel] =
        ["unsaved_title", "unsaved_message", "discard", "cancel"]
            .map(|k| s.runtime.app.chrome_text(k));
    let w = &s.renderer.widgets;
    let Ok(root) = w.root.XamlRoot() else {
        return Err([title, message]);
    };
    ROOT.with(|r| *r.borrow_mut() = Some((root, w.dark())));
    let spec = AlertSpec {
        id: 0,
        title,
        message: Some(message),
        style: Default::default(),
        actions: vec![
            (discard, ActionRole::Destructive),
            (cancel, ActionRole::Cancel),
        ],
    };
    LEAVING.set(true);
    QUEUE.with(|q| q.borrow_mut().0.push_back((spec, Asker::Leave(main))));
    next();
    Ok(())
}

/// The question as a message box (OK discards).
fn ask_plainly(main: isize, title: &str, message: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONWARNING, MB_OKCANCEL, MessageBoxW};
    let r = unsafe {
        MessageBoxW(
            Some(hwnd(main)),
            &h(message),
            &h(title),
            MB_OKCANCEL | MB_ICONWARNING,
        )
    };
    if r == IDOK {
        crate::imp::discard_and_close(main);
    }
}

fn next() {
    let item = QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if q.1 {
            return None;
        }
        let s = q.0.pop_front()?;
        q.1 = true;
        Some(s)
    });
    let Some((spec, asker)) = item else { return };
    let Some((root, dark)) = ROOT.with(|r| r.borrow().clone()) else {
        return;
    };
    if let Err(e) = present(&root, dark, &spec, asker) {
        log!("alert {:?}: {e}", spec.title);
        QUEUE.with(|q| q.borrow_mut().1 = false);
        if let Asker::Leave(main) = asker {
            LEAVING.set(false);
            ask_plainly(main, &spec.title, spec.message.as_deref().unwrap_or(""));
        }
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

fn present(root: &XamlRoot, dark: bool, spec: &AlertSpec, asker: Asker) -> Result<()> {
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
    d.SetDefaultButton(match asker {
        _ if b.accent => ContentDialogButton::Primary,
        // Enter keeps the changes.
        Asker::Leave(_) => ContentDialogButton::Close,
        Asker::App => ContentDialogButton::None,
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
            match (asker, chosen) {
                (Asker::App, Some(i)) => crate::imp::dispatch(UiEvent::AlertAction(id, i)),
                (Asker::Leave(main), chosen) => {
                    LEAVING.set(false);
                    if chosen == Some(0) {
                        crate::imp::discard_and_close(main);
                    }
                }
                (Asker::App, None) => {}
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
