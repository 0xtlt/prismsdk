//! The page request, handed over to the live session that takes the page
//! over (`cx.page_request()`).
//!
//! The first paint and the live session are two runs of the app: one for the
//! page request, one for the socket handshake. What the host gave only to the
//! page request (a flash message it cleared, a one-time token) would be lost
//! between them. So each page carries a random id (`data-page` on Prism's
//! script); the page's first handshake sends it back (`page=`), and the
//! session takes that page's request out of this store, once.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a page request waits for its live session.
pub(crate) const TTL: Duration = Duration::from_secs(60);

/// At most this many page requests wait at once (the oldest go first): pages
/// opened by clients that never connect (crawlers) cannot grow it unbounded.
pub(crate) const CAPACITY: usize = 10_000;

struct Waiting {
    /// The browser that loaded the page (its `prism_sid`).
    sid: String,
    request: prism_core::WebRequest,
    at: Instant,
}

/// Page requests waiting for their live session, by page id.
#[derive(Default)]
pub(crate) struct Handoffs {
    inner: Mutex<State>,
}

#[derive(Default)]
struct State {
    waiting: HashMap<String, Waiting>,
    /// Page ids in the order they were added, to expire them.
    order: VecDeque<(Instant, String)>,
}

impl State {
    /// Drops the expired requests, then the oldest beyond `CAPACITY`.
    fn trim(&mut self, now: Instant) {
        while let Some((at, id)) = self.order.front() {
            if now.duration_since(*at) < TTL && self.waiting.len() < CAPACITY {
                break;
            }
            // A taken page's id may still be here: removing it does nothing.
            self.waiting.remove(id);
            self.order.pop_front();
        }
    }
}

impl Handoffs {
    /// Keeps the request of a page loaded by browser `sid`; returns the page's id.
    pub fn put(&self, sid: &str, request: prism_core::WebRequest) -> String {
        let id = crate::app::new_session_id();
        let now = Instant::now();
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.trim(now);
        state.waiting.insert(
            id.clone(),
            Waiting {
                sid: sid.to_string(),
                request,
                at: now,
            },
        );
        state.order.push_back((now, id.clone()));
        id
    }

    /// Takes the request of page `id`, if browser `sid` loaded it less than
    /// `TTL` ago. Gone afterwards, whoever asked.
    pub fn take(&self, id: &str, sid: &str) -> Option<prism_core::WebRequest> {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let waiting = state.waiting.remove(id)?;
        (waiting.sid == sid && waiting.at.elapsed() < TTL).then_some(waiting.request)
    }

    /// Page requests waiting now.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .waiting
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str) -> prism_core::WebRequest {
        prism_core::WebRequest {
            url: url.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_page_request_is_taken_once_by_its_browser() {
        let handoffs = Handoffs::default();
        let id = handoffs.put("a", request("/one"));
        // Another browser: refused, and the request is gone.
        assert!(handoffs.take(&id, "b").is_none());
        assert!(handoffs.take(&id, "a").is_none());
        let id = handoffs.put("a", request("/two"));
        assert_eq!(handoffs.take(&id, "a").unwrap().url, "/two");
        assert!(handoffs.take(&id, "a").is_none());
        assert!(handoffs.take("unknown", "a").is_none());
        assert_eq!(handoffs.len(), 0);
    }

    #[test]
    fn the_oldest_go_beyond_capacity() {
        let handoffs = Handoffs::default();
        let first = handoffs.put("a", request("/first"));
        for _ in 0..CAPACITY {
            handoffs.put("a", request("/later"));
        }
        assert_eq!(handoffs.len(), CAPACITY);
        assert!(handoffs.take(&first, "a").is_none());
    }
}
