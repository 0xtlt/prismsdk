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
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_core_foundation::{CGAffineTransform, CGSize};
use objc2_foundation::{NSArray, NSNumber, NSObjectProtocol, NSRange, NSString};
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
    /// A table or one of its cells (#284).
    table: crate::table::TableIvars,
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
            if self.takes_focus() {
                !self.yields_focus()
            } else {
                self.stands_for_row()
            }
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
            if self.click_row(presses) {
                return;
            }
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

        /// VoiceOver's activation of a table's cell clicks its row when the
        /// row is clickable (#284), as on iOS.
        #[unsafe(method(accessibilityActivate))]
        fn accessibility_activate(&self) -> bool {
            match crate::table::row_of(&self.ivars().table)
                .filter(|r| table_row_clickable(r))
            {
                Some(row) => {
                    crate::imp::dispatch(UiEvent::Click(row.tag() as NodeId));
                    true
                }
                None => unsafe { msg_send![super(self), accessibilityActivate] },
            }
        }

        // Tables (#284): `UIAccessibilityContainerDataTable` and its cells,
        // see `table.rs`.
        #[unsafe(method(accessibilityRowCount))]
        fn accessibility_row_count(&self) -> isize {
            crate::table::row_count(&self.ivars().table)
        }

        #[unsafe(method(accessibilityColumnCount))]
        fn accessibility_column_count(&self) -> isize {
            crate::table::column_count(&self.ivars().table)
        }

        #[unsafe(method_id(accessibilityDataTableCellElementForRow:column:))]
        fn accessibility_cell(&self, row: usize, column: usize) -> Option<Retained<AnyObject>> {
            crate::table::cell(self.mtm(), &self.ivars().table, row, column)
        }

        #[unsafe(method_id(accessibilityHeaderElementsForColumn:))]
        fn accessibility_headers(&self, column: usize) -> Option<Retained<NSArray<AnyObject>>> {
            crate::table::headers(self.mtm(), &self.ivars().table, column)
        }

        /// A cell's row and column; a clickable view inside a cell, which
        /// the focus lands on, answers with the cell's.
        #[unsafe(method(accessibilityRowRange))]
        fn accessibility_row_range(&self) -> NSRange {
            let own = crate::table::row_range(&self.ivars().table);
            if own.length > 0 || !self.takes_focus() {
                return own;
            }
            crate::table::enclosing_range(self, false)
        }

        #[unsafe(method(accessibilityColumnRange))]
        fn accessibility_column_range(&self) -> NSRange {
            let own = crate::table::column_range(&self.ivars().table);
            if own.length > 0 || !self.takes_focus() {
                return own;
            }
            crate::table::enclosing_range(self, true)
        }

        // A cell holding a control is a container of its views, which the
        // remote focuses.
        #[unsafe(method(isAccessibilityElement))]
        fn is_accessibility_element(&self) -> bool {
            let own: bool = unsafe { msg_send![super(self), isAccessibilityElement] };
            own && !crate::table::has_parts(self.mtm(), self, &self.ivars().table)
        }

        #[unsafe(method_id(accessibilityElements))]
        fn accessibility_elements(&self) -> Option<Retained<NSArray>> {
            crate::table::parts_array(self.mtm(), self, &self.ivars().table)
                .or_else(|| unsafe { msg_send![super(self), accessibilityElements] })
        }

        #[unsafe(method(accessibilityElementCount))]
        fn accessibility_element_count(&self) -> isize {
            match crate::table::parts(self.mtm(), self, &self.ivars().table) {
                Some(parts) => parts.len() as isize,
                None => unsafe { msg_send![super(self), accessibilityElementCount] },
            }
        }

        #[unsafe(method_id(accessibilityElementAtIndex:))]
        fn accessibility_element_at(&self, index: isize) -> Option<Retained<AnyObject>> {
            match crate::table::parts(self.mtm(), self, &self.ivars().table) {
                Some(parts) => usize::try_from(index).ok().and_then(|i| parts.get(i).cloned()),
                None => unsafe { msg_send![super(self), accessibilityElementAtIndex: index] },
            }
        }

        #[unsafe(method(indexOfAccessibilityElement:))]
        fn index_of_accessibility_element(&self, element: &AnyObject) -> isize {
            match crate::table::parts(self.mtm(), self, &self.ivars().table) {
                Some(parts) => parts
                    .iter()
                    .position(|p| std::ptr::eq(&**p, element))
                    .map_or(objc2_foundation::NSNotFound, |i| i as isize),
                None => unsafe { msg_send![super(self), indexOfAccessibilityElement: element] },
            }
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

/// Whether a table's row takes clicks (it is then focusable).
fn table_row_clickable(row: &UIView) -> bool {
    row.downcast_ref::<FocusView>()
        .is_some_and(|r| r.ivars().focusable.get())
}

// The focus engine never moves into a focusable view, so a clickable table
// row whose cells hold a control (a button in its last column) would hide
// that control from the remote (#284). Such a row hands the focus to its
// cells instead: its controls take their own, and each of its other cells
// takes the focus for the row and clicks it on select.

/// Whether `view` or a view below it takes the focus on its own.
fn holds_focusable(view: &UIView) -> bool {
    view.subviews().iter().any(|v| {
        !v.isHidden()
            && match v.downcast_ref::<FocusView>() {
                Some(f) => f.takes_focus() || holds_focusable(&v),
                None => v.canBecomeFocused() || holds_focusable(&v),
            }
    })
}

impl FocusView {
    /// Clickable, or listening to the remote's keys.
    fn takes_focus(&self) -> bool {
        self.ivars().focusable.get() || self.ivars().keys.get()
    }

    /// A clickable table row with a control in a cell.
    fn yields_focus(&self) -> bool {
        crate::table::is_row(&self.ivars().table) && holds_focusable(self)
    }

    /// The row this cell takes the focus for (see above).
    fn yielding_row(&self) -> Option<Retained<UIView>> {
        crate::table::row_of(&self.ivars().table).filter(|row| {
            row.downcast_ref::<FocusView>()
                .is_some_and(|r| r.ivars().focusable.get() && r.yields_focus())
        })
    }

    fn stands_for_row(&self) -> bool {
        self.yielding_row().is_some() && !holds_focusable(self)
    }

    /// The select press on a cell standing for its row clicks the row (not
    /// a press a control in the cell passed on).
    fn click_row(&self, presses: &NSSet<UIPress>) -> bool {
        if self.takes_focus()
            || !self.isFocused()
            || !presses.iter().any(|p| p.r#type().0 == PRESS_SELECT)
        {
            return false;
        }
        let Some(row) = self.yielding_row() else {
            return false;
        };
        crate::imp::dispatch(UiEvent::Click(row.tag() as NodeId));
        true
    }
}

/// The table ivars of `view`, if it is a Prism container (#284).
pub(crate) fn table_ivars(view: &UIView) -> Option<&crate::table::TableIvars> {
    view.downcast_ref::<FocusView>().map(|v| &v.ivars().table)
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

/// A clickable container's tap hears the select press only while the
/// container holds the focus itself: not while a control in one of its
/// cells does (a clickable table row hands the focus to its cells, #284),
/// which acts on its own.
pub fn receives_press(g: &objc2_ui_kit::UIGestureRecognizer, press: &UIPress) -> bool {
    let (Some(view), Some(responder)) = (g.view(), press.responder(g.mtm())) else {
        return true;
    };
    let view: &UIResponder = &view;
    std::ptr::eq(&*responder, view)
}

/// Taps fire on the remote's select press only (not the arrows or Menu).
pub fn select_press_only(tap: &UITapGestureRecognizer) {
    let select = NSNumber::new_isize(PRESS_SELECT);
    tap.setAllowedPressTypes(&NSArray::from_retained_slice(&[select]));
}

define_class!(
    /// A button (and a toggle): in a table's cell, where the focus lands on
    /// it, it answers VoiceOver with the cell's row and column (#284).
    #[unsafe(super(UIButton, objc2_ui_kit::UIControl, UIView, UIResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismTVButton"]
    pub struct TvButton;

    unsafe impl NSObjectProtocol for TvButton {}

    impl TvButton {
        #[unsafe(method(accessibilityRowRange))]
        fn accessibility_row_range(&self) -> NSRange {
            crate::table::enclosing_range(self, false)
        }

        #[unsafe(method(accessibilityColumnRange))]
        fn accessibility_column_range(&self) -> NSRange {
            crate::table::enclosing_range(self, true)
        }
    }
);

/// A system button (see [`TvButton`]).
pub fn button(_mtm: MainThreadMarker) -> Retained<UIButton> {
    // `buttonWithType:` makes an instance of the class it is sent to.
    let b: Retained<UIButton> =
        unsafe { msg_send![TvButton::class(), buttonWithType: UIButtonType::System] };
    // tvOS's tint is white in both appearances, and an unfocused system
    // button draws its title and image in it: white on its pale platter in
    // light mode. The label color reads in both (focused, the system draws
    // dark on white).
    unsafe { b.setTintColor(Some(&UIColor::labelColor())) };
    b
}

/// A text field's placeholder, in the placeholder color of the field's own
/// appearance: tvOS draws a plain placeholder in the system's, white on a
/// light theme's field when the system is dark.
pub fn set_placeholder(f: &objc2_ui_kit::UITextField, text: &str) {
    use objc2::AllocAnyThread;
    use objc2_foundation::{NSMutableAttributedString, NSRange};
    let s = NSMutableAttributedString::initWithString(
        NSMutableAttributedString::alloc(),
        &NSString::from_str(text),
    );
    let color = UIColor::placeholderTextColor();
    unsafe {
        s.addAttribute_value_range(
            objc2_ui_kit::NSForegroundColorAttributeName,
            &color,
            NSRange::new(0, s.length()),
        )
    };
    f.setAttributedPlaceholder(Some(&s));
}

/// A toggle: a button showing an empty or checked circle.
pub fn toggle(mtm: MainThreadMarker, target: &AnyObject) -> Retained<UIButton> {
    let b = button(mtm);
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
