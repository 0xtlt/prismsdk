//! Custom window chrome (`WindowChrome::Custom`; `chrome.rs` has the
//! rules): the window's `AppWindow` extends the content into its title bar
//! (`AppWindowTitleBar::ExtendsContentIntoTitleBar`); the system still
//! draws the caption buttons, over the content's top right corner, on a
//! transparent background and in the app's light or dark theme. The
//! window's top bar (the menu bar, else a tab bar or a navigation header
//! along the top edge) and the `.window_drag()` views are its caption
//! regions, and what keeps its presses inside them passthrough regions
//! (`InputNonClientPointerSource`, `chrome.rs` has the rules), set again
//! after each layout and scroll, and when a bar's menus, tabs or buttons
//! move. `cx.window_controls()` is the buttons' corner (`RightInset` or
//! `LeftInset`, `Height`) only while they are over the app's own views:
//! with a menu bar they are in its row, and over a tab bar or navigation
//! header along the top edge that bar makes room for them; it is `None`
//! then, so an app's title bar does not draw a second one under the bar.
//! It follows navigations, bars shown or hidden and resizes ([`follow`]).

use core::ffi::c_void;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};

use prism_core::{NodeId, WindowControls};
use windows::Foundation::IReference;
use windows::Foundation::Point;
use windows::Graphics::RectInt32;
use windows::UI::Color;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::IAgileObject;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows_core::{
    GUID, HRESULT, IInspectable, IInspectable_Vtbl, IUnknown, IUnknown_Vtbl, Interface, Result,
};

use crate::chrome::{self, TopBar};
use crate::imp::State;
use crate::scene::{HEADER, Rect, Scene, TAB_BAR};
use crate::view::Bar as ViewBar;
use crate::widgets::Win;
use crate::win::log;
use crate::winui::bindings::Microsoft::UI::Input::{
    InputNonClientPointerSource, NonClientRegionKind,
};
use crate::winui::bindings::Microsoft::UI::Windowing::{
    AppWindow, AppWindowTitleBar, TitleBarTheme,
};
use crate::winui::xaml::{FrameworkElement, SizeChangedEventHandler, UIElement, Visibility};

/// Regions in pixels: captions, passthroughs.
type Pixels = (Vec<[i32; 4]>, Vec<[i32; 4]>);

struct Bar {
    hwnd: HWND,
    _window: AppWindow,
    title: AppWindowTitleBar,
    input: InputNonClientPointerSource,
    /// The regions set last (`None`: set them again).
    regions: Option<Pixels>,
    /// The caption buttons' frame (DIPs from the window's top left).
    buttons: Option<WindowControls>,
    /// What `cx.window_controls()` was told last.
    reported: Option<Option<WindowControls>>,
}

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

/// Takes window `hwnd`'s title bar away (once its XAML island is there:
/// the caption buttons go above it). Returns whether it could.
pub fn install(hwnd: HWND, dark: bool) -> bool {
    match make(hwnd, dark) {
        Ok(Some(bar)) => {
            BAR.with(|b| *b.borrow_mut() = Some(bar));
            true
        }
        Ok(None) => {
            log!("custom window chrome: the system cannot customize the title bar");
            false
        }
        Err(e) => {
            log!("custom window chrome: {e}");
            false
        }
    }
}

fn make(hwnd: HWND, dark: bool) -> Result<Option<Bar>> {
    if !AppWindowTitleBar::IsCustomizationSupported()? {
        return Ok(None);
    }
    let id = crate::host::window_id(hwnd)?;
    let window = AppWindow::GetFromWindowId(id)?;
    let title = window.TitleBar()?;
    title.SetExtendsContentIntoTitleBar(true)?;
    // The content shows behind the buttons (only their hover and press
    // backgrounds are drawn).
    let clear = boxed_color(Color {
        A: 0,
        R: 0,
        G: 0,
        B: 0,
    });
    if let Err(e) = title
        .SetButtonBackgroundColor(&clear)
        .and_then(|()| title.SetButtonInactiveBackgroundColor(&clear))
    {
        log!("caption buttons' background: {e}");
    }
    title.SetPreferredTheme(theme(dark))?;
    let input = InputNonClientPointerSource::GetForWindowId(id)?;
    Ok(Some(Bar {
        hwnd,
        _window: window,
        title,
        input,
        regions: None,
        buttons: None,
        reported: None,
    }))
}

