//! Form controls: `UISlider`, a `UIButton` menu for pickers, `UIDatePicker`,
//! `UITextView` for text areas, text input traits, and screen reader
//! announcements. tvOS has no slider, date picker or editable text view (the
//! SDK marks them `API_UNAVAILABLE(tvos)`): see `tv.rs` for its stand-ins.

use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_foundation::{NSArray, NSString};
use objc2_ui_kit::{
    UIAction, UIButton, UIButtonConfiguration, UIMenu, UIMenuElement, UIMenuElementState,
    UIMenuOptions, UITextAutocapitalizationType, UITextAutocorrectionType, UITextSpellCheckingType,
};
use prism_core::controls::{Capitalize, InputConfig, Keyboard, ReturnKey};
use prism_core::{NodeId, UiEvent};

// ---------------------------------------------------------------------------
// Text written by the app into a field (#73)
// ---------------------------------------------------------------------------

/// Writes the app's `text` into a text field or view showing something
/// else (events reach the app synchronously, so an echo of the typing
/// equals the field and never gets here), even while it is being edited:
/// the caret keeps its distance from the end.
pub fn set_field_text<T: objc2_ui_kit::UITextInput + objc2::Message>(
    field: &T,
    set: impl FnOnce(),
) {
    let from_end = field
        .selectedTextRange()
        .map(|r| field.offsetFromPosition_toPosition(&r.end(), &field.endOfDocument()));
    set();
    let Some(from_end) = from_end else { return };
    let end = field.endOfDocument();
    let at = field
        .positionFromPosition_offset(&end, -from_end.max(0))
        .unwrap_or_else(|| field.beginningOfDocument());
    if let Some(r) = field.textRangeFromPosition_toPosition(&at, &at) {
        field.setSelectedTextRange(Some(&r));
    }
}

// ---------------------------------------------------------------------------
// Slider (iOS, visionOS)
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "tvos"))]
pub use slider::PrismSlider;

#[cfg(not(target_os = "tvos"))]
mod slider {
    use super::*;
    use std::cell::Cell;

    use objc2::{DefinedClass, define_class, msg_send};
    use objc2_foundation::NSObjectProtocol;
    use objc2_ui_kit::{UIControl, UIControlEvents, UIResponder, UISlider, UIView};
    use prism_core::controls::SliderConfig;

    pub struct SliderIvars {
        config: Cell<SliderConfig>,
        /// The last value reported to the app (reports only real changes).
        reported: Cell<f64>,
    }

