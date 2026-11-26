//! Overlays (`sheet`, `dialog`, `popover`) the Windows 11 way.
//!
//! - **Sheets and dialogs** are a `ContentDialog`'s look over the window:
//!   the smoke (`SmokeFillColorDefault`, or the page blurred for
//!   `Backdrop::Blur`), a panel (`LayerFillColorAlt` on the base color, a
//!   1 px `SurfaceStrokeColorDefault` edge, 8 px corners, the dialog's deep
//!   shadow) holding the app's content, centered. They open and close like
//!   `ContentDialog` (scale from 1.05 and fade in; the reverse out), take
//!   the keyboard focus (Tab cycles inside) and give it back on closing;
//!   Escape and a click on the smoke close them unless `fixed`. A bare
//!   dialog has no panel, shadow or animation. Windows has no sheet: a
//!   sheet is a larger dialog (Settings' and the Store's modal pages).
//! - **Popovers** (and dialogs placed beside their anchor) are WinUI
//!   `Flyout`s on the overlay's parent: the acrylic presenter, its shadow,
//!   entry animation and placement (flipped without room), light dismiss,
//!   Escape, focus in and back are WinUI's. `fixed` and `manual` ones
//!   ignore a click outside; one that leaves the focus to its anchor is
//!   transient and lets the click through. A bare one (#287) has a clear
//!   presenter: no background, border, corners, shadow or animation.
//!
//! A popover or placed dialog anchored at a caret (#328) opens at the
//! caret of the text field in its parent (`GetRectFromCharacterIndex` of a
//! text box, the selection's end of a rich edit box), the flyout's
//! `Position` beside it and its rectangle the `ExclusionRect` WinUI keeps
//! clear (flipping without room). It is shown there again (WinUI moves an
//! open flyout) as the field's text, selection or scrolling changes and
//! after each layout.
//!
//! One anchored to a chip of a rich input (#327, `OverlayAnchor::Text`)
//! goes the same way against the chip's box in its rich edit box
//! (`ITextRange::GetRect`, kept inside the box), and against the field
//! once the chip is gone (`OverlayAnchor::Field`).
//!
//! The content node is laid out by prism-native in the area
//! `Widgets::overlay_viewport` gives; its canvas (`View::kids`) moves into
//! the panel or the flyout.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use prism_core::controls::{Backdrop, OverlayAnchor, OverlayConfig};
use prism_core::{Color, DismissReason, NodeId, OverlayKind, Transition, UiEvent};
use windows::Foundation::TypedEventHandler;
use windows_core::{IInspectable, Interface, Result};

use crate::fluent::Palette;
use crate::imp::{defer, dispatch};
use crate::motion::{self, Visual};
use crate::scene::{Node, Rect};
use crate::style::Shadows;
use crate::widgets::Win;
use crate::winui::xaml::Controls::Primitives::{
    FlyoutBase, FlyoutBaseClosingEventArgs, FlyoutPlacementMode, FlyoutShowMode, FlyoutShowOptions,
};
use crate::winui::xaml::Controls::{Border, Canvas, Flyout, RichEditBox, TextBox};
use crate::winui::xaml::Input::{FocusManager, KeyboardNavigationMode, TappedEventHandler};
use crate::winui::xaml::Markup::XamlReader;
use crate::winui::xaml::Media::{AcrylicBrush, Brush, VisualTreeHelper};
use crate::winui::xaml::{
    DependencyObject, FocusState, FrameworkElement, Style, UIElement, Visibility,
};
use crate::winui::{brush, h, radius, ui, uniform, win_color};

/// How an overlay shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// A panel over the smoke (sheets, dialogs).
    Panel,
    /// A WinUI flyout on the anchor (popovers, placed dialogs).
    Flyout,
}

fn mode(c: OverlayConfig) -> Option<Mode> {
    match c.kind {
        OverlayKind::Keyboard => None,
        _ if c.anchored_side().is_some() => Some(Mode::Flyout),
        _ => Some(Mode::Panel),
    }
}

/// Whether the overlay's own animations run (« Animation effects » on).
fn animates(w: &Win) -> bool {
    !w.env.reduce_motion
}

/// The dialog's colors: the smoke, the panel, its edge.
fn colors(pal: &Palette) -> (Color, Color, Color) {
    let c = prism_core::rgba;
    // A contrast theme's: the window's color, edged with its text's.
    if let Some(f) = pal.forced {
        return (c(0x000000, 0.3), f.sys.window, f.sys.window_text);
    }
    // `SmokeFillColorDefault`, `LayerFillColorAlt` over
    // `SolidBackgroundFillColorBase`, `SurfaceStrokeColorDefault`.
    if pal.dark {
        (c(0x000000, 0.3), c(0x2B2B2B, 1.), c(0x757575, 0.4))
    } else {
        (c(0x000000, 0.3), c(0xFFFFFF, 1.), c(0x757575, 0.4))
    }
}

