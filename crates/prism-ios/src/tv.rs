//! tvOS specifics: the Siri Remote moves a focus between focusable views and
//! "clicks" with its select button. There is no touch, no `UISwitch`.
//!
//! - Clickable containers are `PrismFocusView`s: focusable, and raised
//!   (slightly bigger, with a shadow) while focused, like system cells.
//! - Taps on them listen to the select press only.
//! - Toggles are buttons showing a check mark.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_core_foundation::{CGAffineTransform, CGSize};
use objc2_foundation::{NSArray, NSNumber, NSObjectProtocol, NSString};
use objc2_ui_kit::{
    UIButton, UIButtonType, UIControlEvents, UIControlState, UIFocusAnimationCoordinator,
    UIFocusUpdateContext, UIImage, UIResponder, UITapGestureRecognizer, UIView,
};

/// Prism's reference body text size, in points (the base the iOS backend
/// scales Dynamic Type from).
const PRISM_BODY: f64 = 17.;

/// How much bigger a TV shows everything: the system's body text size over
/// Prism's (`PRISM_ZOOM` overrides it).
pub fn zoom() -> f32 {
    if let Some(z) = std::env::var("PRISM_ZOOM")
        .ok()
        .and_then(|z| z.parse().ok())
    {
        return z;
    }
    let body = unsafe { objc2_ui_kit::UIFontTextStyleBody };
    let size = unsafe { objc2_ui_kit::UIFont::preferredFontForTextStyle(body).pointSize() };
    (size / PRISM_BODY).max(1.) as f32
}

/// `UIPressTypeSelect` (UIPress.h): the remote's click.
const PRESS_SELECT: isize = 4;

#[derive(Default)]
pub struct FocusIvars {
    focusable: Cell<bool>,
}

define_class!(
    #[unsafe(super(UIView, UIResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismFocusView"]
    #[ivars = FocusIvars]
    pub struct FocusView;

    unsafe impl NSObjectProtocol for FocusView {}

    impl FocusView {
        #[unsafe(method(canBecomeFocused))]
        fn can_become_focused(&self) -> bool {
            self.ivars().focusable.get()
        }

        #[unsafe(method(setPrismFocusable:))]
        fn set_prism_focusable(&self, on: bool) {
            self.ivars().focusable.set(on);
        }

        #[unsafe(method(didUpdateFocusInContext:withAnimationCoordinator:))]
        fn did_update_focus(
            &self,
            context: &UIFocusUpdateContext,
            coordinator: &UIFocusAnimationCoordinator,
        ) {
            let _: () = unsafe {
                msg_send![super(self), didUpdateFocusInContext: context, withAnimationCoordinator: coordinator]
            };
            raise(self, self.isFocused());
        }
    }
);

/// Scales the view up with a soft shadow while it has the focus.
fn raise(view: &UIView, focused: bool) {
    let scale = if focused { 1.06 } else { 1. };
    view.setTransform(CGAffineTransform {
        a: scale,
        b: 0.,
        c: 0.,
        d: scale,
        tx: 0.,
        ty: 0.,
    });
    let layer = view.layer();
    layer.setShadowOpacity(if focused { 0.35 } else { 0. });
    layer.setShadowRadius(18.);
    layer.setShadowOffset(CGSize {
        width: 0.,
        height: 12.,
    });
}

/// A container that can take the focus once made clickable.
pub fn focus_view(mtm: MainThreadMarker) -> Retained<UIView> {
    let this = FocusView::alloc(mtm).set_ivars(FocusIvars::default());
    let view: Retained<FocusView> = unsafe { msg_send![super(this), init] };
    Retained::into_super(view)
}

/// Lets the remote focus `view` (only `PrismFocusView`s).
pub fn set_focusable(view: &UIView, on: bool) {
    if view.respondsToSelector(sel!(setPrismFocusable:)) {
        let _: () = unsafe { msg_send![view, setPrismFocusable: on] };
    }
}

/// Taps fire on the remote's select press only (not the arrows or Menu).
pub fn select_press_only(tap: &UITapGestureRecognizer) {
    let select = NSNumber::new_isize(PRESS_SELECT);
    tap.setAllowedPressTypes(&NSArray::from_retained_slice(&[select]));
}

/// A toggle: a button showing an empty or checked circle.
pub fn toggle(mtm: MainThreadMarker, target: &AnyObject) -> Retained<UIButton> {
    let b = UIButton::buttonWithType(UIButtonType::System, mtm);
    let image = |name: &str| UIImage::systemImageNamed(&NSString::from_str(name));
    b.setImage_forState(image("circle").as_deref(), UIControlState::Normal);
    b.setImage_forState(
        image("checkmark.circle.fill").as_deref(),
        UIControlState::Selected,
    );
    unsafe {
        b.addTarget_action_forControlEvents(
            Some(target),
            sel!(onToggleButton:),
            UIControlEvents::PrimaryActionTriggered,
        )
    };
    b
}

pub fn set_toggle(b: &UIButton, on: bool) {
    if b.isSelected() != on {
        b.setSelected(on);
    }
}
