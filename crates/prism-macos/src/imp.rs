use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityButtonRole, NSAccessibilityCheckBoxRole,
    NSAccessibilityGroupRole, NSAccessibilityHeadingRole, NSAccessibilityImageRole,
    NSAccessibilityLinkRole, NSAccessibilityListRole, NSAccessibilityPopUpButtonRole,
    NSAccessibilityRadioButtonRole, NSAccessibilitySliderRole, NSAccessibilityStaticTextRole,
    NSAccessibilityTabGroupRole, NSAnimatablePropertyContainer, NSAppearance, NSApplication,
    NSApplicationActivationPolicy, NSBackingStoreType, NSBezierPath, NSButton, NSColor, NSControl,
    NSControlSize, NSControlStateValueOff, NSControlStateValueOn, NSControlTextEditingDelegate,
    NSDatePicker, NSDatePickerElementFlags, NSDatePickerStyle, NSEvent, NSFont,
    NSFontWeightSemibold, NSImage, NSImageSymbolConfiguration, NSImageView, NSMenu, NSMenuItem,
    NSPopUpButton, NSProgressIndicator, NSProgressIndicatorStyle, NSResponder, NSScrollView,
    NSSegmentedControl, NSSlider, NSSwitch, NSText, NSTextAlignment, NSTextDelegate, NSTextField,
    NSTextFieldDelegate, NSTextView, NSTextViewDelegate, NSUserInterfaceItemIdentification, NSView,
    NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_app_kit::{NSPopover, NSPopoverDelegate, NSTrackingArea, NSTrackingAreaOptions};
use objc2_foundation::{
    NSArray, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use prism_core::controls::{DateConfig, DateMode, InputConfig, PickerConfig, SliderConfig};
use prism_core::{
    Accessibility, AppConfig, AppInit, Appearance, Backend, Color, NodeId, NodeKind, Role, Runtime,
    Style, TextAlign, UiEvent,
};
use prism_core::{DragEvent, DragPhase, Point, PointerEvent, PointerPhase};

use crate::interact::{MacOverlay, MacRadios};
use prism_native::{Insets, NativeRenderer, Viewport, Widgets};

// ---------------------------------------------------------------------------
// Global (main-thread) state. AppKit callbacks re-enter Rust through here.
// ---------------------------------------------------------------------------

struct State {
    runtime: Runtime,
    renderer: NativeRenderer<AppKit>,
    services: crate::services::MacServices,
    /// The window size `device_info` was last reported for.
    device_size: Option<(f32, f32)>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state(f: impl FnOnce(&mut State)) {
    STATE.with(|cell| {
        // The platform can call back while we mutate views (layout passes,
        // navigation callbacks). We always re-layout after applying, so a
        // nested callback is safely skipped.
        if let Ok(mut guard) = cell.try_borrow_mut()
            && let Some(state) = guard.as_mut()
        {
            f(state)
        }
    });
}

/// A text field of a selection group (#231), as a drag looks for it: its
/// text view or field, and the view framing it.
pub(crate) enum GroupField {
    Area(Retained<PrismTextView>, Retained<NSView>),
    Input(Retained<NSTextField>, Retained<NSView>),
}

/// The text fields of the selection group text field `id` is in (#231),
/// in document order; `None` outside one.
pub(crate) fn group_fields(id: NodeId) -> Option<Vec<(NodeId, GroupField)>> {
    let mut out = None;
    with_state(|s| {
        let Some(group) = s.runtime.selection_group(id) else {
            return;
        };
        let fields = s.runtime.selection_group_fields(group);
        out = Some(
            fields
                .into_iter()
                .filter_map(|f| match s.renderer.view(f)? {
                    View::TextArea(t) => Some((
                        f,
                        GroupField::Area(t.text.clone(), Retained::into_super(t.container.clone())),
                    )),
                    View::Input(i) => Some((
                        f,
                        GroupField::Input(i.field(), Retained::into_super(i.host.clone())),
                    )),
                    _ => None,
                })
                .collect(),
        );
    });
    out
}

pub(crate) fn dispatch(event: UiEvent) {
    with_state(|s| s.dispatch(event));
}

/// Lays everything out again, measures included (a video's size became
/// known, #207).
pub(crate) fn relayout_all() {
    with_state(|s| {
        s.renderer.mark_all_dirty();
        s.relayout();
    });
}

/// Dispatches `event` now, or on the next turn of the run loop when Prism is
/// busy (the platform calls back while Prism applies patches: a scroll view
/// moved by `scroll_by`, #204), instead of dropping it.
pub(crate) fn dispatch_or_defer(event: UiEvent) {
    let busy = STATE.with(|cell| cell.try_borrow_mut().is_err());
    if busy {
        on_main_after(std::time::Duration::ZERO, move || dispatch(event));
    } else {
        dispatch(event);
    }
}

thread_local! {
    /// An autoscroll step is scheduled (#251).
    static AUTOSCROLL: Cell<bool> = const { Cell::new(false) };
}

/// While a dragged payload stays near a scroll view's edge (#251): a step
/// every frame, the drag reported again over what moved under it.
fn schedule_autoscroll() {
    if AUTOSCROLL.replace(true) {
        return;
    }
    on_main_after(prism_native::dnd::AUTOSCROLL_INTERVAL, || {
        AUTOSCROLL.set(false);
        with_state(|s| {
            if let Some((event, relayout)) = s.renderer.autoscroll() {
                if relayout {
                    s.relayout();
                }
                s.dispatch(event);
            }
        });
    });
}

/// Files came over a drop target or left it (#111).
pub(crate) fn report_drop_hover(id: NodeId, on: bool) {
    report_state(id, prism_core::Interaction::DropHover, on);
}

/// A node entered or left an interaction state (hover, press, focus):
/// restyled, and laid out again if a refinement changed its size.
fn report_state(id: NodeId, state: prism_core::Interaction, on: bool) {
    with_state(|s| {
        if s.renderer.set_interaction(id, state, on) {
            s.relayout();
        }
        // The app hears focus changes (#224).
        for e in s.renderer.take_focus_changes() {
            s.dispatch(e);
        }
    });
}

thread_local! {
    /// Keys whose press was reported, and to which node (#159): their
    /// release goes there too, and losing the key window releases them.
    static HELD_KEYS: RefCell<Vec<(String, Option<NodeId>)>> = const { RefCell::new(Vec::new()) };
}

fn hold_key(key: &prism_core::KeyEvent, focused: Option<NodeId>) {
    if !key.repeat {
        HELD_KEYS.with(|h| {
            let mut h = h.borrow_mut();
            h.retain(|(k, _)| *k != key.key);
            h.push((key.key.clone(), focused));
        });
    }
}

/// The node a held key was reported to (`None` inside: nothing focused),
/// or `None` if its press was not reported.
fn release_key(key: &str) -> Option<Option<NodeId>> {
    HELD_KEYS.with(|h| {
        let mut h = h.borrow_mut();
        let i = h.iter().position(|(k, _)| k == key)?;
        Some(h.remove(i).1)
    })
}

/// The window stopped being key: AppKit sends no key-up for the keys still
/// held, so they are released now (#159).
fn release_all_keys() {
    let held = HELD_KEYS.with(|h| std::mem::take(&mut *h.borrow_mut()));
    for (key, focused) in held {
        dispatch(UiEvent::KeyUp(focused, prism_core::KeyEvent::new(key)));
    }
}

/// What makes a Prism view focusable.
#[derive(Clone, Copy)]
struct FocusRole {
    clickable: bool,
    sheet_root: bool,
    keys: bool,
}

/// Whether a view takes the focus (#177): a clickable one only with Full
/// Keyboard Access or when the app asks (`keyboard`), like native buttons;
/// a sheet's root and a view with `on_key_down` always.
fn takes_focus(role: FocusRole, keyboard: bool) -> bool {
    (role.clickable && keyboard) || role.sheet_root || role.keys
}

/// Whether a focus came from a mouse press (then no ring, #177): not when
/// the app moves it.
fn focused_by_mouse(event: Option<objc2_app_kit::NSEventType>, app: bool) -> bool {
    use objc2_app_kit::NSEventType as T;
    !app && matches!(
        event,
        Some(T::LeftMouseDown | T::RightMouseDown | T::OtherMouseDown)
    )
}

thread_local! {
    /// `cx.focus` is moving the focus (#177): clickable views accept it
    /// without Full Keyboard Access, and show their ring.
    static APP_FOCUS: Cell<bool> = const { Cell::new(false) };
    /// The app has unsaved changes (`cx.set_unsaved_changes`, #158).
    static UNSAVED: Cell<bool> = const { Cell::new(false) };
}

/// « Leave without saving? » as a sheet on `window` (#158); `leave` runs if
/// the user discards the changes.
fn confirm_leave(window: &NSWindow, leave: impl Fn() + 'static) {
    let mtm = MainThreadMarker::new().expect("main thread");
    let mut texts = None;
    with_state(|s| {
        texts = Some(
            ["unsaved_title", "unsaved_message", "discard", "cancel"]
                .map(|k| s.runtime.app.chrome_text(k)),
        )
    });
    let [title, message, discard, cancel] = texts.unwrap_or_else(|| {
        [
            "Leave without saving?",
            "Your changes will be lost.",
            "Discard Changes",
            "Cancel",
        ]
        .map(String::from)
    });
    let alert = objc2_app_kit::NSAlert::new(mtm);
    alert.setAlertStyle(objc2_app_kit::NSAlertStyle::Warning);
    alert.setMessageText(&NSString::from_str(&title));
    alert.setInformativeText(&NSString::from_str(&message));
    alert
        .addButtonWithTitle(&NSString::from_str(&discard))
        .setHasDestructiveAction(true);
    alert
        .addButtonWithTitle(&NSString::from_str(&cancel))
        .setKeyEquivalent(&NSString::from_str("\u{1b}"));
    let handler = block2::RcBlock::new(move |response: isize| {
        // NSAlertFirstButtonReturn (1000): discard.
        if response == 1000 {
            UNSAVED.set(false);
            leave();
        }
    });
    alert.beginSheetModalForWindow_completionHandler(window, Some(&handler));
}

thread_local! {
    /// Why the closing popover closes (`popoverWillClose:`, #156).
    static POPOVER_CLOSE_REASON: Cell<prism_core::DismissReason> =
        const { Cell::new(prism_core::DismissReason::Unknown) };
}

thread_local! {
    /// The node with the keyboard focus, as last reported.
    static FOCUSED: Cell<Option<NodeId>> = const { Cell::new(None) };
}

/// The Prism node a first responder belongs to: a clickable Prism view, a
/// text area, a text input (through the field editor), a control.
fn focused_node(responder: Option<&NSResponder>) -> Option<NodeId> {
    let r = responder?;
    if let Some(v) = r.downcast_ref::<PrismView>() {
        return v.ivars().clickable.get().then(|| v.ivars().id.get());
    }
    if let Some(t) = r.downcast_ref::<PrismTextView>() {
        return Some(t.ivars().id.get());
    }
    if let Some(tv) = r.downcast_ref::<NSTextView>()
        && tv.isFieldEditor()
    {
        let delegate = tv.delegate()?;
        let obj: &AnyObject = delegate.as_ref();
        let field = obj.downcast_ref::<NSTextField>()?;
        return (field.tag() > 0).then(|| field.tag() as NodeId);
    }
    let control = r.downcast_ref::<NSControl>()?;
    (control.tag() > 0).then(|| control.tag() as NodeId)
}

/// The window's first responder changed: focus moves between nodes. Read
/// on the next turn: AppKit may be inside a Prism update, and a text
/// field hands the focus to its field editor right after taking it.
fn focus_changed(window: &NSWindow) {
    let window: Retained<NSWindow> = objc2::Message::retain(window);
    on_main_after(std::time::Duration::ZERO, move || {
        let responder = window.firstResponder();
        let id = focused_node(responder.as_deref());
        // A selection the app set while the input was not being edited
        // (#86): applied if the app focused it, else the user's click wins.
        if let Some(id) = id
            && let Some(tv) = responder.and_then(|r| r.downcast::<NSTextView>().ok())
        {
            let mut pending = None;
            with_state(|s| {
                let ix = &mut s.renderer.widgets.ix;
                let by_app = ix.app_focus == Some(id);
                if by_app {
                    ix.app_focus = None;
                }
                pending = ix.pending_selection.remove(&id).filter(|_| by_app);
            });
            if let Some((start, end)) = pending {
                crate::interact::select_text(&tv, start, end);
            }
        }
        let old = FOCUSED.with(|f| f.replace(id));
        if old == id {
            return;
        }
        if let Some(old) = old {
            report_state(old, prism_core::Interaction::Focus, false);
        }
        if let Some(id) = id {
            report_state(id, prism_core::Interaction::Focus, true);
        }
    });
}

/// Watches which view has the keyboard focus in `window`.
fn observe_focus(window: &NSWindow, target: &Target) {
    use objc2_foundation::{NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration};
    unsafe {
        window.addObserver_forKeyPath_options_context(
            target,
            &NSString::from_str("firstResponder"),
            NSKeyValueObservingOptions::New,
            std::ptr::null_mut(),
        )
    };
}

fn stop_observing_focus(window: &NSWindow, target: &Target) {
    use objc2_foundation::NSObjectNSKeyValueObserverRegistration;
    unsafe { window.removeObserver_forKeyPath(target, &NSString::from_str("firstResponder")) };
}

/// The pointer entered or left a tracking area of `set_interactions`.
fn hover_event(event: &NSEvent, on: bool) {
    let Some(area) = event.trackingArea() else {
        return;
    };
    with_state(|s| {
        let id = s
            .renderer
            .widgets
            .hover_areas
            .iter()
            .find(|(_, a)| ***a == *area)
            .map(|(id, _)| *id);
        if let Some(id) = id
            && s.renderer
                .set_interaction(id, prism_core::Interaction::Hover, on)
        {
            s.relayout();
        }
    });
}

/// Opens a deep link (notification or menu bar activity).
pub(crate) fn open_link(path: &str) {
    with_state(|s| {
        let patches = s.runtime.open_link(path);
        s.commit(patches);
    });
}

/// Pops one screen: from a specific stack's Back button, or (⌘[) from the
/// innermost stack that can go back.
fn go_back(stack: Option<NodeId>) {
    with_state(|s| {
        let stacks = &s.renderer.widgets.stacks;
        let found = match stack {
            Some(id) => stacks.iter().find(|st| st.id == id),
            None => stacks.iter().rev().find(|st| st.screens.borrow().len() > 1),
        };
        if let Some(st) = found {
            let (id, depth) = (st.id, st.screens.borrow().len());
            s.dispatch(UiEvent::Back(id, depth.saturating_sub(1)));
        }
    });
}

impl State {
    fn dispatch(&mut self, event: UiEvent) {
        // A draggable's drag, with the drop target under the pointer (#251).
        let (event, relayout) = self.renderer.locate_drag(event);
        if relayout {
            self.relayout();
        }
        let patches = self.runtime.dispatch(event);
        self.commit(patches);
        if self.renderer.autoscroll_due() {
            schedule_autoscroll();
        }
    }

    /// Applies patches, lays out, and hands platform requests to the OS.
    fn commit(&mut self, patches: Vec<prism_core::Patch>) {
        if !patches.is_empty() {
            self.renderer.apply(&patches);
            self.relayout();
        }
        for request in self.runtime.take_requests() {
            if let prism_core::PlatformRequest::LoadFont(face) = request {
                self.load_font(face);
                continue;
            }
            if let prism_core::PlatformRequest::SetTitle(title) = &request {
                self.renderer.widgets.set_app_title(title.clone());
            }
            // Unsaved changes (#158): the close button's dot; leaving asks.
            if let prism_core::PlatformRequest::SetUnsavedChanges(on) = &request {
                UNSAVED.set(*on);
                if let Some(window) = self.renderer.widgets.root.window() {
                    window.setDocumentEdited(*on);
                }
            }
            prism_core::PlatformServices::handle(&mut self.services, request);
        }
        schedule_timer(self.runtime.next_timer());
        self.sync_ticker();
    }

    /// Ticks countdowns (timer labels, menu bar countdowns) every second,
    /// only while one is shown: otherwise the app never wakes up for them.
    fn sync_ticker(&self) {
        let needed = !self.renderer.widgets.timers.is_empty() || self.services.has_countdown();
        TICKER.with(|t| {
            let mut t = t.borrow_mut();
            if needed && t.is_none() {
                *t = Some(unsafe {
                    objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                        1.0,
                        &self.renderer.widgets.target,
                        sel!(onTick:),
                        None,
                        true,
                    )
                });
            } else if !needed && let Some(timer) = t.take() {
                timer.invalidate();
            }
        });
    }

    /// Registers a run-time font (`cx.load_font`) with Core Text and draws
    /// the texts again. A URL comes back here with its bytes once loaded.
    fn load_font(&mut self, face: prism_core::FontFace) {
        let sources = self.renderer.widgets.image_sources.clone();
        if let Some(font) = prism_native::fonts::prepare(&mut self.runtime.app, face, &sources) {
            if !prism_native::fonts::register_apple_font_data(&font.bytes) {
                eprintln!(
                    "prism: font {:?}: already registered or refused",
                    font.family
                );
            }
            self.renderer.restyle_all();
            self.relayout();
        }
    }

    /// Full screen and the traffic lights (#67): `cx.is_fullscreen()`,
    /// `cx.window_controls()`, which is `None` while a tab bar or a
    /// navigation header along the top edge is under them (it makes room
    /// for them itself). Returns whether the app rendered again.
    fn sync_window_chrome(&mut self) -> bool {
        let w = &self.renderer.widgets;
        let Some(window) = w.root.window() else {
            return false;
        };
        let fullscreen = crate::chrome::is_fullscreen();
        let controls = crate::chrome::over_app(&window, w.top_bars());
        if (fullscreen, controls)
            == (
                self.runtime.app.is_fullscreen(),
                self.runtime.app.window_controls(),
            )
        {
            return false;
        }
        let patches = self.runtime.set_window_chrome(fullscreen, controls);
        if patches.is_empty() {
            return false;
        }
        self.renderer.apply(&patches);
        true
    }

    fn relayout(&mut self) {
        let size = self.renderer.widgets.root.frame().size;
        // Responsive layouts: tell the app its window size and what covers
        // it (a full-size content view's title bar and toolbar, #28):
        // re-renders when it crosses a breakpoint or the insets move.
        let (w, h) = (size.width as f32, size.height as f32);
        let i = self.renderer.widgets.root.safeAreaInsets();
        // A custom chrome (#67) lays the content out from the window's top
        // edge: the app draws its own title bar there.
        let custom = crate::chrome::chrome().is_custom();
        let safe = prism_core::Edges {
            top: if custom { 0. } else { i.top as f32 },
            right: i.right as f32,
            bottom: i.bottom as f32,
            left: i.left as f32,
        };
        if w > 0.
            && ((w, h) != self.runtime.app.window_size() || safe != self.runtime.app.safe_area())
        {
            let patches = self.runtime.set_window(w, h, safe);
            if !patches.is_empty() {
                self.renderer.apply(&patches);
            }
        }
        self.sync_window_chrome();
        // Fullscreen, another screen: `cx.device_info()`.
        if w > 0.
            && self.device_size != Some((w, h))
            && let Some(window) = self.renderer.widgets.root.window()
        {
            self.device_size = Some((w, h));
            let patches = self
                .runtime
                .set_device_info(crate::device_info::current(&window));
            if !patches.is_empty() {
                self.renderer.apply(&patches);
            }
        }

        let insets = Insets {
            top: safe.top,
            right: safe.right,
            bottom: safe.bottom,
            left: safe.left,
        };
        self.renderer
            .layout(size.width as f32, size.height as f32, insets);
        // The bars' frames are known: where the traffic lights are over
        // the app's own views follows them.
        self.renderer.widgets.place_bars();
        if self.sync_window_chrome() {
            self.renderer
                .layout(size.width as f32, size.height as f32, insets);
            self.renderer.widgets.place_bars();
        }
        let w = &self.renderer.widgets;
        if let Some(main) = w.root.window() {
            w.ix.follow(&main);
        }
        // Elements that report their frame and moved (#192).
        let more = prism_native::report_layout(&mut self.runtime, &mut self.renderer);
        if !more.is_empty() {
            self.commit(more);
        }
        // A new window size can show or hide a countdown.
        self.sync_ticker();
    }
}

thread_local! {
    /// The countdowns' ticker (`State::sync_ticker`).
    static TICKER: RefCell<Option<Retained<objc2_foundation::NSTimer>>> = const { RefCell::new(None) };
}

// ---------------------------------------------------------------------------
// PrismView: a flipped NSView that paints background/border and handles clicks.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct ViewIvars {
    pub(crate) id: Cell<NodeId>,
    clickable: Cell<bool>,
    background: RefCell<Option<Retained<NSColor>>>,
    border_color: RefCell<Option<Retained<NSColor>>>,
    radius: Cell<f64>,
    /// The corners `radius` rounds (leading = left).
    corners: Cell<prism_core::Corners>,
    border: Cell<f64>,
    /// Context menu (right-click, ⌃-click, VoiceOver "show menu").
    menu: RefCell<Option<prism_core::menu::MenuSpec>>,
    /// Root of a sheet: Escape dismisses it.
    pub(crate) sheet_root: Cell<bool>,
    /// Picture content, drawn aspect-fill (or aspect-fit).
    image: RefCell<Option<Retained<NSImage>>>,
    fit: Cell<bool>,
    /// A filled picture's focal point (#218); `None`: the center.
    focus: Cell<Option<(f32, f32)>>,
    /// Trackpad swipes (left, right) reported to Prism.
    swipe: Cell<(bool, bool)>,
    /// What a shape draws, in points, and the layers drawing it.
    shape: RefCell<Vec<prism_core::Path>>,
    shape_layers: RefCell<Vec<Retained<objc2_quartz_core::CAShapeLayer>>>,
    /// Background and border when only some corners are rounded (a path:
    /// `maskedCorners` is skipped by `renderInContext`, so snapshots and
    /// the screen would differ).
    box_layer: RefCell<Option<Retained<objc2_quartz_core::CAShapeLayer>>>,
    /// Shadows, gradient, dashed or per-side borders, clipping, blur and
    /// cursor (see `decor.rs`).
    pub(crate) decor: RefCell<crate::decor::DecorState>,
    /// Reports presses (`.active(|s| …)` refinements).
    report_active: Cell<bool>,
    /// It took the focus from a mouse press: no focus ring (#177).
    mouse_focus: Cell<bool>,
    // Pointer, keys, overlays, layers (#17, #19, #25).
    /// A layer's root: clicks where it has no child go to the windows below.
    pub(crate) pass_through: Cell<bool>,
    /// An overlay's host that Escape does not close.
    pub(crate) overlay_fixed: Cell<bool>,
    /// `prism_core::events::pointer_mask` bits.
    pub(crate) pointer_mask: Cell<u8>,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    /// Where the mouse went down (window coordinates), while pressed.
    press: Cell<Option<NSPoint>>,
    /// A press on a region that moves the window (#67), until the mouse
    /// moves: the window drag starts then, from it.
    window_press: RefCell<Option<Retained<NSEvent>>>,
    dragging: Cell<bool>,
    /// The button and keys of the press being dragged (#136).
    drag_button: Cell<prism_core::PointerButton>,
    drag_modifiers: Cell<prism_core::Modifiers>,
    /// A middle or secondary press that may become a drag (#136): where.
    other_press: Cell<Option<NSPoint>>,
    /// Reports keys while focused (it can take the focus).
    pub(crate) key_local: Cell<bool>,
    // --- Custom window chrome (#67) ---
    /// `.window_drag()`: a press moves the window, a double click zooms it.
    window_drag: Cell<bool>,
    /// A tab bar's or a navigation header's bar: along the window's top
    /// edge, with a custom chrome, it moves the window like a title bar.
    top_bar: Cell<bool>,
    // --- Drop targets (#111) ---
    /// The file types it accepts dropped (`None`: not a drop target).
    drop_types: RefCell<Option<Vec<String>>>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismView"]
    #[ivars = ViewIvars]
    pub struct PrismView;

    impl PrismView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        /// Backgrounds, borders, corners and pictures are layer properties
        /// composited by the GPU: no per-view bitmap to draw or keep.
        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            true
        }

        /// Light / dark (#36): every layer color here is a plain `CGColor`
        /// (borders, shadows, gradients, shapes): `updateLayer` resolves
        /// them again within the new appearance.
        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn view_did_change_effective_appearance(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidChangeEffectiveAppearance] };
            self.setNeedsDisplay(true);
            // The title bar is laid out again in the new appearance: the
            // custom traffic lights (#67) take their place back after it.
            if let Some(window) = self.window()
                && window.contentView().is_some_and(|c| *c == *self.as_super())
            {
                on_main_after(std::time::Duration::ZERO, move || {
                    crate::chrome::place_traffic_lights(&window);
                });
            }
        }

        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            let Some(layer) = self.layer() else { return };
            let iv = self.ivars();
            // Called within the view's appearance: dynamic colors resolve to
            // light or dark here.
            let cg = |c: &NSColor| c.CGColor();
            // Under a backdrop blur, the background is the blur's tint.
            let blurs = iv.decor.borrow().blurs();
            let background = iv.background.borrow().clone().filter(|_| !blurs);
            layer.setBackgroundColor(background.as_deref().map(cg).as_deref());
            // A radius over half the size draws garbage: clamp (pills).
            let b = self.bounds().size;
            let radius = iv.radius.get().min(b.width.min(b.height) / 2.).max(0.);
            // Dashed and per-side borders are shape layers (decor.rs).
            let custom_border = iv.decor.borrow().decor.custom_border();
            let bw = if iv.border_color.borrow().is_some() && !custom_border {
                iv.border.get()
            } else {
                0.
            };
            if radius > 0. && iv.corners.get() != prism_core::Corners::ALL {
                self.update_box_layer(&layer, radius, bw);
            } else {
                if let Some(old) = iv.box_layer.borrow_mut().take() {
                    old.removeFromSuperlayer();
                }
                layer.setCornerRadius(radius);
                layer.setBorderWidth(bw);
                layer.setBorderColor(iv.border_color.borrow().as_deref().map(cg).as_deref());
            }
            let image = iv.image.borrow();
            unsafe {
                // A CGImage: what Core Animation composites directly.
                let cg = image.as_deref().and_then(|i| {
                    i.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None)
                });
                layer.setContents(cg.as_deref().map(|c| {
                    let any: &AnyObject = &*(c as *const _ as *const AnyObject);
                    any
                }));
                // A focal point (#218): the part of the image that covers
                // the frame, stretched over it (contents go up from the
                // bottom unless the layer is flipped).
                let crop = match (iv.fit.get(), iv.focus.get(), image.as_deref()) {
                    (false, Some(at), Some(img)) => {
                        let (s, b) = (img.size(), self.bounds().size);
                        let (x, y, w, h) = prism_core::cover_crop(
                            (s.width as f32, s.height as f32),
                            (b.width as f32, b.height as f32),
                            at,
                        );
                        let y = if crate::decor::is_flipped(&layer) { y } else { 1. - y - h };
                        Some(NSRect::new(
                            NSPoint::new(x as f64, y as f64),
                            NSSize::new(w as f64, h as f64),
                        ))
                    }
                    _ => None,
                };
                layer.setContentsRect(crop.unwrap_or(NSRect::new(NSPoint::ZERO, NSSize::new(1., 1.))));
                layer.setContentsGravity(if iv.fit.get() {
                    objc2_quartz_core::kCAGravityResizeAspect
                } else if crop.is_some() {
                    objc2_quartz_core::kCAGravityResize
                } else {
                    objc2_quartz_core::kCAGravityResizeAspectFill
                });
            }
            // Pictures are cropped to their rounded frame; `clip` crops the
            // children (per-corner boxes: `maskedCorners`).
            let clip = iv.decor.borrow().decor.clip;
            layer.setMasksToBounds(image.is_some() || clip);
            let flipped = crate::decor::is_flipped(&layer);
            if clip {
                crate::decor::round(&layer, radius, iv.corners.get(), flipped);
            } else if iv.corners.get() == prism_core::Corners::ALL {
                crate::decor::round(&layer, radius, prism_core::Corners::ALL, flipped);
            }
            drop(image);
            self.update_shape_layers(&layer);
            let tint = iv.background.borrow().clone().filter(|_| blurs);
            crate::decor::update(
                self,
                &layer,
                &mut iv.decor.borrow_mut(),
                radius,
                iv.corners.get(),
                tint.as_deref(),
            );
        }

        /// Cursor: the style's, or a pointing hand on clickable views.
        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            let iv = self.ivars();
            let cursor = iv.decor.borrow().decor.cursor.or(
                iv.clickable.get().then_some(prism_core::Cursor::Pointer),
            );
            if let Some(c) = cursor {
                self.addCursorRect_cursor(self.bounds(), &crate::decor::ns_cursor(c));
            }
        }

        /// Outer shadows follow the view into its new superview (or go).
        #[unsafe(method(viewDidMoveToSuperview))]
        fn view_did_move_to_superview(&self) {
            crate::decor::sync_shadow_host(self, &self.ivars().decor.borrow());
        }

        /// Files dragged over a drop target (#111): accepted ones highlight
        /// it (`drop_hover`) and are copied.
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, info: &AnyObject) -> objc2_app_kit::NSDragOperation {
            let types = self.ivars().drop_types.borrow().clone();
            match types {
                // SAFETY: AppKit's dragging info, alive for the call.
                Some(t) if unsafe { prism_native::apple::drops::accepts(info as *const _ as _, &t) } => {
                    report_state(self.ivars().id.get(), prism_core::Interaction::DropHover, true);
                    objc2_app_kit::NSDragOperation::Copy
                }
                _ => objc2_app_kit::NSDragOperation::None,
            }
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _info: Option<&AnyObject>) {
            report_state(self.ivars().id.get(), prism_core::Interaction::DropHover, false);
        }

        #[unsafe(method(draggingEnded:))]
        fn dragging_ended(&self, _info: Option<&AnyObject>) {
            report_state(self.ivars().id.get(), prism_core::Interaction::DropHover, false);
        }

        #[unsafe(method(prepareForDragOperation:))]
        fn prepare_for_drag_operation(&self, _info: &AnyObject) -> bool {
            self.ivars().drop_types.borrow().is_some()
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(&self, info: &AnyObject) -> bool {
            let id = self.ivars().id.get();
            report_state(id, prism_core::Interaction::DropHover, false);
            let types = self.ivars().drop_types.borrow().clone();
            // SAFETY: AppKit's dragging info, alive for the call.
            types.is_some_and(|t| unsafe {
                prism_native::apple::drops::perform(id, info as *const _ as _, &t)
            })
        }

        /// Three-finger (or two-finger, per System Settings) swipes.
        #[unsafe(method(swipeWithEvent:))]
        fn swipe_with_event(&self, event: &NSEvent) {
            let (left, right) = self.ivars().swipe.get();
            let dx = event.deltaX();
            let dir = if dx > 0. && left {
                Some(prism_core::SwipeDirection::Left)
            } else if dx < 0. && right {
                Some(prism_core::SwipeDirection::Right)
            } else {
                None
            };
            match dir {
                Some(dir) => dispatch(UiEvent::Swipe(self.ivars().id.get(), dir)),
                None => unsafe { msg_send![super(self), swipeWithEvent: event] },
            }
        }

        /// A table's cell (#284), see `table.rs`.
        #[unsafe(method_id(accessibilityCellForColumn:row:))]
        fn accessibility_cell(&self, column: isize, row: isize) -> Option<Retained<AnyObject>> {
            crate::table::cell_at(self, column, row)
        }

        /// A table cell holding a control (#284) has no label of its own:
        /// VoiceOver reads what it holds ("Track, button"), not its text.
        #[unsafe(method_id(accessibilityLabel))]
        fn accessibility_label(&self) -> Option<Retained<NSString>> {
            if self.is_cell() && !crate::table::controls(self).is_empty() {
                None
            } else {
                unsafe { msg_send![super(self), accessibilityLabel] }
            }
        }

        /// VoiceOver "press" on a clickable view, or on a table cell (#284).
        #[unsafe(method(accessibilityPerformPress))]
        fn accessibility_perform_press(&self) -> objc2::runtime::Bool {
            if self.ivars().clickable.get() {
                dispatch(UiEvent::Click(self.ivars().id.get()));
                return objc2::runtime::Bool::YES;
            }
            objc2::runtime::Bool::new(self.is_cell() && self.press_cell())
        }

        // Like native buttons (#177): clickable views take the focus (a
        // click included) only with Full Keyboard Access, then show a ring
        // and activate with Space or Return; `cx.focus` still focuses them.
        // Views with `on_key_down` always take it.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            let iv = self.ivars();
            takes_focus(
                FocusRole {
                    clickable: iv.clickable.get(),
                    sheet_root: iv.sheet_root.get(),
                    keys: iv.key_local.get(),
                },
                APP_FOCUS.get()
                    || NSApplication::sharedApplication(self.mtm()).isFullKeyboardAccessEnabled(),
            ) && !crate::interact::in_inert(self)
        }

        /// Focused by a mouse press: no ring (#177), like native controls;
        /// Tab and `cx.focus` show it.
        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            let event = NSApplication::sharedApplication(self.mtm())
                .currentEvent()
                .map(|e| e.r#type());
            self.ivars()
                .mouse_focus
                .set(focused_by_mouse(event, APP_FOCUS.get()));
            self.noteFocusRingMaskChanged();
            true
        }

        /// `hit_test(false)` and `inert` subtrees let clicks through; a
        /// layer's root only takes them on its children.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            crate::interact::hit(self, point)
        }

        /// Hover over the part drawn outside its parent (#213).
        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            let own = self.ivars().tracking.borrow().clone();
            if let Some(new) = crate::interact::refresh_tracking(self, own.as_deref()) {
                *self.ivars().tracking.borrow_mut() = Some(new);
            }
            unsafe { msg_send![super(self), updateTrackingAreas] }
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            self.pointer(PointerPhase::Enter, event);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, event: &NSEvent) {
            self.pointer(PointerPhase::Leave, event);
        }

        /// Wheel and trackpad scrolling (#133): the view takes them when it
        /// listens; otherwise they go on to the scroll view around it.
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            use prism_core::events::pointer_mask::WHEEL;
            if self.ivars().pointer_mask.get() & WHEEL == 0 {
                return unsafe { msg_send![super(self), scrollWheel: event] };
            }
            let Some(phase) = gesture_phase(event.phase(), event.momentumPhase()) else {
                return;
            };
            // Coarse wheels count lines: 16 points each, like browsers.
            let precise = event.hasPreciseScrollingDeltas();
            let k = if precise { 1. } else { 16. };
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            dispatch(UiEvent::Wheel(
                self.ivars().id.get(),
                prism_core::WheelEvent {
                    // AppKit's deltas move the content; Prism's scroll it
                    // (positive y: toward the end), like the web.
                    delta: Point::new(
                        (-event.scrollingDeltaX() * k) as f32,
                        (-event.scrollingDeltaY() * k) as f32,
                    ),
                    position: Point::new(p.x as f32, p.y as f32),
                    modifiers: crate::interact::modifiers(event.modifierFlags()),
                    phase,
                    precise,
                },
            ));
        }

        /// Trackpad pinches (#133).
        #[unsafe(method(magnifyWithEvent:))]
        fn magnify_with_event(&self, event: &NSEvent) {
            use prism_core::events::pointer_mask::MAGNIFY;
            if self.ivars().pointer_mask.get() & MAGNIFY == 0 {
                return unsafe { msg_send![super(self), magnifyWithEvent: event] };
            }
            let Some(phase) = gesture_phase(event.phase(), objc2_app_kit::NSEventPhase::None)
            else {
                return;
            };
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            dispatch(UiEvent::Magnify(
                self.ivars().id.get(),
                prism_core::MagnifyEvent {
                    scale: (1. + event.magnification()).max(0.01) as f32,
                    position: Point::new(p.x as f32, p.y as f32),
                    phase,
                },
            ));
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            if self.ivars().pointer_mask.get() & prism_core::events::pointer_mask::MOVE != 0 {
                self.pointer(PointerPhase::Move, event);
            } else {
                unsafe { msg_send![super(self), mouseMoved: event] }
            }
        }

        /// Past 3 points the press becomes a drag (no click follows).
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let iv = self.ivars();
            if let Some(press) = iv.window_press.take() {
                if let Some(window) = self.window() {
                    window.performWindowDragWithEvent(&press);
                }
                return;
            }
            let drags = iv.pointer_mask.get() & prism_core::events::pointer_mask::DRAG != 0;
            if iv.pointer_mask.get() & prism_core::events::pointer_mask::MOVE != 0 {
                self.pointer(PointerPhase::Move, event);
            }
            let Some(start) = iv.press.get().filter(|_| drags) else {
                if !iv.clickable.get() {
                    unsafe { msg_send![super(self), mouseDragged: event] }
                }
                return;
            };
            let at = event.locationInWindow();
            let (dx, dy) = (at.x - start.x, start.y - at.y);
            // The keys held now (#223), not only at the press.
            iv.drag_modifiers
                .set(crate::interact::modifiers(event.modifierFlags()));
            if !iv.dragging.get() {
                if dx.hypot(dy) < 3. {
                    return;
                }
                iv.dragging.set(true);
                self.drag(DragPhase::Start, start, dx, dy);
            } else {
                self.drag(DragPhase::Move, start, dx, dy);
            }
        }

        /// Middle (wheel) and other buttons (#136): a drag when the view
        /// lets the middle button drag.
        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            if !self.button_down(event, button_of(event)) {
                unsafe { msg_send![super(self), otherMouseDown: event] }
            }
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            if !self.button_dragged(event) {
                unsafe { msg_send![super(self), otherMouseDragged: event] }
            }
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            if self.button_up(event).is_none() {
                unsafe { msg_send![super(self), otherMouseUp: event] }
            }
        }

        /// The secondary button (#136): a drag when the view lets it drag;
        /// a press that does not move still opens the context menu.
        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            if !self.button_down(event, prism_core::PointerButton::Secondary) {
                unsafe { msg_send![super(self), rightMouseDown: event] }
            }
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            if !self.button_dragged(event) {
                unsafe { msg_send![super(self), rightMouseDragged: event] }
            }
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            match self.button_up(event) {
                None => unsafe { msg_send![super(self), rightMouseUp: event] },
                // Not dragged: the menu it held back.
                Some(false) => {
                    if let Some(menu) = self.context_menu() {
                        NSMenu::popUpContextMenu_withEvent_forView(&menu, event, self);
                    }
                }
                Some(true) => {}
            }
        }

        /// Right-click / ⌃-click.
        #[unsafe(method(menuForEvent:))]
        fn menu_for_event(&self, _event: &NSEvent) -> *mut NSMenu {
            match self.context_menu() {
                Some(menu) => Retained::autorelease_return(menu),
                None => std::ptr::null_mut(),
            }
        }

        /// VoiceOver / Full Keyboard Access "show menu".
        #[unsafe(method(accessibilityPerformShowMenu))]
        fn accessibility_perform_show_menu(&self) -> objc2::runtime::Bool {
            let Some(menu) = self.context_menu() else { return objc2::runtime::Bool::NO };
            let at = NSPoint::new(8., self.bounds().size.height.min(24.));
            menu.popUpMenuPositioningItem_atLocation_inView(None, at, Some(self));
            objc2::runtime::Bool::YES
        }

        /// Escape in a sheet.
        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, sender: Option<&AnyObject>) {
            if self.ivars().overlay_fixed.get() {
                return;
            }
            if self.ivars().sheet_root.get() {
                dispatch(UiEvent::Dismiss(
                    self.ivars().id.get(),
                    prism_core::DismissReason::Escape,
                ));
            } else if let Some(next) = unsafe { self.nextResponder() } {
                // NSView has no `cancelOperation:` of its own (#163): sending
                // it to `super` crashed. Up the chain instead (a sheet, the
                // window), if anything there handles it; the app's key
                // handlers already heard Escape from the key monitor.
                let _ = unsafe { next.tryToPerform_with(sel!(cancelOperation:), sender) };
            }
        }

        /// Like native buttons: reachable with Tab only when the user enabled
        /// Keyboard Navigation (System Settings › Keyboard).
        #[unsafe(method(canBecomeKeyView))]
        fn can_become_key_view(&self) -> bool {
            let iv = self.ivars();
            (iv.key_local.get()
                || (iv.clickable.get()
                    && NSApplication::sharedApplication(self.mtm()).isFullKeyboardAccessEnabled()))
                && !crate::interact::in_inert(self)
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let code = event.keyCode();
            if self.ivars().key_local.get() {
                // Its `on_key_down` (and its ancestors', then global ones).
                if let Some(key) = crate::interact::key_event(event) {
                    hold_key(&key, Some(self.ivars().id.get()));
                    dispatch(UiEvent::Key(Some(self.ivars().id.get()), key));
                }
                if self.ivars().clickable.get() && (code == 49 || code == 36 || code == 76) {
                    dispatch(UiEvent::Click(self.ivars().id.get()));
                }
            } else if self.ivars().clickable.get() && (code == 49 || code == 36 || code == 76) {
                dispatch(UiEvent::Click(self.ivars().id.get()));
            } else {
                unsafe { msg_send![super(self), keyDown: event] }
            }
        }

        /// A key released while it has the focus (#159).
        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            match crate::interact::key_event(event) {
                Some(key) if self.ivars().key_local.get() => {
                    release_key(&key.key);
                    dispatch(UiEvent::KeyUp(Some(self.ivars().id.get()), key));
                }
                _ => unsafe { msg_send![super(self), keyUp: event] },
            }
        }

        #[unsafe(method(focusRingMaskBounds))]
        fn focus_ring_mask_bounds(&self) -> NSRect {
            if self.ivars().mouse_focus.get() {
                return NSRect::ZERO;
            }
            self.bounds()
        }

        /// An empty mask draws no ring (#177): after a mouse press.
        #[unsafe(method(drawFocusRingMask))]
        fn draw_focus_ring_mask(&self) {
            if self.ivars().mouse_focus.get() {
                return;
            }
            let r = self.ivars().radius.get();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), r, r).fill();
        }

        /// Only `.window_drag()` regions move the window (#67): a custom
        /// chrome's title bar area is the app's content.
        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            // `.window_drag()` (#67): presses that clickable descendants
            // did not take move the window; a double click zooms it.
            if self.moves_window()
                && let Some(window) = self.window()
            {
                if event.clickCount() >= 2 {
                    crate::chrome::double_click(&window);
                } else {
                    // The drag starts when the mouse moves: started now,
                    // AppKit would log « Window move completed without
                    // beginning » for each click.
                    self.ivars()
                        .window_press
                        .replace(Some(objc2::Message::retain(event)));
                }
                return;
            }
            if self.ivars().report_active.get() {
                report_state(self.ivars().id.get(), prism_core::Interaction::Active, true);
            }
            let iv = self.ivars();
            let drags = iv.pointer_mask.get() & prism_core::events::pointer_mask::DRAG != 0;
            iv.dragging.set(false);
            iv.drag_button.set(prism_core::PointerButton::Primary);
            iv.drag_modifiers.set(crate::interact::modifiers(event.modifierFlags()));
            iv.press.set(Some(event.locationInWindow()));
            if iv.key_local.get()
                && let Some(w) = self.window()
            {
                w.makeFirstResponder(Some(self));
            }
            if !iv.clickable.get() && !drags {
                iv.press.set(None);
                unsafe { msg_send![super(self), mouseDown: event] }
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            // A click on a region that moves the window: nothing moved.
            if self.ivars().window_press.take().is_some() {
                return;
            }
            if self.ivars().report_active.get() {
                report_state(self.ivars().id.get(), prism_core::Interaction::Active, false);
            }
            let iv = self.ivars();
            let start = iv.press.take();
            if iv.dragging.replace(false) {
                if let Some(start) = start {
                    let at = event.locationInWindow();
                    self.drag(DragPhase::End, start, at.x - start.x, start.y - at.y);
                }
                return;
            }
            if !iv.clickable.get() {
                if start.is_none() {
                    unsafe { msg_send![super(self), mouseUp: event] }
                }
                return;
            }
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            let b = self.bounds();
            if p.x >= 0. && p.y >= 0. && p.x <= b.size.width && p.y <= b.size.height {
                // Where it was clicked, from the top-left corner (flipped).
                let at = Point::new(p.x as f32, p.y as f32);
                // With the keys held and AppKit's click count (#159).
                dispatch(UiEvent::ClickWith(
                    self.ivars().id.get(),
                    prism_core::ClickEvent {
                        position: Some(at),
                        modifiers: crate::interact::modifiers(event.modifierFlags()),
                        count: event.clickCount().max(1) as u32,
                        button: prism_core::PointerButton::Primary,
                        text_offset: None,
                    },
                ));
            }
        }
    }
);

