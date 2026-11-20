#![allow(non_upper_case_globals)]
//! UI Automation for the scene: the window returns a fragment root from
//! `WM_GETOBJECT`; every shown node is a fragment (name, control type,
//! bounds, states) with the patterns its kind has: Invoke (buttons,
//! clickable views), Toggle (switches, check boxes), Value (text fields),
//! SelectionItem (tabs), Scroll (scroll views).
//!
//! Providers say `ProviderOptions_UseComThreading`: UIA calls them on the
//! UI thread (an STA) through COM, never from its own threads.

use std::ffi::c_void;

use prism_core::Role;
use prism_core::{NodeId, NodeKind};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::{BOOL, BSTR, Error, IUnknown, Result, implement};
use windows_core::IUnknownImpl;

use crate::imp::with_state;
use crate::scene::{Rect, Scene};
use crate::win::WM_PRISM_INVOKE;

/// The window itself (the fragment root).
const ROOT: NodeId = u64::MAX;

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    IInvokeProvider,
    IToggleProvider,
    IValueProvider,
    ISelectionItemProvider,
    IScrollProvider
)]
pub struct Element {
    hwnd: HWND,
    id: NodeId,
}

/// What `WM_GETOBJECT` returns for `UiaRootObjectId`.
pub fn root(hwnd: HWND) -> IRawElementProviderSimple {
    Element { hwnd, id: ROOT }.into()
}

fn fragment(hwnd: HWND, id: NodeId) -> IRawElementProviderFragment {
    Element { hwnd, id }.into()
}

fn gone() -> Error {
    Error::from_hresult(windows::core::HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32))
}

/// "No element": S_OK with a null pointer.
fn none<T>() -> Result<T> {
    Err(Error::empty())
}

/// Reads the scene (on the UI thread; `None` while it is being changed).
fn scene<R>(f: impl FnOnce(&Scene, &crate::widgets::Win) -> R) -> Result<R> {
    with_state(|s| {
        let w = &s.renderer.widgets;
        f(&w.scene, w)
    })
    .ok_or_else(gone)
}

fn exposed(scene: &Scene, id: NodeId) -> bool {
    scene.get(id).is_some_and(|n| !n.hidden && !n.a11y.hidden)
}

/// A node's children in the UIA tree.
fn children(scene: &Scene, id: NodeId) -> Vec<NodeId> {
    let list: Vec<NodeId> = if id == ROOT {
        let mut v: Vec<NodeId> = scene.roots().collect();
        v.extend(
            scene
                .open_overlays()
                .into_iter()
                .filter_map(|o| scene.get(o).and_then(|o| o.children.first().copied())),
        );
        v
    } else {
        let Some(n) = scene.get(id) else {
            return Vec::new();
        };
        match n.kind {
            // Every tab is an item; only the selected one has content.
            NodeKind::Tabs => n.children.clone(),
            NodeKind::Tab if !scene.is_shown(id) => Vec::new(),
            NodeKind::Overlay => Vec::new(),
            _ => scene.shown_children(id).into_owned(),
        }
    };
    list.into_iter().filter(|&c| exposed(scene, c)).collect()
}

fn parent(scene: &Scene, id: NodeId) -> Option<NodeId> {
    if id == ROOT {
        return None;
    }
    match scene.get(id)?.parent {
        Some(p) if scene.get(p).is_some_and(|p| p.kind != NodeKind::Overlay) => Some(p),
        _ => Some(ROOT),
    }
}

/// What a screen reader reads: the label, else the text shown.
fn name(scene: &Scene, id: NodeId) -> String {
    let Some(n) = scene.get(id) else {
        return String::new();
    };
    if let Some(l) = &n.a11y.label {
        return l.clone();
    }
    if let Some(t) = scene.own_text(id) {
        return t;
    }
    match n.kind {
        NodeKind::TextInput | NodeKind::TextArea | NodeKind::RichInput => n.placeholder.clone(),
        NodeKind::Tab | NodeKind::Screen => n.text.clone(),
        NodeKind::View if n.clickable => {
            // A clickable view reads what it shows (not the controls in it).
            let mut out = Vec::new();
            let mut stack = vec![id];
            while let Some(c) = stack.pop() {
                if c != id
                    && let Some(t) = scene.own_text(c)
                {
                    out.push(t);
                }
                if let Some(cn) = scene.get(c)
                    && (c == id || !cn.is_pressable())
                    && !cn.a11y.hidden
                {
                    stack.extend(cn.children.iter().rev());
                }
            }
            out.join(" ")
        }
        _ => String::new(),
    }
}

