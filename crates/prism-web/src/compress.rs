//! Compression of what the server sends: socket messages and HTTP bodies.
//!
//! - **Socket.** Axum's WebSocket stack (tungstenite) has no
//!   `permessage-deflate`, so large messages are compressed here instead: a
//!   page whose browser has `DecompressionStream` asks for it in the socket
//!   URL (`compress=deflate-raw`), then every message of at least
//!   [`Frame::MIN_COMPRESSED`] bytes goes out as a binary frame of raw
//!   DEFLATE (RFC 1951) and the client inflates it, keeping the order of
//!   messages. Small patches stay text frames.
//! - **HTTP.** [`WebApp::handle`](crate::WebApp::handle) answers with gzip
//!   when `Accept-Encoding` allows it: pages on the fly, static files
//!   (client, CSS, islands, app assets) compressed once and cached by
//!   content hash. Compressed answers carry their own ETag (`"<hash>-gzip"`)
//!   and `Vary: Accept-Encoding`.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

use flate2::Compression;
use http::{HeaderMap, HeaderValue, Response, Uri, header};

/// Socket messages. On a 5,000-row list's 3.1 MB first tree (release
/// build): level 1 → 406 KB in 3 ms, 3 → 241 KB in 6 ms, 6 → 217 KB in 16 ms.
const SOCKET_LEVEL: u32 = 3;
/// Pages, rendered per request (that list's 991 KB → 59 KB in about 4 ms).
const PAGE_LEVEL: u32 = 6;
/// Static files, compressed once.
const STATIC_LEVEL: u32 = 9;
/// HTTP bodies below this are sent as they are.
const MIN_HTTP: usize = 1024;

/// One message for the page, as the socket should send it: what
/// [`WebApp::connect`](crate::WebApp::connect)'s `send` gets, framed.
///
/// The Axum [`router`](crate::WebApp::router) does this already. Another
/// WebSocket server does the same to get compressed messages:
///
/// ```ignore
/// let compress = prism_web::Frame::negotiated(&uri);
/// let session = app.connect(&uri, &headers, move |text| {
///     match prism_web::Frame::encode(text, compress) {
///         prism_web::Frame::Text(t) => out.send_text(t),
///         prism_web::Frame::Binary(b) => out.send_binary(b),
///     }
/// });
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// A text frame: the JSON message as it is.
    Text(String),
    /// A binary frame: the JSON message as raw DEFLATE (`deflate-raw`).
    Binary(Vec<u8>),
}

impl Frame {
    /// Its size in bytes.
    pub(crate) fn len(&self) -> usize {
        match self {
            Frame::Text(t) => t.len(),
            Frame::Binary(b) => b.len(),
        }
    }

    /// Messages shorter than this (in bytes) stay text frames: most patches
    /// are a few hundred bytes, not worth a round through the decompressor.
    pub const MIN_COMPRESSED: usize = 1024;

    /// Whether the page that opened the socket at `uri` reads compressed
    /// frames (`compress=deflate-raw` in the query, sent by browsers that
    /// have `DecompressionStream`).
    pub fn negotiated(uri: &Uri) -> bool {
        uri.query().unwrap_or("").split('&').any(|kv| {
            kv.strip_prefix("compress=")
                .is_some_and(|v| v.split(',').any(|f| f == "deflate-raw"))
        })
    }

    /// `text` as the frame to send: compressed when the page `negotiated`
    /// it, the message has at least [`MIN_COMPRESSED`](Frame::MIN_COMPRESSED)
    /// bytes and compression makes it smaller; text otherwise.
    pub fn encode(text: String, compress: bool) -> Frame {
        if !compress || text.len() < Self::MIN_COMPRESSED {
            return Frame::Text(text);
        }
        let z = deflate_raw(text.as_bytes(), SOCKET_LEVEL);
        if z.len() < text.len() {
            Frame::Binary(z)
        } else {
            Frame::Text(text)
        }
    }
}

/// `bytes` as raw DEFLATE (no zlib or gzip header), as
/// `DecompressionStream("deflate-raw")` reads it.
pub(crate) fn deflate_raw(bytes: &[u8], level: u32) -> Vec<u8> {
    let mut z = flate2::write::DeflateEncoder::new(
        Vec::with_capacity(bytes.len() / 4),
        Compression::new(level),
    );
    // Writing to a Vec cannot fail.
    let _ = z.write_all(bytes);
    z.finish().unwrap_or_default()
}

/// `bytes` as a gzip stream.
pub(crate) fn gzip(bytes: &[u8], level: u32) -> Vec<u8> {
    let mut z =
        flate2::write::GzEncoder::new(Vec::with_capacity(bytes.len() / 4), Compression::new(level));
    let _ = z.write_all(bytes);
    z.finish().unwrap_or_default()
}

// --- HTTP ---------------------------------------------------------------------------------