/// The mouse button of an `otherMouse…` event: 2 is the middle one.
fn button_of(event: &NSEvent) -> prism_core::PointerButton {
    match event.buttonNumber() {
        2 => prism_core::PointerButton::Middle,
        n => prism_core::PointerButton::Other(n.clamp(0, 255) as u8),
    }
}

/// A trackpad gesture's phase from AppKit's (`None` to skip the event:
/// fingers resting without moving).
fn gesture_phase(
    phase: objc2_app_kit::NSEventPhase,
    momentum: objc2_app_kit::NSEventPhase,
) -> Option<prism_core::GesturePhase> {
    use objc2_app_kit::NSEventPhase as P;
    use prism_core::GesturePhase as G;
    let ended = P::Ended | P::Cancelled;
    Some(if momentum.intersects(ended) {
        G::End
    } else if !momentum.is_empty() {
        G::Momentum
    } else if phase.contains(P::MayBegin) {
        return None;
    } else if phase.contains(P::Began) {
        G::Start
    } else if phase.intersects(P::Changed | P::Stationary) {
        G::Move
    } else if phase.intersects(ended) {
        G::End
    } else {
        G::None
    })
}

impl PrismView {
    /// Whether a press on it moves the window (#67): a `.window_drag()`
    /// region, or a tab bar or header along a custom chrome's top edge,
    /// that does not take the press itself.
    pub(crate) fn moves_window(&self) -> bool {
        let iv = self.ivars();
        (iv.window_drag.get() || iv.top_bar.get() && crate::chrome::along_top(self))
            && !iv.clickable.get()
            && iv.pointer_mask.get() & prism_core::events::pointer_mask::DRAG == 0
    }

    /// Whether it takes presses of its own: clickable, or reporting drags.
    pub(crate) fn takes_presses(&self) -> bool {
        let iv = self.ivars();
        iv.clickable.get() || iv.pointer_mask.get() & prism_core::events::pointer_mask::DRAG != 0
    }

    fn is_cell(&self) -> bool {
        self.accessibilityRole().as_deref()
            == Some(unsafe { objc2_app_kit::NSAccessibilityCellRole })
    }

    /// VoiceOver's press on a table cell (#284): the control it holds when
    /// it holds one, else its row when the row is clickable.
    fn press_cell(&self) -> bool {
        match &crate::table::controls(self)[..] {
            [control] => press_control(control),
            [] => {
                let row = unsafe { objc2_app_kit::NSAccessibilityRowRole };
                // SAFETY: the view's superview, on the main thread.
                let Some(up) = (unsafe { self.superview() }) else {
                    return false;
                };
                match up.downcast_ref::<PrismView>() {
                    Some(up)
                        if up.accessibilityRole().as_deref() == Some(row)
                            && up.ivars().clickable.get() =>
                    {
                        dispatch(UiEvent::Click(up.ivars().id.get()));
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// Reports the pointer entering, moving over or leaving the view.
    fn pointer(&self, phase: PointerPhase, event: &NSEvent) {
        use prism_core::events::pointer_mask::{HOVER, MOVE};
        let mask = self.ivars().pointer_mask.get();
        if mask & (HOVER | MOVE) == 0 {
            return;
        }
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        let position = Point::new(p.x as f32, p.y as f32);
        dispatch(UiEvent::Pointer(
            self.ivars().id.get(),
            PointerEvent { phase, position },
        ));
    }

    /// A drag that started at `start` (window coordinates) and moved by
    /// (`dx`, `dy`) points, y down.
    fn drag(&self, phase: DragPhase, start: NSPoint, dx: f64, dy: f64) {
        let p = self.convertPoint_fromView(start, None);
        let iv = self.ivars();
        // The drag in progress (#223): ⇧ or ⌥ pressed alone resend it.
        DRAGGED.with(|d| {
            *d.borrow_mut() = match phase {
                DragPhase::Start | DragPhase::Move => {
                    let me: &PrismView = self;
                    Some((objc2::Message::retain(me), start, dx, dy))
                }
                _ => None,
            }
        });
        dispatch(UiEvent::Drag(
            iv.id.get(),
            DragEvent {
                phase,
                start: Point::new(p.x as f32, p.y as f32),
                translation: Point::new(dx as f32, dy as f32),
                button: iv.drag_button.get(),
                modifiers: iv.drag_modifiers.get(),
            },
        ));
    }

    /// A middle or secondary press (#136): taken (`true`) when the view lets
    /// that button drag.
    fn button_down(&self, event: &NSEvent, button: prism_core::PointerButton) -> bool {
        let iv = self.ivars();
        let bit = button.drag_bit();
        let drags = bit != 0 && iv.pointer_mask.get() & bit != 0;
        if !(drags || self.aux_clicks(button)) || iv.dragging.get() {
            return false;
        }
        iv.drag_button.set(button);
        iv.drag_modifiers
            .set(crate::interact::modifiers(event.modifierFlags()));
        iv.other_press.set(Some(event.locationInWindow()));
        true
    }

    /// The pressed middle or secondary button moved: past 3 points, a drag.
    fn button_dragged(&self, event: &NSEvent) -> bool {
        let iv = self.ivars();
        let Some(start) = iv.other_press.get() else {
            return false;
        };
        // Taken for a click only (#215): no drag.
        let bit = iv.drag_button.get().drag_bit();
        if bit == 0 || iv.pointer_mask.get() & bit == 0 {
            return true;
        }
        let at = event.locationInWindow();
        let (dx, dy) = (at.x - start.x, start.y - at.y);
        // The keys held now (#223).
        iv.drag_modifiers
            .set(crate::interact::modifiers(event.modifierFlags()));
        if !iv.dragging.get() {
            if dx.hypot(dy) < 3. {
                return true;
            }
            iv.dragging.set(true);
            self.drag(DragPhase::Start, start, dx, dy);
        } else {
            self.drag(DragPhase::Move, start, dx, dy);
        }
        true
    }

    /// The middle or secondary button went up: `None` if the view did not
    /// take the press, else whether it was dragged.
    fn button_up(&self, event: &NSEvent) -> Option<bool> {
        let iv = self.ivars();
        let start = iv.other_press.take()?;
        let dragged = iv.dragging.replace(false);
        if dragged {
            let at = event.locationInWindow();
            self.drag(DragPhase::End, start, at.x - start.x, start.y - at.y);
        } else if self.aux_clicks(iv.drag_button.get()) {
            // A middle, back or forward click (#215), released over the view.
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            let b = self.bounds();
            if p.x >= 0. && p.y >= 0. && p.x <= b.size.width && p.y <= b.size.height {
                dispatch(UiEvent::AuxClick(
                    iv.id.get(),
                    prism_core::ClickEvent {
                        position: Some(Point::new(p.x as f32, p.y as f32)),
                        modifiers: crate::interact::modifiers(event.modifierFlags()),
                        count: event.clickCount().max(1) as u32,
                        button: iv.drag_button.get(),
                        text_offset: None,
                    },
                ));
            }
        }
        Some(dragged)
    }

    /// Whether the view hears clicks of `button` (#215): the middle, back
    /// and forward buttons, with `on_aux_click`.
    fn aux_clicks(&self, button: prism_core::PointerButton) -> bool {
        use prism_core::PointerButton as B;
        self.ivars().pointer_mask.get() & prism_core::events::pointer_mask::AUX_CLICK != 0
            && !matches!(button, B::Primary | B::Secondary)
    }

    /// Hover and move events through a tracking area (key window only).
    fn set_pointer_mask(&self, mask: u8) {
        use prism_core::events::pointer_mask::{HOVER, MOVE};
        let iv = self.ivars();
        iv.pointer_mask.set(mask);
        if let Some(old) = iv.tracking.borrow_mut().take() {
            self.removeTrackingArea(&old);
        }
        if mask & (HOVER | MOVE) != 0 {
            let options = NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::ActiveInKeyWindow
                | NSTrackingAreaOptions::InVisibleRect;
            let owner: &AnyObject = self;
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    <NSTrackingArea as objc2::AllocAnyThread>::alloc(),
                    NSRect::ZERO,
                    options,
                    Some(owner),
                    None,
                )
            };
            self.addTrackingArea(&area);
            *iv.tracking.borrow_mut() = Some(area);
            // Overflowing its parent: over the part outside it too (#213).
            self.updateTrackingAreas();
        }
    }

    fn context_menu(&self) -> Option<Retained<NSMenu>> {
        let spec = self.ivars().menu.borrow().clone()?;
        let mut target = None;
        with_state(|s| target = Some(s.renderer.widgets.target.clone()));
        let target: Retained<AnyObject> = Retained::into_super(Retained::into_super(target?));
        Some(crate::menus::build_menu(self.mtm(), &spec, &target))
    }

    /// One `CAShapeLayer` per path, rebuilt within the view's appearance
    /// (dynamic colors resolve to light or dark here).
    fn update_shape_layers(&self, layer: &objc2_quartz_core::CALayer) {
        use objc2_core_graphics::CGMutablePath;
        use objc2_quartz_core::CAShapeLayer;
        use prism_core::Segment as S;
        let iv = self.ivars();
        for old in iv.shape_layers.borrow_mut().drain(..) {
            old.removeFromSuperlayer();
        }
        let paths = iv.shape.borrow();
        if paths.is_empty() {
            return;
        }
        // Paths are top-left based: flip unless the layer tree already is.
        let mut flipped = false;
        let mut l = Some(objc2::Message::retain(layer));
        while let Some(cur) = l {
            flipped ^= cur.isGeometryFlipped();
            l = cur.superlayer();
        }
        let h = self.bounds().size.height;
        let y = |v: f32| if flipped { v as f64 } else { h - v as f64 };
        let mut layers = Vec::new();
        for p in paths.iter() {
            let cg = CGMutablePath::new();
            let path = Some(&*cg);
            let n = std::ptr::null();
            for s in &p.segments {
                unsafe {
                    match *s {
                        S::Move(px, py) => CGMutablePath::move_to_point(path, n, px as f64, y(py)),
                        S::Line(px, py) => {
                            CGMutablePath::add_line_to_point(path, n, px as f64, y(py))
                        }
                        S::Cubic(a, b, c, d, e, f) => CGMutablePath::add_curve_to_point(
                            path,
                            n,
                            a as f64,
                            y(b),
                            c as f64,
                            y(d),
                            e as f64,
                            y(f),
                        ),
                        S::Close => CGMutablePath::close_subpath(path),
                        S::Arc { .. } => {}
                    }
                }
            }
            let shape = CAShapeLayer::new();
            shape.setFrame(self.bounds());
            shape.setPath(Some(&cg));
            shape.setFillColor(p.fill.map(|c| ns_color(c).CGColor()).as_deref());
            shape.setStrokeColor(p.stroke.map(|c| ns_color(c).CGColor()).as_deref());
            shape.setLineWidth(p.width as f64);
            if p.round {
                unsafe {
                    shape.setLineCap(objc2_quartz_core::kCALineCapRound);
                    shape.setLineJoin(objc2_quartz_core::kCALineJoinRound);
                }
            }
            if let Some((dash, gap)) = p.dash {
                let pattern = NSArray::from_retained_slice(&[
                    objc2_foundation::NSNumber::new_f64(dash as f64),
                    objc2_foundation::NSNumber::new_f64(gap as f64),
                ]);
                shape.setLineDashPattern(Some(&pattern));
            }
            if p.even_odd {
                shape.setFillRule(unsafe { objc2_quartz_core::kCAFillRuleEvenOdd });
            }
            layer.addSublayer(&shape);
            layers.push(shape);
        }
        *iv.shape_layers.borrow_mut() = layers;
    }

    /// Draws the background and border as a path rounding only some corners
    /// (in a sublayer under the content).
    fn update_box_layer(&self, layer: &objc2_quartz_core::CALayer, radius: f64, bw: f64) {
        use objc2_core_graphics::CGMutablePath;
        use objc2_quartz_core::CAShapeLayer;
        use prism_core::Corners as C;
        let iv = self.ivars();
        layer.setBackgroundColor(None);
        layer.setCornerRadius(0.);
        layer.setBorderWidth(0.);
        let mut flipped = false;
        let mut l = Some(objc2::Message::retain(layer));
        while let Some(cur) = l {
            flipped ^= cur.isGeometryFlipped();
            l = cur.superlayer();
        }
        let b = self.bounds().size;
        let (w, h) = (b.width, b.height);
        let y = |v: f64| if flipped { v } else { h - v };
        // Inset by half the border so the stroke stays inside.
        let i = bw / 2.;
        let corners = iv.corners.get();
        let r = |c: C| {
            if corners.contains(c) {
                (radius - i).max(0.)
            } else {
                0.
            }
        };
        let (tl, tr, br, bl) = (
            r(C::TOP_LEADING),
            r(C::TOP_TRAILING),
            r(C::BOTTOM_TRAILING),
            r(C::BOTTOM_LEADING),
        );
        // Quarter circles as cubics (k = 0.5523).
        let k = 0.5523;
        let cg = CGMutablePath::new();
        let p = Some(&*cg);
        let n = std::ptr::null();
        unsafe {
            CGMutablePath::move_to_point(p, n, i + tl, y(i));
            CGMutablePath::add_line_to_point(p, n, w - i - tr, y(i));
            CGMutablePath::add_curve_to_point(
                p,
                n,
                w - i - tr + k * tr,
                y(i),
                w - i,
                y(i + tr - k * tr),
                w - i,
                y(i + tr),
            );
            CGMutablePath::add_line_to_point(p, n, w - i, y(h - i - br));
            CGMutablePath::add_curve_to_point(
                p,
                n,
                w - i,
                y(h - i - br + k * br),
                w - i - br + k * br,
                y(h - i),
                w - i - br,
                y(h - i),
            );
            CGMutablePath::add_line_to_point(p, n, i + bl, y(h - i));
            CGMutablePath::add_curve_to_point(
                p,
                n,
                i + bl - k * bl,
                y(h - i),
                i,
                y(h - i - bl + k * bl),
                i,
                y(h - i - bl),
            );
            CGMutablePath::add_line_to_point(p, n, i, y(i + tl));
            CGMutablePath::add_curve_to_point(
                p,
                n,
                i,
                y(i + tl - k * tl),
                i + tl - k * tl,
                y(i),
                i + tl,
                y(i),
            );
            CGMutablePath::close_subpath(p);
        }
        let mut slot = iv.box_layer.borrow_mut();
        let shape = slot.get_or_insert_with(|| {
            let shape = CAShapeLayer::new();
            shape.setName(Some(&NSString::from_str(crate::decor::BOX_LAYER)));
            layer.insertSublayer_atIndex(&shape, 0);
            shape
        });
        shape.setFrame(self.bounds());
        shape.setPath(Some(&cg));
        let cg_color = |c: &NSColor| c.CGColor();
        shape.setFillColor(iv.background.borrow().as_deref().map(cg_color).as_deref());
        shape.setStrokeColor(iv.border_color.borrow().as_deref().map(cg_color).as_deref());
        shape.setLineWidth(bw);
    }

    pub(crate) fn new(mtm: MainThreadMarker, id: NodeId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            id: Cell::new(id),
            ..Default::default()
        });
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        view.setWantsLayer(true);
        view.setLayerContentsRedrawPolicy(
            objc2_app_kit::NSViewLayerContentsRedrawPolicy::DuringViewResize,
        );
        view
    }
}

// ---------------------------------------------------------------------------
// PrismSlider: an NSSlider that snaps to its step and adjusts by Prism's step
// for VoiceOver (increment / decrement), reading out `a11y.value` if given.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct SliderIvars {
    config: Cell<SliderConfig>,
    /// Last value reported (a drag sends many actions per step).
    last: Cell<f64>,
    /// What VoiceOver reads instead of the number ("40 %").
    a11y_value: RefCell<Option<String>>,
}

define_class!(
    #[unsafe(super(NSSlider, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismSlider"]
    #[ivars = SliderIvars]
    pub struct PrismSlider;

    impl PrismSlider {
        #[unsafe(method(accessibilityValue))]
        fn accessibility_value(&self) -> *mut AnyObject {
            if let Some(v) = self.ivars().a11y_value.borrow().as_deref() {
                return Retained::autorelease_return(NSString::from_str(v)).cast();
            }
            let v: Option<Retained<AnyObject>> = unsafe { msg_send![super(self), accessibilityValue] };
            v.map_or(std::ptr::null_mut(), Retained::autorelease_return)
        }

        #[unsafe(method(accessibilityPerformIncrement))]
        fn accessibility_perform_increment(&self) -> objc2::runtime::Bool {
            self.nudge(1.);
            objc2::runtime::Bool::YES
        }

        #[unsafe(method(accessibilityPerformDecrement))]
        fn accessibility_perform_decrement(&self) -> objc2::runtime::Bool {
            self.nudge(-1.);
            objc2::runtime::Bool::YES
        }
    }
);

impl PrismSlider {
    fn new(mtm: MainThreadMarker, id: NodeId, target: &Target) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SliderIvars::default());
        let s: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        s.setTag(id as isize);
        s.setContinuous(true);
        let target: &AnyObject = target;
        unsafe {
            s.setTarget(Some(target));
            s.setAction(Some(sel!(onSlider:)));
        }
        s
    }

    /// One accessibility step up or down.
    fn nudge(&self, direction: f64) {
        let cfg = self.ivars().config.get();
        let v = cfg.snap(self.doubleValue() + direction * cfg.a11y_step());
        self.setDoubleValue(v);
        self.report(v);
    }

    fn report(&self, v: f64) {
        if self.ivars().last.replace(v) != v {
            dispatch(UiEvent::Change(self.tag() as NodeId, v));
        }
    }
}

