//! XAML input on Prism's own views (controls report theirs in
//! `controls.rs`): clicks on clickable views with their position, keys
//! and count, the pointer over and pressing a view (interaction states),
//! the keyboard focus, scrolling, context and pop-up menus.

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::menu::{MenuItemSpec, MenuSpec};
use prism_core::{ClickEvent, Interaction, Modifiers, NodeId, Point, PointerButton, UiEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXDOUBLECLK, SM_CYDOUBLECLK};
use windows_core::{IInspectable, Interface, Result};

use crate::clicks::{Limits, Series};
use crate::imp::{defer, dispatch};
use crate::winui::xaml::Controls::Primitives::{ButtonBase, FlyoutBase, FlyoutShowOptions};
use crate::winui::xaml::Controls::{
    Button, Canvas, MenuFlyout, MenuFlyoutItem, MenuFlyoutSeparator, MenuFlyoutSubItem,
    ScrollViewer, ScrollViewerViewChangedEventArgs, ToggleMenuFlyoutItem,
};
use crate::winui::xaml::Input::{
    DoubleTappedEventHandler, PointerEventHandler, PointerRoutedEventArgs, TappedEventHandler,
};
use crate::winui::xaml::{DependencyPropertyChangedCallback, RoutedEventHandler, UIElement};
use crate::winui::{h, xaml};

/// A click's position (in its view), keys and count, from the pointer.
#[derive(Clone, Copy, Debug)]
struct Details {
    at: Point,
    modifiers: Modifiers,
    count: u32,
}

/// A clickable button's press, until its release.
#[derive(Clone, Copy, Debug)]
struct Pending {
    modifiers: Modifiers,
    count: u32,
    /// The button clicked before the release was heard.
    clicked: bool,
}

thread_local! {
    /// The last press on a clickable view, and its number in its series.
    static SERIES: RefCell<Series> = RefCell::new(Series::default());
    /// Clickable buttons pressed with the pointer, until released.
    static PENDING: RefCell<HashMap<NodeId, Pending>> = RefCell::default();
    /// A button's release heard before its click (until the next turn).
    static RELEASED: RefCell<HashMap<NodeId, Details>> = RefCell::default();
}

fn click_with(id: NodeId, d: Details) -> UiEvent {
    UiEvent::ClickWith(
        id,
        ClickEvent {
            position: Some(d.at),
            modifiers: d.modifiers,
            count: d.count,
            button: PointerButton::Primary,
            text_offset: None,
        },
    )
}

/// The system's double-click time and rectangle, in DIPs.
fn limits() -> Limits {
    let scale = crate::imp::with_state(|s| s.renderer.widgets.scale()).unwrap_or(1.);
    unsafe {
        Limits {
            time: GetDoubleClickTime() as u64,
            width: GetSystemMetrics(SM_CXDOUBLECLK).max(1) as f32 / scale,
            height: GetSystemMetrics(SM_CYDOUBLECLK).max(1) as f32 / scale,
        }
    }
}

/// A primary press (the left button, a finger, a pen's tip) on clickable
/// view (or field) `id`: its number in its series.
pub(crate) fn count_press(id: NodeId, a: &PointerRoutedEventArgs) -> Option<u32> {
    let p = a.GetCurrentPoint(None::<&UIElement>).ok()?;
    if !p.Properties().ok()?.IsLeftButtonPressed().ok()? {
        return None;
    }
    let at = p.Position().ok()?;
    let (pointer, time) = (p.PointerId().ok()?, p.Timestamp().ok()?);
    let limits = limits();
    Some(SERIES.with(|s| {
        s.borrow_mut()
            .press(id, pointer, time, (at.X, at.Y), limits)
    }))
}

/// Where the pointer is in element `e`.
fn position(e: Option<&IInspectable>, a: &PointerRoutedEventArgs) -> Option<Point> {
    let e: UIElement = e?.cast().ok()?;
    let p = a.GetCurrentPoint(&e).ok()?.Position().ok()?;
    Some(Point { x: p.X, y: p.Y })
}

