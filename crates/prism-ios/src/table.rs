//! Tables for VoiceOver (#284), on iOS, iPadOS, visionOS and tvOS.
//!
//! A table's container is a `UIAccessibilityContainerDataTable`: its
//! accessibility elements are its cells, row by row (header row first), and
//! it answers for the cell at a row and column and for a column's headers.
//! Each cell knows its row and column (`UIAccessibilityContainerDataTableCell`),
//! so VoiceOver reads the column's header when the focus moves to another
//! column, and the row and column counts. Header cells carry the header
//! trait. The methods live on `PrismContainerView` (`interaction.rs`) and
//! tvOS's `PrismFocusView` (`tv.rs`); the data here.
//!
//! A cell of text is one element, read from its text (`prism-core`'s
//! automatic labels). A cell holding a control (a button, a link, a toggle,
//! a field) is a container instead, so each control is reached and
//! activated on its own: its elements are the cell's own elements, in
//! order. On iOS and visionOS each of them is a [`CellPart`], an element
//! that reads and acts as the view it stands for and knows the cell's row
//! and column, so VoiceOver still says where it is. Text fields stay
//! themselves (editing needs the real `UITextInput`), and so does every
//! view on tvOS, where VoiceOver follows the focus engine, which moves
//! between the real views. Activating a text cell clicks its row when the
//! row is clickable.

use std::cell::{Cell, RefCell};

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, AnyProtocol};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::{NSArray, NSNotFound, NSObject, NSObjectProtocol, NSRange, NSString};
use objc2_ui_kit::{
    NSObjectUIAccessibility, NSObjectUIAccessibilityAction, NSObjectUIAccessibilityContainer,
    UIAccessibilityElement, UIAccessibilityTraitAdjustable, UIAccessibilityTraitButton,
    UIAccessibilityTraitKeyboardKey, UIAccessibilityTraitLink, UIAccessibilityTraitToggleButton,
    UIAccessibilityTraits, UIButton, UIControl, UIControlEvents, UIResponder, UIView,
};
#[cfg(not(target_os = "tvos"))]
use objc2_ui_kit::{UISlider, UISwitch};

/// Whether a cell's parts stand in for its views ([`CellPart`]), or are
/// the views themselves (tvOS, see the module docs).
const PROXIES: bool = cfg!(not(target_os = "tvos"));

/// What a Prism container knows as a table or as one of its cells.
#[derive(Default)]
pub struct TableIvars {
    /// A table's cells, row by row (header rows first).
    rows: RefCell<Vec<Vec<Retained<UIView>>>>,
    /// How many of a table's rows are header rows.
    headers: Cell<usize>,
    /// A cell's row and column.
    at: Cell<Option<(usize, usize)>>,
    /// A cell's row, clicked when the cell is activated.
    row: RefCell<Weak<UIView>>,
    /// Whether it is a table's row.
    is_row: Cell<bool>,
    /// The elements of a cell holding a control, kept from one question to
    /// the next so VoiceOver's focus stays on the same objects.
    parts: RefCell<Vec<Retained<AnyObject>>>,
}

/// The table ivars of `view`, if it is a Prism container.
fn ivars(view: &UIView) -> Option<&TableIvars> {
    #[cfg(not(target_os = "tvos"))]
    return crate::interaction::table_ivars(view);
    #[cfg(target_os = "tvos")]
    return crate::tv::table_ivars(view);
}

/// Hands the table its cells: `rows` are each row's view, whether it is a
/// header row, and its cells.
pub fn set(mtm: MainThreadMarker, table: &UIView, rows: &[(&UIView, bool, Vec<&UIView>)]) {
    let Some(t) = ivars(table) else {
        return;
    };
    let mut flat: Vec<Retained<UIView>> = Vec::new();
    let mut grid = Vec::with_capacity(rows.len());
    for (r, (row, _, cells)) in rows.iter().enumerate() {
        if let Some(i) = ivars(row) {
            i.is_row.set(true);
        }
        for (c, cell) in cells.iter().enumerate() {
            if let Some(i) = ivars(cell) {
                i.at.set(Some((r, c)));
                *i.row.borrow_mut() = Weak::from(*row);
            }
            flat.push((*cell).retain());
        }
        grid.push(cells.iter().map(|c| (*c).retain()).collect());
    }
    t.headers.set(rows.iter().take_while(|r| r.1).count());
    *t.rows.borrow_mut() = grid;
    let elements = NSArray::from_retained_slice(&flat);
    // SAFETY: an array of views, which are accessibility elements.
    unsafe {
        let elements: Retained<NSArray> = Retained::cast_unchecked(elements);
        table.setAccessibilityElements(Some(&elements), mtm);
    }
}

