//! Sheets, popovers and dialogs a screen opens without a route (#19).
//!
//! The overlay node is a zero-size, hidden view in its parent (the
//! popover's anchor is that parent). Its content lives in a view controller
//! presented over the topmost one:
//!
//! - Sheet: `UISheetPresentationController` (large, or medium + large),
//!   with a grabber; swiping down closes it.
//! - Popover: `UIPopoverPresentationController` pointing at the parent,
//!   kept a popover on phones too; a tap outside closes it. A bare one
//!   (#287) has a clear background without arrow or shadow.
//! - Dialog: a card centered over a dimmed full-screen presentation; a tap
//!   outside closes it. tvOS has neither sheets nor popovers
//!   (`API_UNAVAILABLE(tvos)`): every overlay is such a card there, over a
//!   blurred screen, and Menu closes it.
//!
//! A dialog with a side (#79) puts its card beside the anchor (on the
//! other side without room, centered without room on either), still
//! modal. The card's backdrop
//! is clear, a dim, or a blur (a material view under the card); a clear
//! sheet leaves the window undimmed.
//!
//! Escape (hardware keyboard) closes non-fixed overlays; `fixed` sets
//! `modalInPresentation`. A user dismissal reports `UiEvent::Back(id, 0)`.
//!
//! An overlay inserted already open (with its parent, #70) is presented
//! once its anchor is in a window: by the layout that follows
//! (`present_if_pending`). A `manual` or `keep_focus` popover lets taps
//! through to the window around it (passthrough views, so they do not
//! close it); a `keep_focus` one leaves the first responder where it is
//! (the keyboard stays up), taking it back if UIKit moved it, and unless
//! `manual` a touch around it closes it all the same (a passive watcher on
//! the window, #182).
//!
//! A popover opens on its `side` of the anchor (#113): the matching
//! `permittedArrowDirections`, the opposite one when the window has no room
//! there, and a `sourceRect` on the part of the anchor it lines up with
//! (`PopoverAlign::arrow_span`).
//!
//! An overlay anchored at a caret (#328) goes against the caret of the
//! text field in its parent (`caretRectForPosition:` of the selection's
//! end, in the parent's coordinates, kept inside the field): the popover's
//! `sourceRect`, or the placed card's anchor. It follows the caret
//! (`follow_anchor`) as the field's text, selection or scrolling changes
//! and after each layout. An overlay anchored to a chip (#327) does the
//! same against the chip's glyph rectangle in its rich input's text view
//! (`Widgets::set_overlay_anchor` gives the view), kept inside the field.
//!
//! A keyboard bar (#242) is never presented: its content lives in an input
//! view (`UIInputView`, keyboard style) that the text fields it serves take
//! as their `inputAccessoryView` (`Widgets::set_keyboard_bar`), laid out at
//! the window's width.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
#[cfg_attr(target_os = "tvos", allow(unused_imports))]
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSObject, NSObjectProtocol, NSSet};
use objc2_ui_kit::{
    NSObjectUIAccessibility, UIAdaptivePresentationControllerDelegate, UIColor,
    UIGestureRecognizerDelegate, UIModalPresentationStyle, UIPress, UIPressesEvent, UIResponder,
    UITapGestureRecognizer, UITraitCollection, UIView, UIViewController,
};
use prism_core::NodeId;
use prism_core::controls::{Backdrop, OverlayAnchor, OverlayConfig, OverlayKind, place_beside};
use prism_native::Viewport;

/// What UIKit tells a popover's background (it reads them back).
#[cfg(not(target_os = "tvos"))]
pub struct BareBackgroundIvars {
    arrow_offset: Cell<f64>,
    arrow_direction: Cell<objc2_ui_kit::UIPopoverArrowDirection>,
}

#[cfg(not(target_os = "tvos"))]
impl Default for BareBackgroundIvars {
    fn default() -> Self {
        BareBackgroundIvars {
            arrow_offset: Cell::new(0.),
            arrow_direction: Cell::new(objc2_ui_kit::UIPopoverArrowDirection::Up),
        }
    }
}

#[cfg(not(target_os = "tvos"))]
define_class!(
    /// A bare popover's background (#287): nothing drawn, no arrow, no
    /// insets, no shadow, so the app's content is the whole popover.
    #[unsafe(super(objc2_ui_kit::UIPopoverBackgroundView, UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismBarePopoverBackground"]
    #[ivars = BareBackgroundIvars]
    pub struct BarePopoverBackground;

    impl BarePopoverBackground {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: objc2::rc::Allocated<Self>, frame: CGRect) -> Retained<Self> {
            let this = this.set_ivars(BareBackgroundIvars::default());
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        #[unsafe(method(arrowBase))]
        fn arrow_base() -> f64 {
            0.
        }

        #[unsafe(method(arrowHeight))]
        fn arrow_height() -> f64 {
            0.
        }

        #[unsafe(method(contentViewInsets))]
        fn content_view_insets() -> objc2_ui_kit::UIEdgeInsets {
            objc2_ui_kit::UIEdgeInsets {
                top: 0.,
                left: 0.,
                bottom: 0.,
                right: 0.,
            }
        }

        /// No rounded, inset content view: the app draws its own card.
        #[unsafe(method(wantsDefaultContentAppearance))]
        fn wants_default_content_appearance() -> bool {
            false
        }

        #[unsafe(method(arrowOffset))]
        fn arrow_offset(&self) -> f64 {
            self.ivars().arrow_offset.get()
        }

        #[unsafe(method(setArrowOffset:))]
        fn set_arrow_offset(&self, offset: f64) {
            self.ivars().arrow_offset.set(offset);
        }

        #[unsafe(method(arrowDirection))]
        fn arrow_direction(&self) -> objc2_ui_kit::UIPopoverArrowDirection {
            self.ivars().arrow_direction.get()
        }

        #[unsafe(method(setArrowDirection:))]
        fn set_arrow_direction(&self, direction: objc2_ui_kit::UIPopoverArrowDirection) {
            self.ivars().arrow_direction.set(direction);
        }

        /// UIKit gives the background a shadow: none here.
        #[unsafe(method(layoutSubviews))]
        fn layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            self.layer().setShadowOpacity(0.);
            self.setBackgroundColor(Some(&UIColor::clearColor()));
        }
    }
);

