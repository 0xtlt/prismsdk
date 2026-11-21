use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSString};
use objc2_ui_kit::{
    NSObjectUIAccessibility, NSObjectUIAccessibilityContainer, NSTextAlignment,
    UIAccessibilityTraitButton, UIAccessibilityTraitHeader, UIAccessibilityTraitImage,
    UIAccessibilityTraitLink, UIAccessibilityTraitNotEnabled, UIAccessibilityTraitSelected,
    UIAccessibilityTraitStaticText, UIAccessibilityTraitTabBar, UIAccessibilityTraitToggleButton,
    UIAdaptivePresentationControllerDelegate, UIApplication, UIApplicationDelegate, UIButton,
    UIButtonType, UIColor, UIContextMenuInteractionDelegate, UIControlEvents, UIControlState,
    UIFont, UIGestureRecognizerDelegate, UIImage, UIImageSymbolConfiguration, UIImageView, UILabel,
    UINavigationController, UINavigationControllerDelegate, UIProgressView, UIResponder, UIScene,
    UISceneConnectionOptions, UISceneDelegate, UISceneSession, UIScrollView,
    UIScrollViewContentInsetAdjustmentBehavior, UIScrollViewDelegate,
    UIScrollViewKeyboardDismissMode, UISwitch, UITabBarController, UITabBarControllerDelegate,
    UITapGestureRecognizer, UITextBorderStyle, UITextField, UITextFieldDelegate,
    UITextViewDelegate, UITraitCollection, UIUserInterfaceStyle, UIView, UIViewController,
    UIWindow, UIWindowScene, UIWindowSceneDelegate,
};
use prism_core::{
    Accessibility, AppConfig, AppInit, Appearance, Backend, Color, FontWeight, NodeId, NodeKind,
    Role, Runtime, Style, TextAlign, UiEvent,
};
use prism_native::{Insets, NativeRenderer, Viewport, Widgets};

static APP: OnceLock<(AppConfig, AppInit)> = OnceLock::new();

/// Toggles: a switch on iOS; tvOS has none (UISwitch.h: API_UNAVAILABLE(tvos)),
/// so a button showing a check mark, operated with the remote.
#[cfg(not(target_os = "tvos"))]
type ToggleView = UISwitch;
#[cfg(target_os = "tvos")]
type ToggleView = UIButton;

/// Sliders and date pickers: UIKit's on iOS and visionOS; tvOS has neither
/// (UISlider.h, UIDatePicker.h: API_UNAVAILABLE(tvos)), so a focusable view
/// adjusted with the remote.
#[cfg(not(target_os = "tvos"))]
type SliderView = crate::forms::PrismSlider;
#[cfg(target_os = "tvos")]
type SliderView = crate::tv::Adjustable;
#[cfg(not(target_os = "tvos"))]
type DateView = objc2_ui_kit::UIDatePicker;
#[cfg(target_os = "tvos")]
type DateView = crate::tv::Adjustable;

// ---------------------------------------------------------------------------
// Main-thread state
// ---------------------------------------------------------------------------

struct State {
    runtime: Runtime,
    renderer: NativeRenderer<UiKit>,
    services: crate::services::IosServices,
    /// The window size `device_info` was last reported for.
    device_size: Option<(f32, f32)>,
    /// The last `report_window` re-rendered something (#28).
    window_rerendered: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// UIKit laid views out while Prism was busy (see [`relayout`]).
    static RELAYOUT_MISSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether a key pressed in `focused` is the app's alone (`consume_keys`,
/// #181).
#[cfg(not(target_os = "tvos"))]
pub(crate) fn consumes_key(
    focused: Option<NodeId>,
    key: &str,
    edges: Option<prism_core::CaretEdges>,
) -> bool {
    let mut out = false;
    with_state(|s| out = s.runtime.consumes_key(focused, key, edges));
    out
}

/// Whether `key` pressed in `focused` is the app's alone, a selection
/// group's keys included (#231).
#[cfg(not(target_os = "tvos"))]
pub(crate) fn consumes_key_event(focused: Option<NodeId>, key: &prism_core::KeyEvent) -> bool {
    let mut out = false;
    with_state(|s| out = s.runtime.consumes_key_event(focused, key));
    out
}

/// The UIKit content type of an HTML `autocomplete` value (#203): its
/// first token UIKit knows (`username webauthn` is a username field).
fn text_content_type(value: &str) -> Option<&'static objc2_ui_kit::UITextContentType> {
    use objc2_ui_kit::*;
    value.split_whitespace().find_map(|token| unsafe {
        Some(match token {
            "username" => UITextContentTypeUsername,
            "current-password" => UITextContentTypePassword,
            "new-password" => UITextContentTypeNewPassword,
            "one-time-code" => UITextContentTypeOneTimeCode,
            "email" => UITextContentTypeEmailAddress,
            "name" => UITextContentTypeName,
            "tel" => UITextContentTypeTelephoneNumber,
            "postal-code" => UITextContentTypePostalCode,
            "url" => UITextContentTypeURL,
            _ => return None,
        })
    })
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

/// For the development hooks.
pub(crate) fn dispatch_event(event: UiEvent) {
    dispatch(event);
}

/// Lays everything out again, measures included (a video's size became
/// known, #207).
pub(crate) fn relayout_all() {
    with_state(|s| {
        s.renderer.mark_all_dirty();
        s.relayout();
    });
}

/// A text field of a selection group (#231), as a pointer drag looks for
/// it.
#[cfg(not(target_os = "tvos"))]
pub(crate) enum GroupField {
    Area(Retained<objc2_ui_kit::UITextView>),
    Input(Retained<UITextField>),
}

/// The text fields of selection group `group` (#231), in document order.
#[cfg(not(target_os = "tvos"))]
pub(crate) fn group_fields(group: NodeId) -> Vec<(NodeId, GroupField)> {
    let mut out = Vec::new();
    with_state(|s| {
        out = s
            .runtime
            .selection_group_fields(group)
            .into_iter()
            .filter_map(|f| Some((f, crate::across::field_view(s.renderer.view(f)?)?)))
            .collect();
    });
    out
}

/// Whether `text`, about to be typed into field `field`, goes to its
/// selection group (#231, `Runtime::takes_text_across`): then the field
/// refuses it and the group hears it once UIKit's callback returned.
fn type_across(field: NodeId, text: &NSString) -> bool {
    let mut takes = false;
    with_state(|s| takes = s.runtime.takes_text_across(field));
    if takes {
        let text = text.to_string();
        dispatch2::DispatchQueue::main()
            .exec_async(move || dispatch(UiEvent::TypeAcross(field, text)));
    }
    takes
}

pub(crate) fn dispatch(event: UiEvent) {
    with_state(|s| {
        // A draggable's drag, with the drop target under the finger (#251).
        let (event, relayout) = s.renderer.locate_drag(event);
        if relayout {
            s.relayout();
        }
        let patches = s.runtime.dispatch(event);
        s.commit(patches);
        if s.renderer.autoscroll_due() {
            schedule_autoscroll();
        }
    });
}

thread_local! {
    /// An autoscroll step is scheduled (#251).
    static AUTOSCROLL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// While a dragged payload stays near a scroll view's edge (#251): a step
/// every frame, the drag reported again over what moved under it.
fn schedule_autoscroll() {
    if AUTOSCROLL.replace(true) {
        return;
    }
    let when = dispatch2::DispatchTime::try_from(prism_native::dnd::AUTOSCROLL_INTERVAL)
        .unwrap_or(dispatch2::DispatchTime::NOW);
    let _ = dispatch2::DispatchQueue::main().after(when, || {
        AUTOSCROLL.set(false);
        let mut step = None;
        with_state(|s| {
            if let Some((event, relayout)) = s.renderer.autoscroll() {
                if relayout {
                    s.relayout();
                }
                step = Some(event);
            }
        });
        if let Some(event) = step {
            dispatch(event);
        }
    });
}

/// A view entered or left an interaction state (hover, press, focus):
/// restyled on the next turn (UIKit may call while views change).
pub(crate) fn set_interaction(id: NodeId, state: prism_core::Interaction, on: bool) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        with_state(|s| {
            if s.renderer.set_interaction(id, state, on) {
                s.relayout();
            }
            // The app hears focus changes (#224).
            for e in s.renderer.take_focus_changes() {
                let patches = s.runtime.dispatch(e);
                s.commit(patches);
            }
        })
    });
}

/// Opens a deep link (widget, Live Activity or notification tap).
pub(crate) fn open_link(path: &str) {
    with_state(|s| {
        let patches = s.runtime.open_link(path);
        s.commit(patches);
    });
}

/// `prism-counter:/todos/1` → `/todos/1`.
fn link_from_url(url: &objc2_foundation::NSURL) -> Option<String> {
    let s = url.absoluteString()?.to_string();
    let path = s.split_once(':')?.1.trim_start_matches("//");
    Some(if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    })
}

impl State {
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
            prism_core::PlatformServices::handle(&mut self.services, request);
        }
        schedule_timer(self.runtime.next_timer());
        self.sync_ticker();
    }

    /// Ticks countdowns every second, only while one is shown: otherwise
    /// the app never wakes up for them.
    fn sync_ticker(&self) {
        let needed = !self.renderer.widgets.timers.is_empty();
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

    fn relayout(&mut self) {
        RELAYOUT_MISSED.set(false);
        let size = self.renderer.widgets.root.bounds().size;
        let (w, h) = self.report_window().unwrap_or((0., 0.));
        // Rotation, Split View: `cx.device_info()` (re-renders if it changed).
        if w > 0.
            && self.device_size != Some((w, h))
            && let Some(mtm) = MainThreadMarker::new()
            && let Some(info) = crate::device_info::current(&self.renderer.widgets.root, mtm)
        {
            self.device_size = Some((w, h));
            let patches = self.runtime.set_device_info(info);
            if !patches.is_empty() {
                self.renderer.apply(&patches);
            }
        }
        self.layout_root(size);
        // A root tab bar's own insets (a sidebar, a side tab bar) are known
        // once UIKit laid it out: lay out again if they changed the app.
        if let Some(v) = self
            .renderer
            .widgets
            .root_tabs
            .as_ref()
            .and_then(|t| t.view())
        {
            v.layoutIfNeeded();
            if self.report_window().is_some() && self.window_rerendered {
                self.layout_root(size);
            }
        }
        // A screen UIKit sized meanwhile (a tab shown for the first time,
        // laid out by `layoutIfNeeded` above): in this frame, not the next.
        if RELAYOUT_MISSED.take() {
            self.layout_root(self.renderer.widgets.root.bounds().size);
        }
        // A new window size can show or hide a countdown.
        self.sync_ticker();
    }

    /// Tells the app its window and what the system covers of it
    /// (`cx.viewport()`, `cx.safe_area()`; #28): re-renders when it crosses a
    /// breakpoint (rotation, split view, resize) or the insets move. The app
    /// sees Prism points (window / zoom: TVs). Returns the window size.
    fn report_window(&mut self) -> Option<(f32, f32)> {
        self.window_rerendered = false;
        let size = self.renderer.widgets.root.bounds().size;
        let zoom = self.renderer.zoom();
        let (w, h) = (size.width as f32 / zoom, size.height as f32 / zoom);
        if w <= 0. {
            return None;
        }
        let i = self.renderer.widgets.content_insets();
        let safe = prism_core::Edges {
            top: i.top / zoom,
            right: i.right / zoom,
            bottom: i.bottom / zoom,
            left: i.left / zoom,
        };
        if (w, h) != self.runtime.app.window_size() || safe != self.runtime.app.safe_area() {
            let patches = self.runtime.set_window(w, h, safe);
            if !patches.is_empty() {
                self.renderer.apply(&patches);
                self.window_rerendered = true;
            }
        }
        Some((w, h))
    }

    fn layout_root(&mut self, size: CGSize) {
        let root = &self.renderer.widgets.root;
        let i = root.safeAreaInsets();
        let insets = Insets {
            top: i.top as f32,
            right: i.right as f32,
            bottom: i.bottom as f32,
            left: i.left as f32,
        };
        self.renderer
            .layout(size.width as f32, size.height as f32, insets);
        self.renderer.widgets.attach_layers();
        crate::interaction::claim_keys(&self.renderer.widgets.controller);
        // Elements that report their frame and moved (#192).
        let more = prism_native::report_layout(&mut self.runtime, &mut self.renderer);
        if !more.is_empty() {
            self.commit(more);
        }
    }
}

thread_local! {
    /// The countdowns' ticker (`State::sync_ticker`).
    static TICKER: std::cell::RefCell<Option<Retained<objc2_foundation::NSTimer>>> =
        const { std::cell::RefCell::new(None) };
}

/// Lays everything out again (UIKit laid out a controller's views, an
/// overlay controller sized them). Called while Prism lays out, the layout
/// running does it again before returning.
pub(crate) fn relayout() {
    let mut ran = false;
    with_state(|s| {
        ran = true;
        s.relayout();
    });
    if !ran {
        RELAYOUT_MISSED.set(true);
    }
}

/// The user closed overlay `id` (swipe down, tap outside, Escape, Menu),
/// and how (#156).
pub(crate) fn overlay_closed(id: NodeId, reason: prism_core::DismissReason) {
    with_state(|s| {
        if let Some(o) = s.renderer.widgets.overlays.get(&id) {
            o.open.set(false);
        }
    });
    dispatch(UiEvent::Dismiss(id, reason));
}

// ---------------------------------------------------------------------------
// Objective-C classes
// ---------------------------------------------------------------------------

thread_local! {
    /// What each shape draws, in points, by node id.
    static SHAPES: RefCell<HashMap<NodeId, Vec<prism_core::Path>>> = RefCell::new(HashMap::new());
}

define_class!(
    /// Draws a shape's paths. Colors resolve while drawing, and UIKit redraws
    /// views when the appearance changes, so dark mode follows by itself.
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismShapeView"]
    pub struct ShapeView;

    impl ShapeView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _rect: CGRect) {
            SHAPES.with(|s| {
                if let Some(paths) = s.borrow().get(&(self.tag() as NodeId)) {
                    draw_paths(paths);
                }
            });
        }
    }
);

/// Fills, then strokes, each path with UIKit Bézier paths.
fn draw_paths(paths: &[prism_core::Path]) {
    use objc2_ui_kit::UIBezierPath;
    use prism_core::Segment as S;
    for p in paths {
        let b = UIBezierPath::bezierPath();
        let pt = |x: f32, y: f32| CGPoint::new(x as f64, y as f64);
        for s in &p.segments {
            match *s {
                S::Move(x, y) => b.moveToPoint(pt(x, y)),
                S::Line(x, y) => b.addLineToPoint(pt(x, y)),
                S::Cubic(x1, y1, x2, y2, x, y) => {
                    b.addCurveToPoint_controlPoint1_controlPoint2(pt(x, y), pt(x1, y1), pt(x2, y2))
                }
                S::Close => b.closePath(),
                S::Arc { .. } => {}
            }
        }
        if let Some(c) = p.fill {
            b.setUsesEvenOddFillRule(p.even_odd);
            ui_color(c).setFill();
            b.fill();
        }
        if let Some(c) = p.stroke {
            b.setLineWidth(p.width as f64);
            if p.round {
                // kCGLineCapRound, kCGLineJoinRound.
                let _: () = unsafe { msg_send![&b, setLineCapStyle: 1i32] };
                let _: () = unsafe { msg_send![&b, setLineJoinStyle: 1i32] };
            }
            if let Some((dash, gap)) = p.dash {
                let pattern = [dash as f64, gap as f64];
                unsafe { b.setLineDash_count_phase(pattern.as_ptr(), 2, 0.) };
            }
            ui_color(c).setStroke();
            b.stroke();
        }
    }
}

