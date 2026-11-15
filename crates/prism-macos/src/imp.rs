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
    NSDatePicker, NSDatePickerElementFlags, NSDatePickerStyle, NSEvent, NSFont, NSFontWeightBold,
    NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold, NSImage,
    NSImageSymbolConfiguration, NSImageView, NSMenu, NSMenuItem, NSPopUpButton,
    NSProgressIndicator, NSProgressIndicatorStyle, NSResponder, NSScrollView, NSSegmentedControl,
    NSSlider, NSSwitch, NSText, NSTextAlignment, NSTextDelegate, NSTextField, NSTextFieldDelegate,
    NSTextView, NSTextViewDelegate, NSUserInterfaceItemIdentification, NSView, NSWindow,
    NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use prism_core::controls::{DateConfig, DateMode, InputConfig, PickerConfig, SliderConfig};
use prism_core::{
    Accessibility, AppConfig, AppInit, Appearance, Backend, Color, FontWeight, NodeId, NodeKind,
    Role, Runtime, Style, TextAlign, UiEvent,
};
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

pub(crate) fn dispatch(event: UiEvent) {
    with_state(|s| s.dispatch(event));
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
        let patches = self.runtime.dispatch(event);
        self.commit(patches);
    }

    /// Applies patches, lays out, and hands platform requests to the OS.
    fn commit(&mut self, patches: Vec<prism_core::Patch>) {
        if !patches.is_empty() {
            self.renderer.apply(&patches);
            self.relayout();
        }
        for request in self.runtime.take_requests() {
            prism_core::PlatformServices::handle(&mut self.services, request);
        }
        schedule_timer(self.runtime.next_timer());
    }

    fn relayout(&mut self) {
        let size = self.renderer.widgets.root.frame().size;
        // Responsive layouts: tell the app its window size (re-renders when
        // it crosses a breakpoint: rotation, split view, resize).
        let (w, h) = (size.width as f32, size.height as f32);
        if w > 0. && (w, h) != self.runtime.app.viewport() {
            let patches = self.runtime.set_viewport(w, h);
            if !patches.is_empty() {
                self.renderer.apply(&patches);
            }
        }
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

        self.renderer
            .layout(size.width as f32, size.height as f32, Insets::default());
    }
}