// ---------------------------------------------------------------------------
// PrismTextView: the editable text of a text area. NSTextView has no public
// placeholder (only NSTextField does), so it draws its own.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct TextViewIvars {
    pub(crate) id: Cell<NodeId>,
    placeholder: RefCell<String>,
    /// A rich input (#115): runs drawn as attributes (`crate::rich`).
    pub(crate) rich: Cell<bool>,
    /// A text area's minimum lines (#235); 0: three.
    pub(crate) rows: Cell<u32>,
    /// Its encoded runs.
    pub(crate) runs: RefCell<String>,
    /// Its style (font, color), once set.
    pub(crate) style: RefCell<Option<Style>>,
    /// The editing events it reports (#217, `edit_events` bits).
    pub(crate) edit_mask: Cell<u8>,
}

define_class!(
    #[unsafe(super(NSTextView, NSText, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismTextView"]
    #[ivars = TextViewIvars]
    pub struct PrismTextView;

    impl PrismTextView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            let _: () = unsafe { msg_send![super(self), drawRect: dirty] };
            self.draw_placeholder();
        }

        /// The app's copy (#217); AppKit's otherwise.
        #[unsafe(method(copy:))]
        fn copy(&self, sender: Option<&AnyObject>) {
            if !crate::edit::copy(self, false) {
                let _: () = unsafe { msg_send![super(self), copy: sender] };
            }
        }

        /// The app's cut (#217), then the selection deleted; AppKit's otherwise.
        #[unsafe(method(cut:))]
        fn cut(&self, sender: Option<&AnyObject>) {
            if crate::edit::copy(self, true) {
                let _: () = unsafe { msg_send![self, delete: sender] };
            } else {
                let _: () = unsafe { msg_send![super(self), cut: sender] };
            }
        }

        /// ⌘- / ⌥-clicks on a rich input's runs (#217).
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if !crate::edit::mouse_down(self, event) {
                let _: () = unsafe { msg_send![super(self), mouseDown: event] };
            }
        }

        /// A rich input pastes plain text, in the style where it lands
        /// (#115), or gives the paste to the app (#217).
        #[unsafe(method(paste:))]
        fn paste(&self, sender: Option<&AnyObject>) {
            if crate::edit::paste(self) {
                return;
            }
            if self.ivars().rich.get() {
                unsafe { self.pasteAsPlainText(sender) };
            } else {
                let _: () = unsafe { msg_send![super(self), paste: sender] };
            }
        }
    }
);

impl PrismTextView {
    fn draw_placeholder(&self) {
        use objc2_app_kit::NSStringDrawing;
        let placeholder = self.ivars().placeholder.borrow();
        if placeholder.is_empty() || self.string().length() > 0 {
            return;
        }
        let font = self
            .font()
            .unwrap_or_else(|| NSFont::systemFontOfSize(NSFont::systemFontSize()));
        let color = NSColor::placeholderTextColor();
        let (f, c): (&AnyObject, &AnyObject) = (&font, &color);
        let keys = unsafe {
            [
                objc2_app_kit::NSFontAttributeName,
                objc2_app_kit::NSForegroundColorAttributeName,
            ]
        };
        let attrs = objc2_foundation::NSDictionary::from_slices(&keys, &[f, c]);
        let s = NSString::from_str(&placeholder);
        let origin = self.textContainerOrigin();
        let pad = unsafe { self.textContainer() }
            .map_or(LABEL_FRAGMENT_PADDING, |c| c.lineFragmentPadding());
        let size = unsafe { s.sizeWithAttributes(Some(&attrs)) };
        let rtl = self.userInterfaceLayoutDirection()
            == objc2_app_kit::NSUserInterfaceLayoutDirection::RightToLeft;
        let x = if rtl {
            self.bounds().size.width - origin.x - pad - size.width
        } else {
            origin.x + pad
        };
        unsafe { s.drawAtPoint_withAttributes(NSPoint::new(x, origin.y), Some(&attrs)) };
    }
}

/// Spelling correction, text replacement and spell checking of a text view
/// (a text area, or the window's field editor while a text input is edited),
/// following the user's System Settings when on.
fn set_text_checking(tv: &NSTextView, on: bool) {
    use objc2_app_kit::NSSpellChecker;
    tv.setAutomaticSpellingCorrectionEnabled(
        on && NSSpellChecker::isAutomaticSpellingCorrectionEnabled(),
    );
    tv.setAutomaticTextReplacementEnabled(
        on && NSSpellChecker::isAutomaticTextReplacementEnabled(),
    );
    tv.setContinuousSpellCheckingEnabled(on);
    tv.setGrammarCheckingEnabled(false);
}

// ---------------------------------------------------------------------------
// Target: receives button actions and text-field edits, routes them by tag.
// ---------------------------------------------------------------------------

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismTarget"]
    pub struct Target;

    impl Target {
        #[unsafe(method(onAction:))]
        fn on_action(&self, sender: &NSButton) {
            dispatch(UiEvent::Click(sender.tag() as NodeId));
        }

        /// A checkbox was checked or unchecked.
        #[unsafe(method(onCheck:))]
        fn on_check(&self, sender: &NSButton) {
            let on = sender.state() == NSControlStateValueOn;
            dispatch(UiEvent::Change(sender.tag() as NodeId, if on { 1. } else { 0. }));
        }

        /// A radio button was chosen (tag = its index; the group is its superview).
        #[unsafe(method(onRadio:))]
        fn on_radio(&self, sender: &NSButton) {
            let group = unsafe { sender.superview() }
                .and_then(|v| v.downcast::<PrismView>().ok())
                .map(|v| v.ivars().id.get());
            if let Some(id) = group {
                dispatch(UiEvent::Change(id, sender.tag() as f64));
            }
        }

        #[unsafe(method(onToggle:))]
        fn on_toggle(&self, sender: &NSSwitch) {
            let on = sender.state() == NSControlStateValueOn;
            dispatch(UiEvent::Change(sender.tag() as NodeId, if on { 1. } else { 0. }));
        }

        #[unsafe(method(onSegment:))]
        fn on_segment(&self, sender: &NSSegmentedControl) {
            let index = sender.selectedSegment().max(0) as f64;
            dispatch(UiEvent::Change(sender.tag() as NodeId, index));
        }

        /// A slider moved (continuously while dragging): snapped to its step.
        #[unsafe(method(onSlider:))]
        fn on_slider(&self, sender: &PrismSlider) {
            let raw = sender.doubleValue();
            let v = sender.ivars().config.get().snap(raw);
            if v != raw && sender.numberOfTickMarks() == 0 {
                sender.setDoubleValue(v);
            }
            sender.report(v);
        }

        /// A pop-up button option was chosen (item tag = option index; the
        /// placeholder item is -1).
        #[unsafe(method(onPicker:))]
        fn on_picker(&self, sender: &NSPopUpButton) {
            let index = sender.selectedItem().map_or(-1, |i| i.tag());
            if index >= 0 {
                dispatch(UiEvent::Change(sender.tag() as NodeId, index as f64));
            }
        }

        /// A text input's action, sent on Return when the field editor
        /// didn't ask `control:textView:doCommandBySelector:` first.
        #[unsafe(method(onSubmit:))]
        fn on_submit(&self, sender: &NSTextField) {
            dispatch(UiEvent::Submit(sender.tag() as NodeId));
        }

        /// A date picker changed: Unix seconds.
        #[unsafe(method(onDate:))]
        fn on_date(&self, sender: &NSDatePicker) {
            let secs = sender.dateValue().timeIntervalSince1970();
            crate::dates::edited(sender.tag() as NodeId, secs);
            dispatch(UiEvent::Change(sender.tag() as NodeId, secs));
        }

        /// The placeholder of an empty optional date picker (#89).
        #[unsafe(method(onDateFill:))]
        fn on_date_fill(&self, sender: &NSButton) {
            crate::dates::fill(sender.tag() as NodeId);
        }

        /// The clear button of an optional date picker (#89).
        #[unsafe(method(onDateClear:))]
        fn on_date_clear(&self, sender: &NSButton) {
            crate::dates::clear(sender.tag() as NodeId);
        }

        /// Any Prism menu item (context menus, tray, app menus).
        /// App items keep the state the app gave them in menus that enable
        /// system items automatically (#161).
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &objc2_app_kit::NSMenuItem) -> bool {
            item.isEnabled()
        }

        /// The Quit item: with unsaved changes, asks first (#158).
        #[unsafe(method(prismQuit:))]
        fn prism_quit(&self, _sender: Option<&AnyObject>) {
            let app = NSApplication::sharedApplication(self.mtm());
            match app.mainWindow().or_else(|| app.keyWindow()) {
                Some(window) if UNSAVED.get() => {
                    let mtm = self.mtm();
                    confirm_leave(&window, move || {
                        NSApplication::sharedApplication(mtm).terminate(None)
                    });
                }
                _ => app.terminate(None),
            }
        }

        #[unsafe(method(onMenuItem:))]
        fn on_menu_item(&self, sender: &NSMenuItem) {
            dispatch(UiEvent::MenuSelect(sender.tag() as u64));
        }

        /// Every second: countdowns in the window and the menu bar.
        #[unsafe(method(onTick:))]
        fn on_tick(&self, _timer: &AnyObject) {
            with_state(|s| {
                s.renderer.widgets.tick();
                s.services.tick();
            });
        }

        /// "Open" in a menu bar activity (tag = activity id).
        #[unsafe(method(onStatusItem:))]
        fn on_status_item(&self, sender: &NSMenuItem) {
            let mut link = None;
            with_state(|s| link = s.services.link_for(sender.tag()));
            if let Some(link) = link {
                NSApplication::sharedApplication(self.mtm()).activate();
                open_link(&link);
            }
        }

        /// Reduce Motion or the language changed in System Settings.
        /// A text view's selection moved: a text area, or the field editor
        /// of a text input (#86).
        #[unsafe(method(onTextSelection:))]
        fn on_text_selection(&self, n: &NSNotification) {
            let Some(tv) = n.object().and_then(|o| o.downcast::<NSTextView>().ok()) else {
                return;
            };
            let Some(id) = crate::interact::text_node(&tv) else {
                return;
            };
            // Typing and scrolling the field move other people's carets (#240).
            crate::carets::refresh(id);
            // Not while Prism changes the field (`with_state` is busy).
            let mut on = false;
            with_state(|s| on = s.renderer.widgets.ix.select_events.contains(&id));
            if on {
                crate::interact::selection_changed(&tv, id);
            }
            // Its rectangle (#217).
            crate::edit::rect_changed(&tv, id);
        }

        /// A clip view scrolled: other people's carets in the text it
        /// shows follow (#240).
        #[unsafe(method(onClipScroll:))]
        fn on_clip_scroll(&self, n: &NSNotification) {
            if let Some(v) = n.object().and_then(|o| o.downcast::<NSView>().ok()) {
                crate::carets::scrolled(&v);
            }
        }

        #[unsafe(method(onEnvironment:))]
        fn on_environment(&self, _n: &NSNotification) {
            with_state(|s| {
                s.runtime.app.set_environment(mac_environment());
                let patches = s.runtime.render();
                s.commit(patches);
            });
        }

        /// Active / inactive / hidden.
        /// Another device changed iCloud keys.
        #[unsafe(method(onCloudChange:))]
        fn on_cloud_change(&self, n: &NSNotification) {
            let keys = prism_native::apple::changed_cloud_keys(n);
            with_state(|s| {
                let patches = s.runtime.cloud_changed(&keys);
                s.commit(patches);
            });
        }

        #[unsafe(method(onLifecycle:))]
        fn on_lifecycle(&self, n: &NSNotification) {
            use objc2_app_kit::*;
            let name = n.name();
            let phase = unsafe {
                if &*name == NSApplicationDidBecomeActiveNotification
                    || &*name == NSWindowDidDeminiaturizeNotification
                    || &*name == NSApplicationDidUnhideNotification
                {
                    prism_core::Lifecycle::Active
                } else if &*name == NSApplicationDidResignActiveNotification {
                    prism_core::Lifecycle::Inactive
                } else {
                    prism_core::Lifecycle::Background
                }
            };
            with_state(|s| {
                let patches = s.runtime.set_lifecycle(phase);
                s.commit(patches);
            });
        }

        /// A scroll view moved (virtualized lists follow it).
        #[unsafe(method(onScrollBounds:))]
        fn on_scroll_bounds(&self, n: &NSNotification) {
            let Some(clip) = n.object().and_then(|o| o.downcast::<objc2_app_kit::NSClipView>().ok())
            else {
                return;
            };
            let Some(doc) = clip.documentView().and_then(|d| d.downcast::<PrismView>().ok()) else {
                return;
            };
            crate::sticky::scrolled(&clip);
            if !crate::sticky::wants_scroll_events(&clip) {
                return;
            }
            let b = clip.bounds();
            // Moved by the app (`scroll_by`, `scroll_into_view`) while Prism
            // applies patches: reported right after, not lost (#204).
            dispatch_or_defer(UiEvent::Scroll(
                doc.ivars().id.get(),
                b.origin.y as f32,
                b.size.height as f32,
            ));
        }

        /// A stack's header Back button (tag = stack id).
        #[unsafe(method(onBack:))]
        fn on_back(&self, sender: &NSButton) {
            go_back(Some(sender.tag() as NodeId));
        }

        /// Go ▸ Back (⌘[).
        #[unsafe(method(onBackMenu:))]
        fn on_back_menu(&self, _sender: Option<&AnyObject>) {
            go_back(None);
        }

        /// Hover tracking areas (`.hover(|s| …)`).
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            hover_event(event, true);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, event: &NSEvent) {
            hover_event(event, false);
        }

        /// A window's `firstResponder` changed (key-value observing).
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_value(
            &self,
            _key: Option<&NSString>,
            object: Option<&AnyObject>,
            _change: Option<&AnyObject>,
            _context: *mut std::ffi::c_void,
        ) {
            if let Some(window) = object.and_then(|o| o.downcast_ref::<NSWindow>()) {
                focus_changed(window);
            }
        }
    }

    impl Target {
        /// Dev hook: `PRISM_CLICK="+1|Save"` presses controls by title or accessibility label, then
        /// `PRISM_SNAPSHOT=out.png` writes the window to a PNG and quits.
        #[unsafe(method(prismAutomation:))]
        fn automation(&self, _sender: Option<&AnyObject>) {
            run_automation(self.mtm());
        }
    }

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSControlTextEditingDelegate for Target {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, notification: &NSNotification) {
            let Some(field) = notification.object().and_then(|o| o.downcast::<NSTextField>().ok())
            else {
                return;
            };
            let id = field.tag() as NodeId;
            let (mut max, mut suggests) = (None, false);
            with_state(|s| {
                let w = &s.renderer.widgets;
                max = w.inputs.get(&id).and_then(|c| c.max_length);
                suggests = w.ix.suggestions.get(&id).is_some_and(|l| !l.is_empty());
            });
            let mut text = field.stringValue().to_string();
            // `max_length`: what goes past it is dropped.
            if let Some(max) = max
                && text.chars().count() > max as usize
            {
                text = text.chars().take(max as usize).collect();
                field.setStringValue(&NSString::from_str(&text));
            }
            let grew = text.chars().count() > LAST_LEN.with(|l| l.replace(text.chars().count()));
            dispatch(UiEvent::Input(id, text));
            // Suggestions: AppKit's completion list as the user types.
            if suggests
                && grew
                && !COMPLETING.with(|c| c.replace(true))
                && let Some(editor) = field.currentEditor().and_then(|e| e.downcast::<NSTextView>().ok())
            {
                unsafe { editor.complete(None) };
            }
            COMPLETING.with(|c| c.set(false));
        }

        /// The completion list of a text input with suggestions.
        #[unsafe(method(control:textView:completions:forPartialWordRange:indexOfSelectedItem:))]
        fn control_completions(
            &self,
            control: &NSControl,
            text_view: &NSTextView,
            _words: &NSArray<NSString>,
            range: objc2_foundation::NSRange,
            index: *mut isize,
        ) -> *mut NSArray<NSString> {
            let id = control.tag() as NodeId;
            let mut list = Vec::new();
            with_state(|s| list = s.renderer.widgets.ix.suggestions.get(&id).cloned().unwrap_or_default());
            let text = text_view.string().to_string();
            // UTF-16 offset of the completed word → characters.
            let from = String::from_utf16_lossy(
                &text.encode_utf16().take(range.location).collect::<Vec<u16>>(),
            )
            .chars()
            .count();
            let found = crate::interact::completions(&text, from, &list);
            if !index.is_null() {
                // Nothing pre-selected: the typed text stays as typed.
                unsafe { *index = -1 };
            }
            let items: Vec<Retained<NSString>> = found.iter().map(|s| NSString::from_str(s)).collect();
            Retained::autorelease_return(NSArray::from_retained_slice(&items))
        }

        /// Return in a text input: `UiEvent::Submit` (the field keeps its
        /// text, caret and focus).
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn control_do_command(
            &self,
            control: &NSControl,
            _text_view: &NSTextView,
            command: objc2::runtime::Sel,
        ) -> objc2::runtime::Bool {
            if command == sel!(insertNewline:) && control.isKindOfClass(NSTextField::class()) {
                dispatch(UiEvent::Submit(control.tag() as NodeId));
                return objc2::runtime::Bool::YES;
            }
            objc2::runtime::Bool::NO
        }

        /// Spelling correction lives on the window's shared field editor: set
        /// it up for the input being edited (`autocorrect=off`, `secure`).
        #[unsafe(method(controlTextDidBeginEditing:))]
        fn control_text_did_begin_editing(&self, notification: &NSNotification) {
            let Some(field) = notification.object().and_then(|o| o.downcast::<NSTextField>().ok())
            else {
                return;
            };
            let mut config = None;
            with_state(|s| config = s.renderer.widgets.inputs.get(&(field.tag() as NodeId)).copied());
            let on = config.is_none_or(|c| c.autocorrect && !c.secure);
            if let Some(editor) = field.currentEditor().and_then(|e| e.downcast::<NSTextView>().ok()) {
                set_text_checking(&editor, on);
            }
        }
    }

    unsafe impl NSTextFieldDelegate for Target {}

    unsafe impl NSTextDelegate for Target {
        /// A text area was edited.
        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, notification: &NSNotification) {
            let Some(tv) = notification.object().and_then(|o| o.downcast::<PrismTextView>().ok())
            else {
                return;
            };
            tv.setNeedsDisplay(true);
            dispatch(UiEvent::Input(tv.ivars().id.get(), tv.string().to_string()));
            // The text under the selection moved (#217).
            crate::edit::rect_changed(&tv, tv.ivars().id.get());
        }
    }

    unsafe impl NSTextViewDelegate for Target {
        /// Return in a rich input: `on_submit` (nothing inserted);
        /// Shift+Return, a line break (#115).
        #[unsafe(method(textView:doCommandBySelector:))]
        fn text_view_do_command(
            &self,
            text_view: &NSTextView,
            command: objc2::runtime::Sel,
        ) -> objc2::runtime::Bool {
            if !crate::rich::is_rich(text_view) {
                return objc2::runtime::Bool::NO;
            }
            let shift = NSApplication::sharedApplication(self.mtm())
                .currentEvent()
                .is_some_and(|e| e.modifierFlags().contains(objc2_app_kit::NSEventModifierFlags::Shift));
            if command == sel!(insertNewline:) && !shift {
                if let Some(id) = crate::interact::text_node(text_view) {
                    dispatch(UiEvent::Submit(id));
                }
                return objc2::runtime::Bool::YES;
            }
            if command == sel!(insertNewline:) || command == sel!(insertLineBreak:) {
                // "\n", not the line separator `insertLineBreak:` types.
                unsafe {
                    objc2_app_kit::NSStandardKeyBindingResponding::insertNewlineIgnoringFieldEditor(
                        text_view, None,
                    )
                };
                return objc2::runtime::Bool::YES;
            }
            objc2::runtime::Bool::NO
        }

        /// A click on a chip with `on_click` (#115).
        #[unsafe(method(textView:clickedOnCell:inRect:atIndex:))]
        fn text_view_clicked_on_cell(
            &self,
            text_view: &NSTextView,
            _cell: &AnyObject,
            _rect: NSRect,
            index: usize,
        ) {
            if let Some(link) = crate::rich::link_at(text_view, index) {
                dispatch(UiEvent::MenuSelect(link));
            }
        }
    }

    unsafe impl NSPopoverDelegate for Target {
        /// The event closing it is still current here (#156): Escape or a
        /// click outside.
        #[unsafe(method(popoverWillClose:))]
        fn popover_will_close(&self, _n: &NSNotification) {
            let mtm = MainThreadMarker::new().expect("main thread");
            let reason = match NSApplication::sharedApplication(mtm).currentEvent() {
                Some(e) if e.r#type() == objc2_app_kit::NSEventType::KeyDown && e.keyCode() == 53 => {
                    prism_core::DismissReason::Escape
                }
                Some(e)
                    if matches!(
                        e.r#type(),
                        objc2_app_kit::NSEventType::LeftMouseDown
                            | objc2_app_kit::NSEventType::RightMouseDown
                            | objc2_app_kit::NSEventType::OtherMouseDown
                    ) =>
                {
                    prism_core::DismissReason::Outside
                }
                _ => prism_core::DismissReason::Unknown,
            };
            POPOVER_CLOSE_REASON.set(reason);
        }

        /// A popover closed: by the user (Escape, a click outside) unless
        /// Prism closed it.
        #[unsafe(method(popoverDidClose:))]
        fn popover_did_close(&self, n: &NSNotification) {
            let Some(popover) = n.object().and_then(|o| o.downcast::<NSPopover>().ok()) else {
                return;
            };
            let mut closed = None;
            with_state(|s| {
                for o in &s.renderer.widgets.ix.overlays {
                    let mine = o.popover.borrow().as_deref().is_some_and(|p| *p == *popover);
                    if mine && o.popover_closed() {
                        closed = Some(o.id);
                    }
                }
            });
            if let Some(id) = closed {
                let reason = POPOVER_CLOSE_REASON.replace(prism_core::DismissReason::Unknown);
                dispatch(UiEvent::Dismiss(id, reason));
            }
        }
    }

    unsafe impl NSWindowDelegate for Target {
        /// Keys held when the window stops being key get no key-up (#159).
        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _n: &NSNotification) {
            release_all_keys();
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, n: &NSNotification) {
            place_traffic_lights(n);
            with_state(State::relayout);
        }

        // --- Custom window chrome (#67): AppKit puts the traffic lights
        // back after these. ---

        #[unsafe(method(windowDidEndLiveResize:))]
        fn window_did_end_live_resize(&self, n: &NSNotification) {
            place_traffic_lights(n);
        }

        #[unsafe(method(windowWillEnterFullScreen:))]
        fn window_will_enter_full_screen(&self, _n: &NSNotification) {
            crate::chrome::set_fullscreen(true);
            with_state(State::relayout);
        }

        /// Takes the window, not a notification.
        #[unsafe(method(windowDidFailToEnterFullScreen:))]
        fn window_did_fail_to_enter_full_screen(&self, window: &NSWindow) {
            crate::chrome::set_fullscreen(false);
            crate::chrome::place_traffic_lights(window);
            with_state(State::relayout);
        }

        #[unsafe(method(windowWillExitFullScreen:))]
        fn window_will_exit_full_screen(&self, _n: &NSNotification) {
            crate::chrome::set_fullscreen(false);
            with_state(State::relayout);
        }

        #[unsafe(method(windowDidExitFullScreen:))]
        fn window_did_exit_full_screen(&self, n: &NSNotification) {
            place_traffic_lights(n);
            with_state(State::relayout);
        }

        #[unsafe(method(windowDidChangeScreen:))]
        fn window_did_change_screen(&self, _n: &NSNotification) {
            with_state(|s| {
                s.device_size = None;
                s.relayout();
            });
        }

        /// With unsaved changes, closing asks first (#158).
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, window: &NSWindow) -> objc2::runtime::Bool {
            if !UNSAVED.get() {
                return objc2::runtime::Bool::YES;
            }
            let w = objc2::Message::retain(window);
            confirm_leave(window, move || w.close());
            objc2::runtime::Bool::NO
        }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _n: &NSNotification) {
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }
    }
);

/// The window of a window notification takes its custom traffic lights'
/// place again (#67).
fn place_traffic_lights(n: &NSNotification) {
    if let Some(window) = n.object().and_then(|o| o.downcast::<NSWindow>().ok()) {
        crate::chrome::place_traffic_lights(&window);
    }
}

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// Widgets implementation
// ---------------------------------------------------------------------------

const HEADER_HEIGHT: f64 = 44.;

/// A navigation stack: a native header bar (Back button + title) above the
/// top screen. The screens below stay in the window, hidden, keeping their
/// state (see `AppKit::show_only`).
pub struct MacStack {
    id: NodeId,
    container: Retained<PrismView>,
    header: Retained<PrismView>,
    separator: Retained<PrismView>,
    back: Retained<NSButton>,
    title: Retained<NSTextField>,
    screens: RefCell<Vec<View>>,
    /// Shows "Close" on the first screen (stacks inside sheets).
    closable: Cell<bool>,
    // --- Hidden header (#8) ---
    /// No header bar at all (`Router::header(false)`).
    no_header: Cell<bool>,
    // --- Bar buttons (#30) ---
    /// The top screen's bar buttons, in the header.
    bar: crate::navbar::Bar,
}

/// A sheet window presenting a modal stack over the main window.
pub struct MacSheet {
    window: Retained<NSWindow>,
    content: Retained<PrismView>,
    presented: Cell<bool>,
}

/// A tab bar: a segmented control above the selected tab. The other tabs
/// stay in the window, hidden (see `AppKit::show_only`).
pub struct MacTabs {
    id: NodeId,
    container: Retained<PrismView>,
    bar: Retained<PrismView>,
    separator: Retained<PrismView>,
    segments: Retained<NSSegmentedControl>,
    tabs: RefCell<Vec<View>>,
    selected: Cell<usize>,
}

/// A scroll view; Prism children live in its flipped document view.
pub struct MacScroll {
    scroll: Retained<PrismScrollView>,
    document: Retained<PrismView>,
}

#[derive(Default)]
pub struct ScrollIvars {
    /// `Scrollbar::Overlay` (#132): overlay scrollers whatever the « Show
    /// scroll bars » setting says.
    overlay: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSScrollView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismScrollView"]
    #[ivars = ScrollIvars]
    pub struct PrismScrollView;

    impl PrismScrollView {
        /// AppKit sets every scroll view's style again when the user
        /// changes the setting or plugs a mouse in: an overlay one stays.
        #[unsafe(method(setScrollerStyle:))]
        fn set_scroller_style(&self, style: objc2_app_kit::NSScrollerStyle) {
            let style = if self.ivars().overlay.get() {
                objc2_app_kit::NSScrollerStyle::Overlay
            } else {
                style
            };
            let _: () = unsafe { msg_send![super(self), setScrollerStyle: style] };
        }

        /// A pull past the top refreshes (`crate::refresh`).
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let _: () = unsafe { msg_send![super(self), scrollWheel: event] };
            if let Some(doc) = self.documentView().and_then(|d| d.downcast::<PrismView>().ok()) {
                crate::refresh::scroll_wheel(node_id(&doc), self, event);
            }
        }
    }
);

impl PrismScrollView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ScrollIvars::default());
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    /// How the scrollers show (#132), for a view scrolling `sideways` or
    /// not. Legacy (always shown) scrollers take room after layout, so a
    /// row never gets one; overlay ones take none, so it may.
    fn set_scrollbar(&self, bar: prism_core::Scrollbar, sideways: bool) {
        use prism_core::Scrollbar;
        let overlay = bar == Scrollbar::Overlay;
        self.ivars().overlay.set(overlay);
        self.setScrollerStyle(if overlay {
            objc2_app_kit::NSScrollerStyle::Overlay
        } else {
            objc2_app_kit::NSScroller::preferredScrollerStyle(MainThreadMarker::from(self))
        });
        let shown = bar != Scrollbar::Hidden;
        self.setHasVerticalScroller(shown && !sideways);
        self.setHasHorizontalScroller(overlay && sideways);
    }
}

/// A text input. Secure entry is a different class (`NSSecureTextField`,
/// whose field editor only reports edits to it), so the field sits in a
/// host view that can swap it when `secure` changes.
pub struct MacInput {
    id: NodeId,
    host: Retained<PrismView>,
    field: RefCell<Retained<NSTextField>>,
    /// The leading icon (an SF Symbol), inside the field.
    icon: RefCell<(String, Option<Retained<NSImageView>>)>,
}

impl MacInput {
    fn field(&self) -> Retained<NSTextField> {
        self.field.borrow().clone()
    }
}

/// A pop-up button and what it lists. A placeholder item (tag -1) leads the
/// menu while nothing is selected.
pub struct MacPicker {
    id: NodeId,
    button: Retained<NSPopUpButton>,
    options: RefCell<Vec<String>>,
    placeholder: RefCell<String>,
    selected: Cell<isize>,
}

impl MacPicker {
    fn rebuild(&self) {
        let b = &self.button;
        b.removeAllItems();
        let options = self.options.borrow();
        let selected = self.selected.get();
        let none = selected < 0 || selected as usize >= options.len();
        // `addItemWithTitle` drops items with the same title: add each under
        // a unique title, then rename it.
        let add = |title: &str, tag: isize| {
            b.addItemWithTitle(&NSString::from_str(&format!("\u{1}{tag}")));
            if let Some(item) = b.lastItem() {
                item.setTitle(&NSString::from_str(title));
                item.setTag(tag);
            }
        };
        if none {
            add(&self.placeholder.borrow(), -1);
        }
        for (i, o) in options.iter().enumerate() {
            add(o, i as isize);
        }
        b.selectItemAtIndex(if none { 0 } else { selected });
    }
}

/// A text area: an NSTextView in a scroll view, inside a Prism view that
/// paints the background and border. The text view runs on TextKit 1 (its
/// layout manager measures the content height).
pub struct MacTextArea {
    container: Retained<PrismView>,
    scroll: Retained<NSScrollView>,
    text: Retained<PrismTextView>,
    /// Owns the TextKit network (layout manager, container) of `text`.
    _storage: Retained<objc2_app_kit::NSTextStorage>,
}

impl MacTextArea {
    /// The border drawn by the container (the scroll view sits inside it).
    fn border(&self) -> f64 {
        let iv = self.container.ivars();
        if iv.border_color.borrow().is_some() {
            iv.border.get()
        } else {
            0.
        }
    }
}

#[derive(Clone)]
pub enum View {
    Container(Retained<PrismView>),
    Screen(Retained<PrismView>),
    Tab(Retained<PrismView>),
    Stack(Rc<MacStack>),
    Tabs(Rc<MacTabs>),
    Scroll(Rc<MacScroll>),
    Sheet(Rc<MacSheet>),
    Toggle(Retained<NSSwitch>),
    Image(Retained<NSImageView>),
    Progress(Retained<NSProgressIndicator>),
    Timer(Retained<NSTextField>),
    Picture(Retained<PrismView>),
    Label(Retained<crate::label::PrismLabel>),
    Button(Retained<NSButton>),
    Input(Rc<MacInput>),
    Slider(Retained<PrismSlider>),
    Picker(Rc<MacPicker>),
    DatePicker(Retained<NSDatePicker>),
    TextArea(Rc<MacTextArea>),
    Spinner(Retained<NSProgressIndicator>),
    // Overlays and choice controls (#18, #19).
    Overlay(Rc<MacOverlay>),
    Check(Retained<NSButton>),
    Radios(Rc<MacRadios>),
    /// A web view (#62): a Prism view (its box) holding the WKWebView host.
    Web(Rc<MacWeb>),
    /// A video (#207): its box, and the bridge's AVKit player filling it.
    Video(Rc<MacVideo>),
}

