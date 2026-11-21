//! Static files with content hashes in their names, for CDNs.
//!
//! - Prism's client: `/_prism/client.<hash>.js` and `/_prism/prism.<hash>.css`.
//! - App assets (`picture("logo.png")`): `/assets/logo.<hash>.png`, from the
//!   assets directory or compiled into the binary ([`EmbeddedAssets`]).
//!
//! Hashed URLs never change content, so they are served with
//! `Cache-Control: public, max-age=31536000, immutable`; a new build gets new
//! names. Unhashed `/assets/logo.png` still works (revalidated with an ETag).
//! [`WebApp::export_static`](crate::WebApp::export_static) writes everything
//! to a directory to upload to a CDN; `asset_url` then points pages at it.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use prism_core::EmbeddedAssets;

use sha2::{Digest, Sha256};

/// `Cache-Control` for files whose name contains their hash.
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// `Cache-Control` for files that can change under the same name.
pub const REVALIDATE: &str = "public, max-age=0, must-revalidate";

/// A file ready to send.
#[derive(Clone, Debug)]
pub struct StaticFile {
    pub body: Vec<u8>,
    pub content_type: &'static str,
    /// Strong ETag (`"<hash>"`).
    pub etag: String,
    pub cache_control: &'static str,
}

/// First 12 hex digits of the SHA-256 of `bytes`.
pub fn content_hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `logo.png` + hash → `logo.<hash>.png` (`icons/a.svg` → `icons/a.<hash>.svg`).
fn hashed_name(path: &str, hash: &str) -> String {
    let (dir, file) = path.rsplit_once('/').map_or(("", path), |(d, f)| (d, f));
    let file = match file.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}.{hash}.{ext}"),
        _ => format!("{file}.{hash}"),
    };
    if dir.is_empty() {
        file
    } else {
        format!("{dir}/{file}")
    }
}

pub fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        // Streaming compilation needs it.
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// One app asset: where its bytes are and its hashed name.
#[derive(Clone, Debug)]
struct Asset {
    /// The file on disk, or `None` for an embedded one.
    file: Option<PathBuf>,
    hashed: String,
    hash: String,
}

/// The client, the base CSS and the app's assets, by hashed name.
#[derive(Clone, Debug, Default)]
pub(crate) struct Assets {
    /// `_prism/client.<hash>.js`, `_prism/prism.<hash>.css` → contents.
    builtin: HashMap<String, (Vec<u8>, String)>,
    client: String,
    css: String,
    /// `logo.png` → where it is and its hashed name.
    app: HashMap<String, Asset>,
    /// `logo.<hash>.png` → `logo.png`.
    by_hash: HashMap<String, String>,
    dir: Option<PathBuf>,
    /// Files compiled into the binary: found before the directory's.
    embedded: EmbeddedAssets,
    /// WASM islands: name → served module path (for `manifest.json`).
    islands: Vec<(String, String)>,
}

