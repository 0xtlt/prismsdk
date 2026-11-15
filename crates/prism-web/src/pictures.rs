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
pub(crate) fn serve(id: &str, sid: Option<&str>) -> Response<Vec<u8>> {
    let src = {
        let r = registry().lock().unwrap();
        match r.entries.get(id) {
            Some(e) if e.sid.is_none() || e.sid.as_deref() == sid => e.src.clone(),
            _ => return status(StatusCode::NOT_FOUND),
        }
    };
    let (url, headers) = prism_core::picture_source(&src);
    let mut request = prism_http::Request::get(url);
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

fn status(code: StatusCode) -> Response<Vec<u8>> {
    let mut r = Response::new(Vec::new());
    *r.status_mut() = code;
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

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
        let src = format!("{}\nAuthorization: Bearer s3cret", upstream(1, "image/png"));
        assert_eq!(proxied("/app", Some("a"), "https://x/y.png"), None);
        let url = proxied("/app", Some("sid-a"), &src).unwrap();
        assert!(url.starts_with("/app/_prism/picture/"));
        assert!(!url.contains("s3cret"));
        // Stable for the same session and source; another session, another id.
        assert_eq!(proxied("/app", Some("sid-a"), &src).unwrap(), url);
        assert_ne!(proxied("/app", Some("sid-b"), &src).unwrap(), url);
        let id = url.rsplit('/').next().unwrap();
        assert_eq!(serve(id, Some("sid-b")).status(), StatusCode::NOT_FOUND);
        assert_eq!(serve(id, None).status(), StatusCode::NOT_FOUND);
        assert_eq!(serve("nope", Some("sid-a")).status(), StatusCode::NOT_FOUND);
        let ok = serve(id, Some("sid-a"));
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
        let src = format!("{}\nAuthorization: Bearer s3cret", upstream(2, "text/html"));
        let url = proxied("", Some("s"), &src).unwrap();
        let id = url.rsplit('/').next().unwrap();
        assert_eq!(serve(id, Some("s")).status(), StatusCode::BAD_GATEWAY);
        // A wrong token upstream: not found.
        let bad = src.replace("s3cret", "wrong");
        let url = proxied("", Some("s"), &bad).unwrap();
        assert_eq!(
            serve(url.rsplit('/').next().unwrap(), Some("s")).status(),
            StatusCode::NOT_FOUND
        );
    }
}