// ---------------------------------------------------------------------------
// PrismView: a flipped NSView that paints background/border and handles clicks.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct ViewIvars {
    id: Cell<NodeId>,
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
    sheet_root: Cell<bool>,
    /// Picture content, drawn aspect-fill (or aspect-fit).
    image: RefCell<Option<Retained<NSImage>>>,
    fit: Cell<bool>,
    /// Trackpad swipes (left, right) reported to Prism.
    swipe: Cell<(bool, bool)>,
    /// What a shape draws, in points, and the layers drawing it.
    shape: RefCell<Vec<prism_core::Path>>,
    shape_layers: RefCell<Vec<Retained<objc2_quartz_core::CAShapeLayer>>>,
    /// Background and border when only some corners are rounded (a path:
    /// `maskedCorners` is skipped by `renderInContext`, so snapshots and
    /// the screen would differ).
    box_layer: RefCell<Option<Retained<objc2_quartz_core::CAShapeLayer>>>,
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

        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            let Some(layer) = self.layer() else { return };
            let iv = self.ivars();
            // Called within the view's appearance: dynamic colors resolve to
            // light or dark here.
            let cg = |c: &NSColor| c.CGColor();
            layer.setBackgroundColor(iv.background.borrow().as_deref().map(cg).as_deref());
            // A radius over half the size draws garbage: clamp (pills).
            let b = self.bounds().size;
            let radius = iv.radius.get().min(b.width.min(b.height) / 2.).max(0.);
            let bw = if iv.border_color.borrow().is_some() { iv.border.get() } else { 0. };
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
                layer.setContentsGravity(if iv.fit.get() {
                    objc2_quartz_core::kCAGravityResizeAspect
                } else {
                    objc2_quartz_core::kCAGravityResizeAspectFill
                });
            }
            // Pictures are cropped to their rounded frame.
            layer.setMasksToBounds(image.is_some());
            drop(image);
            self.update_shape_layers(&layer);
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

        /// VoiceOver "press" on a clickable view.
        #[unsafe(method(accessibilityPerformPress))]
        fn accessibility_perform_press(&self) -> objc2::runtime::Bool {
            if !self.ivars().clickable.get() {
                return objc2::runtime::Bool::NO;
            }
            dispatch(UiEvent::Click(self.ivars().id.get()));
            objc2::runtime::Bool::YES
        }

        // Full keyboard access: clickable views take focus, show a focus
        // ring, and activate with Space or Return.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            self.ivars().clickable.get() || self.ivars().sheet_root.get()
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
            if self.ivars().sheet_root.get() {
                dispatch(UiEvent::Back(self.ivars().id.get(), 0));
            } else {
                unsafe { msg_send![super(self), cancelOperation: sender] }
            }
        }

        /// Like native buttons: reachable with Tab only when the user enabled
        /// Keyboard Navigation (System Settings › Keyboard).
        #[unsafe(method(canBecomeKeyView))]
        fn can_become_key_view(&self) -> bool {
            self.ivars().clickable.get()
                && NSApplication::sharedApplication(self.mtm()).isFullKeyboardAccessEnabled()
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let code = event.keyCode();
            if self.ivars().clickable.get() && (code == 49 || code == 36 || code == 76) {
                dispatch(UiEvent::Click(self.ivars().id.get()));
            } else {
                unsafe { msg_send![super(self), keyDown: event] }
            }
        }

        #[unsafe(method(focusRingMaskBounds))]
        fn focus_ring_mask_bounds(&self) -> NSRect {
            self.bounds()
        }

        #[unsafe(method(drawFocusRingMask))]
        fn draw_focus_ring_mask(&self) {
            let r = self.ivars().radius.get();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), r, r).fill();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if !self.ivars().clickable.get() {
                unsafe { msg_send![super(self), mouseDown: event] }
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            if !self.ivars().clickable.get() {
                return unsafe { msg_send![super(self), mouseUp: event] };
            }
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            let b = self.bounds();
            if p.x >= 0. && p.y >= 0. && p.x <= b.size.width && p.y <= b.size.height {
                dispatch(UiEvent::Click(self.ivars().id.get()));
            }
        }
    }
);

impl PrismView {
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

    fn new(mtm: MainThreadMarker, id: NodeId) -> Retained<Self> {
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
    id: Cell<NodeId>,
    placeholder: RefCell<String>,
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
        let pad = unsafe { self.textContainer() }.map_or(5., |c| c.lineFragmentPadding());
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
            dispatch(UiEvent::Change(sender.tag() as NodeId, secs));
        }

        /// Any Prism menu item (context menus, tray, app menus).
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
            let b = clip.bounds();
            dispatch(UiEvent::Scroll(doc.ivars().id.get(), b.origin.y as f32, b.size.height as f32));
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
            dispatch(UiEvent::Input(field.tag() as NodeId, field.stringValue().to_string()));
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
        }
    }

    unsafe impl NSTextViewDelegate for Target {}

    unsafe impl NSWindowDelegate for Target {
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _n: &NSNotification) {
            with_state(State::relayout);
        }

        #[unsafe(method(windowDidChangeScreen:))]
        fn window_did_change_screen(&self, _n: &NSNotification) {
            with_state(|s| {
                s.device_size = None;
                s.relayout();
            });
        }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _n: &NSNotification) {
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }
    }
);

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
/// top screen. Other screens stay alive, detached, keeping their state.
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
}

/// A sheet window presenting a modal stack over the main window.
pub struct MacSheet {
    window: Retained<NSWindow>,
    content: Retained<PrismView>,
    presented: Cell<bool>,
}

/// A tab bar: a segmented control above the selected tab.
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
    scroll: Retained<NSScrollView>,
    document: Retained<PrismView>,
}