/// A clickable view: a tap (or a click) reports `UiEvent::ClickWith`
/// with where it landed, the keys held and its number in a quick series
/// (a double tap is the second click, as on macOS).
pub fn clickable(outer: &Canvas, id: NodeId) -> Result<()> {
    outer.PointerPressed(&PointerEventHandler::new(move |_, a| {
        if let Some(a) = a.as_ref() {
            count_press(id, a);
        }
        Ok(())
    }))?;
    outer.Tapped(&TappedEventHandler::new(move |s, a| {
        let Some(a) = a.as_ref() else {
            tapped(id, None);
            return Ok(());
        };
        if a.Handled()? {
            return Ok(());
        }
        a.SetHandled(true)?;
        let e: Option<UIElement> = s.as_ref().and_then(|s| s.cast().ok());
        tapped(id, a.GetPosition(e.as_ref()).ok());
        Ok(())
    }))?;
    // XAML sends a double tap's second tap as `DoubleTapped` alone.
    outer.DoubleTapped(&DoubleTappedEventHandler::new(move |s, a| {
        let Some(a) = a.as_ref() else {
            tapped(id, None);
            return Ok(());
        };
        if a.Handled()? {
            return Ok(());
        }
        a.SetHandled(true)?;
        let e: Option<UIElement> = s.as_ref().and_then(|s| s.cast().ok());
        tapped(id, a.GetPosition(e.as_ref()).ok());
        Ok(())
    }))?;
    Ok(())
}

/// Clickable view `id` tapped at `at` (in it).
fn tapped(id: NodeId, at: Option<windows::Foundation::Point>) {
    dispatch(match at {
        Some(p) => click_with(
            id,
            Details {
                at: Point { x: p.X, y: p.Y },
                modifiers: crate::keyboard::modifiers(),
                count: SERIES.with(|c| c.borrow().count(id)),
            },
        ),
        None => UiEvent::Click(id),
    });
}

/// A clickable view's button: its presses and releases, heard though the
/// button handles them (`handledEventsToo`), give its clicks their
/// position, keys and count ([`button_clicked`]).
pub fn button_presses(b: &Button, id: NodeId) -> Result<()> {
    let e: UIElement = b.cast()?;
    let pressed = PointerEventHandler::new(move |_, a| {
        if let Some(a) = a.as_ref()
            && let Some(count) = count_press(id, a)
        {
            let p = Pending {
                modifiers: crate::pointer::modifiers(a),
                count,
                clicked: false,
            };
            PENDING.with(|m| m.borrow_mut().insert(id, p));
        }
        Ok(())
    });
    // The press ends: released, canceled or its capture lost (the button
    // lets go of it as it clicks), in whichever order XAML sends them.
    let ended = PointerEventHandler::new(move |s, a| {
        let Some(p) = PENDING.with(|m| m.borrow_mut().remove(&id)) else {
            return Ok(());
        };
        let Some(at) = a.as_ref().and_then(|a| position(s.as_ref(), a)) else {
            return Ok(());
        };
        let d = Details {
            at,
            modifiers: p.modifiers,
            count: p.count,
        };
        if p.clicked {
            dispatch(click_with(id, d));
        } else {
            // A click may follow in this turn.
            RELEASED.with(|m| m.borrow_mut().insert(id, d));
            defer(Box::new(move |_| {
                RELEASED.with(|m| m.borrow_mut().remove(&id));
            }));
        }
        Ok(())
    });
    e.AddHandler(
        &UIElement::PointerPressedEvent()?,
        &crate::boxed::boxed(pressed),
        true,
    )?;
    for event in [
        UIElement::PointerReleasedEvent()?,
        UIElement::PointerCanceledEvent()?,
        UIElement::PointerCaptureLostEvent()?,
    ] {
        e.AddHandler(&event, &crate::boxed::boxed(ended.clone()), true)?;
    }
    Ok(())
}

/// Clickable view `id`'s button clicked: with the pointer's position,
/// keys and count when a press led to it, else (Space, Enter, UI
/// Automation's Invoke) without.
pub fn button_clicked(id: NodeId) {
    let pressed = PENDING.with(|m| {
        m.borrow_mut()
            .get_mut(&id)
            .map(|p| p.clicked = true)
            .is_some()
    });
    if pressed {
        // Reported with the release's position, once heard.
        return;
    }
    match RELEASED.with(|m| m.borrow_mut().remove(&id)) {
        Some(d) => dispatch(click_with(id, d)),
        None => dispatch(UiEvent::Click(id)),
    }
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