pub fn row_count(t: &TableIvars) -> isize {
    t.rows.borrow().len() as isize
}

pub fn column_count(t: &TableIvars) -> isize {
    t.rows.borrow().iter().map(Vec::len).max().unwrap_or(0) as isize
}

/// The element VoiceOver lands on for a cell: the cell, or the first of
/// its parts that knows its row and column.
fn element_of(mtm: MainThreadMarker, cell: &UIView) -> Retained<AnyObject> {
    let first = ivars(cell).and_then(|t| parts(mtm, cell, t)).and_then(|p| {
        p.into_iter()
            .find(|p| p.downcast_ref::<CellPart>().is_some())
    });
    first.unwrap_or_else(|| as_object(cell))
}

pub fn cell(
    mtm: MainThreadMarker,
    t: &TableIvars,
    row: usize,
    column: usize,
) -> Option<Retained<AnyObject>> {
    let cell = t.rows.borrow().get(row)?.get(column).cloned()?;
    Some(element_of(mtm, &cell))
}

pub fn headers(
    mtm: MainThreadMarker,
    t: &TableIvars,
    column: usize,
) -> Option<Retained<NSArray<AnyObject>>> {
    let header_cells: Vec<Retained<UIView>> = t
        .rows
        .borrow()
        .iter()
        .take(t.headers.get())
        .filter_map(|r| r.get(column).cloned())
        .collect();
    let elements: Vec<Retained<AnyObject>> =
        header_cells.iter().map(|h| element_of(mtm, h)).collect();
    (!elements.is_empty()).then(|| NSArray::from_retained_slice(&elements))
}

fn range(i: Option<usize>) -> NSRange {
    match i {
        Some(i) => NSRange::new(i, 1),
        None => NSRange::new(NSNotFound as usize, 0),
    }
}

pub fn row_range(t: &TableIvars) -> NSRange {
    range(t.at.get().map(|a| a.0))
}

pub fn column_range(t: &TableIvars) -> NSRange {
    range(t.at.get().map(|a| a.1))
}

/// The row and column of the table cell `view` sits in, for a view that
/// stays itself among a cell's parts (a field; every view on tvOS, where
/// the focus lands on it), so VoiceOver still says where it is.
pub fn enclosing_range(view: &UIView, column: bool) -> NSRange {
    let mut at = None;
    let mut up = view.superview();
    while let Some(v) = up {
        at = ivars(&v).and_then(|t| t.at.get());
        if at.is_some() {
            break;
        }
        up = v.superview();
    }
    range(at.map(|(r, c)| if column { c } else { r }))
}

/// Whether a Prism container is a table's row.
#[cfg(target_os = "tvos")]
pub fn is_row(t: &TableIvars) -> bool {
    t.is_row.get()
}

/// A cell's row, when the cell belongs to a table.
pub fn row_of(t: &TableIvars) -> Option<Retained<UIView>> {
    t.at.get()?;
    t.row.borrow().load()
}

fn as_object(view: &UIView) -> Retained<AnyObject> {
    let view = view.retain();
    Retained::into_super(Retained::into_super(Retained::into_super(view)))
}

/// A text field or a text view: VoiceOver edits it through `UITextInput`.
fn is_text_entry(view: &UIView) -> bool {
    AnyProtocol::get(c"UITextInput").is_some_and(|p| view.conformsToProtocol(p))
}

/// A control UIKit always makes one element (its accessibility runs
/// only while an assistive technology does): a button, a switch, a slider.
fn is_element(view: &UIView) -> bool {
    #[cfg(not(target_os = "tvos"))]
    let controls = [UIButton::class(), UISwitch::class(), UISlider::class()];
    #[cfg(target_os = "tvos")]
    let controls = [UIButton::class()];
    controls.iter().any(|c| view.isKindOfClass(c))
}