fn control_type(scene: &Scene, id: NodeId) -> UIA_CONTROLTYPE_ID {
    if id == ROOT {
        return UIA_WindowControlTypeId;
    }
    let Some(n) = scene.get(id) else {
        return UIA_GroupControlTypeId;
    };
    // The tab bar's items, whatever role their content has.
    match n.kind {
        NodeKind::Tabs => return UIA_TabControlTypeId,
        NodeKind::Tab => return UIA_TabItemControlTypeId,
        _ => {}
    }
    if let Some(role) = n.a11y.role {
        return match role {
            Role::Button | Role::Switch => UIA_ButtonControlTypeId,
            Role::Link => UIA_HyperlinkControlTypeId,
            Role::Heading(_) | Role::Text | Role::Status | Role::Alert => UIA_TextControlTypeId,
            Role::Image => UIA_ImageControlTypeId,
            Role::List => UIA_ListControlTypeId,
            Role::ListItem => UIA_ListItemControlTypeId,
            Role::Checkbox => UIA_CheckBoxControlTypeId,
            Role::Tab => UIA_TabItemControlTypeId,
            Role::TabList => UIA_TabControlTypeId,
            Role::ProgressBar => UIA_ProgressBarControlTypeId,
            Role::Slider => UIA_SliderControlTypeId,
            Role::ComboBox => UIA_ComboBoxControlTypeId,
            Role::Radio => UIA_RadioButtonControlTypeId,
            Role::Dialog => UIA_WindowControlTypeId,
            _ => UIA_GroupControlTypeId,
        };
    }
    match n.kind {
        NodeKind::Button | NodeKind::Toggle => UIA_ButtonControlTypeId,
        NodeKind::Text | NodeKind::Timer => UIA_TextControlTypeId,
        NodeKind::TextInput | NodeKind::TextArea | NodeKind::RichInput => UIA_EditControlTypeId,
        NodeKind::Checkbox => UIA_CheckBoxControlTypeId,
        NodeKind::Slider => UIA_SliderControlTypeId,
        NodeKind::Progress | NodeKind::Spinner => UIA_ProgressBarControlTypeId,
        NodeKind::Picker => UIA_ComboBoxControlTypeId,
        NodeKind::DatePicker => UIA_ButtonControlTypeId,
        NodeKind::Image | NodeKind::Picture | NodeKind::Shape => UIA_ImageControlTypeId,
        NodeKind::Tabs => UIA_TabControlTypeId,
        NodeKind::Tab => UIA_TabItemControlTypeId,
        NodeKind::Scroll | NodeKind::Stack | NodeKind::Screen => UIA_PaneControlTypeId,
        NodeKind::RadioGroup => UIA_GroupControlTypeId,
        _ if n.clickable => UIA_ButtonControlTypeId,
        _ => UIA_GroupControlTypeId,
    }
}

/// Where a tab's item is in the bar (window, view units).
fn tab_rect(w: &crate::widgets::Win, tab: NodeId) -> Option<Rect> {
    let scene = &w.scene;
    let tabs = scene.get(tab)?.parent?;
    let i = scene.pages(tabs).iter().position(|&t| t == tab)?;
    let r = crate::controls::tab_items(&w.gfx, &w.texts, scene, tabs)
        .get(i)
        .copied()?;
    let (x, y) = scene.origin(tabs);
    Some(r.offset(x, y))
}

impl Element {
    fn screen_rect(&self, r: Rect, scale: f32) -> UiaRect {
        let mut p = POINT::default();
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut p);
        }
        UiaRect {
            left: p.x as f64 + (r.x * scale) as f64,
            top: p.y as f64 + (r.y * scale) as f64,
            width: (r.w * scale) as f64,
            height: (r.h * scale) as f64,
        }
    }
}

