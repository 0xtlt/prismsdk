//! Text nodes as `GtkLabel`s styled with Pango attributes: font family,
//! any numeric weight, italic, letter spacing, absolute line height (Pango
//! 1.50), decoration, tabular digits (`tnum`), rich-text runs, inline links
//! (Pango markup `<a href>`, clickable, focusable and read with the label)
//! and truncation (`lines` + ellipsizing). The label is also what is
//! measured, so the shared layout sees exactly what is drawn.
//!
//! App fonts: every `.ttf`, `.otf` and `.ttc` in the assets' `fonts/`
//! folder is added to fontconfig before GTK starts.

use gtk::prelude::*;
use gtk::{glib, pango};
use prism_core::{Color, FontFamily, Style};
use prism_native::text::StyledText;

/// Where a label's content comes from: rebuilt when any part changes.
#[derive(Clone, Debug, Default)]
pub(crate) struct LabelState {
    pub text: String,
    /// Encoded runs (`prism_core::spans`); empty for a plain text.
    pub config: String,
    pub style: Style,
    /// The label itself (restyled when the appearance changes).
    pub label: Option<gtk::Label>,
}

/// Inline links are `<a href="prism-link:<id>">`; activating one reports
/// `UiEvent::MenuSelect(id)`.
pub(crate) const LINK_SCHEME: &str = "prism-link:";

/// The link id of a `prism-link:` URI.
pub(crate) fn link_id(uri: &str) -> Option<u64> {
    uri.strip_prefix(LINK_SCHEME)?.parse().ok()
}

fn pango_units(points: f32) -> i32 {
    (points * pango::SCALE as f32).round() as i32
}

fn rgba16(c: Color, dark: bool) -> (u16, u16, u16, u16) {
    let c = if dark {
        c.dark_variant()
    } else {
        c.light_variant()
    };
    let v = |x: f32| (x.clamp(0., 1.) * 65535.).round() as u16;
    (v(c.r), v(c.g), v(c.b), v(c.a))
}

/// Pango family of a Prism family (`None`: the label's own, the system font).
fn family_name(f: &FontFamily) -> Option<String> {
    match f {
        FontFamily::System => None,
        FontFamily::Monospace => Some("monospace".into()),
        FontFamily::Serif => Some("serif".into()),
        // No rounded design in the usual Linux font sets: the system font.
        FontFamily::Rounded => None,
        // A run-time font's alias (`cx.load_font`) → its file's family.
        FontFamily::Named(name) => Some(prism_native::fonts::resolve_name(name)),
    }
}

/// The label's markup: its text escaped, clickable runs inside `<a>`.
fn markup(t: &StyledText) -> String {
    let mut out = String::new();
    for r in &t.runs {
        let text = glib::markup_escape_text(&r.text);
        match r.link {
            Some(id) => {
                out.push_str(&format!("<a href=\"{LINK_SCHEME}{id}\">{text}</a>"));
            }
            None => out.push_str(&text),
        }
    }
    out
}

