//! Translations: `locales/*.json` (English, French, Arabic — right-to-left),
//! compiled by `build.rs` into one checked function per key.

use prism::prelude::*;

prism::include_strings!();

/// Installs the catalogs and the language saved in Settings.
pub fn install(cx: &mut App) {
    cx.set_translations(translations());
    if let Some(tag) = cx.storage().get("locale") {
        cx.set_locale(Some(Locale::new(&tag)));
    }
}

/// Switches language (`None`: follow the system) and remembers it.
pub fn choose(cx: &mut App, tag: Option<&str>) {
    match tag {
        Some(tag) => cx.storage().set("locale", tag),
        None => cx.storage().remove("locale"),
    }
    cx.set_locale(tag.map(Locale::new));
    // Menus and widgets live outside the views: rebuild them too.
    crate::menus::refresh(cx);
    cx.reload_widgets();
}

/// A language's own name ("Français"), from its catalog.
pub fn native_name(cx: &App, lang: &str) -> String {
    cx.try_global::<Translations>()
        .and_then(|t| t.lookup(&Locale::new(lang), "language.name"))
        .unwrap_or(lang)
        .to_string()
}

pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0., |d| d.as_secs_f64())
}
