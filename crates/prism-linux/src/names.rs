//! Accessible names on GTK before 4.12.
//!
//! GTK 4.8 and 4.9 name an element that has no label after its GObject
//! type when its role is not a structural one (gtkatcontext.c,
//! `gtk_at_context_get_name_accumulate`): a table row reads "PrismBox", a
//! scroll bar "GtkScrollbar", a tab "GtkToggleButton". GTK 4.10 removed
//! that fallback (the name is then empty), and GTK 4.12 added ARIA's name
//! from content: a row, a cell, a button or a tab without a label is named
//! after the text inside it.
//!
//! Before 4.12, after each update this pass gives every such element the
//! name GTK 4.12 would: the text of its content for the roles named from
//! content (`NAME_FROM_CONTENT` in GTK 4.12's gtkatcontext.c), an empty
//! name for the others. Labels set by Prism or by GTK itself are kept;
//! the names this pass set are recomputed as the content changes. Plain
//! layout boxes (structural roles) are left without a name.

use std::cell::Cell;

use gtk::glib;
use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;

/// Data key: the name this pass gave an element.
const AUTO: &str = "prism-a11y-auto-name";
/// Data key: the label Prism gave an element (read for its parents' names).
const LABEL: &str = "prism-a11y-label";
/// Data key: an element Prism hid from screen readers.
const HIDDEN: &str = "prism-a11y-hidden";

/// `GtkAccessibleProperty` / `GtkAccessibleRelation` values (gtkenums.h).
const PROPERTY_LABEL: i32 = 4;
const RELATION_LABELLED_BY: i32 = 10;

unsafe extern "C" {
    /// gtktestatcontext.h, `GDK_AVAILABLE_IN_ALL` (gtk-rs binds them from
    /// 4.10): whether an element has a property or relation at all.
    fn gtk_test_accessible_has_property(accessible: *mut std::ffi::c_void, property: i32) -> i32;
    fn gtk_test_accessible_has_relation(accessible: *mut std::ffi::c_void, relation: i32) -> i32;
}

thread_local! {
    static SCHEDULED: Cell<bool> = const { Cell::new(false) };
}

/// Whether this GTK needs the pass (read at run time: the app may run on a
/// newer GTK than it was built for).
fn needed() -> bool {
    gtk::major_version() == 4 && gtk::minor_version() < 12
}

/// Runs the pass once the current updates are done.
pub fn schedule() {
    if !needed() || SCHEDULED.with(|s| s.replace(true)) {
        return;
    }
    glib::idle_add_local_full(glib::Priority::LOW, || {
        SCHEDULED.with(|s| s.set(false));
        for window in gtk::Window::list_toplevels() {
            sweep(&window);
        }
        glib::ControlFlow::Break
    });
}

/// Remembers the label Prism gave `w` (its parents' names read it); the
/// pass leaves `w`'s own name alone from now on.
pub fn set_label(w: &gtk::Widget, label: &str) {
    // SAFETY: the keys are only ever used with `String`.
    unsafe {
        w.set_data(LABEL, label.to_owned());
        let _ = w.steal_data::<String>(AUTO);
    }
}

/// Forgets the label Prism gave `w`; whether it had one.
pub fn take_label(w: &gtk::Widget) -> bool {
    // SAFETY: as in `set_label`.
    unsafe { w.steal_data::<String>(LABEL).is_some() }
}

/// Remembers that Prism hid `w` from screen readers (its parents' names
/// skip it).
pub fn set_hidden(w: &gtk::Widget, hidden: bool) {
    // SAFETY: the key is only ever used with `bool`.
    unsafe {
        if hidden {
            w.set_data(HIDDEN, true);
        } else {
            let _ = w.steal_data::<bool>(HIDDEN);
        }
    }
}

/// Whether Prism gave `w` a label.
pub fn has_label(w: &gtk::Widget) -> bool {
    label_of(w).is_some()
}

fn label_of(w: &gtk::Widget) -> Option<String> {
    // SAFETY: as in `set_label`.
    unsafe { w.data::<String>(LABEL).map(|p| p.as_ref().clone()) }
}

fn hidden(w: &gtk::Widget) -> bool {
    // SAFETY: as in `set_hidden`.
    unsafe { w.data::<bool>(HIDDEN).is_some() }
}

fn auto_name(o: &impl IsA<glib::Object>) -> Option<String> {
    // SAFETY: as in `set_label`.
    unsafe { o.data::<String>(AUTO).map(|p| p.as_ref().clone()) }
}