define_class!(
    /// Receives control events and taps; routes them by view tag (= node id).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismTarget"]
    pub struct Target;

    impl Target {
        #[unsafe(method(onAction:))]
        fn on_action(&self, sender: &UIView) {
            dispatch(UiEvent::Click(sender.tag() as NodeId));
        }

        #[unsafe(method(onEditing:))]
        fn on_editing(&self, sender: &UITextField) {
            let text = sender.text().map(|t| t.to_string()).unwrap_or_default();
            dispatch(UiEvent::Input(sender.tag() as NodeId, text));
            crate::interaction::refresh_suggestions(sender, self);
            #[cfg(not(target_os = "tvos"))]
            crate::carets::refresh(sender.tag() as NodeId);
        }

        /// Text size, reduce motion or language changed in Settings.
        #[unsafe(method(onEnvironment:))]
        fn on_environment(&self, _n: &objc2_foundation::NSNotification) {
            with_state(|s| {
                s.runtime.app.set_environment(ios_environment());
                let patches = s.runtime.render();
                s.commit(patches);
            });
        }

        /// The keyboard moves: shrink the safe area so content stays visible.
        #[unsafe(method(onKeyboard:))]
        fn on_keyboard(&self, n: &objc2_foundation::NSNotification) {
            keyboard_changed(n);
        }

        #[unsafe(method(onBeginEditing:))]
        fn on_begin_editing(&self, sender: &UITextField) {
            crate::forms::began_editing(sender);
            scroll_into_view(sender);
            crate::decor::focus(sender.tag() as NodeId, true);
            crate::interaction::refresh_suggestions(sender, self);
        }

        #[unsafe(method(onEndEditing:))]
        fn on_end_editing(&self, sender: &UITextField) {
            crate::decor::focus(sender.tag() as NodeId, false);
        }

        /// The return key (the field resigns, hiding the keyboard).
        #[unsafe(method(onSubmit:))]
        fn on_submit(&self, sender: &UITextField) {
            dispatch(UiEvent::Submit(sender.tag() as NodeId));
        }

        /// The placeholder pill of an empty optional date picker (#89).
        #[unsafe(method(onDateFill:))]
        fn on_date_fill(&self, sender: &UIView) {
            #[cfg(not(target_os = "tvos"))]
            crate::dates::fill(sender.tag() as NodeId);
            #[cfg(target_os = "tvos")]
            let _ = sender;
        }

        /// The clear button of an optional date picker (#89).
        #[unsafe(method(onDateClear:))]
        fn on_date_clear(&self, sender: &UIView) {
            #[cfg(not(target_os = "tvos"))]
            crate::dates::clear(sender.tag() as NodeId);
            #[cfg(target_os = "tvos")]
            let _ = sender;
        }

        #[unsafe(method(onDate:))]
        fn on_date(&self, sender: &AnyObject) {
            #[cfg(not(target_os = "tvos"))]
            if let Some(picker) = sender.downcast_ref::<objc2_ui_kit::UIDatePicker>() {
                let id = picker.tag() as NodeId;
                let mut secs = picker.date().timeIntervalSince1970().round();
                // A time of day: seconds since midnight, in UTC.
                if crate::interaction::is_time_of_day(id) {
                    secs = secs.rem_euclid(86_400.);
                }
                dispatch(UiEvent::Change(id, secs));
            }
            #[cfg(target_os = "tvos")]
            let _ = sender;
        }

        /// Active / inactive / background.
        /// Another device changed iCloud keys.
        #[unsafe(method(onCloudChange:))]
        fn on_cloud_change(&self, n: &objc2_foundation::NSNotification) {
            let keys = prism_native::apple::changed_cloud_keys(n);
            with_state(|s| {
                let patches = s.runtime.cloud_changed(&keys);
                s.commit(patches);
            });
        }

        #[unsafe(method(onLifecycle:))]
        fn on_lifecycle(&self, n: &objc2_foundation::NSNotification) {
            use objc2_ui_kit::*;
            let name = n.name();
            let phase = unsafe {
                if &*name == UIApplicationDidBecomeActiveNotification {
                    prism_core::Lifecycle::Active
                } else if &*name == UIApplicationDidEnterBackgroundNotification {
                    prism_core::Lifecycle::Background
                } else {
                    prism_core::Lifecycle::Inactive
                }
            };
            // Keys held as the app leaves get their release (#159).
            if phase != prism_core::Lifecycle::Active {
                crate::interaction::release_all_keys();
            }
            with_state(|s| {
                let patches = s.runtime.set_lifecycle(phase);
                s.commit(patches);
            });
            if phase == prism_core::Lifecycle::Active {
                run_queued_widget_actions();
            }
            update_background_time(self.mtm());
        }

        #[unsafe(method(onRefresh:))]
        fn on_refresh(&self, sender: &objc2_ui_kit::UIRefreshControl) {
            dispatch(UiEvent::Click(sender.tag() as NodeId));
        }

        #[unsafe(method(onSwipe:))]
        fn on_swipe(&self, sender: &objc2_ui_kit::UISwipeGestureRecognizer) {
            let dir = if sender.direction().contains(objc2_ui_kit::UISwipeGestureRecognizerDirection::Left) {
                prism_core::SwipeDirection::Left
            } else {
                prism_core::SwipeDirection::Right
            };
            if let Some(view) = sender.view() {
                dispatch(UiEvent::Swipe(view.tag() as NodeId, dir));
            }
        }

        #[unsafe(method(onTick:))]
        fn on_tick(&self, _timer: &AnyObject) {
            with_state(|s| s.renderer.widgets.tick());
        }

        #[unsafe(method(onToggle:))]
        fn on_toggle(&self, sender: &UISwitch) {
            let value = if sender.isOn() { 1. } else { 0. };
            dispatch(UiEvent::Change(sender.tag() as NodeId, value));
        }

        /// tvOS toggles (buttons): flip and report.
        #[unsafe(method(onToggleButton:))]
        fn on_toggle_button(&self, sender: &UIButton) {
            let on = !sender.isSelected();
            sender.setSelected(on);
            dispatch(UiEvent::Change(sender.tag() as NodeId, if on { 1. } else { 0. }));
        }

        /// A tap reports where it landed (VoiceOver activation does not:
        /// see `PrismContainerView`).
        #[unsafe(method(onTap:))]
        fn on_tap(&self, sender: &UITapGestureRecognizer) {
            if let Some(event) = crate::interaction::tap_event(sender) {
                dispatch(event);
            }
        }

        #[unsafe(method(onHover:))]
        fn on_hover(&self, sender: &objc2_ui_kit::UIGestureRecognizer) {
            #[cfg(not(target_os = "tvos"))]
            crate::interaction::on_hover(sender);
            #[cfg(target_os = "tvos")]
            let _ = sender;
        }

        #[unsafe(method(onTouchMove:))]
        fn on_touch_move(&self, sender: &objc2_ui_kit::UIGestureRecognizer) {
            #[cfg(not(target_os = "tvos"))]
            crate::interaction::on_touch_move(sender);
            #[cfg(target_os = "tvos")]
            let _ = sender;
        }

        #[unsafe(method(onPan:))]
        fn on_pan(&self, sender: &objc2_ui_kit::UIPanGestureRecognizer) {
            #[cfg(not(target_os = "tvos"))]
            crate::interaction::on_pan(sender);
            #[cfg(target_os = "tvos")]
            let _ = sender;
        }

        /// An iPad pointer's drag over a selection group (#231).
        #[unsafe(method(onAcrossPan:))]
        fn on_across_pan(&self, sender: &objc2_ui_kit::UIPanGestureRecognizer) {
            #[cfg(not(target_os = "tvos"))]
            crate::across::on_pan(sender);
            #[cfg(target_os = "tvos")]
            let _ = sender;
        }

        /// A drag after a long press (#243).
        #[unsafe(method(onHoldDrag:))]
        fn on_hold_drag(&self, sender: &objc2_ui_kit::UIGestureRecognizer) {
            #[cfg(not(target_os = "tvos"))]
            crate::interaction::on_hold(sender);
            #[cfg(target_os = "tvos")]
            let _ = sender;
        }

        /// A pinch over a view with `on_magnify` (#172).
        #[unsafe(method(onPinch:))]
        fn on_pinch(&self, sender: &objc2_ui_kit::UIGestureRecognizer) {
            #[cfg(not(target_os = "tvos"))]
            if let Some(g) = sender.downcast_ref::<objc2_ui_kit::UIPinchGestureRecognizer>() {
                crate::interaction::on_pinch(g);
            }
            #[cfg(target_os = "tvos")]
            let _ = sender;
        }

        /// Trackpad or mouse scrolling over a view with `on_wheel` (#172).
        #[unsafe(method(onWheel:))]
        fn on_wheel(&self, sender: &objc2_ui_kit::UIPanGestureRecognizer) {
            #[cfg(not(target_os = "tvos"))]
            crate::interaction::on_wheel(sender);
            #[cfg(target_os = "tvos")]
            let _ = sender;
        }

        /// A checkbox was pressed: flip it and report.
        #[unsafe(method(onCheckbox:))]
        fn on_checkbox(&self, sender: &UIButton) {
            let id = sender.tag() as NodeId;
            let mut value = None;
            with_state(|s| {
                if let Some(c) = s.renderer.widgets.checkboxes.get(&id) {
                    c.set_on(!c.is_on());
                    value = Some(if c.is_on() { 1. } else { 0. });
                }
            });
            if let Some(v) = value {
                dispatch(UiEvent::Change(id, v));
            }
        }

        /// A radio option (tag -1 - index) of its group (the superview).
        #[unsafe(method(onRadio:))]
        fn on_radio(&self, sender: &UIButton) {
            let index = (-sender.tag() - 1).max(0) as f64;
            if let Some(group) = sender.superview() {
                dispatch(UiEvent::Change(group.tag() as NodeId, index));
            }
        }

        #[unsafe(method(onSuggestion:))]
        fn on_suggestion(&self, sender: &UIButton) {
            #[cfg(target_os = "ios")]
            crate::interaction::suggestion_chosen(sender, self);
            #[cfg(not(target_os = "ios"))]
            let _ = sender;
        }
    }

    unsafe impl UITextFieldDelegate for Target {
        /// `max_length`: edits that would make the text longer are refused.
        #[unsafe(method(textField:shouldChangeCharactersInRange:replacementString:))]
        fn should_change(
            &self,
            field: &UITextField,
            range: objc2_foundation::NSRange,
            replacement: &NSString,
        ) -> bool {
            let old = field.text().map_or(0, |t| t.length());
            let new = old.saturating_sub(range.length) + replacement.length();
            // Typed over a selection across fields: the group's (#231).
            !type_across(field.tag() as NodeId, replacement)
                && crate::interaction::edit_fits(field.tag() as NodeId, old, new)
        }

        /// The caret or selection moved, typing included (#86).
        #[unsafe(method(textFieldDidChangeSelection:))]
        fn did_change_selection(&self, field: &UITextField) {
            crate::forms::selection_changed(field);
        }
    }

    unsafe impl UIGestureRecognizerDelegate for Target {
        #[unsafe(method(gestureRecognizer:shouldRecognizeSimultaneouslyWithGestureRecognizer:))]
        fn simultaneous(
            &self,
            g: &objc2_ui_kit::UIGestureRecognizer,
            other: &objc2_ui_kit::UIGestureRecognizer,
        ) -> bool {
            crate::interaction::simultaneous(g, other)
        }

        #[unsafe(method(gestureRecognizerShouldBegin:))]
        fn should_begin(&self, g: &objc2_ui_kit::UIGestureRecognizer) -> bool {
            crate::interaction::should_begin(g)
        }

        #[unsafe(method(gestureRecognizer:shouldReceiveTouch:))]
        fn should_receive_touch(
            &self,
            g: &objc2_ui_kit::UIGestureRecognizer,
            touch: &objc2_ui_kit::UITouch,
        ) -> bool {
            crate::interaction::receives_touch(g, touch)
        }

        #[unsafe(method(gestureRecognizer:shouldBeRequiredToFailByGestureRecognizer:))]
        fn required_to_fail(
            &self,
            g: &objc2_ui_kit::UIGestureRecognizer,
            other: &objc2_ui_kit::UIGestureRecognizer,
        ) -> bool {
            crate::interaction::required_to_fail_by(g, other)
        }
    }

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl UITabBarControllerDelegate for Target {
        #[unsafe(method(tabBarController:didSelectViewController:))]
        fn did_select(&self, tabs: &UITabBarController, _controller: &UIViewController) {
            if let Some(view) = tabs.viewIfLoaded() {
                dispatch(UiEvent::Change(view.tag() as NodeId, tabs.selectedIndex() as f64));
            }
        }
    }

    impl Target {
        /// Close button of a stack inside a sheet (tag = stack id).
        #[unsafe(method(onCloseStack:))]
        fn on_close_stack(&self, sender: &objc2_ui_kit::UIBarButtonItem) {
            dispatch(UiEvent::Dismiss(
                sender.tag() as NodeId,
                prism_core::DismissReason::Back,
            ));
        }
    }

    unsafe impl UITextViewDelegate for Target {
        /// Typed over a selection across fields (#231): the group's, from
        /// a hardware keyboard or the on-screen one.
        #[unsafe(method(textView:shouldChangeTextInRange:replacementText:))]
        fn text_view_should_change(
            &self,
            view: &objc2_ui_kit::UITextView,
            _range: objc2_foundation::NSRange,
            text: &NSString,
        ) -> bool {
            !type_across(view.tag() as NodeId, text)
        }

        #[unsafe(method(textViewDidChange:))]
        fn text_view_did_change(&self, view: &objc2_ui_kit::UITextView) {
            #[cfg(not(target_os = "tvos"))]
            {
                crate::forms::text_area_changed(view);
                // The text under the selection moved (#217).
                crate::edit::rect_changed(view);
            }
            #[cfg(target_os = "tvos")]
            let _ = view;
        }

        /// The caret or selection moved, typing included (#86).
        #[unsafe(method(textViewDidChangeSelection:))]
        fn text_view_did_change_selection(&self, view: &objc2_ui_kit::UITextView) {
            crate::forms::selection_changed(view);
            #[cfg(not(target_os = "tvos"))]
            crate::edit::rect_changed(view);
        }

        #[unsafe(method(textViewDidBeginEditing:))]
        fn text_view_did_begin_editing(&self, view: &objc2_ui_kit::UITextView) {
            scroll_into_view(view);
            crate::decor::focus(view.tag() as NodeId, true);
        }

        #[unsafe(method(textViewDidEndEditing:))]
        fn text_view_did_end_editing(&self, view: &objc2_ui_kit::UITextView) {
            crate::decor::focus(view.tag() as NodeId, false);
        }
    }

    unsafe impl UIScrollViewDelegate for Target {
        /// Virtualized lists follow the scroll position.
        #[unsafe(method(scrollViewDidScroll:))]
        fn scroll_view_did_scroll(&self, s: &UIScrollView) {
            crate::layout::scrolled(s);
            if !crate::layout::scroll_events(s.tag() as NodeId) {
                return;
            }
            let y = s.contentOffset().y + s.adjustedContentInset().top;
            let event = UiEvent::Scroll(s.tag() as NodeId, y as f32, s.bounds().size.height as f32);
            // Moved by the app (`scroll_by`, #204) while Prism applies
            // patches: reported right after, not lost.
            if STATE.with(|cell| cell.try_borrow_mut().is_err()) {
                dispatch2::DispatchQueue::main().exec_async(move || dispatch(event));
            } else {
                dispatch(event);
            }
        }
    }

    unsafe impl UIContextMenuInteractionDelegate for Target {
        #[unsafe(method_id(contextMenuInteraction:configurationForMenuAtLocation:))]
        fn context_menu_configuration(
            &self,
            interaction: &objc2_ui_kit::UIContextMenuInteraction,
            _location: objc2_core_foundation::CGPoint,
        ) -> Option<Retained<objc2_ui_kit::UIContextMenuConfiguration>> {
            menu_configuration(self.mtm(), interaction)
        }
    }

    unsafe impl UIAdaptivePresentationControllerDelegate for Target {
        /// A sheet was swiped down: tell its Modal node.
        #[unsafe(method(presentationControllerDidDismiss:))]
        fn presentation_did_dismiss(&self, pc: &objc2_ui_kit::UIPresentationController) {
            let host = pc.presentedViewController();
            if let Some(content) = host.view().and_then(|v| v.subviews().firstObject()) {
                let id = content.tag() as NodeId;
                with_state(|s| s.renderer.widgets.presented.retain(|p| *p != id));
                dispatch(UiEvent::Dismiss(id, prism_core::DismissReason::Swipe));
            }
        }
    }

    unsafe impl UINavigationControllerDelegate for Target {
        /// Fires after pushes, pops and swipe-back. Reports the native depth;
        /// the router ignores it when it already matches.
        #[unsafe(method(navigationController:didShowViewController:animated:))]
        fn did_show(
            &self,
            nav: &UINavigationController,
            _controller: &UIViewController,
            _animated: bool,
        ) {
            let depth = nav.viewControllers().count();
            if let Some(view) = nav.viewIfLoaded() {
                dispatch(UiEvent::Back(view.tag() as NodeId, depth));
            }
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    /// Re-runs layout whenever UIKit lays out the root (rotation, safe area, ...).
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismViewController"]
    pub struct PrismViewController;

    impl PrismViewController {
        #[unsafe(method(viewDidLayoutSubviews))]
        fn view_did_layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidLayoutSubviews] };
            relayout();
        }

        /// Light / dark: layer colors (borders, shadows, gradients) are
        /// plain `CGColor`s, resolved again.
        #[unsafe(method(traitCollectionDidChange:))]
        fn trait_collection_did_change(&self, previous: Option<&UITraitCollection>) {
            let _: () = unsafe { msg_send![super(self), traitCollectionDidChange: previous] };
            // The controller's traits are the new ones; its views get theirs
            // later, at their next layout (#36).
            let traits: Retained<UITraitCollection> = unsafe { msg_send![self, traitCollection] };
            crate::decor::appearance_changed(&traits);
        }

        #[unsafe(method(canBecomeFirstResponder))]
        fn can_become_first_responder(&self) -> bool {
            crate::interaction::has_global_keys()
        }

        /// Keys the focused node takes from its text field (`consume_keys`,
        /// #181): matched before the field acts on them.
        #[cfg(not(target_os = "tvos"))]
        #[unsafe(method_id(keyCommands))]
        fn key_commands(&self) -> Option<Retained<objc2_foundation::NSArray<objc2_ui_kit::UIKeyCommand>>> {
            let own = crate::interaction::consumed_key_commands(self);
            let base: Option<Retained<objc2_foundation::NSArray<objc2_ui_kit::UIKeyCommand>>> =
                unsafe { msg_send![super(self), keyCommands] };
            let mut all: Vec<Retained<objc2_ui_kit::UIKeyCommand>> =
                base.map(|b| b.to_vec()).unwrap_or_default();
            all.extend(own);
            Some(objc2_foundation::NSArray::from_retained_slice(&all))
        }

        #[cfg(not(target_os = "tvos"))]
        #[unsafe(method(onConsumedKey:))]
        fn on_consumed_key(&self, command: &objc2_ui_kit::UIKeyCommand) {
            crate::interaction::consumed_key(self, command);
        }

        /// Keys nothing below handled: global listeners (and local ones
        /// around a focused text field).
        #[unsafe(method(pressesBegan:withEvent:))]
        fn presses_began(
            &self,
            presses: &objc2_foundation::NSSet<objc2_ui_kit::UIPress>,
            event: Option<&objc2_ui_kit::UIPressesEvent>,
        ) {
            if !crate::interaction::handle_presses(presses, self, None, true) {
                let _: () = unsafe { msg_send![super(self), pressesBegan: presses, withEvent: event] };
            }
        }

        #[unsafe(method(pressesEnded:withEvent:))]
        fn presses_ended(
            &self,
            presses: &objc2_foundation::NSSet<objc2_ui_kit::UIPress>,
            event: Option<&objc2_ui_kit::UIPressesEvent>,
        ) {
            if !crate::interaction::handle_presses(presses, self, None, false) {
                let _: () = unsafe { msg_send![super(self), pressesEnded: presses, withEvent: event] };
            }
        }

        /// A press the system took (a shortcut, the app leaving): released.
        #[unsafe(method(pressesCancelled:withEvent:))]
        fn presses_cancelled(
            &self,
            presses: &objc2_foundation::NSSet<objc2_ui_kit::UIPress>,
            event: Option<&objc2_ui_kit::UIPressesEvent>,
        ) {
            if !crate::interaction::handle_presses(presses, self, None, false) {
                let _: () =
                    unsafe { msg_send![super(self), pressesCancelled: presses, withEvent: event] };
            }
        }

        /// tvOS: `cx.focus` moves the focus engine there.
        #[unsafe(method_id(preferredFocusEnvironments))]
        fn preferred_focus_environments(
            &self,
        ) -> Retained<NSArray<ProtocolObject<dyn objc2_ui_kit::UIFocusEnvironment>>> {
            match crate::interaction::take_focus_target() {
                Some(v) => NSArray::from_retained_slice(&[ProtocolObject::from_retained(v)]),
                None => unsafe { msg_send![super(self), preferredFocusEnvironments] },
            }
        }
    }
);

impl PrismViewController {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    /// Hosts one Prism screen inside a `UINavigationController`.
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismScreenController"]
    pub struct ScreenController;

    impl ScreenController {
        #[unsafe(method(viewDidLayoutSubviews))]
        fn view_did_layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidLayoutSubviews] };
            relayout();
        }

        /// The screen's safe area moved (a tab shown for the first time, a
        /// side bar, rotation; #28): lay out again once UIKit is done, as
        /// a layout already running would skip it.
        #[unsafe(method(viewSafeAreaInsetsDidChange))]
        fn view_safe_area_insets_did_change(&self) {
            let _: () = unsafe { msg_send![super(self), viewSafeAreaInsetsDidChange] };
            dispatch2::DispatchQueue::main().exec_async(|| with_state(State::relayout));
        }

        /// Shows or hides the navigation bar for this screen as it comes
        /// on top (pushes, pops, swipe back), animated with the transition.
        #[unsafe(method(viewWillAppear:))]
        fn view_will_appear(&self, animated: bool) {
            let _: () = unsafe { msg_send![super(self), viewWillAppear: animated] };
            apply_navigation_bar(self, animated);
        }
    }
);

// --- Hidden navigation bar (#8) ---

thread_local! {
    /// Stacks without a navigation bar (`Router::header(false)`) and bare
    /// screens (`screen(..).bare()`), by node id. Read from UIKit callbacks,
    /// which may run while the app state is borrowed.
    static NO_BAR: RefCell<std::collections::HashSet<NodeId>> = RefCell::default();
}

/// Records whether node `id` hides the navigation bar.
fn set_no_bar(id: NodeId, hidden: bool) {
    NO_BAR.with(|s| {
        let mut s = s.borrow_mut();
        if hidden {
            s.insert(id);
        } else {
            s.remove(&id);
        }
    });
}

/// Hides the bar of `vc`'s navigation controller if its stack has none or
/// the screen `vc` hosts is bare.
fn apply_navigation_bar(vc: &UIViewController, animated: bool) {
    let Some(nav) = vc.navigationController() else {
        return;
    };
    let stack = nav.viewIfLoaded().map(|v| v.tag() as NodeId);
    let screen = vc
        .viewIfLoaded()
        .and_then(|v| v.subviews().firstObject())
        .map(|v| v.tag() as NodeId);
    let hidden = NO_BAR.with(|s| {
        let s = s.borrow();
        stack.is_some_and(|id| s.contains(&id)) || screen.is_some_and(|id| s.contains(&id))
    });
    if nav.isNavigationBarHidden() != hidden {
        nav.setNavigationBarHidden_animated(hidden, animated);
    }
    #[cfg(not(target_os = "tvos"))]
    keep_swipe_back(&nav, hidden);
}

#[cfg(not(target_os = "tvos"))]
use objc2_ui_kit::UIGestureRecognizer;