/// A text input. Secure entry is a different class (`NSSecureTextField`,
/// whose field editor only reports edits to it), so the field sits in a
/// host view that can swap it when `secure` changes.
pub struct MacInput {
    id: NodeId,
    host: Retained<PrismView>,
    field: RefCell<Retained<NSTextField>>,
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
    Label(Retained<NSTextField>),
    Button(Retained<NSButton>),
    Input(Rc<MacInput>),
    Slider(Retained<PrismSlider>),
    Picker(Rc<MacPicker>),
    DatePicker(Retained<NSDatePicker>),
    TextArea(Rc<MacTextArea>),
    Spinner(Retained<NSProgressIndicator>),
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
    icons: HashMap<NodeId, String>,
    /// Countdown labels and their end time (Unix seconds).
    timers: HashMap<NodeId, (Retained<NSTextField>, f64)>,
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
}

fn ca_equal(a: objc2_quartz_core::CATransform3D, b: objc2_quartz_core::CATransform3D) -> bool {
    let f = |t: objc2_quartz_core::CATransform3D| [t.m11, t.m12, t.m21, t.m22, t.m41, t.m42];
    f(a) == f(b)
}

impl AppKit {
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
            eprintln!("prism-macos: could not load image {src:?}");
            return;
        };
        for (_, view) in waiting {
            *view.ivars().image.borrow_mut() = Some(img.clone());
            view.setNeedsDisplay(true);
        }
        self.image_cache.insert(src.to_string(), img);
    }

    fn tick(&self) {
        for (label, end) in self.timers.values() {
            let text = prism_core::format_remaining(*end);
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
        DateMode::Time => (
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
        storage.addLayoutManager(&layout);
        let tc = NSTextContainer::initWithSize(
            objc2::AllocAnyThread::alloc(),
            NSSize::new(0., f64::MAX / 4.),
        );
        tc.setWidthTracksTextView(true);
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
        text.setTextContainerInset(NSSize::new(7., 8.));
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
        f.setFrame(old.frame());
        unsafe { old.setDelegate(None) };
        input.host.replaceSubview_with(&old, &f);
        *input.field.borrow_mut() = f;
    }

    fn new_stack(&self, id: NodeId) -> MacStack {
        let mtm = self.mtm;
        let header = PrismView::new(mtm, 0);
        *header.ivars().background.borrow_mut() = Some(NSColor::windowBackgroundColor());
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
        }
    }

    fn new_tabs(&self, id: NodeId) -> MacTabs {
        let mtm = self.mtm;
        let bar = PrismView::new(mtm, 0);
        *bar.ivars().background.borrow_mut() = Some(NSColor::windowBackgroundColor());
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
        let mut subviews: Vec<&NSView> = vec![&bar.bar, &bar.separator];
        if let Some(t) = tabs.get(selected) {
            subviews.push(t.ns());
        }
        bar.container.setSubviews(&NSArray::from_slice(&subviews));
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

    /// Shows the top screen, its title, and Back when there is somewhere to go.
    fn refresh_stack(&self, stack: &MacStack) {
        let screens = stack.screens.borrow();
        let mut subviews: Vec<&NSView> = vec![&stack.header, &stack.separator];
        if let Some(top) = screens.last() {
            subviews.push(top.ns());
        }
        stack.container.setSubviews(&NSArray::from_slice(&subviews));
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
        if let Some(window) = stack.container.window()
            && title.length() > 0
        {
            window.setTitle(&title);
        }
    }
}

fn plain_color(c: Color) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(c.r as f64, c.g as f64, c.b as f64, c.a as f64)
}

/// Adaptive colors become dynamic `NSColor`s, resolved per appearance.
fn ns_color(c: Color) -> Retained<NSColor> {
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
    unsafe {
        match style.font_weight {
            FontWeight::Regular => NSFontWeightRegular,
            FontWeight::Medium => NSFontWeightMedium,
            FontWeight::Semibold => NSFontWeightSemibold,
            FontWeight::Bold => NSFontWeightBold,
        }
    }
}