fn theme(dark: bool) -> TitleBarTheme {
    if dark {
        TitleBarTheme::Dark
    } else {
        TitleBarTheme::Light
    }
}

/// The window, its title bar and its pointer source, out of [`BAR`]: the
/// system calls on them may send the window messages that use it.
fn handles() -> Option<(HWND, AppWindowTitleBar, InputNonClientPointerSource)> {
    BAR.with(|b| {
        b.borrow()
            .as_ref()
            .map(|bar| (bar.hwnd, bar.title.clone(), bar.input.clone()))
    })
}

/// The app turned light or dark: so do the caption buttons.
pub fn set_dark(dark: bool) {
    if let Some((_, title, _)) = handles()
        && let Err(e) = title.SetPreferredTheme(theme(dark))
    {
        log!("caption buttons' theme: {e}");
    }
}

/// The window changed size: the regions are set again after the next
/// layout (the system lays out its own on a resize).
pub fn invalidate() {
    BAR.with(|b| {
        if let Some(bar) = &mut *b.borrow_mut() {
            bar.regions = None;
        }
    });
}

/// The caption buttons' frame, with a custom chrome.
fn buttons() -> Option<WindowControls> {
    BAR.with(|b| b.borrow().as_ref().and_then(|bar| bar.buttons))
}

/// The drag regions follow the views (after a layout or a scroll) and the
/// window's top bar.
pub fn update_regions(s: &State) {
    let Some((_, _, input)) = handles() else {
        return;
    };
    let w = &s.renderer.widgets;
    let k = w.scale();
    // The views start under the menu bar's row, which is then the top bar.
    let top = crate::menubar::height();
    let buttons = buttons();
    let in_app = buttons.map(|b| WindowControls { y: b.y - top, ..b });
    let mut r = chrome::regions(
        &w.scene,
        |id, _| w.keeps_presses(id),
        |id, _| if top > 0. { None } else { bar_of(w, id) },
        in_app.as_ref(),
    )
    .moved(top);
    if let Some(row) = crate::menubar::row(w.size.0) {
        r.extend(chrome::top_bar(&row, buttons.as_ref()));
    }
    let pixels: Pixels = (
        r.caption.iter().map(|r| chrome::px(r, k, false)).collect(),
        r.passthrough
            .iter()
            .map(|r| chrome::px(r, k, true))
            .collect(),
    );
    let same = BAR.with(|b| {
        b.borrow()
            .as_ref()
            .is_some_and(|bar| bar.regions.as_ref() == Some(&pixels))
    });
    if same {
        return;
    }
    match set_regions(&input, &pixels) {
        Ok(()) => BAR.with(|b| {
            if let Some(bar) = &mut *b.borrow_mut() {
                bar.regions = Some(pixels);
            }
        }),
        Err(e) => log!("window drag regions: {e}"),
    }
}

/// The frame of the tab bar or navigation header view `id` shows at its
/// top (window DIPs).
fn bar_frame(w: &Win, id: NodeId) -> Option<Rect> {
    let height = match &w.views.get(&id)?.bar {
        ViewBar::Tabs(_) => TAB_BAR,
        ViewBar::Header(_) if w.scene.header_shown(id) => HEADER,
        _ => return None,
    };
    let r = w.scene.window_rect(id);
    Some(Rect::new(r.x, r.y, r.w, height))
}

/// The tab bar or navigation header view `id` shows at its top, and what
/// takes clicks in it (window DIPs).
fn bar_of(w: &Win, id: NodeId) -> Option<TopBar> {
    let frame = bar_frame(w, id)?;
    let controls = match &w.views.get(&id)?.bar {
        ViewBar::Tabs(t) => t.controls(),
        ViewBar::Header(hd) => hd.controls(),
        ViewBar::None => return None,
    };
    Some(TopBar {
        frame,
        controls: controls.iter().filter_map(bounds).collect(),
    })
}