impl IRawElementProviderSimple_Impl for Element_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading)
    }

    fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
        if self.id == ROOT {
            return none();
        }
        let (kind, clickable, field) = scene(|s, _| {
            s.get(self.id)
                .map(|n| (n.kind, n.is_pressable(), n.field.is_some()))
        })?
        .ok_or_else(gone)?;
        let has = match pattern {
            UIA_InvokePatternId => {
                clickable && !matches!(kind, NodeKind::Toggle | NodeKind::Checkbox)
                    || kind == NodeKind::Tab
            }
            UIA_TogglePatternId => matches!(kind, NodeKind::Toggle | NodeKind::Checkbox),
            UIA_ValuePatternId => field,
            UIA_SelectionItemPatternId => kind == NodeKind::Tab,
            UIA_ScrollPatternId => kind == NodeKind::Scroll,
            _ => false,
        };
        if has {
            Ok(self.to_interface::<IUnknown>())
        } else {
            none()
        }
    }

    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let id = self.id;
        scene(|s, w| {
            if id == ROOT {
                return match property {
                    UIA_NamePropertyId => VARIANT::from(w.app_title.as_str()),
                    UIA_FrameworkIdPropertyId => VARIANT::from("Prism"),
                    _ => VARIANT::default(),
                };
            }
            let Some(n) = s.get(id) else {
                return VARIANT::default();
            };
            let plain = n.kind == NodeKind::View
                && n.a11y.role.is_none()
                && n.a11y.label.is_none()
                && !n.clickable;
            match property {
                UIA_ControlTypePropertyId => VARIANT::from(control_type(s, id).0),
                UIA_NamePropertyId => VARIANT::from(name(s, id).as_str()),
                UIA_HelpTextPropertyId => n
                    .a11y
                    .hint
                    .as_deref()
                    .map_or_else(VARIANT::default, VARIANT::from),
                UIA_AutomationIdPropertyId => VARIANT::from(format!("n{id}").as_str()),
                UIA_ClassNamePropertyId => {
                    VARIANT::from(format!("Prism.{}", n.kind.name()).as_str())
                }
                UIA_FrameworkIdPropertyId => VARIANT::from("Prism"),
                UIA_IsKeyboardFocusablePropertyId => VARIANT::from(n.is_focusable()),
                UIA_HasKeyboardFocusPropertyId => VARIANT::from(w.focus == Some(id)),
                UIA_IsEnabledPropertyId => VARIANT::from(!n.a11y.disabled),
                UIA_IsOffscreenPropertyId => {
                    VARIANT::from(s.visible_rect(id).is_none() && n.kind != NodeKind::Tab)
                }
                UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId => {
                    VARIANT::from(!plain)
                }
                UIA_HeadingLevelPropertyId => match n.a11y.role {
                    Some(Role::Heading(l)) => {
                        VARIANT::from(HeadingLevel1.0 + l.clamp(1, 9) as i32 - 1)
                    }
                    _ => VARIANT::default(),
                },
                _ => VARIANT::default(),
            }
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        if self.id == ROOT {
            return unsafe { UiaHostProviderFromHwnd(self.hwnd) };
        }
        none()
    }
}

impl IRawElementProviderFragment_Impl for Element_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let id = self.id;
        let target = scene(|s, _| match direction {
            NavigateDirection_Parent => parent(s, id),
            NavigateDirection_FirstChild => children(s, id).first().copied(),
            NavigateDirection_LastChild => children(s, id).last().copied(),
            NavigateDirection_NextSibling | NavigateDirection_PreviousSibling => {
                let p = parent(s, id)?;
                let list = children(s, p);
                let i = list.iter().position(|&c| c == id)? as isize;
                let j = if direction == NavigateDirection_NextSibling {
                    i + 1
                } else {
                    i - 1
                };
                list.get(usize::try_from(j).ok()?).copied()
            }
            _ => None,
        })?;
        match target {
            Some(t) => Ok(fragment(self.hwnd, t)),
            None => none(),
        }
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        if self.id == ROOT {
            return Ok(std::ptr::null_mut());
        }
        unsafe {
            let sa = SafeArrayCreateVector(VT_I4, 0, 3);
            let parts = [
                UiaAppendRuntimeId as i32,
                (self.id >> 32) as i32,
                self.id as i32,
            ];
            for (i, v) in parts.iter().enumerate() {
                SafeArrayPutElement(sa, &(i as i32), v as *const i32 as *const c_void)?;
            }
            Ok(sa)
        }
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        if self.id == ROOT {
            return Ok(UiaRect::default());
        }
        let id = self.id;
        scene(|s, w| {
            let r = match s.get(id).map(|n| n.kind) {
                Some(NodeKind::Tab) => tab_rect(w, id),
                _ => s.visible_rect(id),
            };
            r.map_or(UiaRect::default(), |r| self.screen_rect(r, w.scale()))
        })
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        unsafe {
            PostMessageW(
                Some(self.hwnd),
                crate::win::WM_PRISM_FOCUS,
                WPARAM(2),
                LPARAM(self.id as isize),
            )
        }
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(Element {
            hwnd: self.hwnd,
            id: ROOT,
        }
        .into())
    }
}