pub struct ControllerIvars {
    id: Cell<NodeId>,
    fixed: Cell<bool>,
    /// A card over a dimmed screen (dialogs; every overlay on tvOS).
    card: Retained<UIView>,
    custom: Cell<bool>,
    /// The touch watcher on the window around a popover that leaves the
    /// focus to its anchor (#182).
    outside: std::cell::RefCell<
        Option<(
            Retained<UIView>,
            Retained<objc2_ui_kit::UIGestureRecognizer>,
        )>,
    >,
    /// The current touch went down outside the popover.
    pressed_outside: Cell<bool>,
}

define_class!(
    /// Hosts an overlay's content; relays layout and closes on Escape.
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismOverlayController"]
    #[ivars = ControllerIvars]
    pub struct OverlayController;

    unsafe impl NSObjectProtocol for OverlayController {}

    impl OverlayController {
        #[unsafe(method(viewDidLayoutSubviews))]
        fn view_did_layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidLayoutSubviews] };
            crate::imp::relayout();
        }

        #[unsafe(method(canBecomeFirstResponder))]
        fn can_become_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(pressesBegan:withEvent:))]
        fn presses_began(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            if self.closing_press(presses) {
                if !self.ivars().fixed.get() {
                    self.close_by_user(prism_core::DismissReason::Escape);
                }
                return;
            }
            let _: () = unsafe { msg_send![super(self), pressesBegan: presses, withEvent: event] };
        }

        #[unsafe(method(pressesEnded:withEvent:))]
        fn presses_ended(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            if !self.closing_press(presses) {
                let _: () = unsafe { msg_send![super(self), pressesEnded: presses, withEvent: event] };
            }
        }

        /// A touch in the window around a popover that leaves the focus to
        /// its anchor (#182): one that went down outside it closes it once
        /// it ends, after what it landed on has had it (a button's action,
        /// the field's caret): closing during the touch would cancel it.
        #[unsafe(method(onOutsideTouch:))]
        fn on_outside_touch(&self, g: &objc2_ui_kit::UIGestureRecognizer) {
            use objc2_ui_kit::UIGestureRecognizerState as S;
            match g.state() {
                S::Began => {
                    let Some(view) = self.viewIfLoaded() else { return };
                    let p = g.locationInView(Some(&view));
                    let b = view.bounds();
                    let inside =
                        p.x >= 0. && p.y >= 0. && p.x <= b.size.width && p.y <= b.size.height;
                    self.ivars().pressed_outside.set(!inside);
                }
                S::Ended | S::Cancelled | S::Failed => {
                    if !self.ivars().pressed_outside.replace(false) || self.ivars().fixed.get() {
                        return;
                    }
                    // On the next turn of the run loop: the touch ends first.
                    let none: Option<&AnyObject> = None;
                    let _: () = unsafe {
                        msg_send![self, performSelector: sel!(closeOutside), withObject: none, afterDelay: 0.0f64]
                    };
                }
                _ => {}
            }
        }

        #[unsafe(method(closeOutside))]
        fn close_outside(&self) {
            if self.presentingViewController().is_some() {
                self.close_by_user(prism_core::DismissReason::Outside);
            }
        }

        /// A tap on the dimmed backdrop, outside the card.
        #[unsafe(method(onBackdrop:))]
        fn on_backdrop(&self, tap: &UITapGestureRecognizer) {
            let card = &self.ivars().card;
            let p = tap.locationInView(Some(card));
            let b = card.bounds();
            let inside = p.x >= 0. && p.y >= 0. && p.x <= b.size.width && p.y <= b.size.height;
            if !inside && !self.ivars().fixed.get() {
                self.close_by_user(prism_core::DismissReason::Outside);
            }
        }

        #[unsafe(method_id(preferredFocusEnvironments))]
        fn preferred_focus_environments(
            &self,
        ) -> Retained<objc2_foundation::NSArray<ProtocolObject<dyn objc2_ui_kit::UIFocusEnvironment>>> {
            match crate::interaction::take_focus_target() {
                Some(v) => objc2_foundation::NSArray::from_retained_slice(&[ProtocolObject::from_retained(v)]),
                None => unsafe { msg_send![super(self), preferredFocusEnvironments] },
            }
        }
    }
);