/// `ContentDialog`'s motion: in, scaling down from 1.05 and fading in;
/// out, the reverse, faster.
const OPEN: Transition = Transition {
    duration: 0.25,
    curve: prism_core::Curve::EaseOut,
    delay: 0.,
};
const FADE: Transition = Transition {
    duration: 0.167,
    curve: prism_core::Curve::Linear,
    delay: 0.,
};
const CLOSE: Transition = Transition {
    duration: 0.167,
    curve: prism_core::Curve::EaseIn,
    delay: 0.,
};

/// Popovers keep their own state across the flyout's events.
#[derive(Default)]
struct FlyoutState {
    /// Shown (between `ShowAt` and `Closed`).
    shown: Cell<bool>,
    /// The app closes it: no dismissal to report, nothing to cancel.
    by_app: Cell<bool>,
    config: Cell<OverlayConfig>,
    /// Where the focus goes back when it closes: what had it before, else
    /// the anchor's first focusable element. WinUI's own choice, with
    /// nothing focused before (opened through UI Automation), is the
    /// page's first element, which scrolls the page back to its top.
    give_back: std::cell::RefCell<Option<UIElement>>,
}

pub struct Overlay {
    id: NodeId,
    /// The content's canvas (`View::kids`), where it is now.
    kids: Canvas,
    shown: Option<Mode>,
    // Panel.
    scrim: Border,
    card: Canvas,
    panel: Border,
    shadows: Shadows,
    closing: Option<Instant>,
    focus_before: Option<UIElement>,
    /// The content that left as it closed: shown until it has animated out.
    leaving: Vec<UIElement>,
    // Flyout.
    flyout: Option<Flyout>,
    host: Canvas,
    state: Rc<FlyoutState>,
    /// Where the flyout was shown: its anchor, the layout direction, and
    /// the caret (#328) or chip (#327) it was shown at.
    anchor: Option<UIElement>,
    rtl: bool,
    caret: Option<(f32, f32, f32, f32)>,
    /// The field it is anchored to instead of its parent (#327), when its
    /// config names one.
    pub field: Option<AnchorField>,
}

thread_local! {
    /// When Escape was last pressed (a flyout closed by it says so).
    static ESCAPE_AT: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Each popover's state, for its flyout's events.
    static STATES: std::cell::RefCell<std::collections::HashMap<NodeId, Rc<FlyoutState>>> =
        std::cell::RefCell::default();
}

fn state_of(id: NodeId) -> Option<Rc<FlyoutState>> {
    STATES.with(|s| s.borrow().get(&id).cloned())
}

fn escape_recent() -> bool {
    ESCAPE_AT
        .get()
        .is_some_and(|at| at.elapsed() < Duration::from_millis(500))
}

impl Overlay {
    /// The overlay's elements, in `layer` (above the window).
    pub fn new(id: NodeId, kids: &Canvas, layer: &Canvas) -> Result<Overlay> {
        let scrim = Border::new()?;
        scrim.SetVisibility(Visibility::Collapsed)?;
        scrim.Tapped(&TappedEventHandler::new(move |_, a| {
            if let Some(a) = a.as_ref() {
                a.SetHandled(true)?;
            }
            outside(id);
            Ok(())
        }))?;
        // A dialog window for UI Automation while shown (`a11y.rs`).
        let card = crate::peers::canvas(crate::peers::Key::Dialog(id))?;
        card.SetVisibility(Visibility::Collapsed)?;
        // Tab and the arrows stay inside while it is open.
        card.SetTabFocusNavigation(KeyboardNavigationMode::Cycle)?;
        let shadows = Shadows::new()?;
        let panel = Border::new()?;
        panel.SetIsHitTestVisible(false)?;
        card.Children()?.Append(&shadows.host)?;
        card.Children()?.Append(&panel)?;
        card.Children()?.Append(kids)?;
        let coll = layer.Children()?;
        coll.Append(&scrim)?;
        coll.Append(&card)?;
        kids.SetVisibility(Visibility::Collapsed)?;
        let state = Rc::<FlyoutState>::default();
        STATES.with(|s| s.borrow_mut().insert(id, state.clone()));
        Ok(Overlay {
            id,
            kids: kids.clone(),
            shown: None,
            scrim,
            card,
            panel,
            shadows,
            closing: None,
            focus_before: None,
            leaving: Vec::new(),
            flyout: None,
            host: Canvas::new()?,
            state,
            anchor: None,
            rtl: false,
            caret: None,
            field: None,
        })
    }

    /// Takes its elements out of the window.
    pub fn remove(&self) {
        STATES.with(|s| s.borrow_mut().remove(&self.id));
        crate::a11y::dialog_hidden(self.id);
        if let Some(f) = &self.flyout {
            self.state.by_app.set(true);
            let _ = f.Hide();
        }
        for e in [ui(&self.scrim), ui(&self.card)].into_iter().flatten() {
            let _ = crate::view::detach(&e);
        }
    }

    /// Moves the content's canvas into `to` (the card or the flyout's host).
    fn home(&self, to: &Canvas) -> Result<()> {
        let e: UIElement = self.kids.cast()?;
        let coll = to.Children()?;
        let mut at = 0u32;
        if !coll.IndexOf(&e, &mut at)? {
            crate::view::detach(&e)?;
            coll.Append(&e)?;
        }
        Ok(())
    }

