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

/// Remembers the padding of leaf `id` (none set: forgets it).
pub fn remember(id: NodeId, s: &Style) {
    PADDING.with(|p| {
        let mut p = p.borrow_mut();
        if !s.has_padding() {
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
/// theme's; `.p(0.)` is a real zero, #69).
pub fn css(s: &Style) -> String {
    let p = s.padding;
    if !s.has_padding() {
        return String::new();
    }
    format!(
        "padding:{}px {}px {}px {}px;",
        p.top, p.right, p.bottom, p.left
    )
}

/// A padded entry is as tall as its text plus its padding, not the
/// theme's minimum height (#69).
pub fn entry_css(s: &Style) -> &'static str {
    if s.has_padding() { "min-height:0;" } else { "" }
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

    #[test]
    fn an_explicit_zero_padding_is_a_real_zero() {
        use prism_core::{Styled, text_input};
        let mut input = text_input().p(0.);
        let s = input.style_mut().clone();
        assert_eq!(css(&s), "padding:0px 0px 0px 0px;");
        assert_eq!(entry_css(&s), "min-height:0;");
        assert_eq!(entry_css(&Style::default()), "");
    }
}
