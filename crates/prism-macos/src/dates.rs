//! Optional date pickers (#89). `NSDatePicker` always holds a date, so an
//! optional picker carries two borderless buttons of its own, inside its
//! bezel like a search field's:
//!
//! - while empty (`NaN`), the date's text is hidden and the placeholder
//!   shows in the placeholder color; a click fills the picker with the
//!   current moment (`DateConfig::start`), which then edits as usual
//!   (typing or the stepper fill it too);
//! - once it has a date, an `xmark.circle.fill` button at its trailing end
//!   empties it again ("Clear" in the app's language for VoiceOver).
//!
//! Pickers that are not optional get nothing.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSAccessibility, NSButton, NSCellImagePosition, NSColor, NSControl, NSDatePicker, NSImage,
    NSTextAlignment, NSUserInterfaceLayoutDirection,
};
use objc2_foundation::{NSDate, NSPoint, NSRect, NSSize, NSString};
use prism_core::controls::DateConfig;
use prism_core::{NodeId, UiEvent};

/// Side of the clear button, and its inset from the bezel's edge.
const CLEAR: f64 = 16.;
const INSET: f64 = 4.;

struct Optional {
    picker: Retained<NSDatePicker>,
    fill: Retained<NSButton>,
    clear: Retained<NSButton>,
    config: DateConfig,
    value: f64,
}

thread_local! {
    static OPTIONAL: RefCell<HashMap<NodeId, Optional>> = RefCell::new(HashMap::new());
    static CLEAR_LABEL: RefCell<String> = RefCell::new("Clear".into());
}

fn with<R>(id: NodeId, f: impl FnOnce(&mut Optional) -> R) -> Option<R> {
    OPTIONAL.with(|o| o.borrow_mut().get_mut(&id).map(f))
}

/// Shows the placeholder or the date, and the clear button.
fn refresh(o: &Optional) {
    let empty = o.value.is_nan();
    let color = if empty {
        NSColor::clearColor()
    } else {
        NSColor::controlTextColor()
    };
    o.picker.setTextColor(&color);
    o.fill.setHidden(!empty);
    o.clear.setHidden(empty);
}

/// The picker's config: an optional one gets its buttons, a picker that is
/// no longer optional loses them.
pub fn configure(d: &Retained<NSDatePicker>, c: DateConfig, target: &AnyObject) {
    let id = d.tag() as NodeId;
    if !c.optional {
        forget(id);
        return;
    }
    if with(id, |o| o.config = c).is_some() {
        return;
    }
    let mtm = MainThreadMarker::from(&**d);
    let fill = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::new(),
            Some(target),
            Some(sel!(onDateFill:)),
            mtm,
        )
    };
    fill.setBordered(false);
    fill.setAlignment(NSTextAlignment::Natural);
    fill.setContentTintColor(Some(&NSColor::placeholderTextColor()));
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str("xmark.circle.fill"),
        None,
    );
    let clear = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::new(),
            Some(target),
            Some(sel!(onDateClear:)),
            mtm,
        )
    };
    clear.setImage(image.as_deref());
    clear.setImagePosition(NSCellImagePosition::ImageOnly);
    clear.setBordered(false);
    clear.setContentTintColor(Some(&NSColor::tertiaryLabelColor()));
    CLEAR_LABEL.with(|l| clear.setAccessibilityLabel(Some(&NSString::from_str(&l.borrow()))));
    for b in [&fill, &clear] {
        b.setTag(id as isize);
        d.addSubview(b);
    }
    let o = Optional {
        picker: d.clone(),
        fill,
        clear,
        config: c,
        value: d.dateValue().timeIntervalSince1970(),
    };
    refresh(&o);
    layout(&o);
    OPTIONAL.with(|m| m.borrow_mut().insert(id, o));
}

/// The app's value (`NaN`: empty). Returns false for pickers that are not
/// optional.
pub fn set_value(d: &NSDatePicker, value: f64) -> bool {
    with(d.tag() as NodeId, |o| {
        o.value = value;
        refresh(o);
    })
    .is_some()
}

/// The text shown while empty.
pub fn set_placeholder(d: &NSDatePicker, text: &str) {
    with(d.tag() as NodeId, |o| {
        o.fill.setTitle(&NSString::from_str(text));
        // The title keeps the placeholder color.
        o.fill
            .setContentTintColor(Some(&NSColor::placeholderTextColor()));
    });
}

/// "Clear" in the app's language.
pub fn set_clear_label(text: &str) {
    CLEAR_LABEL.with(|l| *l.borrow_mut() = text.to_string());
    OPTIONAL.with(|o| {
        let label = NSString::from_str(text);
        for o in o.borrow().values() {
            o.clear.setAccessibilityLabel(Some(&label));
        }
    });
}

/// The natural size of an optional picker: as wide as its placeholder
/// while empty, with room for the clear button otherwise. `None`: not
/// optional.
pub fn natural_size(d: &NSDatePicker, picker: NSSize) -> Option<NSSize> {
    with(d.tag() as NodeId, |o| {
        if o.value.is_nan() {
            let text = o.fill.fittingSize();
            NSSize::new(picker.width.max(text.width + INSET * 2.), picker.height)
        } else {
            NSSize::new(picker.width + CLEAR + INSET, picker.height)
        }
    })
}

fn layout(o: &Optional) {
    let b = o.picker.bounds();
    o.fill.setFrame(NSRect::new(
        NSPoint::new(INSET, 0.),
        NSSize::new((b.size.width - INSET * 2.).max(0.), b.size.height),
    ));
    let rtl =
        o.picker.userInterfaceLayoutDirection() == NSUserInterfaceLayoutDirection::RightToLeft;
    let x = if rtl {
        INSET
    } else {
        b.size.width - CLEAR - INSET
    };
    o.clear.setFrame(NSRect::new(
        NSPoint::new(x, (b.size.height - CLEAR) / 2.),
        NSSize::new(CLEAR, CLEAR),
    ));
}

/// Lays the buttons out after the picker got its frame.
pub fn framed(d: &NSDatePicker) {
    with(d.tag() as NodeId, |o| layout(o));
}

/// The placeholder was clicked: the current moment (local time of day for
/// a time of day), within the bounds.
pub fn fill(id: NodeId) {
    let Some(v) = with(id, |o| {
        let offset = objc2_foundation::NSTimeZone::localTimeZone().secondsFromGMT() as f64;
        let v = o
            .config
            .start(NSDate::now().timeIntervalSince1970(), offset);
        o.picker
            .setDateValue(&NSDate::dateWithTimeIntervalSince1970(v));
        o.value = v;
        refresh(o);
        // Typing goes on in the picker.
        if let Some(w) = o.picker.window() {
            let picker: &NSControl = &o.picker;
            w.makeFirstResponder(Some(picker));
        }
        v
    }) else {
        return;
    };
    crate::imp::dispatch(UiEvent::Change(id, v));
}

/// The clear button was clicked.
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

/// The picker itself changed (typing, the stepper, the calendar): an empty
/// one is no longer empty.
pub fn edited(id: NodeId, value: f64) {
    with(id, |o| {
        o.value = value;
        refresh(o);
    });
}

/// The node is gone, or no longer optional.
pub fn forget(id: NodeId) {
    if let Some(o) = OPTIONAL.with(|o| o.borrow_mut().remove(&id)) {
        o.fill.removeFromSuperview();
        o.clear.removeFromSuperview();
        o.picker.setTextColor(&NSColor::controlTextColor());
    }
}
