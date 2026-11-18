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
// Selections (#86)
// ---------------------------------------------------------------------------

thread_local! {
    /// Fields reporting their selection (`on_select`).
    static SELECT_EVENTS: std::cell::RefCell<std::collections::HashSet<NodeId>> = Default::default();
    /// The selection last reported for each field.
    static REPORTED: std::cell::RefCell<std::collections::HashMap<NodeId, prism_core::SelectionEvent>> = Default::default();
    /// Fields whose report (or pending selection) is scheduled for the
    /// next turn (the closure only carries the id: views stay here).
    static SCHEDULED: std::cell::RefCell<std::collections::HashMap<NodeId, Retained<objc2_ui_kit::UIView>>> = Default::default();
    static BEGAN: std::cell::RefCell<std::collections::HashMap<NodeId, Retained<objc2_ui_kit::UITextField>>> = Default::default();
    /// Selections set on text inputs not being edited: applied when the
    /// app focuses them (`cx.focus`), dropped when the user does (their tap
    /// places the caret). Characters.
    static PENDING: std::cell::RefCell<std::collections::HashMap<NodeId, (usize, usize)>> = Default::default();
    /// The input the app asked to focus last.
    static APP_FOCUS: std::cell::Cell<Option<NodeId>> = const { std::cell::Cell::new(None) };
}

/// The selection of `field` (showing `text`) in characters.
fn selection_of<T: objc2_ui_kit::UITextInput + objc2::Message>(
    field: &T,
    text: &str,
) -> Option<prism_core::SelectionEvent> {
    let r = field.selectedTextRange()?;
    let begin = field.beginningOfDocument();
    let at = |p: &objc2_ui_kit::UITextPosition| {
        let units = field.offsetFromPosition_toPosition(&begin, p).max(0) as usize;
        prism_native::fields::utf16_to_chars(text, units)
    };
    Some(prism_core::SelectionEvent::new(
        at(&r.start()),
        at(&r.end()),
    ))
}

/// Where `field`'s caret is (#211): at the start or end of its text, on
/// its first or last visual line (wrapping included), comparing the caret's
/// rectangle with those at the start and end of the text.
fn edges_of<T: objc2_ui_kit::UITextInput + objc2::Message>(
    field: &T,
) -> Option<prism_core::CaretEdges> {
    let r = field.selectedTextRange()?;
    if !r.isEmpty() {
        return Some(prism_core::CaretEdges::default());
    }
    let (begin, end, caret) = (
        field.beginningOfDocument(),
        field.endOfDocument(),
        r.start(),
    );
    let rect = |p: &objc2_ui_kit::UITextPosition| field.caretRectForPosition(p);
    let same_line = |a: objc2_core_foundation::CGRect, b: objc2_core_foundation::CGRect| {
        let h = a.size.height.max(b.size.height).max(1.);
        let mid = |r: objc2_core_foundation::CGRect| r.origin.y + r.size.height / 2.;
        (mid(a) - mid(b)).abs() < h / 2.
    };
    let at = rect(&caret);
    Some(prism_core::CaretEdges {
        start: field.offsetFromPosition_toPosition(&begin, &caret) <= 0,
        end: field.offsetFromPosition_toPosition(&caret, &end) <= 0,
        first_line: same_line(at, rect(&begin)),
        last_line: same_line(at, rect(&end)),
    })
}

/// [`edges_of`] the focused text field or text view.
pub fn current_edges(view: &objc2_ui_kit::UIView) -> Option<prism_core::CaretEdges> {
    if let Some(f) = view.downcast_ref::<objc2_ui_kit::UITextField>() {
        return edges_of(f);
    }
    edges_of(view.downcast_ref::<objc2_ui_kit::UITextView>()?)
}

