//! Image bytes for `picture(src)` on native backends: remote URLs (with an
//! on-disk cache) or app assets (bundle resources, then the dev assets dir).
//! Runs on a background thread; platforms decode and display the bytes.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Where to look for assets and cache downloads.
#[derive(Clone, Debug, Default)]
pub struct ImageSources {
    /// Directories searched for assets, in order.
    pub asset_dirs: Vec<PathBuf>,
    /// Download cache directory.
    pub cache_dir: Option<PathBuf>,
}

pub fn is_remote(src: &str) -> bool {
    src.starts_with("http://") || src.starts_with("https://")
}

fn cache_path(dir: &Path, url: &str) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    dir.join(format!("{:016x}", h.finish()))
}

/// Loads the bytes of `src` (blocking).
pub fn load(src: &str, sources: &ImageSources) -> Option<Vec<u8>> {
    if is_remote(src) {
        if let Some(dir) = &sources.cache_dir
            && let Ok(bytes) = std::fs::read(cache_path(dir, src))
        {
            return Some(bytes);
        }
        let res = match prism_http::Request::get(src).send_blocking() {
            Ok(res) if res.ok() => res,
            Ok(res) => {
                eprintln!("prism: image {src}: HTTP {}", res.status);
                return None;
            }
            Err(e) => {
                eprintln!("prism: image {src}: {e}");
                return None;
            }
        };
        if let Some(dir) = &sources.cache_dir {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(cache_path(dir, src), &res.body);
        }
        return Some(res.body);
    }
    if src.split('/').any(|s| s == "..") {
        return None;
    }
    sources
        .asset_dirs
        .iter()
        .find_map(|dir| std::fs::read(dir.join(src)).ok())
}
