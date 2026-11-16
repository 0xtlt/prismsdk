//! Optional date pickers (#89) on iOS and visionOS. `UIDatePicker` always
//! holds a date, so an optional picker carries two buttons of its own:
//!
//! - while empty (`NaN`), a gray pill with the placeholder covers it (the
//!   picker's own views are hidden); a tap fills it with the current moment
//!   (`DateConfig::start`), the way Contacts adds a birthday, and the
//!   compact picker shows up to be adjusted;
//! - once it has a date, an `xmark.circle.fill` button after the picker
//!   empties it again (labelled "Clear" in the app's language).
//!
//! The buttons are subviews of the picker, laid out with its frame: the
//! node stays one view, so layout, accessibility and styles do not change.
//! Pickers that are not optional get nothing.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, sel};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSDate, NSString};
use objc2_ui_kit::{
    NSObjectUIAccessibility, UIButton, UIButtonConfiguration, UIControlEvents, UIDatePicker,
    UIImage, UIUserInterfaceLayoutDirection, UIView,
};
use prism_core::controls::DateConfig;
use prism_core::{NodeId, UiEvent};

/// Width of the clear button, and the gap before it.
const CLEAR: f64 = 28.;

struct Optional {
    picker: Retained<UIDatePicker>,
    fill: Retained<UIButton>,
    clear: Retained<UIButton>,
    config: DateConfig,
    value: f64,
}

thread_local! {
    static OPTIONAL: RefCell<HashMap<NodeId, Optional>> = RefCell::new(HashMap::new());
    /// "Clear" in the app's language.
    static CLEAR_LABEL: RefCell<String> = RefCell::new("Clear".into());
}

fn with<R>(id: NodeId, f: impl FnOnce(&mut Optional) -> R) -> Option<R> {
    OPTIONAL.with(|o| o.borrow_mut().get_mut(&id).map(f))
}

fn is_ours(o: &Optional, v: &UIView) -> bool {
    let v: *const UIView = v;
    std::ptr::eq(v, Retained::as_ptr(&o.fill).cast())
        || std::ptr::eq(v, Retained::as_ptr(&o.clear).cast())
}

/// Shows the placeholder or the picker, and the clear button.
fn refresh(o: &Optional) {
    let empty = o.value.is_nan();
    for sub in o.picker.subviews().iter() {
        if !is_ours(o, &sub) {
            sub.setHidden(empty);
        }
    }
    o.fill.setHidden(!empty);
    o.clear.setHidden(empty);
}

/// The picker's config: an optional one gets its buttons, a picker that is
/// no longer optional loses them.
pub fn configure(p: &Retained<UIDatePicker>, c: DateConfig, target: &AnyObject) {
    let id = p.tag() as NodeId;
    if !c.optional {
        forget(id);
        return;
    }
    if with(id, |o| o.config = c).is_some() {
        return;
    }
    let mtm = MainThreadMarker::from(&**p);
    let fill = {
        let config = UIButtonConfiguration::grayButtonConfiguration(mtm);
        config.setCornerStyle(objc2_ui_kit::UIButtonConfigurationCornerStyle::Medium);
        let b = UIButton::buttonWithConfiguration_primaryAction(&config, None);
        unsafe {
            b.addTarget_action_forControlEvents(
                Some(target),
                sel!(onDateFill:),
                UIControlEvents::TouchUpInside,
            )
        };
        b
    };
    let clear = {
        let config = UIButtonConfiguration::plainButtonConfiguration(mtm);
        config.setImage(
            UIImage::systemImageNamed(&NSString::from_str("xmark.circle.fill")).as_deref(),
        );
        config.setBaseForegroundColor(Some(&objc2_ui_kit::UIColor::tertiaryLabelColor()));
        config.setContentInsets(objc2_ui_kit::NSDirectionalEdgeInsets {
            top: 0.,
            leading: 0.,
            bottom: 0.,
            trailing: 0.,
        });
        let b = UIButton::buttonWithConfiguration_primaryAction(&config, None);
        unsafe {
            b.addTarget_action_forControlEvents(
                Some(target),
                sel!(onDateClear:),
                UIControlEvents::TouchUpInside,
            )
        };
        CLEAR_LABEL.with(|l| b.setAccessibilityLabel(Some(&NSString::from_str(&l.borrow())), mtm));
        b
    };
    for b in [&fill, &clear] {
        b.setTag(id as isize);
        p.addSubview(b);
    }
    let o = Optional {
        picker: p.clone(),
        fill,
        clear,
        config: c,
        value: p.date().timeIntervalSince1970(),
    };
    refresh(&o);
    layout(&o);
    OPTIONAL.with(|m| m.borrow_mut().insert(id, o));
}

