//! The app's menus (`cx.set_app_menu`, `cx.set_menu_bar`): a WinUI
//! `MenuBar` across the top of the window, as WinUI apps have it (File,
//! Edit, View…), with access keys (Alt+F opens File) and keyboard
//! accelerators. The app's view starts under it.
//!
//! The bar is on the window's solid color (`SolidBackgroundFillColorBase`,
//! like Prism's tab bars and stack headers under it), so it reads as one
//! band with them under the title bar's Mica, as Notepad's menu row does,
//! rather than a strip of Mica between two solid surfaces. It stays a row
//! under the title bar: the title bar keeps the system's caption buttons,
//! drag and snap layouts, and narrow windows keep room for every menu.
//!
//! Shortcuts are `KeyboardAccelerator`s of the bar itself, so they work
//! with every menu closed, and items show their text (`Ctrl+N`). A hint
//! (#247) is only shown: the key stays the focused view's. Platform
//! actions (#161) do what Windows does: Cut, Copy, Paste, Select All, Undo
//! and Redo in the focused field; Minimize, Zoom (maximize), Close and Quit
//! on the window.

use std::cell::{Cell, RefCell};

use prism_core::UiEvent;
use prism_core::menu::{MenuItemSpec, MenuSpec, SystemAction};
use windows::Foundation::TypedEventHandler;
use windows::System::{VirtualKey, VirtualKeyModifiers};
use windows::Win32::Foundation::{LPARAM, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, IsZoomed, PostMessageW, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, ShowWindow,
    WM_CLOSE,
};
use windows_core::{Interface, Ref, Result};

use crate::fluent::Palette;
use crate::imp::{State, dispatch};
use crate::win::log;
use crate::winui::xaml::Controls::{
    Canvas, MenuBar, MenuBarItem, MenuFlyoutItem, MenuFlyoutItemBase, MenuFlyoutSeparator,
    MenuFlyoutSubItem, PasswordBox, TextBox, ToggleMenuFlyoutItem,
};
use crate::winui::xaml::Input::{
    FocusManager, KeyboardAccelerator, KeyboardAcceleratorInvokedEventArgs,
};
use crate::winui::xaml::{RoutedEventHandler, UIElement};
use crate::winui::{brush, h};

/// WinUI's `MenuBarHeight`.
const HEIGHT: f32 = 40.;

thread_local! {
    static BAR: RefCell<Option<MenuBar>> = const { RefCell::new(None) };
    static SHOWN: Cell<f32> = const { Cell::new(0.) };
}

/// How much of the window's top the bar takes (DIPs).
pub fn height() -> f32 {
    SHOWN.get()
}

/// The window's width changed (DIPs).
pub fn resize(width: f32) {
    BAR.with(|b| {
        if let Some(bar) = b.borrow().as_ref() {
            let _ = bar.SetWidth(width as f64);
        }
    });
}

/// The theme changed: the bar's color again.
pub fn retheme(pal: &Palette) {
    BAR.with(|b| {
        if let Some(bar) = b.borrow().as_ref()
            && let Ok(c) = brush(pal.window)
        {
            let _ = bar.SetBackground(&c);
        }
    });
}

/// Replaces the menu bar (none when `menus` is empty), then lays the app
/// out again under it.
pub fn set(s: &mut State, menus: Vec<MenuSpec>) {
    let w = &mut s.renderer.widgets;
    if let Some(old) = BAR.with(|b| b.borrow_mut().take())
        && let Ok(children) = w.root.Children()
        && let Ok(old) = old.cast::<UIElement>()
    {
        let mut at = 0;
        if children.IndexOf(&old, &mut at).unwrap_or(false) {
            let _ = children.RemoveAt(at);
        }
    }
    let shown = if menus.is_empty() {
        0.
    } else {
        match build(&menus) {
            Ok(bar) => {
                let placed = (|| -> Result<()> {
                    bar.SetBackground(&brush(w.palette.window)?)?;
                    bar.SetHeight(HEIGHT as f64)?;
                    bar.SetWidth(w.size.0 as f64)?;
                    // Above the app, under its overlays.
                    w.root.Children()?.InsertAt(1, &bar)?;
                    Ok(())
                })();
                match placed {
                    Ok(()) => {
                        BAR.with(|b| *b.borrow_mut() = Some(bar));
                        HEIGHT
                    }
                    Err(e) => {
                        log!("menu bar: {e}");
                        0.
                    }
                }
            }
            Err(e) => {
                log!("menu bar: {e}");
                0.
            }
        }
    };
    SHOWN.set(shown);
    let _ = Canvas::SetTop(&w.app, shown as f64);
    let _ = Canvas::SetTop(&w.overlays, shown as f64);
    let _ = Canvas::SetTop(&w.top, shown as f64);
    // The app's viewport is what the bar leaves.
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(w.hwnd, &mut client);
    }
    let k = w.scale();
    w.resize(client.right as f32 / k, client.bottom as f32 / k);
    crate::imp::relayout(s);
}

