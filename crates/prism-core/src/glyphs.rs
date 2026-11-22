//! SF Symbol names → glyphs, so the same `image("…")` works where SF
//! Symbols do not exist (web, Linux).
//! Decorative only: meaning must come from labels, never from the glyph.

/// SF Symbol name → glyph, for the symbols Prism's components and examples use.
pub const GLYPHS: &[(&str, &str)] = &[
    ("house", "⌂"),
    ("house.fill", "⌂"),
    ("gear", "⚙"),
    ("gearshape", "⚙"),
    ("gearshape.fill", "⚙"),
    ("checklist", "☑"),
    ("list.bullet", "☰"),
    ("star", "☆"),
    ("star.fill", "★"),
    ("arrow.clockwise", "↻"),
    ("person", "☺"),
    ("person.crop.circle", "☺"),
    ("magnifyingglass", "⌕"),
    ("bell", "🔔"),
    ("bell.fill", "🔔"),
    ("heart", "♡"),
    ("heart.fill", "♥"),
    ("square.grid.2x2", "▦"),
    ("paintbrush", "🖌"),
    ("paintpalette", "🎨"),
    ("timer", "⏱"),
    ("stopwatch", "⏱"),
    ("clock", "🕒"),
    ("checkmark", "✓"),
    ("checkmark.circle", "✓"),
    ("checkmark.circle.fill", "✓"),
    ("circle", "○"),
    ("xmark", "✕"),
    ("plus", "+"),
    ("minus", "−"),
    ("chevron.right", "›"),
    ("chevron.left", "‹"),
    ("brain.head.profile", "🧠"),
    ("cup.and.saucer", "☕"),
    ("cup.and.saucer.fill", "☕"),
    ("flame", "🔥"),
    ("flame.fill", "🔥"),
    ("bolt", "⚡"),
    ("bolt.fill", "⚡"),
    ("sparkles", "✨"),
    ("app.badge", "🔔"),
    ("rectangle.stack", "▤"),
    // Bar buttons (#30).
    ("pencil", "✎"),
    ("square.and.pencil", "✎"),
    ("trash", "🗑"),
    ("ellipsis", "…"),
    ("ellipsis.circle", "…"),
    ("square.and.arrow.up", "⇪"),
    // Documents, as in a page's chip (`chip(..).chip_icon("doc.text")`).
    ("doc", "📄"),
    ("doc.text", "📄"),
];

/// The glyph for an SF Symbol name, `•` if unknown.
pub fn glyph(symbol: &str) -> &'static str {
    GLYPHS
        .iter()
        .find(|(s, _)| *s == symbol)
        .map_or("•", |(_, g)| g)
}

/// The table as a JS object literal, for the client.
pub fn js_table() -> String {
    let entries: Vec<String> = GLYPHS
        .iter()
        .map(|(s, g)| {
            format!(
                "{}:{}",
                crate::surfaces::json_str(s),
                crate::surfaces::json_str(g)
            )
        })
        .collect();
    format!("{{{}}}", entries.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_chip_has_a_document_glyph() {
        // The icon of the docs' chip example, drawn from this table on
        // Android and the web: not the unknown symbol's bullet.
        assert_eq!(glyph("doc.text"), "📄");
        assert_eq!(glyph("no.such.symbol"), "•");
    }

    #[test]
    fn each_symbol_is_listed_once() {
        for (i, (name, _)) in GLYPHS.iter().enumerate() {
            assert!(GLYPHS[..i].iter().all(|(n, _)| n != name), "{name} twice");
        }
    }
}