/// The app's value (`NaN`: empty). Returns false for pickers that are not
/// optional.
pub fn set_value(p: &UIDatePicker, value: f64) -> bool {
    with(p.tag() as NodeId, |o| {
        o.value = value;
        refresh(o);
    })
    .is_some()
}

/// The text of the pill shown while empty.
pub fn set_placeholder(p: &UIDatePicker, text: &str) {
    with(p.tag() as NodeId, |o| {
        let text = NSString::from_str(text);
        crate::forms::update_button(&o.fill, |c| c.setTitle(Some(&text)));
    });
}

/// "Clear" in the app's language.
pub fn set_clear_label(text: &str) {
    CLEAR_LABEL.with(|l| *l.borrow_mut() = text.to_string());
    OPTIONAL.with(|o| {
        let label = NSString::from_str(text);
        for o in o.borrow().values() {
            o.clear
                .setAccessibilityLabel(Some(&label), MainThreadMarker::from(&*o.clear));
        }
    });
}

/// The natural size of an optional picker: the pill while empty, the
/// picker and its clear button otherwise. `None`: not optional.
pub fn natural_size(p: &UIDatePicker, picker: CGSize) -> Option<CGSize> {
    with(p.tag() as NodeId, |o| {
        if o.value.is_nan() {
            let pill = o
                .fill
                .sizeThatFits(CGSize::new(f64::MAX / 4., f64::MAX / 4.));
            CGSize::new(pill.width.ceil(), pill.height.max(picker.height).ceil())
        } else {
            CGSize::new(picker.width + CLEAR, picker.height)
        }
    })
}

fn layout(o: &Optional) {
    let b = o.picker.bounds();
    o.fill.setFrame(b);
    let rtl = o.picker.effectiveUserInterfaceLayoutDirection()
        == UIUserInterfaceLayoutDirection::RightToLeft;
    let side = CLEAR.min(b.size.height);
    // After the leading-aligned compact picker.
    let x = if rtl { 0. } else { b.size.width - side };
    o.clear.setFrame(CGRect::new(
        CGPoint::new(x, (b.size.height - side) / 2.),
        CGSize::new(side, side),
    ));
}

/// Lays the buttons out after the picker got its frame.
pub fn framed(p: &UIDatePicker) {
    with(p.tag() as NodeId, |o| layout(o));
}

/// The pill was tapped: the current moment (local time of day for a time
/// of day), within the bounds.
pub fn fill(id: NodeId) {
    let Some(v) = with(id, |o| {
        let offset = objc2_foundation::NSTimeZone::localTimeZone().secondsFromGMT() as f64;
        let v = o
            .config
            .start(NSDate::now().timeIntervalSince1970(), offset);
        o.picker
            .setDate_animated(&NSDate::dateWithTimeIntervalSince1970(v), false);
        o.value = v;
        refresh(o);
        v
    }) else {
        return;
    };
    crate::imp::dispatch(UiEvent::Change(id, v));
}

/// The clear button was tapped.
pub fn clear(id: NodeId) {
    if with(id, |o| {
        o.value = f64::NAN;
        refresh(o);
    })
    .is_some()
    {
        crate::imp::dispatch(UiEvent::Change(id, f64::NAN));
    }
}

/// The node is gone, or no longer optional.
pub fn forget(id: NodeId) {
    if let Some(o) = OPTIONAL.with(|o| o.borrow_mut().remove(&id)) {
        o.fill.removeFromSuperview();
        o.clear.removeFromSuperview();
        for sub in o.picker.subviews().iter() {
            sub.setHidden(false);
        }
    }
}
