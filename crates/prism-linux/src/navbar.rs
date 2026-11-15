//! A screen's bar buttons in the stack header (#30): flat `GtkButton`s
//! after Back (leading) and after the title (trailing), showing the
//! icon's glyph or the title (GTK has no SF Symbols). One title size:
//! `large_title` changes nothing. The tint colors the header's buttons and
//! the selected tab through CSS.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::prelude::*;
use prism_core::navbar::{BarItem, ScreenConfig};
use prism_core::{Color, NodeId, UiEvent};

thread_local! {
    static CONFIGS: RefCell<HashMap<NodeId, ScreenConfig>> = RefCell::default();
}

/// Records screen `id`'s config. Returns whether it changed.
pub(crate) fn set_config(id: NodeId, config: &str) -> bool {
    let c = ScreenConfig::decode(config);
    CONFIGS.with(|m| m.borrow_mut().insert(id, c.clone()) != Some(c))
}

/// Forgets a destroyed screen.
pub(crate) fn forget(id: NodeId) {
    CONFIGS.with(|m| m.borrow_mut().remove(&id));
}

/// What a button shows: its icon's glyph when there is one, else its title.
fn face(item: &BarItem) -> String {
    match prism_core::glyphs::GLYPHS
        .iter()
        .find(|(s, _)| *s == item.icon)
    {
        Some((_, g)) => (*g).to_string(),
        None if item.title.is_empty() => prism_core::glyphs::glyph(&item.icon).to_string(),
        None => item.title.clone(),
    }
}

/// Puts screen `top`'s buttons in `header`: leading ones after `back`,
/// trailing ones after `title` (replacing the previous screen's).
pub(crate) fn refresh(
    header: &gtk::Box,
    back: &gtk::Button,
    title: &gtk::Label,
    top: Option<NodeId>,
) {
    let mut child = header.first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        if c.has_css_class("prism-bar-button") {
            header.remove(&c);
        }
    }
    let Some(top) = top else { return };
    let config = CONFIGS
        .with(|m| m.borrow().get(&top).cloned())
        .unwrap_or_default();
    let button = |index: usize, item: &BarItem| {
        let b = gtk::Button::with_label(&face(item));
        b.set_has_frame(false);
        b.add_css_class("prism-bar-button");
        b.set_tooltip_text(Some(item.spoken()));
        crate::imp::set_accessible_string(&b, crate::imp::a11y_property::LABEL, item.spoken());
        b.connect_clicked(move |_| crate::imp::dispatch(UiEvent::Change(top, index as f64)));
        b
    };
    let mut after: gtk::Widget = back.clone().upcast();
    for (i, item) in config.leading() {
        let b = button(i, item);
        header.insert_child_after(&b, Some(&after));
        after = b.upcast();
    }
    let mut after: gtk::Widget = title.clone().upcast();
    for (i, item) in config.trailing() {
        let b = button(i, item);
        header.insert_child_after(&b, Some(&after));
        after = b.upcast();
    }
}

/// CSS for a tab bar's or a stack's tint (node `id`, class `p{id}`).
pub(crate) fn tint_rules(id: NodeId, tabs: bool, tint: Option<Color>, dark: bool) -> String {
    let Some(tint) = tint else {
        return String::new();
    };
    let color = crate::decor::color(tint, dark);
    if tabs {
        format!(".p{id} > .prism-tabbar > button.toggle:checked{{color:{color};}}\n")
    } else {
        format!(".p{id} > .prism-header > button{{color:{color};}}\n")
    }
}
