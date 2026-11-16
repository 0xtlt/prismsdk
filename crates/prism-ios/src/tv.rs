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
    /// Reports the remote's presses while focused (`on_key_down`).
    keys: Cell<bool>,
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
            self.ivars().focusable.get() || self.ivars().keys.get()
        }

        #[unsafe(method(setPrismFocusable:))]
        fn set_prism_focusable(&self, on: bool) {
            self.ivars().focusable.set(on);
        }

        #[unsafe(method(setPrismKeys:))]
        fn set_prism_keys(&self, on: bool) {
            self.ivars().keys.set(on);
        }

        /// A key pad keeps the focus on left / right: the app gets them.
        #[unsafe(method(shouldUpdateFocusInContext:))]
        fn should_update_focus(&self, context: &UIFocusUpdateContext) -> bool {
            !(self.ivars().keys.get()
                && self.isFocused()
                && context.focusHeading().0 & HEADING_SIDEWAYS != 0)
        }

        #[unsafe(method(pressesBegan:withEvent:))]
        fn presses_began(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            let local = self.ivars().keys.get().then_some(self.tag() as NodeId);
            if local.is_none() || !crate::interaction::handle_presses(presses, self, local, true) {
                let _: () = unsafe { msg_send![super(self), pressesBegan: presses, withEvent: event] };
            }
        }

        #[unsafe(method(pressesEnded:withEvent:))]
        fn presses_ended(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            let local = self.ivars().keys.get().then_some(self.tag() as NodeId);
            if local.is_none() || !crate::interaction::handle_presses(presses, self, local, false) {
                let _: () = unsafe { msg_send![super(self), pressesEnded: presses, withEvent: event] };
            }
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
            // `.focus(|s| …)`, `.hover(|s| …)`: the focused view hovers too.
            crate::decor::tv_focus(self.tag() as NodeId, self.isFocused());
        }
    }
);