/// Whether `Accept-Encoding` allows gzip (`gzip`, `x-gzip` or `*` with a
/// non-zero quality).
pub(crate) fn accepts_gzip(headers: &HeaderMap) -> bool {
    let mut gzip = None;
    let mut any = None;
    for value in headers.get_all(header::ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else { continue };
        for item in value.split(',') {
            let mut parts = item.split(';');
            let coding = parts.next().unwrap_or("").trim().to_ascii_lowercase();
            let q = parts
                .find_map(|p| {
                    let p = p.trim();
                    p.strip_prefix("q=").or_else(|| p.strip_prefix("Q="))
                })
                .map_or(1.0, |q| q.trim().parse::<f32>().unwrap_or(0.0));
            match coding.as_str() {
                "gzip" | "x-gzip" => gzip = Some(q),
                "*" => any = Some(q),
                _ => {}
            }
        }
    }
    gzip.or(any).is_some_and(|q| q > 0.0)
}

/// Whether a body of this type and size is worth gzipping: text, JSON,
/// JavaScript, SVG, WASM and uncompressed fonts; not images, video,
/// `woff2` or PDF, which are compressed already.
pub(crate) fn worth_it(content_type: &str, len: usize) -> bool {
    if len < MIN_HTTP {
        return false;
    }
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mime.starts_with("text/")
        || matches!(
            mime.as_str(),
            "application/json"
                | "application/manifest+json"
                | "application/javascript"
                | "application/wasm"
                | "application/xml"
                | "image/svg+xml"
                | "image/x-icon"
                | "font/ttf"
                | "font/otf"
        )
}

/// The ETag of the gzipped form of the file whose ETag is `etag`
/// (`"abc"` → `"abc-gzip"`).
pub(crate) fn gzip_etag(etag: &str) -> String {
    match etag.strip_suffix('"') {
        Some(open) => format!("{open}-gzip\""),
        None => format!("{etag}-gzip"),
    }
}

/// Whether `If-None-Match` names `etag` or its gzipped form (weak
/// comparison, as RFC 9110 asks: `W/"abc"` matches `"abc"`).
pub(crate) fn etag_matches(headers: &HeaderMap, etag: &str) -> bool {
    let zipped = gzip_etag(etag);
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim())
        .any(|t| {
            let t = t.strip_prefix("W/").unwrap_or(t);
            t == "*" || t == etag || t == zipped
        })
}

/// Gzipped static files, by ETag (their content hash): each is compressed
/// once per process.
fn cache() -> &'static Mutex<HashMap<String, Arc<Vec<u8>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Vec<u8>>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The gzipped `body` of the static file whose ETag is `etag`: from the
/// cache when `immutable` (its name carries its hash), compressed now
/// otherwise.
pub(crate) fn gzip_static(etag: &str, body: &[u8], immutable: bool) -> Vec<u8> {
    if !immutable {
        return gzip(body, STATIC_LEVEL);
    }
    if let Some(z) = cache().lock().ok().and_then(|c| c.get(etag).cloned()) {
        return z.to_vec();
    }
    let z = Arc::new(gzip(body, STATIC_LEVEL));
    if let Ok(mut c) = cache().lock() {
        c.insert(etag.to_string(), z.clone());
    }
    z.to_vec()
}