    fn flyout(&mut self) -> Result<Flyout> {
        if let Some(f) = &self.flyout {
            return Ok(f.clone());
        }
        let f = Flyout::new()?;
        f.SetContent(&self.host)?;
        let base: FlyoutBase = f.cast()?;
        // In the window, like the other overlays (and in its snapshots).
        base.SetShouldConstrainToRootBounds(true)?;
        let id = self.id;
        base.Closing(
            &TypedEventHandler::<FlyoutBase, FlyoutBaseClosingEventArgs>::new(move |_, a| {
                let Some(state) = state_of(id) else {
                    return Ok(());
                };
                let c = state.config.get();
                let keep = !state.by_app.get() && (c.fixed || (c.manual && !escape_recent()));
                if keep && let Some(a) = a.as_ref() {
                    a.SetCancel(true)?;
                    return Ok(());
                }
                // The focus goes back before the flyout takes it away.
                if let Some(e) = state.give_back.borrow_mut().take() {
                    let _ = e.Focus(FocusState::Programmatic);
                }
                Ok(())
            }),
        )?;
        base.Closed(&windows::Foundation::EventHandler::<IInspectable>::new(
            move |_, _| {
                let Some(state) = state_of(id) else {
                    return Ok(());
                };
                state.shown.set(false);
                if !state.by_app.replace(false) {
                    let reason = if escape_recent() {
                        DismissReason::Escape
                    } else {
                        DismissReason::Outside
                    };
                    dismiss(id, reason);
                }
                Ok(())
            },
        ))?;
        self.flyout = Some(f.clone());
        Ok(f)
    }

    fn hide_flyout(&self) {
        if let Some(f) = &self.flyout
            && self.state.shown.get()
        {
            self.state.by_app.set(true);
            let _ = f.Hide();
        }
    }

    /// The content that left goes for good.
    fn drop_leaving(&mut self) {
        for e in self.leaving.drain(..) {
            let _ = crate::view::detach(&e);
        }
    }

    /// The panel shows (or stops showing) at once.
    fn panel_visible(&self, on: bool) -> Result<()> {
        let v = if on {
            Visibility::Visible
        } else {
            Visibility::Collapsed
        };
        self.scrim.SetVisibility(v)?;
        self.scrim.SetIsHitTestVisible(on)?;
        self.card.SetVisibility(v)?;
        self.card.SetIsHitTestVisible(on)?;
        self.kids.SetVisibility(v)
    }
}

/// `FlyoutPresenter` without its padding and size limits (the content is
/// laid out by Prism), in the app's theme (a flyout does not follow the
/// theme of the element it opens on); a bare one (#287) without its
/// background, border, corners or shadow either.
fn presenter_style(dark: bool, bare: bool) -> Option<Style> {
    const XAML: &str = r#"<Style xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" TargetType="FlyoutPresenter">
<Setter Property="RequestedTheme" Value="THEME"/>
<Setter Property="Padding" Value="0"/>
<Setter Property="MinWidth" Value="0"/>
<Setter Property="MinHeight" Value="0"/>
<Setter Property="MaxWidth" Value="100000"/>
<Setter Property="MaxHeight" Value="100000"/>
<Setter Property="ScrollViewer.HorizontalScrollMode" Value="Disabled"/>
<Setter Property="ScrollViewer.VerticalScrollMode" Value="Disabled"/>
<Setter Property="ScrollViewer.HorizontalScrollBarVisibility" Value="Disabled"/>
<Setter Property="ScrollViewer.VerticalScrollBarVisibility" Value="Disabled"/>
BARE</Style>"#;
    const BARE: &str = r#"<Setter Property="Background" Value="Transparent"/>
<Setter Property="BorderThickness" Value="0"/>
<Setter Property="CornerRadius" Value="0"/>
<Setter Property="IsDefaultShadowEnabled" Value="False"/>
"#;
    let load = |theme: &str, bare: bool| {
        let xaml = XAML
            .replace("THEME", theme)
            .replace("BARE", if bare { BARE } else { "" });
        XamlReader::Load(&h(&xaml))
            .and_then(|o| o.cast())
            .map_err(|e| crate::win::log!("flyout style: {e}"))
            .ok()
    };
    thread_local! {
        /// Light, dark, bare light, bare dark: loaded once each.
        static STYLES: std::cell::RefCell<[Option<Option<Style>>; 4]> =
            const { std::cell::RefCell::new([None, None, None, None]) };
    }
    STYLES.with(|s| {
        let i = usize::from(dark) + 2 * usize::from(bare);
        s.borrow_mut()[i]
            .get_or_insert_with(|| load(if dark { "Dark" } else { "Light" }, bare))
            .clone()
    })
}

/// A click on the smoke.
fn outside(id: NodeId) {
    defer(Box::new(move |s| {
        let fixed = s
            .renderer
            .widgets
            .scene
            .get(id)
            .is_some_and(|n| OverlayConfig::decode(&n.config).fixed);
        if !fixed {
            dismiss(id, DismissReason::Outside);
        }
    }));
}

