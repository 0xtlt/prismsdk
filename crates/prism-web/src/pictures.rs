//! Pictures with headers (`picture(url).header(name, value)`, #32) on the
//! web: the app runs on the server, so the server fetches them. The page gets
//! `<base>/_prism/picture/<id>`, a same-origin URL; the headers (a bearer
//! token) stay on the server and never reach the browser.
//!
//! - `<id>` is a keyed hash (a random key per process) of the session and the
//!   source: unguessable, and it reveals nothing about the token.
//! - It is bound to the session that rendered it: another browser (another
//!   `prism_sid` cookie) gets a 404, even with the URL.
//! - Only `image/*` answers are passed on, with `nosniff` and a sandboxing
//!   CSP (an SVG opened on its own runs no script on the app's origin), and
//!   `Cache-Control: private` (never on a shared cache or CDN).
//! - The server connects only to public addresses, unless the app trusts
//!   the origin (`WebApp::trust_media_origin`), and follows redirects only
//!   within the origin: a URL cannot reach the server's private network.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use http::{HeaderValue, Response, StatusCode, header};

/// At most this many sources are remembered (the oldest are forgotten:
/// their pages show them again once they re-render).
const MAX: usize = 4096;

struct Entry {
    sid: Option<String>,
    src: String,
}

#[derive(Default)]
struct Registry {
    entries: HashMap<String, Entry>,
    order: VecDeque<String>,
}

fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn key() -> &'static [u8; 32] {
    static K: OnceLock<[u8; 32]> = OnceLock::new();
    K.get_or_init(|| {
        let mut k = [0u8; 32];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut k)
            .expect("system random");
        k
    })
}

/// The proxy URL for `src` when it carries headers (`None` otherwise), for
/// the session `sid` (any session when `None`).
pub(crate) fn proxied(base: &str, sid: Option<&str>, src: &str) -> Option<String> {
    let (_, headers) = prism_core::picture_source(src);
    if headers.is_empty() {
        return None;
    }
    let tag = ring::hmac::sign(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key()),
        format!("{}\0{src}", sid.unwrap_or("")).as_bytes(),
    );
    let id = crate::security::base64url(&tag.as_ref()[..18]);
    let mut r = registry().lock().unwrap();
    if !r.entries.contains_key(&id) {
        if r.order.len() >= MAX
            && let Some(old) = r.order.pop_front()
        {
            r.entries.remove(&old);
        }
        r.order.push_back(id.clone());
        r.entries.insert(
            id.clone(),
            Entry {
                sid: sid.map(str::to_string),
                src: src.to_string(),
            },
        );
    }
    Some(format!("{base}/_prism/picture/{id}"))
}

/// `GET <base>/_prism/picture/<id>` from the session `sid` (its cookie).
pub(crate) fn serve(id: &str, sid: Option<&str>, trusted: &[String]) -> Response<Vec<u8>> {
    let src = {
        let r = registry().lock().unwrap();
        match r.entries.get(id) {
            Some(e) if e.sid.is_none() || e.sid.as_deref() == sid => e.src.clone(),
            _ => return status(StatusCode::NOT_FOUND),
        }
    };
    let (url, headers) = prism_core::picture_source(&src);
    let mut request = upstream(url, trusted);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let res = match request.send_blocking() {
        Ok(res) => res,
        // Never log the headers: they hold tokens.
        Err(e) => {
            eprintln!("prism-web: picture {url}: {e}");
            return status(StatusCode::BAD_GATEWAY);
        }
    };
    if !res.ok() {
        eprintln!("prism-web: picture {url}: HTTP {}", res.status);
        return status(match res.status {
            401 | 403 | 404 | 410 => StatusCode::NOT_FOUND,
            _ => StatusCode::BAD_GATEWAY,
        });
    }
    let mime = res
        .header("content-type")
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !mime.starts_with("image/") {
        eprintln!("prism-web: picture {url}: not an image ({mime:?})");
        return status(StatusCode::BAD_GATEWAY);
    }
    let mut r = Response::new(res.body);
    let h = r.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&mime) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=300"),
    );
    h.insert(header::VARY, HeaderValue::from_static("Cookie"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
    );
    r
}

/// The most bytes one ranged media request fetches (#207): the browser
/// asks for the rest as it plays.
const MEDIA_CHUNK: u64 = 4 * 1024 * 1024;