impl Assets {
    /// Hashes the client, the base CSS, the `embedded` files and every file
    /// under `dir` (an embedded file wins over the directory's of the same
    /// name).
    pub fn new(
        client_js: String,
        css: String,
        dir: Option<PathBuf>,
        embedded: EmbeddedAssets,
    ) -> Self {
        let mut assets = Assets {
            dir: dir.clone(),
            embedded,
            ..Default::default()
        };
        // Every app file: (logical name, file on disk or none, contents).
        let mut files: Vec<(String, Option<PathBuf>, Cow<'_, [u8]>)> = assets
            .embedded
            .iter()
            .map(|(name, bytes)| (name.to_string(), None, Cow::Borrowed(bytes)))
            .collect();
        if let Some(dir) = &dir {
            let mut found = Vec::new();
            walk(dir, &mut found);
            for (logical, file) in found {
                if assets.embedded.get(&logical).is_some() {
                    continue;
                }
                if let Ok(bytes) = std::fs::read(&file) {
                    files.push((logical, Some(file), Cow::Owned(bytes)));
                }
            }
        }
        // Bundled fonts (`assets/fonts/`): `@font-face` rules in the base
        // stylesheet, pointing at their hashed files (`_prism/` → `../assets/`).
        let fonts: Vec<(String, String, Vec<u8>)> = files
            .iter()
            .filter(|(logical, ..)| logical.starts_with("fonts/"))
            .map(|(logical, _, bytes)| {
                let url = format!("../assets/{}", hashed_name(logical, &content_hash(bytes)));
                (logical.clone(), url, bytes.to_vec())
            })
            .collect();
        let fonts = crate::fonts::font_faces(&fonts);
        let css = if fonts.is_empty() {
            css
        } else {
            format!("{css}\n/* App fonts (assets/fonts). */\n{fonts}")
        };
        for (name, body) in [("client.js", client_js), ("prism.css", css)] {
            let hash = content_hash(body.as_bytes());
            let served = format!("_prism/{}", hashed_name(name, &hash));
            if name.ends_with(".js") {
                assets.client = served.clone();
            } else {
                assets.css = served.clone();
            }
            assets.builtin.insert(served, (body.into_bytes(), hash));
        }
        for (logical, file, bytes) in files {
            let hash = content_hash(&bytes);
            let hashed = hashed_name(&logical, &hash);
            assets.by_hash.insert(hashed.clone(), logical.clone());
            assets.app.insert(logical, Asset { file, hashed, hash });
        }
        assets
    }

    /// The directory and the embedded files the assets were read from.
    pub fn sources(&self) -> (Option<PathBuf>, EmbeddedAssets) {
        (self.dir.clone(), self.embedded.clone())
    }

    /// Served path (no leading slash) of the client script.
    pub fn client(&self) -> &str {
        &self.client
    }

    /// Served path of the base stylesheet.
    pub fn css(&self) -> &str {
        &self.css
    }

    /// Served path of app asset `name` (`assets/logo.<hash>.png`), or the
    /// unhashed path for files added after startup.
    pub fn app_path(&self, name: &str) -> String {
        match self.app.get(name) {
            Some(a) => format!("assets/{}", a.hashed),
            None => format!("assets/{name}"),
        }
    }

    /// The contents of app asset `logical`.
    fn read(&self, logical: &str, asset: &Asset) -> Option<Vec<u8>> {
        match &asset.file {
            Some(file) => std::fs::read(file).ok(),
            None => self.embedded.get(logical).map(<[u8]>::to_vec),
        }
    }

    /// The file for served path `path` (`_prism/…` or `assets/…`).
    pub fn get(&self, path: &str) -> Option<StaticFile> {
        if let Some((body, hash)) = self.builtin.get(path) {
            return Some(StaticFile {
                body: body.clone(),
                content_type: content_type(path),
                etag: format!("\"{hash}\""),
                cache_control: IMMUTABLE,
            });
        }
        let name = path.strip_prefix("assets/")?;
        if let Some(logical) = self.by_hash.get(name) {
            let asset = &self.app[logical];
            return Some(StaticFile {
                body: self.read(logical, asset)?,
                content_type: content_type(name),
                etag: format!("\"{}\"", asset.hash),
                cache_control: IMMUTABLE,
            });
        }
        // Unhashed: read now (a file on disk may have changed), revalidate
        // every time.
        if !safe(name) {
            return None;
        }
        let body = match self.embedded.get(name) {
            Some(bytes) => bytes.to_vec(),
            None => std::fs::read(file_under(self.dir.as_ref()?, name)?).ok()?,
        };
        Some(StaticFile {
            etag: format!("\"{}\"", content_hash(&body)),
            body,
            content_type: content_type(name),
            cache_control: REVALIDATE,
        })
    }

    /// Writes every hashed file under `out` (`_prism/…`, `assets/…`) plus
    /// `manifest.json` (logical name → served path). Returns the file count.
    pub fn export(&self, out: &Path) -> std::io::Result<usize> {
        let mut count = 0;
        let mut write = |path: &str, body: &[u8]| -> std::io::Result<()> {
            let target = out.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(target, body)?;
            count += 1;
            Ok(())
        };
        for (path, (body, _)) in &self.builtin {
            write(path, body)?;
        }
        let mut manifest = serde_json::Map::new();
        manifest.insert("client.js".into(), self.client.clone().into());
        manifest.insert("prism.css".into(), self.css.clone().into());
        for (name, module) in &self.islands {
            manifest.insert(format!("islands/{name}"), module.clone().into());
        }
        for (logical, asset) in &self.app {
            let body = self.read(logical, asset).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, logical.clone())
            })?;
            write(&format!("assets/{}", asset.hashed), &body)?;
            manifest.insert(
                format!("assets/{logical}"),
                format!("assets/{}", asset.hashed).into(),
            );
        }
        let json = serde_json::to_string_pretty(&serde_json::Value::Object(manifest))?;
        write("manifest.json", json.as_bytes())?;
        Ok(count)
    }
}