/// The views a cell is read as, in order: the visible views below it that
/// are elements, controls and fields (UIKit makes them elements once an
/// assistive technology runs), and containers listing their own elements
/// (a table in a cell).
fn descendants(mtm: MainThreadMarker, view: &UIView, out: &mut Vec<Retained<UIView>>) {
    for sub in view.subviews().iter() {
        if sub.isHidden() || sub.alpha() < 0.01 || sub.accessibilityElementsHidden(mtm) {
            continue;
        }
        if sub.isAccessibilityElement(mtm)
            || sub.isKindOfClass(UIControl::class())
            || is_text_entry(&sub)
            || sub.accessibilityElements(mtm).is_some()
        {
            out.push(sub.clone());
        } else {
            descendants(mtm, &sub, out);
        }
    }
}

/// Whether `view` (one of [`descendants`]) is something VoiceOver acts on
/// rather than reads: a button, a link, a toggle, an adjustable control, a
/// field, an element with actions, or a container of elements.
fn operable(mtm: MainThreadMarker, view: &UIView) -> bool {
    let acts: UIAccessibilityTraits = unsafe {
        UIAccessibilityTraitButton
            | UIAccessibilityTraitLink
            | UIAccessibilityTraitAdjustable
            | UIAccessibilityTraitKeyboardKey
            | UIAccessibilityTraitToggleButton
    };
    !view.isAccessibilityElement(mtm)
        || view.accessibilityTraits(mtm) & acts != 0
        || view.isKindOfClass(UIControl::class())
        || is_text_entry(view)
        || view
            .accessibilityCustomActions(mtm)
            .is_some_and(|a| a.count() > 0)
}

/// Whether a table's `cell` holds a control, and so is read part by part.
pub fn has_parts(mtm: MainThreadMarker, cell: &UIView, t: &TableIvars) -> bool {
    if t.at.get().is_none() {
        return false;
    }
    let mut views = Vec::new();
    descendants(mtm, cell, &mut views);
    views.iter().any(|v| operable(mtm, v))
}

/// The elements of a table's `cell` when it holds a control (see the
/// module docs); `None` when it is one element.
pub fn parts(
    mtm: MainThreadMarker,
    cell: &UIView,
    t: &TableIvars,
) -> Option<Vec<Retained<AnyObject>>> {
    t.at.get()?;
    let mut views = Vec::new();
    descendants(mtm, cell, &mut views);
    if !views.iter().any(|v| operable(mtm, v)) {
        t.parts.borrow_mut().clear();
        return None;
    }
    let old = std::mem::take(&mut *t.parts.borrow_mut());
    let parts: Vec<Retained<AnyObject>> = views
        .iter()
        .map(|v| {
            if !PROXIES || is_text_entry(v) || !(v.isAccessibilityElement(mtm) || is_element(v)) {
                return as_object(v);
            }
            let kept = old.iter().find(|p| {
                p.downcast_ref::<CellPart>()
                    .is_some_and(|p| p.stands_for(v))
            });
            match kept {
                Some(p) => p.clone(),
                None => Retained::into_super(Retained::into_super(Retained::into_super(
                    Retained::into_super(CellPart::new(mtm, cell, v)),
                ))),
            }
        })
        .collect();
    *t.parts.borrow_mut() = parts.clone();
    Some(parts)
}

/// [`parts`] as the array `accessibilityElements` answers.
pub fn parts_array(
    mtm: MainThreadMarker,
    cell: &UIView,
    t: &TableIvars,
) -> Option<Retained<NSArray>> {
    let parts = parts(mtm, cell, t)?;
    Some(NSArray::from_retained_slice(&parts))
}

pub struct PartIvars {
    /// The view it reads and acts as.
    view: Weak<UIView>,
    /// The cell it belongs to (its row and column).
    cell: Weak<UIView>,
}