#[cfg(not(target_os = "tvos"))]
define_class!(
    /// Lets the edge swipe go back while the navigation bar is hidden
    /// (UIKit's own delegate refuses it then): it starts whenever there is a
    /// screen to go back to and no transition is running.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismPopGestureDelegate"]
    struct PopGestureDelegate;

    unsafe impl NSObjectProtocol for PopGestureDelegate {}

    unsafe impl UIGestureRecognizerDelegate for PopGestureDelegate {
        #[unsafe(method(gestureRecognizerShouldBegin:))]
        fn should_begin(&self, gesture: &UIGestureRecognizer) -> bool {
            // The gesture sits on the navigation controller's view, whose
            // next responder is the controller.
            gesture
                .view()
                .and_then(|v| v.nextResponder())
                .and_then(|r| r.downcast::<UINavigationController>().ok())
                .is_some_and(|nav| {
                    nav.viewControllers().count() > 1 && nav.transitionCoordinator().is_none()
                })
        }
    }
);

#[cfg(not(target_os = "tvos"))]
thread_local! {
    /// The delegate above (gesture delegates are weak references).
    static POP_DELEGATE: Retained<PopGestureDelegate> = {
        let mtm = MainThreadMarker::new().expect("main thread");
        let this = PopGestureDelegate::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    };
    /// UIKit's delegate of each navigation controller's pop gesture, put
    /// back when its bar shows again.
    static POP_ORIGINAL: RefCell<HashMap<usize, Retained<ProtocolObject<dyn UIGestureRecognizerDelegate>>>> =
        RefCell::default();
}

/// Keeps the edge swipe back working while `nav`'s bar is hidden.
#[cfg(not(target_os = "tvos"))]
fn keep_swipe_back(nav: &UINavigationController, hidden: bool) {
    // The edge swipe, and on iOS 26 and later the swipe from anywhere in
    // the content.
    let mut gestures: Vec<Retained<UIGestureRecognizer>> =
        nav.interactivePopGestureRecognizer().into_iter().collect();
    if nav.respondsToSelector(sel!(interactiveContentPopGestureRecognizer)) {
        gestures.extend(nav.interactiveContentPopGestureRecognizer());
    }
    POP_DELEGATE.with(|ours| {
        let ours: &ProtocolObject<dyn UIGestureRecognizerDelegate> =
            ProtocolObject::from_ref(&**ours);
        POP_ORIGINAL.with(|saved| {
            let mut saved = saved.borrow_mut();
            for gesture in gestures {
                let key = &*gesture as *const UIGestureRecognizer as usize;
                let current = gesture.delegate();
                let is_ours = current.as_deref().is_some_and(|d| std::ptr::eq(d, ours));
                if hidden && !is_ours {
                    if let Some(original) = current {
                        saved.insert(key, original);
                    }
                    gesture.setDelegate(Some(ours));
                } else if !hidden && is_ours {
                    gesture.setDelegate(saved.remove(&key).as_deref());
                }
            }
        });
    });
}

impl ScreenController {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismAppDelegate"]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}
    unsafe impl UIApplicationDelegate for AppDelegate {
        /// The notification delegate must be set before launch finishes, or
        /// a tap that launched the app is lost.
        #[unsafe(method(application:didFinishLaunchingWithOptions:))]
        fn did_finish_launching(&self, _app: &UIApplication, _options: Option<&AnyObject>) -> bool {
            crate::services::init_bridge();
            // Background tasks must be registered before launch completes.
            crate::services::register_background_tasks();
            // Widget buttons: iOS may have launched the app just to run one.
            crate::services::install_widget_actions();
            observe_lifecycle(self.mtm());
            true
        }

        /// APNs answered `registerForRemoteNotifications` (prism_push_register).
        #[unsafe(method(application:didRegisterForRemoteNotificationsWithDeviceToken:))]
        fn did_register_push(&self, _app: &UIApplication, token: &objc2_foundation::NSData) {
            let hex: String = token.to_vec().iter().map(|b| format!("{b:02x}")).collect();
            push_registered(Ok(prism_native::apple::push::subscription(&hex)));
        }

        #[unsafe(method(application:didFailToRegisterForRemoteNotificationsWithError:))]
        fn did_fail_push(&self, _app: &UIApplication, error: &objc2_foundation::NSError) {
            let reason = error.localizedDescription().to_string();
            push_registered(Err(prism_core::DeviceError::Failed(reason)));
        }

        /// A push with `content-available` (the app may have been launched
        /// in the background for it): run the app's handler, then tell iOS.
        #[unsafe(method(application:didReceiveRemoteNotification:fetchCompletionHandler:))]
        fn did_receive_push(
            &self,
            _app: &UIApplication,
            info: &objc2_foundation::NSDictionary,
            done: &block2::DynBlock<dyn Fn(objc2_ui_kit::UIBackgroundFetchResult)>,
        ) {
            let json = unsafe {
                prism_native::apple::push::payload(info as *const _ as *const std::ffi::c_void)
            };
            let message = prism_native::apple::push::message(2, &json, None, None);
            push_background(message, done.copy());
        }
    }
);

#[derive(Default)]
pub struct SceneIvars {
    window: RefCell<Option<Retained<UIWindow>>>,
}

define_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismSceneDelegate"]
    #[ivars = SceneIvars]
    pub struct SceneDelegate;

    impl SceneDelegate {
        #[unsafe(method_id(init))]
        fn init(this: objc2::rc::Allocated<Self>) -> Option<Retained<Self>> {
            unsafe { msg_send![super(this.set_ivars(SceneIvars::default())), init] }
        }
    }

    unsafe impl NSObjectProtocol for SceneDelegate {}

    unsafe impl UISceneDelegate for SceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn will_connect(
            &self,
            scene: &UIScene,
            _session: &UISceneSession,
            _options: &UISceneConnectionOptions,
        ) {
            let Ok(scene) = scene.retain().downcast::<UIWindowScene>() else { return };
            let window = connect(self.mtm(), &scene);
            *self.ivars().window.borrow_mut() = Some(window);
            // Cold launch from a widget or Live Activity.
            // `URLContexts` is nil on a normal launch (objc2 declares it non-null).
            let contexts: Option<Retained<objc2_foundation::NSSet<objc2_ui_kit::UIOpenURLContext>>> =
                unsafe { msg_send![_options, URLContexts] };
            for ctx in contexts.iter().flat_map(|c| c.iter()) {
                if let Some(path) = link_from_url(&ctx.URL()) {
                    open_link(&path);
                }
            }
        }

        #[unsafe(method(scene:openURLContexts:))]
        fn open_urls(&self, _scene: &UIScene, contexts: &objc2_foundation::NSSet<objc2_ui_kit::UIOpenURLContext>) {
            for ctx in contexts.iter() {
                if let Some(path) = link_from_url(&ctx.URL()) {
                    open_link(&path);
                }
            }
        }
    }

    unsafe impl UIWindowSceneDelegate for SceneDelegate {}
);

fn connect(mtm: MainThreadMarker, scene: &UIWindowScene) -> Retained<UIWindow> {
    let (_config, init) = APP.get().expect("prism-ios: run() was not called");
    let window = UIWindow::initWithWindowScene(UIWindow::alloc(mtm), scene);
    let controller = PrismViewController::new(mtm);
    let root = controller.view().expect("view controller has a view");
    root.setBackgroundColor(Some(&UIColor::systemBackgroundColor()));
    crate::decor::set_root(&root);

    let path = std::env::var("PRISM_PATH").unwrap_or_else(|_| "/".into());
    // Dynamic Island iPhones have a taller status bar (≥ 54 pt).
    let island = !cfg!(target_os = "visionos")
        && scene
            .statusBarManager()
            .is_some_and(|m| m.statusBarFrame().size.height >= 54.);
    let services =
        crate::services::IosServices::new(island, Retained::into_super(controller.clone()));
    // Launched in the background for a task earlier? Keep that app state.
    let mut runtime = match HEADLESS.with(|h| h.borrow_mut().take()) {
        Some(h) => {
            let mut runtime = h.runtime;
            runtime
                .app
                .set_capabilities(prism_core::PlatformServices::capabilities(&services));
            if path != "/" {
                runtime.app.navigate(path);
            }
            runtime
        }
        None => make_runtime(init, path, &services),
    };
    #[allow(unused_mut)]
    let mut renderer = NativeRenderer::new(UiKit {
        mtm,
        target: Target::new(mtm),
        root,
        controller: controller.clone(),
        screens: HashMap::new(),
        titles: HashMap::new(),
        icons: HashMap::new(),
        selected: HashMap::new(),
        timers: HashMap::new(),
        timer_after: HashMap::new(),
        menus: HashMap::new(),
        presented: Vec::new(),
        pictures: HashMap::new(),
        image_cache: HashMap::new(),
        image_sources: image_sources(_config.assets.as_deref(), &_config.embedded_assets),
        semantic: None,
        radii: HashMap::new(),
        pickers: HashMap::new(),
        date_pickers: HashMap::new(),
        announced: HashMap::new(),
        lang: String::new(),
        overlays: HashMap::new(),
        checkboxes: HashMap::new(),
        no_hit: Default::default(),
        inert: Default::default(),
        a11y_hidden: Default::default(),
        layers: Vec::new(),
        layer_window: None,
        root_tabs: None,
        drag_preview: None,
    });
    #[cfg(target_os = "tvos")]
    renderer.set_zoom(crate::tv::zoom());
    let first = runtime.render();
    let mut state = State {
        runtime,
        renderer,
        services,
        device_size: None,
        window_rerendered: false,
    };
    state.commit(first);
    STATE.with(|cell| *cell.borrow_mut() = Some(state));
    prism_native::apple::set_device_sink(on_device);
    STATE.with(|cell| {
        if let Some(s) = cell.borrow().as_ref() {
            crate::device_info::observe_hinge(&s.renderer.widgets.root, hinge_changed);
        }
    });
    crate::automation::schedule();
    flush_responses();
    // Development: `PRISM_RUN_BACKGROUND=sync` runs a background task now.
    if let Ok(id) = std::env::var("PRISM_RUN_BACKGROUND") {
        run_background(&id, Completion::None);
    }
    observe_system(mtm);

    window.setRootViewController(Some(&controller));
    window.makeKeyAndVisible();
    // Bars attached by UIKit after creation follow the app's direction too.
    with_state(|s| {
        if let Some(attr) = s.renderer.widgets.semantic {
            set_semantic(&window, attr);
        }
    });
    window
}

// ---------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------

pub struct NavStack {
    nav: Retained<UINavigationController>,
    view: Retained<UIView>,
    /// Shows a Close button on the first screen (stacks inside sheets).
    closable: std::cell::Cell<bool>,
}

pub struct TabBar {
    controller: Retained<UITabBarController>,
    view: Retained<UIView>,
}

/// Makes `child` a child view controller of the nearest controller above its
/// view (stacks nested in tabs or screens), so it gets safe-area, rotation and
/// appearance callbacks.
fn ensure_contained(child: &UIViewController, view: &UIView) {
    if child.parentViewController().is_some() {
        return;
    }
    let mut responder = view.superview().map(Retained::into_super);
    while let Some(r) = responder {
        if let Ok(parent) = r.clone().downcast::<UIViewController>() {
            parent.addChildViewController(child);
            child.didMoveToParentViewController(Some(&parent));
            return;
        }
        responder = r.nextResponder();
    }
}

#[derive(Clone)]
pub enum View {
    Container(Retained<UIView>),
    Screen(Retained<UIView>),
    Stack(Rc<NavStack>),
    /// The content of a sheet (hosted by a presented view controller).
    Sheet(Retained<UIView>),
    Tabs(Rc<TabBar>),
    Tab(Retained<UIView>),
    Scroll(Retained<UIScrollView>),
    Toggle(Retained<ToggleView>),
    Image(Retained<UIImageView>),
    Progress(Retained<UIProgressView>),
    Timer(Retained<crate::label::PrismLabel>),
    Picture(Retained<UIImageView>),
    Label(Retained<crate::label::PrismLabel>),
    Button(Retained<UIButton>),
    Input(Retained<UITextField>),
    Slider(Retained<SliderView>),
    /// A pop-up button listing the options.
    Picker(Retained<UIButton>),
    DatePicker(Retained<DateView>),
    /// tvOS text areas are text fields (`UITextView.editable` is
    /// API_UNAVAILABLE(tvos)).
    #[cfg(not(target_os = "tvos"))]
    TextArea(Rc<crate::forms::TextArea>),
    Spinner(Retained<objc2_ui_kit::UIActivityIndicatorView>),
    /// A sheet, popover or dialog (its view is a hidden anchor).
    Overlay(Rc<crate::overlay::Overlay>),
    /// A drawn checkbox.
    Checkbox(Rc<crate::choice::Checkbox>),
    /// Drawn radio buttons.
    Radio(Rc<crate::choice::RadioGroup>),
    /// A web view (#62): a box holding the bridge's WKWebView host.
    Web(Rc<WebBox>),
    /// A video (#207): a box holding the bridge's AVKit player.
    Video(Rc<VideoBox>),
}

/// A video: its box and the bridge's player host.
pub struct VideoBox {
    view: Retained<UIView>,
    host: Option<prism_native::apple::video::VideoHost>,
}

/// A web view: its box, and the bridge's host view filling it (`None` on
/// tvOS, which has no WebKit: an empty box).
pub struct WebBox {
    view: Retained<UIView>,
    host: Option<prism_native::apple::web::WebHost>,
}

impl View {
    pub(crate) fn ui(&self) -> &UIView {
        match self {
            View::Container(v) | View::Screen(v) | View::Tab(v) => v,
            View::Stack(s) => &s.view,
            View::Sheet(v) => v,
            View::Tabs(t) => &t.view,
            View::Scroll(v) => v,
            View::Toggle(v) => v,
            View::Image(v) => v,
            View::Progress(v) => v,
            View::Timer(v) => v,
            View::Picture(v) => v,
            View::Label(v) => v,
            View::Button(v) => v,
            View::Input(v) => v,
            View::Slider(v) => v,
            View::Picker(v) => v,
            View::DatePicker(v) => v,
            #[cfg(not(target_os = "tvos"))]
            View::TextArea(t) => &t.view,
            View::Spinner(v) => v,
            View::Overlay(o) => &o.anchor,
            View::Checkbox(c) => &c.button,
            View::Radio(r) => &r.view,
            View::Web(w) => &w.view,
            View::Video(v) => &v.view,
        }
    }

    pub(crate) fn id(&self) -> NodeId {
        self.ui().tag() as NodeId
    }
}

pub struct UiKit {
    mtm: MainThreadMarker,
    target: Retained<Target>,
    root: Retained<UIView>,
    controller: Retained<PrismViewController>,
    /// One view controller per child of a navigation stack.
    screens: HashMap<NodeId, Retained<ScreenController>>,
    titles: HashMap<NodeId, String>,
    icons: HashMap<NodeId, String>,
    /// Selected index of each tab bar, applied once its tabs exist.
    selected: HashMap<NodeId, usize>,
    /// Countdown labels and their end time (Unix seconds).
    timers: HashMap<NodeId, (Retained<UILabel>, f64)>,
    /// What each countdown shows once ended (`timer(..).after`, #281).
    timer_after: HashMap<NodeId, String>,
    /// Context menus by view id (read by the interaction delegate).
    menus: HashMap<NodeId, prism_core::menu::MenuSpec>,
    /// Pictures waiting for their image, by source.
    pictures: HashMap<String, Vec<(NodeId, Retained<UIImageView>)>>,
    /// Decoded images by source.
    image_cache: HashMap<String, Retained<UIImage>>,
    image_sources: prism_native::images::ImageSources,
    /// Requested corner radius by node (clamped to the frame).
    radii: HashMap<NodeId, f64>,
    /// Layout direction of native chrome (set once the locale is known).
    semantic: Option<objc2_ui_kit::UISemanticContentAttribute>,
    /// Sheets currently presented, by modal id.
    presented: Vec<NodeId>,
    /// Options, placeholder and selection of each picker.
    pickers: HashMap<NodeId, crate::forms::PickerState>,
    /// Date pickers (they follow the app's language).
    date_pickers: HashMap<NodeId, Retained<DateView>>,
    /// The last announcement of each status region.
    announced: HashMap<NodeId, String>,
    /// The app's language (BCP 47).
    lang: String,
    // Overlays, choices, hit testing, layers (#18, #19, #25).
    overlays: HashMap<NodeId, Rc<crate::overlay::Overlay>>,
    checkboxes: HashMap<NodeId, Rc<crate::choice::Checkbox>>,
    /// Nodes made unreachable by `hit_test(false)` or `inert`.
    no_hit: std::collections::HashSet<NodeId>,
    inert: std::collections::HashSet<NodeId>,
    /// `Accessibility::hidden` of each node (combined with `inert`).
    a11y_hidden: std::collections::HashSet<NodeId>,
    /// The runtime's layers, and the window showing them.
    layers: Vec<Retained<UIView>>,
    layer_window: Option<(Retained<UIWindow>, Retained<UIView>)>,
    /// The root tab bar, which layers (toasts) keep clear of.
    root_tabs: Option<Retained<UITabBarController>>,
    /// The snapshot following a dragged payload (#251).
    drag_preview: Option<Retained<UIView>>,
}

// --- Content area (#28) ---
impl UiKit {
    /// What the system covers of the root (points): its safe area without
    /// the keyboard, and with a root tab bar, what the selected tab's
    /// screens lose to it too (a bottom bar, an iPad sidebar, a foldable's
    /// side bar): the area screens lay out in.
    fn content_insets(&self) -> Insets {
        let root = &self.root;
        let i = root.safeAreaInsets();
        // The keyboard (a root without stacks takes it as its own inset).
        let kb = self.controller.additionalSafeAreaInsets();
        let mut out = Insets {
            top: (i.top - kb.top).max(0.) as f32,
            right: (i.right - kb.right).max(0.) as f32,
            bottom: (i.bottom - kb.bottom).max(0.) as f32,
            left: (i.left - kb.left).max(0.) as f32,
        };
        let Some(tabs) = &self.root_tabs else {
            return out;
        };
        let Some(tab) = (unsafe { tabs.selectedViewController() })
            .filter(|vc| vc.isViewLoaded())
            .and_then(|vc| vc.view())
            .filter(|v| v.window().is_some())
        else {
            return out;
        };
        let f = tab.convertRect_toView(tab.bounds(), Some(root));
        let si = tab.safeAreaInsets();
        let b = root.bounds().size;
        if f.size.width <= 0. || f.size.height <= 0. {
            return out;
        }
        let (left, top) = (f.origin.x + si.left, f.origin.y + si.top);
        let right = b.width - (f.origin.x + f.size.width - si.right);
        let bottom = b.height - (f.origin.y + f.size.height - si.bottom);
        out.top = out.top.max(top.max(0.) as f32);
        out.left = out.left.max(left.max(0.) as f32);
        out.right = out.right.max(right.max(0.) as f32);
        out.bottom = out.bottom.max(bottom.max(0.) as f32);
        out
    }
}

impl UiKit {
    /// Puts the layers in their pass-through window, once the app's window
    /// is on screen, framed like the app's root.
    fn attach_layers(&mut self) {
        if self.layers.is_empty() && self.layer_window.is_none() {
            return;
        }
        if self.layer_window.is_none() {
            let Some(scene) = self.root.window().and_then(|w| w.windowScene()) else {
                return;
            };
            self.layer_window = Some(crate::interaction::layer_window(self.mtm, &scene));
        }
        let Some((_, host)) = &self.layer_window else {
            return;
        };
        let frame = self.root.convertRect_toView(self.root.bounds(), None);
        crate::interaction::frame_layer_host(host, frame);
        for (i, v) in self.layers.iter().enumerate() {
            if v.superview().as_deref() != Some(&**host) {
                host.insertSubview_atIndex(v, i as isize);
            }
        }
        // A copy: removing from the array being enumerated aborts (#66).
        for sub in host.subviews().to_vec() {
            if !self.layers.iter().any(|l| **l == *sub) {
                sub.removeFromSuperview();
            }
        }
    }