/// The user closed overlay `id`: it is closed (like the app's copy of it,
/// which opens it again unless the app follows), and the app hears why.
fn dismiss(id: NodeId, reason: DismissReason) {
    defer(Box::new(move |s| {
        if let Some(n) = s.renderer.widgets.scene.get_mut(id) {
            n.value = 0.;
        }
    }));
    dispatch(UiEvent::Dismiss(id, reason));
}

/// Escape went down: closes the topmost panel (sheet or dialog) unless a
/// popup (a flyout, a combo box's list…) is open above it, which closes
/// on its own. Returns whether the key was used.
pub fn escape(w: &Win) -> bool {
    ESCAPE_AT.set(Some(Instant::now()));
    let Some(top) = w.scene.open_overlays().last().copied() else {
        return false;
    };
    let Some(o) = w.layers.get(&top) else {
        return false;
    };
    if o.shown != Some(Mode::Panel) {
        return false;
    }
    let popups = w
        .root
        .XamlRoot()
        .and_then(|r| VisualTreeHelper::GetOpenPopupsForXamlRoot(&r))
        .and_then(|p| p.Size())
        .unwrap_or(0);
    if popups > 0 {
        return false;
    }
    let fixed = w
        .scene
        .get(top)
        .is_some_and(|n| OverlayConfig::decode(&n.config).fixed);
    if fixed {
        return false;
    }
    dismiss(top, DismissReason::Escape);
    true
}

impl Win {
    /// Creates the elements of overlay `id` (`Widgets::create`).
    pub fn new_overlay(&mut self, id: NodeId) {
        let Some(v) = self.views.get(&id) else { return };
        match Overlay::new(id, &v.kids, &self.overlays) {
            Ok(o) => {
                self.layers.insert(id, o);
            }
            Err(e) => crate::win::log!("overlay {id}: {e}"),
        }
    }

    /// `Widgets::set_children` on overlay `id`: the content leaves as it
    /// closes (Prism drops a closed overlay's content); shown in a panel, it
    /// stays there, still, until the panel has animated out.
    pub fn overlay_children(&mut self, id: NodeId, children: &[NodeId]) {
        let shown = self
            .layers
            .get(&id)
            .is_some_and(|o| o.shown == Some(Mode::Panel) && animates(self));
        if !shown {
            return;
        }
        let leaving: Vec<NodeId> = self.views.get(&id).map_or_else(Vec::new, |v| {
            v.children
                .iter()
                .filter(|c| !children.contains(c))
                .copied()
                .collect()
        });
        for c in leaving {
            let Some(e) = self.outer(c) else { continue };
            self.keep_tree(c);
            let (Some(v), Some(o)) = (self.views.get_mut(&id), self.layers.get_mut(&id)) else {
                continue;
            };
            // Out of the children `View::set_children` sees, at the end of
            // the canvas.
            if let Some(i) = v.children.iter().position(|x| *x == c) {
                v.children.remove(i);
                if let Ok(coll) = v.kids.Children() {
                    let _ = coll.RemoveAt(i as u32);
                    let _ = coll.Append(&e);
                }
            }
            o.leaving.push(e);
        }
    }

    /// Overlay `id` is gone (`Widgets::destroy`).
    pub fn drop_overlay(&mut self, id: NodeId) {
        if let Some(o) = self.layers.remove(&id) {
            o.remove();
        }
    }

    /// After each layout: each overlay shown, hidden or moved as the app
    /// and its content's frame say.
    pub fn place_overlays(&mut self) {
        let open = self.scene.open_overlays();
        let ids: Vec<NodeId> = self.layers.keys().copied().collect();
        for id in ids {
            if let Err(e) = self.place_overlay(id, open.contains(&id)) {
                crate::win::log!("overlay {id}: {e}");
            }
        }
    }