/// What `cx.window_controls()` says for caption buttons at `buttons`: the
/// buttons only while the app's own views reach them. With a menu bar they
/// are in its row (the app's views start under it); a tab bar or a
/// navigation header along the top edge under them makes room for them
/// itself.
fn over_app(s: &State, buttons: Option<WindowControls>) -> Option<WindowControls> {
    let b = buttons?;
    if crate::menubar::height() > 0. {
        return None;
    }
    let w = &s.renderer.widgets;
    (!chrome::bar_under(&w.scene, |id, _| bar_frame(w, id), &b)).then_some(b)
}

/// After a layout: when a bar came to or left the caption buttons (a
/// navigation, a tab bar or header shown or hidden, a resize),
/// `cx.window_controls()` follows, at the next turn of the loop.
pub fn follow(s: &State) {
    let Some((buttons, reported)) =
        BAR.with(|b| b.borrow().as_ref().map(|b| (b.buttons, b.reported)))
    else {
        return;
    };
    if reported.is_some_and(|r| r != over_app(s, buttons)) {
        crate::imp::defer(Box::new(report_controls));
    }
}

/// The room a bar of view `id` (a tab bar, a navigation header) keeps at
/// its trailing end for the caption buttons over it: along the window's
/// top edge, without a menu bar.
pub fn inset(scene: &Scene, id: NodeId) -> f32 {
    if crate::menubar::height() > 0. {
        return 0.;
    }
    let Some(b) = buttons() else {
        return 0.;
    };
    let r = scene.window_rect(id);
    if r.y.abs() >= 0.5 {
        return 0.;
    }
    let (left, right) = chrome::overlap(&r, &b);
    left + right
}

/// Where `e` is in the window (DIPs), if it is shown.
pub fn bounds(e: &UIElement) -> Option<Rect> {
    if e.Visibility().ok()? != Visibility::Visible {
        return None;
    }
    let fe: FrameworkElement = e.cast().ok()?;
    let (w, h) = (
        fe.ActualWidth().ok()? as f32,
        fe.ActualHeight().ok()? as f32,
    );
    if w <= 0. || h <= 0. {
        return None;
    }
    // Both corners: a right-to-left element's origin is its right edge.
    let t = e.TransformToVisual(None::<&UIElement>).ok()?;
    let a = t.TransformPoint(Point { X: 0., Y: 0. }).ok()?;
    let b = t.TransformPoint(Point { X: w, Y: h }).ok()?;
    Some(Rect::new(
        a.X.min(b.X),
        a.Y.min(b.Y),
        (a.X - b.X).abs(),
        (a.Y - b.Y).abs(),
    ))
}

/// The regions follow `e` (a bar, a menu, a tab) when its size changes:
/// it was laid out, or what it shows changed.
pub fn watch(e: &FrameworkElement) -> Result<()> {
    e.SizeChanged(&SizeChangedEventHandler::new(|_, _| {
        if handles().is_some() {
            crate::imp::defer(Box::new(|s| update_regions(s)));
        }
        Ok(())
    }))?;
    Ok(())
}

fn set_regions(input: &InputNonClientPointerSource, (caption, pass): &Pixels) -> Result<()> {
    let rects = |v: &[[i32; 4]]| -> Vec<RectInt32> {
        v.iter()
            .map(|&[x, y, w, h]| RectInt32 {
                X: x,
                Y: y,
                Width: w,
                Height: h,
            })
            .collect()
    };
    for (kind, v) in [
        (NonClientRegionKind::Caption, caption),
        (NonClientRegionKind::Passthrough, pass),
    ] {
        if v.is_empty() {
            // No drag view: no caption region at all, not the system's
            // default strip.
            input.ClearRegionRects(kind)?;
        } else {
            input.SetRegionRects(kind, &rects(v))?;
        }
    }
    Ok(())
}

