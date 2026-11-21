//! Files the app hands to the user on the web (`cx.save_file`, #210). The
//! app runs on the server and holds the bytes (fetched with a bearer token,
//! generated), so the server keeps them and the page downloads them from
//! `<base>/_prism/download/<id>`, a same-origin URL with
//! `Content-Disposition: attachment`: no header or token reaches the page.
//!
//! - `<id>` is random (144 bits) and bound to the session that offered it:
//!   another browser (another `prism_sid` cookie) gets a 404.
//! - Files are kept 10 minutes, the last 64 and 256 MiB at most (the oldest
//!   go first), so a page can download one again soon after.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use http::{HeaderValue, Response, StatusCode, header};

const KEEP: Duration = Duration::from_secs(600);
const MAX_FILES: usize = 64;
const MAX_BYTES: usize = 256 * 1024 * 1024;

struct Entry {
    id: String,
    sid: Option<String>,
    name: String,
    mime: String,
    bytes: Arc<Vec<u8>>,
    at: Instant,
}

fn store() -> &'static Mutex<VecDeque<Entry>> {
    static S: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Keeps `file` for the session `sid` and returns its download URL.
pub(crate) fn offer(
    base: &str,
    sid: Option<&str>,
    name: String,
    mime: String,
    bytes: Vec<u8>,
) -> String {
    let mut raw = [0u8; 18];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut raw)
        .expect("system random");
    let id = crate::security::base64url(&raw);
    let mut s = store().lock().unwrap();
    let now = Instant::now();
    s.retain(|e| now.duration_since(e.at) < KEEP);
    s.push_back(Entry {
        id: id.clone(),
        sid: sid.map(str::to_string),
        name,
        mime,
        bytes: Arc::new(bytes),
        at: now,
    });
    while s.len() > MAX_FILES
        || (s.len() > 1 && s.iter().map(|e| e.bytes.len()).sum::<usize>() > MAX_BYTES)
    {
        s.pop_front();
    }
    format!("{base}/_prism/download/{id}")
}

/// The browser `old` now has the id `new` (S10): its downloads follow it.
pub(crate) fn renamed(old: &str, new: &str) {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    for e in s.iter_mut() {
        if e.sid.as_deref() == Some(old) {
            e.sid = Some(new.to_string());
        }
    }
}

/// `GET <base>/_prism/download/<id>` from the session `sid` (its cookie).
pub(crate) fn serve(id: &str, sid: Option<&str>) -> Response<Vec<u8>> {
    let found = {
        let s = store().lock().unwrap();
        s.iter()
            .find(|e| e.id == id && e.at.elapsed() < KEEP)
            .filter(|e| e.sid.is_none() || e.sid.as_deref() == sid)
            .map(|e| (e.name.clone(), e.mime.clone(), e.bytes.clone()))
    };
    let Some((name, mime, bytes)) = found else {
        let mut r = Response::new(Vec::new());
        *r.status_mut() = StatusCode::NOT_FOUND;
        return r;
    };
    let mut r = Response::new(bytes.as_ref().clone());
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    if let Ok(v) = HeaderValue::from_str(&disposition(&name)) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(crate::app::SANDBOX),
    );
    r
}

/// `attachment` with an ASCII `filename` and the UTF-8 `filename*` (RFC 6266).
fn disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s10_a_download_follows_its_browser_to_its_new_id() {
        let url = offer(
            "",
            Some("s10-dl-old"),
            "a.txt".into(),
            "text/plain".into(),
            b"a".to_vec(),
        );
        let id = url.strip_prefix("/_prism/download/").unwrap();
        renamed("s10-dl-old", "s10-dl-new");
        assert_eq!(
            serve(id, Some("s10-dl-old")).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(serve(id, Some("s10-dl-new")).status(), StatusCode::OK);
    }

    #[test]
    fn a_download_is_served_once_offered_to_its_session_only() {
        let url = offer(
            "/app",
            Some("sid-a"),
            "Rapport été.pdf".into(),
            "application/pdf".into(),
            b"%PDF-1.7".to_vec(),
        );
        let id = url.strip_prefix("/app/_prism/download/").unwrap();
        let r = serve(id, Some("sid-a"));
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.body(), b"%PDF-1.7");
        assert_eq!(r.headers()[header::CONTENT_TYPE], "application/pdf");
        assert_eq!(
            r.headers()[header::CONTENT_DISPOSITION],
            "attachment; filename=\"Rapport _t_.pdf\"; filename*=UTF-8''Rapport%20%C3%A9t%C3%A9.pdf"
        );
        assert_eq!(r.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(serve(id, Some("sid-b")).status(), StatusCode::NOT_FOUND);
        assert_eq!(serve(id, None).status(), StatusCode::NOT_FOUND);
        assert_eq!(serve("nope", Some("sid-a")).status(), StatusCode::NOT_FOUND);
    }
}