fn build(menus: &[MenuSpec]) -> Result<MenuBar> {
    let bar = MenuBar::new()?;
    let items = bar.Items()?;
    let accelerators = bar.KeyboardAccelerators()?;
    let mut keys: Vec<char> = Vec::new();
    for m in menus {
        let item = MenuBarItem::new()?;
        item.SetTitle(&h(&m.title))?;
        // Alt + the title's first free letter opens it.
        if let Some(c) = m
            .title
            .chars()
            .filter(|c| c.is_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .find(|c| !keys.contains(c))
        {
            keys.push(c);
            let _ = item.SetAccessKey(&h(&c.to_string()));
        }
        let list = item.Items()?;
        for i in &m.items {
            if let Some(e) = entry(i, &accelerators)? {
                list.Append(&e)?;
            }
        }
        items.Append(&item)?;
    }
    Ok(bar)
}

fn accelerator(
    keys: &str,
    enabled: bool,
    on: impl Fn() + Send + 'static,
) -> Result<Option<(KeyboardAccelerator, String)>> {
    let Some(s) = crate::accel::parse(keys) else {
        log!("menu shortcut {keys:?}: no such key on Windows");
        return Ok(None);
    };
    let a = KeyboardAccelerator::new()?;
    a.SetKey(VirtualKey(s.key))?;
    a.SetModifiers(VirtualKeyModifiers(s.modifiers))?;
    a.SetIsEnabled(enabled)?;
    a.Invoked(&TypedEventHandler::new(
        move |_, args: Ref<KeyboardAcceleratorInvokedEventArgs>| {
            if let Some(args) = args.as_ref() {
                args.SetHandled(true)?;
            }
            on();
            Ok(())
        },
    ))?;
    Ok(Some((a, s.label)))
}

fn entry(
    item: &MenuItemSpec,
    accelerators: &windows_collections::IVector<KeyboardAccelerator>,
) -> Result<Option<MenuFlyoutItemBase>> {
    Ok(Some(match item {
        MenuItemSpec::Action {
            id,
            title,
            shortcut,
            shortcut_hint,
            checked,
            enabled,
            ..
        } => {
            let id = *id;
            let label = match shortcut {
                Some(keys) if *shortcut_hint => crate::accel::parse(keys).map(|s| s.label),
                Some(keys) => {
                    match accelerator(keys, *enabled, move || dispatch(UiEvent::MenuSelect(id)))? {
                        Some((a, label)) => {
                            accelerators.Append(&a)?;
                            Some(label)
                        }
                        None => None,
                    }
                }
                None => None,
            };
            let click = RoutedEventHandler::new(move |_, _| {
                dispatch(UiEvent::MenuSelect(id));
                Ok(())
            });
            let e: MenuFlyoutItem = match checked {
                Some(on) => {
                    let t = ToggleMenuFlyoutItem::new()?;
                    t.SetIsChecked(*on)?;
                    t.cast()?
                }
                None => MenuFlyoutItem::new()?,
            };
            e.SetText(&h(title))?;
            e.SetIsEnabled(*enabled)?;
            if let Some(l) = label {
                e.SetKeyboardAcceleratorTextOverride(&h(&l))?;
            }
            e.Click(&click)?;
            e.cast()?
        }
        MenuItemSpec::Separator => MenuFlyoutSeparator::new()?.cast()?,
        MenuItemSpec::Submenu { title, menu, .. } => {
            let sub = MenuFlyoutSubItem::new()?;
            sub.SetText(&h(title))?;
            let list = sub.Items()?;
            for i in &menu.items {
                if let Some(e) = entry(i, accelerators)? {
                    list.Append(&e)?;
                }
            }
            sub.cast()?
        }
        MenuItemSpec::System {
            action,
            title,
            shortcut,
        } => {
            let action = *action;
            if !handled(action) {
                return Ok(None);
            }
            let e = MenuFlyoutItem::new()?;
            // Windows' words where the app kept Prism's English default.
            let title = match action {
                SystemAction::Quit if title == action.default_title() => "Exit",
                SystemAction::Zoom if title == action.default_title() => "Maximize",
                _ => title.as_str(),
            };
            e.SetText(&h(title))?;
            // Windows' own keys (Alt+F4, not ⌘Q), shown, not bound: fields
            // and the window handle them.
            let keys = default_keys(action).or(shortcut.as_deref());
            if let Some(s) = keys.and_then(crate::accel::parse) {
                e.SetKeyboardAcceleratorTextOverride(&h(&s.label))?;
            }
            e.Click(&RoutedEventHandler::new(move |_, _| {
                system(action);
                Ok(())
            }))?;
            e.cast()?
        }
    }))
}

/// The platform actions Windows has.
fn handled(a: SystemAction) -> bool {
    use SystemAction::*;
    matches!(
        a,
        Undo | Redo
            | Cut
            | Copy
            | Paste
            | PasteAndMatchStyle
            | Delete
            | SelectAll
            | Minimize
            | Zoom
            | CloseWindow
            | Quit
    )
}

fn default_keys(a: SystemAction) -> Option<&'static str> {
    use SystemAction::*;
    Some(match a {
        Undo => "ctrl+z",
        Redo => "ctrl+y",
        Cut => "ctrl+x",
        Copy => "ctrl+c",
        Paste | PasteAndMatchStyle => "ctrl+v",
        SelectAll => "ctrl+a",
        Delete => "delete",
        CloseWindow | Quit => "alt+f4",
        _ => return None,
    })
}