/// A video: its box and the bridge's player host (`None` without AVKit).
pub struct MacVideo {
    container: Retained<PrismView>,
    host: Option<prism_native::apple::video::VideoHost>,
}

/// A web view: its box, and the bridge's host view filling it (`None`
/// without WebKit).
pub struct MacWeb {
    container: Retained<PrismView>,
    host: Option<prism_native::apple::web::WebHost>,
}

impl View {
    fn ns(&self) -> &NSView {
        match self {
            View::Container(v) | View::Screen(v) | View::Tab(v) => v,
            View::Stack(s) => &s.container,
            View::Tabs(t) => &t.container,
            View::Scroll(s) => &s.scroll,
            View::Sheet(s) => &s.content,
            View::Toggle(t) => t,
            View::Image(v) => v,
            View::Progress(v) => v,
            View::Timer(v) => v,
            View::Picture(v) => v,
            View::Label(v) => v,
            View::Input(i) => &i.host,
            View::Button(v) => v,
            View::Slider(v) => v,
            View::Picker(p) => &p.button,
            View::DatePicker(v) => v,
            View::TextArea(t) => &t.container,
            View::Spinner(v) => v,
            View::Overlay(o) => &o.anchor,
            View::Check(v) => v,
            View::Radios(r) => &r.container,
            View::Web(w) => &w.container,
            View::Video(v) => &v.container,
        }
    }

    /// The Prism view drawing this node's box, if any.
    fn prism_view(&self) -> Option<&Retained<PrismView>> {
        match self {
            View::Container(v) | View::Screen(v) | View::Tab(v) | View::Picture(v) => Some(v),
            View::Web(w) => Some(&w.container),
            View::Video(v) => Some(&v.container),
            View::TextArea(t) => Some(&t.container),
            View::Input(i) => Some(&i.host),
            _ => None,
        }
    }

    fn id(&self) -> NodeId {
        match self {
            View::Container(v) | View::Screen(v) | View::Tab(v) => v.ivars().id.get(),
            View::Stack(s) => s.id,
            View::Tabs(t) => t.id,
            View::Scroll(s) => s.document.ivars().id.get(),
            View::Sheet(s) => s.content.ivars().id.get(),
            View::Toggle(t) => t.tag() as NodeId,
            View::Image(v) => v.tag() as NodeId,
            // Progress bars need no id (no events, no per-node state).
            View::Progress(_) => 0,
            View::Timer(v) => v.tag() as NodeId,
            View::Picture(v) => v.ivars().id.get(),
            View::Label(v) => v.tag() as NodeId,
            View::Input(i) => i.id,
            View::Button(v) => v.tag() as NodeId,
            View::Slider(v) => v.tag() as NodeId,
            View::Picker(p) => p.id,
            View::DatePicker(v) => v.tag() as NodeId,
            View::TextArea(t) => t.container.ivars().id.get(),
            // `NSView.tag` is read-only: the id is the view's identifier.
            View::Spinner(v) => v
                .identifier()
                .and_then(|i| i.to_string().parse().ok())
                .unwrap_or(0),
            View::Overlay(o) => o.id,
            View::Check(v) => v.tag() as NodeId,
            View::Radios(r) => r.id,
            View::Web(w) => w.container.ivars().id.get(),
            View::Video(v) => v.container.ivars().id.get(),
        }
    }
}

pub struct AppKit {
    mtm: MainThreadMarker,
    target: Retained<Target>,
    root: Retained<PrismView>,
    stacks: Vec<Rc<MacStack>>,
    tab_bars: Vec<Rc<MacTabs>>,
    titles: HashMap<NodeId, String>,
    /// The app's title (`cx.set_title`, #154): the window's while set, over
    /// the top screen's; and the window's own (the app's name).
    app_title: Option<String>,
    window_title: String,
    icons: HashMap<NodeId, String>,
    /// Countdown labels and their end time (Unix seconds).
    timers: HashMap<NodeId, (Retained<NSTextField>, f64)>,
    /// What each countdown shows once ended (`timer(..).after`, #281).
    timer_after: HashMap<NodeId, String>,
    /// Presented sheets.
    sheets: Vec<Rc<MacSheet>>,
    /// Pictures waiting for their image, by source.
    pictures: HashMap<String, Vec<(NodeId, Retained<PrismView>)>>,
    /// Decoded images by source.
    image_cache: HashMap<String, Retained<NSImage>>,
    image_sources: prism_native::images::ImageSources,
    /// Right-to-left language: header buttons swap sides.
    rtl: bool,
    /// Header button texts: (back, close), in the app's language.
    chrome: (String, String),
    /// Visual transforms (offset, scale, rotation), applied around the center.
    transforms: HashMap<NodeId, prism_core::Transform>,
    /// Text input behaviour, read when a field starts editing.
    inputs: HashMap<NodeId, InputConfig>,
    /// Last value of each status region (announced when it changes).
    announced: HashMap<NodeId, String>,
    /// App language (BCP 47): date pickers format in it.
    lang: String,
    // --- Hidden header (#8) ---
    /// Screens that hide their stack's header while on top (`.bare()`).
    bare: std::collections::HashSet<NodeId>,
    /// Tabs and stacked screens the app hides (`display_none`), even when
    /// selected or on top.
    hidden_by_app: std::collections::HashSet<NodeId>,
    /// Hover tracking areas, by node (`set_interactions`).
    hover_areas: HashMap<NodeId, Retained<objc2_app_kit::NSTrackingArea>>,
    /// Keys, overlays, layers, suggestions (#17, #19, #25).
    ix: crate::interact::Interaction,
    /// The snapshot following a dragged payload (#251).
    drag_preview: Option<Retained<NSImageView>>,
}

fn ca_equal(a: objc2_quartz_core::CATransform3D, b: objc2_quartz_core::CATransform3D) -> bool {
    let f = |t: objc2_quartz_core::CATransform3D| [t.m11, t.m12, t.m21, t.m22, t.m41, t.m42];
    f(a) == f(b)
}

/// The node a Prism view shows.
pub(crate) fn node_id(view: &PrismView) -> NodeId {
    view.ivars().id.get()
}

impl AppKit {
    /// Shadows, gradient, borders, clipping, blur and cursor of a Prism view.
    /// A Prism view's box: background, border, corners, decorations.
    fn style_box(&mut self, v: &Retained<PrismView>, style: &Style) {
        let iv = v.ivars();
        *iv.background.borrow_mut() = style.background.map(ns_color);
        *iv.border_color.borrow_mut() = style.border_color.map(ns_color);
        iv.radius.set(style.corner_radius as f64);
        iv.corners.set(style.rounded_corners);
        iv.border.set(style.border_width as f64);
        self.apply_decor(v, style);
        v.setNeedsDisplay(true);
    }

    fn apply_decor(&self, v: &PrismView, style: &Style) {
        let decor = crate::decor::Decor::from_style(style);
        let mut st = v.ivars().decor.borrow_mut();
        if st.decor == decor {
            return;
        }
        let cursor_changed = st.decor.cursor != decor.cursor;
        st.decor = decor;
        crate::decor::set_blur(v, &mut st, self.mtm);
        drop(st);
        if cursor_changed && let Some(w) = v.window() {
            w.invalidateCursorRectsForView(v);
        }
    }

    /// The view's transform around its center (AppKit layers turn around
    /// their corner).
    fn apply_transform(&self, view: &View) {
        let v = view.ns();
        let t = self.transforms.get(&view.id()).copied().unwrap_or_default();
        v.setWantsLayer(true);
        if let Some(layer) = v.layer() {
            let size = v.bounds().size;
            layer.setAffineTransform(prism_native::apple::motion::matrix(
                t.matrix_around_center(size.width as f32, size.height as f32),
            ));
        }
    }

    /// Shows `src` in `view`: from memory, or loaded on a background thread.
    fn load_picture(&mut self, id: NodeId, view: &Retained<PrismView>, src: &str) {
        if let Some(img) = self.image_cache.get(src) {
            *view.ivars().image.borrow_mut() = Some(img.clone());
            view.setNeedsDisplay(true);
            return;
        }
        *view.ivars().image.borrow_mut() = None;
        view.setNeedsDisplay(true);
        let waiting = self.pictures.entry(src.to_string()).or_default();
        waiting.retain(|(i, _)| *i != id);
        waiting.push((id, view.clone()));
        if waiting.len() > 1 {
            return; // already loading
        }
        let (src, sources) = (src.to_string(), self.image_sources.clone());
        std::thread::spawn(move || {
            let bytes = prism_native::images::load(&src, &sources);
            dispatch2::DispatchQueue::main().exec_async(move || {
                with_state(|s| {
                    s.renderer.widgets.picture_loaded(&src, bytes);
                    s.renderer.mark_all_dirty();
                    s.relayout();
                })
            });
        });
    }

    fn picture_loaded(&mut self, src: &str, bytes: Option<Vec<u8>>) {
        let waiting = self.pictures.remove(src).unwrap_or_default();
        let data = bytes.map(objc2_foundation::NSData::from_vec);
        let Some(img) =
            data.and_then(|d| NSImage::initWithData(objc2::AllocAnyThread::alloc(), &d))
        else {
            eprintln!(
                "prism-macos: could not load image {:?}",
                prism_core::picture_source(src).0
            );
            return;
        };
        for (_, view) in waiting {
            *view.ivars().image.borrow_mut() = Some(img.clone());
            view.setNeedsDisplay(true);
        }
        self.image_cache.insert(src.to_string(), img);
    }

    fn tick(&self) {
        for (id, (label, end)) in &self.timers {
            let after = self.timer_after.get(id).map_or("", String::as_str);
            let text = prism_core::timer_text(*end, after);
            if label.stringValue().to_string() != text {
                label.setStringValue(&NSString::from_str(&text));
            }
        }
    }
}

/// Mode and bounds. Days get a text field that opens a calendar on click
/// (the compact style); times a text field with a stepper.
fn configure_date_picker(d: &NSDatePicker, c: DateConfig) {
    let (style, elements) = match c.mode {
        DateMode::Date => (
            NSDatePickerStyle::TextField,
            NSDatePickerElementFlags::YearMonthDay,
        ),
        DateMode::Time | DateMode::TimeOfDay => (
            NSDatePickerStyle::TextFieldAndStepper,
            NSDatePickerElementFlags::HourMinute,
        ),
        DateMode::DateTime => (
            NSDatePickerStyle::TextField,
            NSDatePickerElementFlags::YearMonthDay | NSDatePickerElementFlags::HourMinute,
        ),
    };
    d.setDatePickerStyle(style);
    d.setDatePickerElements(elements);
    d.setPresentsCalendarOverlay(style == NSDatePickerStyle::TextField);
    // A time of day is the same everywhere: seconds since midnight, shown in UTC.
    let utc = objc2_foundation::NSTimeZone::timeZoneForSecondsFromGMT(0);
    d.setTimeZone((c.mode == DateMode::TimeOfDay).then_some(&*utc));
    let date = |v: Option<f64>| v.map(objc2_foundation::NSDate::dateWithTimeIntervalSince1970);
    d.setMinDate(date(c.min).as_deref());
    d.setMaxDate(date(c.max).as_deref());
}

impl AppKit {
    /// Controls mirror in right-to-left languages (a slider fills from the
    /// right, a pop-up's chevron moves left…). `userInterfaceLayoutDirection`
    /// is per view in AppKit, not inherited.
    fn directional(&self, v: &NSView) {
        v.setUserInterfaceLayoutDirection(if self.rtl {
            objc2_app_kit::NSUserInterfaceLayoutDirection::RightToLeft
        } else {
            objc2_app_kit::NSUserInterfaceLayoutDirection::LeftToRight
        });
    }

    /// A status region's value changed: VoiceOver reads it out (an
    /// announcement request posted for the application element, with a
    /// priority, as NSAccessibilityConstants.h asks).
    fn announce(&mut self, id: NodeId, value: &str) {
        let previous = self.announced.insert(id, value.to_string());
        if value.is_empty() || previous.as_deref() == Some(value) {
            return;
        }
        if std::env::var_os("PRISM_A11Y_LOG").is_some() {
            println!("prism-macos: announcement: {value:?}");
        }
        use objc2_app_kit::{
            NSAccessibilityAnnouncementKey, NSAccessibilityAnnouncementRequestedNotification,
            NSAccessibilityPostNotificationWithUserInfo, NSAccessibilityPriorityKey,
            NSAccessibilityPriorityLevel,
        };
        let message = NSString::from_str(value);
        let priority =
            objc2_foundation::NSNumber::new_isize(NSAccessibilityPriorityLevel::Medium.0);
        let (m, p): (&AnyObject, &AnyObject) = (&message, &priority);
        let app = NSApplication::sharedApplication(self.mtm);
        unsafe {
            let info = objc2_foundation::NSDictionary::from_slices(
                &[NSAccessibilityAnnouncementKey, NSAccessibilityPriorityKey],
                &[m, p],
            );
            NSAccessibilityPostNotificationWithUserInfo(
                &app,
                NSAccessibilityAnnouncementRequestedNotification,
                Some(&info),
            );
        }
    }

    /// Applies the direction to the form controls under `v`.
    fn redirect_controls(&self, v: &NSView) {
        let control = v.isKindOfClass(NSSlider::class())
            || v.isKindOfClass(NSPopUpButton::class())
            || v.isKindOfClass(NSDatePicker::class())
            || v.isKindOfClass(PrismTextView::class())
            || v.downcast_ref::<NSScrollView>()
                .and_then(|s| s.documentView())
                .is_some_and(|d| d.isKindOfClass(PrismTextView::class()));
        if control {
            self.directional(v);
        }
        for sub in v.subviews().iter() {
            self.redirect_controls(&sub);
        }
    }

    /// Dates read in the app's language (Arabic digits in `ar`…), in the
    /// device's time zone.
    fn set_date_locale(&self, d: &NSDatePicker) {
        if !self.lang.is_empty() {
            let id = NSString::from_str(&self.lang);
            let locale = objc2_foundation::NSLocale::localeWithLocaleIdentifier(&id);
            d.setLocale(Some(&locale));
        }
    }

    fn new_text_area(&self, id: NodeId) -> MacTextArea {
        use objc2_app_kit::{NSLayoutManager, NSTextContainer, NSTextStorage};
        let mtm = self.mtm;
        let storage = NSTextStorage::new();
        let layout = NSLayoutManager::new();
        // Laid out like a label's text (`NSStringDrawing`'s typesetter), so
        // its lines sit where `text()` draws them (#179).
        layout.setTypesetterBehavior(
            objc2_app_kit::NSTypesetterBehavior::Behavior_10_2_WithCompatibility,
        );
        storage.addLayoutManager(&layout);
        let tc = NSTextContainer::initWithSize(
            objc2::AllocAnyThread::alloc(),
            NSSize::new(0., f64::MAX / 4.),
        );
        tc.setWidthTracksTextView(true);
        // The line fragment padding of a label's cell (2 pt, not the text
        // view's 5, #179): with an explicit `p(0.)` the text starts where
        // `text()` draws it.
        tc.setLineFragmentPadding(LABEL_FRAGMENT_PADDING);
        layout.addTextContainer(&tc);
        let this = PrismTextView::alloc(mtm).set_ivars(TextViewIvars {
            id: Cell::new(id),
            ..Default::default()
        });
        let text: Retained<PrismTextView> = unsafe {
            msg_send![super(this), initWithFrame: NSRect::ZERO, textContainer: Some(&*tc)]
        };
        text.setRichText(false);
        text.setImportsGraphics(false);
        text.setAllowsUndo(true);
        text.setDrawsBackground(false);
        text.setVerticallyResizable(true);
        text.setHorizontallyResizable(false);
        text.setMinSize(NSSize::ZERO);
        text.setMaxSize(NSSize::new(f64::MAX / 4., f64::MAX / 4.));
        text.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
        text.setTextContainerInset(TEXT_AREA_INSET);
        text.setTextColor(Some(&NSColor::labelColor()));
        // Default `autocorrect`: as the user set it in System Settings.
        set_text_checking(&text, true);
        // The delegate is weak; `self.target` outlives every view.
        text.setDelegate(Some(ProtocolObject::from_ref(&*self.target)));
        let scroll = NSScrollView::new(mtm);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        // Not under a title bar: no automatic top inset.
        scroll.setAutomaticallyAdjustsContentInsets(false);
        scroll.setDocumentView(Some(&text));
        self.directional(&scroll);
        self.directional(&text);
        let container = PrismView::new(mtm, id);
        container.setSubviews(&NSArray::from_slice(&[&*scroll as &NSView]));
        MacTextArea {
            container,
            scroll,
            text,
            _storage: storage,
        }
    }

    /// A text field (an `NSSecureTextField` showing bullets when `secure`)
    /// reporting edits, and Return through its action.
    fn new_text_field(&self, id: NodeId, secure: bool) -> Retained<NSTextField> {
        let empty = NSString::new();
        let f: Retained<NSTextField> = if secure {
            // `+textFieldWithString:` (inherited class method) on the subclass.
            let cls = objc2_app_kit::NSSecureTextField::class();
            let s: Retained<objc2_app_kit::NSSecureTextField> =
                unsafe { msg_send![cls, textFieldWithString: &*empty] };
            Retained::into_super(s)
        } else {
            NSTextField::textFieldWithString(&empty, self.mtm)
        };
        f.setTag(id as isize);
        // SAFETY: the delegate is weak; `self.target` outlives every view.
        unsafe { f.setDelegate(Some(ProtocolObject::from_ref(&*self.target))) };
        // The action is sent on Return only (not when focus leaves).
        let target: &AnyObject = &self.target;
        unsafe {
            f.setTarget(Some(target));
            f.setAction(Some(sel!(onSubmit:)));
        }
        if let Some(cell) = f.cell() {
            cell.setSendsActionOnEndEditing(false);
        }
        if secure {
            let password = unsafe { objc2_app_kit::NSTextContentTypePassword };
            objc2_app_kit::NSTextContent::setContentType(&*f, Some(password));
        }
        f
    }

    /// Swaps the field for a secure (or plain) one, keeping its text, look
    /// and accessibility.
    fn set_secure(&self, input: &MacInput, secure: bool) {
        let old = input.field();
        let is_secure = old.isKindOfClass(objc2_app_kit::NSSecureTextField::class());
        if is_secure == secure {
            return;
        }
        let f = self.new_text_field(input.id, secure);
        f.setStringValue(&old.stringValue());
        f.setPlaceholderString(old.placeholderString().as_deref());
        f.setFont(old.font().as_deref());
        f.setTextColor(old.textColor().as_deref());
        f.setAlignment(old.alignment());
        f.setAccessibilityLabel(old.accessibilityLabel().as_deref());
        f.setAccessibilityHelp(old.accessibilityHelp().as_deref());
        f.setEnabled(old.isEnabled());
        f.setBezeled(old.isBezeled());
        f.setDrawsBackground(old.drawsBackground());
        f.setFrame(old.frame());
        unsafe { old.setDelegate(None) };
        input.host.replaceSubview_with(&old, &f);
        *input.field.borrow_mut() = f;
        // The new field gets the icon again (secure ones have none).
        let name = input.icon.borrow().0.clone();
        let icon = crate::interact::set_leading_icon(&input.field(), &name, None);
        input.icon.borrow_mut().1 = icon;
        crate::carets::input_swapped(input.id, &input.host, &input.field());
    }

    fn new_stack(&self, id: NodeId) -> MacStack {
        let mtm = self.mtm;
        let header = PrismView::new(mtm, 0);
        *header.ivars().background.borrow_mut() = Some(NSColor::windowBackgroundColor());
        header.ivars().top_bar.set(true);
        let separator = PrismView::new(mtm, 0);
        *separator.ivars().background.borrow_mut() = Some(NSColor::separatorColor());
        let target: &AnyObject = &self.target;
        let back = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("‹ Back"),
                Some(target),
                Some(sel!(onBack:)),
                mtm,
            )
        };
        back.setTag(id as isize);
        let title = NSTextField::labelWithString(&NSString::new(), mtm);
        title.setFont(Some(&NSFont::systemFontOfSize_weight(13., unsafe {
            NSFontWeightSemibold
        })));
        title.setAlignment(NSTextAlignment::Center);
        header.setSubviews(&NSArray::from_slice(&[&*title as &NSView, &back]));
        MacStack {
            id,
            container: PrismView::new(mtm, id),
            header,
            separator,
            back,
            title,
            screens: RefCell::new(Vec::new()),
            closable: Cell::new(false),
            no_header: Cell::new(false),
            bar: Default::default(),
        }
    }

    fn new_tabs(&self, id: NodeId) -> MacTabs {
        let mtm = self.mtm;
        let bar = PrismView::new(mtm, 0);
        *bar.ivars().background.borrow_mut() = Some(NSColor::windowBackgroundColor());
        bar.ivars().top_bar.set(true);
        let separator = PrismView::new(mtm, 0);
        *separator.ivars().background.borrow_mut() = Some(NSColor::separatorColor());
        let segments = NSSegmentedControl::new(mtm);
        segments.setTag(id as isize);
        let target: &AnyObject = &self.target;
        unsafe {
            segments.setTarget(Some(target));
            segments.setAction(Some(sel!(onSegment:)));
        }
        bar.setSubviews(&NSArray::from_slice(&[&*segments as &NSView]));
        MacTabs {
            id,
            container: PrismView::new(mtm, id),
            bar,
            separator,
            segments,
            tabs: RefCell::new(Vec::new()),
            selected: Cell::new(0),
        }
    }

    /// Syncs segments (titles, SF Symbols) and shows the selected tab.
    fn refresh_tabs(&self, bar: &MacTabs) {
        let tabs = bar.tabs.borrow();
        bar.segments.setSegmentCount(tabs.len() as isize);
        for (i, t) in tabs.iter().enumerate() {
            let title = self.titles.get(&t.id()).map(String::as_str).unwrap_or("");
            bar.segments
                .setLabel_forSegment(&NSString::from_str(title), i as isize);
            let image = self.icons.get(&t.id()).and_then(|icon| {
                NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(icon),
                    None,
                )
            });
            bar.segments
                .setImage_forSegment(image.as_deref(), i as isize);
        }
        let selected = bar.selected.get().min(tabs.len().saturating_sub(1));
        bar.segments.setSelectedSegment(selected as isize);
        // The selected tab's symbol in the tint (#30).
        let icon = tabs.get(selected).and_then(|t| self.icons.get(&t.id()));
        crate::navbar::tint_selected_segment(bar.id, &bar.segments, selected, icon);
        self.show_only(&bar.container, &[&bar.bar, &bar.separator], &tabs, selected);
    }

    /// Puts `fixed` then `views` in `container` and shows only `views[shown]`
    /// (unless the app hides it): the others stay in the window, hidden.
    /// Detaching a tree of hundreds of views and attaching it again (a tab
    /// switch, Back) made AppKit draw every layer of it again.
    fn show_only(&self, container: &NSView, fixed: &[&NSView], views: &[View], shown: usize) {
        let mut subviews = fixed.to_vec();
        subviews.extend(views.iter().map(View::ns));
        let current = container.subviews();
        if current.len() != subviews.len() || current.iter().zip(&subviews).any(|(a, b)| &*a != *b)
        {
            container.setSubviews(&NSArray::from_slice(&subviews));
        }
        let window = container.window();
        for (i, view) in views.iter().enumerate() {
            let hidden = i != shown || self.hidden_by_app.contains(&view.id());
            let v = view.ns();
            // Like a detached view, a hidden one gives up the focus.
            if hidden
                && !v.isHidden()
                && let Some(w) = &window
                && w.firstResponder()
                    .and_then(|r| r.downcast::<NSView>().ok())
                    .is_some_and(|r| r.isDescendantOf(v))
            {
                w.makeFirstResponder(None);
            }
            v.setHidden(hidden);
        }
    }

    fn present_sheets(&mut self, sheets: &[View]) {
        for s in sheets {
            if let View::Sheet(sheet) = s
                && !sheet.presented.get()
                && let Some(main) = self.root.window()
            {
                sheet.presented.set(true);
                main.beginSheet_completionHandler(&sheet.window, None);
                sheet.window.makeFirstResponder(Some(&sheet.content));
                self.sheets.push(sheet.clone());
            }
        }
        // Sheets no longer listed are dismissed.
        let keep: Vec<NodeId> = sheets.iter().map(View::id).collect();
        let gone: Vec<Rc<MacSheet>> = self
            .sheets
            .iter()
            .filter(|s| !keep.contains(&s.content.ivars().id.get()))
            .cloned()
            .collect();
        for sheet in gone {
            self.dismiss_sheet(&sheet);
        }
    }

    fn dismiss_sheet(&mut self, sheet: &MacSheet) {
        if sheet.presented.replace(false)
            && let Some(main) = self.root.window()
        {
            main.endSheet(&sheet.window);
            sheet.window.orderOut(None);
        }
        let id = sheet.content.ivars().id.get();
        self.sheets.retain(|s| s.content.ivars().id.get() != id);
    }

    /// Whether `stack` shows no header bar: none at all, or a bare screen on top.
    fn header_hidden(&self, stack: &MacStack) -> bool {
        stack.no_header.get()
            || stack
                .screens
                .borrow()
                .last()
                .is_some_and(|top| self.bare.contains(&top.id()))
    }

    /// Places a tab bar's segments: centered, clear of the traffic lights
    /// over it (#67, `chrome::bar_room`).
    fn place_tabs(&self, t: &MacTabs) {
        let w = t.bar.frame().size.width;
        let (left, right) = crate::chrome::bar_room(&t.bar);
        let sz = t.segments.fittingSize();
        // Narrower than the segments: they shrink (their titles truncate).
        let width = sz.width.min((w - left - right - 16.).max(0.));
        let x = ((w - width) / 2.)
            .min(w - right - 8. - width)
            .max(left + 8.)
            .round();
        t.segments.setFrame(NSRect::new(
            NSPoint::new(x, ((HEADER_HEIGHT - 1. - sz.height) / 2.).round()),
            NSSize::new(width, sz.height),
        ));
    }

    /// Places a header's Back, title and bar buttons, clear of the traffic
    /// lights over it (#67, `chrome::bar_room`): Back at the leading end,
    /// after them; the title centered in the window.
    fn place_header(&self, st: &MacStack) {
        let size = st.header.frame().size;
        let (w, h) = (size.width, size.height);
        let room = crate::chrome::bar_room(&st.header);
        let b = st.back.fittingSize();
        st.back.setFrame(NSRect::new(
            NSPoint::new(
                if self.rtl {
                    w - 8. - room.1 - b.width
                } else {
                    8. + room.0
                },
                ((h - b.height) / 2.).round(),
            ),
            b,
        ));
        let t = st.title.fittingSize();
        let side = room.0.max(room.1);
        st.title.setFrame(NSRect::new(
            NSPoint::new(side, ((h - t.height) / 2.).round()),
            NSSize::new((w - 2. * side).max(0.), t.height),
        ));
        crate::navbar::layout(&st.bar, &st.back, w, h, self.rtl, room);
    }

    /// The bars Prism draws: tab bars and navigation headers.
    fn top_bars(&self) -> impl Iterator<Item = &NSView> {
        self.stacks
            .iter()
            .map(|st| &**st.header as &NSView)
            .chain(self.tab_bars.iter().map(|t| &**t.bar as &NSView))
    }

    /// Every bar's contents again, after a layout: the traffic lights over
    /// one came or went (full screen, a bar moved to the top edge or away).
    fn place_bars(&self) {
        for t in &self.tab_bars {
            self.place_tabs(t);
        }
        for st in &self.stacks {
            self.place_header(st);
        }
    }

    /// Shows the top screen, its title, and Back when there is somewhere to go.
    fn refresh_stack(&self, stack: &MacStack) {
        let hidden = self.header_hidden(stack);
        let screens = stack.screens.borrow();
        let header: &[&NSView] = if hidden {
            &[]
        } else {
            &[&stack.header, &stack.separator]
        };
        let top = screens.len().saturating_sub(1);
        self.show_only(&stack.container, header, &screens, top);
        let close = stack.closable.get() && screens.len() <= 1;
        stack.back.setHidden(screens.len() <= 1 && !close);
        let label = if close {
            self.chrome.1.clone()
        } else if self.rtl {
            format!("{} ›", self.chrome.0)
        } else {
            format!("‹ {}", self.chrome.0)
        };
        stack.back.setTitle(&NSString::from_str(&label));
        let title = screens
            .last()
            .and_then(|t| self.titles.get(&t.id()))
            .map(String::as_str)
            .unwrap_or("");
        let title = NSString::from_str(title);
        stack.title.setStringValue(&title);
        // The top screen's bar buttons (#30).
        let top = screens.last().map(View::id);
        crate::navbar::refresh(self.mtm, stack.id, &stack.header, &stack.bar, top);
        self.place_header(stack);
        if let Some(window) = stack.container.window()
            && title.length() > 0
            && self.app_title.is_none()
        {
            window.setTitle(&title);
        }
    }
}

fn plain_color(c: Color) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(c.r as f64, c.g as f64, c.b as f64, c.a as f64)
}

/// Adaptive colors become dynamic `NSColor`s, resolved per appearance.
pub(crate) fn ns_color(c: Color) -> Retained<NSColor> {
    if c.dark.is_none() {
        return plain_color(c);
    }
    let (light, dark) = (c.light_variant(), c.dark_variant());
    let provider = block2::RcBlock::new(move |appearance: std::ptr::NonNull<NSAppearance>| {
        let appearance = unsafe { appearance.as_ref() };
        let names = unsafe {
            NSArray::from_slice(&[
                objc2_app_kit::NSAppearanceNameAqua,
                objc2_app_kit::NSAppearanceNameDarkAqua,
            ])
        };
        let is_dark = appearance
            .bestMatchFromAppearancesWithNames(&names)
            .is_some_and(|n| &*n == unsafe { objc2_app_kit::NSAppearanceNameDarkAqua });
        let color = plain_color(if is_dark { dark } else { light });
        std::ptr::NonNull::new(Retained::autorelease_return(color)).expect("color")
    });
    unsafe { NSColor::colorWithName_dynamicProvider(None, &provider) }
}

fn font_weight(style: &Style) -> objc2_app_kit::NSFontWeight {
    crate::label::ns_weight(style.font_weight)
}

/// The font of a button, input or text area: size, any weight, family,
/// italic and tabular digits of its style.
fn font(style: &Style) -> Retained<NSFont> {
    crate::label::ns_font(&prism_native::text::FontSpec {
        family: style.font_family.clone(),
        size: style.font_size,
        weight: style.font_weight,
        italic: style.italic,
        tabular: style.tabular_numbers,
    })
}