    /// The user can point at / tap the view (`hit_test`, `inert`).
    fn set_hit(&mut self, view: &View, style: &Style) {
        let id = view.id();
        let v = view.ui();
        let blocked = !style.hit_test || style.inert;
        if blocked {
            v.setUserInteractionEnabled(false);
            self.no_hit.insert(id);
        } else if self.no_hit.remove(&id) {
            // Labels and images take no touches by default.
            let default_on = !matches!(
                view,
                View::Label(_) | View::Timer(_) | View::Image(_) | View::Picture(_)
            );
            v.setUserInteractionEnabled(default_on);
        }
        let inert = style.inert;
        if inert != self.inert.contains(&id) {
            if inert {
                self.inert.insert(id);
            } else {
                self.inert.remove(&id);
            }
            v.setAccessibilityElementsHidden(inert || self.a11y_hidden.contains(&id), self.mtm);
        }
    }
}

/// A picture's fit (`true`: letterboxed) and focal point (#218).
type PictureScale = (bool, Option<(f32, f32)>);

thread_local! {
    /// Each picture's scaling.
    static PICTURES: RefCell<HashMap<NodeId, PictureScale>> = RefCell::default();
}

/// Scales picture `v` as it asks: fitted, filled around the center, or
/// filled around a focal point (#218): the part of the image that covers
/// the frame (`contentsRect`), stretched over it.
fn refocus(v: &UIImageView) {
    use objc2_ui_kit::UIViewContentMode as M;
    let (fit, at) = PICTURES.with(|p| {
        p.borrow()
            .get(&(v.tag() as NodeId))
            .copied()
            .unwrap_or_default()
    });
    let b = v.bounds().size;
    let crop = match (fit, at, v.image()) {
        (false, Some(at), Some(img)) => {
            // SAFETY: a plain property read.
            let s = unsafe { img.size() };
            Some(prism_core::cover_crop(
                (s.width as f32, s.height as f32),
                (b.width as f32, b.height as f32),
                at,
            ))
        }
        _ => None,
    };
    v.setContentMode(match (fit, crop) {
        (true, _) => M::ScaleAspectFit,
        (false, Some(_)) => M::ScaleToFill,
        (false, None) => M::ScaleAspectFill,
    });
    let (x, y, w, h) = crop.unwrap_or((0., 0., 1., 1.));
    v.layer()
        .setContentsRect(objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(x as f64, y as f64),
            objc2_core_foundation::CGSize::new(w as f64, h as f64),
        ));
}

impl UiKit {
    /// Shows `src` in `view`: from memory, or loaded on a background thread.
    fn load_picture(&mut self, id: NodeId, view: &Retained<UIImageView>, src: &str) {
        if let Some(img) = self.image_cache.get(src) {
            view.setImage(Some(img));
            refocus(view);
            return;
        }
        view.setImage(None);
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
        let Some(img) =
            bytes.and_then(|b| UIImage::imageWithData(&objc2_foundation::NSData::from_vec(b)))
        else {
            eprintln!(
                "prism-ios: could not load image {:?}",
                prism_core::picture_source(src).0
            );
            return;
        };
        for (_, view) in waiting {
            view.setImage(Some(&img));
            refocus(&view);
        }
        self.image_cache.insert(src.to_string(), img);
    }

    fn present_sheets(&mut self, sheets: &[View]) {
        for sheet in sheets {
            let id = sheet.id();
            if self.presented.contains(&id) {
                continue;
            }
            let vc = self.screen_controller(sheet);
            if let Some(pc) = vc.presentationController() {
                unsafe { pc.setDelegate(Some(ProtocolObject::from_ref(&*self.target))) };
            }
            crate::menus::top_controller(&self.controller)
                .presentViewController_animated_completion(&vc, true, None);
            self.presented.push(id);
        }
        let keep: Vec<NodeId> = sheets.iter().map(View::id).collect();
        let gone: Vec<NodeId> = self
            .presented
            .iter()
            .copied()
            .filter(|id| !keep.contains(id))
            .collect();
        for id in gone {
            self.presented.retain(|p| *p != id);
            if let Some(vc) = self.screens.get(&id)
                && vc.presentingViewController().is_some()
            {
                vc.dismissViewControllerAnimated_completion(true, None);
            }
        }
    }

    /// The Close button of a stack inside a sheet.
    fn apply_close_button(&self, stack: &NavStack, stack_id: NodeId) {
        let Some(first) = stack.nav.viewControllers().firstObject() else {
            return;
        };
        if stack.closable.get() {
            let target: &AnyObject = &self.target;
            let item = unsafe {
                objc2_ui_kit::UIBarButtonItem::initWithBarButtonSystemItem_target_action(
                    objc2_ui_kit::UIBarButtonItem::alloc(self.mtm),
                    // No "Close" item on tvOS (UIBarButtonItem.h).
                    if cfg!(target_os = "tvos") {
                        objc2_ui_kit::UIBarButtonSystemItem::Done
                    } else {
                        objc2_ui_kit::UIBarButtonSystemItem::Close
                    },
                    Some(target),
                    Some(sel!(onCloseStack:)),
                )
            };
            item.setTag(stack_id as isize);
            first.navigationItem().setLeftBarButtonItem(Some(&item));
        } else {
            first.navigationItem().setLeftBarButtonItem(None);
        }
        // The screen's own leading buttons follow Close (#30).
        crate::navbar::refresh(&first);
    }
}

impl UiKit {
    fn tick(&self) {
        for (id, (label, end)) in &self.timers {
            let after = self.timer_after.get(id).map_or("", String::as_str);
            let text = NSString::from_str(&prism_core::timer_text(*end, after));
            if label.text().as_deref() != Some(&*text) {
                label.setText(Some(&text));
            }
        }
    }
}

impl UiKit {
    /// The view controller hosting `view` in a stack (created on first use).
    fn screen_controller(&mut self, view: &View) -> Retained<ScreenController> {
        let id = view.id();
        let mtm = self.mtm;
        let vc = self
            .screens
            .entry(id)
            .or_insert_with(|| ScreenController::new(mtm))
            .clone();
        let host = vc.view().expect("view controller has a view");
        host.setBackgroundColor(Some(&UIColor::systemBackgroundColor()));
        if view.ui().superview().as_deref() != Some(&*host) {
            host.addSubview(view.ui());
        }
        let title = self.titles.get(&id).map(|t| NSString::from_str(t));
        vc.setTitle(title.as_deref());
        // Bar buttons and title size (#30).
        crate::navbar::apply(&vc, id);
        if let (Some(item), Some(icon)) = (vc.tabBarItem(), self.icons.get(&id)) {
            item.setImage(UIImage::systemImageNamed(&NSString::from_str(icon)).as_deref());
        }
        vc
    }
}

#[cfg(not(target_os = "tvos"))]
fn ns_locale(lang: &str) -> Retained<objc2_foundation::NSLocale> {
    objc2_foundation::NSLocale::localeWithLocaleIdentifier(&NSString::from_str(lang))
}

fn plain_color(c: Color) -> Retained<UIColor> {
    UIColor::colorWithRed_green_blue_alpha(c.r as f64, c.g as f64, c.b as f64, c.a as f64)
}

/// Adaptive colors become dynamic `UIColor`s, resolved live by UIKit.
pub(crate) fn ui_color(c: Color) -> Retained<UIColor> {
    if c.dark.is_none() {
        return plain_color(c);
    }
    let (light, dark) = (c.light_variant(), c.dark_variant());
    let provider = block2::RcBlock::new(move |traits: std::ptr::NonNull<UITraitCollection>| {
        let traits = unsafe { traits.as_ref() };
        let style = unsafe { traits.userInterfaceStyle() };
        let pick = if style == UIUserInterfaceStyle::Dark {
            dark
        } else {
            light
        };
        let color = plain_color(pick);
        std::ptr::NonNull::new(Retained::autorelease_return(color)).expect("color")
    });
    unsafe { UIColor::colorWithDynamicProvider(&provider) }
}

fn font_weight(style: &Style) -> objc2_ui_kit::UIFontWeight {
    crate::label::ui_weight(style.font_weight)
}

fn symbol_weight(style: &Style) -> objc2_ui_kit::UIImageSymbolWeight {
    symbol_weight_for(style.font_weight)
}

/// The SF Symbol weight nearest to a CSS weight (symbols have nine).
pub(crate) fn symbol_weight_for(w: FontWeight) -> objc2_ui_kit::UIImageSymbolWeight {
    use objc2_ui_kit::UIImageSymbolWeight as W;
    match w.nearest_hundred() {
        100 => W::UltraLight,
        200 => W::Thin,
        300 => W::Light,
        400 => W::Regular,
        500 => W::Medium,
        600 => W::Semibold,
        700 => W::Bold,
        800 => W::Heavy,
        _ => W::Black,
    }
}