impl OverlayController {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let card = UIView::new(mtm);
        let this = Self::alloc(mtm).set_ivars(ControllerIvars {
            id: Cell::new(0),
            fixed: Cell::new(false),
            card,
            custom: Cell::new(false),
            outside: Default::default(),
            pressed_outside: Cell::new(false),
        });
        unsafe { msg_send![super(this), init] }
    }

    /// Escape on a keyboard, Menu on a TV remote.
    fn closing_press(&self, presses: &NSSet<UIPress>) -> bool {
        let mtm = self.mtm();
        presses.iter().any(|p| match p.key(mtm) {
            Some(k) => k.keyCode().0 == 0x29,
            None => cfg!(target_os = "tvos") && p.r#type() == objc2_ui_kit::UIPressType::Menu,
        })
    }

    /// Watches touches in `window` while shown (#182): a zero-duration
    /// press that never blocks or cancels anything (the field still gets
    /// its tap).
    #[cfg_attr(target_os = "tvos", allow(dead_code))]
    fn watch_outside(&self, window: &UIView, delegate: &OverlayDelegate) {
        self.unwatch_outside();
        let target: &AnyObject = self;
        let g = unsafe {
            objc2_ui_kit::UILongPressGestureRecognizer::initWithTarget_action(
                objc2_ui_kit::UILongPressGestureRecognizer::alloc(self.mtm()),
                Some(target),
                Some(sel!(onOutsideTouch:)),
            )
        };
        g.setMinimumPressDuration(0.);
        g.setAllowableMovement(f64::MAX);
        g.setCancelsTouchesInView(false);
        g.setDelaysTouchesBegan(false);
        g.setDelaysTouchesEnded(false);
        let d: &AnyObject = delegate;
        let _: () = unsafe { msg_send![&*g, setDelegate: d] };
        let g: Retained<objc2_ui_kit::UIGestureRecognizer> = Retained::into_super(g);
        window.addGestureRecognizer(&g);
        *self.ivars().outside.borrow_mut() = Some((window.retain(), g));
    }

    fn unwatch_outside(&self) {
        if let Some((w, g)) = self.ivars().outside.borrow_mut().take() {
            w.removeGestureRecognizer(&g);
        }
    }

    fn close_by_user(&self, reason: prism_core::DismissReason) {
        self.unwatch_outside();
        let id = self.ivars().id.get();
        if self.presentingViewController().is_some() {
            self.dismissViewControllerAnimated_completion(true, None);
        }
        crate::imp::overlay_closed(id, reason);
    }
}

define_class!(
    /// Tells Prism a sheet or popover was closed by the user, and keeps
    /// popovers popovers on phones.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismOverlayDelegate"]
    #[ivars = Cell<NodeId>]
    pub struct OverlayDelegate;

    unsafe impl NSObjectProtocol for OverlayDelegate {}

    /// The outside-touch watcher (#182) runs beside every other gesture.
    unsafe impl UIGestureRecognizerDelegate for OverlayDelegate {
        #[unsafe(method(gestureRecognizer:shouldRecognizeSimultaneouslyWithGestureRecognizer:))]
        fn simultaneous(
            &self,
            _g: &objc2_ui_kit::UIGestureRecognizer,
            _other: &objc2_ui_kit::UIGestureRecognizer,
        ) -> bool {
            true
        }
    }

    unsafe impl UIAdaptivePresentationControllerDelegate for OverlayDelegate {
        /// A popover closes on a tap outside it, a sheet on a swipe down.
        #[unsafe(method(presentationControllerDidDismiss:))]
        fn did_dismiss(&self, pc: &objc2_ui_kit::UIPresentationController) {
            let popover = pc.isKindOfClass(<objc2_ui_kit::UIPopoverPresentationController as objc2::ClassType>::class());
            let reason = if popover {
                prism_core::DismissReason::Outside
            } else {
                prism_core::DismissReason::Swipe
            };
            crate::imp::overlay_closed(self.ivars().get(), reason);
        }

        #[unsafe(method(adaptivePresentationStyleForPresentationController:traitCollection:))]
        fn adaptive_style(
            &self,
            pc: &objc2_ui_kit::UIPresentationController,
            _traits: &UITraitCollection,
        ) -> UIModalPresentationStyle {
            // Popovers stay popovers on iPhone (sheets already are sheets).
            let _ = pc;
            UIModalPresentationStyle::None
        }
    }
);

/// One overlay node.
pub struct Overlay {
    /// The zero-size view in the parent (the popover's anchor is its superview).
    pub anchor: Retained<UIView>,
    pub vc: Retained<OverlayController>,
    delegate: Retained<OverlayDelegate>,
    pub config: Cell<OverlayConfig>,
    /// The app wants it shown.
    pub open: Cell<bool>,
    /// Size of the content Prism laid out.
    pub size: Cell<(f64, f64)>,
    /// The content view.
    content: std::cell::RefCell<Option<Retained<UIView>>>,
    /// A keyboard bar's host (#242): the fields' `inputAccessoryView`.
    pub keyboard: Retained<UIView>,
    /// A blurred backdrop under the card (#79).
    blur: std::cell::RefCell<Option<Retained<objc2_ui_kit::UIVisualEffectView>>>,
    /// The field it is anchored in (#327), for a config anchor naming one:
    /// its view (a rich input's text view).
    anchor_to: std::cell::RefCell<Option<Retained<UIView>>>,
}