    fn place_overlay(&mut self, id: NodeId, open: bool) -> Result<()> {
        let Some(n) = self.scene.get(id).cloned() else {
            return Ok(());
        };
        let config = OverlayConfig::decode(&n.config);
        let content = n.children.first().and_then(|c| self.scene.get(*c)).cloned();
        let want = mode(config).filter(|_| open && content.is_some());
        let animate = animates(self);
        let (size, pal, dark) = (self.size, self.palette, self.dark());
        let anchor = n.parent.and_then(|p| self.outer(p));
        let rtl = self.env.direction == prism_core::Direction::Rtl;
        let name = (want == Some(Mode::Panel))
            .then(|| crate::a11y::dialog_name(&self.scene, id))
            .flatten();
        let Some(o) = self.layers.get_mut(&id) else {
            return Ok(());
        };
        o.state.config.set(config);
        // Shown another way (or not at all): the old way goes.
        if o.shown.is_some() && o.shown != want {
            match o.shown {
                Some(Mode::Flyout) => o.hide_flyout(),
                Some(Mode::Panel) => {
                    crate::a11y::dialog_hidden(id);
                    close_panel(o, animate)?
                }
                None => {}
            }
            o.shown = None;
        }
        let (Some(mode), Some(c)) = (want, content) else {
            return Ok(());
        };
        let f = c.frame;
        match mode {
            Mode::Panel => {
                o.home(&o.card.clone())?;
                let opening = o.shown != Some(Mode::Panel) || o.closing.is_some();
                if opening {
                    o.drop_leaving();
                }
                o.closing = None;
                o.shown = Some(Mode::Panel);
                style_panel(o, &n, &c, config, size, &pal)?;
                o.panel_visible(true)?;
                crate::a11y::dialog_shown(id, &o.card, name.as_deref());
                if opening {
                    open_panel(o, config, animate)?;
                }
            }
            Mode::Flyout => {
                o.home(&o.host.clone())?;
                o.kids.SetVisibility(Visibility::Visible)?;
                o.host.SetWidth(f.w.max(1.) as f64)?;
                o.host.SetHeight(f.h.max(1.) as f64)?;
                let k: UIElement = o.kids.cast()?;
                Canvas::SetLeft(&k, -f.x as f64)?;
                Canvas::SetTop(&k, -f.y as f64)?;
                let flyout = o.flyout()?;
                o.shown = Some(Mode::Flyout);
                if !o.state.shown.get()
                    && f.w > 0.
                    && f.h > 0.
                    && let Some(anchor) = anchor
                    && anchor.XamlRoot().is_ok()
                {
                    let base: FlyoutBase = flyout.cast()?;
                    if let Some(s) = presenter_style(dark, config.bare) {
                        flyout.SetFlyoutPresenterStyle(&s)?;
                    }
                    // A bare one appears at once, like a bare dialog.
                    base.SetAreOpenCloseAnimationsEnabled(!config.bare)?;
                    let options = show_options(config, rtl)?;
                    let transient = config.keep_focus;
                    o.caret = at_anchor(&options, &anchor, config, o.field.as_ref())?;
                    o.anchor = Some(anchor.clone());
                    o.rtl = rtl;
                    base.SetAllowFocusOnInteraction(!config.keep_focus)?;
                    // Presses outside reach the page when it is not modal.
                    if config.keep_focus || config.manual {
                        base.SetOverlayInputPassThroughElement(&self.app)?;
                    }
                    o.state.by_app.set(false);
                    o.state.shown.set(true);
                    // A flyout that takes the focus gives it back.
                    let back = if transient {
                        None
                    } else {
                        anchor
                            .XamlRoot()
                            .ok()
                            .and_then(|r| FocusManager::GetFocusedElementWithRoot(&r).ok())
                            .and_then(|e| e.cast::<UIElement>().ok())
                            .or_else(|| {
                                FocusManager::FindFirstFocusableElement(&anchor)
                                    .ok()
                                    .and_then(|e| e.cast::<UIElement>().ok())
                            })
                    };
                    o.state.give_back.replace(back);
                    // Where the anchor is now (a scroll just asked for).
                    if let Ok(fe) = anchor.cast::<crate::winui::xaml::FrameworkElement>() {
                        let _ = fe.UpdateLayout();
                    }
                    base.ShowAtWithOptions(&anchor, &options)?;
                } else {
                    // At a caret (#328) or a chip (#327): where it moved.
                    o.follow_anchor()?;
                }
            }
        }
        Ok(())
    }

    /// Flyouts at a caret (#328) or a chip (#327) follow it.
    pub fn follow_anchors(&mut self) {
        for o in self.layers.values_mut() {
            if let Err(e) = o.follow_anchor() {
                crate::win::log!("overlay {}: following its anchor: {e}", o.id);
            }
        }
    }
}

impl Overlay {
    /// A flyout at a caret (#328) or a chip (#327) follows it: shown again
    /// at its new place, which moves the open flyout.
    fn follow_anchor(&mut self) -> Result<()> {
        let c = self.state.config.get();
        if c.anchor == OverlayAnchor::Parent
            || self.shown != Some(Mode::Flyout)
            || !self.state.shown.get()
        {
            return Ok(());
        }
        let (Some(anchor), Some(flyout)) = (self.anchor.clone(), self.flyout.clone()) else {
            return Ok(());
        };
        let options = show_options(c, self.rtl)?;
        let caret = at_anchor(&options, &anchor, c, self.field.as_ref())?;
        if caret == self.caret {
            return Ok(());
        }
        self.caret = caret;
        flyout
            .cast::<FlyoutBase>()?
            .ShowAtWithOptions(&anchor, &options)
    }
}

thread_local! {
    /// A caret moved: flyouts at a caret follow on the next turn.
    static CARET_DUE: Cell<bool> = const { Cell::new(false) };
}

/// A field's text, selection or scrolling changed: flyouts at a caret
/// (#328) or a chip (#327) follow it on the next turn, once XAML has laid
/// the field out.
pub fn caret_moved() {
    if CARET_DUE.replace(true) {
        return;
    }
    let queued =
        crate::winui::bindings::Microsoft::UI::Dispatching::DispatcherQueue::GetForCurrentThread()
            .and_then(|q| {
                q.TryEnqueue(
                &crate::winui::bindings::Microsoft::UI::Dispatching::DispatcherQueueHandler::new(
                    || {
                        CARET_DUE.set(false);
                        crate::imp::with_state(|s| s.renderer.widgets.follow_anchors());
                        Ok(())
                    },
                ),
            )
            });
    if !queued.unwrap_or(false) {
        CARET_DUE.set(false);
    }
}

