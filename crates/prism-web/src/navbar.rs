//! A screen's bar buttons in the stack header (#30), as the client builds
//! them: `<button class="prism-bar-button" data-bar="i">`, in
//! `.prism-bar-leading` (after Back) and `.prism-bar-trailing`.

use prism_core::navbar::{BarItem, ScreenConfig};

use crate::html::escape;

/// What a bar button shows: its icon's glyph when the icon has one
/// (`plus` → `+`), else its title.
pub fn face(item: &BarItem) -> String {
    let known = prism_core::glyphs::GLYPHS
        .iter()
        .find(|(s, _)| *s == item.icon)
        .map(|(_, g)| *g);
    match known {
        Some(g) => g.to_string(),
        None if item.title.is_empty() => prism_core::glyphs::glyph(&item.icon).to_string(),
        None => item.title.clone(),
    }
}

/// The buttons of one side of the bar of a screen with this config.
pub fn buttons(config: &str, trailing: bool) -> String {
    let c = ScreenConfig::decode(config);
    let side: Vec<(usize, &BarItem)> = if trailing {
        c.trailing().collect()
    } else {
        c.leading().collect()
    };
    side.into_iter()
        .map(|(i, item)| {
            format!(
                r#"<button type="button" class="prism-bar-button" data-bar="{i}" aria-label="{}">{}</button>"#,
                escape(item.spoken()),
                escape(&face(item))
            )
        })
        .collect()
}
