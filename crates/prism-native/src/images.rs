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

/// The bytes of a `data:` URL (`picture_data`): base64 or percent-encoded.
pub fn data_url(src: &str) -> Option<Vec<u8>> {
    let (meta, payload) = src.strip_prefix("data:")?.split_once(',')?;
    if meta.ends_with(";base64") {
        return base64(payload);
    }
    let b = payload.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (
            b[i],
            b.get(i + 1).copied().and_then(hex),
            b.get(i + 2).copied().and_then(hex),
        ) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, ..) => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some(out)
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        acc = acc << 6 | value(c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Loads the bytes of `src` (blocking).
pub fn load(src: &str, sources: &ImageSources) -> Option<Vec<u8>> {
    if src.starts_with("data:") {
        return data_url(src);
    }
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
    // A file on disk (a picked photo, a download…).
    if let Some(path) = src
        .strip_prefix("file://")
        .or(src.starts_with('/').then_some(src))
    {
        return std::fs::read(path).ok();
    }
    if src.split('/').any(|s| s == "..") {
        return None;
    }
    sources
        .asset_dirs
        .iter()
        .find_map(|dir| std::fs::read(dir.join(src)).ok())
}