fn alignment(a: TextAlign) -> NSTextAlignment {
    match a {
        TextAlign::Left => NSTextAlignment::Left,
        TextAlign::Center => NSTextAlignment::Center,
        TextAlign::Right => NSTextAlignment::Right,
    }
}

/// The horizontal padding `NSTextFieldCell` (`text()`) lays its text out
/// with; a text area's container uses it too (#179).
const LABEL_FRAGMENT_PADDING: f64 = 2.;

/// A text area's default inset: with the fragment padding, the text sits
/// where AppKit's 7 × 8 inset and 5 pt padding put it (#179).
const TEXT_AREA_INSET: NSSize = NSSize::new(10., 8.);

/// A text area's content height for `max_width` (at least three lines),
/// with its insets and border. Taffy caps it at `max_height`; beyond, the
/// text scrolls.
fn text_area_size(t: &MacTextArea, max_width: Option<f32>) -> NSSize {
    let width = max_width.map_or(240., |w| w as f64);
    let border = t.border();
    let inset = t.text.textContainerInset();
    let (lm, tc) = unsafe { (t.text.layoutManager(), t.text.textContainer()) };
    let content = match (lm, tc) {
        (Some(lm), Some(tc)) => {
            // Lay out at the asked width, then give the container back.
            let old = tc.size();
            let w = (width - 2. * inset.width - 2. * border).max(1.);
            tc.setSize(NSSize::new(w, f64::MAX / 4.));
            lm.ensureLayoutForTextContainer(&tc);
            let used = lm.usedRectForTextContainer(&tc).size.height;
            tc.setSize(old);
            let font = t
                .text
                .font()
                .unwrap_or_else(|| NSFont::systemFontOfSize(NSFont::systemFontSize()));
            // A rich input is a paragraph: one line at least (#115). A
            // fixed line height sets the lines' (#179).
            let lines = match (t.text.ivars().rich.get(), t.text.ivars().rows.get()) {
                (true, _) => 1.,
                (false, 0) => 3.,
                // `rows(n)` (#235).
                (false, n) => n as f64,
            };
            let line = t
                .text
                .ivars()
                .style
                .borrow()
                .as_ref()
                .and_then(Style::line_height_points)
                .map_or_else(|| lm.defaultLineHeightForFont(&font), f64::from);
            used.max(lines * line)
        }
        _ => 54.,
    };
    NSSize::new(width, content + 2. * inset.height + 2. * border)
}

impl AppKit {
    /// `cx.set_title` (#154): the window's title while set; back to the
    /// app's name otherwise (the next screen change titles it again).
    fn set_app_title(&mut self, title: Option<String>) {
        self.app_title = title;
        if let Some(window) = self.root.window() {
            let shown = self.app_title.as_deref().unwrap_or(&self.window_title);
            window.setTitle(&NSString::from_str(shown));
        }
    }
}

impl Widgets for AppKit {
    type View = View;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> View {
        let mtm = self.mtm;
        let empty = NSString::new();
        match kind {
            NodeKind::View => View::Container(PrismView::new(mtm, id)),
            // Tables (#284): `set_table` gives them their rows and columns.
            NodeKind::Table | NodeKind::TableRow | NodeKind::ColumnHeader | NodeKind::TableCell => {
                View::Container(PrismView::new(mtm, id))
            }
            NodeKind::Screen => View::Screen(PrismView::new(mtm, id)),
            NodeKind::Tab => View::Tab(PrismView::new(mtm, id)),
            NodeKind::Tabs => {
                let tabs = Rc::new(self.new_tabs(id));
                self.tab_bars.push(tabs.clone());
                View::Tabs(tabs)
            }
            NodeKind::Modal => {
                let content = PrismView::new(mtm, id);
                content.ivars().sheet_root.set(true);
                let host = PrismView::new(mtm, 0);
                let window = unsafe {
                    NSWindow::initWithContentRect_styleMask_backing_defer(
                        NSWindow::alloc(mtm),
                        NSRect::new(NSPoint::ZERO, NSSize::new(520., 620.)),
                        NSWindowStyleMask::Titled | NSWindowStyleMask::Resizable,
                        NSBackingStoreType::Buffered,
                        false,
                    )
                };
                unsafe { window.setReleasedWhenClosed(false) };
                observe_focus(&window, &self.target);
                window.setContentView(Some(&host));
                host.setSubviews(&NSArray::from_slice(&[&*content as &NSView]));
                View::Sheet(Rc::new(MacSheet {
                    window,
                    content,
                    presented: Cell::new(false),
                }))
            }
            NodeKind::Scroll => {
                let scroll = PrismScrollView::new(mtm);
                scroll.setHasVerticalScroller(true);
                scroll.setDrawsBackground(false);
                let document = PrismView::new(mtm, id);
                scroll.setDocumentView(Some(&document));
                View::Scroll(Rc::new(MacScroll { scroll, document }))
            }
            NodeKind::Image => {
                let v = NSImageView::new(mtm);
                v.setTag(id as isize);
                View::Image(v)
            }
            NodeKind::Progress => {
                let p = NSProgressIndicator::new(mtm);
                p.setStyle(NSProgressIndicatorStyle::Bar);
                p.setIndeterminate(false);
                p.setMaxValue(1.);
                View::Progress(p)
            }
            NodeKind::Picture => View::Picture(PrismView::new(mtm, id)),
            NodeKind::Shape => View::Container(PrismView::new(mtm, id)),
            NodeKind::Scene3d => View::Container(crate::scene::create(mtm, id)),
            NodeKind::WebView => {
                let container = PrismView::new(mtm, id);
                let host = prism_native::apple::web::WebHost::new(id);
                if let Some(h) = &host {
                    // SAFETY: the bridge's host is an NSView, kept alive by `host`.
                    let v = unsafe { &*(h.view_ptr() as *const NSView) };
                    container.addSubview(v);
                }
                View::Web(Rc::new(MacWeb { container, host }))
            }
            NodeKind::Video => {
                let container = PrismView::new(mtm, id);
                let host = prism_native::apple::video::VideoHost::new(id);
                if let Some(h) = &host {
                    // SAFETY: the bridge's host is an NSView, kept alive by `host`.
                    let v = unsafe { &*(h.view_ptr() as *const NSView) };
                    v.setFrame(container.bounds());
                    container.addSubview(v);
                }
                View::Video(Rc::new(MacVideo { container, host }))
            }
            NodeKind::Slider => {
                let s = PrismSlider::new(mtm, id, &self.target);
                self.directional(&s);
                View::Slider(s)
            }
            NodeKind::Picker => {
                let b = NSPopUpButton::initWithFrame_pullsDown(
                    NSPopUpButton::alloc(mtm),
                    NSRect::ZERO,
                    false,
                );
                b.setTag(id as isize);
                let target: &AnyObject = &self.target;
                unsafe {
                    b.setTarget(Some(target));
                    b.setAction(Some(sel!(onPicker:)));
                }
                self.directional(&b);
                let p = Rc::new(MacPicker {
                    id,
                    button: b,
                    options: RefCell::new(Vec::new()),
                    placeholder: RefCell::new(String::new()),
                    selected: Cell::new(-1),
                });
                p.rebuild();
                View::Picker(p)
            }
            NodeKind::DatePicker => {
                let d = NSDatePicker::new(mtm);
                d.setTag(id as isize);
                d.setBezeled(true);
                d.setDrawsBackground(true);
                let target: &AnyObject = &self.target;
                unsafe {
                    d.setTarget(Some(target));
                    d.setAction(Some(sel!(onDate:)));
                }
                self.set_date_locale(&d);
                self.directional(&d);
                configure_date_picker(&d, DateConfig::default());
                View::DatePicker(d)
            }
            NodeKind::TextArea => View::TextArea(Rc::new(self.new_text_area(id))),
            // A text area drawing runs and chips (#115).
            NodeKind::RichInput => {
                let t = self.new_text_area(id);
                crate::rich::setup(&t.text);
                View::TextArea(Rc::new(t))
            }
            NodeKind::Spinner => {
                let p = NSProgressIndicator::new(mtm);
                p.setStyle(NSProgressIndicatorStyle::Spinning);
                p.setIndeterminate(true);
                p.setControlSize(NSControlSize::Small);
                p.setIdentifier(Some(&NSString::from_str(&id.to_string())));
                unsafe { p.startAnimation(None) };
                View::Spinner(p)
            }
            NodeKind::Timer => {
                let l = NSTextField::labelWithString(&empty, mtm);
                l.setTag(id as isize);
                View::Timer(l)
            }
            NodeKind::Toggle => {
                let t = NSSwitch::new(mtm);
                t.setTag(id as isize);
                let target: &AnyObject = &self.target;
                unsafe {
                    t.setTarget(Some(target));
                    t.setAction(Some(sel!(onToggle:)));
                }
                View::Toggle(t)
            }
            NodeKind::Stack => {
                let stack = Rc::new(self.new_stack(id));
                self.stacks.push(stack.clone());
                View::Stack(stack)
            }
            NodeKind::Text => View::Label(crate::label::new_label(mtm, id)),
            NodeKind::Button => {
                let target: &AnyObject = &self.target;
                let b = unsafe {
                    NSButton::buttonWithTitle_target_action(
                        &empty,
                        Some(target),
                        Some(sel!(onAction:)),
                        mtm,
                    )
                };
                b.setTag(id as isize);
                View::Button(b)
            }
            NodeKind::Overlay => {
                let o = Rc::new(MacOverlay::new(mtm, id));
                self.ix.overlays.push(o.clone());
                View::Overlay(o)
            }
            NodeKind::Checkbox => {
                let target: &AnyObject = &self.target;
                let b = unsafe {
                    NSButton::checkboxWithTitle_target_action(
                        &empty,
                        Some(target),
                        Some(sel!(onCheck:)),
                        mtm,
                    )
                };
                b.setTag(id as isize);
                View::Check(b)
            }
            NodeKind::RadioGroup => {
                let r = MacRadios::new(mtm, id);
                self.directional(&r.container);
                View::Radios(Rc::new(r))
            }
            NodeKind::TextInput => {
                let f = self.new_text_field(id, false);
                let host = PrismView::new(mtm, id);
                host.setSubviews(&NSArray::from_slice(&[&*f as &NSView]));
                View::Input(Rc::new(MacInput {
                    id,
                    host,
                    field: RefCell::new(f),
                    icon: RefCell::new((String::new(), None)),
                }))
            }
        }
    }

    fn set_hidden(&mut self, view: &View, hidden: bool) {
        if let View::Tab(_) | View::Screen(_) = view {
            let id = view.id();
            let changed = if hidden {
                self.hidden_by_app.insert(id)
            } else {
                self.hidden_by_app.remove(&id)
            };
            let has = |views: &RefCell<Vec<View>>| views.borrow().iter().any(|v| v.id() == id);
            // Tabs and stacked screens show only while selected or on top.
            if let Some(bar) = self.tab_bars.iter().find(|b| has(&b.tabs)) {
                if changed {
                    self.refresh_tabs(bar);
                }
            } else if let Some(stack) = self.stacks.iter().find(|st| has(&st.screens)) {
                if changed {
                    self.refresh_stack(stack);
                }
            } else {
                view.ns().setHidden(hidden);
            }
        } else {
            view.ns().setHidden(hidden);
        }
        if let Some(v) = view.prism_view() {
            crate::decor::sync_shadow_host(v, &v.ivars().decor.borrow());
        }
    }