define_class!(
    /// One element of a table's cell that holds a control (iOS, visionOS):
    /// it reads and acts as its view, and has the cell's row and column
    /// (`UIAccessibilityContainerDataTableCell`).
    #[unsafe(super(UIAccessibilityElement, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismTableCellPart"]
    #[ivars = PartIvars]
    pub struct CellPart;

    unsafe impl NSObjectProtocol for CellPart {}

    impl CellPart {
        #[unsafe(method(isAccessibilityElement))]
        fn is_accessibility_element(&self) -> bool {
            self.view().is_some()
        }

        #[unsafe(method_id(accessibilityLabel))]
        fn accessibility_label(&self) -> Option<Retained<NSString>> {
            self.view().and_then(|v| v.accessibilityLabel(self.mtm()))
        }

        #[unsafe(method_id(accessibilityValue))]
        fn accessibility_value(&self) -> Option<Retained<NSString>> {
            self.view().and_then(|v| v.accessibilityValue(self.mtm()))
        }

        #[unsafe(method_id(accessibilityHint))]
        fn accessibility_hint(&self) -> Option<Retained<NSString>> {
            self.view().and_then(|v| v.accessibilityHint(self.mtm()))
        }

        #[unsafe(method_id(accessibilityIdentifier))]
        fn accessibility_identifier(&self) -> Option<Retained<NSString>> {
            self.view()
                .and_then(|v| unsafe { msg_send![&*v, accessibilityIdentifier] })
        }

        #[unsafe(method_id(accessibilityLanguage))]
        fn accessibility_language(&self) -> Option<Retained<NSString>> {
            self.view().and_then(|v| v.accessibilityLanguage(self.mtm()))
        }

        #[unsafe(method(accessibilityTraits))]
        fn accessibility_traits(&self) -> UIAccessibilityTraits {
            self.view().map_or(0, |v| v.accessibilityTraits(self.mtm()))
        }

        #[unsafe(method(accessibilityFrame))]
        fn accessibility_frame(&self) -> CGRect {
            self.view()
                .map_or(CGRect::ZERO, |v| v.accessibilityFrame(self.mtm()))
        }

        #[unsafe(method(accessibilityActivationPoint))]
        fn accessibility_activation_point(&self) -> CGPoint {
            self.view()
                .map_or(CGPoint::ZERO, |v| v.accessibilityActivationPoint(self.mtm()))
        }

        #[unsafe(method_id(accessibilityCustomActions))]
        fn accessibility_custom_actions(&self) -> Option<Retained<NSArray<AnyObject>>> {
            self.view()
                .and_then(|v| unsafe { msg_send![&*v, accessibilityCustomActions] })
        }

        #[unsafe(method_id(accessibilityUserInputLabels))]
        fn accessibility_user_input_labels(&self) -> Option<Retained<NSArray<NSString>>> {
            self.view()
                .and_then(|v| unsafe { msg_send![&*v, accessibilityUserInputLabels] })
        }

        /// VoiceOver's double tap: the view's own activation; a button
        /// sends its action. Otherwise (`false`) VoiceOver taps the view's
        /// activation point, as for the view itself.
        #[unsafe(method(accessibilityActivate))]
        fn accessibility_activate(&self) -> bool {
            self.activate()
        }

        #[unsafe(method(accessibilityIncrement))]
        fn accessibility_increment(&self) {
            if let Some(v) = self.view() {
                v.accessibilityIncrement(self.mtm());
            }
        }

        #[unsafe(method(accessibilityDecrement))]
        fn accessibility_decrement(&self) {
            if let Some(v) = self.view() {
                v.accessibilityDecrement(self.mtm());
            }
        }

        #[unsafe(method(accessibilityRowRange))]
        fn accessibility_row_range(&self) -> NSRange {
            range(self.at().map(|a| a.0))
        }

        #[unsafe(method(accessibilityColumnRange))]
        fn accessibility_column_range(&self) -> NSRange {
            range(self.at().map(|a| a.1))
        }
    }
);

impl CellPart {
    fn new(mtm: MainThreadMarker, cell: &UIView, view: &UIView) -> Retained<CellPart> {
        let this = CellPart::alloc(mtm).set_ivars(PartIvars {
            view: Weak::from(view),
            cell: Weak::from(cell),
        });
        let container: &AnyObject = cell;
        unsafe { msg_send![super(this), initWithAccessibilityContainer: container] }
    }