impl Assets {
    /// Serves the files of the app's WASM islands (already content-hashed
    /// by `prism`) as `_prism/islands/…`, immutable.
    pub(crate) fn add_islands(&mut self, islands: &crate::islands::Islands) {
        for (served, body) in &islands.files {
            let hash = content_hash(body);
            self.builtin.insert(served.clone(), (body.clone(), hash));
        }
        self.islands = islands
            .modules
            .iter()
            .map(|(n, m)| (n.clone(), m.clone()))
            .collect();
        self.islands.sort();
    }
}

/// `client.js` as served: without its comment lines, blank lines and
/// indentation (a quarter of its size, gzipped too). Lines of code are kept
/// whole, so strings and trailing comments are never cut, and so are the
/// lines of a multi-line template literal (the WebGL shaders).
pub(crate) fn compact_js(src: &str) -> String {
    compact(src, true)
}

/// `prism.css` as served: without its comments, blank lines and indentation.
pub(crate) fn compact_css(src: &str) -> String {
    compact(src, false)
}

fn compact(src: &str, js: bool) -> String {
    let mut out = String::with_capacity(src.len());
    // Inside a `/* … */` comment, or (JS) a multi-line template literal.
    let (mut comment, mut template) = (false, false);
    for line in src.lines() {
        if template {
            out.push_str(line);
            out.push('\n');
            template = !opens_or_closes(line);
            continue;
        }
        let mut code = line.trim();
        if comment {
            let Some((_, rest)) = code.split_once("*/") else {
                continue;
            };
            comment = false;
            code = rest.trim_start();
        }
        if code.is_empty() || (js && code.starts_with("//")) {
            continue;
        }
        if let Some(rest) = code.strip_prefix("/*") {
            match rest.split_once("*/") {
                None => {
                    comment = true;
                    continue;
                }
                Some((_, after)) if after.trim().is_empty() => continue,
                // Code after the comment: the line stays.
                Some(_) => {}
            }
        }
        out.push_str(code);
        out.push('\n');
        template = js && opens_or_closes(code);
    }
    out
}

/// Whether a line of JavaScript has an odd number of unescaped backticks:
/// it opens a template literal it does not close, or closes one.
fn opens_or_closes(line: &str) -> bool {
    let mut escaped = false;
    let ticks = line
        .chars()
        .filter(|&c| {
            let tick = c == '`' && !escaped;
            escaped = c == '\\' && !escaped;
            tick
        })
        .count();
    ticks % 2 == 1
}

/// A relative path that names a file inside its directory on every OS:
/// `/`-separated plain segments. Checked as text, the same way on every
/// host: no dot segments or dotfiles, and nothing Windows reads as a drive
/// (`C:`), a UNC or verbatim prefix, a stream (`a:b`), a device (`NUL.txt`)
/// or a separator (`\`).
pub(crate) fn safe(path: &str) -> bool {
    !path.is_empty() && path.split('/').all(plain_segment)
}