impl IRawElementProviderFragmentRoot_Impl for Element_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let mut p = POINT {
            x: x as i32,
            y: y as i32,
        };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut p);
        }
        let hit = scene(|s, w| {
            let k = w.scale();
            let mut hit = s.hit_test(p.x as f32 / k, p.y as f32 / k)?;
            // The nearest node in the UIA tree.
            while !exposed(s, hit) {
                hit = s.get(hit)?.parent?;
            }
            Some(hit)
        })?;
        match hit {
            Some(id) => Ok(fragment(self.hwnd, id)),
            None => none(),
        }
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        match scene(|_, w| w.focus)? {
            Some(id) => Ok(fragment(self.hwnd, id)),
            None => none(),
        }
    }
}

impl IInvokeProvider_Impl for Element_Impl {
    fn Invoke(&self) -> Result<()> {
        // Asynchronously, as UIA recommends: the app runs on the UI thread.
        unsafe {
            PostMessageW(
                Some(self.hwnd),
                WM_PRISM_INVOKE,
                WPARAM(0),
                LPARAM(self.id as isize),
            )
        }
    }
}

impl IToggleProvider_Impl for Element_Impl {
    fn Toggle(&self) -> Result<()> {
        IInvokeProvider_Impl::Invoke(self)
    }

    fn ToggleState(&self) -> Result<ToggleState> {
        let on = scene(|s, _| s.get(self.id).is_some_and(|n| n.value >= 0.5))?;
        Ok(if on { ToggleState_On } else { ToggleState_Off })
    }
}

impl IValueProvider_Impl for Element_Impl {
    fn SetValue(&self, value: &windows::core::PCWSTR) -> Result<()> {
        let f = scene(|_, w| w.field_of(self.id))?.ok_or_else(gone)?;
        let text = unsafe { value.to_string().unwrap_or_default() };
        crate::imp::fill(f, &text);
        Ok(())
    }

    fn Value(&self) -> Result<BSTR> {
        let f = scene(|_, w| w.field_of(self.id))?.ok_or_else(gone)?;
        Ok(BSTR::from(crate::fields::text(f)))
    }

    fn IsReadOnly(&self) -> Result<BOOL> {
        Ok(false.into())
    }
}

impl ISelectionItemProvider_Impl for Element_Impl {
    fn Select(&self) -> Result<()> {
        IInvokeProvider_Impl::Invoke(self)
    }

    fn AddToSelection(&self) -> Result<()> {
        IInvokeProvider_Impl::Invoke(self)
    }

    fn RemoveFromSelection(&self) -> Result<()> {
        Ok(())
    }

    fn IsSelected(&self) -> Result<BOOL> {
        let id = self.id;
        Ok(scene(|s, _| s.is_shown(id))?.into())
    }

    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        let p = scene(|s, _| s.get(self.id).and_then(|n| n.parent))?;
        match p {
            Some(p) => Ok(Element {
                hwnd: self.hwnd,
                id: p,
            }
            .into()),
            None => none(),
        }
    }
}

const STEP: f32 = 48.;

impl IScrollProvider_Impl for Element_Impl {
    fn Scroll(&self, horizontal: ScrollAmount, vertical: ScrollAmount) -> Result<()> {
        let id = self.id;
        crate::imp::with_state(|s| {
            let w = &mut s.renderer.widgets;
            let Some(n) = w.scene.get(id) else { return };
            let amount = |a: ScrollAmount, page: f32| match a {
                ScrollAmount_SmallIncrement => STEP,
                ScrollAmount_SmallDecrement => -STEP,
                ScrollAmount_LargeIncrement => page,
                ScrollAmount_LargeDecrement => -page,
                _ => 0.,
            };
            let (dx, dy) = (
                amount(horizontal, n.frame.w * 0.9),
                amount(vertical, n.frame.h * 0.9),
            );
            if w.scroll_by(id, dx, dy) {
                crate::input::scrolled(s, id);
            }
            s.renderer.widgets.flush();
        })
        .ok_or_else(gone)
    }