/// The flyout's placement and show mode.
fn show_options(c: OverlayConfig, rtl: bool) -> Result<FlyoutShowOptions> {
    let options = FlyoutShowOptions::new()?;
    options.SetPlacement(placement(c, rtl))?;
    options.SetShowMode(if c.keep_focus {
        FlyoutShowMode::Transient
    } else {
        FlyoutShowMode::Standard
    })?;
    Ok(options)
}

/// At a caret (#328) or a chip (#327): the flyout opens at its edge on
/// its side (`Position`, in `anchor`'s coordinates), it kept clear
/// (`ExclusionRect`). Returns its rectangle, `None` when the overlay is
/// anchored to its parent or has no field.
fn at_anchor(
    options: &FlyoutShowOptions,
    anchor: &UIElement,
    c: OverlayConfig,
    field: Option<&AnchorField>,
) -> Result<Option<(f32, f32, f32, f32)>> {
    let rect = match c.anchor {
        OverlayAnchor::Parent => None,
        OverlayAnchor::Caret => caret_anchor(anchor),
        a => field.and_then(|f| field_anchor(anchor, f, a)),
    };
    let Some((x, y, w, h)) = rect else {
        return Ok(None);
    };
    use prism_core::PopoverSide as S;
    let (px, py) = match c.anchored_side().unwrap_or(S::Bottom) {
        S::Bottom => (x, y + h),
        S::Top => (x, y),
        // The placement mode puts it on the point's right or left.
        S::Trailing | S::Leading => (x + w / 2., y),
    };
    use windows::Foundation::{IReference, Point, PropertyValue, Rect};
    use windows_core::Interface as _;
    options.SetPosition(
        &PropertyValue::CreatePoint(Point { X: px, Y: py })?.cast::<IReference<Point>>()?,
    )?;
    let exclusion = Rect {
        X: x,
        Y: y,
        Width: w,
        Height: h,
    };
    options.SetExclusionRect(&PropertyValue::CreateRect(exclusion)?.cast::<IReference<Rect>>()?)?;
    Ok(Some((x, y, w, h)))
}

/// A field an overlay is anchored to (#327): its outer element, and its
/// rich edit box (a rich input's, which lays its chips out).
pub type AnchorField = (UIElement, Option<RichEditBox>);

/// A field anchor's box in `anchor`'s coordinates (#327): the characters
/// ([`OverlayAnchor::Text`], a chip's) in its rich edit box, kept inside
/// it; the whole field otherwise (also when they are not laid out).
fn field_anchor(
    anchor: &UIElement,
    (outer, rich): &AnchorField,
    a: OverlayAnchor,
) -> Option<(f32, f32, f32, f32)> {
    let in_anchor = |e: &UIElement, (x, y, w, h): (f32, f32, f32, f32)| {
        let p = e
            .TransformToVisual(anchor)
            .ok()?
            .TransformPoint(windows::Foundation::Point { X: x, Y: y })
            .ok()?;
        Some((p.X, p.Y, w, h))
    };
    if let (OverlayAnchor::Text { start, end, .. }, Some(r)) = (a, rich)
        && let Some(b) = crate::edit::chars_box(r, start, end)
        && let Ok(e) = r.cast::<UIElement>()
        && let Some(at) = in_anchor(&e, (b.x, b.y, b.width, b.height))
    {
        return Some(at);
    }
    let fe: FrameworkElement = outer.cast().ok()?;
    let (w, h) = (
        fe.ActualWidth().ok()? as f32,
        fe.ActualHeight().ok()? as f32,
    );
    in_anchor(outer, (0., 0., w, h))
}

/// The caret an overlay anchored at it goes against (#328), in `anchor`'s
/// coordinates: the caret of the text field inside `anchor` that has the
/// focus, kept inside the field; the whole first field there when none
/// has it; `None` without one.
fn caret_anchor(anchor: &UIElement) -> Option<(f32, f32, f32, f32)> {
    let size = |e: &UIElement| {
        let fe: FrameworkElement = e.cast().ok()?;
        Some((
            fe.ActualWidth().ok()? as f32,
            fe.ActualHeight().ok()? as f32,
        ))
    };
    let in_anchor = |e: &UIElement, (x, y, w, h): (f32, f32, f32, f32)| {
        let p = e
            .TransformToVisual(anchor)
            .ok()?
            .TransformPoint(windows::Foundation::Point { X: x, Y: y })
            .ok()?;
        Some((p.X, p.Y, w, h))
    };
    let root = anchor.XamlRoot().ok()?;
    let focused = FocusManager::GetFocusedElementWithRoot(&root)
        .ok()
        .and_then(|e| e.cast::<UIElement>().ok())
        .filter(|f| inside(f, anchor));
    if let Some(f) = focused {
        let caret = if let Ok(t) = f.cast::<TextBox>() {
            text_box_caret(&t)
        } else if let Ok(r) = f.cast::<RichEditBox>() {
            crate::edit::caret_rect(&r).ok()
        } else {
            None
        };
        if let (Some(caret), Some((w, h))) = (caret, size(&f)) {
            let r = prism_core::controls::caret_in_field(caret, (0., 0., w, h));
            return in_anchor(&f, r);
        }
    }
    let first = first_field(&anchor.cast().ok()?)?;
    let (w, h) = size(&first)?;
    in_anchor(&first, (0., 0., w, h))
}

