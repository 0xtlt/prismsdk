//! Tables for VoiceOver (#284), like the web's in Safari.
//!
//! The table's view has the table role, its rows the row role, their cells
//! the cell role (the header row's included, as WebKit does). `set_table`
//! gives the table its rows, its columns (`ColumnElement`s, which are not
//! views) and its column headers; each row its index; each cell its row and
//! column and its column's header (`AXColumnHeaderUIElements`), which
//! VoiceOver reads with the cell. `accessibilityCellForColumn:row:` is
//! answered by `PrismView` through `cell_at`.

use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityColumnRole, NSAccessibilityElement, NSControl, NSTextField,
    NSView,
};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSRange, NSRect, NSString};

pub struct ColumnIvars {
    header: Weak<NSView>,
    table: Weak<NSView>,
}

define_class!(
    /// A table's column: under its header, as tall as the table.
    #[unsafe(super(NSAccessibilityElement, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismTableColumn"]
    #[ivars = ColumnIvars]
    pub struct ColumnElement;

    unsafe impl NSObjectProtocol for ColumnElement {}

    impl ColumnElement {
        #[unsafe(method(accessibilityFrame))]
        fn accessibility_frame(&self) -> NSRect {
            let iv = self.ivars();
            let (Some(header), Some(table)) = (iv.header.load(), iv.table.load()) else {
                return NSRect::ZERO;
            };
            let h = header.accessibilityFrame();
            let t = table.accessibilityFrame();
            NSRect::new(
                objc2_foundation::NSPoint::new(h.origin.x, t.origin.y),
                objc2_foundation::NSSize::new(h.size.width, t.size.height),
            )
        }
    }
);

fn objects<T: Message>(items: &[Retained<T>]) -> Retained<NSArray> {
    let array = NSArray::from_retained_slice(items);
    // SAFETY: an array of objects is an array of `AnyObject`s.
    unsafe { Retained::cast_unchecked(array) }
}

/// Hands the table its rows, columns and headers: `rows` are each row's
/// view, whether it is a header row, and its cells.
pub fn set(mtm: MainThreadMarker, table: &NSView, rows: &[(&NSView, bool, Vec<&NSView>)]) {
    let columns = rows.iter().map(|r| r.2.len()).max().unwrap_or(0);
    let headers: Vec<&NSView> = rows
        .iter()
        .take_while(|r| r.1)
        .last()
        .map(|r| r.2.clone())
        .unwrap_or_default();
    let mut by_column: Vec<Vec<Retained<NSView>>> = vec![Vec::new(); columns];
    for (r, (row, header, cells)) in rows.iter().enumerate() {
        row.setAccessibilityIndex(r as isize);
        for (c, cell) in cells.iter().enumerate() {
            cell.setAccessibilityRowIndexRange(NSRange::new(r, 1));
            cell.setAccessibilityColumnIndexRange(NSRange::new(c, 1));
            let over = headers.get(c).filter(|_| !header).map(|h| (*h).retain());
            // SAFETY: an array of views.
            unsafe {
                cell.setAccessibilityColumnHeaderUIElements(over.map(|h| objects(&[h])).as_deref())
            };
            by_column[c].push((*cell).retain());
        }
    }
    let column_elements: Vec<Retained<ColumnElement>> = by_column
        .iter()
        .enumerate()
        .map(|(c, cells)| {
            let header = headers.get(c).copied();
            let this = ColumnElement::alloc(mtm).set_ivars(ColumnIvars {
                header: header.map(Weak::from).unwrap_or_default(),
                table: Weak::from(table),
            });
            let column: Retained<ColumnElement> = unsafe { msg_send![super(this), init] };
            column.setAccessibilityRole(Some(unsafe { NSAccessibilityColumnRole }));
            column.setAccessibilityIndex(c as isize);
            column.setAccessibilityLabel(header.and_then(|h| h.accessibilityLabel()).as_deref());
            // SAFETY: the table view, the header view and the cell views.
            unsafe {
                column.setAccessibilityParent(Some(table));
                column.setAccessibilityHeader(header.map(|h| h as &AnyObject));
                column.setAccessibilityChildren(Some(&objects(cells)));
            }
            column
        })
        .collect();
    let row_views: Vec<Retained<NSView>> = rows.iter().map(|r| r.0.retain()).collect();
    let header_views: Vec<Retained<NSView>> = headers.iter().map(|h| (*h).retain()).collect();
    table.setAccessibilityRowCount(rows.len() as isize);
    table.setAccessibilityColumnCount(columns as isize);
    // SAFETY: arrays of views and of column elements.
    unsafe {
        table.setAccessibilityRows(Some(&objects(&row_views)));
        table.setAccessibilityVisibleRows(Some(&objects(&row_views)));
        table.setAccessibilityColumns(Some(&objects(&column_elements)));
        table.setAccessibilityVisibleColumns(Some(&objects(&column_elements)));
        table.setAccessibilityColumnHeaderUIElements(
            (!header_views.is_empty())
                .then(|| objects(&header_views))
                .as_deref(),
        );
    }
}