fn font(style: &Style) -> Retained<NSFont> {
    let weight = unsafe {
        match style.font_weight {
            FontWeight::Regular => NSFontWeightRegular,
            FontWeight::Medium => NSFontWeightMedium,
            FontWeight::Semibold => NSFontWeightSemibold,
            FontWeight::Bold => NSFontWeightBold,
        }
    };
    NSFont::systemFontOfSize_weight(style.font_size as f64, weight)
}

fn alignment(a: TextAlign) -> NSTextAlignment {
    match a {
        TextAlign::Left => NSTextAlignment::Left,
        TextAlign::Center => NSTextAlignment::Center,
        TextAlign::Right => NSTextAlignment::Right,
    }
}

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
            used.max(3. * lm.defaultLineHeightForFont(&font))
        }
        _ => 54.,
    };
    NSSize::new(width, content + 2. * inset.height + 2. * border)
}

impl Widgets for AppKit {
    type View = View;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> View {
        let mtm = self.mtm;
        let empty = NSString::new();
        match kind {
            NodeKind::View => View::Container(PrismView::new(mtm, id)),
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
                window.setContentView(Some(&host));
                host.setSubviews(&NSArray::from_slice(&[&*content as &NSView]));
                View::Sheet(Rc::new(MacSheet {
                    window,
                    content,
                    presented: Cell::new(false),
                }))
            }
            NodeKind::Scroll => {
                let scroll = NSScrollView::new(mtm);
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
            NodeKind::Text => View::Label(NSTextField::wrappingLabelWithString(&empty, mtm)),
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
            NodeKind::TextInput => {
                let f = self.new_text_field(id, false);
                let host = PrismView::new(mtm, id);
                host.setSubviews(&NSArray::from_slice(&[&*f as &NSView]));
                View::Input(Rc::new(MacInput {
                    id,
                    host,
                    field: RefCell::new(f),
                }))
            }
        }
    }

    fn set_hidden(&mut self, view: &View, hidden: bool) {
        view.ns().setHidden(hidden);
    }

