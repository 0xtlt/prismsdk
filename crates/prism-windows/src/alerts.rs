//! Alerts and action sheets (`cx.show_alert`): a WinUI `ContentDialog` over
//! the window, its title, message and buttons. The first action is the
//! accent (default) button unless it is destructive, a Cancel action is
//! the close button (Escape), and actions beyond the dialog's three
//! buttons are listed under the message. The chosen one is reported as
//! `UiEvent::AlertAction`.

use prism_core::UiEvent;
use prism_core::menu::{ActionRole, AlertSpec};
use windows::Foundation::PropertyValue;
use windows_core::Result;

use crate::imp::State;
use crate::win::log;
use crate::winui::h;
use crate::winui::xaml::Controls::{
    Button, ContentDialog, ContentDialogButton, ContentDialogResult, Orientation, StackPanel,
    TextBlock,
};
use crate::winui::xaml::{ElementTheme, RoutedEventHandler, TextWrapping};

/// Which action each of the dialog's buttons reports.
#[derive(Default, Debug, PartialEq)]
struct Buttons {
    primary: Option<usize>,
    secondary: Option<usize>,
    close: Option<usize>,
    more: Vec<usize>,
}

fn assign(actions: &[(String, ActionRole)]) -> Buttons {
    let mut b = Buttons::default();
    for (i, (_, role)) in actions.iter().enumerate() {
        if *role == ActionRole::Cancel && b.close.is_none() {
            b.close = Some(i);
        } else if b.primary.is_none() {
            b.primary = Some(i);
        } else if b.secondary.is_none() {
            b.secondary = Some(i);
        } else {
            b.more.push(i);
        }
    }
    b
}

fn report(id: u64, index: usize) {
    crate::shell::on_ui(move || crate::imp::dispatch(UiEvent::AlertAction(id, index)));
}

pub fn show(s: &mut State, spec: AlertSpec) {
    if let Err(e) = try_show(s, &spec) {
        log!("alert {:?}: {e}", spec.title);
    }
}

fn try_show(s: &mut State, spec: &AlertSpec) -> Result<()> {
    let w = &s.renderer.widgets;
    let dialog = ContentDialog::new()?;
    dialog.SetXamlRoot(&w.root.XamlRoot()?)?;
    dialog.SetRequestedTheme(if w.dark() {
        ElementTheme::Dark
    } else {
        ElementTheme::Light
    })?;
    dialog.SetTitle(&PropertyValue::CreateString(&h(&spec.title))?)?;
    let buttons = assign(&spec.actions);
    let label = |i: Option<usize>| i.map(|i| h(&spec.actions[i].0));
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
    let reported = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    for &i in &buttons.more {
        let b = Button::new()?;
        b.SetContent(&PropertyValue::CreateString(&h(&spec.actions[i].0))?)?;
        b.SetHorizontalAlignment(crate::winui::xaml::HorizontalAlignment::Stretch)?;
        let (d, id, done) = (dialog.clone(), spec.id, reported.clone());
        b.Click(&RoutedEventHandler::new(move |_, _| {
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            report(id, i);
            d.Hide()
        }))?;
        content.Children()?.Append(&b)?;
    }
    dialog.SetContent(&content)?;
    if let Some(t) = label(buttons.primary) {
        dialog.SetPrimaryButtonText(&t)?;
    }
    if let Some(t) = label(buttons.secondary) {
        dialog.SetSecondaryButtonText(&t)?;
    }
    match label(buttons.close) {
        Some(t) => dialog.SetCloseButtonText(&t)?,
        // No button at all: an OK that reports nothing.
        None if spec.actions.is_empty() => dialog.SetCloseButtonText(&h("OK"))?,
        None => {}
    }
    let destructive = buttons
        .primary
        .is_some_and(|i| spec.actions[i].1 == ActionRole::Destructive);
    dialog.SetDefaultButton(if buttons.primary.is_some() && !destructive {
        ContentDialogButton::Primary
    } else {
        ContentDialogButton::None
    })?;
    let id = spec.id;
    dialog.ShowAsync()?.when(move |r| {
        let chosen = match r {
            Ok(ContentDialogResult::Primary) => buttons.primary,
            Ok(ContentDialogResult::Secondary) => buttons.secondary,
            // A listed action, already reported.
            Ok(_) if reported.load(std::sync::atomic::Ordering::SeqCst) => None,
            // Close, Escape.
            Ok(_) => buttons.close,
            Err(e) => {
                log!("alert: {e}");
                None
            }
        };
        if let Some(i) = chosen {
            report(id, i);
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_follow_roles() {
        let a = |t: &str, r| (t.to_string(), r);
        let b = assign(&[
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
                more: vec![3],
            }
        );
    }
}