/// Roles of the elements a VoiceOver user acts on.
const ACTIVE: [&str; 13] = [
    "AXButton",
    "AXCheckBox",
    "AXRadioButton",
    "AXRadioGroup",
    "AXLink",
    "AXTextField",
    "AXTextArea",
    "AXComboBox",
    "AXPopUpButton",
    "AXSlider",
    "AXIncrementor",
    "AXDateTimeArea",
    "AXMenuButton",
];

/// The controls in a table cell (#284), outermost first: buttons, switches,
/// fields, links, clickable views (what VoiceOver acts on).
pub fn controls(view: &NSView) -> Vec<Retained<NSView>> {
    let mut out = Vec::new();
    for v in view.subviews().iter() {
        if v.isHidden() {
            continue;
        }
        let role = v.accessibilityRole().map(|r| r.to_string());
        let active = role.is_some_and(|r| ACTIVE.contains(&r.as_str()));
        // A control (whose cell has the role), unless it is a text: a text
        // counts when it is a link or a button.
        let control = v.downcast_ref::<NSControl>().is_some_and(|c| {
            c.downcast_ref::<NSTextField>()
                .is_none_or(|t| t.isEditable())
        });
        if control || active {
            out.push(v.clone());
        } else {
            out.extend(controls(&v));
        }
    }
    out
}

/// The cell at `column` and `row` of a table's view.
pub fn cell_at(table: &NSView, column: isize, row: isize) -> Option<Retained<AnyObject>> {
    let rows = table.accessibilityRows()?;
    let row: Retained<AnyObject> = rows.to_vec().get(usize::try_from(row).ok()?)?.clone();
    let row = row.downcast::<NSView>().ok()?;
    row.subviews()
        .iter()
        .find(|c| c.accessibilityColumnIndexRange().location == column as usize)
        .map(|c| Retained::into_super(Retained::into_super(Retained::into_super(c))))
}

fn label(o: &AnyObject) -> Option<String> {
    let v = o.downcast_ref::<NSView>()?;
    v.accessibilityLabel().map(|l| l.to_string())
}

/// What the accessibility dump says of an element under a cell: its class,
/// role, label (else title), value, and whether VoiceOver can press it.
fn part(o: &AnyObject) -> String {
    use crate::a11y_dump::{json_str, opt};
    let text = |sel: objc2::runtime::Sel| -> Option<String> {
        if !o.class().responds_to(sel) {
            return None;
        }
        // SAFETY: an NSAccessibility getter; its value is any object or nil.
        let v: Option<Retained<AnyObject>> = unsafe { msg_send![o, performSelector: sel] };
        let v = v?;
        let s = match v.downcast_ref::<NSString>() {
            Some(s) => s.to_string(),
            None => v
                .downcast_ref::<objc2_foundation::NSNumber>()?
                .stringValue()
                .to_string(),
        };
        (!s.is_empty()).then_some(s)
    };
    let label =
        text(objc2::sel!(accessibilityLabel)).or_else(|| text(objc2::sel!(accessibilityTitle)));
    let press = objc2::sel!(accessibilityPerformPress);
    // SAFETY: NSAccessibility's check of the actions an element allows.
    let pressable = o.class().responds_to(press)
        && unsafe { msg_send![o, isAccessibilitySelectorAllowed: press] };
    format!(
        r#"{{"class":{},"role":{},"label":{},"value":{},"press":{pressable}}}"#,
        json_str(o.class().name().to_str().unwrap_or("?")),
        opt(text(objc2::sel!(accessibilityRole))),
        opt(label),
        opt(text(objc2::sel!(accessibilityValue))),
    )
}

/// For the accessibility dump: a table's `"table"` entry (row and column
/// counts, its column headers' labels), a cell's `"cell"` one (row, column),
/// `"header"` (its column header's label) and `"parts"` (the elements
/// VoiceOver reaches inside it: its controls and texts).
pub fn dump(view: &NSView) -> String {
    use crate::a11y_dump::opt;
    let role = view.accessibilityRole().map(|r| r.to_string());
    match role.as_deref() {
        Some("AXTable") => {
            let headers: Vec<String> = view
                .accessibilityColumnHeaderUIElements()
                .map(|h| h.to_vec())
                .unwrap_or_default()
                .iter()
                .map(|h| opt(label(h)))
                .collect();
            format!(
                r#","table":{{"rows":{},"columns":{},"headers":[{}]}}"#,
                view.accessibilityRowCount(),
                view.accessibilityColumnCount(),
                headers.join(",")
            )
        }
        Some("AXCell") => {
            let header = view
                .accessibilityColumnHeaderUIElements()
                .and_then(|h| h.to_vec().first().cloned())
                .and_then(|h| label(&h));
            let parts: Vec<String> = view
                .accessibilityChildren()
                .map(|c| c.to_vec())
                .unwrap_or_default()
                .iter()
                .map(|c| part(c))
                .collect();
            format!(
                r#","cell":[{},{}],"header":{},"parts":[{}]"#,
                view.accessibilityRowIndexRange().location,
                view.accessibilityColumnIndexRange().location,
                opt(header),
                parts.join(",")
            )
        }
        Some("AXRow") => format!(r#","index":{}"#, view.accessibilityIndex()),
        _ => String::new(),
    }
}
