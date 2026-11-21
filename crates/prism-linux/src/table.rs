//! Tables for screen readers (#284).
//!
//! The table's widget has `GTK_ACCESSIBLE_ROLE_TABLE`, its rows `ROW`, the
//! header row's cells `COLUMN_HEADER` and the others `CELL` (set as they
//! are created: GTK takes a role only before a widget is realized).
//! `set_table` gives the table its row and column counts and each row and
//! cell its index (`GTK_ACCESSIBLE_RELATION_ROW_COUNT`, `ROW_INDEX`,
//! `COL_INDEX`…, counted from 1 as in ARIA).
//!
//! What reaches screen readers: no GTK 4 (4.8 to 4.25) implements AT-SPI's
//! `Table` and `TableCell` interfaces (gtk/a11y has no vtable for them),
//! which Orca needs for a cell's coordinates, its column header and table
//! navigation; nor does any export the integer counts and indexes above
//! (gtkatspicontext.c exports `colindextext` and `rowindextext` from 4.14,
//! `posinset` and `setsize` from 4.22, as attributes). They stay set for
//! the GTK versions that will. What Orca gets: the table, its rows (named
//! after their content, see `names.rs` before GTK 4.12) and its cells, each
//! described by its column's header (`DESCRIBED_BY`), whose text Orca reads
//! after the cell. GTK up to 4.24 exposes `COLUMN_HEADER` as a filler
//! (gtkatspiutils.c, `gtk_accessible_role_to_atspi_role`), 4.25 as a column
//! header: until then the header's text is read, not its role.
//!
//! Not set: `COL_INDEX_TEXT` and `ROW_INDEX_TEXT`, which Orca reads as a
//! spreadsheet cell's coordinates ("Time2"), and `POS_IN_SET`: a row is not
//! an item of a list. Prism tables have no row headers (`ROW_HEADER`) and
//! no cell navigation (`GRID`).

use gtk::glib::translate::{ToGlibPtr, ToGlibPtrMut};
use gtk::prelude::*;
use prism_core::NodeKind;

/// `GtkAccessibleRelation` values (gtkenums.h).
const COL_COUNT: i32 = 1;
const COL_INDEX: i32 = 2;
const DESCRIBED_BY: i32 = 6;
pub const LABELLED_BY: i32 = 10;
const ROW_COUNT: i32 = 13;
const ROW_INDEX: i32 = 14;

unsafe extern "C" {
    /// gtkaccessible.h, `GDK_AVAILABLE_IN_ALL` (gtk-rs binds them from 4.10).
    fn gtk_accessible_update_relation_value(
        accessible: *mut std::ffi::c_void,
        n_relations: std::ffi::c_int,
        relations: *const i32,
        values: *const gtk::glib::gobject_ffi::GValue,
    );
    fn gtk_accessible_reset_relation(accessible: *mut std::ffi::c_void, relation: i32);
}

fn accessible(w: &gtk::Widget) -> *mut std::ffi::c_void {
    let widget: *mut gtk::ffi::GtkWidget = w.to_glib_none().0;
    widget.cast()
}

/// Sets integer relations (counts and indexes) of `w`.
fn set_ints(w: &gtk::Widget, relations: &[(i32, i32)]) {
    for &(relation, n) in relations {
        let mut value = n.to_value();
        // SAFETY: every GtkWidget implements GtkAccessible; one integer
        // relation.
        unsafe {
            gtk_accessible_update_relation_value(
                accessible(w),
                1,
                &relation,
                value.to_glib_none_mut().0,
            );
        }
    }
}

/// `w`'s `relation` (`DESCRIBED_BY`, `LABELLED_BY`) points to `to` (`None`:
/// to nothing).
pub fn set_relation(w: &gtk::Widget, relation: i32, to: Option<&gtk::Widget>) {
    let Some(to) = to else {
        // SAFETY: as above.
        unsafe { gtk_accessible_reset_relation(accessible(w), relation) };
        return;
    };
    let mut value = gtk::glib::Value::from_type(gtk::glib::Type::POINTER);
    // SAFETY: a reference list relation is a `GList` of `GtkAccessible`s in
    // a pointer value, the list (not its widgets) given to GTK (as gtk-rs's
    // `Relation::DescribedBy` does).
    unsafe {
        let list = gtk::glib::ffi::g_list_prepend(std::ptr::null_mut(), accessible(to));
        gtk::glib::gobject_ffi::g_value_set_pointer(value.to_glib_none_mut().0, list.cast());
        gtk_accessible_update_relation_value(
            accessible(w),
            1,
            &relation,
            value.to_glib_none_mut().0,
        );
    }
}

/// The role of a table part's widget.
pub fn role(kind: NodeKind) -> Option<gtk::AccessibleRole> {
    Some(match kind {
        NodeKind::Table => gtk::AccessibleRole::Table,
        NodeKind::TableRow => gtk::AccessibleRole::Row,
        NodeKind::ColumnHeader => gtk::AccessibleRole::ColumnHeader,
        NodeKind::TableCell => gtk::AccessibleRole::Cell,
        _ => return None,
    })
}

/// Hands the table its rows: each row's widget, whether it is a header row,
/// and its cells.
pub fn set(table: &gtk::Widget, rows: &[(gtk::Widget, bool, Vec<gtk::Widget>)]) {
    let columns = rows.iter().map(|r| r.2.len()).max().unwrap_or(0);
    set_ints(
        table,
        &[(ROW_COUNT, rows.len() as i32), (COL_COUNT, columns as i32)],
    );
    let headers: Vec<gtk::Widget> = rows
        .iter()
        .take_while(|r| r.1)
        .last()
        .map(|r| r.2.clone())
        .unwrap_or_default();
    for (r, (row, header, cells)) in rows.iter().enumerate() {
        let index = r as i32 + 1;
        set_ints(row, &[(ROW_INDEX, index)]);
        for (c, cell) in cells.iter().enumerate() {
            set_ints(cell, &[(ROW_INDEX, index), (COL_INDEX, c as i32 + 1)]);
            set_relation(cell, DESCRIBED_BY, headers.get(c).filter(|_| !header));
        }
    }
}