/// Adds `Vary: Accept-Encoding` (once).
pub(crate) fn vary(r: &mut Response<Vec<u8>>) {
    let has = r
        .headers()
        .get_all(header::VARY)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| {
            v.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("accept-encoding"))
        });
    if !has {
        r.headers_mut()
            .append(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
}

/// Gzips a response made for this request (a page, the service worker…)
/// when the browser accepts it and it is worth it. Static files (with an
/// ETag) are left to [`gzip_static`]; an answer already encoded is kept.
pub(crate) fn encode_response(headers: &HeaderMap, r: &mut Response<Vec<u8>>) {
    if r.headers().contains_key(header::CONTENT_ENCODING) || r.headers().contains_key(header::ETAG)
    {
        return;
    }
    let content_type = r
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !worth_it(content_type, r.body().len()) {
        return;
    }
    vary(r);
    if !accepts_gzip(headers) {
        return;
    }
    let z = gzip(r.body(), PAGE_LEVEL);
    *r.body_mut() = z;
    r.headers_mut()
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn inflate_raw(z: &[u8]) -> String {
        let mut s = String::new();
        flate2::read::DeflateDecoder::new(z)
            .read_to_string(&mut s)
            .unwrap();
        s
    }

    fn gunzip(z: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(z)
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    /// A JSON message like a "reset" with `n` patches.
    fn message(n: usize) -> String {
        let patches: Vec<String> = (0..n)
            .map(|i| format!(r#"{{"op":"create","id":{i},"kind":"text","text":"Row {i}"}}"#))
            .collect();
        format!(r#"{{"t":"reset","p":[{}]}}"#, patches.join(","))
    }

    #[test]
    fn small_messages_stay_text_and_large_ones_are_deflated() {
        let small = message(3);
        assert!(small.len() < Frame::MIN_COMPRESSED);
        assert_eq!(Frame::encode(small.clone(), true), Frame::Text(small));

        let large = message(200);
        assert!(large.len() > Frame::MIN_COMPRESSED);
        let Frame::Binary(z) = Frame::encode(large.clone(), true) else {
            panic!("a large message is compressed");
        };
        assert!(z.len() * 5 < large.len(), "{} → {}", large.len(), z.len());
        assert_eq!(inflate_raw(&z), large);
        // Not negotiated: text, whatever the size.
        assert_eq!(Frame::encode(large.clone(), false), Frame::Text(large));
    }

    #[test]
    fn the_threshold_is_inclusive() {
        // `MIN_COMPRESSED` bytes of a repeated pattern: compressed; one less: text.
        let at = "ab".repeat(Frame::MIN_COMPRESSED / 2);
        assert!(matches!(Frame::encode(at.clone(), true), Frame::Binary(_)));
        assert!(matches!(
            Frame::encode(at[1..].to_string(), true),
            Frame::Text(_)
        ));
    }

    #[test]
    fn the_page_asks_for_compressed_frames_in_the_socket_url() {
        let uri = |q: &str| format!("/ws?{q}").parse::<Uri>().unwrap();
        assert!(Frame::negotiated(&uri(
            "path=%2F&tz=0&w=1&h=1&compress=deflate-raw"
        )));
        assert!(Frame::negotiated(&uri(
            "compress=gzip,deflate-raw&path=%2F"
        )));
        assert!(!Frame::negotiated(&uri("path=%2F&tz=0")));
        assert!(!Frame::negotiated(&uri("compress=gzip")));
        assert!(!Frame::negotiated(&"/ws".parse::<Uri>().unwrap()));
    }

    #[test]
    fn accept_encoding_with_qualities() {
        let ok = |v: &str| accepts_gzip(&headers(&[("accept-encoding", v)]));
        assert!(ok("gzip, deflate, br, zstd"));
        assert!(ok("br;q=1.0, gzip;q=0.8"));
        assert!(ok("*"));
        assert!(ok("x-gzip"));
        assert!(!ok("gzip;q=0"));
        assert!(!ok("br, *;q=0"));
        assert!(!ok("identity"));
        assert!(!ok("br"));
        assert!(ok("*;q=0.5, br"));
        assert!(!accepts_gzip(&HeaderMap::new()));
    }

    #[test]
    fn only_compressible_types_of_some_size() {
        assert!(worth_it("text/html; charset=utf-8", 5000));
        assert!(worth_it("text/javascript; charset=utf-8", 5000));
        assert!(worth_it("application/wasm", 5000));
        assert!(worth_it("image/svg+xml", 5000));
        assert!(!worth_it("text/html; charset=utf-8", 100));
        for t in [
            "image/png",
            "font/woff2",
            "video/mp4",
            "application/octet-stream",
        ] {
            assert!(!worth_it(t, 1 << 20), "{t}");
        }
    }

    #[test]
    fn etags_of_the_gzipped_form() {
        assert_eq!(gzip_etag("\"abc\""), "\"abc-gzip\"");
        let inm = |v: &str| headers(&[("if-none-match", v)]);
        assert!(etag_matches(&inm("\"abc\""), "\"abc\""));
        assert!(etag_matches(&inm("\"abc-gzip\""), "\"abc\""));
        assert!(etag_matches(&inm("W/\"abc-gzip\""), "\"abc\""));
        assert!(etag_matches(&inm("\"x\", \"abc\""), "\"abc\""));
        assert!(etag_matches(&inm("*"), "\"abc\""));
        assert!(!etag_matches(&inm("\"abd\""), "\"abc\""));
        assert!(!etag_matches(&HeaderMap::new(), "\"abc\""));
    }

    #[test]
    fn static_files_are_gzipped_once() {
        let body = "body { color: red }\n".repeat(200);
        let etag = format!("\"{}\"", crate::content_hash(body.as_bytes()));
        let a = gzip_static(&etag, body.as_bytes(), true);
        assert_eq!(gunzip(&a), body.as_bytes());
        assert!(cache().lock().unwrap().contains_key(&etag));
        assert_eq!(gzip_static(&etag, body.as_bytes(), true), a);
    }

    #[test]
    fn dynamic_answers_are_gzipped_when_accepted() {
        let page = || {
            let mut r = Response::new("<p>hello</p>".repeat(200).into_bytes());
            r.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            r
        };
        let mut r = page();
        encode_response(&headers(&[("accept-encoding", "gzip, br")]), &mut r);
        assert_eq!(r.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(r.headers()[header::VARY], "accept-encoding");
        assert_eq!(gunzip(r.body()), page().body().as_slice());

        // Not accepted: as it is, but it still varies.
        let mut r = page();
        encode_response(&HeaderMap::new(), &mut r);
        assert!(!r.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(r.headers()[header::VARY], "accept-encoding");
        assert_eq!(r.body(), page().body());

        // Small, or an image: untouched.
        let mut small = Response::new(b"<p>hi</p>".to_vec());
        small.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        encode_response(&headers(&[("accept-encoding", "gzip")]), &mut small);
        assert!(small.headers().get(header::VARY).is_none());
    }
}
