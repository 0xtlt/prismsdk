//! Padding on leaves (#14). `prism-native` lays a padded leaf out with its
//! padding around the measured content and hands the padding over in the
//! style (left/right already mirrored right to left): GTK draws it as the
//! widget's CSS padding (texts, timers, buttons and entries, replacing the
//! theme's own), or as a text view's margins (text areas). `measure`
//! returns the content without it.

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::{Edges, NodeId, Style};

thread_local! {
    static PADDING: RefCell<HashMap<NodeId, Edges>> = RefCell::new(HashMap::new());
}

/// Remembers the padding of leaf `id` (zero: forgets it).
pub fn remember(id: NodeId, s: &Style) {
    PADDING.with(|p| {
        let mut p = p.borrow_mut();
        if s.padding.is_zero() {
            p.remove(&id);
        } else {
            p.insert(id, s.padding);
        }
    });
}

/// The padding of leaf `id` (zero when it has none).
pub fn of(id: NodeId) -> Edges {
    PADDING.with(|p| p.borrow().get(&id).copied().unwrap_or_default())
}

/// The CSS padding of a widget that draws its own (empty when unset: the
/// theme's).
pub fn css(s: &Style) -> String {
    let p = s.padding;
    if p.is_zero() {
        return String::new();
    }
    format!(
        "padding:{}px {}px {}px {}px;",
        p.top, p.right, p.bottom, p.left
    )
}

/// `.border(0.)`: not even the theme's frame around a field (its border
/// and the shadow GTK draws it with).
pub const NO_BORDER: &str = "border:none;box-shadow:none;outline:none;";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_becomes_css_only_when_set() {
        assert_eq!(css(&Style::default()), "");
        let s = Style {
            padding: Edges {
                top: 4.,
                right: 12.,
                bottom: 4.,
                left: 40.,
            },
            ..Style::default()
        };
        assert_eq!(css(&s), "padding:4px 12px 4px 40px;");
    }
}