impl Overlay {
    pub fn new(mtm: MainThreadMarker, id: NodeId) -> Overlay {
        let anchor = UIView::new(mtm);
        anchor.setHidden(true);
        anchor.setUserInteractionEnabled(false);
        let vc = OverlayController::new(mtm);
        vc.ivars().id.set(id);
        let delegate = OverlayDelegate::alloc(mtm).set_ivars(Cell::new(id));
        let delegate: Retained<OverlayDelegate> = unsafe { msg_send![super(delegate), init] };
        #[cfg(target_os = "ios")]
        let keyboard =
            Retained::into_super(objc2_ui_kit::UIInputView::initWithFrame_inputViewStyle(
                objc2_ui_kit::UIInputView::alloc(mtm),
                CGRect::new(CGPoint::ZERO, CGSize::new(320., 44.)),
                objc2_ui_kit::UIInputViewStyle::Keyboard,
            ));
        #[cfg(not(target_os = "ios"))]
        let keyboard = UIView::new(mtm);
        keyboard.setAutoresizingMask(objc2_ui_kit::UIViewAutoresizing::FlexibleWidth);
        Overlay {
            anchor,
            vc,
            delegate,
            config: Cell::new(OverlayConfig::default()),
            open: Cell::new(false),
            size: Cell::new((0., 0.)),
            content: Default::default(),
            blur: Default::default(),
            keyboard,
            anchor_to: Default::default(),
        }
    }

    /// The field it is anchored in (#327): its view, or `None`. Placed
    /// again there if shown.
    pub fn set_anchor(&self, field: Option<Retained<UIView>>) {
        let changed = *self.anchor_to.borrow() != field;
        *self.anchor_to.borrow_mut() = field;
        if changed {
            self.follow_anchor();
        }
    }

    /// A bar above the on-screen keyboard (#242).
    fn is_keyboard(&self) -> bool {
        self.config.get().kind == OverlayKind::Keyboard
    }

    /// Dialogs everywhere, and every overlay on tvOS, are drawn cards.
    fn custom(&self) -> bool {
        cfg!(target_os = "tvos") || self.config.get().kind == OverlayKind::Dialog
    }

    fn host(&self) -> Retained<UIView> {
        if self.is_keyboard() {
            self.keyboard.clone()
        } else if self.custom() {
            self.vc.ivars().card.clone()
        } else {
            self.vc.view().expect("view controller has a view")
        }
    }

    pub fn set_content(&self, children: &[Retained<UIView>]) {
        let host = self.host();
        let mut current = self.content.borrow_mut();
        if let Some(old) = current.take()
            && !children.contains(&old)
        {
            old.removeFromSuperview();
        }
        if let Some(c) = children.first() {
            if c.superview().as_deref() != Some(&*host) {
                host.addSubview(c);
            }
            *current = Some(c.clone());
        }
    }

    pub fn set_config(&self, config: &str) {
        let c = OverlayConfig::decode(config);
        self.config.set(c);
        self.vc.ivars().fixed.set(c.fixed);
        self.vc.setModalInPresentation(c.fixed);
        // Content added before the kind was known moves to its host.
        let host = self.host();
        if let Some(content) = self.content.borrow().as_ref()
            && content.superview().as_deref() != Some(&*host)
        {
            host.addSubview(content);
        }
    }

    fn presented(&self) -> bool {
        self.vc.presentingViewController().is_some()
    }

    /// Shows or closes the overlay, over the topmost presented controller.
    pub fn set_open(&self, open: bool, root: &UIViewController) {
        self.open.set(open);
        if self.is_keyboard() {
            // Shown by UIKit above the keyboard of the fields it serves.
            return;
        }
        if !open {
            self.close();
            return;
        }
        if self.presented() {
            return;
        }
        // Inserted open: the anchor (a popover's source) or the app's view
        // is not in a window yet; the next layout presents it.
        let Some(window) = root.viewIfLoaded().and_then(|v| v.window()) else {
            return;
        };
        if self.config.get().kind == OverlayKind::Popover
            && !cfg!(target_os = "tvos")
            && self.anchor.superview().and_then(|s| s.window()).is_none()
        {
            return;
        }
        self.configure();
        let top = crate::menus::top_controller(root);
        if *top == **self.vc {
            return;
        }
        let vc = self.vc.clone();
        let keep_focus = self.keeps_focus();
        #[cfg(not(target_os = "tvos"))]
        let focused = first_responder(&window);
        #[cfg(target_os = "tvos")]
        let _ = &window;
        let done = block2::RcBlock::new(move || {
            #[cfg(not(target_os = "tvos"))]
            if keep_focus {
                // The anchor's field keeps the keys (and the keyboard).
                if let Some(f) = &focused
                    && !f.isFirstResponder()
                {
                    let _ = f.becomeFirstResponder();
                }
            } else if vc.view().and_then(|v| first_responder(&v)).is_none() {
                // Escape reaches the overlay unless a field inside has the focus.
                let _ = vc.becomeFirstResponder();
            }
            #[cfg(target_os = "tvos")]
            let _ = (&vc, keep_focus);
        });
        // A press around it closes it, and reaches what it landed on (#182).
        #[cfg(not(target_os = "tvos"))]
        if self.config.get().light_dismiss_beside_focus() {
            self.vc.watch_outside(&window, &self.delegate);
        }
        // A bare dialog appears at once: no cross-fade of its own (#166).
        let animated = !self.config.get().bare;
        top.presentViewController_animated_completion(&self.vc, animated, Some(&done));
    }

    /// Presents an overlay the app opened before it could be shown.
    pub fn present_if_pending(&self, root: &UIViewController) {
        if self.open.get() && !self.presented() {
            self.set_open(true, root);
        }
    }

    /// A popover that leaves the first responder to its anchor's field.
    fn keeps_focus(&self) -> bool {
        let c = self.config.get();
        c.kind == OverlayKind::Popover && c.keep_focus && !cfg!(target_os = "tvos")
    }

    pub fn close(&self) {
        self.open.set(false);
        self.vc.unwatch_outside();
        if self.presented() {
            self.vc.dismissViewControllerAnimated_completion(true, None);
        }
    }

