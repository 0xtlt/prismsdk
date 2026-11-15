//! Static files with content hashes in their names, for CDNs.
//!
//! - Prism's client: `/_prism/client.<hash>.js` and `/_prism/prism.<hash>.css`.
//! - App assets (`picture("logo.png")`): `/assets/logo.<hash>.png`.
//!
//! Hashed URLs never change content, so they are served with
//! `Cache-Control: public, max-age=31536000, immutable`; a new build gets new
//! names. Unhashed `/assets/logo.png` still works (revalidated with an ETag).
//! [`WebApp::export_static`](crate::WebApp::export_static) writes everything
//! to a directory to upload to a CDN; `asset_url` then points pages at it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
        _ => "application/octet-stream",
    }
}

/// One app asset: where it is on disk and its hashed name.
#[derive(Clone, Debug)]
struct Asset {
    file: PathBuf,
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
    /// `logo.png` → its file and hashed name.
    app: HashMap<String, Asset>,
    /// `logo.<hash>.png` → `logo.png`.
    by_hash: HashMap<String, String>,
    dir: Option<PathBuf>,
}

impl Assets {
    /// Hashes the client, the base CSS and every file under `dir`.
    pub fn new(client_js: String, css: String, dir: Option<PathBuf>) -> Self {
        let mut assets = Assets {
            dir: dir.clone(),
            ..Default::default()
        };
        // Bundled fonts (`assets/fonts/`): `@font-face` rules in the base
        // stylesheet, pointing at their hashed files (`_prism/` → `../assets/`).
        let fonts = dir.as_ref().map_or_else(String::new, |dir| {
            let mut files = Vec::new();
            walk(dir, dir, &mut files);
            let fonts: Vec<(String, String, Vec<u8>)> = files
                .into_iter()
                .filter(|(logical, _)| logical.starts_with("fonts/"))
                .filter_map(|(logical, file)| {
                    let bytes = std::fs::read(&file).ok()?;
                    let url = format!("../assets/{}", hashed_name(&logical, &content_hash(&bytes)));
                    Some((logical, url, bytes))
                })
                .collect();
            crate::fonts::font_faces(&fonts)
        });
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
        if let Some(dir) = dir {
            let mut files = Vec::new();
            walk(&dir, &dir, &mut files);
            for (logical, file) in files {
                let Ok(bytes) = std::fs::read(&file) else {
                    continue;
                };
                let hash = content_hash(&bytes);
                let hashed = hashed_name(&logical, &hash);
                assets.by_hash.insert(hashed.clone(), logical.clone());
                assets.app.insert(logical, Asset { file, hashed, hash });
            }
        }
        assets
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
                body: std::fs::read(&asset.file).ok()?,
                content_type: content_type(name),
                etag: format!("\"{}\"", asset.hash),
                cache_control: IMMUTABLE,
            });
        }
        // Unhashed: read now (it may have changed), revalidate every time.
        if !safe(name) {
            return None;
        }
        let body = std::fs::read(self.dir.as_ref()?.join(name)).ok()?;
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
        for (logical, asset) in &self.app {
            write(
                &format!("assets/{}", asset.hashed),
                &std::fs::read(&asset.file)?,
            )?;
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

/// A relative path that stays inside its directory.
pub(crate) fn safe(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path
            .split('/')
            .any(|seg| seg == ".." || seg.is_empty() || seg.contains('\\'))
}

/// Every file under `dir` as (path relative to `root` with `/`, full path).
fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_carry_the_content_hash() {
        assert_eq!(hashed_name("logo.png", "abc"), "logo.abc.png");
        assert_eq!(hashed_name("icons/a.b.svg", "abc"), "icons/a.b.abc.svg");
        assert_eq!(hashed_name("LICENSE", "abc"), "LICENSE.abc");
        assert_eq!(content_hash(b"x").len(), 12);
        assert!(!safe("../secret") && !safe("/etc/passwd") && safe("img/a.png"));
    }

    #[test]
    fn hashed_files_are_immutable_and_exportable() {
        let dir = std::env::temp_dir().join(format!("prism-assets-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("img")).unwrap();
        std::fs::write(dir.join("img/logo.png"), b"png").unwrap();
        let assets = Assets::new("js".into(), "css".into(), Some(dir.clone()));
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
        let _ = std::fs::remove_dir_all(dir);
    }
}
