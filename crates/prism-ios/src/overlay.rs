//! Sheets, popovers and dialogs a screen opens without a route (#19).
//!
//! The overlay node is a zero-size, hidden view in its parent (the
//! popover's anchor is that parent). Its content lives in a view controller
//! presented over the topmost one:
//!
//! - Sheet: `UISheetPresentationController` (large, or medium + large),
//!   with a grabber; swiping down closes it.
//! - Popover: `UIPopoverPresentationController` pointing at the parent,
//!   kept a popover on phones too; a tap outside closes it.
//! - Dialog: a card centered over a dimmed full-screen presentation; a tap
//!   outside closes it. tvOS has neither sheets nor popovers
//!   (`API_UNAVAILABLE(tvos)`): every overlay is such a card there, over a
//!   blurred screen, and Menu closes it.
//!
//! Escape (hardware keyboard) closes non-fixed overlays; `fixed` sets
//! `modalInPresentation`. A user dismissal reports `UiEvent::Back(id, 0)`.
//!
//! An overlay inserted already open (with its parent, #70) is presented
//! once its anchor is in a window: by the layout that follows
//! (`present_if_pending`). A `manual` or `keep_focus` popover lets taps
//! through to the window around it (passthrough views, so they do not
//! close it); a `keep_focus` one leaves the first responder where it is
//! (the keyboard stays up), taking it back if UIKit moved it.
//!
//! A popover opens on its `side` of the anchor (#113): the matching
//! `permittedArrowDirections`, the opposite one when the window has no room
//! there, and a `sourceRect` on the part of the anchor it lines up with
//! (`PopoverAlign::arrow_span`).

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
#[cfg_attr(target_os = "tvos", allow(unused_imports))]
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSObject, NSObjectProtocol, NSSet};
use objc2_ui_kit::{
    NSObjectUIAccessibility, UIAdaptivePresentationControllerDelegate, UIColor,
    UIModalPresentationStyle, UIPress, UIPressesEvent, UIResponder, UITapGestureRecognizer,
    UITraitCollection, UIView, UIViewController,
};
use prism_core::NodeId;
use prism_core::controls::{OverlayConfig, OverlayKind};
use prism_native::Viewport;

pub struct ControllerIvars {
    id: Cell<NodeId>,
    fixed: Cell<bool>,
    /// A card over a dimmed screen (dialogs; every overlay on tvOS).
    card: Retained<UIView>,
    custom: Cell<bool>,
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
                    self.close_by_user();
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

        /// A tap on the dimmed backdrop, outside the card.
        #[unsafe(method(onBackdrop:))]
        fn on_backdrop(&self, tap: &UITapGestureRecognizer) {
            let card = &self.ivars().card;
            let p = tap.locationInView(Some(card));
            let b = card.bounds();
            let inside = p.x >= 0. && p.y >= 0. && p.x <= b.size.width && p.y <= b.size.height;
            if !inside && !self.ivars().fixed.get() {
                self.close_by_user();
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

    fn close_by_user(&self) {
        let id = self.ivars().id.get();
        if self.presentingViewController().is_some() {
            self.dismissViewControllerAnimated_completion(true, None);
        }
        crate::imp::overlay_closed(id);
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

    unsafe impl UIAdaptivePresentationControllerDelegate for OverlayDelegate {
        #[unsafe(method(presentationControllerDidDismiss:))]
        fn did_dismiss(&self, _pc: &objc2_ui_kit::UIPresentationController) {
            crate::imp::overlay_closed(self.ivars().get());
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
        Overlay {
            anchor,
            vc,
            delegate,
            config: Cell::new(OverlayConfig::default()),
            open: Cell::new(false),
            size: Cell::new((0., 0.)),
            content: Default::default(),
        }
    }

    /// Dialogs everywhere, and every overlay on tvOS, are drawn cards.
    fn custom(&self) -> bool {
        cfg!(target_os = "tvos") || self.config.get().kind == OverlayKind::Dialog
    }

    fn host(&self) -> Retained<UIView> {
        if self.custom() {
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
    }

    fn presented(&self) -> bool {
        self.vc.presentingViewController().is_some()
    }

    /// Shows or closes the overlay, over the topmost presented controller.
    pub fn set_open(&self, open: bool, root: &UIViewController) {
        self.open.set(open);
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
        top.presentViewController_animated_completion(&self.vc, true, Some(&done));
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
            #[cfg(target_os = "tvos")]
            self.vc
                .setModalPresentationStyle(UIModalPresentationStyle::BlurOverFullScreen);
            #[cfg(not(target_os = "tvos"))]
            self.vc
                .setModalPresentationStyle(UIModalPresentationStyle::OverFullScreen);
            self.vc
                .setModalTransitionStyle(objc2_ui_kit::UIModalTransitionStyle::CrossDissolve);
            let dim = if cfg!(target_os = "tvos") { 0. } else { 0.35 };
            view.setBackgroundColor(Some(&UIColor::colorWithWhite_alpha(0., dim)));
            if card.superview().is_none() {
                // White in light mode, the raised grey of alerts in dark mode.
                card.setBackgroundColor(Some(&UIColor::tertiarySystemBackgroundColor()));
                card.layer().setCornerRadius(16.);
                card.setClipsToBounds(true);
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
            view.setBackgroundColor(Some(&UIColor::systemBackgroundColor()));
            #[cfg(not(target_os = "tvos"))]
            match c.kind {
                OverlayKind::Popover => {
                    self.vc
                        .setModalPresentationStyle(UIModalPresentationStyle::Popover);
                    if let Some(pc) = self.vc.popoverPresentationController() {
                        let source = self.anchor.superview();
                        pc.setSourceView(source.as_deref());
                        let window = source.as_ref().and_then(|s| s.window());
                        if let Some(s) = &source {
                            let rtl = s.effectiveUserInterfaceLayoutDirection()
                                == objc2_ui_kit::UIUserInterfaceLayoutDirection::RightToLeft;
                            let room = window
                                .as_ref()
                                .map(|w| (s.convertRect_toView(s.bounds(), Some(w)), w.bounds()));
                            let (rect, arrows) =
                                popover_placement(s.bounds(), self.size.get(), c, rtl, room);
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
                    }
                }
            }
        }
        self.vc.setModalInPresentation(c.fixed);
        if let Some(pc) = self.vc.presentationController() {
            unsafe { pc.setDelegate(Some(ProtocolObject::from_ref(&*self.delegate))) };
        }
    }

    /// The card's frame: the content's size, centered.
    fn place_card(&self) {
        let Some(view) = self.vc.viewIfLoaded() else {
            return;
        };
        let (w, h) = self.size.get();
        let b = view.bounds().size;
        let card = &self.vc.ivars().card;
        let frame = CGRect::new(
            CGPoint::new(((b.width - w) / 2.).max(0.), ((b.height - h) / 2.).max(0.)),
            CGSize::new(w, h),
        );
        if card.frame() != frame {
            card.setFrame(frame);
        }
    }

    /// The content's laid-out size: the popover's or card's size.
    pub fn set_size(&self, w: f64, h: f64) {
        if self.size.get() == (w, h) {
            return;
        }
        self.size.set((w, h));
        if self.custom() {
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