    /// Presentation style, detents, anchor, backdrop.
    fn configure(&self) {
        let mtm = self.vc.mtm();
        let c = self.config.get();
        let custom = self.custom();
        self.vc.ivars().custom.set(custom);
        let view = self.vc.view().expect("view controller has a view");
        let card = &self.vc.ivars().card;
        if custom {
            let tv = cfg!(target_os = "tvos");
            let backdrop = c
                .backdrop
                .unwrap_or(if tv { Backdrop::Blur } else { Backdrop::Dim });
            self.vc
                .setModalPresentationStyle(if tv && backdrop == Backdrop::Blur {
                    UIModalPresentationStyle::BlurOverFullScreen
                } else {
                    UIModalPresentationStyle::OverFullScreen
                });
            self.vc
                .setModalTransitionStyle(objc2_ui_kit::UIModalTransitionStyle::CrossDissolve);
            // A clear backdrop still takes the taps that close the card.
            let dim = match backdrop {
                Backdrop::Clear => 0.,
                Backdrop::Dim => 0.35,
                Backdrop::Blur if tv => 0.,
                Backdrop::Blur => 0.15,
            };
            view.setBackgroundColor(Some(&UIColor::colorWithWhite_alpha(0., dim)));
            self.set_blur(&view, backdrop == Backdrop::Blur && !tv);
            // Over an undimmed page, a hairline marks the card's edge; a
            // bare dialog has no card of its own (#166): the app draws it,
            // its shadows included.
            let edge = UIColor::colorWithWhite_alpha(0.5, 0.3);
            card.layer()
                .setBorderWidth(if backdrop == Backdrop::Clear && !c.bare {
                    0.5
                } else {
                    0.
                });
            let edge = unsafe { edge.CGColor() };
            card.layer().setBorderColor(Some(&edge));
            // White in light mode, the raised grey of alerts in dark mode.
            let fill = if c.bare {
                UIColor::clearColor()
            } else {
                UIColor::tertiarySystemBackgroundColor()
            };
            card.setBackgroundColor(Some(&fill));
            card.layer().setCornerRadius(if c.bare { 0. } else { 16. });
            card.setClipsToBounds(!c.bare);
            if card.superview().is_none() {
                view.addSubview(card);
                let target: &AnyObject = &self.vc;
                let tap = unsafe {
                    UITapGestureRecognizer::initWithTarget_action(
                        UITapGestureRecognizer::alloc(mtm),
                        Some(target),
                        Some(sel!(onBackdrop:)),
                    )
                };
                tap.setCancelsTouchesInView(false);
                view.addGestureRecognizer(&tap);
            }
            view.setAccessibilityViewIsModal(true, mtm);
            self.place_card();
        } else {
            // A bare popover (#287): the app's content is the whole card.
            let fill = if c.bare && c.kind == OverlayKind::Popover {
                UIColor::clearColor()
            } else {
                UIColor::systemBackgroundColor()
            };
            view.setBackgroundColor(Some(&fill));
            #[cfg(not(target_os = "tvos"))]
            match c.kind {
                OverlayKind::Popover => {
                    self.vc
                        .setModalPresentationStyle(UIModalPresentationStyle::Popover);
                    if let Some(pc) = self.vc.popoverPresentationController() {
                        // SAFETY: the class is a `UIPopoverBackgroundView`.
                        unsafe {
                            pc.setPopoverBackgroundViewClass(
                                c.bare
                                    .then(<BarePopoverBackground as objc2::ClassType>::class),
                            );
                        }
                        let source = self.anchor.superview();
                        pc.setSourceView(source.as_deref());
                        let window = source.as_ref().and_then(|s| s.window());
                        if let Some(s) = &source {
                            let rtl = s.effectiveUserInterfaceLayoutDirection()
                                == objc2_ui_kit::UIUserInterfaceLayoutDirection::RightToLeft;
                            let anchor = self.anchor_rect(s);
                            let room = window
                                .as_ref()
                                .map(|w| (s.convertRect_toView(anchor, Some(w)), w.bounds()));
                            let (rect, arrows) =
                                popover_placement(anchor, self.size.get(), c, rtl, room);
                            pc.setSourceRect(rect);
                            pc.setPermittedArrowDirections(arrows);
                        }
                        // Taps around a manual or inline popover reach the
                        // window (the anchor's field) and do not close it.
                        let through: Vec<Retained<UIView>> = match window {
                            Some(w) if c.manual || c.keep_focus => vec![Retained::into_super(w)],
                            _ => Vec::new(),
                        };
                        pc.setPassthroughViews(Some(
                            &objc2_foundation::NSArray::from_retained_slice(&through),
                        ));
                    }
                    let (w, h) = self.size.get();
                    if w > 0. && h > 0. {
                        self.vc.setPreferredContentSize(CGSize::new(w, h));
                    }
                }
                _ => {
                    self.vc
                        .setModalPresentationStyle(UIModalPresentationStyle::PageSheet);
                    if let Some(sheet) = self.vc.sheetPresentationController() {
                        use objc2_ui_kit::UISheetPresentationControllerDetent as D;
                        let detents = if c.medium {
                            vec![D::mediumDetent(mtm), D::largeDetent(mtm)]
                        } else {
                            vec![D::largeDetent(mtm)]
                        };
                        sheet.setDetents(&objc2_foundation::NSArray::from_retained_slice(&detents));
                        sheet.setPrefersGrabberVisible(true);
                        // A clear sheet leaves the window undimmed (#79).
                        let undimmed = (c.backdrop == Some(Backdrop::Clear)).then_some(unsafe {
                            objc2_ui_kit::UISheetPresentationControllerDetentIdentifierLarge
                        });
                        sheet.setLargestUndimmedDetentIdentifier(undimmed);
                    }
                }
            }
        }
        self.vc.setModalInPresentation(c.fixed);
        if let Some(pc) = self.vc.presentationController() {
            unsafe { pc.setDelegate(Some(ProtocolObject::from_ref(&*self.delegate))) };
        }
    }

