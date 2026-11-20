//! Run-time fonts (`cx.load_font`): XAML loads font files only by URI, and
//! an unpackaged app's `ms-appx:///` is its executable's folder. The font
//! is written there (`prism-fonts\<hash>.ttf`, named by its content, once)
//! and texts in its family name it by that URI
//! (`ms-appx:///prism-fonts/<hash>.ttf#Family`), for this app only:
//! nothing is installed. Texts are then styled and measured again.
//!
//! XAML ignores files elsewhere (absolute paths, `file:` URIs) and fonts
//! added with `AddFontResourceEx`, so an app installed in a read-only
//! folder cannot load fonts at run time (it logs why).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use prism_core::FontFace;

use crate::imp::State;
use crate::win::log;

thread_local! {
    /// Family (lower case) → its XAML font source.
    static SOURCES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// The XAML source of a run-time font's family, if it is one.
pub fn source(family: &str) -> Option<String> {
    SOURCES.with(|s| s.borrow().get(&family.to_lowercase()).cloned())
}

/// `<exe dir>\prism-fonts`: what `ms-appx:///prism-fonts/` names.
fn folder() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("prism-fonts"))
}

pub fn load(s: &mut State, face: FontFace) {
    let w = &s.renderer.widgets;
    let sources = w.images.sources.clone();
    let requested = face.family.clone();
    // A URL comes back here with its bytes once loaded.
    let Some(font) = prism_native::fonts::prepare(&mut s.runtime.app, face, &sources) else {
        return;
    };
    let Some(dir) = folder() else { return };
    let Some(path) = prism_native::fonts::save(&font, &dir) else {
        log!(
            "font {requested:?}: cannot write it under {} (XAML loads run-time fonts only \
             from the app's folder)",
            dir.display()
        );
        return;
    };
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return;
    };
    let src = format!("ms-appx:///prism-fonts/{name}#{}", font.family);
    SOURCES.with(|m| {
        let mut m = m.borrow_mut();
        m.insert(font.family.to_lowercase(), src.clone());
        m.insert(requested.to_lowercase(), src);
    });
    // Every text again (styled and measured anew).
    s.renderer.restyle_all();
    crate::imp::relayout(s);
}
