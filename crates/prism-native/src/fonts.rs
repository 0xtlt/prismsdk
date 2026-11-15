//! Fonts bundled with the app: every `.ttf`, `.otf` or `.ttc` file in
//! `assets/fonts/` is registered with the platform at startup (Core Text on
//! Apple platforms, fontconfig on Linux, `Typeface.createFromAsset` on
//! Android, `@font-face` on the web), so `.font_family("Inter")` finds it by
//! its family name.

use std::path::{Path, PathBuf};

/// The font files in `<dir>/fonts/` of each asset directory (sorted).
pub fn font_files(asset_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in asset_dirs {
        collect(&dir.join("fonts"), &mut out);
    }
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            collect(&p, out);
        } else if is_font(&p) {
            out.push(p);
        }
    }
}

/// A TrueType / OpenType font or collection.
pub fn is_font(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
}

/// Registers the bundled fonts for this process with Core Text (AppKit and
/// UIKit then find them by family). Returns how many were registered.
#[cfg(target_vendor = "apple")]
pub fn register_apple_fonts(asset_dirs: &[PathBuf]) -> usize {
    use objc2_foundation::{NSString, NSURL};
    #[link(name = "CoreText", kind = "framework")]
    unsafe extern "C" {
        fn CTFontManagerRegisterFontsForURL(
            url: *const std::ffi::c_void,
            scope: u32,
            error: *mut *mut std::ffi::c_void,
        ) -> bool;
    }
    // kCTFontManagerScopeProcess (CTFontManager.h).
    const SCOPE_PROCESS: u32 = 1;
    let mut n = 0;
    for file in font_files(asset_dirs) {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&file.to_string_lossy()));
        // SAFETY: NSURL is toll-free bridged with CFURL; a null error pointer
        // is allowed. Already registered (same file twice) just fails.
        let ok = unsafe {
            CTFontManagerRegisterFontsForURL(
                objc2::rc::Retained::as_ptr(&url).cast(),
                SCOPE_PROCESS,
                std::ptr::null_mut(),
            )
        };
        n += ok as usize;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_are_found_under_assets_fonts() {
        let dir = std::env::temp_dir().join(format!("prism-fonts-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("fonts/extra")).unwrap();
        for f in [
            "fonts/B.otf",
            "fonts/A.TTF",
            "fonts/extra/C.ttc",
            "fonts/readme.txt",
            "logo.png",
        ] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let files: Vec<String> = font_files(std::slice::from_ref(&dir))
            .iter()
            .map(|p| {
                p.strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(files, ["fonts/A.TTF", "fonts/B.otf", "fonts/extra/C.ttc"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
