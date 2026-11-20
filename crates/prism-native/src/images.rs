//! Image bytes for `picture(src)` on native backends: remote URLs (with an
//! on-disk cache) or app assets (compiled into the binary, then bundle
//! resources, then the dev assets dir). Runs on a background thread;
//! platforms decode and display the bytes.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use prism_core::EmbeddedAssets;

/// Where to look for assets and cache downloads.
#[derive(Clone, Debug, Default)]
pub struct ImageSources {
    /// Assets compiled into the binary, searched first.
    pub embedded: EmbeddedAssets,
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

/// Loads the bytes of `src` (blocking). A remote source may carry headers
/// (`Picture::header`, see [`prism_core::picture_source`]): they go with the
/// request, and the image skips the on-disk cache (it is private).
pub fn load(src: &str, sources: &ImageSources) -> Option<Vec<u8>> {
    if src.starts_with("data:") {
        return data_url(src);
    }
    let (url, headers) = prism_core::picture_source(src);
    if is_remote(url) {
        let cache = sources.cache_dir.as_ref().filter(|_| headers.is_empty());
        if let Some(dir) = cache
            && let Ok(bytes) = std::fs::read(cache_path(dir, src))
        {
            return Some(bytes);
        }
        let mut request = prism_http::Request::get(url);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        // Never log the headers: they hold tokens.
        let res = match request.send_blocking() {
            Ok(res) if res.ok() => res,
            Ok(res) => {
                eprintln!("prism: image {url}: HTTP {}", res.status);
                return None;
            }
            Err(e) => {
                eprintln!("prism: image {url}: {e}");
                return None;
            }
        };
        if let Some(dir) = cache {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(cache_path(dir, src), &res.body);
        }
        return Some(res.body);
    }
    // A file on disk (a picked photo, a download…).
    if let Some(path) = file_url_path(src).or(src.starts_with('/').then_some(src)) {
        return std::fs::read(path).ok();
    }
    if src.split('/').any(|s| s == "..") {
        return None;
    }
    if let Some(bytes) = sources.embedded.get(src) {
        return Some(bytes.to_vec());
    }
    sources
        .asset_dirs
        .iter()
        .find_map(|dir| std::fs::read(dir.join(src)).ok())
}

/// A URL the platform's media player opens for `src` (#207): a remote URL
/// as it is (headers on the following lines), a file as a `file://` URL,
/// an app asset as the file URL of its copy in an asset folder, or, for
/// one compiled into the binary, of a copy in the cache.
pub fn media_url(src: &str, sources: &ImageSources) -> Option<String> {
    let (url, _) = prism_core::picture_source(src);
    if is_remote(url) {
        return Some(src.to_string());
    }
    if url.starts_with("file://") {
        return Some(url.to_string());
    }
    if url.starts_with('/') {
        return Some(format!("file://{url}"));
    }
    if url.split('/').any(|s| s == "..") {
        return None;
    }
    if let Some(path) = sources
        .asset_dirs
        .iter()
        .map(|d| d.join(url))
        .find(|p| p.exists())
    {
        return Some(file_url(&path));
    }
    let bytes = sources.embedded.get(url)?;
    let dir = sources.cache_dir.as_ref()?.join("media");
    let _ = std::fs::create_dir_all(&dir);
    let name = url.replace('/', "_");
    let path = dir.join(name);
    if !path.exists() {
        std::fs::write(&path, bytes).ok()?;
    }
    Some(file_url(&path))
}

/// `path` as a `file://` URL, with `/` separators: `file:///C:/…` for a
/// Windows path.
fn file_url(path: &Path) -> String {
    let path = path.display().to_string();
    if cfg!(windows) {
        format!("file:///{}", path.replace('\\', "/"))
    } else {
        format!("file://{path}")
    }
}

/// The path a `file://` URL names (`file:///C:/…` is `C:/…` on Windows).
fn file_url_path(url: &str) -> Option<&str> {
    let path = url.strip_prefix("file://")?;
    let drive = cfg!(windows) && path.starts_with('/') && path.as_bytes().get(2) == Some(&b':');
    Some(if drive { &path[1..] } else { path })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A server answering `n` requests: 200 when the bearer token is sent.
    fn server(n: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..n {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let len = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..len]).to_ascii_lowercase();
                let reply: &[u8] = if req.contains("authorization: bearer t0ken") {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\npng"
                } else {
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"
                };
                let _ = s.write_all(reply);
            }
        });
        format!("http://{addr}/me.png")
    }

    #[test]
    fn embedded_assets_come_before_the_asset_dirs() {
        let dir = std::env::temp_dir().join(format!("prism-images-dirs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("logo.png"), b"disk").unwrap();
        std::fs::write(dir.join("other.png"), b"other").unwrap();
        let sources = ImageSources {
            embedded: EmbeddedAssets::new().file("logo.png", &b"binary"[..]),
            asset_dirs: vec![dir.clone()],
            ..Default::default()
        };
        assert_eq!(load("logo.png", &sources).as_deref(), Some(&b"binary"[..]));
        assert_eq!(load("other.png", &sources).as_deref(), Some(&b"other"[..]));
        assert_eq!(load("../logo.png", &sources), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remote_pictures_send_their_headers_and_skip_the_disk_cache() {
        let url = server(2);
        let cache = std::env::temp_dir().join(format!("prism-images-{}", std::process::id()));
        let sources = ImageSources {
            cache_dir: Some(cache.clone()),
            ..Default::default()
        };
        let src = format!("{url}\nAuthorization: Bearer t0ken");
        assert_eq!(load(&src, &sources).as_deref(), Some(&b"png"[..]));
        assert!(!cache_path(&cache, &src).exists());
        assert!(!cache_path(&cache, &url).exists());
        // Without the header: refused.
        assert_eq!(load(&url, &sources), None);
        let _ = std::fs::remove_dir_all(cache);
    }

    #[test]
    fn media_urls_point_at_files_the_player_opens() {
        let dir = std::env::temp_dir().join(format!("prism-media-{}", std::process::id()));
        let assets = dir.join("assets");
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(assets.join("intro.mp4"), b"mp4").unwrap();
        let sources = ImageSources {
            embedded: EmbeddedAssets::new().file("clips/outro.mp4", &b"mp4"[..]),
            asset_dirs: vec![assets.clone()],
            cache_dir: Some(dir.join("cache")),
        };
        let remote = "https://example.com/a.mp4\nAuthorization: Bearer t";
        assert_eq!(media_url(remote, &sources).as_deref(), Some(remote));
        assert_eq!(
            media_url("/tmp/a.mp4", &sources).as_deref(),
            Some("file:///tmp/a.mp4")
        );
        let asset = file_url(&assets.join("intro.mp4"));
        assert_eq!(media_url("intro.mp4", &sources), Some(asset));
        let copy = media_url("clips/outro.mp4", &sources).unwrap();
        assert!(copy.ends_with("media/clips_outro.mp4"));
        assert!(Path::new(file_url_path(&copy).unwrap()).exists());
        assert_eq!(media_url("../secret.mp4", &sources), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