    define_class!(
        /// A `UISlider` that snaps to its step and reports changes; VoiceOver
        /// adjusts it by `SliderConfig::a11y_step`.
        #[unsafe(super(UISlider, UIControl, UIView, UIResponder, objc2_foundation::NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "PrismSlider"]
        #[ivars = SliderIvars]
        pub struct PrismSlider;

        unsafe impl NSObjectProtocol for PrismSlider {}

        impl PrismSlider {
            #[unsafe(method(prismChanged:))]
            fn prism_changed(&self, _sender: &AnyObject) {
                let c = self.ivars().config.get();
                let v = c.snap(self.value() as f64);
                if c.step > 0. && (self.value() as f64 - v).abs() > f64::EPSILON {
                    self.setValue(v as f32);
                }
                self.report(v);
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

    impl PrismSlider {
        pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(SliderIvars {
                config: Cell::new(SliderConfig::default()),
                reported: Cell::new(f64::NAN),
            });
            let s: Retained<Self> = unsafe { msg_send![super(this), init] };
            let target: &AnyObject = &s;
            unsafe {
                s.addTarget_action_forControlEvents(
                    Some(target),
                    sel!(prismChanged:),
                    UIControlEvents::ValueChanged,
                )
            };
            s
        }

        pub fn configure(&self, config: SliderConfig) {
            self.ivars().config.set(config);
            self.setMinimumValue(config.min as f32);
            self.setMaximumValue(config.max as f32);
        }

        /// The app's value (no report).
        pub fn set(&self, value: f64) {
            self.ivars().reported.set(value);
            if (self.value() as f64 - value).abs() > 1e-6 {
                self.setValue(value as f32);
            }
        }

        fn adjust(&self, direction: f64) {
            let c = self.ivars().config.get();
            let v = c.snap(self.value() as f64 + direction * c.a11y_step());
            self.setValue(v as f32);
            self.report(v);
        }

        fn report(&self, v: f64) {
            if self.ivars().reported.get() != v {
                self.ivars().reported.set(v);
                crate::imp::dispatch(UiEvent::Change(self.tag() as NodeId, v));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Picker: a button showing the selection, whose menu lists the options
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct PickerState {
    pub options: Vec<String>,
    pub placeholder: String,
    pub selected: Option<usize>,
}

/// A borderless pop-up button (tvOS 17+, iOS 14+: `UIButton.menu`,
/// `showsMenuAsPrimaryAction`).
pub fn picker(mtm: MainThreadMarker) -> Retained<UIButton> {
    let config = UIButtonConfiguration::plainButtonConfiguration(mtm);
    // The title lines up with the text around it.
    config.setContentInsets(objc2_ui_kit::NSDirectionalEdgeInsets {
        top: 6.,
        leading: 0.,
        bottom: 6.,
        trailing: 0.,
    });
    // `indicator` is iOS 16+ only (UIButtonConfiguration.h); tvOS gets a
    // chevron image instead.
    #[cfg(not(target_os = "tvos"))]
    config.setIndicator(objc2_ui_kit::UIButtonConfigurationIndicator::Popup);
    #[cfg(target_os = "tvos")]
    {
        config.setImage(
            objc2_ui_kit::UIImage::systemImageNamed(&NSString::from_str("chevron.up.chevron.down"))
                .as_deref(),
        );
        config.setImagePlacement(objc2_ui_kit::NSDirectionalRectEdge::Trailing);
        config.setImagePadding(8.);
    }
    let b = UIButton::buttonWithConfiguration_primaryAction(&config, None);
    b.setShowsMenuAsPrimaryAction(true);
    b
}

/// The button's configuration, changed by `f`.
pub fn update_button(b: &UIButton, f: impl FnOnce(&UIButtonConfiguration)) {
    let mtm = b.mtm();
    let config = b
        .configuration()
        .unwrap_or_else(|| UIButtonConfiguration::plainButtonConfiguration(mtm));
    f(&config);
    b.setConfiguration(Some(&config));
}

/// Shows the selection (or the placeholder) and rebuilds the menu.
pub fn refresh_picker(b: &UIButton, p: &PickerState) {
    let mtm = b.mtm();
    let id = b.tag() as NodeId;
    let selected = p.selected.and_then(|i| p.options.get(i));
    let title = selected.cloned().unwrap_or_else(|| p.placeholder.clone());
    update_button(b, |c| c.setTitle(Some(&NSString::from_str(&title))));
    let items: Vec<Retained<UIMenuElement>> = p
        .options
        .iter()
        .enumerate()
        .map(|(i, option)| {
            let handler = block2::RcBlock::new(move |_: NonNull<UIAction>| {
                crate::imp::dispatch(UiEvent::Change(id, i as f64))
            });
            let action = unsafe {
                UIAction::actionWithTitle_image_identifier_handler(
                    &NSString::from_str(option),
                    None,
                    None,
                    &*handler as *const _ as *mut _,
                    mtm,
                )
            };
            if p.selected == Some(i) {
                action.setState(UIMenuElementState::On);
            }
            Retained::into_super(action)
        })
        .collect();
    let menu = UIMenu::menuWithTitle_image_identifier_options_children(
        &NSString::from_str(""),
        None,
        None,
        // One checkable choice (iOS 15+, tvOS 15+).
        UIMenuOptions::SingleSelection,
        &NSArray::from_retained_slice(&items),
        mtm,
    );
    b.setMenu(Some(&menu));
}

// ---------------------------------------------------------------------------
// Text area (not tvOS: UITextView.editable is API_UNAVAILABLE(tvos))
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "tvos"))]
pub use text_area::{TextArea, changed as text_area_changed};

#[cfg(not(target_os = "tvos"))]
mod text_area {
    use super::*;
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_ui_kit::NSObjectUIAccessibility;
    use objc2_ui_kit::{UIColor, UILabel, UITextView, UITextViewDelegate};

    /// `viewWithTag:` of the placeholder label (Prism views have tags > 0).
    const PLACEHOLDER_TAG: isize = -7;

    pub struct TextArea {
        pub view: Retained<UITextView>,
        pub placeholder: Retained<UILabel>,
    }

    impl TextArea {
        pub fn new(
            mtm: MainThreadMarker,
            delegate: &objc2::runtime::ProtocolObject<dyn UITextViewDelegate>,
        ) -> TextArea {
            let view = UITextView::new(mtm);
            unsafe { view.setDelegate(Some(delegate)) };
            view.setBackgroundColor(Some(&UIColor::clearColor()));
            let placeholder = UILabel::new(mtm);
            placeholder.setTag(PLACEHOLDER_TAG);
            placeholder.setNumberOfLines(0);
            unsafe { placeholder.setTextColor(Some(&UIColor::placeholderTextColor())) };
            placeholder.setUserInteractionEnabled(false);
            // VoiceOver reads the field's label, not this.
            placeholder.setIsAccessibilityElement(false, mtm);
            view.addSubview(&placeholder);
            TextArea { view, placeholder }
        }

        /// Places the placeholder where the first line of text starts.
        pub fn layout_placeholder(&self) {
            let v = &self.view;
            let inset = v.textContainerInset();
            let pad = v.textContainer().lineFragmentPadding();
            let w = (v.bounds().size.width - inset.left - inset.right - 2. * pad).max(0.);
            let size = self.placeholder.sizeThatFits(CGSize::new(w, f64::MAX / 4.));
            self.placeholder.setFrame(CGRect::new(
                CGPoint::new(inset.left + pad, inset.top),
                CGSize::new(w, size.height),
            ));
        }

        pub fn set_text(&self, text: &str) {
            let current = self.view.text().to_string();
            if current != text {
                super::set_field_text(&*self.view, || {
                    self.view.setText(Some(&NSString::from_str(text)))
                });
            }
            self.placeholder.setHidden(!text.is_empty());
        }

        /// Content height for `width`, at least three lines.
        pub fn measure(&self, width: f64) -> CGSize {
            let v = &self.view;
            let fit = v.sizeThatFits(CGSize::new(width, f64::MAX / 4.));
            let inset = v.textContainerInset();
            let line = v.font().map_or(20., |f| unsafe { f.lineHeight() });
            let min = 3. * line + inset.top + inset.bottom;
            CGSize::new(width, fit.height.max(min))
        }
    }

    /// Called by the delegate as the user types.
    pub fn changed(view: &UITextView) {
        let text = view.text().to_string();
        if let Some(p) = view.viewWithTag(PLACEHOLDER_TAG) {
            p.setHidden(!text.is_empty());
        }
        crate::imp::dispatch(UiEvent::Input(view.tag() as NodeId, text));
    }
}

// ---------------------------------------------------------------------------
// Text input traits
// ---------------------------------------------------------------------------

/// Keyboard, return key, capitalization, correction and secure entry
/// (`UITextInputTraits`, adopted by `UITextField` and `UITextView`).
pub fn apply_input_config(field: &AnyObject, config: &str) {
    let c = InputConfig::decode(config);
    use objc2_ui_kit::{UIKeyboardType as K, UIReturnKeyType as R};
    set_bool(field, sel!(setSecureTextEntry:), c.secure);
    let keyboard = match c.keyboard {
        Keyboard::Default => K::Default,
        Keyboard::Email => K::EmailAddress,
        Keyboard::Number => K::NumberPad,
        Keyboard::Decimal => K::DecimalPad,
        Keyboard::Phone => K::PhonePad,
        Keyboard::Url => K::URL,
    };
    set_int(field, sel!(setKeyboardType:), keyboard.0);
    let return_key = match c.return_key {
        ReturnKey::Default => R::Default,
        ReturnKey::Done => R::Done,
        ReturnKey::Go => R::Go,
        ReturnKey::Next => R::Next,
        ReturnKey::Search => R::Search,
        ReturnKey::Send => R::Send,
    };
    set_int(field, sel!(setReturnKeyType:), return_key.0);
    // Secure entry: no capitalization, correction or spell checking.
    let (capitalize, correct) = if c.secure {
        (Capitalize::None, false)
    } else {
        (c.capitalize, c.autocorrect)
    };
    let capitalize = match capitalize {
        Capitalize::Sentences => UITextAutocapitalizationType::Sentences,
        Capitalize::None => UITextAutocapitalizationType::None,
        Capitalize::Words => UITextAutocapitalizationType::Words,
        Capitalize::All => UITextAutocapitalizationType::AllCharacters,
    };
    set_int(field, sel!(setAutocapitalizationType:), capitalize.0);
    let (correction, spelling) = if correct {
        (
            UITextAutocorrectionType::Default,
            UITextSpellCheckingType::Default,
        )
    } else {
        (UITextAutocorrectionType::No, UITextSpellCheckingType::No)
    };
    set_int(field, sel!(setAutocorrectionType:), correction.0);
    set_int(field, sel!(setSpellCheckingType:), spelling.0);
}

// UIKit forwards most text input traits of a text field to an internal
// object: the runtime has no such method on `UITextField` itself, so objc2's
// debug check of `msg_send!` rejects them. They are sent unchecked.

fn set_int(obj: &AnyObject, sel: Sel, value: isize) {
    unsafe {
        let send: unsafe extern "C-unwind" fn(&AnyObject, Sel, isize) =
            std::mem::transmute(objc2::ffi::objc_msgSend as *const ());
        send(obj, sel, value);
    }
}

fn set_bool(obj: &AnyObject, sel: Sel, value: bool) {
    unsafe {
        let send: unsafe extern "C-unwind" fn(&AnyObject, Sel, objc2::runtime::Bool) =
            std::mem::transmute(objc2::ffi::objc_msgSend as *const ());
        send(obj, sel, objc2::runtime::Bool::new(value));
    }
}

// ---------------------------------------------------------------------------
// Announcements (Role::Status)
// ---------------------------------------------------------------------------

/// Has VoiceOver read `message` now (`UIAccessibilityAnnouncementNotification`).
pub fn announce(message: &str) {
    let s = NSString::from_str(message);
    let arg: &AnyObject = &s;
    unsafe {
        objc2_ui_kit::UIAccessibilityPostNotification(
            objc2_ui_kit::UIAccessibilityAnnouncementNotification,
            Some(arg),
        )
    };
}