/// The font of a button or input: size, any weight, family, italic and
/// tabular digits of its style.
fn font(style: &Style) -> Retained<UIFont> {
    crate::label::ui_font(&prism_native::text::FontSpec {
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

impl Widgets for UiKit {
    type View = View;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> View {
        let mtm = self.mtm;
        let target: &AnyObject = &self.target;
        let view = match kind {
            // Reports presses for `.active(|s| …)`.
            #[cfg(not(target_os = "tvos"))]
            NodeKind::View => View::Container(crate::interaction::container(mtm)),
            // tvOS: containers can take the remote's focus when clickable.
            #[cfg(target_os = "tvos")]
            NodeKind::View => View::Container(crate::tv::focus_view(mtm)),
            // Tables (#284): containers, made a data table by `set_table`.
            #[cfg(not(target_os = "tvos"))]
            NodeKind::Table | NodeKind::TableRow | NodeKind::ColumnHeader | NodeKind::TableCell => {
                View::Container(crate::interaction::container(mtm))
            }
            #[cfg(target_os = "tvos")]
            NodeKind::Table | NodeKind::TableRow | NodeKind::ColumnHeader | NodeKind::TableCell => {
                View::Container(crate::tv::focus_view(mtm))
            }
            NodeKind::Screen => View::Screen(UIView::new(mtm)),
            NodeKind::Modal => View::Sheet(UIView::new(mtm)),
            NodeKind::Stack => {
                let nav = UINavigationController::new(mtm);
                #[cfg(not(target_os = "tvos"))]
                nav.navigationBar().setPrefersLargeTitles(true);
                unsafe { nav.setDelegate(Some(ProtocolObject::from_ref(&*self.target))) };
                let view = nav.view().expect("navigation controller has a view");
                View::Stack(Rc::new(NavStack {
                    nav,
                    view,
                    closable: std::cell::Cell::new(false),
                }))
            }
            NodeKind::Tabs => {
                let controller = UITabBarController::new(mtm);
                controller.setDelegate(Some(ProtocolObject::from_ref(&*self.target)));
                let view = controller.view().expect("tab bar controller has a view");
                View::Tabs(Rc::new(TabBar { controller, view }))
            }
            NodeKind::Tab => View::Tab(UIView::new(mtm)),
            NodeKind::Scroll => {
                let s = UIScrollView::new(mtm);
                s.setAlwaysBounceVertical(true);
                s.setKeyboardDismissMode(UIScrollViewKeyboardDismissMode::OnDrag);
                // UIKit insets content under bars (screens let a leading scroll
                // view run under them, see prism-native).
                s.setContentInsetAdjustmentBehavior(
                    UIScrollViewContentInsetAdjustmentBehavior::Automatic,
                );
                View::Scroll(s)
            }
            NodeKind::Image => {
                let v = UIImageView::new(mtm);
                v.setContentMode(objc2_ui_kit::UIViewContentMode::Center);
                View::Image(v)
            }
            NodeKind::Progress => View::Progress(UIProgressView::new(mtm)),
            NodeKind::Scene3d => View::Container(crate::scene::create(id)),
            NodeKind::Shape => {
                let v: Retained<ShapeView> = unsafe { msg_send![ShapeView::alloc(mtm), init] };
                v.setOpaque(false);
                v.setBackgroundColor(Some(&UIColor::clearColor()));
                // Redrawn at the new size, not stretched.
                v.setContentMode(objc2_ui_kit::UIViewContentMode::Redraw);
                View::Container(Retained::into_super(v))
            }
            NodeKind::WebView => {
                let view = UIView::new(mtm);
                // Rounded corners clip the page.
                view.setClipsToBounds(true);
                let host = prism_native::apple::web::WebHost::new(id);
                if let Some(h) = &host {
                    // SAFETY: the bridge's host is a UIView, kept alive by `host`.
                    let v = unsafe { &*(h.view_ptr() as *const UIView) };
                    view.addSubview(v);
                }
                View::Web(Rc::new(WebBox { view, host }))
            }
            NodeKind::Video => {
                let view = UIView::new(mtm);
                view.setClipsToBounds(true);
                let host = prism_native::apple::video::VideoHost::new(id);
                if let Some(h) = &host {
                    // SAFETY: the bridge's host is a UIView, kept alive by `host`.
                    let v = unsafe { &*(h.view_ptr() as *const UIView) };
                    v.setFrame(view.bounds());
                    view.addSubview(v);
                }
                View::Video(Rc::new(VideoBox { view, host }))
            }
            NodeKind::Picture => {
                let v = UIImageView::new(mtm);
                v.setClipsToBounds(true);
                v.setContentMode(objc2_ui_kit::UIViewContentMode::ScaleAspectFill);
                View::Picture(v)
            }
            NodeKind::Timer => View::Timer(crate::label::new_timer(mtm, id)),
            #[cfg(not(target_os = "tvos"))]
            NodeKind::Toggle => {
                let t = UISwitch::new(mtm);
                unsafe {
                    t.addTarget_action_forControlEvents(
                        Some(target),
                        sel!(onToggle:),
                        UIControlEvents::ValueChanged,
                    )
                };
                View::Toggle(t)
            }
            #[cfg(target_os = "tvos")]
            NodeKind::Toggle => View::Toggle(crate::tv::toggle(mtm, target)),
            NodeKind::Text => View::Label(crate::label::new_label(mtm, id)),
            NodeKind::Button => {
                let b = UIButton::buttonWithType(UIButtonType::System, mtm);
                // tvOS: the remote's select press (there is no touch).
                let event = if cfg!(target_os = "tvos") {
                    UIControlEvents::PrimaryActionTriggered
                } else {
                    UIControlEvents::TouchUpInside
                };
                unsafe {
                    b.addTarget_action_forControlEvents(Some(target), sel!(onAction:), event)
                };
                View::Button(b)
            }
            NodeKind::TextInput => {
                // Its text sits inside its padding (#14).
                let f = crate::padding::PrismField::field(mtm);
                f.setBorderStyle(UITextBorderStyle::RoundedRect);
                unsafe {
                    f.addTarget_action_forControlEvents(
                        Some(target),
                        sel!(onEditing:),
                        UIControlEvents::EditingChanged,
                    );
                    f.addTarget_action_forControlEvents(
                        Some(target),
                        sel!(onBeginEditing:),
                        UIControlEvents::EditingDidBegin,
                    );
                    f.addTarget_action_forControlEvents(
                        Some(target),
                        sel!(onEndEditing:),
                        UIControlEvents::EditingDidEnd,
                    );
                    f.addTarget_action_forControlEvents(
                        Some(target),
                        sel!(onSubmit:),
                        UIControlEvents::EditingDidEndOnExit,
                    );
                };
                // Max length.
                f.setDelegate(Some(ProtocolObject::from_ref(&*self.target)));
                View::Input(f)
            }
            #[cfg(not(target_os = "tvos"))]
            NodeKind::Slider => View::Slider(crate::forms::PrismSlider::new(mtm)),
            #[cfg(target_os = "tvos")]
            NodeKind::Slider => View::Slider(crate::tv::Adjustable::slider(mtm)),
            NodeKind::Picker => {
                self.pickers.insert(id, Default::default());
                View::Picker(crate::forms::picker(mtm))
            }
            #[cfg(not(target_os = "tvos"))]
            NodeKind::DatePicker => {
                let p = objc2_ui_kit::UIDatePicker::new(mtm);
                p.setPreferredDatePickerStyle(objc2_ui_kit::UIDatePickerStyle::Compact);
                // Compact pickers draw at the trailing edge of their frame.
                p.setContentHorizontalAlignment(
                    objc2_ui_kit::UIControlContentHorizontalAlignment::Leading,
                );
                p.setDatePickerMode(objc2_ui_kit::UIDatePickerMode::Date);
                if !self.lang.is_empty() {
                    p.setLocale(Some(&ns_locale(&self.lang)));
                }
                unsafe {
                    p.addTarget_action_forControlEvents(
                        Some(target),
                        sel!(onDate:),
                        UIControlEvents::ValueChanged,
                    )
                };
                self.date_pickers.insert(id, p.clone());
                View::DatePicker(p)
            }
            #[cfg(target_os = "tvos")]
            NodeKind::DatePicker => {
                let p = crate::tv::Adjustable::date_picker(mtm);
                p.set_lang(&self.lang);
                self.date_pickers.insert(id, p.clone());
                View::DatePicker(p)
            }
            #[cfg(not(target_os = "tvos"))]
            NodeKind::TextArea => View::TextArea(Rc::new(crate::forms::TextArea::new(
                mtm,
                ProtocolObject::from_ref(&*self.target),
            ))),
            // A text field: the remote opens the full-screen keyboard.
            #[cfg(target_os = "tvos")]
            NodeKind::TextArea => return self.create(id, NodeKind::TextInput),
            // A text area showing runs as attributes, chips as
            // attachments (#115, `crate::rich`).
            #[cfg(not(target_os = "tvos"))]
            NodeKind::RichInput => {
                let t = crate::forms::TextArea::new(mtm, ProtocolObject::from_ref(&*self.target));
                t.view.setTag(id as isize);
                crate::rich::setup(&t.view);
                View::TextArea(Rc::new(t))
            }
            // Not rich on tvOS (`Widgets::rich_input`): never asked.
            #[cfg(target_os = "tvos")]
            NodeKind::RichInput => return self.create(id, NodeKind::TextArea),
            NodeKind::Spinner => {
                let s = objc2_ui_kit::UIActivityIndicatorView::initWithActivityIndicatorStyle(
                    objc2_ui_kit::UIActivityIndicatorView::alloc(mtm),
                    objc2_ui_kit::UIActivityIndicatorViewStyle::Medium,
                );
                s.setHidesWhenStopped(false);
                s.startAnimating();
                View::Spinner(s)
            }
            NodeKind::Overlay => {
                let o = Rc::new(crate::overlay::Overlay::new(mtm, id));
                self.overlays.insert(id, o.clone());
                View::Overlay(o)
            }
            NodeKind::Checkbox => {
                let c = Rc::new(crate::choice::Checkbox::new(mtm, target));
                self.checkboxes.insert(id, c.clone());
                View::Checkbox(c)
            }
            NodeKind::RadioGroup => {
                View::Radio(Rc::new(crate::choice::RadioGroup::new(mtm, target)))
            }
        };
        view.ui().setTag(id as isize);
        if let Some(attr) = self.semantic {
            set_semantic(view.ui(), attr);
        }
        view
    }

    fn set_hidden(&mut self, view: &View, hidden: bool) {
        view.ui().setHidden(hidden);
        crate::decor::set_hidden(view.ui(), hidden);
    }

    fn set_interactions(&mut self, view: &View, states: prism_core::Interactions) {
        crate::decor::set_interactions(view.ui(), states);
    }

    fn set_style(&mut self, view: &View, style: &Style) {
        if let View::Overlay(_) = view {
            return;
        }
        // Bar tint (#30).
        match view {
            View::Tabs(t) => {
                crate::navbar::tint_tabs(&t.controller, style.tint.map(ui_color).as_deref())
            }
            View::Stack(st) => {
                crate::navbar::tint_stack(&st.nav, style.tint.map(ui_color).as_deref())
            }
            _ => {}
        }
        self.set_hit(view, style);
        let v = view.ui();
        v.setAlpha(style.opacity as f64);
        v.setTransform(prism_native::apple::motion::affine(style.transform));
        if !matches!(view, View::Input(_)) {
            // `nil` renders black for some UIKit views; be explicit.
            let bg = style.background.map_or_else(UIColor::clearColor, ui_color);
            v.setBackgroundColor(Some(&bg));
        }
        let layer = v.layer();
        // Clamped to half the size in `set_frame` (a larger radius makes
        // Core Animation skip the background entirely).
        self.radii.insert(view.id(), style.corner_radius as f64);
        let b = v.bounds().size;
        layer.setCornerRadius(
            (style.corner_radius as f64)
                .min(b.width.min(b.height) / 2.)
                .max(0.),
        );
        layer.setMaskedCorners(prism_native::apple::corner_mask(
            style.rounded_corners,
            true,
        ));
        layer.setBorderWidth(style.border_width as f64);
        crate::decor::set_layer_border(v, style.border_color);

        if let View::Scroll(s) = view {
            crate::layout::set_axis(s, style.scroll_axis, style.scrollbar);
        }
        crate::layout::remember_font(view, || font(style));
        let text_color = style.text_color.map(ui_color);
        if matches!(view, View::Button(_) | View::Input(_)) || is_text_area(view) {
            crate::padding::remember(view.id(), style);
        }
        match view {
            View::Container(_)
            | View::Screen(_)
            | View::Sheet(_)
            | View::Stack(_)
            | View::Tabs(_)
            | View::Tab(_)
            | View::Scroll(_)
            | View::Picture(_)
            | View::Toggle(_)
            | View::Overlay(_)
            | View::Web(_)
            | View::Video(_) => {}
            View::Checkbox(c) => c.set_style(font(style), text_color.clone()),
            View::Radio(r) => r.set_style(font(style), text_color.clone()),
            #[cfg(not(target_os = "tvos"))]
            View::Slider(s) => s.setMinimumTrackTintColor(text_color.as_deref()),
            #[cfg(target_os = "tvos")]
            View::Slider(s) => s.set_tint(text_color.as_deref()),
            View::Picker(b) => {
                let color = text_color.clone();
                crate::forms::update_button(b, |c| c.setBaseForegroundColor(color.as_deref()));
            }
            #[cfg(not(target_os = "tvos"))]
            View::DatePicker(p) => unsafe { p.setTintColor(text_color.as_deref()) },
            #[cfg(target_os = "tvos")]
            View::DatePicker(p) => {
                p.set_tint(text_color.as_deref());
                p.set_font(&font(style));
                p.set_alignment(alignment(style.text_align));
            }
            #[cfg(not(target_os = "tvos"))]
            View::TextArea(t) => {
                let f = font(style);
                t.view.setFont(Some(&f));
                t.view
                    .setTextColor(Some(&text_color.unwrap_or_else(UIColor::labelColor)));
                t.view.setTextAlignment(alignment(style.text_align));
                unsafe { t.placeholder.setFont(Some(&f)) };
                t.placeholder.setTextAlignment(alignment(style.text_align));
                crate::padding::style_text_area(t, style);
                t.line_height.set(style.line_height_points());
                t.apply_line_height();
                crate::rich::set_style(&t.view, style);
            }
            View::Spinner(s) => unsafe { s.setColor(text_color.as_deref()) },
            View::Image(v) => {
                let config = UIImageSymbolConfiguration::configurationWithPointSize_weight(
                    style.font_size as f64,
                    symbol_weight(style),
                );
                v.setPreferredSymbolConfiguration(Some(&config));
                unsafe {
                    v.setTintColor(Some(
                        &text_color.clone().unwrap_or_else(UIColor::labelColor),
                    ))
                };
            }
            View::Progress(p) => p.setProgressTintColor(text_color.as_deref()),
            View::Label(l) => crate::label::set_style(l, style),
            View::Timer(l) => {
                // Digits keep a fixed width while the countdown ticks.
                let f = UIFont::monospacedDigitSystemFontOfSize_weight(
                    style.font_size as f64,
                    font_weight(style),
                );
                unsafe { l.setFont(Some(&f)) };
                unsafe { l.setTextColor(Some(&text_color.unwrap_or_else(UIColor::labelColor))) };
                l.setTextAlignment(alignment(style.text_align));
                crate::label::set_insets(l, style.padding);
            }
            View::Input(f) => {
                f.setFont(Some(&font(style)));
                f.setTextColor(Some(&text_color.unwrap_or_else(UIColor::labelColor)));
                f.setTextAlignment(alignment(style.text_align));
                crate::padding::style_field(f, style);
            }
            View::Button(b) => {
                if let Some(label) = b.titleLabel() {
                    unsafe { label.setFont(Some(&font(style))) };
                }
                b.setTitleColor_forState(text_color.as_deref(), UIControlState::Normal);
                crate::padding::style_button(b, style);
            }
        }
        // Shadows, per-side and dashed borders, clip, gradient, blur.
        crate::decor::apply(v, style);
        if matches!(view, View::Label(_) | View::Timer(_)) {
            // A label paints its background itself, not its layer: rounded
            // only when clipped (a chip, #14). Shadows live in a companion
            // view, so they survive.
            v.setClipsToBounds(
                style.clip || (style.corner_radius > 0. && style.background.is_some()),
            );
        }
    }

    fn set_text(&mut self, view: &View, text: &str) {
        let s = NSString::from_str(text);
        // Other people's carets follow the new text (#240), once it is in.
        #[cfg(not(target_os = "tvos"))]
        if let View::Input(_) | View::TextArea(_) = view {
            let id = view.id();
            dispatch2::DispatchQueue::main().exec_async(move || crate::carets::refresh(id));
        }
        match view {
            View::Label(l) => crate::label::set_text(l, text),
            View::Timer(l) => l.setText(Some(&s)),
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
            // The app's value reaches a focused field too (#73).
            View::Input(f) => {
                if f.text().map(|t| t.to_string()).unwrap_or_default() != text {
                    crate::forms::set_field_text(&**f, || f.setText(Some(&s)));
                }
            }
            View::Button(b) => b.setTitle_forState(Some(&s), UIControlState::Normal),
            View::Checkbox(c) => c.set_title(text),
            #[cfg(not(target_os = "tvos"))]
            View::TextArea(t) => t.set_text(text),
            View::Screen(_) | View::Tab(_) => {
                let id = view.id();
                self.titles.insert(id, text.to_string());
                if let Some(vc) = self.screens.get(&id) {
                    vc.setTitle(Some(&s));
                    // A tab's item keeps its first title otherwise (language changes).
                    if matches!(view, View::Tab(_))
                        && let Some(item) = vc.tabBarItem()
                    {
                        item.setTitle(Some(&s));
                    }
                }
            }
            _ => {}
        }
    }

    fn set_placeholder(&mut self, view: &View, text: &str) {
        match view {
            View::Input(f) => f.setPlaceholder(Some(&NSString::from_str(text))),
            #[cfg(not(target_os = "tvos"))]
            View::TextArea(t) => {
                t.placeholder.setText(Some(&NSString::from_str(text)));
                t.layout_placeholder();
            }
            View::Picker(b) => {
                if let Some(p) = self.pickers.get_mut(&view.id()) {
                    p.placeholder = text.to_string();
                    crate::forms::refresh_picker(b, p);
                }
            }
            // Shown while an optional date picker is empty (#89).
            #[cfg(not(target_os = "tvos"))]
            View::DatePicker(p) => crate::dates::set_placeholder(p, text),
            #[cfg(target_os = "tvos")]
            View::DatePicker(p) => p.set_placeholder(text),
            View::Timer(l) => {
                self.timer_after.insert(view.id(), text.to_string());
                if let Some((_, end)) = self.timers.get(&view.id()) {
                    l.setText(Some(&NSString::from_str(&prism_core::timer_text(
                        *end, text,
                    ))));
                }
            }
            _ => {}
        }
    }

    fn set_config(&mut self, view: &View, _kind: NodeKind, config: &str) {
        use prism_core::controls::{DateConfig, PickerConfig, SliderConfig};
        match view {
            // A filled picture's focal point (#218).
            View::Picture(v) => {
                let at = prism_core::picture_position(config);
                PICTURES.with(|p| {
                    p.borrow_mut().entry(view.id()).or_default().1 =
                        (at != (0.5, 0.5)).then_some(at)
                });
                refocus(v);
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
            // Bar buttons and title size (#30).
            View::Screen(_) => {
                let id = view.id();
                crate::navbar::set_config(id, config, self.screens.get(&id).map(|vc| &***vc));
            }
            // A rich text's runs.
            View::Label(l) => crate::label::set_runs(l, config),
            View::Input(f) => {
                crate::forms::apply_input_config(f, config);
                let id = view.id();
                let c = prism_core::controls::InputConfig::decode(config);
                crate::interaction::set_max_length(id, c.max_length);
                crate::interaction::set_suggestions(
                    id,
                    prism_core::controls::input_suggestions(config),
                );
            }
            View::Radio(r) => r.set_config(config),
            View::Overlay(o) => o.set_config(config),
            #[cfg(not(target_os = "tvos"))]
            View::TextArea(t) => {
                crate::forms::apply_input_config(&t.view, config);
                // Its minimum lines (#235).
                t.rows
                    .set(prism_core::controls::InputConfig::decode(config).min_rows());
            }
            #[cfg(not(target_os = "tvos"))]
            View::Slider(s) => s.configure(SliderConfig::decode(config)),
            #[cfg(target_os = "tvos")]
            View::Slider(s) => s.set_slider_config(SliderConfig::decode(config)),
            View::Picker(b) => {
                if let Some(p) = self.pickers.get_mut(&view.id()) {
                    p.options = PickerConfig::decode(config).options;
                    crate::forms::refresh_picker(b, p);
                }
            }
            #[cfg(not(target_os = "tvos"))]
            View::DatePicker(p) => {
                use objc2_ui_kit::UIDatePickerMode as M;
                let c = DateConfig::decode(config);
                p.setDatePickerMode(match c.mode {
                    prism_core::DateMode::Date => M::Date,
                    prism_core::DateMode::Time | prism_core::DateMode::TimeOfDay => M::Time,
                    prism_core::DateMode::DateTime => M::DateAndTime,
                });
                // A time of day is the same in every zone: shown in UTC.
                let of_day = c.mode == prism_core::DateMode::TimeOfDay;
                crate::interaction::set_time_of_day(view.id(), of_day);
                let utc =
                    of_day.then(|| objc2_foundation::NSTimeZone::timeZoneForSecondsFromGMT(0));
                p.setTimeZone(utc.as_deref());
                let date =
                    |v: Option<f64>| v.map(objc2_foundation::NSDate::dateWithTimeIntervalSince1970);
                p.setMinimumDate(date(c.min).as_deref());
                p.setMaximumDate(date(c.max).as_deref());
                let target: &AnyObject = &self.target;
                crate::dates::configure(p, c, target);
            }
            #[cfg(target_os = "tvos")]
            View::DatePicker(p) => p.set_date_config(DateConfig::decode(config)),
            _ => {}
        }
    }

    fn set_clickable(&mut self, view: &View, clickable: bool) {
        // A "clickable" scroll view is refreshable: pull to refresh (no
        // UIRefreshControl on tvOS).
        #[cfg(target_os = "tvos")]
        if let View::Scroll(_) = view {
            return;
        }
        #[cfg(not(target_os = "tvos"))]
        if let View::Scroll(s) = view {
            if clickable {
                let rc = objc2_ui_kit::UIRefreshControl::new(self.mtm);
                rc.setTag(s.tag());
                let target: &AnyObject = &self.target;
                unsafe {
                    rc.addTarget_action_forControlEvents(
                        Some(target),
                        sel!(onRefresh:),
                        UIControlEvents::ValueChanged,
                    )
                };
                s.setRefreshControl(Some(&rc));
            } else {
                s.setRefreshControl(None);
            }
            return;
        }
        if let View::Label(l) = view {
            crate::label::set_clickable(l, clickable);
            return;
        }
        let View::Container(v) = view else { return };
        crate::decor::set_clickable(v, clickable);
        crate::interaction::set_activatable(v, clickable);
        if let Some(existing) = v.gestureRecognizers() {
            for g in existing.iter() {
                if g.isKindOfClass(UITapGestureRecognizer::class()) {
                    v.removeGestureRecognizer(&g);
                }
            }
        }
        if clickable {
            let target: &AnyObject = &self.target;
            let tap = unsafe {
                UITapGestureRecognizer::initWithTarget_action(
                    UITapGestureRecognizer::alloc(self.mtm),
                    Some(target),
                    Some(sel!(onTap:)),
                )
            };
            #[cfg(target_os = "tvos")]
            crate::tv::select_press_only(&tap);
            // The delegate notes each touch's tap count (#159).
            let _: () = unsafe { msg_send![&*tap, setDelegate: target] };
            v.addGestureRecognizer(&tap);
        }
        #[cfg(target_os = "tvos")]
        crate::tv::set_focusable(v, clickable);
    }

    fn set_children(&mut self, parent: &View, children: &[View]) {
        if let View::Overlay(o) = parent {
            let views: Vec<Retained<UIView>> = children.iter().map(|c| c.ui().retain()).collect();
            o.set_content(&views);
            return;
        }
        // Sheets attached to a root container are presented modally.
        let (sheets, children): (Vec<View>, Vec<View>) = children
            .iter()
            .cloned()
            .partition(|c| matches!(c, View::Sheet(_)));
        if matches!(parent, View::Stack(_) | View::Tabs(_)) {
            self.present_sheets(&sheets);
        }
        let children = &children[..];
        if let View::Stack(stack) = parent {
            let controllers: Vec<Retained<UIViewController>> = children
                .iter()
                .map(|c| Retained::into_super(self.screen_controller(c)))
                .collect();
            let current = stack.nav.viewControllers();
            let unchanged = current.count() == controllers.len()
                && current.iter().zip(&controllers).all(|(a, b)| *a == **b);
            if !unchanged {
                // Animate pushes/pops, but not the very first screen(s).
                let animated = current.count() > 0;
                stack.nav.setViewControllers_animated(
                    &NSArray::from_retained_slice(&controllers),
                    animated,
                );
            }
            self.apply_close_button(stack, parent.id());
            return;
        }
        if let View::Tabs(bar) = parent {
            let controllers: Vec<Retained<UIViewController>> = children
                .iter()
                .map(|c| Retained::into_super(self.screen_controller(c)))
                .collect();
            bar.controller.setViewControllers_animated(
                Some(&NSArray::from_retained_slice(&controllers)),
                false,
            );
            if let Some(i) = self.selected.get(&parent.id()) {
                bar.controller.setSelectedIndex(*i);
            }
            return;
        }
        let parent = parent.ui();
        for (i, child) in children.iter().enumerate() {
            parent.insertSubview_atIndex(child.ui(), i as isize);
        }
        // Remove former Prism children only (tag > 0); UIKit's own subviews,
        // like scroll indicators, have tag 0.
        let keep: Vec<isize> = children.iter().map(|c| c.ui().tag()).collect();
        // A copy: removing from the array being enumerated aborts (#66).
        for sub in parent.subviews().to_vec() {
            if sub.tag() > 0 && !keep.contains(&sub.tag()) {
                sub.removeFromSuperview();
            }
        }
        crate::decor::after_children(parent);
    }

    fn set_root(&mut self, view: &View) {
        for sub in self.root.subviews().to_vec() {
            sub.removeFromSuperview();
        }
        for child in self.controller.childViewControllers().to_vec() {
            child.removeFromParentViewController();
        }
        // Proper containment, so container controllers get the safe area,
        // rotation and appearance callbacks.
        let child: Option<Retained<UIViewController>> = match view {
            View::Stack(stack) => Some(Retained::into_super(stack.nav.clone())),
            View::Tabs(bar) => Some(Retained::into_super(bar.controller.clone())),
            _ => None,
        };
        if let Some(child) = &child {
            self.controller.addChildViewController(child);
        }
        self.root_tabs = match view {
            View::Tabs(bar) => Some(bar.controller.clone()),
            _ => None,
        };
        // Give the new root the root's size before it joins the window: a
        // tab bar controller builds its bar for the size its view enters
        // with, and a foldable's side bar is not rebuilt when the layout
        // resizes it just after, only on a trait change (#39).
        let size = self.root.bounds().size;
        if size.width > 0. && size.height > 0. {
            place(view.ui(), 0., 0., size.width as f32, size.height as f32);
        }
        self.root.addSubview(view.ui());
        if let Some(child) = &child {
            child.didMoveToParentViewController(Some(&self.controller));
        }
    }

    fn set_drawing(&mut self, view: &View, paths: &[prism_core::Path]) {
        SHAPES.with(|s| s.borrow_mut().insert(view.id(), paths.to_vec()));
        view.ui().setNeedsDisplay();
    }

    fn set_scene(&mut self, view: &View, update: &prism_core::scene3d::SceneUpdate, json: &str) {
        crate::scene::set(view.ui(), update, json, &self.image_sources);
    }

    fn destroy(&mut self, view: &View) {
        let id = view.id();
        crate::forms::forget_selection(id);
        #[cfg(not(target_os = "tvos"))]
        crate::carets::forget(id);
        #[cfg(not(target_os = "tvos"))]
        crate::edit::forget(id);
        #[cfg(not(target_os = "tvos"))]
        crate::rich::forget(id);
        crate::decor::destroy(id);
        crate::navbar::forget(id);
        crate::padding::forget(id);
        SHAPES.with(|s| s.borrow_mut().remove(&id));
        crate::layout::forget(id);
        self.screens.remove(&id);
        self.radii.remove(&id);
        self.titles.remove(&id);
        self.icons.remove(&id);
        self.selected.remove(&id);
        self.timers.remove(&id);
        self.timer_after.remove(&id);
        self.pickers.remove(&id);
        self.date_pickers.remove(&id);
        #[cfg(not(target_os = "tvos"))]
        crate::dates::forget(id);
        self.announced.remove(&id);
        if let Some(o) = self.overlays.remove(&id) {
            o.close();
        }
        self.checkboxes.remove(&id);
        self.no_hit.remove(&id);
        self.inert.remove(&id);
        self.a11y_hidden.remove(&id);
        self.layers.retain(|l| l.tag() as NodeId != id);
        crate::interaction::forget(id);
        self.pictures.retain(|_, ids| {
            ids.retain(|(i, _)| *i != id);
            !ids.is_empty()
        });
        view.ui().removeFromSuperview();
    }

    fn measure(&mut self, view: &View, max_width: Option<f32>) -> (f32, f32) {
        // A countdown with an `after` text is as large as the larger of the
        // two, so the text fits once it ends (#281).
        if let View::Timer(l) = view
            && let Some(after) = self.timer_after.remove(&view.id())
        {
            let shown = l.text();
            l.setText(Some(&NSString::from_str(&after)));
            let ended = self.measure(view, max_width);
            l.setText(shown.as_deref());
            let counting = self.measure(view, max_width);
            self.timer_after.insert(view.id(), after);
            return (ended.0.max(counting.0), ended.1.max(counting.1));
        }
        let limit = CGSize::new(max_width.map_or(f64::MAX / 4., |w| w as f64), f64::MAX / 4.);
        let size = match view {
            View::Toggle(t) => t.sizeThatFits(limit),
            View::Image(v) => v.sizeThatFits(limit),
            View::Timer(l) => l.sizeThatFits(limit),
            // A progress bar takes the width it is given.
            View::Progress(_) => CGSize::new(max_width.unwrap_or(120.) as f64, 4.),
            // The video's size, once AVKit knows it (#207).
            View::Video(v) => match &v.host {
                Some(h) => {
                    let (w, h) = prism_native::apple::video::measure(h, max_width);
                    CGSize::new(w as f64, h as f64)
                }
                None => CGSize::ZERO,
            },
            // Natural size of the loaded image, scaled to the available width.
            View::Picture(v) => match v.image() {
                Some(img) => {
                    let size = unsafe { img.size() };
                    let w = max_width.map_or(size.width, |m| (m as f64).min(size.width));
                    CGSize::new(
                        w,
                        if size.width > 0. {
                            w * size.height / size.width
                        } else {
                            0.
                        },
                    )
                }
                None => CGSize::ZERO,
            },
            View::Container(_)
            | View::Screen(_)
            | View::Stack(_)
            | View::Tabs(_)
            | View::Tab(_)
            | View::Scroll(_) => CGSize::ZERO,
            View::Input(f) => {
                let style = crate::padding::style_of(view.id());
                if crate::padding::custom_field(&style) {
                    crate::padding::field_content(f, &style)
                } else {
                    let s = f.sizeThatFits(limit);
                    CGSize::new(s.width.max(120.), s.height.max(36.))
                }
            }
            // Padded: without its content insets (the layout adds them).
            View::Button(b) => crate::padding::button_content(
                b.sizeThatFits(limit),
                &crate::padding::style_of(view.id()),
            ),
            // A comfortable default width; flex makes it wider.
            #[cfg(not(target_os = "tvos"))]
            View::Slider(s) => {
                let h = s.sizeThatFits(limit).height.max(31.);
                CGSize::new(max_width.map_or(200., |w| (w as f64).min(200.)), h)
            }
            #[cfg(target_os = "tvos")]
            View::Slider(s) => {
                let n = s.natural_size();
                CGSize::new(
                    max_width.map_or(n.width, |w| (w as f64).min(n.width)),
                    n.height,
                )
            }
            #[cfg(target_os = "tvos")]
            View::DatePicker(p) => p.natural_size(),
            #[cfg(not(target_os = "tvos"))]
            // Compact pickers have no intrinsic size (-1); Auto Layout's
            // fitting size is their button's.
            View::DatePicker(p) => {
                let size = p.systemLayoutSizeFittingSize(unsafe {
                    objc2_ui_kit::UILayoutFittingCompressedSize
                });
                // With its placeholder or clear button (#89).
                crate::dates::natural_size(p, size).unwrap_or(size)
            }
            View::Spinner(s) => s.intrinsicContentSize(),
            View::Checkbox(c) => c.button.sizeThatFits(limit),
            View::Radio(r) => r.measure(max_width),
            View::Overlay(_) => CGSize::ZERO,
            #[cfg(not(target_os = "tvos"))]
            View::TextArea(t) => {
                // Its padding is its text container's inset: measured with
                // it, returned without (the layout adds it).
                let p = crate::padding::style_of(view.id()).padding;
                let w = max_width.map_or(240., |w| w as f64);
                let s = t.measure(w + (p.left + p.right) as f64);
                CGSize::new(w, (s.height - (p.top + p.bottom) as f64).max(0.))
            }
            other => other.ui().sizeThatFits(limit),
        };
        (size.width.ceil() as f32, size.height.ceil() as f32)
    }

    /// A label wraps down to its longest word (CSS min-content), so flex
    /// rows shrink it before squeezing fixed-size neighbours.
    fn measure_min_content(&mut self, view: &View) -> f32 {
        let View::Label(l) = view else {
            return self.measure(view, None).0;
        };
        let text = l.text().map(|t| t.to_string()).unwrap_or_default();
        let scratch = UILabel::new(self.mtm);
        unsafe { scratch.setFont(l.font().as_deref()) };
        let limit = CGSize::new(f64::MAX / 4., f64::MAX / 4.);
        text.split_whitespace()
            .map(|word| {
                scratch.setText(Some(&NSString::from_str(word)));
                scratch.sizeThatFits(limit).width.ceil() as f32
            })
            .fold(0., f32::max)
    }

    fn set_frame(&mut self, view: &View, x: f32, y: f32, w: f32, h: f32) {
        match view {
            View::Stack(st) => ensure_contained(&st.nav, &st.view),
            View::Tabs(bar) => ensure_contained(&bar.controller, &bar.view),
            _ => {}
        }
        place(view.ui(), x, y, w, h);
        crate::layout::framed(view.id());
        #[cfg(not(target_os = "tvos"))]
        crate::carets::refresh(view.id());
        if let View::Picture(v) = view {
            refocus(v);
        }
        #[cfg(not(target_os = "tvos"))]
        if let View::DatePicker(p) = view {
            crate::dates::framed(p);
        }
        #[cfg(not(target_os = "tvos"))]
        if let View::TextArea(t) = view {
            t.layout_placeholder();
        }
        if let View::Radio(r) = view {
            r.layout(w as f64);
        }
        if let Some(r) = self.radii.get(&view.id()) {
            view.ui()
                .layer()
                .setCornerRadius(r.min(w.min(h) as f64 / 2.).max(0.));
        }
        crate::decor::layout(view.ui());
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
        let v: Retained<UIView> = view.ui().retain();
        let old = (v.bounds(), v.center());
        self.set_frame(view, x, y, w, h);
        let new = (v.bounds(), v.center());
        v.setBounds(old.0);
        v.setCenter(old.1);
        // Drop shadows move along.
        let shadow = crate::decor::companion(view.id());
        if let Some(c) = &shadow {
            c.setBounds(old.0);
            c.setCenter(old.1);
        }
        animate(self.mtm, t, move || {
            v.setBounds(new.0);
            v.setCenter(new.1);
            if let Some(c) = &shadow {
                c.setBounds(new.0);
                c.setCenter(new.1);
            }
        });
    }

    fn set_style_animated(&mut self, view: &View, style: &Style, t: prism_core::Transition) {
        let v: Retained<UIView> = view.ui().retain();
        let (alpha, bg, transform) = (v.alpha(), v.backgroundColor(), v.transform());
        self.set_style(view, style);
        let (new_alpha, new_bg, new_transform) = (v.alpha(), v.backgroundColor(), v.transform());
        v.setAlpha(alpha);
        v.setBackgroundColor(bg.as_deref());
        v.setTransform(transform);
        let shadow = crate::decor::companion(view.id());
        if let Some(c) = &shadow {
            c.setAlpha(alpha);
            c.setTransform(transform);
        }
        animate(self.mtm, t, move || {
            v.setAlpha(new_alpha);
            v.setBackgroundColor(new_bg.as_deref());
            v.setTransform(new_transform);
            if let Some(c) = &shadow {
                c.setAlpha(new_alpha);
                c.setTransform(new_transform);
            }
        });
    }

    /// A snapshot of the view stays in its place and animates out.
    fn animate_exit(&mut self, parent: &View, view: &View, to: &Style, t: prism_core::Transition) {
        let (parent, v) = (parent.ui(), view.ui());
        if v.window().is_none() || v.superview().is_none_or(|s| *s != *parent) {
            return;
        }
        let Some(snapshot) = v.snapshotViewAfterScreenUpdates(false) else {
            return;
        };
        snapshot.setUserInteractionEnabled(false);
        snapshot.setBounds(CGRect::new(CGPoint::ZERO, v.bounds().size));
        snapshot.setCenter(v.center());
        snapshot.setTransform(v.transform());
        snapshot.setAlpha(v.alpha());
        parent.insertSubview_aboveSubview(&snapshot, v);
        let (alpha, transform) = (
            to.opacity as f64,
            prism_native::apple::motion::affine(to.transform),
        );
        let s = snapshot.clone();
        let done = snapshot.clone();
        animate_then(
            self.mtm,
            t,
            move || {
                s.setAlpha(alpha);
                s.setTransform(transform);
            },
            move || done.removeFromSuperview(),
        );
    }

    fn set_repeat(
        &mut self,
        view: &View,
        style: &Style,
        repeat: Option<(prism_core::Pose, prism_core::Transition)>,
    ) {
        use prism_native::apple::motion;
        let layer = view.ui().layer();
        match repeat {
            None => motion::stop_repeat(&layer),
            Some((pose, t)) => {
                let (opacity, transform) = pose.apply(style.opacity, style.transform);
                motion::repeat(
                    &layer,
                    (style.opacity, style.transform),
                    (opacity, transform),
                    t,
                    style.animations.restart,
                    |tr| motion::transform3d(motion::affine(tr)),
                );
            }
        }
    }

    fn set_environment(&mut self, env: &prism_core::RenderEnvironment) {
        use objc2_ui_kit::UISemanticContentAttribute as A;
        let attr = if env.direction == prism_core::Direction::Rtl {
            A::ForceRightToLeft
        } else {
            A::ForceLeftToRight
        };
        // Navigation bars, tab bars, back buttons and swipe-back follow it;
        // views created later get it in `create`.
        self.semantic = Some(attr);
        set_appearance_semantic(attr);
        // Date pickers speak the app's language (month names, digits).
        #[cfg(not(target_os = "tvos"))]
        crate::dates::set_clear_label(&env.clear);
        if self.lang != env.lang {
            self.lang = env.lang.clone();
            for p in self.date_pickers.values() {
                #[cfg(not(target_os = "tvos"))]
                p.setLocale(Some(&ns_locale(&self.lang)));
                #[cfg(target_os = "tvos")]
                p.set_lang(&self.lang);
            }
        }
        match self.controller.view().and_then(|v| v.window()) {
            Some(window) => set_semantic(&window, attr),
            None => set_semantic(&self.root, attr),
        }
    }

    fn set_value(&mut self, view: &View, value: f64) {
        match view {
            View::Stack(st) => {
                let flags = value as u32;
                st.closable.set(flags & prism_core::STACK_CLOSABLE != 0);
                self.apply_close_button(st, view.id());
                set_no_bar(view.id(), flags & prism_core::STACK_NO_HEADER != 0);
                if let Some(top) = st.nav.topViewController() {
                    apply_navigation_bar(&top, false);
                }
            }
            View::Screen(_) => {
                set_no_bar(view.id(), value == 1.);
                if let Some(vc) = self.screens.get(&view.id())
                    && let Some(nav) = vc.navigationController()
                    && nav.topViewController().as_deref() == Some(&***vc)
                {
                    apply_navigation_bar(vc, true);
                }
            }
            View::Progress(p) => p.setProgress_animated(value as f32, true),
            View::Picture(v) => {
                PICTURES.with(|p| p.borrow_mut().entry(view.id()).or_default().0 = value == 1.);
                refocus(v);
            }
            #[cfg(not(target_os = "tvos"))]
            View::Scroll(s) => {
                if let Some(rc) = s.refreshControl() {
                    if value != 0. && !rc.isRefreshing() {
                        rc.beginRefreshing();
                    } else if value == 0. && rc.isRefreshing() {
                        rc.endRefreshing();
                    }
                }
            }
            View::Timer(l) => {
                self.timers
                    .insert(view.id(), (Retained::into_super(l.clone()), value));
                let after = self.timer_after.get(&view.id()).map_or("", String::as_str);
                l.setText(Some(&NSString::from_str(&prism_core::timer_text(
                    value, after,
                ))));
            }
            #[cfg(not(target_os = "tvos"))]
            View::Toggle(t) => {
                if t.isOn() != (value != 0.) {
                    t.setOn_animated(value != 0., true);
                }
            }
            #[cfg(target_os = "tvos")]
            View::Toggle(t) => crate::tv::set_toggle(t, value != 0.),
            #[cfg(not(target_os = "tvos"))]
            View::Slider(s) => s.set(value),
            #[cfg(target_os = "tvos")]
            View::Slider(s) => s.set_value(value),
            View::Picker(b) => {
                if let Some(p) = self.pickers.get_mut(&view.id()) {
                    p.selected = (value >= 0.).then_some(value as usize);
                    crate::forms::refresh_picker(b, p);
                }
            }
            #[cfg(not(target_os = "tvos"))]
            View::DatePicker(p) => {
                // An optional picker shows NaN as its placeholder (#89).
                crate::dates::set_value(p, value);
                if !value.is_nan() && (p.date().timeIntervalSince1970() - value).abs() >= 0.5 {
                    let d = objc2_foundation::NSDate::dateWithTimeIntervalSince1970(value);
                    p.setDate_animated(&d, false);
                }
            }
            #[cfg(target_os = "tvos")]
            View::DatePicker(p) => p.set_value(value),
            View::Spinner(s) => s.setActivityIndicatorViewStyle(if value == 1. {
                objc2_ui_kit::UIActivityIndicatorViewStyle::Large
            } else {
                objc2_ui_kit::UIActivityIndicatorViewStyle::Medium
            }),
            View::Checkbox(c) => c.set_on(value != 0.),
            View::Radio(r) => r.set_selected((value >= 0.).then_some(value as usize)),
            View::Overlay(o) => {
                let root: Retained<UIViewController> =
                    Retained::into_super(self.controller.clone());
                o.set_open(value != 0., &root);
            }
            View::Tabs(bar) => {
                let i = value.max(0.) as usize;
                self.selected.insert(view.id(), i);
                if bar.controller.selectedIndex() != i {
                    bar.controller.setSelectedIndex(i);
                }
            }
            _ => {}
        }
    }

    fn set_icon(&mut self, view: &View, icon: &str) {
        if let View::Input(f) = view {
            crate::interaction::set_leading_icon(f, icon);
            return;
        }
        if let View::Image(v) = view {
            v.setImage(UIImage::systemImageNamed(&NSString::from_str(icon)).as_deref());
            return;
        }
        let id = view.id();
        self.icons.insert(id, icon.to_string());
        if let Some(item) = self.screens.get(&id).and_then(|vc| vc.tabBarItem()) {
            item.setImage(UIImage::systemImageNamed(&NSString::from_str(icon)).as_deref());
        }
    }

    fn set_appearance(&mut self, appearance: Appearance) {
        let style = match appearance {
            Appearance::System => objc2_ui_kit::UIUserInterfaceStyle::Unspecified,
            Appearance::Light => objc2_ui_kit::UIUserInterfaceStyle::Light,
            Appearance::Dark => objc2_ui_kit::UIUserInterfaceStyle::Dark,
        };
        self.controller.setOverrideUserInterfaceStyle(style);
    }

    fn set_scroll_events(&mut self, view: &View, on: bool) {
        if let View::Scroll(s) = view {
            crate::layout::set_scroll_events(view.id(), on);
            // Sticky views follow the scroll position too.
            let on = crate::layout::wants_delegate(view.id());
            let delegate = on.then(|| ProtocolObject::from_ref(&*self.target));
            unsafe { s.setDelegate(delegate) };
        }
    }

    fn set_sticky(&mut self, view: &View, sticky: Option<(&View, prism_native::Sticky)>) {
        match sticky {
            Some((View::Scroll(s), st)) => {
                if s.delegate().is_none() {
                    unsafe { s.setDelegate(Some(ProtocolObject::from_ref(&*self.target))) };
                }
                crate::layout::set_sticky(view.ui(), view.id(), Some((s, st)));
            }
            _ => crate::layout::set_sticky(view.ui(), view.id(), None),
        }
    }

    fn baseline(&mut self, view: &View, width: f32, height: f32) -> Option<f32> {
        crate::layout::baseline(view, width, height)
    }

    fn set_swipe(&mut self, view: &View, left: bool, right: bool) {
        let v = view.ui();
        if let Some(existing) = v.gestureRecognizers() {
            for g in existing.iter() {
                if g.isKindOfClass(objc2_ui_kit::UISwipeGestureRecognizer::class()) {
                    v.removeGestureRecognizer(&g);
                }
            }
        }
        use objc2_ui_kit::UISwipeGestureRecognizerDirection as D;
        for (on, dir) in [(left, D::Left), (right, D::Right)] {
            if !on {
                continue;
            }
            let target: &AnyObject = &self.target;
            let swipe = unsafe {
                objc2_ui_kit::UISwipeGestureRecognizer::initWithTarget_action(
                    objc2_ui_kit::UISwipeGestureRecognizer::alloc(self.mtm),
                    Some(target),
                    Some(sel!(onSwipe:)),
                )
            };
            swipe.setDirection(dir);
            v.addGestureRecognizer(&swipe);
        }
    }

    fn set_menu(&mut self, view: &View, menu: Option<&prism_core::menu::MenuSpec>) {
        let id = view.id();
        let v = view.ui();
        let has = self.menus.contains_key(&id);
        match menu {
            Some(spec) => {
                self.menus.insert(id, spec.clone());
                if !has {
                    let interaction = objc2_ui_kit::UIContextMenuInteraction::initWithDelegate(
                        objc2_ui_kit::UIContextMenuInteraction::alloc(self.mtm),
                        ProtocolObject::from_ref(&*self.target),
                    );
                    v.addInteraction(ProtocolObject::from_ref(&*interaction));
                }
            }
            None => {
                self.menus.remove(&id);
                for i in v.interactions().iter() {
                    v.removeInteraction(&i);
                }
            }
        }
    }

    /// A pop-up menu (#247) over the view (the window's content): see
    /// [`crate::menus::pop_up`]. After the patches, once laid out.
    fn show_menu(
        &mut self,
        view: Option<&View>,
        menu: &prism_core::menu::MenuSpec,
        at: Option<(f32, f32)>,
    ) {
        let parent: Retained<UIView> = match view {
            Some(v) => objc2::Message::retain(v.ui()),
            None => self.root.clone(),
        };
        let parent = dispatch2::MainThreadBound::new(parent, self.mtm);
        let spec = menu.clone();
        dispatch2::DispatchQueue::main().exec_async(move || {
            let mtm = MainThreadMarker::new().expect("the main queue");
            crate::menus::pop_up(mtm, parent.get(mtm), &spec, at, dispatch);
        });
    }

    fn set_content_size(&mut self, view: &View, width: f32, height: f32) {
        if let View::Overlay(o) = view {
            o.set_size(width as f64, height as f64);
            // Opened in the patch that inserted it: its anchor is placed now.
            let root: Retained<UIViewController> = Retained::into_super(self.controller.clone());
            o.present_if_pending(&root);
            return;
        }
        if let View::Scroll(s) = view {
            s.setContentSize(CGSize::new(width as f64, height as f64));
        }
    }

    fn set_table(&mut self, table: &View, rows: &[prism_native::table::TableRow<'_, View>]) {
        #[cfg(not(target_os = "tvos"))]
        {
            let rows: Vec<(&UIView, bool, Vec<&UIView>)> = rows
                .iter()
                .map(|r| {
                    (
                        r.view.ui(),
                        r.header,
                        r.cells.iter().map(|c| c.ui()).collect(),
                    )
                })
                .collect();
            crate::table::set(self.mtm, table.ui(), &rows);
        }
        #[cfg(target_os = "tvos")]
        let _ = (table, rows);
    }

    fn set_accessibility(&mut self, view: &View, a: &Accessibility) {
        let mtm = self.mtm;
        let v = view.ui();
        // Tooltips (#216): under an iPad or visionOS pointer.
        #[cfg(not(target_os = "tvos"))]
        {
            use objc2_ui_kit::UIToolTipInteraction;
            let tip = UIToolTipInteraction::class();
            let existing = v.interactions().iter().find(|i| {
                let any: &AnyObject = i.as_ref();
                std::ptr::eq(any.class(), tip)
            });
            match (&a.tooltip, existing) {
                (Some(prism_core::Tooltip::Text(t)), Some(i)) => {
                    let any: &AnyObject = i.as_ref();
                    if let Some(i) = any.downcast_ref::<UIToolTipInteraction>() {
                        i.setDefaultToolTip(Some(&NSString::from_str(t)));
                    }
                }
                (Some(prism_core::Tooltip::Text(t)), None) => {
                    let i = UIToolTipInteraction::initWithDefaultToolTip(
                        UIToolTipInteraction::alloc(mtm),
                        &NSString::from_str(t),
                    );
                    v.addInteraction(ProtocolObject::from_ref(&*i));
                }
                (_, Some(i)) => v.removeInteraction(&i),
                (_, None) => {}
            }
        }
        let ns = |s: &Option<String>| s.as_deref().map(NSString::from_str);
        let id = view.id();
        if a.hidden {
            self.a11y_hidden.insert(id);
        } else {
            self.a11y_hidden.remove(&id);
        }
        v.setAccessibilityElementsHidden(a.hidden || self.inert.contains(&id), mtm);
        match view {
            View::Overlay(_) => return,
            View::Radio(r) => {
                r.set_label(a.label.as_deref());
                return;
            }
            _ => {}
        }
        // Platform containers announce themselves through their titles.
        if matches!(
            view,
            View::Stack(_) | View::Tabs(_) | View::Tab(_) | View::Screen(_) | View::Scroll(_)
        ) {
            return;
        }
        // A live region: not an element itself, it announces its value.
        if a.role == Some(Role::Status) {
            v.setIsAccessibilityElement(false, mtm);
            let message = a.value.clone().unwrap_or_default();
            let previous = self.announced.insert(view.id(), message.clone());
            if !message.is_empty() && previous.as_ref() != Some(&message) {
                crate::forms::announce(&message);
            }
            return;
        }
        let native_control = match view {
            View::Button(_)
            | View::Input(_)
            | View::Toggle(_)
            | View::Slider(_)
            | View::Picker(_)
            | View::DatePicker(_)
            | View::Spinner(_) => true,
            #[cfg(not(target_os = "tvos"))]
            View::TextArea(_) => true,
            _ => false,
        };
        if !native_control {
            // Operable containers and images are single elements; plain text
            // labels already are; groups gather their children.
            let element = a.role.is_some_and(|r| {
                r.is_interactive() || matches!(r, Role::Image | Role::Cell | Role::ColumnHeader)
            }) || matches!(view, View::Label(_));
            v.setIsAccessibilityElement(element && !a.hidden, mtm);
            v.setShouldGroupAccessibilityChildren(a.role == Some(Role::Group), mtm);
            // Drawn choices (#84), like the native radio group: VoiceOver
            // reads the group's label when the focus enters it.
            if let View::Container(_) = view {
                v.setAccessibilityContainerType(
                    match a.role {
                        Some(Role::RadioGroup) => {
                            objc2_ui_kit::UIAccessibilityContainerType::SemanticGroup
                        }
                        // Its cells come from `set_table` (#284).
                        Some(Role::Table) => objc2_ui_kit::UIAccessibilityContainerType::DataTable,
                        _ => objc2_ui_kit::UIAccessibilityContainerType::None,
                    },
                    mtm,
                );
            }
            let mut traits = 0;
            unsafe {
                traits |= match a.role {
                    Some(Role::Button) => UIAccessibilityTraitButton,
                    Some(Role::Link) => UIAccessibilityTraitLink,
                    Some(Role::Heading(_)) => UIAccessibilityTraitHeader,
                    Some(Role::Image) => UIAccessibilityTraitImage,
                    Some(Role::Text) => UIAccessibilityTraitStaticText,
                    Some(Role::Switch | Role::Checkbox) => {
                        UIAccessibilityTraitToggleButton | UIAccessibilityTraitButton
                    }
                    Some(Role::Tab | Role::Radio) => UIAccessibilityTraitButton,
                    Some(Role::TabList) => UIAccessibilityTraitTabBar,
                    _ => 0,
                };
                if a.disabled {
                    traits |= UIAccessibilityTraitNotEnabled;
                }
                // Selected tabs and items, the current page of a navigation.
                if a.is_selected() {
                    traits |= UIAccessibilityTraitSelected;
                }
            }
            v.setAccessibilityTraits(traits, mtm);
        } else {
            // A native control keeps the traits UIKit gives it; the selected
            // one is added or taken away. Once set, the traits no longer
            // follow `enabled` by themselves: the not-enabled one is kept in
            // step too.
            let (selected, not_enabled) =
                unsafe { (UIAccessibilityTraitSelected, UIAccessibilityTraitNotEnabled) };
            let traits = v.accessibilityTraits(mtm);
            if a.is_selected() || traits & selected != 0 {
                let mut wanted = traits & !(selected | not_enabled);
                if a.is_selected() {
                    wanted |= selected;
                }
                if a.disabled {
                    wanted |= not_enabled;
                }
                if wanted != traits {
                    v.setAccessibilityTraits(wanted, mtm);
                }
            }
        }
        // Labels override the visible text only when given.
        if a.label.is_some() || !native_control {
            v.setAccessibilityLabel(ns(&a.label).as_deref(), mtm);
        }
        v.setAccessibilityHint(ns(&a.hint).as_deref(), mtm);
        let value = a.value.clone().or_else(|| match (a.role, a.checked) {
            (Some(Role::Switch | Role::Checkbox), Some(on)) if !native_control => {
                Some(if on { "1".into() } else { "0".into() })
            }
            _ => None,
        });
        if value.is_some() || !native_control {
            v.setAccessibilityValue(ns(&value).as_deref(), mtm);
        }
    }

    fn set_pointer_events(&mut self, view: &View, mask: u8) {
        // tvOS has no pointer over views (the remote moves a focus).
        #[cfg(not(target_os = "tvos"))]
        crate::interaction::set_pointer_events(view.ui(), mask, &self.target);
        #[cfg(target_os = "tvos")]
        let _ = (view, mask);
    }

    fn set_drag_delay(&mut self, view: &View, ms: u32) {
        // tvOS has no touch on views.
        #[cfg(not(target_os = "tvos"))]
        crate::interaction::set_drag_delay(view.ui(), ms, &self.target);
        #[cfg(target_os = "tvos")]
        let _ = (view, ms);
    }

    // --- Drag and drop between elements (#251) ---

    fn window_origin(&mut self, view: &View) -> Option<(f32, f32)> {
        let v = view.ui();
        v.window()?;
        let r = v.convertRect_toView(v.bounds(), Some(&self.root));
        Some((r.origin.x as f32, r.origin.y as f32))
    }

    fn node_at(&mut self, x: f32, y: f32) -> Option<NodeId> {
        let p = objc2_core_foundation::CGPoint::new(x as f64, y as f64);
        let mut view = self.root.hitTest_withEvent(p, None);
        while let Some(v) = view {
            if v.tag() != 0 {
                return Some(v.tag() as NodeId);
            }
            view = v.superview();
        }
        None
    }

    fn scroll_offset(&mut self, view: &View) -> (f32, f32) {
        match view {
            View::Scroll(s) => {
                let o = s.contentOffset();
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
            let Some(snapshot) = view.ui().snapshotViewAfterScreenUpdates(false) else {
                return;
            };
            // Never what is under the finger: hit-testing skips it.
            snapshot.setUserInteractionEnabled(false);
            snapshot.setAlpha(0.85);
            let layer = snapshot.layer();
            layer.setShadowOpacity(0.25);
            layer.setShadowRadius(10.);
            layer.setShadowOffset(objc2_core_foundation::CGSize::new(0., 4.));
            self.root.addSubview(&snapshot);
            self.drag_preview = Some(snapshot);
        }
        if let Some(p) = &self.drag_preview {
            let mut frame = p.frame();
            frame.origin = objc2_core_foundation::CGPoint::new(x as f64, y as f64);
            p.setFrame(frame);
        }
    }

    fn set_drop_target(&mut self, view: &View, types: Option<&[String]>) {
        // A UIDropInteraction (#111); tvOS has no drag and drop.
        // SAFETY: the node's view, alive, on the main thread.
        #[cfg(not(target_os = "tvos"))]
        unsafe {
            prism_native::apple::drops::set_target(
                view.ui() as *const UIView as *const std::ffi::c_void,
                view.id(),
                types,
            )
        };
        #[cfg(target_os = "tvos")]
        let _ = (view, types);
    }

    fn set_key_events(&mut self, view: &View, local: bool, global: bool) {
        crate::interaction::set_keys(view.id(), local, global);
        if let View::Container(v) = view {
            crate::interaction::set_view_keys(v, local);
        }
    }

    /// Text field selections (#86).
    fn set_selection(&mut self, view: &View, start: usize, end: usize) {
        match view {
            View::Input(f) => crate::forms::set_selection(f, start, end),
            #[cfg(not(target_os = "tvos"))]
            View::TextArea(t) => crate::forms::set_selection(&t.view, start, end),
            _ => {}
        }
    }

    fn set_select_events(&mut self, view: &View, on: bool) {
        crate::forms::set_select_events(view.id(), on);
    }

    /// Other people's carets (#240); tvOS edits in its full-screen keyboard.
    #[cfg(not(target_os = "tvos"))]
    fn set_remote_carets(&mut self, view: &View, carets: &[prism_core::RemoteCaret]) {
        match view {
            View::Input(f) => crate::carets::set(self.mtm, view.id(), f, carets),
            View::TextArea(t) => crate::carets::set(self.mtm, view.id(), &t.view, carets),
            _ => {}
        }
    }

    /// A selection group follows an iPad pointer's drags across its fields
    /// (#231).
    fn set_selection_group(&mut self, view: &View, on: bool) {
        #[cfg(not(target_os = "tvos"))]
        crate::across::set_group(view.ui(), on, &self.target);
        #[cfg(target_os = "tvos")]
        let _ = (view, on);
    }

    /// A text field's part of a selection across fields (#231).
    #[cfg(not(target_os = "tvos"))]
    fn set_selection_highlight(&mut self, view: &View, start: usize, end: usize) {
        match view {
            View::TextArea(t) => crate::edit::highlight(&t.view, start, end),
            View::Input(f) => crate::edit::highlight_field(f, start, end),
            _ => {}
        }
    }

    /// Rich inputs edit their runs as attributes and attachments (#115,
    /// `crate::rich`); tvOS has no text area.
    fn rich_input(&self) -> bool {
        cfg!(not(target_os = "tvos"))
    }

    #[cfg(not(target_os = "tvos"))]
    fn set_rich_value(&mut self, view: &View, runs: &str) {
        if let View::TextArea(t) = view {
            crate::rich::set_runs(&t.view, runs);
        }
    }

    /// A rich input's editing hooks (#217, `crate::edit`): its text area.
    /// tvOS has neither text areas nor a pasteboard.
    #[cfg(not(target_os = "tvos"))]
    fn set_edit_events(&mut self, view: &View, mask: u8) {
        if let View::TextArea(t) = view {
            crate::edit::set_mask(&t.view, mask);
        }
    }

    #[cfg(not(target_os = "tvos"))]
    fn write_clipboard(&mut self, _view: &View, content: &prism_core::ClipboardContent) {
        crate::edit::write_clipboard(content);
    }

    #[cfg(not(target_os = "tvos"))]
    fn insert_text(&mut self, view: &View, text: &str) {
        if let View::TextArea(t) = view {
            crate::edit::insert_text(&t.view, text);
        }
    }

    fn focus(&mut self, view: &View) {
        if let View::Input(_) = view {
            crate::forms::app_focuses(view.id());
        }
        let target: Retained<UIView> = match view {
            #[cfg(not(target_os = "tvos"))]
            View::TextArea(t) => Retained::into_super(Retained::into_super(t.view.clone())),
            other => other.ui().retain(),
        };
        crate::interaction::focus(target);
    }

    fn scroll_into_view(&mut self, view: &View) {
        scroll_into_view(view.ui());
    }

    /// A text field's `autocomplete` (#203): its content type, so the
    /// keyboard's suggestions offer passkeys, passwords or codes.
    fn set_attributes(&mut self, view: &View, attributes: &[(String, String)]) {
        let View::Input(field) = view else {
            return;
        };
        let value = attributes
            .iter()
            .find(|(n, _)| n == "autocomplete")
            .map(|(_, v)| v.to_ascii_lowercase());
        let kind = value.as_deref().and_then(text_content_type);
        objc2_ui_kit::UITextInputTraits::setTextContentType(&**field, kind);
    }

    /// `cx.scroll_by` / `cx.scroll_to` (#193): the content offset, kept
    /// within the content and its insets.
    fn scroll_to(&mut self, view: &View, x: f32, y: f32, relative: bool, animated: bool) {
        let View::Scroll(s) = view else {
            return;
        };
        let inset = s.adjustedContentInset();
        let size = s.contentSize();
        let b = s.bounds().size;
        let now = s.contentOffset();
        let (tx, ty) = if relative {
            (now.x + x as f64, now.y + y as f64)
        } else {
            (x as f64 - inset.left, y as f64 - inset.top)
        };
        let clamp = |v: f64, min: f64, max: f64| {
            if v.is_nan() {
                min
            } else {
                v.clamp(min, max.max(min))
            }
        };
        let to = CGPoint::new(
            clamp(tx, -inset.left, size.width - b.width + inset.right),
            clamp(ty, -inset.top, size.height - b.height + inset.bottom),
        );
        s.setContentOffset_animated(to, animated);
    }

    /// Every field of the window resigns (the keyboard hides; sheets and
    /// overlays are in the same window).
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

    fn dismiss_keyboard(&mut self) {
        match self.root.window() {
            Some(w) => w.endEditing(true),
            None => self.root.endEditing(true),
        };
    }

    /// The field's `inputAccessoryView` (#242); visionOS and tvOS have no
    /// keyboard bar (left out of their trees).
    fn set_keyboard_bar(&mut self, field: &View, bar: Option<&View>) {
        #[cfg(target_os = "ios")]
        {
            let bar = match bar {
                Some(View::Overlay(o)) => Some(o.keyboard.clone()),
                _ => None,
            };
            let view: Retained<UIView> = match field {
                View::Input(f) => Retained::into_super(Retained::into_super(f.clone())),
                View::TextArea(t) => Retained::into_super(Retained::into_super(t.view.clone())),
                _ => return,
            };
            crate::interaction::set_keyboard_bar(field.id(), &view, bar.as_deref());
        }
        #[cfg(not(target_os = "ios"))]
        let _ = (field, bar);
    }

    fn set_layers(&mut self, layers: &[View]) {
        self.layers = layers.iter().map(|l| l.ui().retain()).collect();
        for l in &self.layers {
            crate::interaction::set_pass_through(l, true);
        }
        self.attach_layers();
    }

    /// Above a root tab bar (at the bottom on iPhone, the top on iPad).
    fn layer_insets(&mut self, safe_area: Insets) -> Insets {
        let Some(tabs) = &self.root_tabs else {
            return safe_area;
        };
        let bar: Retained<UIView> = Retained::into_super(tabs.tabBar());
        if bar.isHidden() {
            return safe_area;
        }
        let f = bar.convertRect_toView(bar.bounds(), Some(&self.root));
        let height = self.root.bounds().size.height;
        let mut i = safe_area;
        if f.origin.y > height / 2. {
            i.bottom = i.bottom.max((height - f.origin.y) as f32);
        } else {
            i.top = i.top.max((f.origin.y + f.size.height) as f32);
        }
        i
    }

    fn overlay_viewport(&mut self, overlay: &View, _content: &View) -> Viewport {
        let window = self.root.bounds().size;
        match overlay {
            View::Overlay(o) => o.viewport(window),
            _ => Viewport::default(),
        }
    }

    fn screen_viewport(&mut self, stack: &View, screen: &View) -> Viewport {
        // A tab hosts a navigation stack, which handles the safe area itself.
        // Tabs and sheets host a navigation stack, which handles the safe area.
        let own_insets = !matches!(screen, View::Tab(_) | View::Sheet(_));
        let host = self
            .screens
            .get(&screen.id())
            .and_then(|vc| vc.view())
            .unwrap_or_else(|| stack.ui().retain());
        let b = host.bounds();
        let i = if own_insets {
            host.safeAreaInsets()
        } else {
            objc2_ui_kit::UIEdgeInsets {
                top: 0.,
                left: 0.,
                bottom: 0.,
                right: 0.,
            }
        };
        Viewport {
            x: 0.,
            y: 0.,
            width: b.size.width as f32,
            height: b.size.height as f32,
            insets: Insets {
                top: i.top as f32,
                right: i.right as f32,
                bottom: i.bottom as f32,
                left: i.left as f32,
            },
        }
    }
}

/// Starts UIKit. Never returns.
pub fn run(config: AppConfig, init: AppInit) {
    let mtm = MainThreadMarker::new().expect("prism-ios: run() must be called on the main thread");
    let _ = APP.set((config, init));
    // Register the classes UIKit instantiates by name (Info.plist, UIApplicationMain).
    let _ = (AppDelegate::class(), SceneDelegate::class());
    let delegate = NSString::from_str("PrismAppDelegate");
    UIApplication::main(None, Some(&delegate), mtm);
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
    poll_headless();
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

/// App data: the App Group container when the app has one (widgets can read
/// it), else Application Support.
fn data_dir() -> std::path::PathBuf {
    let bundle = objc2_foundation::NSBundle::mainBundle();
    let group = bundle
        .objectForInfoDictionaryKey(&NSString::from_str("PrismAppGroup"))
        .and_then(|g| g.downcast::<NSString>().ok());
    if let Some(group) = group
        && let Some(url) = objc2_foundation::NSFileManager::defaultManager()
            .containerURLForSecurityApplicationGroupIdentifier(&group)
        && let Some(path) = url.path()
    {
        return std::path::PathBuf::from(path.to_string()).join("prism-data");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join("Library/Application Support/prism-data")
}

/// The context menu of the view hosting `interaction`, if it has one.
fn menu_configuration(
    mtm: MainThreadMarker,
    interaction: &objc2_ui_kit::UIContextMenuInteraction,
) -> Option<Retained<objc2_ui_kit::UIContextMenuConfiguration>> {
    let id = objc2_ui_kit::UIInteraction::view(interaction)?.tag() as NodeId;
    let mut spec = None;
    with_state(|s| spec = s.renderer.widgets.menus.get(&id).cloned());
    let spec = spec?;
    let provider = block2::RcBlock::new(
        move |_: std::ptr::NonNull<NSArray<objc2_ui_kit::UIMenuElement>>| {
            Retained::autorelease_return(crate::menus::build_menu(mtm, &spec, dispatch))
        },
    );
    Some(unsafe {
        objc2_ui_kit::UIContextMenuConfiguration::configurationWithIdentifier_previewProvider_actionProvider(
            None,
            std::ptr::null_mut(),
            &*provider as *const _ as *mut _,
            mtm,
        )
    })
}

/// Assets: those compiled into the binary, the bundle's `assets/` folder,
/// then the dev directory; downloads are cached in Library/Caches.
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

/// Locale, Dynamic Type and Reduce Motion (`PRISM_TEXT_SCALE` overrides the
/// text size, e.g. for screenshots).
fn ios_environment() -> prism_core::Environment {
    let scale = objc2_ui_kit::UIFontMetrics::defaultMetrics().scaledValueForValue(17.) / 17.;
    prism_core::Environment {
        locale: prism_native::apple::system_locale(),
        text_scale: std::env::var("PRISM_TEXT_SCALE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(scale as f32),
        reduce_motion: objc2_ui_kit::UIAccessibilityIsReduceMotionEnabled(),
        utc_offset: prism_native::apple::utc_offset(),
    }
}

/// Settings changes and keyboard moves.
fn observe_system(mtm: MainThreadMarker) {
    let target = Target::new(mtm);
    let center = objc2_foundation::NSNotificationCenter::defaultCenter();
    unsafe {
        for name in [
            objc2_ui_kit::UIContentSizeCategoryDidChangeNotification,
            objc2_ui_kit::UIAccessibilityReduceMotionStatusDidChangeNotification,
            objc2_foundation::NSCurrentLocaleDidChangeNotification,
        ] {
            center.addObserver_selector_name_object(
                &target,
                sel!(onEnvironment:),
                Some(name),
                None,
            );
        }
        // tvOS: the keyboard is a full-screen page, nothing to make room for.
        #[cfg(not(target_os = "tvos"))]
        for name in [
            objc2_ui_kit::UIKeyboardWillChangeFrameNotification,
            objc2_ui_kit::UIKeyboardWillHideNotification,
        ] {
            center.addObserver_selector_name_object(&target, sel!(onKeyboard:), Some(name), None);
        }
    }
    // Observers are weak: keep the target alive for the app's lifetime.
    std::mem::forget(target);
}

/// Places a view by its bounds and center: `frame` is undefined while a
/// transform is set. A scroll view's bounds origin (its offset) is kept.
fn place(v: &UIView, x: f32, y: f32, w: f32, h: f32) {
    let origin = v.bounds().origin;
    v.setBounds(CGRect::new(origin, CGSize::new(w as f64, h as f64)));
    v.setCenter(CGPoint::new((x + w / 2.) as f64, (y + h / 2.) as f64));
}

/// Runs `f` in a UIKit animation shaped like `t`.
fn animate(mtm: MainThreadMarker, t: prism_core::Transition, f: impl Fn() + 'static) {
    animate_then(mtm, t, f, || {});
}

/// [`animate`], then `done`.
fn animate_then(
    mtm: MainThreadMarker,
    t: prism_core::Transition,
    f: impl Fn() + 'static,
    done: impl Fn() + 'static,
) {
    use objc2_ui_kit::UIViewAnimationOptions as O;
    let block = block2::RcBlock::new(f);
    let completion = block2::RcBlock::new(move |_: objc2::runtime::Bool| done());
    let (duration, delay) = (t.duration as f64, t.delay as f64);
    let options = O::AllowUserInteraction | O::BeginFromCurrentState;
    let curve = match t.curve {
        prism_core::Curve::Spring => {
            return UIView::animateWithSpringDuration_bounce_initialSpringVelocity_delay_options_animations_completion(
                duration, 0.25, 0., delay, options, &block, Some(&completion), mtm,
            );
        }
        prism_core::Curve::Linear => O::CurveLinear,
        prism_core::Curve::EaseInOut => O::CurveEaseInOut,
        prism_core::Curve::EaseIn => O::CurveEaseIn,
        prism_core::Curve::EaseOut => O::CurveEaseOut,
    };
    UIView::animateWithDuration_delay_options_animations_completion(
        duration,
        delay,
        options | curve,
        &block,
        Some(&completion),
        mtm,
    );
}

/// Raises the bottom of the safe area of each visible screen above the
/// keyboard, animated like it. Tab bars stay put under the keyboard.
#[cfg(target_os = "tvos")]
fn keyboard_changed(_: &objc2_foundation::NSNotification) {}

#[cfg(not(target_os = "tvos"))]
fn keyboard_changed(n: &objc2_foundation::NSNotification) {
    let hiding = &*n.name() == unsafe { objc2_ui_kit::UIKeyboardWillHideNotification };
    let info = n.userInfo();
    let get = |key: &NSString| info.as_ref().and_then(|i| i.objectForKey(key));
    let duration: f64 = get(unsafe { objc2_ui_kit::UIKeyboardAnimationDurationUserInfoKey })
        .map_or(0.25, |d| unsafe { msg_send![&*d, doubleValue] });
    let keyboard: Option<CGRect> = get(unsafe { objc2_ui_kit::UIKeyboardFrameEndUserInfoKey })
        .map(|v| unsafe { msg_send![&*v, CGRectValue] });
    keyboard_moved(hiding, keyboard.map(|k| k.origin.y), duration);
}

/// The keyboard's new top edge (window coordinates), if known.
#[cfg(not(target_os = "tvos"))]
fn keyboard_moved(hiding: bool, keyboard: Option<f64>, duration: f64) {
    // Screens in a stack, or the root controller when there is none; each
    // with whether it is on screen. Screens off screen, and the root
    // controller once screens show, lose a keyboard inset they kept (#29:
    // the root swapped while the keyboard was up).
    let mut controllers: Vec<(Retained<UIViewController>, bool)> = Vec::new();
    let mut ran = false;
    with_state(|s| {
        ran = true;
        let w = &s.renderer.widgets;
        controllers.extend(w.screens.values().map(|c| {
            let c: Retained<UIViewController> = Retained::into_super(c.clone());
            let shown = c.isViewLoaded() && c.view().is_some_and(|v| v.window().is_some());
            (c, shown)
        }));
        let none_shown = !controllers.iter().any(|(_, shown)| *shown);
        controllers.push((Retained::into_super(w.controller.clone()), none_shown));
    });
    if !ran {
        // Posted while Prism changes views (the focused field went away
        // with the root it was in): once that is done.
        dispatch2::DispatchQueue::main()
            .exec_async(move || keyboard_moved(hiding, keyboard, duration));
        return;
    }
    let Some(mtm) = controllers.first().map(|(c, _)| c.mtm()) else {
        return;
    };
    let mut changes = Vec::new();
    let mut window = None;
    for (vc, shown) in controllers {
        let Some(view) = vc.view().filter(|_| vc.isViewLoaded()) else {
            continue;
        };
        let frame = view.convertRect_toView(view.bounds(), None);
        let overlap = match (hiding, keyboard) {
            (false, Some(top)) if shown => {
                (frame.origin.y + frame.size.height - top).clamp(0., frame.size.height)
            }
            _ => 0.,
        };
        let current = vc.additionalSafeAreaInsets().bottom;
        // Tab bar and home indicator already count in the safe area.
        let base = view.safeAreaInsets().bottom - current;
        let extra = (overlap - base).max(0.);
        if (extra - current).abs() >= 0.5 {
            changes.push((vc, extra));
        }
        window = window.or(view.window());
    }
    if !changes.is_empty() {
        let block = block2::RcBlock::new(move || {
            for (vc, extra) in &changes {
                vc.setAdditionalSafeAreaInsets(objc2_ui_kit::UIEdgeInsets {
                    top: 0.,
                    left: 0.,
                    bottom: *extra,
                    right: 0.,
                });
                if let Some(v) = vc.view() {
                    v.layoutIfNeeded();
                }
            }
        });
        UIView::animateWithDuration_delay_options_animations_completion(
            duration,
            0.,
            objc2_ui_kit::UIViewAnimationOptions::BeginFromCurrentState,
            &block,
            None,
            mtm,
        );
    }
    if !hiding && let Some(field) = window.as_deref().and_then(|w| first_responder(w)) {
        scroll_into_view(&field);
    }
}

#[cfg(not(target_os = "tvos"))]
fn first_responder(v: &UIView) -> Option<Retained<UIView>> {
    if v.isFirstResponder() {
        return Some(v.retain());
    }
    v.subviews().iter().find_map(|c| first_responder(&c))
}

/// Scrolls the nearest scroll view so `view` sits above the keyboard.
fn scroll_into_view(view: &UIView) {
    let mut parent = view.superview();
    while let Some(p) = parent {
        if let Ok(scroll) = p.clone().downcast::<UIScrollView>() {
            let r = scroll.convertRect_fromView(view.bounds(), Some(view));
            let padded = CGRect::new(
                CGPoint::new(r.origin.x, r.origin.y - 16.),
                CGSize::new(r.size.width, r.size.height + 32.),
            );
            scroll.scrollRectToVisible_animated(padded, true);
            return;
        }
        parent = p.superview();
    }
}

/// Applies a layout direction to a view and everything inside it.
fn set_semantic(v: &UIView, attr: objc2_ui_kit::UISemanticContentAttribute) {
    if v.semanticContentAttribute() != attr {
        v.setSemanticContentAttribute(attr);
        // Bars cache their layout (large titles, back buttons).
        v.setNeedsLayout();
    }
    for child in v.subviews().iter() {
        set_semantic(&child, attr);
    }
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

/// `[[UIView appearance] setSemanticContentAttribute:]`: views UIKit creates
/// later (large titles, bar buttons) follow the direction too. The proxy
/// forwards the message, so it is sent without objc2's selector check.
fn set_appearance_semantic(attr: objc2_ui_kit::UISemanticContentAttribute) {
    unsafe {
        let proxy: *mut AnyObject = msg_send![UIView::class(), appearance];
        if proxy.is_null() {
            return;
        }
        let send: unsafe extern "C-unwind" fn(*mut AnyObject, objc2::runtime::Sel, isize) =
            std::mem::transmute(objc2::ffi::objc_msgSend as *const ());
        send(proxy, sel!(setSemanticContentAttribute:), attr.0);
    }
}

// ---------------------------------------------------------------------------
// Background execution
// ---------------------------------------------------------------------------

/// The app without UI: iOS launched it in the background to run a task. The
/// window's runtime takes it over when a scene connects.
struct Headless {
    runtime: Runtime,
    services: crate::services::IosServices,
}

thread_local! {
    static HEADLESS: RefCell<Option<Headless>> = const { RefCell::new(None) };
    /// Running background tasks and who to tell when they finish.
    static BG_WAITING: RefCell<Vec<(u64, Completion)>> = const { RefCell::new(Vec::new()) };
    /// Extra time asked with `beginBackgroundTask` (finish_in_background).
    static UIKIT_TASK: std::cell::Cell<Option<objc2_ui_kit::UIBackgroundTaskIdentifier>> =
        const { std::cell::Cell::new(None) };
}

/// Who started a background task.
pub(crate) enum Completion {
    /// BGTaskScheduler (full identifier).
    System(String),
    /// A silent push: its fetch completion handler.
    Push(block2::RcBlock<dyn Fn(objc2_ui_kit::UIBackgroundFetchResult)>),
    /// A widget button's intent, waiting in the bridge under this token.
    WidgetAction(u64),
    /// Development hook.
    None,
}

fn make_runtime(
    init: &prism_core::AppInit,
    path: String,
    services: &crate::services::IosServices,
) -> Runtime {
    let mut runtime = Runtime::with_options(
        init,
        prism_core::RuntimeOptions {
            path,
            capabilities: prism_core::PlatformServices::capabilities(services),
            storage: Some(Box::new(prism_core::FileStorage::new(data_dir()))),
            wake_ui: Some(wake_ui()),
            environment: ios_environment(),
            formatter: Some(Box::new(prism_native::apple::FoundationFormatter)),
            cloud_storage: Some(cloud_storage(&data_dir())),
            // Set from the window at the first layout.
            viewport: None,
            platform: None,
            data_dir: Some(data_dir()),
            shared_data_dir: None,
            // Reported from the window at the first layout too.
            device_info: None,
        },
    );
    // Configurable widgets: the instances the extension saw on screen.
    runtime
        .app
        .set_widget_configurations_source(prism_native::apple::widget_configurations);
    runtime
}

/// `PRISM_BACKGROUND_INTERVAL=<seconds>` shortens every background task's
/// interval (development).
pub(crate) fn background_interval(every: std::time::Duration) -> std::time::Duration {
    std::env::var("PRISM_BACKGROUND_INTERVAL")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .map_or(every, std::time::Duration::from_secs_f64)
}

/// Runs the app's code for a headless launch, if there is one.
fn poll_headless() {
    HEADLESS.with(|h| {
        let Ok(mut guard) = h.try_borrow_mut() else {
            return;
        };
        let Some(h) = guard.as_mut() else { return };
        h.runtime.poll_headless();
        for request in h.runtime.take_requests() {
            prism_core::PlatformServices::handle(&mut h.services, request);
        }
        schedule_timer(h.runtime.next_timer());
    });
}

/// The app without a window: iOS launched it for a background task, or only
/// CarPlay shows it.
fn new_headless() -> Headless {
    let mtm = MainThreadMarker::new().expect("main thread");
    let (_, init) = APP.get().expect("prism-ios: run() was not called");
    let services = crate::services::IosServices::new(false, UIViewController::new(mtm));
    let runtime = make_runtime(init, "/".into(), &services);
    Headless { runtime, services }
}

/// Runs `f` on the app's runtime: the window's, or one without a window
/// (CarPlay can launch the app alone).
pub(crate) fn with_runtime(f: impl FnOnce(&mut Runtime) -> Vec<prism_core::Patch>) {
    let mut f = Some(f);
    with_state(|s| {
        if let Some(f) = f.take() {
            let patches = f(&mut s.runtime);
            s.commit(patches);
        }
    });
    if let Some(f) = f {
        HEADLESS.with(|h| {
            let mut h = h.borrow_mut();
            let headless = h.get_or_insert_with(new_headless);
            let _ = f(&mut headless.runtime);
        });
        poll_headless();
    }
}

/// Runs background task `id` in the window's runtime, or in a headless one
/// when iOS launched the app just for it.
pub(crate) fn run_background(id: &str, completion: Completion) {
    let mut started = None;
    let mut has_ui = false;
    with_state(|s| {
        has_ui = true;
        started = s.runtime.run_background_task(id);
        let patches = s.runtime.poll();
        s.commit(patches);
    });
    if !has_ui {
        HEADLESS.with(|h| {
            let mut h = h.borrow_mut();
            let headless = h.get_or_insert_with(new_headless);
            started = headless.runtime.run_background_task(id);
        });
        poll_headless();
    }
    eprintln!(
        "prism-ios: background task {id:?} started ({})",
        if has_ui { "app open" } else { "headless" }
    );
    match started {
        Some(task) => BG_WAITING.with(|w| w.borrow_mut().push((task, completion))),
        None => finish(&completion, false),
    }
    check_background();
}

/// A push token (or a registration error), in the window's runtime or a
/// headless one (the token can arrive before the scene connects).
pub(crate) fn push_registered(
    result: Result<prism_core::PushSubscription, prism_core::DeviceError>,
) {
    if let Err(e) = &result {
        eprintln!("prism-ios: push registration failed: {e}");
    }
    with_runtime(|rt| rt.push_registered(result));
}

/// A push received in the foreground or tapped (the tap can launch the app).
pub(crate) fn push_received(message: prism_core::PushMessage) {
    with_runtime(|rt| rt.push_received(message, false));
}

/// A silent push: runs the app's `on_background_push` job (headless when iOS
/// launched the app for it) and calls `done` when it finishes.
fn push_background(
    message: prism_core::PushMessage,
    done: block2::RcBlock<dyn Fn(objc2_ui_kit::UIBackgroundFetchResult)>,
) {
    let mut message = Some(message);
    let mut started = None;
    with_state(|s| {
        if let Some(m) = message.take() {
            started = s.runtime.run_background_push(m);
            let patches = s.runtime.poll();
            s.commit(patches);
        }
    });
    if let Some(m) = message {
        HEADLESS.with(|h| {
            let mut h = h.borrow_mut();
            let headless = h.get_or_insert_with(new_headless);
            started = headless.runtime.run_background_push(m);
        });
        poll_headless();
    }
    let completion = Completion::Push(done);
    match started {
        Some(task) => BG_WAITING.with(|w| w.borrow_mut().push((task, completion))),
        None => finish(&completion, true),
    }
    check_background();
}

/// The system's time ran out: stop the task and report failure.
pub(crate) fn expire_background(full_id: &str) {
    let expired: Vec<u64> = BG_WAITING.with(|w| {
        let mut w = w.borrow_mut();
        let ids = w
            .iter()
            .filter(|(_, c)| matches!(c, Completion::System(f) if f == full_id))
            .map(|(t, _)| *t)
            .collect();
        w.retain(|(_, c)| !matches!(c, Completion::System(f) if f == full_id));
        ids
    });
    for task in expired {
        with_state(|s| s.runtime.cancel_task(task));
        HEADLESS.with(|h| {
            if let Some(h) = h.borrow_mut().as_mut() {
                h.runtime.cancel_task(task);
            }
        });
    }
    crate::services::complete_background_task(full_id, false);
}

fn finish(completion: &Completion, success: bool) {
    match completion {
        Completion::System(full) => crate::services::complete_background_task(full, success),
        Completion::Push(done) => done.call((if success {
            objc2_ui_kit::UIBackgroundFetchResult::NewData
        } else {
            objc2_ui_kit::UIBackgroundFetchResult::Failed
        },)),
        Completion::WidgetAction(token) => crate::services::widget_action_done(*token),
        Completion::None => {}
    }
}

/// A widget button was tapped: runs the app's `on_widget_action` job in the
/// window's runtime, or in a headless one when iOS launched the app for it,
/// and tells `completion` when it is done (the widgets are reloaded then).
pub(crate) fn run_widget_action(action: prism_core::WidgetAction, completion: Completion) {
    eprintln!(
        "prism-ios: widget action {:?} ({:?})",
        action.action, action.kind
    );
    let mut action = Some(action);
    let mut started = None;
    with_state(|s| {
        if let Some(a) = action.take() {
            started = Some(s.runtime.run_widget_action(a));
            let patches = s.runtime.poll();
            s.commit(patches);
        }
    });
    if let Some(a) = action {
        HEADLESS.with(|h| {
            let mut h = h.borrow_mut();
            let headless = h.get_or_insert_with(new_headless);
            started = Some(headless.runtime.run_widget_action(a));
        });
        poll_headless();
    }
    match started {
        Some(task) => BG_WAITING.with(|w| w.borrow_mut().push((task, completion))),
        None => finish(&completion, false),
    }
    check_background();
}

/// Runs the actions the widget extension queued (it ran their intents
/// itself, without the app's code).
fn run_queued_widget_actions() {
    for action in crate::services::take_queued_widget_actions() {
        run_widget_action(action, Completion::None);
    }
}

fn is_running(task: u64) -> bool {
    let mut running = false;
    with_state(|s| running = s.runtime.is_task_running(task));
    HEADLESS.with(|h| {
        if let Ok(h) = h.try_borrow()
            && let Some(h) = h.as_ref()
        {
            running |= h.runtime.is_task_running(task);
        }
    });
    running
}

/// Reports finished background tasks and releases unneeded background time.
fn check_background() {
    let done: Vec<(u64, Completion)> = BG_WAITING.with(|w| {
        let mut w = w.borrow_mut();
        let (done, running): (Vec<_>, Vec<_>) = w.drain(..).partition(|(t, _)| !is_running(*t));
        *w = running;
        done
    });
    for (_, completion) in done {
        eprintln!("prism-ios: background task finished");
        finish(&completion, true);
    }
    if let Some(mtm) = MainThreadMarker::new() {
        update_background_time(mtm);
    }
}

/// Holds `beginBackgroundTask` while a task asked to finish in the background.
fn update_background_time(mtm: MainThreadMarker) {
    let mut wants = false;
    with_state(|s| {
        wants = s.runtime.wants_background_time()
            && s.runtime.app.lifecycle() == prism_core::Lifecycle::Background
    });
    let app = UIApplication::sharedApplication(mtm);
    match (wants, UIKIT_TASK.with(|t| t.get())) {
        (true, None) => {
            let expired = block2::RcBlock::new(|| {
                if let Some(mtm) = MainThreadMarker::new()
                    && let Some(id) = UIKIT_TASK.with(|t| t.take())
                {
                    UIApplication::sharedApplication(mtm).endBackgroundTask(id);
                }
            });
            let id = app.beginBackgroundTaskWithName_expirationHandler(
                Some(&NSString::from_str("prism.finish_in_background")),
                Some(&expired),
            );
            eprintln!("prism-ios: keeping the app alive to finish a task");
            UIKIT_TASK.with(|t| t.set(Some(id)));
        }
        (false, Some(id)) => {
            UIKIT_TASK.with(|t| t.set(None));
            app.endBackgroundTask(id);
            eprintln!("prism-ios: background work done");
        }
        _ => {}
    }
}

/// App lifecycle notifications (registered at launch, before any scene).
fn observe_lifecycle(mtm: MainThreadMarker) {
    let target = Target::new(mtm);
    let center = objc2_foundation::NSNotificationCenter::defaultCenter();
    unsafe {
        use objc2_ui_kit::*;
        for name in [
            UIApplicationDidBecomeActiveNotification,
            UIApplicationWillResignActiveNotification,
            UIApplicationDidEnterBackgroundNotification,
            UIApplicationWillEnterForegroundNotification,
        ] {
            center.addObserver_selector_name_object(&target, sel!(onLifecycle:), Some(name), None);
        }
        center.addObserver_selector_name_object(
            &target,
            sel!(onCloudChange:),
            Some(objc2_foundation::NSUbiquitousKeyValueStoreDidChangeExternallyNotification),
            None,
        );
    }
    std::mem::forget(target);
}

/// Answers from the device bridge (on the main thread). An answer given
/// while the app is busy (synchronously, during the call) is handled on the
/// next turn of the main loop.
/// A foldable opened, closed or bent: report `cx.device_info()` again (on
/// the next turn: UIKit may call while views are being changed).
fn hinge_changed() {
    dispatch2::DispatchQueue::main().exec_async(|| {
        with_state(|s| {
            s.device_size = None;
            s.relayout();
        })
    });
}

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

/// Whether `view` is a text area (tvOS has none: they are text inputs).
fn is_text_area(view: &View) -> bool {
    #[cfg(not(target_os = "tvos"))]
    if let View::TextArea(_) = view {
        return true;
    }
    let _ = view;
    false
}

/// `cx.set_title` (#154): the title of the app's scenes (shown by iPadOS in
/// its multitasking switcher and window menus; `None`: the app's name).
pub(crate) fn set_scene_title(title: Option<&str>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let title = title.map(NSString::from_str);
    for scene in UIApplication::sharedApplication(mtm)
        .connectedScenes()
        .iter()
    {
        scene.setTitle(title.as_deref());
    }
}