/// `GET <base>/_prism/media/<id>` (#207): a video or audio file behind
/// headers, for the session `sid`, in ranges (`range`: the browser's
/// `Range` header). The id is the one [`proxied`] gives (the same
/// registry); the upstream gets the headers and a range of at most
/// [`MEDIA_CHUNK`] bytes.
pub(crate) fn serve_media(
    id: &str,
    sid: Option<&str>,
    range: Option<&str>,
    trusted: &[String],
) -> Response<Vec<u8>> {
    let src = {
        let r = registry().lock().unwrap();
        match r.entries.get(id) {
            Some(e) if e.sid.is_none() || e.sid.as_deref() == sid => e.src.clone(),
            _ => return status(StatusCode::NOT_FOUND),
        }
    };
    let (start, end) = parse_range(range);
    let last = end
        .unwrap_or(start + MEDIA_CHUNK - 1)
        .min(start + MEDIA_CHUNK - 1);
    let (url, headers) = prism_core::picture_source(&src);
    let mut request = upstream(url, trusted).header("Range", format!("bytes={start}-{last}"));
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let res = match request.send_blocking() {
        Ok(res) => res,
        Err(e) => {
            eprintln!("prism-web: media {url}: {e}");
            return status(StatusCode::BAD_GATEWAY);
        }
    };
    if !(res.ok() || res.status == 206) {
        eprintln!("prism-web: media {url}: HTTP {}", res.status);
        return status(match res.status {
            401 | 403 | 404 | 410 => StatusCode::NOT_FOUND,
            416 => StatusCode::RANGE_NOT_SATISFIABLE,
            _ => StatusCode::BAD_GATEWAY,
        });
    }
    let mime = res
        .header("content-type")
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let media = mime.starts_with("video/")
        || mime.starts_with("audio/")
        || mime == "application/octet-stream";
    if !media {
        eprintln!("prism-web: media {url}: not a video ({mime:?})");
        return status(StatusCode::BAD_GATEWAY);
    }
    // The upstream's range when it answered one; else the whole file came:
    // pass the asked part on, as a range.
    let content_range = res.header("content-range").map(str::to_string);
    let (body, content_range) = match content_range {
        Some(cr) if res.status == 206 => (res.body, cr),
        _ => {
            let total = res.body.len() as u64;
            if start >= total {
                return status(StatusCode::RANGE_NOT_SATISFIABLE);
            }
            let stop = last.min(total - 1);
            let part = res.body[start as usize..=stop as usize].to_vec();
            (part, format!("bytes {start}-{stop}/{total}"))
        }
    };
    let mut r = Response::new(body);
    *r.status_mut() = StatusCode::PARTIAL_CONTENT;
    let h = r.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&mime) {
        h.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(&content_range) {
        h.insert(header::CONTENT_RANGE, v);
    }
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=300"),
    );
    h.insert(header::VARY, HeaderValue::from_static("Cookie"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    r
}

/// The request for `url` (a picture or video with headers): redirects
/// within its origin only, and public addresses only unless the app trusts
/// the origin.
fn upstream(url: &str, trusted: &[String]) -> prism_http::Request {
    let request = prism_http::Request::get(url).redirects(prism_http::Redirects::SameOrigin);
    match origin(url) {
        Some(o) if trusted.contains(&o) => request,
        _ => request.public_only(),
    }
}

/// `scheme://host:port` of an HTTP(S) URL, lowercase, with the default port
/// written out; `None` for anything else.
pub(crate) fn origin(url: &str) -> Option<String> {
    let uri: http::Uri = url.parse().ok()?;
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    let port = match (scheme.as_str(), uri.port_u16()) {
        (_, Some(p)) => p,
        ("https", None) => 443,
        ("http", None) => 80,
        _ => return None,
    };
    if scheme != "https" && scheme != "http" {
        return None;
    }
    Some(format!(
        "{scheme}://{}:{port}",
        uri.host()?.to_ascii_lowercase()
    ))
}

/// `bytes=start-[end]` (one range; anything else: from the start).
fn parse_range(range: Option<&str>) -> (u64, Option<u64>) {
    let Some(spec) = range.and_then(|r| r.trim().strip_prefix("bytes=")) else {
        return (0, None);
    };
    let (a, b) = spec
        .split(',')
        .next()
        .unwrap_or("")
        .split_once('-')
        .unwrap_or(("", ""));
    let start = a.trim().parse().unwrap_or(0);
    let end = b.trim().parse().ok().filter(|e: &u64| *e >= start);
    (start, end)
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    let mut r = Response::new(Vec::new());
    *r.status_mut() = code;
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #207: one range, else from the start.
    #[test]
    fn media_ranges_parse() {
        assert_eq!(parse_range(None), (0, None));
        assert_eq!(parse_range(Some("bytes=0-")), (0, None));
        assert_eq!(parse_range(Some("bytes=100-199")), (100, Some(199)));
        assert_eq!(parse_range(Some("bytes=100-50")), (100, None));
        assert_eq!(parse_range(Some("bytes=5-9, 20-30")), (5, Some(9)));
        assert_eq!(parse_range(Some("items=1-2")), (0, None));
    }

    /// #207: a video behind a token, served in ranges for its session.
    #[test]
    fn media_behind_headers_is_served_in_ranges() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let len = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..len]).to_ascii_lowercase();
                let body = b"0123456789";
                let reply = if !req.contains("authorization: bearer s3cret") {
                    "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_string()
                } else {
                    // A server that ignores ranges: the whole file.
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        String::from_utf8_lossy(body)
                    )
                };
                let _ = s.write_all(reply.as_bytes());
            }
        });
        let src = format!("http://{addr}/talk.mp4\nAuthorization: Bearer s3cret");
        // Loopback: trusted.
        let t = trust(&format!("http://{addr}"));
        let url = proxied("/app", Some("sid-a"), &src).unwrap();
        let id = url.rsplit('/').next().unwrap();
        assert_eq!(
            serve_media(id, Some("sid-b"), None, &t).status(),
            StatusCode::NOT_FOUND
        );
        let r = serve_media(id, Some("sid-a"), Some("bytes=2-5"), &t);
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(r.body(), b"2345");
        assert_eq!(r.headers()[header::CONTENT_RANGE], "bytes 2-5/10");
        assert_eq!(r.headers()[header::CONTENT_TYPE], "video/mp4");
        let all = serve_media(id, Some("sid-a"), None, &t);
        assert_eq!(all.body(), b"0123456789");
    }
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// The upstreams of these tests are on loopback: the app trusts them.
    fn trust(url: &str) -> Vec<String> {
        origin(url).into_iter().collect()
    }

    #[test]
    fn origins_are_normalized() {
        assert_eq!(
            origin("HTTPS://API.example/x").unwrap(),
            "https://api.example:443"
        );
        assert_eq!(
            origin("http://10.0.0.5:8080").unwrap(),
            "http://10.0.0.5:8080"
        );
        assert_eq!(origin("http://[::1]/a").unwrap(), "http://[::1]:80");
        assert_eq!(origin("ftp://x.example/"), None);
        assert_eq!(origin("logo.png"), None);
    }

    fn upstream(n: usize, ctype: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..n {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let len = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..len]).to_ascii_lowercase();
                let reply = if req.contains("authorization: bearer s3cret") {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: 3\r\n\r\nimg"
                    )
                } else {
                    "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_string()
                };
                let _ = s.write_all(reply.as_bytes());
            }
        });
        format!("http://{addr}/avatar")
    }

    #[test]
    fn proxies_with_the_headers_for_its_session_only() {
        let up = upstream(1, "image/png");
        let t = trust(&up);
        let src = format!("{up}\nAuthorization: Bearer s3cret");
        assert_eq!(proxied("/app", Some("a"), "https://x/y.png"), None);
        let url = proxied("/app", Some("sid-a"), &src).unwrap();
        assert!(url.starts_with("/app/_prism/picture/"));
        assert!(!url.contains("s3cret"));
        // Stable for the same session and source; another session, another id.
        assert_eq!(proxied("/app", Some("sid-a"), &src).unwrap(), url);
        assert_ne!(proxied("/app", Some("sid-b"), &src).unwrap(), url);
        let id = url.rsplit('/').next().unwrap();
        assert_eq!(serve(id, Some("sid-b"), &t).status(), StatusCode::NOT_FOUND);
        assert_eq!(serve(id, None, &t).status(), StatusCode::NOT_FOUND);
        assert_eq!(
            serve("nope", Some("sid-a"), &t).status(),
            StatusCode::NOT_FOUND
        );
        let ok = serve(id, Some("sid-a"), &t);
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(ok.body(), b"img");
        assert_eq!(ok.headers()[header::CONTENT_TYPE], "image/png");
        assert!(
            ok.headers()[header::CACHE_CONTROL]
                .to_str()
                .unwrap()
                .starts_with("private")
        );
        assert_eq!(ok.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    }

    #[test]
    fn refuses_what_is_not_an_image() {
        let up = upstream(2, "text/html");
        let t = trust(&up);
        let src = format!("{up}\nAuthorization: Bearer s3cret");
        let url = proxied("", Some("s"), &src).unwrap();
        let id = url.rsplit('/').next().unwrap();
        assert_eq!(serve(id, Some("s"), &t).status(), StatusCode::BAD_GATEWAY);
        // A wrong token upstream: not found.
        let bad = src.replace("s3cret", "wrong");
        let url = proxied("", Some("s"), &bad).unwrap();
        assert_eq!(
            serve(url.rsplit('/').next().unwrap(), Some("s"), &t).status(),
            StatusCode::NOT_FOUND
        );
    }
}
