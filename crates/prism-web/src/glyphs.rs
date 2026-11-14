//! SF Symbol names → glyphs, so the same `image("…")` works on the web.
//! Decorative only: meaning must come from labels, never from the glyph.

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
];

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
                prism_core::surfaces::json_str(s),
                prism_core::surfaces::json_str(g)
            )
        })
        .collect();
    format!("{{{}}}", entries.join(","))
}