/// Configures `label` for `state` (its base font size, weight and color come
/// from the node's CSS rule) and returns what it shows.
pub(crate) fn apply(label: &gtk::Label, state: &LabelState, dark: bool) -> StyledText {
    let s = &state.style;
    let t = StyledText::new(&state.text, &state.config, s);
    if t.has_links() {
        label.set_markup(&markup(&t));
    } else {
        label.set_use_markup(false);
        label.set_text(&t.text());
    }

    let attrs = pango::AttrList::new();
    let whole = |mut a: pango::Attribute, attrs: &pango::AttrList| {
        a.set_start_index(0);
        a.set_end_index(u32::MAX);
        attrs.insert(a);
    };
    if t.letter_spacing != 0. {
        whole(
            pango::AttrInt::new_letter_spacing(pango_units(t.letter_spacing)).upcast(),
            &attrs,
        );
    }
    if let Some(lh) = t.line_height {
        // Pango splits the extra space above and below each line, like CSS.
        whole(
            pango::AttrInt::new_line_height_absolute(pango_units(lh)).upcast(),
            &attrs,
        );
    }
    if s.tabular_numbers {
        whole(pango::AttrFontFeatures::new("tnum").upcast(), &attrs);
    }
    for r in &t.runs {
        let (start, end) = (r.byte_start as u32, (r.byte_start + r.text.len()) as u32);
        let add = |mut a: pango::Attribute| {
            a.set_start_index(start);
            a.set_end_index(end);
            attrs.insert(a);
        };
        if let Some(name) = family_name(&r.font.family) {
            add(pango::AttrString::new_family(&name).upcast());
        }
        if r.font.weight != s.font_weight {
            let w: pango::Weight =
                unsafe { glib::translate::from_glib(r.font.weight.css() as i32) };
            add(pango::AttrInt::new_weight(w).upcast());
        }
        if r.font.italic {
            add(pango::AttrInt::new_style(pango::Style::Italic).upcast());
        }
        if r.font.size != s.font_size {
            add(pango::AttrSize::new_size_absolute(pango_units(r.font.size)).upcast());
        }
        if r.underline {
            add(pango::AttrInt::new_underline(pango::Underline::Single).upcast());
        }
        if r.strikethrough {
            add(pango::AttrInt::new_strikethrough(true).upcast());
        }
        // The node's own color is its CSS rule; runs may set theirs (links
        // too: they keep the text's color unless a run sets one).
        if let Some(c) = r
            .color
            .filter(|_| r.color != s.text_color || r.link.is_some())
        {
            let (red, green, blue, alpha) = rgba16(c, dark);
            add(pango::AttrColor::new_foreground(red, green, blue).upcast());
            add(pango::AttrInt::new_foreground_alpha(alpha).upcast());
        }
        if let Some(c) = r.background {
            let (red, green, blue, alpha) = rgba16(c, dark);
            add(pango::AttrColor::new_background(red, green, blue).upcast());
            add(pango::AttrInt::new_background_alpha(alpha).upcast());
        }
    }
    label.set_attributes(Some(&attrs));

    match (t.max_lines, t.no_wrap) {
        // One line, cut with an ellipsis.
        (Some(1), _) => {
            label.set_wrap(false);
            label.set_single_line_mode(true);
            label.set_ellipsize(pango::EllipsizeMode::End);
            label.set_lines(-1);
        }
        (Some(n), _) => {
            label.set_wrap(true);
            label.set_single_line_mode(false);
            label.set_ellipsize(pango::EllipsizeMode::End);
            label.set_lines(n.min(i32::MAX as u32) as i32);
        }
        // GTK never allocates a label narrower than its minimum, and an
        // unwrapped label's minimum is its whole line: a cut line ends with
        // an ellipsis instead of being clipped.
        (None, true) => {
            label.set_wrap(false);
            label.set_single_line_mode(false);
            label.set_ellipsize(pango::EllipsizeMode::End);
            label.set_lines(-1);
        }
        (None, false) => {
            label.set_wrap(true);
            label.set_single_line_mode(false);
            label.set_ellipsize(pango::EllipsizeMode::None);
            label.set_lines(-1);
        }
    }
    t
}

/// CSS of a label's links: the text's own color and no decoration (runs
/// set theirs with Pango attributes).
pub(crate) fn link_css(id: prism_core::NodeId) -> String {
    format!(".p{id} link{{color:inherit;text-decoration-line:none;}}\n")
}

#[link(name = "fontconfig")]
unsafe extern "C" {
    /// fontconfig.h: `FcBool FcConfigAppFontAddFile (FcConfig *config, const FcChar8 *file)`;
    /// a null config is the current one.
    fn FcConfigAppFontAddFile(config: *mut std::ffi::c_void, file: *const u8) -> std::ffi::c_int;
}

/// Adds the app's fonts (`<assets>/fonts/*.ttf|otf|ttc`, then those
/// compiled into the binary) to fontconfig's current configuration. Called
/// before GTK creates Pango's font map, so `font_family("Inter")` finds them
/// by family name.
pub(crate) fn register_fonts(
    assets: Option<&std::path::Path>,
    embedded: &prism_core::EmbeddedAssets,
) -> usize {
    let mut files: Vec<std::path::PathBuf> = assets
        .map(|a| a.join("fonts"))
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| prism_native::fonts::is_font(p))
        .collect();
    files.sort();
    files.extend(embedded_font_files(embedded, &glib::user_cache_dir()));
    files.into_iter().filter(|f| add_font_file(f)).count()
}

