//! 3D scene blobs on the web, `GET <base>/_prism/3d/<id>`: the meshes,
//! instance sets, clips, skins, environments and embedded images of the
//! scenes pages show (`prism_core::scene3d::blobs`).
//!
//! - A blob goes only to the browsers it was sent to (their `prism_sid`
//!   cookie), in a first paint or a live session's scene update, for
//!   [`SENT_FOR`] after it was last sent, while the server holds it. A
//!   scene may hold a user's own data, and an id says nothing about who
//!   may see it: other browsers get a 404.
//! - `Cache-Control: private, max-age=31536000, immutable`: the browser
//!   keeps it (an id never changes), shared caches do not. An app whose
//!   scenes hold nothing private serves them to anyone and lets CDNs keep
//!   them with `WebApp::public_scenes(true)`.
//! - A first paint keeps its scenes, and so their blobs, [`FIRST_PAINT_PIN`]
//!   for its live session to take over: the last [`MAX_PINNED`] pages and
//!   [`MAX_PINNED_BYTES`] of blobs at most (the oldest go first).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use http::{HeaderValue, Response, StatusCode, header};
use prism_core::scene3d::{FlatScene, SceneUpdate, blobs};
use serde_json::Value;

/// How long a browser may fetch a blob after it was last sent to it: it
/// fetches at once, and again only after a failure, when the next update
/// that refers to the blob sends it again.
const SENT_FOR: Duration = Duration::from_secs(600);
/// Grants kept at most (the oldest go first).
const MAX_GRANTS: usize = 1 << 17;
/// How long the blobs of a server-rendered page stay fetchable without a
/// live session holding them: the time for the browser to load the page
/// and its socket to take over.
const FIRST_PAINT_PIN: Duration = Duration::from_secs(120);
/// First paints pinned at most.
const MAX_PINNED: usize = 256;
/// Bytes of blobs the pinned first paints hold at most.
const MAX_PINNED_BYTES: usize = 128 << 20;

/// Which browser may fetch which blob: (blob, browser) → when it was last
/// sent, and the grant's number (newer grants have larger ones).
#[derive(Default)]
struct Grants {
    sent: HashMap<(u128, u128), (Instant, u64)>,
    count: u64,
}

impl Grants {
    fn grant(&mut self, sid: u128, ids: &[u128], now: Instant) {
        for &id in ids {
            self.count += 1;
            self.sent.insert((id, sid), (now, self.count));
        }
        if self.sent.len() > MAX_GRANTS {
            self.trim(now, MAX_GRANTS / 2);
        }
    }

    /// Forgets expired grants, then all but the `keep` newest.
    fn trim(&mut self, now: Instant, keep: usize) {
        self.sent
            .retain(|_, (at, _)| now.duration_since(*at) < SENT_FOR);
        if self.sent.len() > keep {
            let cut = self.count.saturating_sub(keep as u64);
            self.sent.retain(|_, (_, n)| *n > cut);
        }
    }

    fn allows(&self, sid: u128, id: u128, now: Instant) -> bool {
        self.sent
            .get(&(id, sid))
            .is_some_and(|(at, _)| now.duration_since(*at) < SENT_FOR)
    }
}

/// The scenes of recent first paints, kept so the blobs they reference
/// (built during that render: `Instances`, `Mesh::custom`, clips) stay
/// until the page's session renders them again.
#[derive(Default)]
struct Pinned(VecDeque<Pin>);

struct Pin {
    at: Instant,
    bytes: usize,
    scenes: Vec<FlatScene>,
}

impl Pinned {
    fn pin(&mut self, pin: Pin, now: Instant) {
        self.0
            .retain(|p| now.duration_since(p.at) <= FIRST_PAINT_PIN);
        if !pin.scenes.is_empty() {
            self.0.push_back(pin);
        }
        while self.0.len() > MAX_PINNED
            || (self.0.len() > 1
                && self.0.iter().map(|p| p.bytes).sum::<usize>() > MAX_PINNED_BYTES)
        {
            self.0.pop_front();
        }
    }
}

#[derive(Default)]
struct State {
    grants: Grants,
    pinned: Pinned,
}

fn state() -> MutexGuard<'static, State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// A blob id or a browser id: 32 hex digits.
fn hex_id(s: &str) -> Option<u128> {
    if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u128::from_str_radix(s, 16).ok()
}

/// Every blob id a scene update names (`"id"`, `"clip"`, an embedded
/// image's `"p3d:<id>"`…): whatever the client may fetch.
fn ids_in(v: &Value, out: &mut Vec<u128>) {
    match v {
        Value::String(s) => out.extend(hex_id(s.strip_prefix("p3d:").unwrap_or(s))),
        Value::Array(items) => items.iter().for_each(|v| ids_in(v, out)),
        Value::Object(fields) => fields.values().for_each(|v| ids_in(v, out)),
        _ => {}
    }
}

/// A live session sent `update` (a scene update's JSON) to the browser
/// `sid`: it may fetch the blobs it names.
pub(crate) fn sent(sid: Option<&str>, update: &Value) {
    let Some(sid) = sid.and_then(hex_id) else {
        return;
    };
    let mut ids = Vec::new();
    ids_in(update, &mut ids);
    if !ids.is_empty() {
        state().grants.grant(sid, &ids, Instant::now());
    }
}