/// A text box's caret (the selection's end), one point wide, in the box;
/// `None` while it is empty.
fn text_box_caret(t: &TextBox) -> Option<(f32, f32, f32, f32)> {
    let len = t.Text().ok()?.to_string().encode_utf16().count() as i32;
    let at = t.SelectionStart().ok()? + t.SelectionLength().ok()?;
    if len == 0 {
        return None;
    }
    let r = if at >= len {
        t.GetRectFromCharacterIndex(len - 1, true)
    } else {
        t.GetRectFromCharacterIndex(at.max(0), false)
    }
    .ok()?;
    Some((r.X, r.Y, 1., r.Height))
}

/// Whether `e` is `anchor` or inside it.
fn inside(e: &UIElement, anchor: &UIElement) -> bool {
    let Ok(target) = anchor.cast::<DependencyObject>() else {
        return false;
    };
    let mut at = e.cast::<DependencyObject>().ok();
    while let Some(n) = at {
        if n == target {
            return true;
        }
        at = VisualTreeHelper::GetParent(&n).ok();
    }
    false
}

/// The first text field (a text box or a rich edit box) inside `e`, in
/// tree order.
fn first_field(e: &DependencyObject) -> Option<UIElement> {
    let n = VisualTreeHelper::GetChildrenCount(e).ok()?;
    for i in 0..n {
        let Ok(c) = VisualTreeHelper::GetChild(e, i) else {
            continue;
        };
        if let Ok(u) = c.cast::<UIElement>()
            && u.Visibility().is_ok_and(|v| v == Visibility::Visible)
        {
            if u.cast::<TextBox>().is_ok() || u.cast::<RichEditBox>().is_ok() {
                return Some(u);
            }
            if let Some(f) = first_field(&c) {
                return Some(f);
            }
        }
    }
    None
}

/// The panel's place, size, look and the smoke.
fn style_panel(
    o: &mut Overlay,
    _n: &Node,
    c: &Node,
    config: OverlayConfig,
    (ww, wh): (f32, f32),
    pal: &Palette,
) -> Result<()> {
    let f = c.frame;
    let (smoke, fill, edge) = colors(pal);
    // The smoke over the whole window.
    let bg: Brush = match config.backdrop {
        Some(Backdrop::Clear) => crate::winui::transparent()?.cast()?,
        Some(Backdrop::Blur) => {
            let a = AcrylicBrush::new()?;
            a.SetTintColor(win_color(Color::BLACK))?;
            a.SetTintOpacity(smoke.a as f64)?;
            a.SetFallbackColor(win_color(smoke))?;
            a.cast()?
        }
        _ => brush(smoke)?.cast()?,
    };
    o.scrim.SetBackground(&bg)?;
    o.scrim.SetWidth(ww as f64)?;
    o.scrim.SetHeight(wh as f64)?;
    // The card: the panel's box, the content on it.
    // Centered in the window, like `ContentDialog` (the content is as
    // wide and as tall as it needs, within its area).
    let card: UIElement = o.card.cast()?;
    Canvas::SetLeft(&card, ((ww - f.w) / 2.).max(0.).round() as f64)?;
    Canvas::SetTop(&card, ((wh - f.h) / 2.).max(0.).round() as f64)?;
    o.card.SetWidth(f.w.max(0.) as f64)?;
    o.card.SetHeight(f.h.max(0.) as f64)?;
    let k: UIElement = o.kids.cast()?;
    Canvas::SetLeft(&k, -f.x as f64)?;
    Canvas::SetTop(&k, -f.y as f64)?;
    let r = 8.;
    let bare = config.bare;
    o.panel.SetVisibility(if bare {
        Visibility::Collapsed
    } else {
        Visibility::Visible
    })?;
    o.panel.SetWidth(f.w.max(0.) as f64)?;
    o.panel.SetHeight(f.h.max(0.) as f64)?;
    o.panel.SetBackground(&brush(fill)?)?;
    o.panel.SetBorderBrush(&brush(edge)?)?;
    o.panel.SetBorderThickness(uniform(1.))?;
    o.panel.SetCornerRadius(radius(r, r, r, r))?;
    // The dialog's elevation: a wide soft shadow and a close one.
    let mut s = prism_core::Style {
        corner_radius: r,
        ..Default::default()
    };
    if !bare {
        // Like `ContentDialog`'s `ThemeShadow` (Composition blurs wider
        // than CSS for the same radius).
        let (far, near) = if pal.dark { (0.5, 0.3) } else { (0.28, 0.14) };
        s.shadows = vec![
            prism_core::decoration::Shadow::new(0., 16., 32., Color::BLACK.with_alpha(far)),
            prism_core::decoration::Shadow::new(0., 2., 8., Color::BLACK.with_alpha(near)),
        ];
    }
    o.shadows.apply(&s, pal, Rect::new(0., 0., f.w, f.h))?;
    Ok(())
}

