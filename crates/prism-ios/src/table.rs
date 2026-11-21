//! Tables for VoiceOver (#284).
//!
//! A table's container is a `UIAccessibilityContainerDataTable`: its
//! accessibility elements are its cells, row by row (header row first), and
//! it answers for the cell at a row and column and for a column's headers.
//! Each cell is an element that knows its row and column
//! (`UIAccessibilityContainerDataTableCell`), so VoiceOver reads the
//! column's header when the focus moves to another column, and the row and
//! column counts. The methods live on `PrismContainerView` (see
//! `interaction.rs`); the data here.
//!
//! A cell is read as one element (its text, see `prism-core`'s automatic
//! labels): a control inside a cell is not reached on its own. Activating a
//! cell clicks its row when the row is clickable.

use std::cell::{Cell, RefCell};

use objc2::rc::{Retained, Weak};
use objc2::{MainThreadMarker, Message};
use objc2_foundation::{NSArray, NSNotFound, NSRange};
use objc2_ui_kit::{NSObjectUIAccessibilityContainer, UIView};

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
}

/// Hands the table its cells: `rows` are each row's view, whether it is a
/// header row, and its cells.
pub fn set(mtm: MainThreadMarker, table: &UIView, rows: &[(&UIView, bool, Vec<&UIView>)]) {
    let Some(t) = crate::interaction::table_ivars(table) else {
        return;
    };
    let mut flat: Vec<Retained<UIView>> = Vec::new();
    let mut grid = Vec::with_capacity(rows.len());
    for (r, (row, _, cells)) in rows.iter().enumerate() {
        for (c, cell) in cells.iter().enumerate() {
            if let Some(i) = crate::interaction::table_ivars(cell) {
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

pub fn cell(t: &TableIvars, row: usize, column: usize) -> Option<Retained<UIView>> {
    t.rows.borrow().get(row)?.get(column).cloned()
}

pub fn headers(t: &TableIvars, column: usize) -> Option<Retained<NSArray<UIView>>> {
    let rows = t.rows.borrow();
    let cells: Vec<Retained<UIView>> = rows
        .iter()
        .take(t.headers.get())
        .filter_map(|r| r.get(column).cloned())
        .collect();
    (!cells.is_empty()).then(|| NSArray::from_retained_slice(&cells))
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

/// A cell's row, when the cell belongs to a table.
pub fn row_of(t: &TableIvars) -> Option<Retained<UIView>> {
    t.at.get()?;
    t.row.borrow().load()
}

/// What the accessibility dump shows of a table or a cell.
pub enum Dump {
    /// A table's row and column counts and its header cells' labels.
    Table(isize, isize, Vec<Option<String>>),
    /// A cell's row and column.
    Cell(usize, usize),
}

pub fn dump(mtm: MainThreadMarker, view: &UIView) -> Option<Dump> {
    use objc2_ui_kit::NSObjectUIAccessibility;
    let t = crate::interaction::table_ivars(view)?;
    if let Some((r, c)) = t.at.get() {
        return Some(Dump::Cell(r, c));
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