    /// A material view under the card (a blurred backdrop), or none.
    fn set_blur(&self, view: &UIView, on: bool) {
        use objc2_ui_kit::{
            UIBlurEffect, UIBlurEffectStyle, UIViewAutoresizing, UIVisualEffectView,
        };
        let mut blur = self.blur.borrow_mut();
        if !on {
            if let Some(b) = blur.take() {
                b.removeFromSuperview();
            }
            return;
        }
        let mtm = view.mtm();
        let b = blur.get_or_insert_with(|| {
            #[cfg(not(target_os = "tvos"))]
            let style = UIBlurEffectStyle::SystemUltraThinMaterial;
            #[cfg(target_os = "tvos")]
            let style = UIBlurEffectStyle::Regular;
            let effect = UIBlurEffect::effectWithStyle(style, mtm);
            let b =
                UIVisualEffectView::initWithEffect(UIVisualEffectView::alloc(mtm), Some(&effect));
            b.setUserInteractionEnabled(false);
            b.setAutoresizingMask(
                UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight,
            );
            b
        });
        if b.superview().as_deref() != Some(view) {
            view.insertSubview_atIndex(b, 0);
        }
        b.setFrame(view.bounds());
    }

    /// The card's frame: the content's size, beside its anchor for a
    /// dialog with a side (#79), else centered.
    fn place_card(&self) {
        let Some(view) = self.vc.viewIfLoaded() else {
            return;
        };
        let (w, h) = self.size.get();
        let b = view.bounds().size;
        let card = &self.vc.ivars().card;
        let origin = self.beside_anchor(w, h).unwrap_or_else(|| {
            CGPoint::new(((b.width - w) / 2.).max(0.), ((b.height - h) / 2.).max(0.))
        });
        let frame = CGRect::new(origin, CGSize::new(w, h));
        if card.frame() != frame {
            card.setFrame(frame);
        }
    }

    /// Where a placed dialog's card goes, in window coordinates (the
    /// card's full-screen view covers the window): `None` without a
    /// side, an anchor in a window, or room on either side.
    #[allow(clippy::cast_possible_truncation)]
    fn beside_anchor(&self, w: f64, h: f64) -> Option<CGPoint> {
        let c = self.config.get();
        if c.kind != OverlayKind::Dialog || cfg!(target_os = "tvos") {
            return None;
        }
        let side = c.anchored_side()?;
        let anchor = self.anchor.superview()?;
        let window = anchor.window()?;
        let a = anchor.convertRect_toView(self.anchor_rect(&anchor), None);
        let (b, i) = (window.bounds(), window.safeAreaInsets());
        let rtl = anchor.effectiveUserInterfaceLayoutDirection()
            == objc2_ui_kit::UIUserInterfaceLayoutDirection::RightToLeft;
        let (m, f) = (8., |v: f64| v as f32);
        let (x, y) = place_beside(
            (
                f(a.origin.x),
                f(a.origin.y),
                f(a.size.width),
                f(a.size.height),
            ),
            (f(w), f(h)),
            (
                f(b.origin.x + i.left + m),
                f(b.origin.y + i.top + m),
                f(b.size.width - i.left - i.right - 2. * m),
                f(b.size.height - i.top - i.bottom - 2. * m),
            ),
            side,
            c.align,
            rtl,
            8.,
        )?;
        Some(CGPoint::new(f64::from(x), f64::from(y)))
    }

    /// What it goes against, in `source`'s coordinates (its parent):
    /// `source`'s bounds, or the caret of a text field in it (#328).
    fn anchor_rect(&self, source: &UIView) -> CGRect {
        let c = self.config.get();
        match c.anchor {
            #[cfg(not(target_os = "tvos"))]
            OverlayAnchor::Caret => {
                let Some(caret) = caret_anchor(source) else {
                    return source.bounds();
                };
                // UIKit centers a popover on its source: a bare one (no
                // arrow) lines up with the caret as `align` says when the
                // source spans its width from the caret.
                let w = self.size.get().0;
                if !(c.kind == OverlayKind::Popover && c.bare && !c.side.beside() && w > 0.) {
                    return caret;
                }
                let rtl = source.effectiveUserInterfaceLayoutDirection()
                    == objc2_ui_kit::UIUserInterfaceLayoutDirection::RightToLeft;
                let x = caret.origin.x;
                let left = match c.align {
                    prism_core::PopoverAlign::Center => return caret,
                    prism_core::PopoverAlign::Start if !rtl => x,
                    prism_core::PopoverAlign::End if rtl => x,
                    _ => x - w,
                };
                CGRect::new(
                    CGPoint::new(left, caret.origin.y),
                    CGSize::new(w, caret.size.height),
                )
            }
            #[cfg(not(target_os = "tvos"))]
            anchor @ (OverlayAnchor::Text { .. } | OverlayAnchor::Field(_)) => self
                .field_anchor(source, anchor)
                .unwrap_or_else(|| source.bounds()),
            _ => source.bounds(),
        }
    }