/// Scales the view up with a soft shadow while it has the focus.
pub(crate) fn raise(view: &UIView, focused: bool) {
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

/// Lets `view` take the focus and report the remote's presses (only
/// `PrismFocusView`s).
pub fn set_keys(view: &UIView, on: bool) {
    if view.respondsToSelector(sel!(setPrismKeys:)) {
        let _: () = unsafe { msg_send![view, setPrismKeys: on] };
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

// ---------------------------------------------------------------------------
// Sliders and date pickers: UISlider and UIDatePicker are
// API_UNAVAILABLE(tvos) (UISlider.h, UIDatePicker.h). Their stand-in is a
// focusable view adjusted with the remote's left / right presses (and
// VoiceOver's increment / decrement): a track for sliders, the formatted
// date for date pickers. An optional date picker (#89) shows its
// placeholder, dimmed, while empty: the first press fills it with the
// current moment, and Play/Pause empties it again.
// ---------------------------------------------------------------------------

use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::{NSDate, NSDateFormatter, NSDateFormatterStyle, NSLocale, NSSet};
use objc2_ui_kit::{UIColor, UILabel, UIPress, UIPressesEvent, UIProgressView};
use prism_core::controls::{DateConfig, DateMode, SliderConfig};
use prism_core::{NodeId, UiEvent};

/// `UIPressTypeLeftArrow`, `UIPressTypeRightArrow` (UIPress.h).
const PRESS_LEFT: isize = 2;
const PRESS_RIGHT: isize = 3;
/// `UIPressTypePlayPause`: clears an optional date picker.
const PRESS_PLAY_PAUSE: isize = 6;
/// `UIFocusHeadingLeft | UIFocusHeadingRight` (UIFocus.h).
const HEADING_SIDEWAYS: usize = (1 << 2) | (1 << 3);

pub struct AdjustIvars {
    value: Cell<f64>,
    /// `Some`: a date picker; `None`: a slider.
    date: Cell<Option<DateConfig>>,
    slider: Cell<SliderConfig>,
    track: Retained<UIProgressView>,
    label: Retained<UILabel>,
    lang: std::cell::RefCell<String>,
    /// Shown while an optional date picker is empty.
    placeholder: std::cell::RefCell<String>,
}

define_class!(
    #[unsafe(super(UIView, UIResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismAdjustable"]
    #[ivars = AdjustIvars]
    pub struct Adjustable;

    unsafe impl NSObjectProtocol for Adjustable {}

    impl Adjustable {
        #[unsafe(method(canBecomeFocused))]
        fn can_become_focused(&self) -> bool {
            true
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

        /// Left / right adjust the value instead of moving the focus.
        #[unsafe(method(shouldUpdateFocusInContext:))]
        fn should_update_focus(&self, context: &UIFocusUpdateContext) -> bool {
            !(self.isFocused() && context.focusHeading().0 & HEADING_SIDEWAYS != 0)
        }

        #[unsafe(method(pressesBegan:withEvent:))]
        fn presses_began(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            let rtl = self.effectiveUserInterfaceLayoutDirection()
                == objc2_ui_kit::UIUserInterfaceLayoutDirection::RightToLeft;
            let mut handled = false;
            for p in presses.iter() {
                let step = match p.r#type().0 {
                    PRESS_LEFT => -1.,
                    PRESS_RIGHT => 1.,
                    PRESS_PLAY_PAUSE if self.clearable() => {
                        self.clear();
                        handled = true;
                        continue;
                    }
                    _ => continue,
                };
                // A right-to-left slider fills from the right.
                self.adjust(if rtl && self.ivars().date.get().is_none() { -step } else { step });
                handled = true;
            }
            if !handled {
                let _: () = unsafe { msg_send![super(self), pressesBegan: presses, withEvent: event] };
            }
        }

        #[unsafe(method(layoutSubviews))]
        fn layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            let b = self.bounds();
            self.ivars().label.setFrame(b);
            let h = 8.;
            self.ivars().track.setFrame(CGRect::new(
                CGPoint::new(0., (b.size.height - h) / 2.),
                CGSize { width: b.size.width, height: h },
            ));
        }

        #[unsafe(method(isAccessibilityElement))]
        fn is_accessibility_element(&self) -> bool {
            true
        }

        #[unsafe(method(accessibilityTraits))]
        fn accessibility_traits(&self) -> u64 {
            unsafe { objc2_ui_kit::UIAccessibilityTraitAdjustable }
        }

        /// The app's read-out (`setAccessibilityValue:`), else the date shown.
        #[unsafe(method_id(accessibilityValue))]
        fn accessibility_value(&self) -> Option<Retained<NSString>> {
            let own: Option<Retained<NSString>> = unsafe { msg_send![super(self), accessibilityValue] };
            own.or_else(|| self.ivars().label.text())
        }

        #[unsafe(method(accessibilityIncrement))]
        fn accessibility_increment(&self) {
            self.adjust(1.);
        }

        #[unsafe(method(accessibilityDecrement))]
        fn accessibility_decrement(&self) {
            self.adjust(-1.);
        }
    }
);

impl Adjustable {
    fn make(mtm: MainThreadMarker, date: Option<DateConfig>) -> Retained<Self> {
        let track = UIProgressView::new(mtm);
        let label = UILabel::new(mtm);
        let this = Self::alloc(mtm).set_ivars(AdjustIvars {
            value: Cell::new(0.),
            date: Cell::new(date),
            slider: Cell::new(SliderConfig::default()),
            track: track.clone(),
            label: label.clone(),
            lang: Default::default(),
            placeholder: Default::default(),
        });
        let v: Retained<Self> = unsafe { msg_send![super(this), init] };
        if date.is_some() {
            v.addSubview(&label);
        } else {
            track.setUserInteractionEnabled(false);
            v.addSubview(&track);
        }
        v
    }

    pub fn slider(mtm: MainThreadMarker) -> Retained<Self> {
        Self::make(mtm, None)
    }

    pub fn date_picker(mtm: MainThreadMarker) -> Retained<Self> {
        Self::make(mtm, Some(DateConfig::default()))
    }

    pub fn set_slider_config(&self, c: SliderConfig) {
        self.ivars().slider.set(c);
        self.show();
    }

    pub fn set_date_config(&self, c: DateConfig) {
        self.ivars().date.set(Some(c));
        self.show();
    }

    pub fn set_lang(&self, lang: &str) {
        *self.ivars().lang.borrow_mut() = lang.to_string();
        self.show();
    }

    pub fn set_value(&self, v: f64) {
        self.ivars().value.set(v);
        self.show();
    }

    pub fn set_placeholder(&self, text: &str) {
        *self.ivars().placeholder.borrow_mut() = text.to_string();
        self.show();
    }

    /// An optional date picker with a value: Play/Pause empties it.
    fn clearable(&self) -> bool {
        self.ivars().date.get().is_some_and(|c| c.optional) && !self.ivars().value.get().is_nan()
    }

    fn clear(&self) {
        self.ivars().value.set(f64::NAN);
        self.show();
        crate::imp::dispatch(UiEvent::Change(self.tag() as NodeId, f64::NAN));
    }

    /// Tint: the track's fill, or the date's text color.
    pub fn set_tint(&self, color: Option<&UIColor>) {
        self.ivars().track.setProgressTintColor(color);
        let text = color.map_or_else(UIColor::labelColor, objc2::Message::retain);
        unsafe { self.ivars().label.setTextColor(Some(&text)) };
    }

    pub fn set_font(&self, font: &objc2_ui_kit::UIFont) {
        unsafe { self.ivars().label.setFont(Some(font)) };
    }

    pub fn set_alignment(&self, a: objc2_ui_kit::NSTextAlignment) {
        self.ivars().label.setTextAlignment(a);
    }

    /// Natural size: the formatted date, or a 400 pt track.
    pub fn natural_size(&self) -> CGSize {
        if self.ivars().date.get().is_some() {
            let s = self.ivars().label.intrinsicContentSize();
            CGSize {
                width: s.width + 24.,
                height: s.height + 16.,
            }
        } else {
            CGSize {
                width: 400.,
                height: 40.,
            }
        }
    }

    fn adjust(&self, direction: f64) {
        let v = self.ivars().value.get();
        // An empty date picker: the first press fills it with now.
        if let (Some(c), true) = (self.ivars().date.get(), v.is_nan()) {
            let offset = objc2_foundation::NSTimeZone::localTimeZone().secondsFromGMT() as f64;
            let next = c.start(NSDate::now().timeIntervalSince1970(), offset);
            self.ivars().value.set(next);
            self.show();
            crate::imp::dispatch(UiEvent::Change(self.tag() as NodeId, next));
            return;
        }
        let next = match self.ivars().date.get() {
            // A day, a quarter hour, an hour.
            Some(c) => c.clamp(
                v + direction
                    * match c.mode {
                        DateMode::Date => 86_400.,
                        DateMode::Time | DateMode::TimeOfDay => 900.,
                        DateMode::DateTime => 3_600.,
                    },
            ),
            None => {
                let c = self.ivars().slider.get();
                c.snap(v + direction * c.a11y_step())
            }
        };
        // A time of day wraps around midnight.
        let next = match self.ivars().date.get() {
            Some(c) if c.mode == DateMode::TimeOfDay => next.rem_euclid(86_400.),
            _ => next,
        };
        if next != v {
            self.ivars().value.set(next);
            self.show();
            crate::imp::dispatch(UiEvent::Change(self.tag() as NodeId, next));
        }
    }

    fn show(&self) {
        let v = self.ivars().value.get();
        match self.ivars().date.get() {
            Some(_) if v.is_nan() => {
                let text = NSString::from_str(&self.ivars().placeholder.borrow());
                self.ivars().label.setText(Some(&text));
                self.ivars().label.setAlpha(0.5);
            }
            Some(c) => {
                self.ivars().label.setAlpha(1.);
                let f = NSDateFormatter::new();
                let lang = self.ivars().lang.borrow();
                if !lang.is_empty() {
                    f.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(
                        &NSString::from_str(&lang),
                    )));
                }
                let (date, time) = match c.mode {
                    DateMode::Date => (
                        NSDateFormatterStyle::MediumStyle,
                        NSDateFormatterStyle::NoStyle,
                    ),
                    DateMode::Time | DateMode::TimeOfDay => (
                        NSDateFormatterStyle::NoStyle,
                        NSDateFormatterStyle::ShortStyle,
                    ),
                    DateMode::DateTime => (
                        NSDateFormatterStyle::MediumStyle,
                        NSDateFormatterStyle::ShortStyle,
                    ),
                };
                f.setDateStyle(date);
                f.setTimeStyle(time);
                // A time of day is the same in every zone: shown in UTC.
                if c.mode == DateMode::TimeOfDay {
                    f.setTimeZone(Some(
                        &objc2_foundation::NSTimeZone::timeZoneForSecondsFromGMT(0),
                    ));
                }
                let text = f.stringFromDate(&NSDate::dateWithTimeIntervalSince1970(v));
                self.ivars().label.setText(Some(&text));
            }
            None => {
                let c = self.ivars().slider.get();
                self.ivars().track.setProgress(c.fraction(v) as f32);
            }
        }
    }
}