/// Selects `start..end` (characters, clamped) in `field` showing `text`.
fn select_in<T: objc2_ui_kit::UITextInput + objc2::Message>(
    field: &T,
    text: &str,
    start: usize,
    end: usize,
) {
    let begin = field.beginningOfDocument();
    let at = |c: usize| {
        let units = prism_native::fields::chars_to_utf16(text, c) as isize;
        field.positionFromPosition_offset(&begin, units)
    };
    if let (Some(a), Some(b)) = (at(start), at(end.max(start)))
        && let Some(r) = field.textRangeFromPosition_toPosition(&a, &b)
    {
        field.setSelectedTextRange(Some(&r));
    }
}

/// The selection of a text field or text view, in characters.
pub fn current_selection(view: &objc2_ui_kit::UIView) -> Option<prism_core::SelectionEvent> {
    if let Some(f) = view.downcast_ref::<objc2_ui_kit::UITextField>() {
        let text = f.text().map(|t| t.to_string()).unwrap_or_default();
        return selection_of(f, &text);
    }
    let tv = view.downcast_ref::<objc2_ui_kit::UITextView>()?;
    selection_of(tv, &tv.text().to_string())
}

/// Selects `start..end` (characters) in a text field or text view. A text
/// field not being edited takes it when it begins editing.
pub fn set_selection(view: &objc2_ui_kit::UIView, start: usize, end: usize) {
    if let Some(f) = view.downcast_ref::<objc2_ui_kit::UITextField>() {
        if !f.isEditing() {
            PENDING.with(|p| p.borrow_mut().insert(f.tag() as NodeId, (start, end)));
            return;
        }
        let text = f.text().map(|t| t.to_string()).unwrap_or_default();
        select_in(f, &text, start, end);
    } else if let Some(tv) = view.downcast_ref::<objc2_ui_kit::UITextView>() {
        select_in(tv, &tv.text().to_string(), start, end);
    }
}

/// The app focuses input `id` (`cx.focus`).
pub fn app_focuses(id: NodeId) {
    APP_FOCUS.set(Some(id));
}

/// Text field `field` began editing: if the app focused it, the selection
/// it set meanwhile, after UIKit placed its caret.
pub fn began_editing(field: &objc2_ui_kit::UITextField) {
    let id = field.tag() as NodeId;
    let by_app = APP_FOCUS.get() == Some(id);
    if by_app {
        APP_FOCUS.set(None);
    }
    let pending = PENDING.with(|p| p.borrow_mut().remove(&id));
    let Some((start, end)) = pending.filter(|_| by_app) else {
        return;
    };
    BEGAN.with(|b| b.borrow_mut().insert(id, objc2::Message::retain(field)));
    dispatch2::DispatchQueue::main().exec_async(move || {
        let Some(field) = BEGAN.with(|b| b.borrow_mut().remove(&id)) else {
            return;
        };
        let text = field.text().map(|t| t.to_string()).unwrap_or_default();
        select_in(&*field, &text, start, end);
    });
}

/// Whether field `id` reports its selection (`on_select`).
pub fn set_select_events(id: NodeId, on: bool) {
    SELECT_EVENTS.with(|s| {
        if on {
            s.borrow_mut().insert(id);
        } else {
            s.borrow_mut().remove(&id);
        }
    });
    if !on {
        REPORTED.with(|r| r.borrow_mut().remove(&id));
    }
}

/// Field `id` is gone.
pub fn forget_selection(id: NodeId) {
    set_select_events(id, false);
    PENDING.with(|p| p.borrow_mut().remove(&id));
}