    /// The box of the anchor's characters in its field's text view (a
    /// chip's glyph rectangle), kept inside the field's visible part, or
    /// the whole field, in `source`'s coordinates (#327). `None` without
    /// the field in the window.
    #[cfg(not(target_os = "tvos"))]
    fn field_anchor(&self, source: &UIView, anchor: OverlayAnchor) -> Option<CGRect> {
        let view = self.anchor_to.borrow().clone()?;
        view.window()?;
        // A text view's bounds are its visible part (scrolled).
        let field = view.bounds();
        let chars = match (anchor, view.downcast_ref::<objc2_ui_kit::UITextView>()) {
            (OverlayAnchor::Text { start, end, .. }, Some(tv)) => {
                crate::rich::range_box(tv, start, end)
            }
            _ => None,
        };
        let r = match chars {
            Some(r) => {
                let f = |r: CGRect| {
                    (
                        r.origin.x as f32,
                        r.origin.y as f32,
                        r.size.width as f32,
                        r.size.height as f32,
                    )
                };
                // Scrolled out of the field: on its nearest edge.
                let (x, y, w, h) = prism_core::controls::caret_in_field(f(r), f(field));
                CGRect::new(
                    CGPoint::new(f64::from(x), f64::from(y)),
                    CGSize::new(f64::from(w), f64::from(h)),
                )
            }
            None => field,
        };
        Some(source.convertRect_fromView(r, Some(&view)))
    }

    /// An overlay at a caret (#328) or a chip (#327) follows it: the card
    /// moves, or the popover's `sourceRect` (UIKit moves the popover on
    /// its next layout, asked for now).
    pub fn follow_anchor(&self) {
        let c = self.config.get();
        if c.anchor == OverlayAnchor::Parent || !self.presented() || self.is_keyboard() {
            return;
        }
        if self.custom() {
            self.place_card();
        } else {
            self.follow_source(c);
        }
    }

    /// A shown popover's `sourceRect` on its anchor again (tvOS has none).
    fn follow_source(&self, c: OverlayConfig) {
        #[cfg(target_os = "tvos")]
        let _ = c;
        #[cfg(not(target_os = "tvos"))]
        if c.kind == OverlayKind::Popover
            && let Some(pc) = self.vc.popoverPresentationController()
            && let Some(s) = self.anchor.superview()
        {
            let rtl = s.effectiveUserInterfaceLayoutDirection()
                == objc2_ui_kit::UIUserInterfaceLayoutDirection::RightToLeft;
            let anchor = self.anchor_rect(&s);
            let room = s
                .window()
                .map(|w| (s.convertRect_toView(anchor, Some(&w)), w.bounds()));
            let (rect, _) = popover_placement(anchor, self.size.get(), c, rtl, room);
            if pc.sourceRect() != rect {
                pc.setSourceRect(rect);
                if let Some(container) = pc.containerView() {
                    container.setNeedsLayout();
                    container.layoutIfNeeded();
                }
            }
        }
    }

    /// The content's laid-out size: the popover's or card's size.
    pub fn set_size(&self, w: f64, h: f64) {
        if self.size.get() == (w, h) {
            // The anchor may have moved (a rotation, a resize).
            if self.custom() {
                self.place_card();
            }
            return;
        }
        self.size.set((w, h));
        if self.is_keyboard() {
            self.keyboard
                .setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(w, h)));
            crate::interaction::keyboard_bar_resized(&self.keyboard);
        } else if self.custom() {
            self.place_card();
        } else if self.config.get().kind == OverlayKind::Popover {
            self.vc.setPreferredContentSize(CGSize::new(w, h));
        }
    }

    /// Where the content goes and how big it may be (see
    /// `Widgets::overlay_viewport`): a sheet's own size and safe area; a
    /// popover or card at most 340 points wide inside the window.
    pub fn viewport(&self, window: CGSize) -> Viewport {
        let kind = self.config.get().kind;
        if kind == OverlayKind::Keyboard {
            // The window's width; the bar's own height, at most half the
            // window.
            return Viewport {
                x: 0.,
                y: 0.,
                width: window.width as f32,
                height: (window.height / 2.) as f32,
                insets: prism_native::Insets::default(),
            };
        }
        let view = self.vc.viewIfLoaded();
        if !self.custom() && kind == OverlayKind::Sheet {
            let (size, i) = match view.filter(|v| v.window().is_some()) {
                Some(v) => (v.bounds().size, v.safeAreaInsets()),
                None => (
                    window,
                    objc2_ui_kit::UIEdgeInsets {
                        top: 0.,
                        left: 0.,
                        bottom: 0.,
                        right: 0.,
                    },
                ),
            };
            return Viewport {
                x: 0.,
                y: 0.,
                width: size.width as f32,
                height: size.height as f32,
                insets: prism_native::Insets {
                    // The grabber sits over the top edge.
                    top: (i.top as f32).max(if cfg!(target_os = "ios") { 12. } else { 0. }),
                    right: i.right as f32,
                    bottom: i.bottom as f32,
                    left: i.left as f32,
                },
            };
        }
        let margin = if cfg!(target_os = "tvos") { 120. } else { 32. };
        let width = (window.width - margin).clamp(
            0.,
            if kind == OverlayKind::Popover {
                320.
            } else {
                340.
            },
        );
        Viewport {
            x: 0.,
            y: 0.,
            width: width as f32,
            height: ((window.height - 2. * margin)
                * if kind == OverlayKind::Popover {
                    0.7
                } else {
                    1.
                })
            .max(0.) as f32,
            insets: prism_native::Insets::default(),
        }
    }
}