/// Adds one font file to fontconfig's current configuration.
fn add_font_file(file: &std::path::Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let mut path = file.as_os_str().as_bytes().to_vec();
    path.push(0);
    let ok = unsafe { FcConfigAppFontAddFile(std::ptr::null_mut(), path.as_ptr()) } != 0;
    if !ok {
        eprintln!("prism-linux: could not load font {}", file.display());
    }
    ok
}

#[link(name = "pangocairo-1.0")]
unsafe extern "C" {
    /// pangocairo.h: the font map GTK's widgets use (none set on them).
    fn pango_cairo_font_map_get_default() -> *mut std::ffi::c_void;
}

#[link(name = "pangoft2-1.0")]
unsafe extern "C" {
    /// pangofc-fontmap.h: the font map reads fontconfig's configuration again.
    fn pango_fc_font_map_config_changed(fontmap: *mut std::ffi::c_void);
}

/// Adds a font loaded while the app runs (`cx.load_font`, saved to a file):
/// fontconfig gets it, then Pango's font map reads the configuration again,
/// so labels laid out from now on find its family.
pub(crate) fn add_runtime_font(file: &std::path::Path) -> bool {
    if !add_font_file(file) {
        return false;
    }
    // SAFETY: the default font map is a PangoFcFontMap on Linux (fontconfig
    // backend); it is owned by Pango and outlives the call.
    unsafe {
        let map = pango_cairo_font_map_get_default();
        if !map.is_null() {
            pango_fc_font_map_config_changed(map);
        }
    }
    true
}

/// The embedded fonts as files, since fontconfig only reads files:
/// `<cache>/prism-fonts/<content hash>/<file name>`, written when missing.
fn embedded_font_files(
    embedded: &prism_core::EmbeddedAssets,
    cache: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut out = Vec::new();
    for (name, bytes) in prism_native::fonts::embedded_fonts(embedded) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        let file_name = name.rsplit('/').next().unwrap_or(name);
        let file = cache
            .join("prism-fonts")
            .join(format!("{:016x}", hasher.finish()))
            .join(file_name);
        let current = std::fs::metadata(&file).is_ok_and(|m| m.len() == bytes.len() as u64);
        let written = current
            || file
                .parent()
                .is_some_and(|dir| std::fs::create_dir_all(dir).is_ok())
                && std::fs::write(&file, bytes).is_ok();
        if written {
            out.push(file);
        } else {
            eprintln!("prism-linux: could not write the embedded font {name}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fonts_are_written_once_for_fontconfig() {
        let cache = std::env::temp_dir().join(format!("prism-linux-fonts-{}", std::process::id()));
        let embedded = prism_core::EmbeddedAssets::new()
            .file("fonts/sub/Brand.ttf", &b"font"[..])
            .file("logo.png", &b"png"[..]);
        let files = embedded_font_files(&embedded, &cache);
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("Brand.ttf") && files[0].starts_with(cache.join("prism-fonts")));
        assert_eq!(std::fs::read(&files[0]).unwrap(), b"font");
        assert_eq!(embedded_font_files(&embedded, &cache), files);
        let _ = std::fs::remove_dir_all(cache);
    }

    #[test]
    fn link_uris_carry_the_run_id() {
        assert_eq!(link_id("prism-link:65536"), Some(65536));
        assert_eq!(link_id("https://example.com"), None);
    }

    #[test]
    fn links_become_markup_anchors_and_text_is_escaped() {
        let runs = vec![
            prism_core::TextRun {
                text: "a < b ".into(),
                ..Default::default()
            },
            prism_core::TextRun {
                text: "go".into(),
                link: Some(9),
                ..Default::default()
            },
        ];
        let t = StyledText::new("", &prism_core::encode_runs(&runs), &Style::default());
        assert_eq!(markup(&t), "a &lt; b <a href=\"prism-link:9\">go</a>");
    }

    #[test]
    fn families_map_to_fontconfig_names() {
        assert_eq!(
            family_name(&FontFamily::Monospace).as_deref(),
            Some("monospace")
        );
        assert_eq!(family_name(&FontFamily::System), None);
        assert_eq!(family_name(&"Inter".into()).as_deref(), Some("Inter"));
    }
}