/// Reports where the caption buttons are (`cx.window_controls()`) when
/// that changed: re-renders the app.
pub fn report_controls(s: &mut State) {
    let Some((hwnd, title, _)) = handles() else {
        return;
    };
    let mut r = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut r) }.is_err() || r.right <= 0 {
        // Minimized: they stay where they were.
        return;
    }
    let buttons = chrome::controls(
        r.right,
        title.LeftInset().unwrap_or(0),
        title.RightInset().unwrap_or(0),
        title.Height().unwrap_or(0),
        s.renderer.widgets.scale(),
    );
    // Over a menu bar's row or a bar along the top edge, the app's views
    // need no room for them.
    let report = over_app(s, buttons);
    let (moved, changed) = BAR.with(|b| {
        let mut b = b.borrow_mut();
        let Some(bar) = b.as_mut() else {
            return (false, false);
        };
        let moved = bar.buttons != buttons;
        let changed = bar.reported != Some(report);
        bar.buttons = buttons;
        bar.reported = Some(report);
        (moved, changed)
    });
    if moved {
        // The bars under them make room again.
        s.renderer.widgets.restyle_bars();
    }
    if changed {
        let patches = s.runtime.set_window_chrome(false, report);
        crate::imp::commit(s, patches);
    } else if moved {
        crate::imp::relayout(s);
    }
}

// --- A boxed color ---------------------------------------------------------

/// `IReference<Color>`'s vtable: `IInspectable`'s, then `get_Value`.
#[repr(C)]
struct ColorVtbl {
    base: IInspectable_Vtbl,
    value: unsafe extern "system" fn(*mut c_void, *mut Color) -> HRESULT,
}

/// A `Color` as the `IReference<Color>` the title bar's colors take
/// (windows-rs boxes no structs): written by hand, like `boxed.rs`.
#[repr(C)]
struct ColorBox {
    vtable: *const ColorVtbl,
    refs: AtomicU32,
    color: Color,
}

static COLOR_VTABLE: ColorVtbl = ColorVtbl {
    base: IInspectable_Vtbl {
        base: IUnknown_Vtbl {
            QueryInterface: color_qi,
            AddRef: color_add_ref,
            Release: color_release,
        },
        GetIids: color_iids,
        GetRuntimeClassName: color_class_name,
        GetTrustLevel: color_trust_level,
    },
    value: color_value,
};

fn boxed_color(color: Color) -> IReference<Color> {
    let b = Box::new(ColorBox {
        vtable: &COLOR_VTABLE,
        refs: AtomicU32::new(1),
        color,
    });
    // SAFETY: a COM object laid out with its vtable first, one reference.
    unsafe { IReference::from_raw(Box::into_raw(b) as *mut c_void) }
}

unsafe fn color_box<'a>(p: *mut c_void) -> &'a ColorBox {
    unsafe { &*(p as *const ColorBox) }
}

unsafe extern "system" fn color_qi(
    p: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe {
        let iid = &*iid;
        if *iid == IUnknown::IID
            || *iid == IInspectable::IID
            || *iid == IAgileObject::IID
            || *iid == <IReference<Color> as Interface>::IID
        {
            color_box(p).refs.fetch_add(1, Ordering::Relaxed);
            *out = p;
            return HRESULT(0);
        }
        *out = core::ptr::null_mut();
        HRESULT(0x8000_4002_u32 as i32) // E_NOINTERFACE
    }
}

unsafe extern "system" fn color_add_ref(p: *mut c_void) -> u32 {
    unsafe { color_box(p).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn color_release(p: *mut c_void) -> u32 {
    unsafe {
        let left = color_box(p).refs.fetch_sub(1, Ordering::Release) - 1;
        if left == 0 {
            std::sync::atomic::fence(Ordering::Acquire);
            drop(Box::from_raw(p as *mut ColorBox));
        }
        left
    }
}

unsafe extern "system" fn color_value(p: *mut c_void, out: *mut Color) -> HRESULT {
    unsafe { *out = color_box(p).color };
    HRESULT(0)
}

unsafe extern "system" fn color_iids(
    _: *mut c_void,
    count: *mut u32,
    iids: *mut *mut GUID,
) -> HRESULT {
    unsafe {
        *count = 0;
        *iids = core::ptr::null_mut();
    }
    HRESULT(0)
}

unsafe extern "system" fn color_class_name(_: *mut c_void, name: *mut *mut c_void) -> HRESULT {
    unsafe { *name = core::ptr::null_mut() };
    HRESULT(0)
}

unsafe extern "system" fn color_trust_level(_: *mut c_void, level: *mut i32) -> HRESULT {
    unsafe { *level = 0 };
    HRESULT(0)
}