    fn set_style(&mut self, view: &View, style: &Style) {
        view.ns().setAlphaValue(style.opacity as f64);
        // Bar tint (#30).
        match view {
            View::Tabs(t) => {
                crate::navbar::tint_tabs(t.id, &t.segments, style.tint.map(ns_color));
                self.refresh_tabs(t);
            }
            View::Stack(st) => {
                crate::navbar::set_stack_tint(st.id, &st.back, style.tint.map(ns_color));
                self.refresh_stack(st);
            }
            _ => {}
        }
        // `hit_test(false)` / `inert`: clicks go through the subtree.
        crate::interact::set_no_hit(view.ns(), !style.hit_test || style.inert);
        // `.window_drag()` (#67).
        if let Some(v) = view.prism_view() {
            v.ivars().window_drag.set(style.window_drag);
        }
        if crate::interact::set_inert(view.ns(), style.inert) {
            crate::interact::refuse_focus(view.ns(), style.inert);
            view.ns().setAccessibilityHidden(style.inert);
        }
        if !style.transform.is_identity() || self.transforms.contains_key(&view.id()) {
            self.transforms.insert(view.id(), style.transform);
            self.apply_transform(view);
        }
        if matches!(
            view,
            View::Label(_) | View::Timer(_) | View::Button(_) | View::Input(_) | View::TextArea(_)
        ) {
            crate::padding::remember(view.id(), style);
        }
        match view {
            View::Sheet(sheet) => {
                *sheet.content.ivars().background.borrow_mut() = style.background.map(ns_color);
                sheet.content.setNeedsDisplay(true);
            }
            View::Stack(st) => {
                *st.container.ivars().background.borrow_mut() = style.background.map(ns_color);
                st.container.setNeedsDisplay(true);
            }
            View::Tabs(t) => {
                *t.container.ivars().background.borrow_mut() = style.background.map(ns_color);
                t.container.setNeedsDisplay(true);
            }
            View::Scroll(sc) => {
                *sc.document.ivars().background.borrow_mut() = style.background.map(ns_color);
                sc.document.setNeedsDisplay(true);
                // `scroll_x()`: sideways only (the document is as wide as
                // `set_content_size` says, as tall as the view).
                let sideways = style.scroll_axis == prism_core::ScrollAxis::Horizontal;
                let (on, off) = (
                    objc2_app_kit::NSScrollElasticity::Automatic,
                    objc2_app_kit::NSScrollElasticity::None,
                );
                // No legacy horizontal scroller: AppKit would take 15 pt
                // of the row's height for it, after layout. Trackpad swipes
                // and shift + wheel scroll.
                sc.scroll.set_scrollbar(style.scrollbar, sideways);
                sc.scroll
                    .setVerticalScrollElasticity(if sideways { off } else { on });
            }
            View::Toggle(_) | View::Progress(_) => {}
            // No tint API: `controlTint` is "not respected on 10.15 and
            // later" (NSProgressIndicator.h); pop-up buttons only tint
            // borderless content (NSButton.h, contentTintColor) and date
            // pickers have none. They follow the system accent color.
            View::Spinner(_) | View::Picker(_) | View::DatePicker(_) => {}
            View::Slider(s) => {
                s.setTrackFillColor(style.text_color.map(ns_color).as_deref());
                // macOS 26 leaves sliders with tick marks untinted unless
                // asked (NSSlider.h, tintProminence).
                if s.respondsToSelector(sel!(setTintProminence:)) {
                    s.setTintProminence(if style.text_color.is_some() {
                        objc2_app_kit::NSTintProminence::Primary
                    } else {
                        objc2_app_kit::NSTintProminence::Automatic
                    });
                }
            }
            View::TextArea(t) => {
                let iv = t.container.ivars();
                *iv.background.borrow_mut() = style.background.map(ns_color);
                *iv.border_color.borrow_mut() = style.border_color.map(ns_color);
                iv.radius.set(style.corner_radius as f64);
                iv.corners.set(style.rounded_corners);
                iv.border.set(style.border_width as f64);
                self.apply_decor(&t.container, style);
                t.container.setNeedsDisplay(true);
                // Unstyled (`.native()`): the standard bezeled, white field.
                let bare = style.background.is_none() && style.border_color.is_none();
                t.scroll.setBorderType(if bare {
                    objc2_app_kit::NSBorderType::BezelBorder
                } else {
                    objc2_app_kit::NSBorderType::NoBorder
                });
                t.text.setDrawsBackground(bare);
                if bare {
                    t.text.setBackgroundColor(&NSColor::textBackgroundColor());
                }
                // Its padding replaces the text's own inset (#14), an
                // explicit zero included (#69).
                t.text.setTextContainerInset(if !style.has_padding() {
                    TEXT_AREA_INSET
                } else {
                    NSSize::ZERO
                });
                t.text.setFont(Some(&font(style)));
                t.text.setTextColor(Some(
                    &style.text_color.map_or_else(NSColor::labelColor, ns_color),
                ));
                t.text.setAlignment(alignment(style.text_align));
                // A rich input draws its runs again over the new font; a
                // plain one takes the line height and baseline of `text()`
                // (#179).
                if t.text.ivars().rich.get() {
                    crate::rich::set_style(&t.text, style);
                } else {
                    *t.text.ivars().style.borrow_mut() = Some(style.clone());
                    crate::rich::set_plain_style(&t.text, style);
                }
            }
            View::Image(v) => {
                let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(
                    style.font_size as f64,
                    font_weight(style),
                );
                v.setSymbolConfiguration(Some(&config));
                v.setContentTintColor(Some(
                    &style.text_color.map_or_else(NSColor::labelColor, ns_color),
                ));
            }
            View::Timer(l) => {
                l.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                    style.font_size as f64,
                    font_weight(style),
                )));
                l.setTextColor(Some(
                    &style.text_color.map_or_else(NSColor::labelColor, ns_color),
                ));
                l.setAlignment(alignment(style.text_align));
            }
            View::Container(v) | View::Screen(v) | View::Tab(v) | View::Picture(v) => {
                self.style_box(v, style)
            }
            // A web view's box: its background shows while the page loads.
            View::Web(w) => self.style_box(&w.container, style),
            View::Video(v) => self.style_box(&v.container, style),
            View::Label(l) => crate::label::set_style(l, style),
            View::Input(i) => {
                // Padding, a background, a border or `.border(0.)`: Prism's
                // box around a bare field (#14), else the bezeled field.
                let iv = i.host.ivars();
                let decor = crate::padding::field_box(style);
                let b = decor.as_ref();
                *iv.background.borrow_mut() = b.and_then(|s| s.background).map(ns_color);
                *iv.border_color.borrow_mut() = b.and_then(|s| s.border_color).map(ns_color);
                iv.radius.set(b.map_or(0., |s| s.corner_radius as f64));
                iv.corners.set(style.rounded_corners);
                iv.border.set(b.map_or(0., |s| s.border_width as f64));
                self.apply_decor(&i.host, b.unwrap_or(&Style::default()));
                i.host.setNeedsDisplay(true);
                let l = i.field();
                crate::padding::style_field(&l, style);
                l.setFont(Some(&font(style)));
                l.setTextColor(Some(
                    &style.text_color.map_or_else(NSColor::labelColor, ns_color),
                ));
                l.setAlignment(alignment(style.text_align));
            }
            View::Button(b) => {
                b.setFont(Some(&font(style)));
                b.setBezelColor(style.background.map(ns_color).as_deref());
                crate::padding::style_button(b, style);
            }
            View::Check(b) => b.setFont(Some(&font(style))),
            View::Radios(r) => r.set_font(font(style)),
            View::Overlay(_) => {}
        }
        // Outer shadows follow the opacity and transform.
        if let Some(v) = view.prism_view() {
            crate::decor::sync_shadow_host(v, &v.ivars().decor.borrow());
        }
    }

    fn set_text(&mut self, view: &View, text: &str) {
        let s = NSString::from_str(text);
        // Other people's carets follow the new text (#240).
        crate::carets::refresh(view.id());
        match view {
            View::Label(l) => crate::label::set_text(l, text),
            View::Timer(l) => l.setStringValue(&s),
            View::Picture(v) => self.load_picture(view.id(), v, text),
            View::Web(w) => {
                if let Some(h) = &w.host {
                    h.load(text);
                }
            }
            // A video's source (#207): assets and files as file URLs.
            View::Video(v) => {
                if let Some(h) = &v.host
                    && let Some(url) = prism_native::images::media_url(text, &self.image_sources)
                {
                    h.load(&url);
                }
            }
            // Events reach the app synchronously, so an echo of the typing
            // equals the field (left alone, caret and selection included);
            // anything else is the app's value, written even while focused,
            // the caret keeping its distance from the end (#73).
            View::Input(i) => {
                let f = i.field();
                let current = f.stringValue();
                if current.to_string() != text {
                    let editor = || {
                        f.currentEditor()
                            .and_then(|e| e.downcast::<NSTextView>().ok())
                    };
                    let caret = editor().map(|e| {
                        let r = e.selectedRange();
                        prism_native::fields::caret_from_end(
                            current.length(),
                            r.location + r.length,
                            s.length(),
                        )
                    });
                    f.setStringValue(&s);
                    if let (Some(e), Some(caret)) = (editor(), caret) {
                        e.setSelectedRange(objc2_foundation::NSRange::new(caret, 0));
                    }
                }
            }
            View::Button(b) | View::Check(b) => b.setTitle(&s),
            // A rich input's text comes with its runs (`set_rich_value`).
            View::TextArea(t) if t.text.ivars().rich.get() => {}
            View::TextArea(t) => {
                let current = t.text.string();
                if current.to_string() != text {
                    let r = t.text.selectedRange();
                    let caret = prism_native::fields::caret_from_end(
                        current.length(),
                        r.location + r.length,
                        s.length(),
                    );
                    t.text.setString(&s);
                    t.text
                        .setSelectedRange(objc2_foundation::NSRange::new(caret, 0));
                    t.text.setNeedsDisplay(true);
                }
            }
            View::Screen(_) => {
                self.titles.insert(view.id(), text.to_string());
                for stack in &self.stacks {
                    self.refresh_stack(stack);
                }
            }
            View::Tab(_) => {
                self.titles.insert(view.id(), text.to_string());
                for bar in &self.tab_bars {
                    self.refresh_tabs(bar);
                }
            }
            _ => {}
        }
    }

    fn set_placeholder(&mut self, view: &View, text: &str) {
        match view {
            View::Input(i) => i
                .field()
                .setPlaceholderString(Some(&NSString::from_str(text))),
            View::TextArea(t) => {
                *t.text.ivars().placeholder.borrow_mut() = text.to_string();
                t.text.setNeedsDisplay(true);
            }
            View::Picker(p) => {
                *p.placeholder.borrow_mut() = text.to_string();
                p.rebuild();
            }
            View::DatePicker(d) => crate::dates::set_placeholder(d, text),
            View::Timer(l) => {
                self.timer_after.insert(view.id(), text.to_string());
                if let Some((_, end)) = self.timers.get(&view.id()) {
                    l.setStringValue(&NSString::from_str(&prism_core::timer_text(*end, text)));
                }
            }
            _ => {}
        }
    }

    fn set_clickable(&mut self, view: &View, clickable: bool) {
        // A "clickable" scroll view refreshes (`crate::refresh`).
        if let View::Scroll(sc) = view {
            crate::refresh::set_refreshable(view.id(), &sc.scroll, clickable);
        }
        if let View::Container(v) = view {
            v.ivars().clickable.set(clickable);
            if let Some(w) = v.window() {
                w.invalidateCursorRectsForView(v);
            }
        }
        if let View::Label(l) = view {
            crate::label::set_clickable(l, clickable);
        }
    }

    /// Hover: a tracking area (owned by the target, which reports it);
    /// presses: the view's mouse down and up. Focus is always reported
    /// (first responder observation).
    fn set_interactions(&mut self, view: &View, states: prism_core::Interactions) {
        use objc2_app_kit::{NSTrackingArea, NSTrackingAreaOptions as O};
        let id = view.id();
        let hover = states.contains(prism_core::Interaction::Hover);
        match (hover, self.hover_areas.contains_key(&id)) {
            (true, false) => {
                let target: &AnyObject = &self.target;
                let area = unsafe {
                    NSTrackingArea::initWithRect_options_owner_userInfo(
                        <NSTrackingArea as objc2::AllocAnyThread>::alloc(),
                        NSRect::ZERO,
                        O::MouseEnteredAndExited | O::ActiveAlways | O::InVisibleRect,
                        Some(target),
                        None,
                    )
                };
                view.ns().addTrackingArea(&area);
                self.hover_areas.insert(id, area);
                // Overflowing its parent: over the part outside it too (#213).
                if let Some(v) = view.ns().downcast_ref::<PrismView>() {
                    v.updateTrackingAreas();
                }
            }
            (false, true) => {
                self.hover_areas.remove(&id);
                // The target's area, as `updateTrackingAreas` may have
                // replaced it (#213).
                let target: &AnyObject = &self.target;
                for a in view.ns().trackingAreas().iter() {
                    if a.options().contains(O::ActiveAlways)
                        && a.owner().is_some_and(|o| std::ptr::eq(&*o, target))
                    {
                        view.ns().removeTrackingArea(&a);
                    }
                }
            }
            _ => {}
        }
        if let Some(v) = view.prism_view() {
            v.ivars()
                .report_active
                .set(states.contains(prism_core::Interaction::Active));
        }
    }

    fn set_children(&mut self, parent: &View, children: &[View]) {
        // Sheets attached to a root container are presented, not laid out inline.
        let (sheets, children): (Vec<View>, Vec<View>) = children
            .iter()
            .cloned()
            .partition(|c| matches!(c, View::Sheet(_)));
        if matches!(parent, View::Stack(_) | View::Tabs(_)) {
            self.present_sheets(&sheets);
        }
        let children = &children[..];
        match parent {
            View::Stack(stack) => {
                *stack.screens.borrow_mut() = children.to_vec();
                self.refresh_stack(stack);
                return;
            }
            View::Tabs(bar) => {
                *bar.tabs.borrow_mut() = children.to_vec();
                self.refresh_tabs(bar);
                return;
            }
            View::Scroll(sc) => {
                let views: Vec<&NSView> = children.iter().map(View::ns).collect();
                sc.document.setSubviews(&NSArray::from_slice(&views));
                crate::sticky::raise(&sc.document);
                return;
            }
            View::Overlay(o) => {
                let views: Vec<&NSView> = children.iter().map(View::ns).collect();
                o.host.setSubviews(&NSArray::from_slice(&views));
                return;
            }
            _ => {}
        }
        // A backdrop blur stays under the children.
        let blur = parent
            .prism_view()
            .and_then(|v| v.ivars().decor.borrow().blur_view());
        let mut views: Vec<&NSView> = children.iter().map(View::ns).collect();
        if let Some(b) = &blur {
            views.insert(0, b);
        }
        parent.ns().setSubviews(&NSArray::from_slice(&views));
        crate::sticky::raise(parent.ns());
        for c in children {
            if let Some(v) = c.prism_view() {
                crate::decor::sync_shadow_host(v, &v.ivars().decor.borrow());
            }
        }
    }

    fn set_root(&mut self, view: &View) {
        self.root.setSubviews(&NSArray::from_slice(&[view.ns()]));
    }

    fn set_drawing(&mut self, view: &View, paths: &[prism_core::Path]) {
        if let View::Container(v) = view {
            *v.ivars().shape.borrow_mut() = paths.to_vec();
            v.setNeedsDisplay(true);
        }
    }

    fn set_scene(&mut self, view: &View, update: &prism_core::scene3d::SceneUpdate, json: &str) {
        crate::scene::set(view.id(), update, json, &self.image_sources);
    }

    fn destroy(&mut self, view: &View) {
        crate::scene::forget(view.id());
        crate::carets::forget(view.id());
        self.ix.select_events.remove(&view.id());
        self.ix.pending_selection.remove(&view.id());
        crate::interact::forget_selection(view.id());
        crate::edit::forget(view.id());
        crate::sticky::forget(view.id());
        crate::dates::forget(view.id());
        crate::navbar::forget(view.id());
        crate::padding::forget(view.id());
        crate::refresh::forget(view.id());
        self.hover_areas.remove(&view.id());
        if let Some(v) = view.prism_view() {
            crate::decor::detach(&v.ivars().decor.borrow());
        }
        match view {
            View::Input(i) => {
                unsafe { i.field().setDelegate(None) };
                self.inputs.remove(&view.id());
            }
            View::TextArea(t) => t.text.setDelegate(None),
            View::Spinner(p) => unsafe { p.stopAnimation(None) },
            View::Container(_) => {
                self.announced.remove(&view.id());
            }
            View::Stack(st) => self.stacks.retain(|s| s.id != st.id),
            View::Tabs(t) => self.tab_bars.retain(|b| b.id != t.id),
            View::Sheet(sheet) => {
                self.dismiss_sheet(sheet);
                stop_observing_focus(&sheet.window, &self.target);
            }
            View::Overlay(o) => {
                o.dismiss(self.root.window().as_deref());
                self.ix.overlays.retain(|x| x.id != o.id);
            }
            View::Timer(_) => {
                self.timers.remove(&view.id());
                self.timer_after.remove(&view.id());
            }
            View::Picture(_) => {
                let id = view.id();
                self.pictures.retain(|_, ids| {
                    ids.retain(|(i, _)| *i != id);
                    !ids.is_empty()
                });
            }
            View::Tab(_) => {
                self.titles.remove(&view.id());
                self.icons.remove(&view.id());
                self.hidden_by_app.remove(&view.id());
            }
            View::Screen(_) => {
                self.titles.remove(&view.id());
                self.bare.remove(&view.id());
                self.hidden_by_app.remove(&view.id());
            }
            _ => {}
        }
        crate::interact::forget(view.ns());
        self.ix.key_local.remove(&view.id());
        self.ix.key_global.remove(&view.id());
        self.ix.suggestions.remove(&view.id());
        view.ns().removeFromSuperview();
    }

    fn measure(&mut self, view: &View, max_width: Option<f32>) -> (f32, f32) {
        // A countdown with an `after` text is as large as the larger of the
        // two, so the text fits once it ends (#281).
        if let View::Timer(l) = view
            && let Some(after) = self.timer_after.remove(&view.id())
        {
            let shown = l.stringValue();
            l.setStringValue(&NSString::from_str(&after));
            let ended = self.measure(view, max_width);
            l.setStringValue(&shown);
            let counting = self.measure(view, max_width);
            self.timer_after.insert(view.id(), after);
            return (ended.0.max(counting.0), ended.1.max(counting.1));
        }
        let size = match view {
            View::Label(l) => {
                let w = max_width.map_or(f64::MAX / 4., |w| w as f64);
                let bounds = NSRect::new(NSPoint::ZERO, NSSize::new(w, f64::MAX / 4.));
                l.cell()
                    .map_or(NSSize::ZERO, |c| c.cellSizeForBounds(bounds))
            }
            View::Input(i) => {
                let s = i.field().fittingSize();
                let style = crate::padding::style_of(i.id);
                if crate::padding::custom_field(&style) {
                    crate::padding::field_content(&style, s)
                } else {
                    NSSize::new(s.width.max(120.), s.height)
                }
            }
            // Padded: its title alone (the layout adds the padding).
            View::Button(b) if crate::padding::style_of(view.id()).has_padding() => {
                crate::padding::button_content(b)
            }
            View::Button(b) => b.fittingSize(),
            View::Toggle(t) => t.fittingSize(),
            View::Sheet(_) => NSSize::ZERO,
            View::Image(v) => v.fittingSize(),
            View::Timer(l) => l.fittingSize(),
            // Natural size of the loaded image, scaled to the available width.
            // Sized by its style only (no measure context).
            View::Web(_) => NSSize::ZERO,
            // The video's size, once AVKit knows it (#207).
            View::Video(v) => match &v.host {
                Some(h) => {
                    let (w, h) = prism_native::apple::video::measure(h, max_width);
                    NSSize::new(w as f64, h as f64)
                }
                None => NSSize::ZERO,
            },
            View::Picture(v) => match v.ivars().image.borrow().as_ref() {
                Some(img) => {
                    let size = img.size();
                    let w = max_width.map_or(size.width, |m| (m as f64).min(size.width));
                    let h = if size.width > 0. {
                        w * size.height / size.width
                    } else {
                        0.
                    };
                    NSSize::new(w, h)
                }
                None => NSSize::ZERO,
            },
            View::Progress(_) => NSSize::new(max_width.unwrap_or(120.) as f64, 6.),
            // Natural width 180 (fills less when it must), the control's height.
            View::Slider(s) => {
                let h = s.fittingSize().height;
                NSSize::new(max_width.map_or(180., |w| (w as f64).min(180.)), h)
            }
            // The widest option plus the chevron (NSPopUpButtonCell sizes to
            // its widest item).
            View::Picker(p) => p.button.fittingSize(),
            // With its placeholder or clear button (#89).
            View::DatePicker(d) => {
                let size = d.fittingSize();
                crate::dates::natural_size(d, size).unwrap_or(size)
            }
            View::Spinner(p) => {
                let s = p.fittingSize();
                if s.width > 0. {
                    s
                } else if p.controlSize() == NSControlSize::Regular {
                    NSSize::new(32., 32.)
                } else {
                    NSSize::new(16., 16.)
                }
            }
            View::TextArea(t) => text_area_size(t, max_width),
            View::Check(b) => b.fittingSize(),
            View::Radios(r) => r.size(),
            View::Overlay(_) => NSSize::ZERO,
            View::Container(_)
            | View::Screen(_)
            | View::Tab(_)
            | View::Stack(_)
            | View::Tabs(_)
            | View::Scroll(_) => NSSize::ZERO,
        };
        (size.width.ceil() as f32, size.height.ceil() as f32)
    }

    /// A label wraps down to its longest word (CSS min-content).
    fn measure_min_content(&mut self, view: &View) -> f32 {
        match view {
            View::Label(l) => crate::label::min_content_width(l) as f32,
            _ => self.measure(view, None).0,
        }
    }

    fn set_frame(&mut self, view: &View, x: f32, y: f32, w: f32, h: f32) {
        let rect =
            |x: f64, y: f64, w: f64, h: f64| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        // Sticky views add how far their scroll view has carried them.
        let mut frame =
            crate::sticky::frame_for(view.id(), rect(x as f64, y as f64, w as f64, h as f64));
        if let View::Timer(_) = view {
            // A timer's digits sit inside its padding (#14), in its
            // parent's (flipped) coordinates.
            let p = crate::padding::of(view.id());
            let (l, t) = (p.left as f64, p.top as f64);
            frame = rect(
                frame.origin.x + l,
                frame.origin.y + t,
                (frame.size.width - l - p.right as f64).max(0.),
                (frame.size.height - t - p.bottom as f64).max(0.),
            );
        }
        view.ns().setFrame(frame);
        if let View::DatePicker(d) = view {
            crate::dates::framed(d);
        }
        if self.transforms.contains_key(&view.id()) {
            self.apply_transform(view);
        }
        if let Some(v) = view.prism_view() {
            crate::decor::sync_shadow_host(v, &v.ivars().decor.borrow());
        }
        if let View::Input(i) = view {
            // Inside its padding and border when Prism draws its box.
            let style = crate::padding::style_of(i.id);
            let mut e = crate::padding::field_inset(&style);
            if crate::padding::custom_field(&style) && i.icon.borrow().1.is_some() {
                // The field's cell draws the icon in its first 24 points:
                // the text starts at the leading padding (32 at least, like
                // the web).
                let rtl = i.field().userInterfaceLayoutDirection()
                    == objc2_app_kit::NSUserInterfaceLayoutDirection::RightToLeft;
                let lead = if rtl { &mut e.right } else { &mut e.left };
                *lead = (lead.max(32.) - 24.).max(0.);
            }
            let (l, t) = (e.left as f64, e.top as f64);
            let room = (h as f64 - t - e.bottom as f64).max(0.);
            // A bare field draws its text at the top of its frame: in a box
            // taller than its line it gets its own height, centered, like
            // the web and iOS (#55). (The bezeled field fills its frame.)
            let field = i.field();
            let line = field.intrinsicContentSize().height.ceil();
            let (y, fh) = if crate::padding::custom_field(&style) && line > 0. && line < room {
                (t + ((room - line) / 2.).round(), line)
            } else {
                (t, room)
            };
            field.setFrame(rect(l, y, (w as f64 - l - e.right as f64).max(0.), fh));
            crate::carets::laid_out(i.id);
            if let Some(icon) = i.icon.borrow().1.as_deref() {
                crate::interact::place_icon(&i.field(), icon);
            }
        }
        if let View::Radios(r) = view {
            r.layout();
        }
        if let View::TextArea(t) = view {
            let b = t.border();
            let (w, h) = (w as f64, h as f64);
            // Inside its border, then its padding (#14).
            let p = crate::padding::of(view.id());
            let (l, r, top, bottom) = (
                b + p.left as f64,
                b + p.right as f64,
                b + p.top as f64,
                b + p.bottom as f64,
            );
            t.scroll.setFrame(rect(
                l,
                top,
                (w - l - r).max(0.),
                (h - top - bottom).max(0.),
            ));
            // The text fills the visible area (clicks anywhere edit) and
            // grows past it (scrolls).
            let visible = t.scroll.contentSize();
            t.text.setMinSize(NSSize::new(0., visible.height));
            let current = t.text.frame().size.height;
            t.text
                .setFrameSize(NSSize::new(visible.width, current.max(visible.height)));
            t.text.sizeToFit();
            crate::carets::laid_out(t.text.ivars().id.get());
        }
        if let View::Tabs(t) = view {
            let w = w as f64;
            t.bar.setFrame(rect(0., 0., w, HEADER_HEIGHT - 1.));
            t.separator.setFrame(rect(0., HEADER_HEIGHT - 1., w, 1.));
            self.place_tabs(t);
        }
        if let View::Stack(st) = view {
            let w = w as f64;
            st.header.setFrame(rect(0., 0., w, HEADER_HEIGHT - 1.));
            st.separator.setFrame(rect(0., HEADER_HEIGHT - 1., w, 1.));
            self.place_header(st);
        }
    }

    fn set_value(&mut self, view: &View, value: f64) {
        match view {
            View::Stack(st) => {
                let flags = value as u32;
                st.closable.set(flags & prism_core::STACK_CLOSABLE != 0);
                st.no_header.set(flags & prism_core::STACK_NO_HEADER != 0);
                self.refresh_stack(st);
            }
            View::Screen(_) => {
                let changed = if value == 1. {
                    self.bare.insert(view.id())
                } else {
                    self.bare.remove(&view.id())
                };
                if changed {
                    for stack in &self.stacks {
                        self.refresh_stack(stack);
                    }
                }
            }
            View::Progress(p) => p.setDoubleValue(value),
            View::Scroll(sc) => crate::refresh::set_refreshing(view.id(), &sc.scroll, value != 0.),
            View::Picture(v) => {
                v.ivars().fit.set(value == 1.);
                v.setNeedsDisplay(true);
            }
            View::Timer(l) => {
                self.timers.insert(view.id(), (l.clone(), value));
                let after = self.timer_after.get(&view.id()).map_or("", String::as_str);
                l.setStringValue(&NSString::from_str(&prism_core::timer_text(value, after)));
            }
            View::Toggle(t) => t.setState(if value != 0. {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            }),
            View::Check(b) => b.setState(if value != 0. {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            }),
            View::Radios(r) => r.set_selected(if value >= 0. { value as isize } else { -1 }),
            View::Overlay(o) => {
                o.open.set(value != 0.);
                o.update(
                    self.root.window().as_deref(),
                    ProtocolObject::from_ref(&*self.target),
                );
            }
            View::Tabs(bar) => {
                bar.selected.set(value.max(0.) as usize);
                self.refresh_tabs(bar);
            }
            View::Slider(s) => {
                let v = s.ivars().config.get().snap(value);
                s.ivars().last.set(v);
                if s.doubleValue() != v {
                    s.setDoubleValue(v);
                }
            }
            View::Picker(p) => {
                let index = if value >= 0. { value as isize } else { -1 };
                if p.selected.replace(index) != index {
                    p.rebuild();
                }
            }
            View::DatePicker(d) => {
                // An optional picker shows NaN as its placeholder (#89).
                crate::dates::set_value(d, value);
                if !value.is_nan() && d.dateValue().timeIntervalSince1970() != value {
                    d.setDateValue(&objc2_foundation::NSDate::dateWithTimeIntervalSince1970(
                        value,
                    ));
                }
            }
            // 0: regular (16 pt, `NSControlSize::Small`, the size next to a
            // line of text), 1: large (32 pt, `Regular`).
            View::Spinner(p) => p.setControlSize(if value >= 1. {
                NSControlSize::Regular
            } else {
                NSControlSize::Small
            }),
            _ => {}
        }
    }

    fn set_config(&mut self, view: &View, _kind: NodeKind, config: &str) {
        match view {
            // A filled picture's focal point (#218).
            View::Picture(v) => {
                let at = prism_core::picture_position(config);
                v.ivars().focus.set((at != (0.5, 0.5)).then_some(at));
                v.setNeedsDisplay(true);
            }
            View::Video(v) => {
                if let Some(h) = &v.host {
                    h.configure(&prism_core::VideoConfig::decode(config));
                }
            }
            View::Web(w) => {
                if let Some(h) = &w.host {
                    h.configure(config);
                }
            }
            // Bar buttons (#30): the stack showing the screen puts them up.
            View::Screen(_) => {
                if crate::navbar::set_config(view.id(), config) {
                    for stack in &self.stacks {
                        self.refresh_stack(stack);
                    }
                }
            }
            // A rich text's runs.
            View::Label(l) => crate::label::set_runs(l, config),
            View::Slider(s) => {
                let c = SliderConfig::decode(config);
                s.ivars().config.set(c);
                s.setMinValue(c.min);
                s.setMaxValue(c.max);
                // Few steps: tick marks the knob snaps to (also with the
                // keyboard). Many: no marks, the action snaps.
                let steps = if c.step > 0. {
                    ((c.max - c.min) / c.step).round() as isize
                } else {
                    0
                };
                let ticks = (1..=10).contains(&steps);
                s.setNumberOfTickMarks(if ticks { steps + 1 } else { 0 });
                s.setAllowsTickMarkValuesOnly(ticks);
                let v = c.snap(s.doubleValue());
                s.setDoubleValue(v);
                s.ivars().last.set(v);
            }
            View::Picker(p) => {
                *p.options.borrow_mut() = PickerConfig::decode(config).options;
                p.rebuild();
            }
            View::DatePicker(d) => {
                let c = DateConfig::decode(config);
                configure_date_picker(d, c);
                let target: &AnyObject = &self.target;
                crate::dates::configure(d, c, target);
            }
            View::Radios(r) => {
                let target: &AnyObject = &self.target;
                r.set_config(config, target);
            }
            View::Overlay(o) => o.set_config(config),
            View::Input(i) => {
                let c = InputConfig::decode(config);
                let suggestions = prism_core::controls::input_suggestions(config);
                if suggestions.is_empty() {
                    self.ix.suggestions.remove(&view.id());
                } else {
                    self.ix.suggestions.insert(view.id(), suggestions);
                }
                self.set_secure(i, c.secure);
                // Completion suggestions (Touch Bar, typing). On-screen
                // keyboards, return-key labels and per-field
                // auto-capitalization don't exist on macOS.
                i.field()
                    .setAutomaticTextCompletionEnabled(c.autocorrect && !c.secure);
                self.inputs.insert(view.id(), c);
            }
            View::TextArea(t) => {
                let c = InputConfig::decode(config);
                t.text.ivars().rows.set(c.min_rows());
                set_text_checking(&t.text, c.autocorrect);
                t.text.setAutomaticTextCompletionEnabled(c.autocorrect);
            }
            _ => {}
        }
    }

    fn set_icon(&mut self, view: &View, icon: &str) {
        if let View::Input(i) = view {
            let existing = i.icon.borrow_mut().1.take();
            let v = crate::interact::set_leading_icon(&i.field(), icon, existing);
            *i.icon.borrow_mut() = (icon.to_string(), v);
            return;
        }
        if let View::Image(v) = view {
            let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(icon),
                None,
            );
            v.setImage(image.as_deref());
            return;
        }
        self.icons.insert(view.id(), icon.to_string());
        for bar in &self.tab_bars {
            self.refresh_tabs(bar);
        }
    }

    fn set_appearance(&mut self, appearance: Appearance) {
        let appearance = match appearance {
            Appearance::System => env_appearance(),
            Appearance::Light => objc2_app_kit::NSAppearance::appearanceNamed(unsafe {
                objc2_app_kit::NSAppearanceNameAqua
            }),
            Appearance::Dark => objc2_app_kit::NSAppearance::appearanceNamed(unsafe {
                objc2_app_kit::NSAppearanceNameDarkAqua
            }),
        };
        NSApplication::sharedApplication(self.mtm).setAppearance(appearance.as_deref());
    }

    fn set_scroll_events(&mut self, view: &View, on: bool) {
        let View::Scroll(sc) = view else { return };
        // One observer per clip view, shared with sticky views.
        crate::sticky::set_scroll_events(&self.target, &sc.scroll.contentView(), on);
    }

    /// Where the first line of text sits (`items_baseline()`). Labels draw
    /// at the top; buttons and pop-ups center their content in taller frames.
    fn baseline(&mut self, view: &View, _width: f32, height: f32) -> Option<f32> {
        let h = height as f64;
        let centered = |v: &NSView| {
            v.firstBaselineOffsetFromTop() + ((h - v.fittingSize().height) / 2.).max(0.)
        };
        let b = match view {
            View::Label(l) => l.firstBaselineOffsetFromTop(),
            View::Timer(l) => l.firstBaselineOffsetFromTop(),
            View::Button(b) => centered(b),
            View::Picker(p) => centered(&p.button),
            View::Input(i) => i.field().firstBaselineOffsetFromTop(),
            View::DatePicker(d) => d.firstBaselineOffsetFromTop(),
            _ => return None,
        };
        (b > 0.).then_some(b as f32)
    }

    /// Follows the scroll view's clip view (see `sticky.rs`).
    fn set_sticky(&mut self, view: &View, sticky: Option<(&View, prism_native::Sticky)>) {
        let follow = sticky.and_then(|(scroll, st)| match scroll {
            View::Scroll(sc) => Some((sc.scroll.contentView(), st)),
            _ => None,
        });
        crate::sticky::set(&self.target, view.id(), view.ns(), follow);
    }

    fn set_swipe(&mut self, view: &View, left: bool, right: bool) {
        if let View::Container(v) | View::Screen(v) | View::Tab(v) = view {
            v.ivars().swipe.set((left, right));
        }
    }

    fn set_frame_animated(
        &mut self,
        view: &View,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        t: prism_core::Transition,
    ) {
        let v: Retained<NSView> = objc2::Message::retain(view.ns());
        let old = v.frame();
        self.set_frame(view, x, y, w, h);
        let new = v.frame();
        v.setFrame(old);
        animate(t, move || v.animator().setFrame(new));
    }

    fn set_style_animated(&mut self, view: &View, style: &Style, t: prism_core::Transition) {
        let v: Retained<NSView> = objc2::Message::retain(view.ns());
        let alpha = v.alphaValue();
        let transform = v.layer().map(|l| l.transform());
        self.set_style(view, style);
        let new = v.alphaValue();
        v.setAlphaValue(alpha);
        if let (Some(layer), Some(from)) = (v.layer(), transform) {
            let to = layer.transform();
            if !ca_equal(from, to) {
                prism_native::apple::motion::animate_transform(&layer, from, t);
            }
        }
        animate(t, move || v.animator().setAlphaValue(new));
    }

    /// A picture of the view stays in its place and animates out.
    fn animate_exit(&mut self, parent: &View, view: &View, to: &Style, t: prism_core::Transition) {
        let v = view.ns();
        let parent = match parent {
            View::Scroll(sc) => {
                let d: &NSView = &sc.document;
                objc2::Message::retain(d)
            }
            p => objc2::Message::retain(p.ns()),
        };
        if v.window().is_none() || unsafe { v.superview() }.is_none_or(|s| *s != *parent) {
            return;
        }
        let bounds = v.bounds();
        let Some(rep) = v.bitmapImageRepForCachingDisplayInRect(bounds) else {
            return;
        };
        v.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        let image = NSImage::initWithSize(objc2::AllocAnyThread::alloc(), bounds.size);
        image.addRepresentation(&rep);
        let picture = objc2_app_kit::NSImageView::imageViewWithImage(&image, self.mtm);
        picture.setFrame(v.frame());
        picture.setWantsLayer(true);
        picture.setAlphaValue(v.alphaValue());
        parent.addSubview_positioned_relativeTo(
            &picture,
            objc2_app_kit::NSWindowOrderingMode::Above,
            Some(v),
        );
        let size = bounds.size;
        if let Some(layer) = picture.layer() {
            let from = v.layer().map_or(layer.transform(), |l| l.transform());
            layer.setAffineTransform(prism_native::apple::motion::matrix(
                to.transform
                    .matrix_around_center(size.width as f32, size.height as f32),
            ));
            prism_native::apple::motion::animate_transform(&layer, from, t);
        }
        let (p, alpha) = (picture.clone(), to.opacity as f64);
        animate(t, move || p.animator().setAlphaValue(alpha));
        let done = std::time::Duration::from_secs_f32(t.delay + t.duration * 1.2 + 0.05);
        on_main_after(done, move || picture.removeFromSuperview());
    }

    fn set_repeat(
        &mut self,
        view: &View,
        style: &Style,
        repeat: Option<(prism_core::Pose, prism_core::Transition)>,
    ) {
        use prism_native::apple::motion;
        let v = view.ns();
        v.setWantsLayer(true);
        let Some(layer) = v.layer() else { return };
        let Some((pose, t)) = repeat else {
            motion::stop_repeat(&layer);
            return;
        };
        let size = v.bounds().size;
        let m = |tr: prism_core::Transform| {
            motion::transform3d(motion::matrix(
                tr.matrix_around_center(size.width as f32, size.height as f32),
            ))
        };
        let (opacity, transform) = pose.apply(style.opacity, style.transform);
        motion::repeat(
            &layer,
            (style.opacity, style.transform),
            (opacity, transform),
            t,
            style.animations.restart,
            m,
        );
    }

    fn set_environment(&mut self, env: &prism_core::RenderEnvironment) {
        self.rtl = env.direction == prism_core::Direction::Rtl;
        self.chrome = (env.back.clone(), env.close.clone());
        crate::dates::set_clear_label(&env.clear);
        let dir = if self.rtl {
            objc2_app_kit::NSUserInterfaceLayoutDirection::RightToLeft
        } else {
            objc2_app_kit::NSUserInterfaceLayoutDirection::LeftToRight
        };
        self.root.setUserInterfaceLayoutDirection(dir);
        for stack in self.stacks.clone() {
            self.refresh_stack(&stack);
        }
        // Controls already on screen (and on screens kept off it).
        self.redirect_controls(&self.root.clone());
        for stack in &self.stacks {
            for screen in stack.screens.borrow().iter() {
                self.redirect_controls(screen.ns());
            }
        }
        for bar in &self.tab_bars {
            for tab in bar.tabs.borrow().iter() {
                self.redirect_controls(tab.ns());
            }
        }
        self.lang = env.lang.clone();
    }

    fn set_menu(&mut self, view: &View, menu: Option<&prism_core::menu::MenuSpec>) {
        if let View::Container(v) | View::Screen(v) | View::Tab(v) = view {
            *v.ivars().menu.borrow_mut() = menu.cloned();
        }
    }

    /// A pop-up `NSMenu` (#247), after the patches (it tracks the pointer
    /// in a loop of its own).
    fn show_menu(
        &mut self,
        view: Option<&View>,
        menu: &prism_core::menu::MenuSpec,
        at: Option<(f32, f32)>,
    ) {
        let v: Retained<NSView> = match view {
            Some(view) => objc2::Message::retain(view.ns()),
            None => objc2::Message::retain(&**self.root),
        };
        let target: Retained<AnyObject> =
            Retained::into_super(Retained::into_super(self.target.clone()));
        let (mtm, spec) = (self.mtm, menu.clone());
        on_main_after(std::time::Duration::ZERO, move || {
            crate::menus::pop_up(mtm, &spec, &target, &v, at);
        });
    }

    fn set_content_size(&mut self, view: &View, width: f32, height: f32) {
        if let View::Scroll(sc) = view {
            sc.document
                .setFrameSize(NSSize::new(width as f64, height as f64));
        }
        // An overlay's laid out content: present it, or follow its size.
        if let View::Overlay(o) = view {
            o.size
                .set(NSSize::new(width.ceil() as f64, height.ceil() as f64));
            o.update(
                self.root.window().as_deref(),
                ProtocolObject::from_ref(&*self.target),
            );
        }
    }

    fn overlay_viewport(&mut self, overlay: &View, _content: &View) -> Viewport {
        let window = self.root.frame().size;
        let config = match overlay {
            View::Overlay(o) => o.config.get(),
            _ => Default::default(),
        };
        let (width, height) = crate::interact::overlay_area(config, window);
        Viewport {
            x: 0.,
            y: 0.,
            width,
            height,
            insets: Insets::default(),
        }
    }

    fn set_pointer_events(&mut self, view: &View, mask: u8) {
        if let View::Container(v) | View::Screen(v) | View::Tab(v) | View::Picture(v) = view {
            v.set_pointer_mask(mask);
        }
    }

    fn set_drop_target(&mut self, view: &View, types: Option<&[String]>) {
        if let View::Container(v) | View::Screen(v) | View::Tab(v) | View::Picture(v) = view {
            let was = v.ivars().drop_types.replace(types.map(<[String]>::to_vec));
            match (was.is_some(), types.is_some()) {
                (false, true) => {
                    let file_url = unsafe { objc2_app_kit::NSPasteboardTypeFileURL };
                    v.registerForDraggedTypes(&NSArray::from_slice(&[file_url]));
                }
                (true, false) => v.unregisterDraggedTypes(),
                _ => {}
            }
        }
    }

    // --- Drag and drop between elements (#251) ---

    fn window_origin(&mut self, view: &View) -> Option<(f32, f32)> {
        let v = view.ns();
        v.window()?;
        let r = self.root.convertRect_fromView(v.bounds(), Some(v));
        // The root is flipped: y grows down, like Prism's.
        Some((r.origin.x as f32, r.origin.y as f32))
    }

    fn node_at(&mut self, x: f32, y: f32) -> Option<NodeId> {
        // The preview is never what is under the pointer.
        let preview = self.drag_preview.clone();
        if let Some(p) = &preview {
            p.setHidden(true);
        }
        let root: &NSView = &self.root;
        let p = root.convertPoint_toView(
            NSPoint::new(x as f64, y as f64),
            unsafe { root.superview() }.as_deref(),
        );
        let hit = root.hitTest(p);
        if let Some(p) = &preview {
            p.setHidden(false);
        }
        let mut view = hit;
        while let Some(v) = view {
            if let Some(p) = v.downcast_ref::<PrismView>()
                && node_id(p) != 0
            {
                return Some(node_id(p));
            }
            view = unsafe { v.superview() };
        }
        None
    }

    fn scroll_offset(&mut self, view: &View) -> (f32, f32) {
        match view {
            View::Scroll(s) => {
                let o = s.scroll.contentView().bounds().origin;
                (o.x as f32, o.y as f32)
            }
            _ => (0., 0.),
        }
    }

    fn set_drag_preview(&mut self, view: &View, at: Option<(f32, f32)>) {
        let Some((x, y)) = at else {
            if let Some(p) = self.drag_preview.take() {
                p.removeFromSuperview();
            }
            return;
        };
        if self.drag_preview.is_none() {
            let v = view.ns();
            let bounds = v.bounds();
            let Some(rep) = v.bitmapImageRepForCachingDisplayInRect(bounds) else {
                return;
            };
            v.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
            let image = NSImage::initWithSize(objc2::AllocAnyThread::alloc(), bounds.size);
            image.addRepresentation(&rep);
            let preview = NSImageView::imageViewWithImage(&image, self.mtm);
            preview.setFrame(bounds);
            preview.setAlphaValue(0.85);
            preview.setImageScaling(objc2_app_kit::NSImageScaling::ScaleAxesIndependently);
            let shadow = objc2_app_kit::NSShadow::new();
            shadow.setShadowBlurRadius(12.);
            shadow.setShadowOffset(NSSize::new(0., -4.));
            preview.setShadow(Some(&shadow));
            self.root.addSubview(&preview);
            self.drag_preview = Some(preview);
        }
        if let Some(p) = &self.drag_preview {
            p.setFrameOrigin(NSPoint::new(x as f64, y as f64));
        }
    }

    fn set_key_events(&mut self, view: &View, local: bool, global: bool) {
        let id = view.id();
        if let View::Container(v) | View::Screen(v) | View::Tab(v) | View::Picture(v) = view {
            v.ivars().key_local.set(local);
        }
        for (set, on) in [
            (&mut self.ix.key_local, local),
            (&mut self.ix.key_global, global),
        ] {
            if on {
                set.insert(id);
            } else {
                set.remove(&id);
            }
        }
    }

    /// Text field selections (#86): through the field editor while an
    /// input is edited (else when it takes the focus), on the text view of
    /// a text area.
    fn rich_input(&self) -> bool {
        true
    }

    fn set_rich_value(&mut self, view: &View, runs: &str) {
        if let View::TextArea(t) = view
            && t.text.ivars().rich.get()
        {
            crate::rich::set_runs(&t.text, runs);
        }
    }

    /// A rich input's editing hooks (#217, `crate::edit`).
    fn set_edit_events(&mut self, view: &View, mask: u8) {
        if let View::TextArea(t) = view {
            t.text.ivars().edit_mask.set(mask);
            if mask & prism_core::edit_events::SELECTION_RECT == 0 {
                crate::edit::forget(view.id());
            } else {
                crate::edit::rect_changed(&t.text, view.id());
            }
        }
    }

    fn write_clipboard(&mut self, _view: &View, content: &prism_core::ClipboardContent) {
        crate::edit::write_clipboard(content);
    }

    fn insert_text(&mut self, view: &View, text: &str) {
        if let View::TextArea(t) = view {
            crate::edit::insert_text(&t.text, text);
        }
    }

    fn set_selection(&mut self, view: &View, start: usize, end: usize) {
        match view {
            View::Input(i) => {
                let editor = i
                    .field()
                    .currentEditor()
                    .and_then(|e| e.downcast::<NSTextView>().ok());
                match editor {
                    Some(e) => crate::interact::select_text(&e, start, end),
                    None => {
                        self.ix.pending_selection.insert(view.id(), (start, end));
                    }
                }
            }
            View::TextArea(t) => crate::interact::select_text(&t.text, start, end),
            _ => {}
        }
    }

    fn set_remote_carets(&mut self, view: &View, carets: &[prism_core::RemoteCaret]) {
        use crate::carets::Host;
        match view {
            View::TextArea(t) => {
                let host = Host::Text {
                    host: &t.container,
                    scroll: &t.scroll,
                    text: &t.text,
                };
                crate::carets::set(self.mtm, view.id(), host, carets);
            }
            View::Input(i) => {
                let field = i.field();
                let host = Host::Input {
                    host: &i.host,
                    field: &field,
                };
                crate::carets::set(self.mtm, view.id(), host, carets);
            }
            _ => {}
        }
    }

    fn set_selection_group(&mut self, view: &View, on: bool) {
        crate::edit::set_group(view.id(), on);
    }

    fn set_selection_highlight(&mut self, view: &View, start: usize, end: usize) {
        // Text areas and rich inputs draw it with their layout manager, a
        // one-line input over its text (#231).
        match view {
            View::TextArea(t) => crate::interact::highlight_text(&t.text, start, end),
            View::Input(i) => crate::across::highlight_field(&i.field(), start, end),
            _ => {}
        }
    }

    fn set_select_events(&mut self, view: &View, on: bool) {
        if on {
            self.ix.select_events.insert(view.id());
        } else {
            self.ix.select_events.remove(&view.id());
            crate::interact::forget_selection(view.id());
        }
    }

    fn post_web_message(&mut self, view: &View, data: &str) {
        if let View::Web(w) = view
            && let Some(h) = &w.host
        {
            h.post(data);
        }
    }

    fn reload_web_view(&mut self, view: &View) {
        if let View::Web(w) = view
            && let Some(h) = &w.host
        {
            h.reload();
        }
    }

    /// The key window's first responder lets go (the window keeps the
    /// focus itself), after the layout like `focus`.
    fn dismiss_keyboard(&mut self) {
        on_main_after(std::time::Duration::ZERO, || {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let app = NSApplication::sharedApplication(mtm);
            if let Some(w) = app.keyWindow().or_else(|| app.mainWindow()) {
                w.makeFirstResponder(None);
            }
        });
    }

    /// First responder after the layout (sheets present then).
    fn focus(&mut self, view: &View) {
        if let View::Input(_) = view {
            self.ix.app_focus = Some(view.id());
        }
        let v: Retained<NSView> = match view {
            View::Input(i) => Retained::into_super(Retained::into_super(i.field())),
            View::TextArea(t) => {
                Retained::into_super(Retained::into_super(Retained::into_super(t.text.clone())))
            }
            other => objc2::Message::retain(other.ns()),
        };
        on_main_after(std::time::Duration::ZERO, move || {
            if let Some(w) = v.window() {
                // The app asks: a clickable view takes it (#177).
                APP_FOCUS.set(true);
                w.makeFirstResponder(Some(&v));
                APP_FOCUS.set(false);
            }
        });
    }

    /// After the layout: its frame is where the scroll view must go.
    fn scroll_into_view(&mut self, view: &View) {
        let v: Retained<NSView> = objc2::Message::retain(view.ns());
        on_main_after(std::time::Duration::ZERO, move || {
            v.scrollRectToVisible(v.bounds());
        });
    }

    /// `cx.scroll_by` / `cx.scroll_to` (#193): the clip view's origin, kept
    /// within the document (flipped: y from the top).
    fn scroll_to(&mut self, view: &View, x: f32, y: f32, relative: bool, animated: bool) {
        let View::Scroll(s) = view else {
            return;
        };
        let clip = s.scroll.contentView();
        let bounds = clip.bounds();
        let doc = s.document.frame().size;
        let (tx, ty) = if relative {
            (bounds.origin.x + x as f64, bounds.origin.y + y as f64)
        } else {
            (x as f64, y as f64)
        };
        let clamp = |v: f64, max: f64| {
            if v.is_nan() {
                0.
            } else {
                v.clamp(0., max.max(0.))
            }
        };
        let to = NSPoint::new(
            clamp(tx, doc.width - bounds.size.width),
            clamp(ty, doc.height - bounds.size.height),
        );
        if animated {
            let proxy: Retained<objc2::runtime::AnyObject> = unsafe { msg_send![&*clip, animator] };
            let _: () = unsafe { msg_send![&*proxy, setBoundsOrigin: to] };
        } else {
            clip.scrollToPoint(to);
        }
        s.scroll.reflectScrolledClipView(&clip);
    }

    fn set_layers(&mut self, layers: &[View]) {
        let views: Vec<Retained<NSView>> = layers
            .iter()
            .map(|l| objc2::Message::retain(l.ns()))
            .collect();
        let main = self.root.window();
        self.ix.set_layers(main.as_deref(), views);
    }

    fn set_table(&mut self, table: &View, rows: &[prism_native::table::TableRow<'_, View>]) {
        let rows: Vec<(&NSView, bool, Vec<&NSView>)> = rows
            .iter()
            .map(|r| {
                (
                    r.view.ns(),
                    r.header,
                    r.cells.iter().map(|c| c.ns()).collect(),
                )
            })
            .collect();
        crate::table::set(self.mtm, table.ns(), &rows);
    }

    fn set_accessibility(&mut self, view: &View, a: &Accessibility) {
        // A text area is its text view, an input its field (their hosts are
        // just chrome).
        let field;
        let v: &NSView = match view {
            View::TextArea(t) => &t.text,
            View::Input(i) => {
                field = i.field();
                &field
            }
            _ => view.ns(),
        };
        if matches!(view, View::Stack(_) | View::Tabs(_) | View::Scroll(_)) {
            return;
        }
        if a.role == Some(Role::Status) {
            self.announce(view.id(), a.value.as_deref().unwrap_or(""));
        }
        let hidden = a.hidden || crate::interact::in_inert(v);
        v.setAccessibilityHidden(hidden);
        // VoiceOver reads a control through its cell, which the view's
        // label, help and hidden flag do not reach: a picker's label was not
        // read, and a decorative image read its symbol's name.
        let cell = v.downcast_ref::<NSControl>().and_then(|c| c.cell());
        if let Some(cell) = &cell {
            cell.setAccessibilityElement(!hidden);
        }
        // Tooltips (#216): AppKit's own, or the label's full text while it
        // is cut short (asked of the label as the pointer rests).
        v.removeAllToolTips();
        match &a.tooltip {
            Some(prism_core::Tooltip::Text(t)) => v.setToolTip(Some(&NSString::from_str(t))),
            Some(prism_core::Tooltip::WhenTruncated) => {
                v.setToolTip(None);
                if let View::Label(l) = view {
                    let owner: &AnyObject = l;
                    let all = NSRect::new(NSPoint::ZERO, NSSize::new(1e6, 1e6));
                    // SAFETY: the label owns its tooltip rect and outlives it.
                    unsafe { l.addToolTipRect_owner_userData(all, owner, std::ptr::null_mut()) };
                }
            }
            None => v.setToolTip(None),
        }
        let ns = |s: &Option<String>| s.as_deref().map(NSString::from_str);
        let native = matches!(
            view,
            View::Button(_)
                | View::Input(_)
                | View::Toggle(_)
                | View::Slider(_)
                | View::Picker(_)
                | View::DatePicker(_)
                | View::TextArea(_)
                | View::Spinner(_)
                | View::Check(_)
        );
        if let View::Slider(s) = view {
            // Read through `accessibilityValue` (the native value otherwise).
            *s.ivars().a11y_value.borrow_mut() = a.value.clone();
        }
        if !native {
            let role = unsafe {
                match a.role {
                    Some(Role::Button) => Some(NSAccessibilityButtonRole),
                    Some(Role::Link) => Some(NSAccessibilityLinkRole),
                    Some(Role::Heading(_)) => Some(NSAccessibilityHeadingRole),
                    Some(Role::Image) => Some(NSAccessibilityImageRole),
                    Some(Role::Text) => Some(NSAccessibilityStaticTextRole),
                    Some(Role::Switch | Role::Checkbox) => Some(NSAccessibilityCheckBoxRole),
                    Some(Role::Tab) => Some(NSAccessibilityRadioButtonRole),
                    Some(Role::TabList) => Some(NSAccessibilityTabGroupRole),
                    Some(Role::List) => Some(NSAccessibilityListRole),
                    Some(Role::Group | Role::ListItem | Role::TabPanel) => {
                        Some(NSAccessibilityGroupRole)
                    }
                    Some(Role::Slider) => Some(NSAccessibilitySliderRole),
                    Some(Role::ComboBox) => Some(NSAccessibilityPopUpButtonRole),
                    Some(Role::Radio) => Some(NSAccessibilityRadioButtonRole),
                    Some(Role::RadioGroup) => Some(objc2_app_kit::NSAccessibilityRadioGroupRole),
                    Some(Role::Dialog) => Some(NSAccessibilityGroupRole),
                    // Tables (#284): see `table.rs`.
                    Some(Role::Table) => Some(objc2_app_kit::NSAccessibilityTableRole),
                    Some(Role::Row) => Some(objc2_app_kit::NSAccessibilityRowRole),
                    Some(Role::Cell | Role::ColumnHeader) => {
                        Some(objc2_app_kit::NSAccessibilityCellRole)
                    }
                    // Status: not an element (its children stay reachable),
                    // its value is announced.
                    _ => None,
                }
            };
            if let View::Container(_) | View::Screen(_) | View::Tab(_) | View::Radios(_) = view {
                v.setAccessibilityElement(role.is_some() && !a.hidden);
                v.setAccessibilityRole(role);
                if a.role == Some(Role::Switch) {
                    v.setAccessibilityRoleDescription(Some(&NSString::from_str("switch")));
                }
            } else if let Some(role) = role
                && matches!(a.role, Some(Role::Heading(_)))
            {
                v.setAccessibilityRole(Some(role));
            } else if let View::Label(_) = view {
                // A clickable text is a button (or a link), a plain one text.
                let role = role.filter(|_| matches!(a.role, Some(Role::Button | Role::Link)));
                v.setAccessibilityRole(Some(
                    role.unwrap_or(unsafe { NSAccessibilityStaticTextRole }),
                ));
            }
            if let Some(checked) = a.checked {
                let value = NSString::from_str(if checked { "1" } else { "0" });
                unsafe { v.setAccessibilityValue(Some(&value)) };
            }
        }
        if a.label.is_some() || !native {
            v.setAccessibilityLabel(ns(&a.label).as_deref());
            if let Some(cell) = &cell {
                cell.setAccessibilityLabel(ns(&a.label).as_deref());
            }
        }
        v.setAccessibilityHelp(ns(&a.hint).as_deref());
        if let Some(cell) = &cell {
            cell.setAccessibilityHelp(ns(&a.hint).as_deref());
        }
        // Pop-ups and date pickers read their own (live) selection; sliders
        // go through `PrismSlider::accessibilityValue`.
        let own_value = matches!(
            view,
            View::Slider(_) | View::Picker(_) | View::DatePicker(_) | View::TextArea(_)
        );
        if let Some(value) = &a.value
            && !own_value
            && a.role != Some(Role::Status)
        {
            unsafe { v.setAccessibilityValue(Some(&NSString::from_str(value))) };
        }
        v.setAccessibilityEnabled(!a.disabled);
        // Selected items and the current page of a navigation: AppKit has no
        // « current » attribute, VoiceOver reads them as selected.
        v.setAccessibilitySelected(a.is_selected());
    }

    fn screen_viewport(&mut self, stack: &View, screen: &View) -> Viewport {
        if let View::Sheet(sheet) = screen {
            let size = sheet
                .window
                .contentView()
                .map_or(NSSize::new(520., 620.), |v| v.frame().size);
            return Viewport {
                x: 0.,
                y: 0.,
                width: size.width as f32,
                height: size.height as f32,
                insets: Insets::default(),
            };
        }
        let size = stack.ns().frame().size;
        // No header bar: the screen gets the whole stack.
        let header = match stack {
            View::Stack(st) if self.header_hidden(st) => 0.,
            _ => HEADER_HEIGHT,
        };
        Viewport {
            x: 0.,
            y: header as f32,
            width: size.width as f32,
            height: (size.height - header).max(0.) as f32,
            insets: Insets::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Opens a window and runs the AppKit event loop. Must be called on the main thread.
pub fn run(config: AppConfig, init: AppInit) {
    let mtm =
        MainThreadMarker::new().expect("prism-macos: run() must be called on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.setAppearance(env_appearance().as_deref());
    let target = Target::new(mtm);
    crate::menus::install_main_menu(mtm, &config.title, &target, &[]);
    let root = PrismView::new(mtm, 0);
    let (width, height) = window_size(&config);
    let frame = NSRect::new(NSPoint::ZERO, NSSize::new(width as f64, height as f64));
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    // Custom window chrome (#67): the content covers the title bar.
    let style = crate::chrome::style_mask(config.window_chrome, style);
    let window = crate::chrome::PrismWindow::create(mtm, frame, style);
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str(&config.title));
    window.setContentView(Some(&root));
    crate::chrome::setup(&window, config.window_chrome);
    window.setDelegate(Some(ProtocolObject::from_ref(&*target)));
    observe_focus(&window, &target);
    window.center();

    let path = std::env::var("PRISM_PATH").unwrap_or_else(|_| "/".into());
    let services = crate::services::MacServices::new(
        mtm,
        Retained::into_super(Retained::into_super(target.clone())),
        window.clone(),
        config.title.clone(),
    );
    let capabilities = prism_core::PlatformServices::capabilities(&services);
    let mut runtime = Runtime::with_options(
        &init,
        prism_core::RuntimeOptions {
            path,
            capabilities,
            storage: Some(Box::new(prism_core::FileStorage::new(data_dir(
                &config.title,
            )))),
            wake_ui: Some(wake_ui()),
            environment: mac_environment(),
            formatter: Some(Box::new(prism_native::apple::FoundationFormatter)),
            cloud_storage: Some(cloud_storage(&data_dir(&config.title))),
            viewport: Some(window_size(&config)),
            platform: None,
            data_dir: Some(data_dir(&config.title)),
            shared_data_dir: None,
            device_info: Some(crate::device_info::current(&window)),
        },
    );
    // Configurable widgets: the instances the extension saw on screen.
    runtime
        .app
        .set_widget_configurations_source(prism_native::apple::widget_configurations);
    // Widget buttons open the app on links that only its widgets can sign,
    // each usable once; the key and the ledger of used links stay between
    // runs in a folder of their own, out of the app's storage.
    if capabilities.widgets {
        let data = data_dir(&config.title);
        // Where an earlier version kept the key, among the app's stored keys.
        let _ = std::fs::remove_file(data.join("widget-link-key"));
        let dir = data.join(crate::widget_key::DIR);
        runtime.app.set_widget_key(
            crate::widget_key::load(&dir),
            Box::new(prism_core::FileStorage::new(&dir)),
        );
    }
    let renderer = NativeRenderer::new(AppKit {
        mtm,
        target: target.clone(),
        root,
        stacks: Vec::new(),
        tab_bars: Vec::new(),
        titles: HashMap::new(),
        app_title: None,
        window_title: config.title.clone(),
        icons: HashMap::new(),
        timers: HashMap::new(),
        timer_after: HashMap::new(),
        sheets: Vec::new(),
        pictures: HashMap::new(),
        image_cache: HashMap::new(),
        image_sources: image_sources(config.assets.as_deref(), &config.embedded_assets),
        rtl: false,
        chrome: ("Back".into(), "Close".into()),
        transforms: HashMap::new(),
        inputs: HashMap::new(),
        announced: HashMap::new(),
        lang: String::new(),
        bare: Default::default(),
        hidden_by_app: Default::default(),
        hover_areas: HashMap::new(),
        ix: Default::default(),
        drag_preview: None,
    });
    crate::trace::log("app runtime");
    let t = std::time::Instant::now();
    let first = runtime.render();
    crate::trace::log(&format!(
        "the app's tree ({:.0} ms, {} patches)",
        crate::trace::ms(t),
        first.len()
    ));
    let mut state = State {
        runtime,
        renderer,
        services,
        device_size: None,
    };
    let t = std::time::Instant::now();
    state.commit(first);
    state.relayout();
    if crate::trace::on() {
        eprintln!(
            "prism-macos: AppKit views: patches and layout {:.1} ms",
            crate::trace::ms(t)
        );
        crate::trace::log("first layout");
    }
    STATE.with(|cell| *cell.borrow_mut() = Some(state));
    prism_native::apple::set_device_sink(on_device);
    flush_responses();
    // Development: `PRISM_RUN_BACKGROUND=sync` runs a background task now.
    if let Ok(id) = std::env::var("PRISM_RUN_BACKGROUND") {
        run_background(&id, std::sync::mpsc::channel().0);
    }

    observe_system(&target);
    install_key_monitor();
    install_flags_monitor();
    window.makeKeyAndOrderFront(None);
    app.activate();
    if crate::trace::on() {
        // What AppKit would do at the end of this turn of the run loop:
        // draw, and commit the layers to the render server.
        window.displayIfNeeded();
        objc2_quartz_core::CATransaction::flush();
        eprintln!(
            "prism-macos: first frame after {:.0} ms; {}",
            crate::trace::since_start(),
            crate::trace::memory()
        );
    }
    if std::env::var_os("PRISM_SNAPSHOT").is_some()
        || std::env::var_os("PRISM_CLICK").is_some()
        || std::env::var_os("PRISM_A11Y_DUMP").is_some()
    {
        let none: Option<&AnyObject> = None;
        // `PRISM_DELAY` (seconds) leaves time for images or network to load.
        let delay: f64 = std::env::var("PRISM_DELAY")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(0.5);
        let _: () = unsafe {
            msg_send![&*target, performSelector: sel!(prismAutomation:), withObject: none, afterDelay: delay]
        };
    }
    app.run();
    drop(window);
}

thread_local! {
    /// Characters in the text input being edited (completion only while typing).
    static LAST_LEN: Cell<usize> = const { Cell::new(0) };
    /// `complete:` re-enters `controlTextDidChange:`.
    static COMPLETING: Cell<bool> = const { Cell::new(false) };
}

/// The view being dragged, where the drag started and how far it went
/// (#223).
type Dragged = (Retained<PrismView>, NSPoint, f64, f64);

thread_local! {
    static DRAGGED: RefCell<Option<Dragged>> = const { RefCell::new(None) };
}

/// Modifier keys pressed or released alone during a drag (#223): the drag
/// is reported again with them, where it is.
fn install_flags_monitor() {
    let block = block2::RcBlock::new(|event: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a valid event.
        let e = unsafe { event.as_ref() };
        let dragged = DRAGGED.with(|d| d.borrow().clone());
        if let Some((view, start, dx, dy)) = dragged {
            let mods = crate::interact::modifiers(e.modifierFlags());
            if view.ivars().drag_modifiers.replace(mods) != mods {
                view.drag(DragPhase::Move, start, dx, dy);
            }
        }
        event.as_ptr()
    });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            objc2_app_kit::NSEventMask::FlagsChanged,
            &block,
        )
    };
    // Lives as long as the app.
    std::mem::forget(monitor);
}

