//! Fonts bundled with the app: every `.ttf`, `.otf` or `.ttc` file in
//! `assets/fonts/` is registered with the platform at startup (Core Text on
//! Apple platforms, fontconfig on Linux, `Typeface.createFromAsset` on
//! Android, `@font-face` on the web), so `.font_family("Inter")` finds it by
//! its family name. Fonts compiled into the binary
//! ([`EmbeddedAssets`], under `fonts/`) are
//! registered too.
//!
//! Fonts loaded while the app runs (`cx.load_font`, see
//! [`prism_core::fonts`]) go through [`prepare`]: a URL is loaded in the
//! background (remote, a file, or an asset, embedded ones included), the
//! file's own family becomes the target of the name the app gave
//! ([`resolve_name`]), and the backend registers the bytes, then draws texts
//! again ([`NativeRenderer::restyle_all`](crate::NativeRenderer::restyle_all)).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use prism_core::{App, EmbeddedAssets, FontFace, FontFamily, FontSource};

use crate::images::ImageSources;

/// The font files under `fonts/` of the embedded assets: (name, contents).
pub fn embedded_fonts(embedded: &EmbeddedAssets) -> impl Iterator<Item = (&str, &[u8])> {
    embedded
        .iter()
        .filter(|(name, _)| name.starts_with("fonts/") && is_font(Path::new(name)))
}

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
/// UIKit then find them by family): the files of each asset directory's
/// `fonts/`, then the embedded ones (from memory, the first face of a
/// collection). Returns how many were registered.
#[cfg(target_vendor = "apple")]
pub fn register_apple_fonts(asset_dirs: &[PathBuf], embedded: &EmbeddedAssets) -> usize {
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
    for (name, bytes) in embedded_fonts(embedded) {
        if register_apple_font_data(bytes) {
            n += 1;
        } else {
            eprintln!("prism: could not register the embedded font {name}");
        }
    }
    n
}

/// Registers one font from memory with Core Text, for this process
/// (`CTFontManagerRegisterGraphicsFont`): embedded fonts, and those loaded
/// while the app runs. False if Core Text refused it (already registered
/// included).
#[cfg(target_vendor = "apple")]
pub fn register_apple_font_data(bytes: &[u8]) -> bool {
    use std::ffi::c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *mut c_void;
        fn CFRelease(cf: *const c_void);
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGDataProviderCreateWithCFData(data: *const c_void) -> *mut c_void;
        fn CGFontCreateWithDataProvider(provider: *const c_void) -> *mut c_void;
    }
    #[link(name = "CoreText", kind = "framework")]
    unsafe extern "C" {
        fn CTFontManagerRegisterGraphicsFont(font: *const c_void, error: *mut *mut c_void) -> bool;
    }
    let Ok(len) = isize::try_from(bytes.len()) else {
        return false;
    };
    // SAFETY: Core Foundation create rules: every object created here is
    // released once; CFDataCreate copies the bytes; a null error pointer is
    // allowed, and Core Text keeps its own reference to a registered font.
    unsafe {
        let data = CFDataCreate(std::ptr::null(), bytes.as_ptr(), len);
        if data.is_null() {
            return false;
        }
        let provider = CGDataProviderCreateWithCFData(data);
        CFRelease(data);
        if provider.is_null() {
            return false;
        }
        let font = CGFontCreateWithDataProvider(provider);
        CFRelease(provider);
        if font.is_null() {
            return false;
        }
        let ok = CTFontManagerRegisterGraphicsFont(font, std::ptr::null_mut());
        CFRelease(font);
        ok
    }
}

/// App family (lower case) → the family its file declares.
static ALIASES: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

fn aliases() -> std::sync::MutexGuard<'static, BTreeMap<String, String>> {
    ALIASES.lock().unwrap_or_else(|e| e.into_inner())
}

/// The family the platform knows `name` by: the font file's own family when
/// `name` was given to a run-time font that declares another one.
pub fn resolve_name(name: &str) -> String {
    aliases()
        .get(&name.to_lowercase())
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

/// [`resolve_name`] for a family of a style.
pub fn resolve(family: &FontFamily) -> FontFamily {
    match family {
        FontFamily::Named(name) => FontFamily::Named(resolve_name(name)),
        other => other.clone(),
    }
}

/// A run-time font ready for the platform: the file and its own family.
#[derive(Clone, Debug)]
pub struct ReadyFont {
    /// What the app asked for.
    pub face: FontFace,
    /// The font file.
    pub bytes: Arc<[u8]>,
    /// The family the file declares (what the platform registers).
    pub family: String,
}

/// The native side of `PlatformRequest::LoadFont`. Bytes come back ready to
/// register (`None` when they are not a TrueType / OpenType font); a URL is
/// loaded in the background (like a picture: remote, `file://`, an asset)
/// and asked for again with its bytes, so it comes back through the
/// backend's requests.
pub fn prepare(app: &mut App, face: FontFace, sources: &ImageSources) -> Option<ReadyFont> {
    let bytes = match &face.source {
        FontSource::Bytes(b) => b.clone(),
        FontSource::Url(url) => {
            let (url, sources) = (url.clone(), sources.clone());
            app.spawn(async move |cx| {
                let source = url.clone();
                let bytes = cx
                    .background(move || crate::images::load(&source, &sources))
                    .await;
                match bytes {
                    Some(b) => cx.update(|app| app.load_font(face.with_bytes(b))),
                    None => eprintln!("prism: font {:?}: cannot load {url}", face.family),
                }
            });
            return None;
        }
    };
    let Some(info) = prism_core::fonts::font_file_info(&bytes) else {
        eprintln!(
            "prism: font {:?}: not a TrueType or OpenType file (WOFF and WOFF2 are web only)",
            face.family
        );
        return None;
    };
    if !info.family.eq_ignore_ascii_case(&face.family) {
        aliases().insert(face.family.to_lowercase(), info.family.clone());
    }
    Some(ReadyFont {
        face,
        bytes,
        family: info.family,
    })
}

/// Writes a run-time font to `dir` (named by its content, kept if already
/// there) for the platform APIs that take a file. Returns its path.
pub fn save(font: &ReadyFont, dir: &Path) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    font.bytes.hash(&mut h);
    let ext = match prism_core::fonts::font_format(&font.bytes) {
        Some("opentype") => "otf",
        Some("collection") => "ttc",
        _ => "ttf",
    };
    let path = dir.join(format!("{:016x}.{ext}", h.finish()));
    if !path.exists() {
        std::fs::create_dir_all(dir).ok()?;
        // Written whole, then renamed: another process never reads half a file.
        let tmp = path.with_extension("part");
        std::fs::write(&tmp, &font.bytes).ok()?;
        std::fs::rename(&tmp, &path).ok()?;
    }
    Some(path)
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

    #[test]
    fn embedded_fonts_are_the_font_files_under_fonts() {
        let embedded = EmbeddedAssets::new()
            .file("fonts/A.ttf", &b""[..])
            .file("fonts/extra/B.OTF", &b""[..])
            .file("fonts/A.woff2", &b""[..])
            .file("logo.ttf", &b""[..]);
        let names: Vec<&str> = embedded_fonts(&embedded).map(|(n, _)| n).collect();
        assert_eq!(names, ["fonts/A.ttf", "fonts/extra/B.OTF"]);
    }
}