/// Shows the panel: the focus moves in, it scales and fades in.
fn open_panel(o: &mut Overlay, config: OverlayConfig, animate: bool) -> Result<()> {
    let card: UIElement = o.card.cast()?;
    let scrim: UIElement = o.scrim.cast()?;
    let (w, h) = (o.card.Width()? as f32, o.card.Height()? as f32);
    let rest = Visual {
        opacity: 1.,
        tx: 0.,
        ty: 0.,
        rotate: 0.,
        sx: 1.,
        sy: 1.,
    };
    motion::set_visual(&card, rest, w, h)?;
    motion::set_visual(&scrim, rest, 0., 0.)?;
    if animate && !config.bare {
        let faded = Visual {
            opacity: 0.,
            ..rest
        };
        let big = Visual {
            sx: 1.05,
            sy: 1.05,
            ..rest
        };
        motion::run(&scrim, faded, rest, FADE);
        motion::run(&card, faded, rest, FADE);
        motion::run(&card, big, rest, OPEN);
    }
    // The focus goes in once the content is laid out.
    let root = card.XamlRoot().ok();
    o.focus_before = root
        .as_ref()
        .and_then(|r| FocusManager::GetFocusedElementWithRoot(r).ok())
        .and_then(|e| e.cast::<UIElement>().ok());
    let target = o.card.clone();
    motion::after(
        Duration::from_millis(30),
        Box::new(move || {
            let first = FocusManager::FindFirstFocusableElement(&target)
                .ok()
                .and_then(|e| e.cast::<UIElement>().ok());
            if let Some(e) = first {
                let _ = e.Focus(FocusState::Programmatic);
            }
        }),
    );
    Ok(())
}

/// Hides the panel: it fades and scales out, then goes; the focus goes
/// back where it was.
fn close_panel(o: &mut Overlay, animate: bool) -> Result<()> {
    o.card.SetIsHitTestVisible(false)?;
    o.scrim.SetIsHitTestVisible(false)?;
    if let Some(e) = o.focus_before.take() {
        let _ = e.Focus(FocusState::Programmatic);
    }
    let bare = !o.panel.Visibility().is_ok_and(|v| v == Visibility::Visible);
    if !animate || bare {
        o.panel_visible(false)?;
        o.drop_leaving();
        return Ok(());
    }
    let card: UIElement = o.card.cast()?;
    let scrim: UIElement = o.scrim.cast()?;
    let rest = Visual {
        opacity: 1.,
        tx: 0.,
        ty: 0.,
        rotate: 0.,
        sx: 1.,
        sy: 1.,
    };
    let gone = Visual {
        opacity: 0.,
        ..rest
    };
    let big = Visual {
        sx: 1.05,
        sy: 1.05,
        ..rest
    };
    motion::run(&scrim, rest, gone, CLOSE);
    motion::run(&card, rest, gone, CLOSE);
    motion::run(&card, rest, big, CLOSE);
    let at = Instant::now();
    o.closing = Some(at);
    let id = o.id;
    motion::after(
        Duration::from_secs_f32(CLOSE.duration + 0.03),
        Box::new(move || {
            crate::imp::with_state(|s| {
                let Some(o) = s.renderer.widgets.layers.get_mut(&id) else {
                    return;
                };
                // Opened again meanwhile: it stays.
                if o.closing != Some(at) {
                    return;
                }
                o.closing = None;
                let _ = o.panel_visible(false);
                o.drop_leaving();
            });
        }),
    );
    Ok(())
}

/// Where the flyout opens: Prism's side and alignment (leading = left in
/// left-to-right layouts).
fn placement(c: OverlayConfig, rtl: bool) -> FlyoutPlacementMode {
    use FlyoutPlacementMode as P;
    use prism_core::{PopoverAlign as A, PopoverSide as S};
    let side = c.anchored_side().unwrap_or(S::Bottom);
    // Start and end along a horizontal edge, mirrored right to left.
    let (start, end) = if rtl {
        (A::End, A::Start)
    } else {
        (A::Start, A::End)
    };
    match side {
        S::Bottom | S::Top => {
            let bottom = side == S::Bottom;
            match c.align {
                a if a == start => {
                    if bottom {
                        P::BottomEdgeAlignedLeft
                    } else {
                        P::TopEdgeAlignedLeft
                    }
                }
                a if a == end => {
                    if bottom {
                        P::BottomEdgeAlignedRight
                    } else {
                        P::TopEdgeAlignedRight
                    }
                }
                _ => {
                    if bottom {
                        P::Bottom
                    } else {
                        P::Top
                    }
                }
            }
        }
        S::Trailing | S::Leading => {
            let right = side.is_right(rtl).unwrap_or(true);
            match (right, c.align) {
                (true, A::Start) => P::RightEdgeAlignedTop,
                (true, A::End) => P::RightEdgeAlignedBottom,
                (true, A::Center) => P::Right,
                (false, A::Start) => P::LeftEdgeAlignedTop,
                (false, A::End) => P::LeftEdgeAlignedBottom,
                (false, A::Center) => P::Left,
            }
        }
    }
}