    fn view(&self) -> Option<Retained<UIView>> {
        self.ivars().view.load()
    }

    fn activate(&self) -> bool {
        let Some(view) = self.view() else {
            return false;
        };
        if view.accessibilityActivate(self.mtm()) {
            return true;
        }
        match view.downcast_ref::<UIButton>() {
            Some(b) if b.isEnabled() => {
                b.sendActionsForControlEvents(
                    UIControlEvents::PrimaryActionTriggered | UIControlEvents::TouchUpInside,
                );
                true
            }
            _ => false,
        }
    }

    fn stands_for(&self, view: &UIView) -> bool {
        self.view().is_some_and(|v| std::ptr::eq(&*v, view))
    }

    /// The row and column of its cell.
    fn at(&self) -> Option<(usize, usize)> {
        let cell = self.ivars().cell.load()?;
        ivars(&cell)?.at.get()
    }
}

/// What the accessibility dump shows of a table or a cell.
pub enum Dump {
    /// A table's row and column counts and its header cells' labels.
    Table(isize, isize, Vec<Option<String>>),
    /// A cell's row and column, and its parts when it holds a control.
    Cell(usize, usize, Option<Vec<PartDump>>),
}

/// One element of a cell holding a control, as VoiceOver gets it.
pub struct PartDump {
    pub label: Option<String>,
    pub traits: UIAccessibilityTraits,
    /// Its row and column, when it knows them.
    pub at: Option<(usize, usize)>,
    /// The class of the view it is (or stands for).
    pub class: String,
}

/// The row and column a view that stays itself answers VoiceOver with.
fn real_at(part: &AnyObject) -> Option<(usize, usize)> {
    let answers: bool =
        unsafe { msg_send![part, respondsToSelector: objc2::sel!(accessibilityRowRange)] };
    if !answers {
        return None;
    }
    let row: NSRange = unsafe { msg_send![part, accessibilityRowRange] };
    let column: NSRange = unsafe { msg_send![part, accessibilityColumnRange] };
    // Outside a cell: an empty range (UIKit's own is at `INT_MAX`).
    (row.length > 0).then_some((row.location, column.location))
}

fn part_dump(part: &AnyObject) -> PartDump {
    let (view, at) = match part.downcast_ref::<CellPart>() {
        Some(p) => (p.view(), p.at()),
        None => (
            part.downcast_ref::<UIView>().map(|v| v.retain()),
            real_at(part),
        ),
    };
    // A label's text is its label once UIKit's accessibility runs.
    let label: Option<Retained<NSString>> = unsafe { msg_send![part, accessibilityLabel] };
    let label = label.or_else(|| {
        let view = view.as_deref()?;
        view.downcast_ref::<objc2_ui_kit::UILabel>()
            .and_then(|l| l.text())
            .or_else(|| {
                view.downcast_ref::<UIButton>()
                    .and_then(|b| b.titleForState(objc2_ui_kit::UIControlState::Normal))
            })
    });
    let traits: UIAccessibilityTraits = unsafe { msg_send![part, accessibilityTraits] };
    PartDump {
        label: label.map(|l| l.to_string()),
        traits,
        at,
        class: view.map_or_else(
            || part.class().name().to_string_lossy().into_owned(),
            |v| v.class().name().to_string_lossy().into_owned(),
        ),
    }
}

pub fn dump(mtm: MainThreadMarker, view: &UIView) -> Option<Dump> {
    let t = ivars(view)?;
    if let Some((r, c)) = t.at.get() {
        let parts = parts(mtm, view, t).map(|p| p.iter().map(|p| part_dump(p)).collect());
        return Some(Dump::Cell(r, c, parts));
    }
    let rows = t.rows.borrow();
    if rows.is_empty() {
        return None;
    }
    let headers = rows
        .iter()
        .take(t.headers.get())
        .flatten()
        .map(|h| h.accessibilityLabel(mtm).map(|l| l.to_string()))
        .collect();
    Some(Dump::Table(row_count(t), column_count(t), headers))
}