/// One segment of a [`safe`] path.
fn plain_segment(seg: &str) -> bool {
    !seg.is_empty()
        && !seg.starts_with('.')
        // Windows drops trailing dots and spaces: `a.png.` is `a.png`.
        && !seg.ends_with(['.', ' '])
        && !seg.chars().any(|c| {
            c.is_control() || matches!(c, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
        && !windows_device(seg)
}

/// `CON`, `nul.txt`, `Com1.png`…: Windows opens a device for these names,
/// in any directory, whatever their extension.
fn windows_device(seg: &str) -> bool {
    let stem = seg.split('.').next().unwrap_or(seg).trim_end_matches(' ');
    let stem = stem.to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) {
        return true;
    }
    let port = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    port.is_some_and(|n| {
        let mut chars = n.chars();
        matches!(
            (chars.next(), chars.next()),
            (Some('0'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None)
        )
    })
}

/// The file `name` ([`safe`]) names under `root`, links resolved, if it is
/// still under `root` once they are: a link out of the directory is never
/// followed.
pub(crate) fn file_under(root: &Path, name: &str) -> Option<PathBuf> {
    if !safe(name) {
        return None;
    }
    let mut path = root.to_path_buf();
    path.extend(name.split('/'));
    let real = path.canonicalize().ok()?;
    (real.starts_with(root.canonicalize().ok()?) && real.is_file()).then_some(real)
}

/// Every file under `root` as (its [`safe`] path relative to `root`, the
/// file). Links are followed only inside `root`, each directory once (a
/// cycle ends); dotfiles and names that are not [`safe`] are skipped.
fn walk(root: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(real_root) = root.canonicalize() else {
        return;
    };
    let mut seen = std::collections::HashSet::new();
    walk_dir(&real_root, root, "", &mut seen, out);
}

fn walk_dir(
    real_root: &Path,
    dir: &Path,
    prefix: &str,
    seen: &mut std::collections::HashSet<PathBuf>,
    out: &mut Vec<(String, PathBuf)>,
) {
    let Ok(real) = dir.canonicalize() else {
        return;
    };
    if !seen.insert(real) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|n| !n.starts_with('.')) else {
            continue;
        };
        let logical = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        if !plain_segment(name) {
            eprintln!("prism-web: assets/{logical} is not served: not a portable file name");
            continue;
        }
        let Ok(target) = path.canonicalize() else {
            continue;
        };
        if !target.starts_with(real_root) {
            eprintln!(
                "prism-web: assets/{logical} is not served: it links out of the assets directory"
            );
            continue;
        }
        if target.is_dir() {
            walk_dir(real_root, &path, &logical, seen, out);
        } else if target.is_file() {
            out.push((logical, target));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_dir::TempDir;

    #[test]
    fn names_carry_the_content_hash() {
        assert_eq!(hashed_name("logo.png", "abc"), "logo.abc.png");
        assert_eq!(hashed_name("icons/a.b.svg", "abc"), "icons/a.b.abc.svg");
        assert_eq!(hashed_name("LICENSE", "abc"), "LICENSE.abc");
        assert_eq!(content_hash(b"x").len(), 12);
        assert!(!safe("../secret") && !safe("/etc/passwd") && safe("img/a.png"));
    }

    #[test]
    fn comment_lines_go_and_code_stays_whole() {
        let js = "// a comment\n  const a = 1; // kept\n\n  /** doc\n   * more\n   */\n  /* x */ b();\n  const s = `one\n  // in the template\n  two`;\n  c(`\\``);\n";
        assert_eq!(
            compact_js(js),
            "const a = 1; // kept\n/* x */ b();\nconst s = `one\n  // in the template\n  two`;\nc(`\\``);\n"
        );
        let css = "/* rule\n   on two lines */\n.a {\n  color: red; /* kept */\n}\n";
        assert_eq!(compact_css(css), ".a {\ncolor: red; /* kept */\n}\n");
    }

    #[test]
    fn the_served_client_is_compact_and_keeps_its_shaders() {
        let client = crate::app::client_js();
        let served = compact_js(&client);
        assert!(
            served.len() * 4 < client.len() * 3,
            "{} of {} bytes",
            served.len(),
            client.len()
        );
        // prism-core's shaders, byte for byte (JSON strings in the client).
        use prism_core::scene3d::shaders;
        for (name, glsl) in [
            ("VERTEX", shaders::VERTEX),
            ("FRAGMENT", shaders::FRAGMENT),
            ("SKY_VERTEX", shaders::SKY_VERTEX),
            ("SKY_FRAGMENT", shaders::SKY_FRAGMENT),
        ] {
            let open = format!("const {name} = ");
            let start = served.find(&open).unwrap() + open.len();
            let mut strings =
                serde_json::Deserializer::from_str(&served[start..]).into_iter::<String>();
            assert_eq!(strings.next().unwrap().unwrap(), glsl, "{name}");
        }
        // Still a script (checked when `node` is installed).
        let dir = TempDir::new("client");
        std::fs::create_dir_all(&*dir).unwrap();
        let file = dir.join("client.js");
        std::fs::write(&file, &served).unwrap();
        if let Ok(out) = std::process::Command::new("node")
            .arg("--check")
            .arg(&file)
            .output()
        {
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        // The stylesheet too.
        assert!(compact_css(crate::PRISM_CSS).len() < crate::PRISM_CSS.len() * 9 / 10);
    }

    #[test]
    fn hashed_files_are_immutable_and_exportable() {
        let dir = TempDir::new("assets");
        std::fs::create_dir_all(dir.join("img")).unwrap();
        std::fs::write(dir.join("img/logo.png"), b"png").unwrap();
        let assets = Assets::new(
            "js".into(),
            "css".into(),
            Some(dir.to_path_buf()),
            EmbeddedAssets::new(),
        );
        let path = assets.app_path("img/logo.png");
        assert_eq!(
            path,
            format!("assets/img/logo.{}.png", content_hash(b"png"))
        );
        let f = assets.get(&path).unwrap();
        assert_eq!(
            (f.body.as_slice(), f.cache_control),
            (&b"png"[..], IMMUTABLE)
        );
        let plain = assets.get("assets/img/logo.png").unwrap();
        assert_eq!(plain.cache_control, REVALIDATE);
        assert!(assets.get(assets.client()).is_some());
        assert!(assets.client().starts_with("_prism/client.") && assets.css().ends_with(".css"));

        let out = dir.join("public");
        assert_eq!(assets.export(&out).unwrap(), 4);
        assert!(out.join(&path).exists() && out.join(assets.client()).exists());
        let manifest = std::fs::read_to_string(out.join("manifest.json")).unwrap();
        assert!(manifest.contains(&path));
    }

    #[test]
    fn embedded_files_win_over_the_directory() {
        let dir = TempDir::new("assets-embed");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("logo.png"), b"disk").unwrap();
        std::fs::write(dir.join("other.png"), b"other").unwrap();
        let embedded = EmbeddedAssets::new().file("logo.png", &b"binary"[..]);
        let assets = Assets::new("js".into(), "css".into(), Some(dir.to_path_buf()), embedded);
        let logo = assets.app_path("logo.png");
        assert_eq!(logo, format!("assets/logo.{}.png", content_hash(b"binary")));
        assert_eq!(assets.get(&logo).unwrap().body, b"binary");
        assert_eq!(assets.get("assets/logo.png").unwrap().body, b"binary");
        assert_eq!(
            assets.get(&assets.app_path("other.png")).unwrap().body,
            b"other"
        );

        let out = dir.join("public");
        assert_eq!(assets.export(&out).unwrap(), 5);
        assert_eq!(std::fs::read(out.join(&logo)).unwrap(), b"binary");
    }

    /// S4 (security review): a requested name is checked as text, the same
    /// way on every host, before it meets a path. Windows reads a drive
    /// (`C:`), a UNC or verbatim prefix, a stream (`a:b`) or a device
    /// (`NUL.txt`) in names Unix calls ordinary.
    #[test]
    fn s4_only_plain_relative_names_are_safe() {
        for name in [
            // Drive-qualified and drive-relative.
            "C:/Windows/win.ini",
            "c:/x",
            "C:win.ini",
            "C:",
            "img/C:/x.png",
            // UNC and verbatim (`\\?\`), with either separator.
            "\\\\server\\share\\x.png",
            "//server/share/x.png",
            "\\\\?\\C:\\Windows\\win.ini",
            "//?/C:/Windows/win.ini",
            "\\\\.\\pipe\\x",
            // Alternate data streams.
            "logo.png:secret",
            "logo.png::$DATA",
            // Devices, with or without an extension, in any case.
            "CON",
            "nul.txt",
            "img/Aux.png",
            "COM1",
            "lpt9.js",
            "COM\u{b9}.png",
            "CONIN$",
            "prn .css",
            // Dotfiles, dot segments, empty segments, other separators.
            ".env",
            ".git/config",
            "img/.hidden.png",
            ".",
            "./a.png",
            "a/../b.png",
            "../secret",
            "a//b.png",
            "a\\b.png",
            "/etc/passwd",
            // Trailing dots and spaces (Windows drops them), reserved and
            // control characters.
            "a.png.",
            "a.png ",
            "a\0b.png",
            "a\nb.png",
            "a<b.png",
            "a|b.png",
            "a?b.png",
            "a*b.png",
            "a\"b.png",
            "",
        ] {
            assert!(!safe(name), "{name:?} must be refused");
        }
        for name in [
            "logo.png",
            "img/a.png",
            "fonts/Inter-Regular.woff2",
            "icons/a.b.svg",
            "console.js",
            "com10.png",
            "nul-free.png",
            "LICENSE",
            "caf\u{e9}.png",
        ] {
            assert!(safe(name), "{name:?} must be accepted");
        }
    }

    /// S4: the unhashed fallback serves what the index would publish, and
    /// nothing a link takes out of the directory.
    #[test]
    fn s4_the_fallback_serves_only_published_files() {
        let root = TempDir::new("assets-s4");
        let (dir, outside) = (root.join("assets"), root.join("outside"));
        std::fs::create_dir_all(dir.join("img")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(dir.join("img/logo.png"), b"logo").unwrap();
        std::fs::write(dir.join(".env"), b"SECRET=1").unwrap();
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&outside, dir.join("out")).unwrap();
            symlink(outside.join("secret.txt"), dir.join("secret.txt")).unwrap();
            symlink(dir.join("img/logo.png"), dir.join("alias.png")).unwrap();
            // A cycle.
            symlink(&dir, dir.join("img/loop")).unwrap();
        }
        let assets = Assets::new(
            "js".into(),
            "css".into(),
            Some(dir.to_path_buf()),
            EmbeddedAssets::new(),
        );
        assert_eq!(assets.get("assets/img/logo.png").unwrap().body, b"logo");
        assert!(assets.get("assets/.env").is_none());
        #[cfg(unix)]
        {
            // Links out of the directory: neither published nor served.
            for name in ["out/secret.txt", "secret.txt"] {
                assert!(!assets.app.contains_key(name), "{name} published");
                assert!(
                    assets.get(&format!("assets/{name}")).is_none(),
                    "{name} served"
                );
            }
            // A link inside it still works; the cycle is walked once.
            assert!(assets.app_path("alias.png").starts_with("assets/alias."));
            assert_eq!(assets.get("assets/alias.png").unwrap().body, b"logo");
            assert!(
                assets.app.keys().all(|k| !k.contains("loop/")),
                "{:?}",
                assets.app.keys()
            );
        }
    }
}
