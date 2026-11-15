//! Bundling scripts and platform sources (Apple, Android), embedded so `prism` works in
//! any project. Extracted under `<target>/prism/platform` when they change.
//!
//! The scripts and the Android host live in this crate (`platform/`), the
//! Apple sources in `prism-apple-build` (`apple/`), which also compiles them
//! for the Apple backends. Both lists are generated from the directories.

use std::path::{Path, PathBuf};

/// This crate's `platform/` directory (`scripts/…`, `android/…`), listed by
/// `build.rs`.
const FILES: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/embedded.rs"));

/// Writes the files (if needed) and returns the platform directory, laid out
/// as `scripts/…`, `apple/…` and `android/…`.
pub fn extract(target_dir: &Path) -> Result<PathBuf, String> {
    let dir = target_dir.join("prism").join("platform");
    let own = FILES
        .iter()
        .map(|(path, contents)| (PathBuf::from(path), *contents));
    let apple = prism_apple_build::SOURCES
        .iter()
        .map(|(path, contents)| (Path::new("apple").join(path), *contents));
    for (rel, contents) in own.chain(apple) {
        let dest = dir.join(rel);
        if std::fs::read_to_string(&dest).ok().as_deref() == Some(contents) {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&dest, contents).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    Ok(dir)
}