/// Carries out a platform action.
fn system(a: SystemAction) {
    use SystemAction::*;
    let main = crate::imp::main_hwnd();
    match a {
        Minimize => unsafe {
            let _ = ShowWindow(main, SW_MINIMIZE);
        },
        Zoom => unsafe {
            let zoomed = IsZoomed(main).as_bool();
            let _ = ShowWindow(main, if zoomed { SW_RESTORE } else { SW_MAXIMIZE });
        },
        // Through WM_CLOSE: unsaved changes ask first.
        CloseWindow | Quit => unsafe {
            let _ = PostMessageW(Some(main), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        _ => edit(a),
    }
}

/// An edit action in the focused field.
fn edit(a: SystemAction) {
    use SystemAction::*;
    let root = crate::imp::with_state(|s| s.renderer.widgets.root.XamlRoot().ok()).flatten();
    let focused = match root {
        Some(r) => FocusManager::GetFocusedElementWithRoot(&r),
        None => FocusManager::GetFocusedElement(),
    };
    let Ok(focused) = focused else { return };
    if let Ok(t) = focused.cast::<TextBox>() {
        let _ = match a {
            Undo => t.Undo(),
            Redo => t.Redo(),
            Cut => t.CutSelectionToClipboard(),
            Copy => t.CopySelectionToClipboard(),
            Paste | PasteAndMatchStyle => t.PasteFromClipboard(),
            SelectAll => t.SelectAll(),
            Delete => t.SetSelectedText(&h("")),
            _ => Ok(()),
        };
    } else if let Ok(p) = focused.cast::<PasswordBox>() {
        let _ = match a {
            Paste | PasteAndMatchStyle => p.PasteFromClipboard(),
            SelectAll => p.SelectAll(),
            _ => Ok(()),
        };
    }
}