    fn SetScrollPercent(&self, horizontal: f64, vertical: f64) -> Result<()> {
        let id = self.id;
        crate::imp::with_state(|s| {
            let w = &mut s.renderer.widgets;
            let Some(n) = w.scene.get(id) else { return };
            let (mx, my) = n.max_scroll();
            let to = |p: f64, max: f32, cur: f32| {
                if p == UIA_ScrollPatternNoScroll {
                    0.
                } else {
                    (p as f32 / 100.) * max - cur
                }
            };
            let (dx, dy) = (to(horizontal, mx, n.scroll.0), to(vertical, my, n.scroll.1));
            if w.scroll_by(id, dx, dy) {
                crate::input::scrolled(s, id);
            }
            s.renderer.widgets.flush();
        })
        .ok_or_else(gone)
    }

    fn HorizontalScrollPercent(&self) -> Result<f64> {
        scene(|s, _| {
            s.get(self.id).map_or(UIA_ScrollPatternNoScroll, |n| {
                let m = n.max_scroll().0;
                if m > 0. {
                    (n.scroll.0 / m * 100.) as f64
                } else {
                    UIA_ScrollPatternNoScroll
                }
            })
        })
    }

    fn VerticalScrollPercent(&self) -> Result<f64> {
        scene(|s, _| {
            s.get(self.id).map_or(UIA_ScrollPatternNoScroll, |n| {
                let m = n.max_scroll().1;
                if m > 0. {
                    (n.scroll.1 / m * 100.) as f64
                } else {
                    UIA_ScrollPatternNoScroll
                }
            })
        })
    }

    fn HorizontalViewSize(&self) -> Result<f64> {
        scene(|s, _| {
            s.get(self.id).map_or(100., |n| {
                if n.content.0 > 0. {
                    (n.frame.w / n.content.0 * 100.).min(100.) as f64
                } else {
                    100.
                }
            })
        })
    }

    fn VerticalViewSize(&self) -> Result<f64> {
        scene(|s, _| {
            s.get(self.id).map_or(100., |n| {
                if n.content.1 > 0. {
                    (n.frame.h / n.content.1 * 100.).min(100.) as f64
                } else {
                    100.
                }
            })
        })
    }

    fn HorizontallyScrollable(&self) -> Result<BOOL> {
        Ok(scene(|s, _| s.get(self.id).is_some_and(|n| n.max_scroll().0 > 0.))?.into())
    }

    fn VerticallyScrollable(&self) -> Result<BOOL> {
        Ok(scene(|s, _| s.get(self.id).is_some_and(|n| n.max_scroll().1 > 0.))?.into())
    }
}

/// A node's description changed: listening clients refresh it.
pub fn changed(hwnd: HWND, id: NodeId) {
    unsafe {
        if !UiaClientsAreListening().as_bool() {
            return;
        }
        let e: IRawElementProviderSimple = Element { hwnd, id }.into();
        let _ = UiaRaiseAutomationPropertyChangedEvent(
            &e,
            UIA_NamePropertyId,
            &VARIANT::default(),
            &VARIANT::default(),
        );
    }
}

/// The keyboard focus moved to `id`.
pub fn focus_changed(hwnd: HWND, id: Option<NodeId>) {
    let Some(id) = id else { return };
    unsafe {
        if !UiaClientsAreListening().as_bool() {
            return;
        }
        let e: IRawElementProviderSimple = Element { hwnd, id }.into();
        let _ = UiaRaiseAutomationEvent(&e, UIA_AutomationFocusChangedEventId);
    }
}

/// `WM_GETOBJECT`.
pub fn get_object(
    hwnd: HWND,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<windows::Win32::Foundation::LRESULT> {
    if lparam.0 as i32 != UiaRootObjectId {
        return None;
    }
    let p = root(hwnd);
    Some(unsafe { UiaReturnRawElementProvider(hwnd, wparam, lparam, &p) })
}