/// The selection of `view` (a text field or view) changed
/// (`textFieldDidChangeSelection:`, `textViewDidChangeSelection:`):
/// reported on the next turn, once UIKit finished the edit (so after its
/// `UiEvent::Input`), at most once per turn and only when it moved.
pub fn selection_changed(view: &objc2_ui_kit::UIView) {
    let id = view.tag() as NodeId;
    // Typing and moving the caret move other people's carets too (#240).
    #[cfg(not(target_os = "tvos"))]
    crate::carets::refresh(id);
    if !SELECT_EVENTS.with(|s| s.borrow().contains(&id)) {
        return;
    }
    let first = SCHEDULED.with(|s| {
        s.borrow_mut()
            .insert(id, objc2::Message::retain(view))
            .is_none()
    });
    if !first {
        return;
    }
    dispatch2::DispatchQueue::main().exec_async(move || {
        let Some(view) = SCHEDULED.with(|s| s.borrow_mut().remove(&id)) else {
            return;
        };
        if !SELECT_EVENTS.with(|s| s.borrow().contains(&id)) {
            return;
        }
        let Some(sel) = current_selection(&view) else {
            return;
        };
        let old = REPORTED.with(|r| r.borrow_mut().insert(id, sel));
        if old != Some(sel) {
            crate::imp::dispatch(UiEvent::Select(id, sel.start, sel.end));
        }
    });
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
        /// The style's line height (#179): kept on the text as it changes.
        pub line_height: std::cell::Cell<Option<f32>>,
        /// The minimum lines (#235).
        pub rows: std::cell::Cell<u32>,
    }

    impl TextArea {
        pub fn new(
            mtm: MainThreadMarker,
            delegate: &objc2::runtime::ProtocolObject<dyn UITextViewDelegate>,
        ) -> TextArea {
            // Copy, cut and paste can go to the app (#217).
            let view = crate::edit::PrismTextView::text_view(mtm);
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
            // The inset holds all of the space around the text (#179): an
            // explicit `p(0.)` puts it where `text()` draws it.
            view.textContainer().setLineFragmentPadding(0.);
            // TextKit 1, like labels: its fixed line heights match theirs.
            let _ = view.layoutManager();
            TextArea {
                view,
                placeholder,
                line_height: std::cell::Cell::new(None),
                rows: std::cell::Cell::new(3),
            }
        }

        /// The text, and what is typed next, take the font, color,
        /// alignment and line height of the style, with the baseline of
        /// `text()` (#179).
        pub fn apply_line_height(&self) {
            use objc2_ui_kit::{
                NSBaselineOffsetAttributeName, NSFontAttributeName, NSForegroundColorAttributeName,
                NSMutableParagraphStyle, NSParagraphStyleAttributeName,
            };
            let v = &self.view;
            let Some(font) = v.font() else { return };
            let color = v.textColor().unwrap_or_else(UIColor::labelColor);
            let lh = self.line_height.get();
            let paragraph = NSMutableParagraphStyle::new();
            paragraph.setAlignment(v.textAlignment());
            if let Some(lh) = lh {
                paragraph.setMinimumLineHeight(lh as f64);
                paragraph.setMaximumLineHeight(lh as f64);
            }
            let offset =
                crate::label::baseline_offset(&font, lh).map(objc2_foundation::NSNumber::new_f64);
            let mut keys: Vec<&objc2_foundation::NSAttributedStringKey> = unsafe {
                vec![
                    NSFontAttributeName,
                    NSForegroundColorAttributeName,
                    NSParagraphStyleAttributeName,
                ]
            };
            let mut values: Vec<&AnyObject> = vec![&font, &color, &paragraph];
            if let Some(o) = &offset {
                keys.push(unsafe { NSBaselineOffsetAttributeName });
                values.push(o);
            }
            let attrs = objc2_foundation::NSDictionary::from_slices(&keys, &values);
            let storage = v.textStorage();
            let all = objc2_foundation::NSRange::new(0, storage.length());
            storage.beginEditing();
            unsafe { storage.setAttributes_range(Some(&attrs), all) };
            storage.endEditing();
            unsafe { v.setTypingAttributes(&attrs) };
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
                self.apply_line_height();
            }
            self.placeholder.setHidden(!text.is_empty());
        }

        /// Content height for `width`, at least three lines.
        pub fn measure(&self, width: f64) -> CGSize {
            let v = &self.view;
            let fit = v.sizeThatFits(CGSize::new(width, f64::MAX / 4.));
            let inset = v.textContainerInset();
            let line = self
                .line_height
                .get()
                .map(f64::from)
                .or_else(|| v.font().map(|f| unsafe { f.lineHeight() }))
                .unwrap_or(20.);
            let min = self.rows.get().max(1) as f64 * line + inset.top + inset.bottom;
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