/// A first paint for the browser `sid` showed `scenes`: it may fetch their
/// blobs, which stay until its live session takes over.
pub(crate) fn first_paint(sid: &str, scenes: Vec<FlatScene>) {
    let mut ids = Vec::new();
    let scenes: Vec<FlatScene> = scenes
        .into_iter()
        .map(|scene| {
            let full = SceneUpdate::Full(scene);
            ids_in(&full.to_value(), &mut ids);
            match full {
                SceneUpdate::Full(scene) => scene,
                SceneUpdate::Diff(_) => FlatScene::default(),
            }
        })
        .collect();
    let unique: HashSet<u128> = ids.iter().copied().collect();
    let bytes = unique
        .iter()
        .filter_map(|id| blobs::size(&format!("{id:032x}")))
        .sum();
    let now = Instant::now();
    let mut state = state();
    state.pinned.pin(
        Pin {
            at: now,
            bytes,
            scenes,
        },
        now,
    );
    if let Some(sid) = hex_id(sid).filter(|_| !ids.is_empty()) {
        state.grants.grant(sid, &ids, now);
    }
}

/// `GET <base>/_prism/3d/<id>` from the browser `sid` (its cookie);
/// `public`: the app's scenes hold nothing private
/// (`WebApp::public_scenes`).
pub(crate) fn serve(id: &str, sid: Option<&str>, public: bool) -> Response<Vec<u8>> {
    let allowed = public
        || match (hex_id(id), sid.and_then(hex_id)) {
            (Some(id), Some(sid)) => state().grants.allows(sid, id, Instant::now()),
            _ => false,
        };
    let found = if allowed { blobs::get(id) } else { None };
    let Some((bytes, mime)) = found else {
        let mut r = Response::new(Vec::new());
        *r.status_mut() = StatusCode::NOT_FOUND;
        return r;
    };
    let mut r = Response::new(bytes.to_vec());
    if let Ok(v) = HeaderValue::from_str(&mime) {
        r.headers_mut().insert(header::CONTENT_TYPE, v);
    }
    r.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if public {
            "public, max-age=31536000, immutable"
        } else {
            "private, max-age=31536000, immutable"
        }),
    );
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::scene3d::blobs::BlobKind;

    const A: &str = "0123456789abcdef0123456789abcdef";
    const B: &str = "fedcba9876543210fedcba9876543210";

    /// S29: a blob goes only to the browser it was sent to, never to a
    /// shared cache.
    #[test]
    fn s29_a_blob_goes_only_to_the_browser_it_was_sent_to() {
        let mesh = blobs::put(
            BlobKind::Mesh,
            blobs::BINARY_MIME,
            b"s29 private mesh".to_vec(),
        );
        let image = blobs::put(BlobKind::Image, "image/png", b"s29 private image".to_vec());
        let update = serde_json::json!({
            "v": 2, "full": true,
            "nodes": [{ "key": "0", "mesh": { "type": "blob", "id": mesh.id() },
                        "mat": { "map": format!("p3d:{}", image.id()) } }],
        });
        // Nobody was sent it yet.
        assert_eq!(
            serve(mesh.id(), Some(A), false).status(),
            StatusCode::NOT_FOUND
        );
        sent(Some(A), &update);
        for blob in [&mesh, &image] {
            let r = serve(blob.id(), Some(A), false);
            assert_eq!(r.status(), StatusCode::OK);
            assert_eq!(
                r.headers()[header::CACHE_CONTROL],
                "private, max-age=31536000, immutable"
            );
            assert_eq!(
                serve(blob.id(), Some(B), false).status(),
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                serve(blob.id(), None, false).status(),
                StatusCode::NOT_FOUND
            );
        }
        // Declared public: anyone, and shared caches may keep it.
        let r = serve(mesh.id(), None, true);
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        // Gone from the server: gone for everyone.
        let id = mesh.id().to_string();
        drop(mesh);
        assert_eq!(serve(&id, Some(A), false).status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn s29_grants_expire_and_stay_bounded() {
        let t0 = Instant::now();
        let mut g = Grants::default();
        g.grant(1, &[10, 11], t0);
        assert!(g.allows(1, 10, t0) && !g.allows(2, 10, t0));
        assert!(!g.allows(1, 10, t0 + SENT_FOR));
        for sid in 0..(MAX_GRANTS as u128 + 10) {
            g.grant(sid + 100, &[sid], t0 + Duration::from_secs(1));
        }
        assert!(g.sent.len() <= MAX_GRANTS);
        // The newest are kept.
        let last = MAX_GRANTS as u128 + 9;
        assert!(g.allows(last + 100, last, t0 + Duration::from_secs(1)));
    }

    #[test]
    fn s29_first_paints_pinned_are_bounded_in_count_and_bytes() {
        let t0 = Instant::now();
        let pin = |at, bytes| Pin {
            at,
            bytes,
            scenes: vec![FlatScene::default()],
        };
        let mut p = Pinned::default();
        for _ in 0..MAX_PINNED + 5 {
            p.pin(pin(t0, 1), t0);
        }
        assert_eq!(p.0.len(), MAX_PINNED);
        p.pin(pin(t0, MAX_PINNED_BYTES), t0);
        assert_eq!(p.0.len(), 1, "the oldest went first");
        // Alone, a page larger than the bound is still kept.
        assert_eq!(p.0[0].bytes, MAX_PINNED_BYTES);
        p.pin(pin(t0 + FIRST_PAINT_PIN * 2, 1), t0 + FIRST_PAINT_PIN * 2);
        assert_eq!(p.0.len(), 1, "expired pins go");
    }
}