/// Roles GTK 4.12 names from their content (gtkatcontext.c, the
/// `NAME_FROM_CONTENT` ones).
fn named_from_content(role: gtk::AccessibleRole) -> bool {
    use gtk::AccessibleRole as R;
    matches!(
        role,
        R::Button
            | R::Cell
            | R::Checkbox
            | R::ColumnHeader
            | R::GridCell
            | R::Heading
            | R::Label
            | R::Link
            | R::MenuItem
            | R::MenuItemCheckbox
            | R::MenuItemRadio
            | R::Option
            | R::Radio
            | R::Row
            | R::RowHeader
            | R::Switch
            | R::Tab
            | R::Tooltip
            | R::TreeItem
            | R::Widget
    )
}

/// Roles GTK 4.8 never names after the type (gtkatcontext.c,
/// `is_structural_role`).
fn structural(role: gtk::AccessibleRole) -> bool {
    use gtk::AccessibleRole as R;
    matches!(
        role,
        R::Form
            | R::Group
            | R::Generic
            | R::Landmark
            | R::ListItem
            | R::Region
            | R::Search
            | R::Separator
    )
}

/// The role of a GtkAccessible (gtk-rs's `accessible_role()` needs 4.10).
fn role_of(acc: &impl IsA<glib::Object>) -> gtk::AccessibleRole {
    acc.property("accessible-role")
}

fn shown(w: &gtk::Widget) -> bool {
    w.is_visible() && w.is_child_visible() && !hidden(w)
}

/// What `w` reads as when its parent is named from its content: its label,
/// its text, else its children's, as ARIA computes a name from content.
fn content_name(w: &gtk::Widget) -> String {
    let mut parts = Vec::new();
    let mut child = w.first_child();
    while let Some(c) = child {
        collect(&c, &mut parts);
        child = c.next_sibling();
    }
    parts.join(" ")
}

fn collect(w: &gtk::Widget, parts: &mut Vec<String>) {
    if !shown(w) {
        return;
    }
    let text = if let Some(label) = label_of(w) {
        label
    } else if let Some(l) = w.downcast_ref::<gtk::Label>() {
        l.text().to_string()
    } else if let Some(e) = w.dynamic_cast_ref::<gtk::Editable>() {
        e.text().to_string()
    } else if let Some(b) = w.downcast_ref::<gtk::Button>()
        && let Some(label) = b.label()
    {
        label.to_string()
    } else {
        let name = content_name(w);
        if !name.is_empty() {
            parts.push(name);
        }
        return;
    };
    let text = text.trim();
    if !text.is_empty() {
        parts.push(text.to_owned());
    }
}

fn sweep(w: &impl IsA<gtk::Widget>) {
    let w = w.as_ref();
    let role = role_of(w);
    let name = match w.downcast_ref::<gtk::Label>() {
        // A label is named by its own text (GTK sets it as it changes).
        Some(l) => l.text().to_string(),
        None if named_from_content(role) => content_name(w),
        None => String::new(),
    };
    fix(w, role, name);
    if let Some(stack) = w.downcast_ref::<gtk::Stack>() {
        let pages = stack.pages();
        for i in 0..pages.n_items() {
            if let Some(page) = pages.item(i).and_downcast::<gtk::StackPage>() {
                fix(&page, role_of(&page), String::new());
            }
        }
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        sweep(&c);
        child = c.next_sibling();
    }
}

/// Gives `acc` `name` when it has no label of its own and GTK would name it
/// after its type (4.8, 4.9) or leave it unnamed though GTK 4.12 names it
/// from its content (4.10, 4.11).
fn fix(acc: &impl IsA<glib::Object>, role: gtk::AccessibleRole, name: String) {
    let previous = auto_name(acc);
    if previous.is_none() {
        let ptr: *mut glib::gobject_ffi::GObject = acc.as_ref().to_glib_none().0;
        // SAFETY: `acc` is a live widget or stack page, a GtkAccessible.
        let labelled = unsafe {
            gtk_test_accessible_has_property(ptr.cast(), PROPERTY_LABEL) != 0
                || gtk_test_accessible_has_relation(ptr.cast(), RELATION_LABELLED_BY) != 0
        };
        let leaks = gtk::minor_version() < 10 && !structural(role);
        if labelled || (name.is_empty() && !leaks) {
            return;
        }
    } else if previous.as_deref() == Some(name.as_str()) {
        return;
    }
    crate::imp::update_string(acc, PROPERTY_LABEL, &name);
    // SAFETY: as in `set_label`.
    unsafe { acc.set_data(AUTO, name) };
}