/// Key presses for `on_key_down` handlers around the focus (text fields,
/// buttons…; a focused Prism view with a handler gets them in `keyDown:`)
/// and `on_global_key_down` ones. Typing in a text field only reaches global
/// handlers for keys that do not type, or with ⌘ / ⌃.
fn install_key_monitor() {
    let block = block2::RcBlock::new(|event: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
        let e = unsafe { event.as_ref() };
        let Some(key) = crate::interact::key_event(e) else {
            return event.as_ptr();
        };
        // A release (#159) goes where its press went, if it was reported.
        if e.r#type() == objc2_app_kit::NSEventType::KeyUp {
            let mtm = MainThreadMarker::new().expect("main thread");
            let own = e
                .window(mtm)
                .and_then(|w| w.firstResponder())
                .and_then(|r| r.downcast::<PrismView>().ok())
                .is_some_and(|p| p.ivars().key_local.get());
            // A focused Prism view with a handler reports in `keyUp:`.
            if !own && let Some(focused) = release_key(&key.key) {
                dispatch(UiEvent::KeyUp(focused, key));
            }
            return event.as_ptr();
        }
        let mtm = MainThreadMarker::new().expect("main thread");
        let responder = e.window(mtm).and_then(|w| w.firstResponder());
        let view = responder
            .as_ref()
            .and_then(|r| r.clone().downcast::<NSView>().ok());
        let mut send = None;
        with_state(|s| {
            let ix = &s.renderer.widgets.ix;
            if ix.key_local.is_empty() && ix.key_global.is_empty() {
                return;
            }
            // A focused Prism view with a handler reports in `keyDown:`.
            if view
                .as_ref()
                .and_then(|v| v.downcast_ref::<PrismView>())
                .is_some_and(|p| p.ivars().key_local.get())
            {
                return;
            }
            let chain = view
                .as_deref()
                .map(crate::interact::node_chain)
                .unwrap_or_default();
            let local = chain.iter().any(|id| ix.key_local.contains(id));
            let typing = view
                .as_ref()
                .is_some_and(|v| v.isKindOfClass(NSText::class()));
            let shortcut = key.modifiers.control || key.modifiers.meta;
            let global = !ix.key_global.is_empty() && !(typing && key.is_character() && !shortcut);
            if local || global {
                send = Some(chain.first().copied());
            }
        });
        if let Some(focused) = send {
            // In a text area or input: its selection before the key (#86).
            let field = view
                .as_ref()
                .and_then(|v| v.downcast_ref::<NSTextView>())
                .filter(|tv| crate::interact::text_node(tv).is_some());
            let selection = field.map(crate::interact::text_selection);
            // And where its caret is (#211), for `consume_key_at`.
            let edges = field.map(crate::interact::caret_edges);
            hold_key(&key, focused);
            // The app's alone (`consume_keys`, #181): the focused text view
            // does not also act on it (↑ moving its caret, ↵ submitting).
            // Decided as pressed: the handler may close the menu that
            // declared it (↵ picking a suggestion).
            // A selection group's keys too (#231), decided from the key as
            // dispatched: its modifiers, the field's selection and edges.
            let key = prism_core::KeyEvent {
                selection,
                edges,
                ..key
            };
            let mut consumed = false;
            with_state(|s| consumed = s.runtime.consumes_key_event(focused, &key));
            dispatch(UiEvent::Key(focused, key));
            if consumed {
                return std::ptr::null_mut();
            }
        }
        event.as_ptr()
    });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            objc2_app_kit::NSEventMask::KeyDown | objc2_app_kit::NSEventMask::KeyUp,
            &block,
        )
    };
    // Lives as long as the app.
    std::mem::forget(monitor);
}

// ---------------------------------------------------------------------------
// Dev automation (env-driven, used by `mise run macos-snapshot`)
// ---------------------------------------------------------------------------

/// Something `PRISM_CLICK` can press: a native button by title, or any
/// clickable Prism view by its accessibility label (what VoiceOver reads).
enum Pressable {
    Native(Retained<NSButton>),
    Custom(Retained<PrismView>),
}

fn find_pressable(view: &NSView, title: &str) -> Option<Pressable> {
    for sub in view.subviews().iter() {
        if sub.isHiddenOrHasHiddenAncestor() {
            continue;
        }
        if let Ok(b) = sub.clone().downcast::<NSButton>()
            && (b.title().to_string() == title
                // A symbol alone (bar buttons): by its label.
                || (b.title().length() == 0
                    && b.accessibilityLabel().is_some_and(|l| l.to_string() == title)))
        {
            return Some(Pressable::Native(b));
        }
        if let Ok(p) = sub.clone().downcast::<PrismView>()
            && p.ivars().clickable.get()
            && p.accessibilityLabel()
                .is_some_and(|l| l.to_string() == title)
        {
            return Some(Pressable::Custom(p));
        }
        if let Some(found) = find_pressable(&sub, title) {
            return Some(found);
        }
    }
    None
}

/// Dev hook (`PRISM_CLICK`): picks the visible segment titled `title` (a
/// tab of the tab bar), as a click on it would.
fn press_segment(view: &NSView, title: &str) -> bool {
    for sub in view.subviews().iter() {
        if sub.isHiddenOrHasHiddenAncestor() {
            continue;
        }
        if let Some(seg) = sub.downcast_ref::<NSSegmentedControl>()
            && let Some(i) = (0..seg.segmentCount()).find(|i| {
                seg.labelForSegment(*i)
                    .is_some_and(|l| l.to_string() == title)
            })
        {
            seg.setSelectedSegment(i);
            dispatch(UiEvent::Change(seg.tag() as NodeId, i as f64));
            return true;
        }
        if press_segment(&sub, title) {
            return true;
        }
    }
    false
}

/// VoiceOver's press on a table cell's only control (#284), and whether it
/// acted. AppKit's buttons, switches and pickers act on
/// `accessibilityPerformPress` but answer NO, a failed press to VoiceOver:
/// a button or switch is clicked instead (its action runs, the press
/// succeeded), a picker opens its menu once the press returns, and a field
/// takes the focus to be typed in.
fn press_control(control: &NSView) -> bool {
    let Some(c) = control.downcast_ref::<NSControl>() else {
        // A link, or a clickable view: its own press answers.
        let done: objc2::runtime::Bool = unsafe { msg_send![control, accessibilityPerformPress] };
        return done.as_bool();
    };
    if !c.isEnabled() {
        return false;
    }
    if c.downcast_ref::<NSPopUpButton>().is_some() {
        // Its click tracks the menu until it closes: it is the next turn's,
        // and the press returns now.
        let none: Option<&AnyObject> = None;
        // SAFETY: `performClick:` takes a sender, nil here.
        let _: () = unsafe {
            msg_send![c, performSelector: sel!(performClick:), withObject: none, afterDelay: 0.0f64]
        };
        return true;
    }
    if c.downcast_ref::<NSButton>().is_some() || c.downcast_ref::<NSSwitch>().is_some() {
        // SAFETY: the control's own click, on the main thread.
        unsafe { c.performClick(None) };
        return true;
    }
    if c.downcast_ref::<NSTextField>().is_some() || c.downcast_ref::<NSDatePicker>().is_some() {
        return c.window().is_some_and(|w| w.makeFirstResponder(Some(c)));
    }
    let done: objc2::runtime::Bool = unsafe { msg_send![c, accessibilityPerformPress] };
    done.as_bool()
}

/// The first accessibility element under `o` labelled (or titled) `label`,
/// down the accessibility children as VoiceOver goes.
fn find_element(o: &AnyObject, label: &str, depth: usize) -> Option<Retained<AnyObject>> {
    if depth > 64 {
        return None;
    }
    // SAFETY: NSAccessibility getters of views and accessibility elements.
    let children: Option<Retained<NSArray>> = unsafe { msg_send![o, accessibilityChildren] };
    for c in children?.to_vec() {
        let named: [Option<Retained<NSString>>; 2] = unsafe {
            [
                msg_send![&*c, accessibilityLabel],
                msg_send![&*c, accessibilityTitle],
            ]
        };
        if named.iter().flatten().any(|s| s.to_string() == label) {
            return Some(c);
        }
        if let Some(found) = find_element(&c, label, depth + 1) {
            return Some(found);
        }
    }
    None
}

/// The cell at `"ROW,COLUMN"` of the first table under `view`, by the
/// table's `accessibilityCellForColumn:row:` (dev hooks).
fn table_cell(view: &NSView, at: &str) -> Option<Retained<AnyObject>> {
    fn table(view: &NSView) -> Option<Retained<NSView>> {
        view.subviews().iter().find_map(|sub| {
            let role = sub.accessibilityRole().map(|r| r.to_string());
            if role.as_deref() == Some("AXTable") {
                Some(sub.clone())
            } else {
                table(&sub)
            }
        })
    }
    let (row, column) = at.split_once(',')?;
    let (row, column): (isize, isize) = (row.trim().parse().ok()?, column.trim().parse().ok()?);
    let table = table(view)?;
    // SAFETY: NSAccessibility's table navigation; the cell or nil.
    unsafe { msg_send![&*table, accessibilityCellForColumn: column, row: row] }
}

/// The visible view whose accessibility label, button title or placeholder
/// is `label` (dev hooks).
fn find_labelled(view: &NSView, label: &str) -> Option<Retained<NSView>> {
    for sub in view.subviews().iter() {
        if sub.isHiddenOrHasHiddenAncestor() {
            continue;
        }
        let is = |s: Option<Retained<NSString>>| s.is_some_and(|s| s.to_string() == label);
        if is(sub.accessibilityLabel())
            || sub
                .downcast_ref::<NSButton>()
                .is_some_and(|b| is(Some(b.title())))
            || sub
                .downcast_ref::<NSTextField>()
                .is_some_and(|f| f.isEditable() && is(f.placeholderString()))
            || sub
                .downcast_ref::<PrismTextView>()
                .is_some_and(|t| *t.ivars().placeholder.borrow() == label)
        {
            return Some(sub.clone());
        }
        if let Some(found) = find_labelled(&sub, label) {
            return Some(found);
        }
    }
    None
}

/// Clicks the middle of `view` (mouse down, then the queued mouse up).
fn post_click(window: &NSWindow, view: &NSView) {
    use objc2_app_kit::{NSEventModifierFlags, NSEventType};
    let b = view.bounds();
    let mid = NSPoint::new(
        b.origin.x + b.size.width / 2.,
        b.origin.y + b.size.height / 2.,
    );
    let at = view.convertPoint_toView(mid, None);
    let app = NSApplication::sharedApplication(window.mtm());
    let now = objc2_foundation::NSProcessInfo::processInfo().systemUptime();
    let event = |kind| {
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind,
            at,
            NSEventModifierFlags::empty(),
            now,
            window.windowNumber(),
            None,
            0,
            1,
            1.,
        )
    };
    // The view tracks the mouse until the (queued) mouse up. Sent to the
    // view itself: a window that isn't key would swallow a first click.
    if let (Some(down), Some(up)) = (
        event(NSEventType::LeftMouseDown),
        event(NSEventType::LeftMouseUp),
    ) {
        app.postEvent_atStart(&up, false);
        view.mouseDown(&down);
    }
}

/// `PRISM_CLICK=dblclick:#window@x,y`: a double click sent through
/// `window` at (`x`, `y`) points from its content's top-left corner, as the
/// mouse sends it (the view under the point gets it); logs the window's
/// frame before and a second after (zoomed or minimized, as System
/// Settings › Desktop & Dock › « Double-click a window's title bar » says).
fn window_double_click(window: &NSWindow, at: &str) {
    use objc2_app_kit::{NSEventModifierFlags, NSEventType};
    let Some((x, y)) = at
        .split_once(',')
        .and_then(|(x, y)| Some((x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?)))
    else {
        eprintln!("prism-macos: PRISM_CLICK: dblclick:#window@{at}: not x,y");
        return;
    };
    let Some(content) = window.contentView() else {
        return;
    };
    let log = |w: &NSWindow, when: &str| {
        let f = w.frame();
        println!(
            "prism-macos: PRISM_CLICK: window {when}: ({}, {}), {} x {}{}{}",
            f.origin.x,
            f.origin.y,
            f.size.width,
            f.size.height,
            if w.isZoomed() { ", zoomed" } else { "" },
            if w.isMiniaturized() {
                ", minimized"
            } else {
                ""
            }
        );
    };
    log(window, "before");
    // Window coordinates grow upwards.
    let point = NSPoint::new(x, content.frame().size.height - y);
    // The frame view (the title bar's container is above the content).
    let frame = unsafe { content.superview() }.unwrap_or_else(|| content.clone());
    let hit = frame
        .hitTest(point)
        .map(|v| v.class().name().to_str().unwrap_or("?").to_string());
    println!("prism-macos: PRISM_CLICK: ({x}, {y}) hits {hit:?}");
    let app = NSApplication::sharedApplication(window.mtm());
    let now = objc2_foundation::NSProcessInfo::processInfo().systemUptime();
    // The second click's press (a click count of 2) and its release,
    // queued first: a single press would start the system's window drag.
    let event = |kind| {
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind,
            point,
            NSEventModifierFlags::empty(),
            now,
            window.windowNumber(),
            None,
            0,
            2,
            1.,
        )
    };
    if let (Some(down), Some(up)) = (
        event(NSEventType::LeftMouseDown),
        event(NSEventType::LeftMouseUp),
    ) {
        app.postEvent_atStart(&up, false);
        window.sendEvent(&down);
    }
    let window = objc2::Message::retain(window);
    on_main_after(std::time::Duration::from_secs(1), move || {
        log(&window, "after");
    });
}

/// A mouse drag from a third of the way into `from` to a third of the way
/// into `to` (`PRISM_DRAG_TO`, #231), in a few steps, as the mouse sends it.
fn post_drag(window: &NSWindow, from: &NSView, to: &NSView) {
    use objc2_app_kit::{NSEventModifierFlags, NSEventType};
    let third = |v: &NSView| {
        let b = v.bounds();
        v.convertPoint_toView(
            NSPoint::new(
                b.origin.x + b.size.width / 3.,
                b.origin.y + b.size.height / 3.,
            ),
            None,
        )
    };
    // A field's label can be on the box around its text view.
    fn text_view(v: &NSView) -> Retained<NSView> {
        fn find(v: &NSView) -> Option<Retained<NSView>> {
            if v.downcast_ref::<PrismTextView>().is_some() {
                return Some(objc2::Message::retain(v));
            }
            v.subviews().iter().find_map(|s| find(&s))
        }
        find(v).unwrap_or_else(|| objc2::Message::retain(v))
    }
    let (from, to) = (text_view(from), text_view(to));
    // Both on screen, as the user would see them.
    to.scrollRectToVisible(to.bounds());
    from.scrollRectToVisible(from.bounds());
    let (a, z) = (third(&from), third(&to));
    let app = NSApplication::sharedApplication(window.mtm());
    let now = objc2_foundation::NSProcessInfo::processInfo().systemUptime();
    let event = |kind, at| {
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind,
            at,
            NSEventModifierFlags::empty(),
            now,
            window.windowNumber(),
            None,
            0,
            1,
            1.,
        )
    };
    for i in 1..=8 {
        let t = i as f64 / 8.;
        let at = NSPoint::new(a.x + (z.x - a.x) * t, a.y + (z.y - a.y) * t);
        if let Some(e) = event(NSEventType::LeftMouseDragged, at) {
            app.postEvent_atStart(&e, false);
        }
    }
    if let (Some(down), Some(up)) = (
        event(NSEventType::LeftMouseDown, a),
        event(NSEventType::LeftMouseUp, z),
    ) {
        app.postEvent_atStart(&up, false);
        from.mouseDown(&down);
    }
}

/// Types `text` into the focused view, one key every 50 ms, as the text
/// system receives key presses once interpreted: `insertText:` for
/// characters, `doCommandBySelector: insertNewline:` for Return (`\n`).
fn post_typing(window: &NSWindow, text: &str) {
    for (i, c) in text.chars().enumerate() {
        let window = objc2::Message::retain(window);
        let at = std::time::Duration::from_millis(50 * (i as u64 + 1));
        on_main_after(at, move || {
            let Some(responder) = window.firstResponder() else {
                return;
            };
            if c == '\n' {
                let _: () =
                    unsafe { msg_send![&*responder, doCommandBySelector: sel!(insertNewline:)] };
            } else {
                let s = NSString::from_str(&c.to_string());
                let _: () = unsafe { msg_send![&*responder, insertText: &*s] };
            }
        });
    }
}