/// The caret an overlay anchored at it goes against (#328), in `source`'s
/// coordinates: the caret of the text field inside `source` that is first
/// responder (`caretRectForPosition:` at the selection's end), kept inside
/// the field's visible part; the whole first field there when none is;
/// `None` without one.
#[cfg(not(target_os = "tvos"))]
fn caret_anchor(source: &UIView) -> Option<CGRect> {
    use objc2_ui_kit::{UITextField, UITextInput, UITextView};
    fn caret<T: UITextInput + Message>(input: &T, field: &UIView) -> Option<CGRect> {
        let end = input.selectedTextRange()?.end();
        let r = input.caretRectForPosition(&end);
        if !r.origin.x.is_finite() || !r.origin.y.is_finite() {
            return None;
        }
        Some(field.convertRect_fromView(r, Some(&input.textInputView())))
    }
    let in_source = |v: &UIView, r: CGRect| source.convertRect_fromView(r, Some(v));
    if let Some(focused) = first_responder(source) {
        let found = if let Some(f) = focused.downcast_ref::<UITextField>() {
            caret(f, &focused)
        } else if let Some(t) = focused.downcast_ref::<UITextView>() {
            caret(t, &focused)
        } else {
            None
        };
        if let Some(r) = found {
            let f = |r: CGRect| {
                (
                    r.origin.x as f32,
                    r.origin.y as f32,
                    r.size.width as f32,
                    r.size.height as f32,
                )
            };
            // A text view's bounds are its visible part (scrolled).
            let (x, y, w, h) = prism_core::controls::caret_in_field(f(r), f(focused.bounds()));
            let r = CGRect::new(
                CGPoint::new(f64::from(x), f64::from(y)),
                CGSize::new(f64::from(w), f64::from(h)),
            );
            return Some(in_source(&focused, r));
        }
    }
    let first = first_field(source)?;
    Some(in_source(&first, first.bounds()))
}

/// The first editable text field in `view` (a text input, or a text area's
/// or rich input's text view), in tree order.
#[cfg(not(target_os = "tvos"))]
fn first_field(view: &UIView) -> Option<Retained<UIView>> {
    for v in view.subviews().iter() {
        if v.isHidden() {
            continue;
        }
        let editable = v
            .downcast_ref::<objc2_ui_kit::UITextField>()
            .is_some_and(|f| f.isEnabled())
            || v.downcast_ref::<objc2_ui_kit::UITextView>()
                .is_some_and(|t| t.isEditable());
        if editable {
            return Some(v.clone());
        }
        if let Some(f) = first_field(&v) {
            return Some(f);
        }
    }
    None
}

#[cfg(not(target_os = "tvos"))]
fn first_responder(v: &UIView) -> Option<Retained<UIView>> {
    if v.isFirstResponder() {
        return Some(v.retain());
    }
    v.subviews().iter().find_map(|c| first_responder(&c))
}

/// Where a popover points (#113): the part of the anchor's `bounds` it
/// lines up with, and the arrow directions UIKit may use. Its `side` when
/// the window (`room`: the anchor's frame in the window, the window's
/// bounds) has space for a `size` popover there, else the opposite side
/// when that one has; both when neither has (UIKit picks).
#[cfg(not(target_os = "tvos"))]
fn popover_placement(
    bounds: CGRect,
    size: (f64, f64),
    c: OverlayConfig,
    rtl: bool,
    room: Option<(CGRect, CGRect)>,
) -> (CGRect, objc2_ui_kit::UIPopoverArrowDirection) {
    use objc2_ui_kit::UIPopoverArrowDirection as Arrow;
    use prism_core::PopoverSide;
    // The arrow points back at the anchor: up for a popover below it, left
    // for one on its right.
    let arrow = |side: PopoverSide| match side {
        PopoverSide::Bottom => Arrow::Up,
        PopoverSide::Top => Arrow::Down,
        PopoverSide::Trailing if rtl => Arrow::Right,
        PopoverSide::Trailing => Arrow::Left,
        PopoverSide::Leading if rtl => Arrow::Left,
        PopoverSide::Leading => Arrow::Right,
    };
    // The arrow and a margin.
    let gap = 20.;
    let (w, h) = size;
    let fits = |side: PopoverSide| {
        let Some((a, win)) = room else { return false };
        if w <= 0. || h <= 0. {
            return false;
        }
        let (x0, y0) = (a.origin.x, a.origin.y);
        let (x1, y1) = (x0 + a.size.width, y0 + a.size.height);
        let (wx, wy) = (win.origin.x, win.origin.y);
        match arrow(side) {
            a if a == Arrow::Up => y1 + gap + h <= wy + win.size.height,
            a if a == Arrow::Down => y0 - gap - h >= wy,
            a if a == Arrow::Left => x1 + gap + w <= wx + win.size.width,
            _ => x0 - gap - w >= wx,
        }
    };
    let arrows = if fits(c.side) {
        arrow(c.side)
    } else if fits(c.side.opposite()) {
        arrow(c.side.opposite())
    } else {
        arrow(c.side) | arrow(c.side.opposite())
    };
    let o = bounds.origin;
    let (bw, bh) = (bounds.size.width, bounds.size.height);
    let rect = if c.side.beside() {
        // Start is the top (UIKit's y goes down).
        let (offset, part) = c.align.arrow_span(bh, h, true);
        CGRect::new(CGPoint::new(o.x, o.y + offset), CGSize::new(bw, part))
    } else {
        let (offset, part) = c.align.arrow_span(bw, w, !rtl);
        CGRect::new(CGPoint::new(o.x + offset, o.y), CGSize::new(part, bh))
    };
    (rect, arrows)
}
