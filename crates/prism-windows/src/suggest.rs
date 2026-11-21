//! A text input's suggestions (`TextInput::suggestions`, #18): as the user
//! types, those that start with the text (`inputs::matches`) show in a
//! list under the field, a transient `Flyout` that leaves the keyboard
//! focus in the field, like `AutoSuggestBox`'s. Down and Up highlight one
//! (a screen reader says it), Enter or a click takes it, Escape closes
//! the list. The field reports the new text like typing.

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::NodeId;
use windows::System::VirtualKey;
use windows_core::{Interface, Result};

use crate::inputs;
use crate::winui::xaml::Controls::Primitives::{
    FlyoutBase, FlyoutPlacementMode, FlyoutShowMode, FlyoutShowOptions,
};
use crate::winui::xaml::Controls::{Button, Flyout, StackPanel, TextBlock, TextBox};
use crate::winui::xaml::Input::KeyEventHandler;
use crate::winui::xaml::Media::Brush;
use crate::winui::xaml::{
    Application, FocusState, HorizontalAlignment, RoutedEventHandler, TextTrimming, UIElement,
};
use crate::winui::{boxed, h, transparent, uniform};

/// One field's suggestions and their list.
struct List {
    all: Vec<String>,
    field: Option<TextBox>,
    flyout: Option<(Flyout, StackPanel)>,
    /// The suggestions shown (indexes into `all`), their buttons.
    shown: Vec<usize>,
    buttons: Vec<Button>,
    highlight: Option<usize>,
}

thread_local! {
    static LISTS: RefCell<HashMap<NodeId, List>> = RefCell::default();
}

/// Node `id`'s suggestions (`set_config`); none: no list.
pub fn set(id: NodeId, all: Vec<String>) -> Result<()> {
    if all.is_empty() {
        hide(id)?;
        LISTS.with(|l| l.borrow_mut().remove(&id));
        return Ok(());
    }
    LISTS.with(|l| {
        let mut l = l.borrow_mut();
        let list = l.entry(id).or_insert_with(|| List {
            all: Vec::new(),
            field: None,
            flyout: None,
            shown: Vec::new(),
            buttons: Vec::new(),
            highlight: None,
        });
        list.all = all;
    });
    Ok(())
}

/// A single-line field's hooks: its text, keys and focus drive its list.
pub fn connect(id: NodeId, t: &TextBox) -> Result<()> {
    let field = t.clone();
    LISTS.with(|l| {
        if let Some(list) = l.borrow_mut().get_mut(&id) {
            list.field = Some(field);
        }
    });
    t.TextChanged(&crate::winui::xaml::Controls::TextChangedEventHandler::new(
        move |s, _| {
            if let Some(t) = s.as_ref().and_then(|s| s.cast::<TextBox>().ok()) {
                LISTS.with(|l| {
                    if let Some(list) = l.borrow_mut().get_mut(&id) {
                        list.field = Some(t.clone());
                    }
                });
                if let Err(e) = update(id, &t) {
                    crate::win::log!("suggestions of node {id}: {e}");
                }
            }
            Ok(())
        },
    ))?;
    t.PreviewKeyDown(&KeyEventHandler::new(move |s, a| {
        if let (Some(a), Some(t)) = (
            a.as_ref(),
            s.as_ref().and_then(|s| s.cast::<TextBox>().ok()),
        ) && key(id, &t, a.Key()?)?
        {
            a.SetHandled(true)?;
        }
        Ok(())
    }))?;
    t.LostFocus(&RoutedEventHandler::new(move |_, _| hide(id)))?;
    Ok(())
}

/// The list's flyout and panel, made the first time.
fn ui(id: NodeId) -> Result<(Flyout, StackPanel)> {
    if let Some(ui) = LISTS.with(|l| l.borrow().get(&id).and_then(|x| x.flyout.clone())) {
        return Ok(ui);
    }
    let flyout = Flyout::new()?;
    let panel = StackPanel::new()?;
    flyout.SetContent(&panel)?;
    let base: FlyoutBase = flyout.cast()?;
    // The field keeps the keyboard focus.
    base.SetAllowFocusOnInteraction(false)?;
    let ui = (flyout, panel);
    LISTS.with(|l| {
        if let Some(x) = l.borrow_mut().get_mut(&id) {
            x.flyout = Some(ui.clone());
        }
    });
    Ok(ui)
}