/// Posts key presses (`PRISM_KEYS`) by name, one every 100 ms; `cmd+c`,
/// `shift+ArrowRight` hold modifiers.
fn post_keys(window: &NSWindow, keys: &str) {
    use objc2_app_kit::NSEventType;
    for (i, combo) in keys.split('|').filter(|k| !k.is_empty()).enumerate() {
        // `cmd+c`, `shift+ArrowRight`: modifiers, then the key.
        let (mods, name) = match combo.rsplit_once('+') {
            Some((m, k)) if !k.is_empty() => (m, k),
            _ => ("", combo),
        };
        let (_, flags) = crate::menus::parse_shortcut(&format!("{mods}+x"));
        let (code, chars) = match name {
            "Escape" => (53, "\u{1b}"),
            "Enter" => (36, "\r"),
            "Tab" => (48, "\t"),
            "ArrowLeft" => (123, "\u{F702}"),
            "ArrowRight" => (124, "\u{F703}"),
            "ArrowDown" => (125, "\u{F701}"),
            "ArrowUp" => (126, "\u{F700}"),
            "a" => (0, "a"),
            "x" => (7, "x"),
            "c" => (8, "c"),
            "v" => (9, "v"),
            other => (0, other),
        };
        let window = objc2::Message::retain(window);
        let chars = chars.to_string();
        on_main_after(
            std::time::Duration::from_millis(300 + 100 * i as u64),
            move || {
                let s = NSString::from_str(&chars);
                let now = objc2_foundation::NSProcessInfo::processInfo().systemUptime();
                let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                NSEventType::KeyDown,
                NSPoint::ZERO,
                flags,
                now,
                window.windowNumber(),
                None,
                &s,
                &s,
                false,
                code,
            );
                if let Some(e) = event {
                    NSApplication::sharedApplication(window.mtm()).sendEvent(&e);
                }
            },
        );
    }
}

/// Writes `window` to a PNG with the app's other visible windows drawn over
/// it where they are on screen (pop-up menus, calendar popovers). Each is
/// drawn by the app itself: no screen recording permission needed.
fn snapshot_with_overlays(window: &NSWindow, path: &str) {
    use objc2_app_kit::NSGraphicsContext;
    let Some(content) = window.contentView() else {
        return;
    };
    let origin = window.contentRectForFrameRect(window.frame()).origin;
    let bounds = content.bounds();
    let Some(base) = content.bitmapImageRepForCachingDisplayInRect(bounds) else {
        return;
    };
    content.cacheDisplayInRect_toBitmapImageRep(bounds, &base);
    let Some(ctx) = NSGraphicsContext::graphicsContextWithBitmapImageRep(&base) else {
        return;
    };
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&ctx));
    let app = NSApplication::sharedApplication(window.mtm());
    for w in app.windows().iter() {
        if *w == *window || !w.isVisible() {
            continue;
        }
        // The transparent layer window draws only its content.
        let opaque = w.isOpaque();
        let Some(view) = w
            .contentView()
            .map(|v| unsafe { v.superview() }.unwrap_or(v))
        else {
            continue;
        };
        let r = view.bounds();
        let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(r) else {
            continue;
        };
        view.cacheDisplayInRect_toBitmapImageRep(r, &rep);
        let image = NSImage::initWithSize(objc2::AllocAnyThread::alloc(), r.size);
        image.addRepresentation(&rep);
        let f = w.frame();
        let at = NSRect::new(
            NSPoint::new(f.origin.x - origin.x, f.origin.y - origin.y),
            f.size,
        );
        if opaque {
            NSColor::windowBackgroundColor().setFill();
            objc2_app_kit::NSRectFill(at);
        }
        image.drawInRect(at);
    }
    // A custom chrome's traffic lights, over the content (#67): what they
    // cover shows in the snapshot.
    if let Some((rect, image)) = crate::chrome::lights_image(window) {
        image.drawInRect(rect);
    }
    ctx.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    let props = objc2_foundation::NSDictionary::new();
    let png = unsafe {
        base.representationUsingType_properties(objc2_app_kit::NSBitmapImageFileType::PNG, &props)
    };
    match png.map(|d| std::fs::write(path, d.to_vec())) {
        Some(Ok(())) => println!("prism-macos: snapshot written to {path}"),
        _ => eprintln!("prism-macos: cannot write {path}"),
    }
}

fn run_automation(mtm: MainThreadMarker) {
    let mut root = None;
    with_state(|s| root = Some(s.renderer.widgets.root.clone()));
    let Some(root) = root else { return };
    let sheet_roots = || {
        let mut roots: Vec<Retained<NSView>> = Vec::new();
        with_state(|s| {
            for sheet in &s.renderer.widgets.sheets {
                roots.push(Retained::into_super(sheet.content.clone()));
            }
            // Open sheets, dialogs and popovers.
            for o in &s.renderer.widgets.ix.overlays {
                if o.presented.get() {
                    roots.push(Retained::into_super(o.host.clone()));
                }
            }
        });
        roots
    };

    thread_local! {
        static CLICKED: Cell<bool> = const { Cell::new(false) };
    }
    if let Ok(clicks) = std::env::var("PRISM_CLICK")
        && !CLICKED.with(|c| c.replace(true))
    {
        for title in clicks.split('|').filter(|t| !t.is_empty()) {
            // `dblclick:#window@x,y`: a double click at that point (from
            // the window's top-left corner), through the window.
            if let Some(at) = title.strip_prefix("dblclick:#window@") {
                if let Some(window) = root.window() {
                    window_double_click(&window, at);
                }
                continue;
            }
            // Sheets first: they are in front.
            let found = sheet_roots()
                .iter()
                .find_map(|r| find_pressable(r, title))
                .or_else(|| find_pressable(&root, title));
            match found {
                Some(Pressable::Native(b)) => unsafe { b.performClick(None) },
                Some(Pressable::Custom(p)) => {
                    let _: objc2::runtime::Bool =
                        unsafe { msg_send![&*p, accessibilityPerformPress] };
                }
                // A clickable text, or an inline link, by its text.
                None if crate::label::press_in(&root, title) => {}
                // A tab of the tab bar, by its title.
                None if press_segment(&root, title) => {}
                None => eprintln!("prism-macos: PRISM_CLICK: nothing labelled {title:?}"),
            }
        }
    }

    // `PRISM_INCREMENT="Volume|Volume"`: VoiceOver's increment action on an
    // adjustable control (a slider), by accessibility label.
    thread_local! {
        static INCREMENTED: Cell<bool> = const { Cell::new(false) };
    }
    if let Ok(labels) = std::env::var("PRISM_INCREMENT")
        && !INCREMENTED.with(|c| c.replace(true))
    {
        for label in labels.split('|').filter(|t| !t.is_empty()) {
            match find_labelled(&root, label) {
                Some(v) => {
                    let _: objc2::runtime::Bool =
                        unsafe { msg_send![&*v, accessibilityPerformIncrement] };
                    let value: Option<Retained<NSObject>> =
                        unsafe { msg_send![&*v, accessibilityValue] };
                    println!(
                        "prism-macos: {label:?} reads {:?}",
                        value.map(|v| format!("{v:?}"))
                    );
                }
                None => eprintln!("prism-macos: PRISM_INCREMENT: nothing labelled {label:?}"),
            }
        }
    }

    // `PRISM_A11Y_PRESS="Rebook|Boarding|cell:1,3"`: VoiceOver's press on
    // the first element labelled (or titled) so, reached the way VoiceOver
    // reaches it: down the accessibility children, not the views; `cell:R,C`
    // is the first table's cell at row R, column C, as VoiceOver's table
    // navigation reaches it.
    thread_local! {
        static A11Y_PRESSED: Cell<bool> = const { Cell::new(false) };
    }
    if let Ok(labels) = std::env::var("PRISM_A11Y_PRESS")
        && !A11Y_PRESSED.with(|c| c.replace(true))
    {
        for label in labels.split('|').filter(|t| !t.is_empty()) {
            let found = match label.strip_prefix("cell:") {
                Some(at) => table_cell(&root, at),
                None => find_element(&root, label, 0),
            };
            match found {
                Some(e) => {
                    let done: objc2::runtime::Bool =
                        unsafe { msg_send![&*e, accessibilityPerformPress] };
                    println!(
                        "prism-macos: PRISM_A11Y_PRESS: {label:?} ({}) returned {}",
                        e.class().name().to_str().unwrap_or("?"),
                        done.as_bool()
                    );
                }
                None => eprintln!("prism-macos: PRISM_A11Y_PRESS: nothing labelled {label:?}"),
            }
        }
    }

    // `PRISM_AFTER=<seconds>`: let async answers (and a press's menu)
    // arrive before the snapshot.
    if let Some(after) = std::env::var("PRISM_AFTER")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        && (CLICKED.with(|c| c.get()) || A11Y_PRESSED.with(|c| c.get()))
    {
        thread_local! {
            static WAITED: Cell<bool> = const { Cell::new(false) };
        }
        if !WAITED.with(|w| w.replace(true)) {
            let mut target = None;
            with_state(|s| target = Some(s.renderer.widgets.target.clone()));
            if let Some(target) = target {
                let none: Option<&AnyObject> = None;
                let _: () = unsafe {
                    msg_send![&*target, performSelector: sel!(prismAutomation:), withObject: none, afterDelay: after]
                };
                return;
            }
        }
    }
    // `PRISM_SCROLL=<y>` scrolls the visible scroll views.
    if let Some(y) = std::env::var("PRISM_SCROLL")
        .ok()
        .and_then(|y| y.parse::<f64>().ok())
    {
        fn scroll_all(view: &NSView, y: f64) {
            if view.isHiddenOrHasHiddenAncestor() || view.window().is_none() {
                return;
            }
            if let Some(sv) = view.downcast_ref::<NSScrollView>() {
                // Text areas keep their own scroll position; rows that
                // scroll sideways (`scroll_x()`) don't scroll down.
                if !sv.hasVerticalScroller()
                    || sv
                        .documentView()
                        .is_some_and(|d| d.isKindOfClass(PrismTextView::class()))
                {
                    return;
                }
                let clip = sv.contentView();
                clip.scrollToPoint(NSPoint::new(0., y));
                sv.reflectScrolledClipView(&clip);
            }
            for child in view.subviews().iter() {
                scroll_all(&child, y);
            }
        }
        scroll_all(&root, y);
    }

    // `PRISM_HOVER=<label>`: the view with that accessibility label is
    // hovered, as when the pointer enters its tracking area (snapshots of
    // `.hover(|s| …)` styles without moving the user's pointer).
    thread_local! {
        static HOVERED: Cell<bool> = const { Cell::new(false) };
    }
    if let Ok(label) = std::env::var("PRISM_HOVER")
        && !HOVERED.with(|h| h.replace(true))
    {
        match find_labelled(&root, &label).and_then(|v| v.downcast::<PrismView>().ok()) {
            Some(v) => report_state(v.ivars().id.get(), prism_core::Interaction::Hover, true),
            None => eprintln!("prism-macos: PRISM_HOVER: no Prism view labelled {label:?}"),
        }
    }

    // `PRISM_TAP=<label>`: a click (mouse down and up) in the middle of the
    // view with that accessibility label, title or placeholder: opens a
    // pop-up's menu, focuses a field (a date picker's calendar only opens
    // in an active app). Then `PRISM_TYPE=<text>` types into the focused view (`\n`: Return). The
    // snapshot, `PRISM_TAP_DELAY` seconds later (default 1), includes the
    // menus and popovers open over the window.
    thread_local! {
        static TAPPED: Cell<bool> = const { Cell::new(false) };
    }
    if let Ok(label) = std::env::var("PRISM_TAP")
        && !TAPPED.with(|t| t.replace(true))
        && let Some(window) = root.window()
    {
        let found = sheet_roots()
            .iter()
            .find_map(|r| find_labelled(r, &label))
            .or_else(|| find_labelled(&root, &label));
        let Some(target) = found else {
            eprintln!("prism-macos: PRISM_TAP: nothing labelled {label:?}");
            std::process::exit(1);
        };
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        window.setLevel(objc2_app_kit::NSFloatingWindowLevel);
        window.orderFrontRegardless();
        window.makeKeyWindow();
        println!(
            "prism-macos: PRISM_TAP: clicking a {} (app active: {}, key window: {})",
            target.class().name().to_str().unwrap_or("?"),
            NSApplication::sharedApplication(mtm).isActive(),
            window.isKeyWindow()
        );
        // A window that isn't key (the app may not get activated from a
        // terminal) swallows the first click of most views: focus directly.
        if target.acceptsFirstResponder() {
            window.makeFirstResponder(Some(&target));
        }
        // Menus and calendars track the mouse in a loop of their own: click
        // on the next turn, after scheduling the snapshot.
        let (w, t) = (window.clone(), target.clone());
        // `PRISM_DRAG_TO=<label>`: the press drags from a third of the way
        // into the tapped view to a third of the way into that one (#231).
        let to = std::env::var("PRISM_DRAG_TO")
            .ok()
            .and_then(|l| find_labelled(&root, &l));
        on_main_after(std::time::Duration::ZERO, move || match to {
            Some(to) => post_drag(&w, &t, &to),
            None => post_click(&w, &t),
        });
        if let Ok(text) = std::env::var("PRISM_TYPE") {
            post_typing(&window, &text.replace("\\n", "\n"));
        }
        // `PRISM_KEYS="ArrowRight|ArrowDown|cmd+c"`: key presses, as the
        // keyboard sends them (through the app's event monitors).
        if let Ok(keys) = std::env::var("PRISM_KEYS") {
            post_keys(&window, &keys);
        }
        let delay = std::env::var("PRISM_TAP_DELAY")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(1.);
        on_main_after(std::time::Duration::from_secs_f64(delay), move || {
            if let Ok(path) = std::env::var("PRISM_SNAPSHOT") {
                snapshot_with_overlays(&window, &path);
            }
            std::process::exit(0);
        });
        return;
    }

    with_state(|s| {
        for title in s.services.status_titles() {
            println!("prism-macos: menu bar item: {title:?}");
        }
        if let Some(sheet) = s
            .renderer
            .widgets
            .root
            .window()
            .and_then(|w| w.attachedSheet())
        {
            println!(
                "prism-macos: attached sheet: {}",
                sheet.class().name().to_str().unwrap_or("?")
            );
        }
    });
    // `PRISM_A11Y_DUMP=tree.json`: the native view / accessibility tree.
    if let Ok(path) = std::env::var("PRISM_A11Y_DUMP") {
        crate::a11y_dump::write(&path, &root, &sheet_roots());
        if std::env::var_os("PRISM_SNAPSHOT").is_none() {
            std::process::exit(0);
        }
    }
    if let Ok(path) = std::env::var("PRISM_SNAPSHOT") {
        for (i, sheet) in sheet_roots().iter().enumerate() {
            write_png(sheet, &format!("{path}.sheet{i}.png"));
        }
        // An alert (`cx.show_alert`: NSAlert's sheet). The system shows it
        // in a glass window of its own that no view draws: its content
        // (title, message, buttons) goes to `<path>.alert.png`.
        if let Some(alert) = root
            .window()
            .and_then(|w| w.attachedSheet())
            .filter(|s| s.class().name().to_str().is_ok_and(|n| n.contains("Alert")))
            && let Some(content) = alert.contentView()
        {
            write_png(&content, &format!("{path}.alert.png"));
            println!("prism-macos: alert snapshot written to {path}.alert.png");
        }
        // Layers (toasts) and popovers drawn over the window, and a custom
        // chrome's traffic lights (#67).
        if let Some(window) = root.window()
            && (std::env::var_os("PRISM_SNAPSHOT_OVERLAYS").is_some()
                || crate::chrome::controls(&window).is_some())
        {
            snapshot_with_overlays(&window, &path);
            std::process::exit(0);
        }
        root.displayIfNeeded();
        let bounds = root.bounds();
        if let Some(rep) = root.bitmapImageRepForCachingDisplayInRect(bounds) {
            root.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
            let props = objc2_foundation::NSDictionary::new();
            let png = unsafe {
                rep.representationUsingType_properties(
                    objc2_app_kit::NSBitmapImageFileType::PNG,
                    &props,
                )
            };
            match png {
                Some(data) => match std::fs::write(&path, data.to_vec()) {
                    Ok(()) => println!(
                        "prism-macos: snapshot written to {path}; {}",
                        crate::trace::memory()
                    ),
                    Err(e) => eprintln!("prism-macos: cannot write {path}: {e}"),
                },
                None => eprintln!("prism-macos: PNG encoding failed"),
            }
        }
        // `terminate:` is refused while a sheet is attached; this is a dev hook.
        let _ = mtm;
        std::process::exit(0);
    }
}

// ---------------------------------------------------------------------------
// Async: wake the UI thread through GCD, and schedule the next timer.
// ---------------------------------------------------------------------------

fn wake_ui() -> prism_core::WakeUi {
    std::sync::Arc::new(|| dispatch2::DispatchQueue::main().exec_async(poll_now))
}

/// Runs ready tasks and applies what they changed.
fn poll_now() {
    with_state(|s| {
        let patches = s.runtime.poll();
        s.commit(patches);
    });
    flush_responses();
    check_background();
}

thread_local! {
    static NEXT_WAKE: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

/// Makes sure the UI thread wakes up when the next timer is due.
fn schedule_timer(at: Option<std::time::Instant>) {
    let Some(at) = at else { return };
    if NEXT_WAKE.with(|w| w.get().is_some_and(|w| w <= at)) {
        return;
    }
    NEXT_WAKE.with(|w| w.set(Some(at)));
    let delay = at.saturating_duration_since(std::time::Instant::now());
    let when = dispatch2::DispatchTime::try_from(delay).unwrap_or(dispatch2::DispatchTime::NOW);
    let _ = dispatch2::DispatchQueue::main().after(when, || {
        NEXT_WAKE.with(|w| w.set(None));
        poll_now();
    });
}

/// `~/Library/Application Support/<bundle id>` (or `prism.<title>` when run
/// unbundled with `cargo run`). `PRISM_DATA_DIR` overrides it.
fn data_dir(title: &str) -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("PRISM_DATA_DIR") {
        return dir.into();
    }
    let id = objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let slug: String = title
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect();
            format!("prism.{slug}")
        });
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home)
        .join("Library/Application Support")
        .join(id)
}

fn write_png(view: &NSView, path: &str) {
    view.displayIfNeeded();
    let bounds = view.bounds();
    if let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(bounds) {
        view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        let props = objc2_foundation::NSDictionary::new();
        if let Some(data) = unsafe {
            rep.representationUsingType_properties(
                objc2_app_kit::NSBitmapImageFileType::PNG,
                &props,
            )
        } {
            let _ = std::fs::write(path, data.to_vec());
        }
    }
}

/// Assets: those compiled into the binary, the bundle's Resources/assets,
/// then the dev directory; downloads are cached in ~/Library/Caches.
fn image_sources(
    dev_assets: Option<&str>,
    embedded: &prism_core::EmbeddedAssets,
) -> prism_native::images::ImageSources {
    let mut asset_dirs = Vec::new();
    if let Some(res) = objc2_foundation::NSBundle::mainBundle().resourcePath() {
        asset_dirs.push(std::path::PathBuf::from(res.to_string()).join("assets"));
    }
    if let Some(dev) = dev_assets {
        asset_dirs.push(dev.into());
    }
    // Fonts in assets/fonts: found by family name from now on.
    prism_native::fonts::register_apple_fonts(&asset_dirs, embedded);
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    prism_native::images::ImageSources {
        embedded: embedded.clone(),
        asset_dirs,
        cache_dir: Some(std::path::PathBuf::from(home).join("Library/Caches/prism-images")),
    }
}

/// Locale and Reduce Motion (`PRISM_TEXT_SCALE` scales text, macOS has no
/// Dynamic Type).
fn mac_environment() -> prism_core::Environment {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    prism_core::Environment {
        locale: prism_native::apple::system_locale(),
        text_scale: std::env::var("PRISM_TEXT_SCALE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1.),
        reduce_motion: workspace.accessibilityDisplayShouldReduceMotion(),
        utc_offset: prism_native::apple::utc_offset(),
    }
}

fn observe_system(target: &Target) {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    unsafe {
        workspace
            .notificationCenter()
            .addObserver_selector_name_object(
                target,
                sel!(onEnvironment:),
                Some(objc2_app_kit::NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                None,
            );
        let center = objc2_foundation::NSNotificationCenter::defaultCenter();
        center.addObserver_selector_name_object(
            target,
            sel!(onEnvironment:),
            Some(objc2_foundation::NSCurrentLocaleDidChangeNotification),
            None,
        );
        center.addObserver_selector_name_object(
            target,
            sel!(onTextSelection:),
            Some(objc2_app_kit::NSTextViewDidChangeSelectionNotification),
            None,
        );
        center.addObserver_selector_name_object(
            target,
            sel!(onClipScroll:),
            Some(objc2_app_kit::NSViewBoundsDidChangeNotification),
            None,
        );
        center.addObserver_selector_name_object(
            target,
            sel!(onCloudChange:),
            Some(objc2_foundation::NSUbiquitousKeyValueStoreDidChangeExternallyNotification),
            None,
        );
        use objc2_app_kit::*;
        for name in [
            NSApplicationDidBecomeActiveNotification,
            NSApplicationDidResignActiveNotification,
            NSApplicationDidHideNotification,
            NSApplicationDidUnhideNotification,
            NSWindowDidMiniaturizeNotification,
            NSWindowDidDeminiaturizeNotification,
        ] {
            center.addObserver_selector_name_object(target, sel!(onLifecycle:), Some(name), None);
        }
    }
}

/// Runs `f` (which changes `animator()` proxies) in an animation group,
/// after `t`'s delay.
fn animate(t: prism_core::Transition, f: impl Fn() + 'static) {
    let run = move || {
        let block = block2::RcBlock::new(
            move |ctx: std::ptr::NonNull<objc2_app_kit::NSAnimationContext>| {
                let ctx = unsafe { ctx.as_ref() };
                // Springs overshoot: approximate with a slightly longer ease.
                let duration = match t.curve {
                    prism_core::Curve::Spring => t.duration * 1.2,
                    _ => t.duration,
                };
                ctx.setDuration(duration as f64);
                ctx.setTimingFunction(Some(&prism_native::apple::motion::timing(t)));
                ctx.setAllowsImplicitAnimation(true);
                f();
            },
        );
        objc2_app_kit::NSAnimationContext::runAnimationGroup(&block);
    };
    if t.delay > 0. {
        on_main_after(std::time::Duration::from_secs_f32(t.delay), run);
    } else {
        run();
    }
}

type Later = (u64, HashMap<u64, Box<dyn FnOnce()>>);

thread_local! {
    /// Callbacks waiting for `on_main_after`, by number (the last one given).
    static LATER: RefCell<Later> = RefCell::new((0, HashMap::new()));
}

/// Runs `f` on the main thread after `delay` (`f` need not be `Send`: it
/// stays on this thread, only its number travels).
/// Nothing runs once the thread's state is being torn down (a view removed
/// while the app quits can still move the focus).
pub(crate) fn on_main_after(delay: std::time::Duration, f: impl FnOnce() + 'static) {
    let Ok(id) = LATER.try_with(|l| {
        let mut l = l.borrow_mut();
        l.0 += 1;
        let id = l.0;
        l.1.insert(id, Box::new(f));
        id
    }) else {
        return;
    };
    let when = dispatch2::DispatchTime::try_from(delay).unwrap_or(dispatch2::DispatchTime::NOW);
    let _ = dispatch2::DispatchQueue::main().after(when, move || {
        if let Some(f) = LATER.with(|l| l.borrow_mut().1.remove(&id)) {
            f();
        }
    });
}

thread_local! {
    /// Notification responses that arrived before the app was ready (launch
    /// from a notification).
    static PENDING_RESPONSES: RefCell<Vec<(prism_core::NotificationResponse, Option<String>)>> =
        const { RefCell::new(Vec::new()) };
}

pub(crate) fn notification_response(
    response: prism_core::NotificationResponse,
    link: Option<String>,
) {
    PENDING_RESPONSES.with(|p| p.borrow_mut().push((response, link)));
    flush_responses();
}

/// A push token or a registration error.
pub(crate) fn push_registered(
    result: Result<prism_core::PushSubscription, prism_core::DeviceError>,
) {
    if let Err(e) = &result {
        eprintln!("prism-macos: push registration failed: {e}");
    }
    with_state(|s| {
        let patches = s.runtime.push_registered(result);
        s.commit(patches);
    });
}

/// A push arrived (macOS does not launch apps for pushes: the app runs).
pub(crate) fn push_received(message: prism_core::PushMessage) {
    with_state(|s| {
        let patches = if message.event == prism_core::PushEvent::Background {
            s.runtime.run_background_push(message);
            s.runtime.poll()
        } else {
            s.runtime.push_received(message, false)
        };
        s.commit(patches);
    });
}

fn flush_responses() {
    let pending = PENDING_RESPONSES.with(|p| std::mem::take(&mut *p.borrow_mut()));
    if pending.is_empty() {
        return;
    }
    let mut handled = false;
    with_state(|s| {
        for (response, link) in pending.iter().cloned() {
            let patches = s.runtime.notification_response(response, link);
            s.commit(patches);
        }
        handled = true;
    });
    if !handled {
        PENDING_RESPONSES.with(|p| {
            p.borrow_mut().splice(0..0, pending);
        });
    }
}

// ---------------------------------------------------------------------------
// Background tasks
// ---------------------------------------------------------------------------

thread_local! {
    /// Running background tasks and who to tell when they finish.
    static BG_WAITING: RefCell<Vec<(u64, std::sync::mpsc::Sender<()>)>> =
        const { RefCell::new(Vec::new()) };
}

/// `PRISM_BACKGROUND_INTERVAL=<seconds>` shortens every background task's
/// interval (development).
pub(crate) fn background_interval(every: std::time::Duration) -> std::time::Duration {
    std::env::var("PRISM_BACKGROUND_INTERVAL")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .map_or(every, std::time::Duration::from_secs_f64)
}

/// Runs background task `id`; `done` is signalled when it completes.
pub(crate) fn run_background(id: &str, done: std::sync::mpsc::Sender<()>) {
    let mut started = None;
    with_state(|s| {
        started = s.runtime.run_background_task(id);
        let patches = s.runtime.poll();
        s.commit(patches);
    });
    eprintln!("prism-macos: background task {id:?} started");
    match started {
        Some(task) => BG_WAITING.with(|w| w.borrow_mut().push((task, done))),
        None => {
            let _ = done.send(());
        }
    }
    check_background();
}

fn check_background() {
    with_state(|s| {
        BG_WAITING.with(|w| {
            w.borrow_mut().retain(|(task, done)| {
                let running = s.runtime.is_task_running(*task);
                if !running {
                    eprintln!("prism-macos: background task finished");
                    let _ = done.send(());
                }
                running
            })
        })
    });
}

/// Answers from the device bridge (on the main thread). An answer given
/// while the app is busy (synchronously, during the call) is handled on the
/// next turn of the main loop.
fn on_device(message: prism_native::apple::DeviceMessage) {
    let mut message = Some(message);
    with_state(|s| {
        let patches = match message.take().expect("message") {
            prism_native::apple::DeviceMessage::Reply(id, result) => {
                s.runtime.device_reply(id, result)
            }
            prism_native::apple::DeviceMessage::Event(id, event) => {
                s.runtime.device_event(id, event)
            }
        };
        s.commit(patches);
    });
    // Busy (not just absent): try again on the next turn.
    let busy = STATE.with(|c| c.try_borrow().is_err());
    if let Some(message) = message
        && busy
    {
        dispatch2::DispatchQueue::main().exec_async(move || on_device(message));
    }
}

/// iCloud key-value storage when the bundle enables it, else local files.
fn cloud_storage(dir: &std::path::Path) -> Box<dyn prism_core::Storage> {
    match prism_native::apple::ICloudStorage::new() {
        Some(icloud) => Box::new(icloud),
        None => Box::new(prism_core::FileStorage::new(dir.join("cloud"))),
    }
}

/// `PRISM_APPEARANCE=light|dark` in place of the system's appearance
/// (development, snapshots: the app's own choice still wins).
fn env_appearance() -> Option<Retained<objc2_app_kit::NSAppearance>> {
    let name = match std::env::var("PRISM_APPEARANCE").as_deref() {
        Ok("dark") => unsafe { objc2_app_kit::NSAppearanceNameDarkAqua },
        Ok("light") => unsafe { objc2_app_kit::NSAppearanceNameAqua },
        _ => return None,
    };
    objc2_app_kit::NSAppearance::appearanceNamed(name)
}

/// The configured window size; `PRISM_WINDOW=1200x800` overrides it.
fn window_size(config: &AppConfig) -> (f32, f32) {
    std::env::var("PRISM_WINDOW")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
        })
        .unwrap_or((config.width, config.height))
}

#[cfg(test)]
mod escape_tests {
    use super::*;

    /// #163: Escape outside a sheet reaches a Prism view as
    /// `cancelOperation:` (AppKit's key bindings send it up the responder
    /// chain from a focused field). It crashed: NSView has no such method.
    #[test]
    fn escape_outside_a_sheet_does_not_crash() {
        // Views are only built here, never shown: no main thread needed.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let none: Option<&AnyObject> = None;
        let parent = PrismView::new(mtm, 1);
        let view = PrismView::new(mtm, 2);
        parent.addSubview(&view);
        // Up the chain: the parent does not handle it either.
        let _: () = unsafe { msg_send![&*view, cancelOperation: none] };
        // Without a next responder.
        let lone = PrismView::new(mtm, 3);
        let _: () = unsafe { msg_send![&*lone, cancelOperation: none] };
    }
}

#[cfg(test)]
mod text_area_tests {
    use super::*;

    /// #179: the text area's default look keeps AppKit's 12 pt before the
    /// text (7 inset + 5 padding), now 10 + the label's 2, so `p(0.)` only
    /// leaves the 2 a label has too.
    #[test]
    fn the_default_inset_keeps_appkits_look() {
        assert_eq!(TEXT_AREA_INSET.width + LABEL_FRAGMENT_PADDING, 7. + 5.);
        assert_eq!(TEXT_AREA_INSET.height, 8.);
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;
    use objc2_app_kit::NSEventType as T;

    /// #177: a click on a clickable view does not focus it (no ring)
    /// unless Full Keyboard Access is on or the app asks; `on_key_down`
    /// views and a sheet's root always take the focus.
    #[test]
    fn clickable_views_take_the_focus_like_native_buttons() {
        let role = |clickable, sheet_root, keys| FocusRole {
            clickable,
            sheet_root,
            keys,
        };
        assert!(!takes_focus(role(true, false, false), false));
        assert!(takes_focus(role(true, false, false), true));
        assert!(takes_focus(role(false, false, true), false));
        assert!(takes_focus(role(false, true, false), false));
        assert!(!takes_focus(role(false, false, false), true));
    }

    /// #177: a focus from a mouse press draws no ring; Tab (a key event)
    /// and `cx.focus` do.
    #[test]
    fn only_a_mouse_focus_hides_the_ring() {
        assert!(focused_by_mouse(Some(T::LeftMouseDown), false));
        assert!(focused_by_mouse(Some(T::RightMouseDown), false));
        assert!(focused_by_mouse(Some(T::OtherMouseDown), false));
        assert!(!focused_by_mouse(Some(T::LeftMouseDown), true));
        assert!(!focused_by_mouse(Some(T::KeyDown), false));
        assert!(!focused_by_mouse(None, false));
    }
}
