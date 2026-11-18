//! One selection across several text fields (#231) on iOS, iPadOS and
//! visionOS: an iPad pointer's drag (trackpad, mouse) from one field of a
//! selection group into the others.
//!
//! The keys go through key commands and the text delegates
//! (`Runtime::consumes_key_event`, `Runtime::takes_text_across`); the
//! fields draw their part with `crate::edit::highlight`. A finger's
//! selection handles stay UIKit's, inside one field.

use std::cell::RefCell;

use objc2::runtime::AnyObject;
use objc2::{MainThreadOnly, Message, msg_send, sel};
use objc2_core_foundation::CGPoint;
use objc2_foundation::{NSArray, NSNumber, NSString};
use objc2_ui_kit::{
    UIGestureRecognizer, UIGestureRecognizerState, UIPanGestureRecognizer, UITextInput,
    UITouchType, UIView,
};
use prism_core::{NodeId, UiEvent};

use crate::imp::GroupField;

/// The name of a group's pointer recognizer.
const ACROSS: &str = "prism.across";

/// Whether `g` is a selection group's pointer recognizer: it follows the
/// pointer alongside the fields' own recognizers.
pub fn is_across(g: &UIGestureRecognizer) -> bool {
    g.name().is_some_and(|n| n.to_string() == ACROSS)
}

/// Adds (or removes) the recognizer that follows an iPad pointer's drags
/// over a selection group's container. Fingers and pens are left alone.
pub fn set_group(view: &UIView, on: bool, target: &AnyObject) {
    let existing = view
        .gestureRecognizers()
        .map(|a| a.to_vec())
        .unwrap_or_default()
        .into_iter()
        .find(|g| is_across(g));
    match (existing, on) {
        (Some(g), false) => view.removeGestureRecognizer(&g),
        (None, true) => {
            let g = unsafe {
                UIPanGestureRecognizer::initWithTarget_action(
                    UIPanGestureRecognizer::alloc(view.mtm()),
                    Some(target),
                    Some(sel!(onAcrossPan:)),
                )
            };
            g.setName(Some(&NSString::from_str(ACROSS)));
            g.setAllowedTouchTypes(&NSArray::from_retained_slice(&[NSNumber::new_isize(
                UITouchType::IndirectPointer.0,
            )]));
            g.setCancelsTouchesInView(false);
            g.setDelaysTouchesBegan(false);
            g.setDelaysTouchesEnded(false);
            let _: () = unsafe { msg_send![&*g, setDelegate: target] };
            view.addGestureRecognizer(&g);
        }
        _ => {}
    }
}

/// A pointer drag from a field of a group.
struct Drag {
    group: NodeId,
    anchor: NodeId,
    /// Where it started, in characters.
    at: usize,
    fields: Vec<(NodeId, GroupField)>,
    /// It left the field for another of the group: the field's own drag
    /// was stopped, every move is reported.
    taken: bool,
}

thread_local! {
    static DRAG: RefCell<Option<Drag>> = const { RefCell::new(None) };
}

fn view_of(f: &GroupField) -> &UIView {
    match f {
        GroupField::Area(tv) => tv,
        GroupField::Input(tf) => tf,
    }
}

/// The character of `input` nearest to `local` (its coordinates).
fn offset<T: UITextInput + Message>(input: &T, text: &str, local: CGPoint) -> usize {
    let begin = input.beginningOfDocument();
    let Some(pos) = input.closestPositionToPoint(local) else {
        return 0;
    };
    let units = input.offsetFromPosition_toPosition(&begin, &pos).max(0) as usize;
    prism_native::fields::utf16_to_chars(text, units)
}

/// The field of the group under window point `p`, and the character there.
fn field_at(fields: &[(NodeId, GroupField)], p: CGPoint) -> Option<(NodeId, usize)> {
    fields.iter().find_map(|(id, f)| {
        let v = view_of(f);
        let l = v.convertPoint_fromView(p, None);
        let b = v.bounds();
        let inside = !v.isHidden()
            && v.window().is_some()
            && l.x >= b.origin.x
            && l.x < b.origin.x + b.size.width
            && l.y >= b.origin.y
            && l.y < b.origin.y + b.size.height;
        inside.then(|| {
            let to = match f {
                GroupField::Area(tv) => offset(&**tv, &tv.text().to_string(), l),
                GroupField::Input(tf) => {
                    let text = tf.text().map(|t| t.to_string()).unwrap_or_default();
                    offset(&**tf, &text, l)
                }
            };
            (*id, to)
        })
    })
}

/// Stops a field's own pointer selection: its recognizers are cancelled.
fn stop_own_drag(v: &UIView) {
    for g in v
        .gestureRecognizers()
        .map(|a| a.to_vec())
        .unwrap_or_default()
    {
        if g.isEnabled() && !is_across(&g) {
            g.setEnabled(false);
            g.setEnabled(true);
        }
    }
}

/// The group's pointer recognizer moved: while the drag stays in the field
/// where it started, the field selects; once it is over another field of
/// the group, the field's own drag stops and the runtime selects and
/// highlights the range (`UiEvent::SelectionAcross`, #264).
pub fn on_pan(g: &UIPanGestureRecognizer) {
    let p = g.locationInView(None);
    match g.state() {
        UIGestureRecognizerState::Began => {
            let t = g.translationInView(None);
            let start = CGPoint::new(p.x - t.x, p.y - t.y);
            let group = g.view().map_or(0, |v| v.tag() as NodeId);
            let fields = crate::imp::group_fields(group);
            let drag = field_at(&fields, start).map(|(anchor, at)| Drag {
                group,
                anchor,
                at,
                fields,
                taken: false,
            });
            DRAG.with(|d| *d.borrow_mut() = drag);
            on_move(p);
        }
        UIGestureRecognizerState::Changed => on_move(p),
        _ => DRAG.with(|d| *d.borrow_mut() = None),
    }
}

fn on_move(p: CGPoint) {
    let event = DRAG.with(|d| {
        let mut d = d.borrow_mut();
        let d = d.as_mut()?;
        let (over, to) = field_at(&d.fields, p)?;
        if over == d.anchor && !d.taken {
            return None;
        }
        if !d.taken {
            d.taken = true;
            let anchor = d.fields.iter().find(|(id, _)| *id == d.anchor)?;
            stop_own_drag(view_of(&anchor.1));
        }
        Some(UiEvent::SelectionAcross(
            d.group,
            Some(((d.anchor, d.at), (over, to))),
        ))
    });
    if let Some(e) = event {
        crate::imp::dispatch(e);
    }
}

/// The views of a group's fields, for [`crate::imp::group_fields`].
pub(crate) fn field_view(view: &crate::imp::View) -> Option<GroupField> {
    match view {
        crate::imp::View::TextArea(t) => Some(GroupField::Area(t.view.clone())),
        crate::imp::View::Input(f) => Some(GroupField::Input(f.clone())),
        _ => None,
    }
}