/// The list for the field's text: shown while it matches and the field
/// has the focus.
fn update(id: NodeId, t: &TextBox) -> Result<()> {
    let Some(all) = LISTS.with(|l| l.borrow().get(&id).map(|x| x.all.clone())) else {
        return Ok(());
    };
    let focused = t.FocusState()? != FocusState::Unfocused;
    let shown = if focused {
        inputs::matches(&all, &t.Text()?.to_string())
    } else {
        Vec::new()
    };
    if shown.is_empty() {
        return hide(id);
    }
    let (flyout, panel) = ui(id)?;
    let kids = panel.Children()?;
    kids.Clear()?;
    let mut buttons = Vec::new();
    for &i in &shown {
        let b = item(id, &all[i], i)?;
        kids.Append(&b)?;
        buttons.push(b);
    }
    panel.SetMinWidth((t.ActualWidth()? - 24.).max(0.))?;
    LISTS.with(|l| {
        if let Some(x) = l.borrow_mut().get_mut(&id) {
            x.shown = shown;
            x.buttons = buttons;
            x.highlight = None;
        }
    });
    let base: FlyoutBase = flyout.cast()?;
    if !base.IsOpen()? {
        let o = FlyoutShowOptions::new()?;
        o.SetShowMode(FlyoutShowMode::Transient)?;
        o.SetPlacement(FlyoutPlacementMode::BottomEdgeAlignedLeft)?;
        base.ShowAtWithOptions(t, &o)?;
    }
    Ok(())
}

/// One suggestion: clicking it takes it, the field keeping the focus.
fn item(id: NodeId, text: &str, index: usize) -> Result<Button> {
    let b = Button::new()?;
    let label = TextBlock::new()?;
    label.SetText(&h(text))?;
    label.SetTextTrimming(TextTrimming::CharacterEllipsis)?;
    b.SetContent(&label)?;
    b.SetHorizontalAlignment(HorizontalAlignment::Stretch)?;
    b.SetHorizontalContentAlignment(HorizontalAlignment::Left)?;
    b.SetBackground(&transparent()?)?;
    b.SetBorderThickness(uniform(0.))?;
    b.SetIsTabStop(false)?;
    b.SetAllowFocusOnInteraction(false)?;
    b.Click(&RoutedEventHandler::new(move |_, _| take(id, index)))?;
    Ok(b)
}

/// The field takes suggestion `index`; the list closes.
fn take(id: NodeId, index: usize) -> Result<()> {
    let Some((field, text)) = LISTS.with(|l| {
        let l = l.borrow();
        let x = l.get(&id)?;
        Some((x.field.clone()?, x.all.get(index)?.clone()))
    }) else {
        return Ok(());
    };
    hide(id)?;
    field.SetText(&h(&text))?;
    field.SetSelectionStart(text.encode_utf16().count() as i32)?;
    Ok(())
}

/// Closes node `id`'s list.
pub fn hide(id: NodeId) -> Result<()> {
    let flyout = LISTS.with(|l| {
        let mut l = l.borrow_mut();
        let x = l.get_mut(&id)?;
        x.highlight = None;
        x.flyout.as_ref().map(|(f, _)| f.clone())
    });
    if let Some(f) = flyout {
        let base: FlyoutBase = f.cast()?;
        if base.IsOpen()? {
            base.Hide()?;
        }
    }
    Ok(())
}

/// A key in the field while its list is open: whether the list took it.
fn key(id: NodeId, t: &TextBox, key: VirtualKey) -> Result<bool> {
    let Some((open, shown, highlight, buttons)) = LISTS.with(|l| {
        let l = l.borrow();
        let x = l.get(&id)?;
        let open = x
            .flyout
            .as_ref()
            .and_then(|(f, _)| f.cast::<FlyoutBase>().ok().and_then(|b| b.IsOpen().ok()));
        Some((
            open == Some(true),
            x.shown.clone(),
            x.highlight,
            x.buttons.clone(),
        ))
    }) else {
        return Ok(false);
    };
    if !open || shown.is_empty() {
        return Ok(false);
    }
    match key {
        VirtualKey::Down | VirtualKey::Up => {
            let next = inputs::step(highlight, shown.len(), key == VirtualKey::Down);
            LISTS.with(|l| {
                if let Some(x) = l.borrow_mut().get_mut(&id) {
                    x.highlight = next;
                }
            });
            let on = highlighted()?;
            let off: Brush = transparent()?.cast()?;
            for (i, b) in buttons.iter().enumerate() {
                b.SetBackground(if Some(i) == next { &on } else { &off })?;
            }
            if let Some(text) = next
                .and_then(|i| shown.get(i))
                .and_then(|&i| LISTS.with(|l| l.borrow().get(&id)?.all.get(i).cloned()))
            {
                let e: UIElement = t.cast()?;
                let _ = crate::peers::announce(&e, &text, false);
            }
            Ok(true)
        }
        VirtualKey::Enter => match highlight.and_then(|i| shown.get(i)) {
            Some(&i) => take(id, i).map(|_| true),
            None => Ok(false),
        },
        VirtualKey::Escape => hide(id).map(|_| true),
        _ => Ok(false),
    }
}

/// A highlighted suggestion's background: the theme's subtle fill, as a
/// list item under the pointer.
fn highlighted() -> Result<Brush> {
    let res = Application::Current()?.Resources()?;
    match res.Lookup(&boxed("SubtleFillColorSecondaryBrush")?) {
        Ok(b) => b.cast(),
        Err(_) => Ok(crate::winui::brush(prism_core::Color {
            r: 0.5,
            g: 0.5,
            b: 0.5,
            a: 0.15,
            dark: None,
        })?
        .cast()?),
    }
}
