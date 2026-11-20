//! XAML input on Prism's own views (controls report theirs in
//! `controls.rs`): clicks on clickable views, the pointer over and
//! pressing a view (interaction states), the keyboard focus, scrolling,
//! context and pop-up menus.

use prism_core::menu::{MenuItemSpec, MenuSpec};
use prism_core::{Interaction, NodeId, UiEvent};
use windows_core::{Interface, Result};

use crate::imp::{defer, dispatch};
use crate::winui::xaml::Controls::Primitives::{ButtonBase, FlyoutBase, FlyoutShowOptions};
use crate::winui::xaml::Controls::{
    Button, Canvas, MenuFlyout, MenuFlyoutItem, MenuFlyoutSeparator, MenuFlyoutSubItem,
    ScrollViewer, ScrollViewerViewChangedEventArgs, ToggleMenuFlyoutItem,
};
use crate::winui::xaml::Input::{PointerEventHandler, TappedEventHandler};
use crate::winui::xaml::{DependencyPropertyChangedCallback, RoutedEventHandler, UIElement};
use crate::winui::{h, xaml};

/// A clickable view: a tap (or a click) reports `UiEvent::Click`.
pub fn clickable(outer: &Canvas, id: NodeId) -> Result<()> {
    outer.Tapped(&TappedEventHandler::new(move |_, a| {
        if let Some(a) = a.as_ref() {
            if a.Handled()? {
                return Ok(());
            }
            a.SetHandled(true)?;
        }
        dispatch(UiEvent::Click(id));
        Ok(())
    }))?;
    Ok(())
}

/// The backdrop of a sheet or a dialog: a tap there closes it
/// (`UiEvent::Back(overlay, 0)`) unless it is fixed.
pub fn scrim(overlay: NodeId) -> Result<xaml::Controls::Border> {
    let b = xaml::Controls::Border::new()?;
    b.SetVisibility(xaml::Visibility::Collapsed)?;
    b.Tapped(&TappedEventHandler::new(move |_, _| {
        defer(Box::new(move |s| {
            let fixed = s
                .renderer
                .widgets
                .scene
                .get(overlay)
                .is_some_and(|n| prism_core::controls::OverlayConfig::decode(&n.config).fixed);
            if !fixed {
                dispatch(UiEvent::Back(overlay, 0));
            }
        }));
        Ok(())
    }))?;
    Ok(b)
}

/// The pointer over and pressing the view, reported to prism-native
/// (hover and pressed style refinements).
pub fn hover(outer: &Canvas, id: NodeId) -> Result<()> {
    let state = |state: Interaction, on: bool| {
        PointerEventHandler::new(move |_, _| {
            defer(Box::new(move |s| {
                if s.renderer.set_interaction(id, state, on) {
                    crate::imp::relayout(s);
                }
            }));
            Ok(())
        })
    };
    outer.PointerEntered(&state(Interaction::Hover, true))?;
    outer.PointerExited(&state(Interaction::Hover, false))?;
    outer.PointerPressed(&state(Interaction::Active, true))?;
    outer.PointerReleased(&state(Interaction::Active, false))?;
    outer.PointerCanceled(&state(Interaction::Active, false))?;
    outer.PointerCaptureLost(&state(Interaction::Active, false))?;
    Ok(())
}

/// A clickable view's button pressed or released (buttons take the
/// pointer: their `IsPressed` tells).
pub fn pressed(b: &Button, id: NodeId) -> Result<()> {
    b.RegisterPropertyChangedCallback(
        &ButtonBase::IsPressedProperty()?,
        &DependencyPropertyChangedCallback::new(move |s, _| {
            let on = s
                .as_ref()
                .and_then(|s| s.cast::<ButtonBase>().ok())
                .and_then(|b| b.IsPressed().ok())
                .unwrap_or(false);
            defer(Box::new(move |s| {
                if s.renderer.set_interaction(id, Interaction::Active, on) {
                    crate::imp::relayout(s);
                }
            }));
            Ok(())
        }),
    )?;
    Ok(())
}

/// A control took or lost the keyboard focus.
pub fn focus(e: &UIElement, id: NodeId) -> Result<()> {
    e.GotFocus(&RoutedEventHandler::new(move |_, _| {
        crate::imp::focus_changed(id, true);
        Ok(())
    }))?;
    e.LostFocus(&RoutedEventHandler::new(move |_, _| {
        crate::imp::focus_changed(id, false);
        Ok(())
    }))?;
    Ok(())
}

/// A scroll view moved: the scene follows, listeners hear it.
pub fn scrolled(sv: &ScrollViewer, id: NodeId) -> Result<()> {
    sv.ViewChanged(&windows::Foundation::EventHandler::<
        ScrollViewerViewChangedEventArgs,
    >::new(move |s, _| {
        if let Some(sv) = s.as_ref().and_then(|s| s.cast::<ScrollViewer>().ok()) {
            let (x, y) = (sv.HorizontalOffset()? as f32, sv.VerticalOffset()? as f32);
            crate::imp::scrolled(id, x, y);
        }
        Ok(())
    }))?;
    Ok(())
}

/// A menu as a `MenuFlyout`: items report `UiEvent::MenuSelect`.
pub fn menu(spec: &MenuSpec) -> Result<MenuFlyout> {
    let flyout = MenuFlyout::new()?;
    let items = flyout.Items()?;
    for item in &spec.items {
        if let Some(e) = menu_item(item)? {
            items.Append(&e)?;
        }
    }
    Ok(flyout)
}

fn menu_item(item: &MenuItemSpec) -> Result<Option<xaml::Controls::MenuFlyoutItemBase>> {
    Ok(Some(match item {
        MenuItemSpec::Action {
            id,
            title,
            checked,
            enabled,
            ..
        } => {
            let id = *id;
            let click = RoutedEventHandler::new(move |_, _| {
                dispatch(UiEvent::MenuSelect(id));
                Ok(())
            });
            match checked {
                Some(on) => {
                    let t = ToggleMenuFlyoutItem::new()?;
                    t.SetText(&h(title))?;
                    t.SetIsChecked(*on)?;
                    t.SetIsEnabled(*enabled)?;
                    t.Click(&click)?;
                    t.cast()?
                }
                None => {
                    let m = MenuFlyoutItem::new()?;
                    m.SetText(&h(title))?;
                    m.SetIsEnabled(*enabled)?;
                    m.Click(&click)?;
                    m.cast()?
                }
            }
        }
        MenuItemSpec::Separator => MenuFlyoutSeparator::new()?.cast()?,
        MenuItemSpec::Submenu { title, menu, .. } => {
            let sub = MenuFlyoutSubItem::new()?;
            sub.SetText(&h(title))?;
            let list = sub.Items()?;
            for i in &menu.items {
                if let Some(e) = menu_item(i)? {
                    list.Append(&e)?;
                }
            }
            sub.cast()?
        }
        // Platform actions (#161): none on Windows yet.
        MenuItemSpec::System { .. } => return Ok(None),
    }))
}

/// Opens `spec` over `target` at `(x, y)` (its coordinates).
pub fn show_menu(target: &UIElement, spec: &MenuSpec, x: f32, y: f32) -> Result<()> {
    let flyout = menu(spec)?;
    let options = FlyoutShowOptions::new()?;
    options.SetPosition(
        &windows::Foundation::PropertyValue::CreatePoint(windows::Foundation::Point {
            X: x,
            Y: y,
        })?
        .cast::<windows::Foundation::IReference<windows::Foundation::Point>>()?,
    )?;
    let base: FlyoutBase = flyout.cast()?;
    base.ShowAtWithOptions(target, &options)
}