    fn set_style(&mut self, view: &View, style: &Style) {
        view.ns().setAlphaValue(style.opacity as f64);
        if !style.transform.is_identity() || self.transforms.contains_key(&view.id()) {
            self.transforms.insert(view.id(), style.transform);
            self.apply_transform(view);
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
                t.text.setFont(Some(&font(style)));
                t.text.setTextColor(Some(
                    &style.text_color.map_or_else(NSColor::labelColor, ns_color),
                ));
                t.text.setAlignment(alignment(style.text_align));
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
                let iv = v.ivars();
                *iv.background.borrow_mut() = style.background.map(ns_color);
                *iv.border_color.borrow_mut() = style.border_color.map(ns_color);
                iv.radius.set(style.corner_radius as f64);
                iv.corners.set(style.rounded_corners);
                iv.border.set(style.border_width as f64);
                v.setNeedsDisplay(true);
            }
            View::Label(_) | View::Input(_) => {
                let l = match view {
                    View::Input(i) => i.field(),
                    View::Label(l) => l.clone(),
                    _ => unreachable!(),
                };
                l.setFont(Some(&font(style)));
                l.setTextColor(Some(
                    &style.text_color.map_or_else(NSColor::labelColor, ns_color),
                ));
                l.setAlignment(alignment(style.text_align));
            }
            View::Button(b) => {
                b.setFont(Some(&font(style)));
                b.setBezelColor(style.background.map(ns_color).as_deref());
            }
        }
    }

    fn set_text(&mut self, view: &View, text: &str) {
        let s = NSString::from_str(text);
        match view {
            View::Label(l) | View::Timer(l) => l.setStringValue(&s),
            View::Picture(v) => self.load_picture(view.id(), v, text),
            View::Input(i) => {
                let f = i.field();
                // Don't reset the caret when the value already matches (controlled input echo).
                if f.stringValue().to_string() != text {
                    f.setStringValue(&s);
                }
            }
            View::Button(b) => b.setTitle(&s),
            View::TextArea(t) => {
                // Same text (controlled echo): keep the caret and selection.
                if t.text.string().to_string() != text {
                    t.text.setString(&s);
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
            _ => {}
        }
    }

    fn set_clickable(&mut self, view: &View, clickable: bool) {
        if let View::Container(v) = view {
            v.ivars().clickable.set(clickable);
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
                return;
            }
            _ => {}
        }
        let views: Vec<&NSView> = children.iter().map(View::ns).collect();
        parent.ns().setSubviews(&NSArray::from_slice(&views));
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

    fn destroy(&mut self, view: &View) {
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
            View::Sheet(sheet) => self.dismiss_sheet(sheet),
            View::Timer(_) => {
                self.timers.remove(&view.id());
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
            }
            View::Screen(_) => {
                self.titles.remove(&view.id());
            }
            _ => {}
        }
        view.ns().removeFromSuperview();
    }

    fn measure(&mut self, view: &View, max_width: Option<f32>) -> (f32, f32) {
        let size = match view {
            View::Label(l) => {
                let w = max_width.map_or(f64::MAX / 4., |w| w as f64);
                let bounds = NSRect::new(NSPoint::ZERO, NSSize::new(w, f64::MAX / 4.));
                l.cell()
                    .map_or(NSSize::ZERO, |c| c.cellSizeForBounds(bounds))
            }
            View::Input(i) => {
                let s = i.field().fittingSize();
                NSSize::new(s.width.max(120.), s.height)
            }
            View::Button(b) => b.fittingSize(),
            View::Toggle(t) => t.fittingSize(),
            View::Sheet(_) => NSSize::ZERO,
            View::Image(v) => v.fittingSize(),
            View::Timer(l) => l.fittingSize(),
            // Natural size of the loaded image, scaled to the available width.
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
            View::DatePicker(d) => d.fittingSize(),
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
            View::Container(_)
            | View::Screen(_)
            | View::Tab(_)
            | View::Stack(_)
            | View::Tabs(_)
            | View::Scroll(_) => NSSize::ZERO,
        };
        (size.width.ceil() as f32, size.height.ceil() as f32)
    }

    fn set_frame(&mut self, view: &View, x: f32, y: f32, w: f32, h: f32) {
        let rect =
            |x: f64, y: f64, w: f64, h: f64| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        view.ns()
            .setFrame(rect(x as f64, y as f64, w as f64, h as f64));
        if self.transforms.contains_key(&view.id()) {
            self.apply_transform(view);
        }
        if let View::Input(i) = view {
            i.field().setFrame(rect(0., 0., w as f64, h as f64));
        }
        if let View::TextArea(t) = view {
            let b = t.border();
            let (w, h) = (w as f64, h as f64);
            t.scroll
                .setFrame(rect(b, b, (w - 2. * b).max(0.), (h - 2. * b).max(0.)));
            // The text fills the visible area (clicks anywhere edit) and
            // grows past it (scrolls).
            let visible = t.scroll.contentSize();
            t.text.setMinSize(NSSize::new(0., visible.height));
            let current = t.text.frame().size.height;
            t.text
                .setFrameSize(NSSize::new(visible.width, current.max(visible.height)));
            t.text.sizeToFit();
        }
        if let View::Tabs(t) = view {
            let w = w as f64;
            t.bar.setFrame(rect(0., 0., w, HEADER_HEIGHT - 1.));
            t.separator.setFrame(rect(0., HEADER_HEIGHT - 1., w, 1.));
            let sz = t.segments.fittingSize();
            t.segments.setFrame(rect(
                ((w - sz.width) / 2.).round(),
                ((HEADER_HEIGHT - 1. - sz.height) / 2.).round(),
                sz.width,
                sz.height,
            ));
        }
        if let View::Stack(st) = view {
            let w = w as f64;
            st.header.setFrame(rect(0., 0., w, HEADER_HEIGHT - 1.));
            st.separator.setFrame(rect(0., HEADER_HEIGHT - 1., w, 1.));
            let b = st.back.fittingSize();
            st.back.setFrame(rect(
                if self.rtl { w - 8. - b.width } else { 8. },
                ((HEADER_HEIGHT - 1. - b.height) / 2.).round(),
                b.width,
                b.height,
            ));
            let t = st.title.fittingSize();
            st.title.setFrame(rect(
                0.,
                ((HEADER_HEIGHT - 1. - t.height) / 2.).round(),
                w,
                t.height,
            ));
        }
    }

    fn set_value(&mut self, view: &View, value: f64) {
        match view {
            View::Stack(st) => {
                st.closable.set(value != 0.);
                self.refresh_stack(st);
            }
            View::Progress(p) => p.setDoubleValue(value),
            View::Picture(v) => {
                v.ivars().fit.set(value == 1.);
                v.setNeedsDisplay(true);
            }
            View::Timer(l) => {
                self.timers.insert(view.id(), (l.clone(), value));
                l.setStringValue(&NSString::from_str(&prism_core::format_remaining(value)));
            }
            View::Toggle(t) => t.setState(if value != 0. {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            }),
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
                if d.dateValue().timeIntervalSince1970() != value {
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
            View::DatePicker(d) => configure_date_picker(d, DateConfig::decode(config)),
            View::Input(i) => {
                let c = InputConfig::decode(config);
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
                set_text_checking(&t.text, c.autocorrect);
                t.text.setAutomaticTextCompletionEnabled(c.autocorrect);
            }
            _ => {}
        }
    }

    fn set_icon(&mut self, view: &View, icon: &str) {
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
        let name = match appearance {
            Appearance::System => None,
            Appearance::Light => Some(unsafe { objc2_app_kit::NSAppearanceNameAqua }),
            Appearance::Dark => Some(unsafe { objc2_app_kit::NSAppearanceNameDarkAqua }),
        };
        let appearance = name.and_then(objc2_app_kit::NSAppearance::appearanceNamed);
        NSApplication::sharedApplication(self.mtm).setAppearance(appearance.as_deref());
    }

    fn set_scroll_events(&mut self, view: &View, on: bool) {
        let View::Scroll(sc) = view else { return };
        let clip = sc.scroll.contentView();
        let center = objc2_foundation::NSNotificationCenter::defaultCenter();
        let name = unsafe { objc2_app_kit::NSViewBoundsDidChangeNotification };
        unsafe { center.removeObserver_name_object(&self.target, Some(name), Some(&clip)) };
        if on {
            clip.setPostsBoundsChangedNotifications(true);
            unsafe {
                center.addObserver_selector_name_object(
                    &self.target,
                    sel!(onScrollBounds:),
                    Some(name),
                    Some(&clip),
                )
            };
        }
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

    fn set_content_size(&mut self, view: &View, width: f32, height: f32) {
        if let View::Scroll(sc) = view {
            sc.document
                .setFrameSize(NSSize::new(width as f64, height as f64));
        }
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
        v.setAccessibilityHidden(a.hidden);
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
                    // Status: not an element (its children stay reachable),
                    // its value is announced.
                    _ => None,
                }
            };
            if let View::Container(_) | View::Screen(_) | View::Tab(_) = view {
                v.setAccessibilityElement(role.is_some() && !a.hidden);
                v.setAccessibilityRole(role);
                if a.role == Some(Role::Switch) {
                    v.setAccessibilityRoleDescription(Some(&NSString::from_str("switch")));
                }
            } else if let Some(role) = role
                && matches!(a.role, Some(Role::Heading(_)))
            {
                v.setAccessibilityRole(Some(role));
            }
            if let Some(checked) = a.checked {
                let value = NSString::from_str(if checked { "1" } else { "0" });
                unsafe { v.setAccessibilityValue(Some(&value)) };
            }
        }
        if a.label.is_some() || !native {
            v.setAccessibilityLabel(ns(&a.label).as_deref());
        }
        v.setAccessibilityHelp(ns(&a.hint).as_deref());
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
        Viewport {
            x: 0.,
            y: HEADER_HEIGHT as f32,
            width: size.width as f32,
            height: (size.height - HEADER_HEIGHT).max(0.) as f32,
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
    let target = Target::new(mtm);
    crate::menus::install_main_menu(mtm, &config.title, &target, &[]);
    let root = PrismView::new(mtm, 0);
    let (width, height) = window_size(&config);
    let frame = NSRect::new(NSPoint::ZERO, NSSize::new(width as f64, height as f64));
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str(&config.title));
    window.setContentView(Some(&root));
    window.setDelegate(Some(ProtocolObject::from_ref(&*target)));
    window.center();

    let path = std::env::var("PRISM_PATH").unwrap_or_else(|_| "/".into());
    let services = crate::services::MacServices::new(
        mtm,
        Retained::into_super(Retained::into_super(target.clone())),
        window.clone(),
        config.title.clone(),
    );
    let mut runtime = Runtime::with_options(
        &init,
        prism_core::RuntimeOptions {
            path,
            capabilities: prism_core::PlatformServices::capabilities(&services),
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
    let renderer = NativeRenderer::new(AppKit {
        mtm,
        target: target.clone(),
        root,
        stacks: Vec::new(),
        tab_bars: Vec::new(),
        titles: HashMap::new(),
        icons: HashMap::new(),
        timers: HashMap::new(),
        sheets: Vec::new(),
        pictures: HashMap::new(),
        image_cache: HashMap::new(),
        image_sources: image_sources(config.assets.as_deref()),
        rtl: false,
        chrome: ("Back".into(), "Close".into()),
        transforms: HashMap::new(),
        inputs: HashMap::new(),
        announced: HashMap::new(),
        lang: String::new(),
    });
    let first = runtime.render();
    let mut state = State {
        runtime,
        renderer,
        services,
        device_size: None,
    };
    state.commit(first);
    state.relayout();
    let _ticker = unsafe {
        objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            1.0,
            &target,
            sel!(onTick:),
            None,
            true,
        )
    };
    STATE.with(|cell| *cell.borrow_mut() = Some(state));
    prism_native::apple::set_device_sink(on_device);
    flush_responses();
    // Development: `PRISM_RUN_BACKGROUND=sync` runs a background task now.
    if let Ok(id) = std::env::var("PRISM_RUN_BACKGROUND") {
        run_background(&id, std::sync::mpsc::channel().0);
    }

    observe_system(&target);
    window.makeKeyAndOrderFront(None);
    app.activate();
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
            && b.title().to_string() == title
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
        NSColor::windowBackgroundColor().setFill();
        objc2_app_kit::NSRectFill(at);
        image.drawInRect(at);
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

    // `PRISM_AFTER=<seconds>`: let async answers arrive before the snapshot.
    if let Some(after) = std::env::var("PRISM_AFTER")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        && CLICKED.with(|c| c.get())
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
                // Text areas keep their own scroll position.
                if sv
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
        on_main_after(std::time::Duration::ZERO, move || post_click(&w, &t));
        if let Ok(text) = std::env::var("PRISM_TYPE") {
            post_typing(&window, &text.replace("\\n", "\n"));
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
                    Ok(()) => println!("prism-macos: snapshot written to {path}"),
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

/// Assets: the bundle's Resources/assets, then the dev directory; downloads
/// are cached in ~/Library/Caches.
fn image_sources(dev_assets: Option<&str>) -> prism_native::images::ImageSources {
    let mut asset_dirs = Vec::new();
    if let Some(res) = objc2_foundation::NSBundle::mainBundle().resourcePath() {
        asset_dirs.push(std::path::PathBuf::from(res.to_string()).join("assets"));
    }
    if let Some(dev) = dev_assets {
        asset_dirs.push(dev.into());
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    prism_native::images::ImageSources {
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
fn on_main_after(delay: std::time::Duration, f: impl FnOnce() + 'static) {
    let id = LATER.with(|l| {
        let mut l = l.borrow_mut();
        l.0 += 1;
        let id = l.0;
        l.1.insert(id, Box::new(f));
        id
    });
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
