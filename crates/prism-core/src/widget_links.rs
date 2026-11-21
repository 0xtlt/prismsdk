//! Widget-action links (security review S19): the links a macOS widget's
//! buttons open the app on, because macOS widgets cannot run the app's code
//! in place. Any app or web page can open an app's links, and a link that
//! was opened once can be captured (a log, a screenshot of a URL, another
//! app that claimed the URL scheme) and opened again. So a link runs its
//! action only when all three hold:
//!
//! 1. **Signed**: its token is an HMAC-SHA256, under the app's key, of the
//!    drawing it belongs to and of the action (widget kind, action,
//!    payload). Only the app can make one.
//! 2. **On screen**: its drawing is the widgets' current one, or one the
//!    app replaced less than [`GRACE`] ago. Each pass that draws the widgets
//!    ([`App::reload_widgets`](crate::App::reload_widgets)) is a new
//!    drawing with a new id; WidgetKit shows it a moment after the app
//!    writes it, so the previous drawing's buttons stay valid for a while.
//! 3. **Unused**: the ledger records each link that ran, and a link runs
//!    once. The action's job reloads the widgets, which gives every button
//!    a fresh link.
//!
//! The ledger (the live drawings and the links used in them) is kept by the
//! backend between runs, like the key: a widget shows its links long after
//! the app drew it, and a tap may launch the app. It is read again before
//! every check and every drawing, so a second instance of the app sharing
//! it refuses a link the first one ran (macOS sends a link to the running
//! instance, so two seldom check at the same moment).
//!
//! Why this design:
//!
//! - **No expiry in the token.** A widget timeline ends only when the app
//!   reloads it (WidgetKit's `.never` policy): a button may be tapped hours
//!   or days after the app drew it, by a tap that launches the app, and that
//!   first tap must work. A link expires when the widget stops showing it:
//!   its drawing replaced, plus the grace.
//! - **One key, a drawing id in the MAC.** A key per drawing would bind the
//!   links the same way, but every live drawing's key would have to be kept;
//!   one long-term secret and public ids do it with nothing secret in the
//!   ledger.
//! - **The used links, not every issued one.** An allow-list of random
//!   per-link nonces works too, but it holds every button of every family,
//!   configuration and drawing ahead, rewritten at each reload. The MAC
//!   makes the issued links implicit; the ledger only holds what was tapped.

use std::time::{Duration, SystemTime};

use crate::storage::Storage;
use crate::surfaces::WidgetAction;

/// How long a drawing's links keep working after the app drew the widgets
/// again: time for WidgetKit to show the new drawing (or for a tap to
/// launch the app, which draws the widgets as it starts) and slack for a
/// reload WidgetKit postpones. A tap on a button older than that redraws
/// the widgets instead of running its action.
pub(crate) const GRACE: Duration = Duration::from_secs(60 * 60);
/// The most drawings kept (a burst of reloads within the grace).
const MAX_DRAWINGS: usize = 32;
/// Bytes of the MAC a token keeps (128 bits).
const MAC_LEN: usize = 16;
/// The ledger's entry in the backend's store.
const LEDGER: &str = "widget-link-ledger";
/// The ledger's format.
const VERSION: u64 = 1;

/// What a widget-action link may do ([`WidgetLinks::check`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkCheck {
    /// Signed, on screen, unused: run it (it is now used).
    Run,
    /// The app's own link, opened before.
    Used,
    /// The app's own link, from a drawing replaced too long ago.
    Expired,
    /// Not the app's: no token, a wrong one, or no key.
    Forged,
}

/// The app's key for widget-action links and the ledger of their drawings
/// ([`App::set_widget_key`](crate::App::set_widget_key)).
pub(crate) struct WidgetLinks {
    key: [u8; 32],
    store: Box<dyn Storage>,
    ledger: Ledger,
}

impl std::fmt::Debug for WidgetLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WidgetLinks(..)")
    }
}

impl WidgetLinks {
    /// The links of `key`, with the ledger `store` keeps.
    pub(crate) fn new(key: [u8; 32], store: Box<dyn Storage>) -> Self {
        let ledger = store
            .get(LEDGER)
            .map(|s| Ledger::parse(&s))
            .unwrap_or_default();
        WidgetLinks { key, store, ledger }
    }

    /// Starts a drawing of the widgets: the tokens signed from now on
    /// belong to it, and the previous drawing's links work for [`GRACE`]
    /// more.
    pub(crate) fn begin_drawing(&mut self, now: SystemTime) {
        self.sync(now);
        let at = nanos(now);
        let id = self
            .ledger
            .drawings
            .last()
            .map_or(at, |d| at.max(d.id.saturating_add(1)));
        self.ledger.drawings.push(Drawing { id, replaced: None });
        self.ledger.prune(secs(now));
        self.save();
    }

    /// Signs the actions of the current drawing; `None` before the first.
    pub(crate) fn signer(&self) -> Option<Signer<'_>> {
        let drawing = self.ledger.drawings.last()?.id;
        Some(Signer {
            links: self,
            drawing,
        })
    }

    /// Whether a link with `token` may run `action` now. A link that may
    /// is recorded as used, in the store too, before it runs.
    pub(crate) fn check(
        &mut self,
        action: &WidgetAction,
        token: Option<&str>,
        now: SystemTime,
    ) -> LinkCheck {
        let Some((drawing, mac)) = token.and_then(parse_token) else {
            return LinkCheck::Forged;
        };
        if !self.verify(drawing, action, &mac) {
            return LinkCheck::Forged;
        }
        self.sync(now);
        if !self.ledger.drawings.iter().any(|d| d.id == drawing) {
            return LinkCheck::Expired;
        }
        if self.ledger.used.contains(&(drawing, mac)) {
            return LinkCheck::Used;
        }
        self.ledger.used.push((drawing, mac));
        self.save();
        LinkCheck::Run
    }

    fn mac(&self, drawing: u64, a: &WidgetAction) -> hmac::Hmac<sha2::Sha256> {
        use hmac::Mac;
        let mut mac = <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(&self.key)
            .expect("HMAC takes any key length");
        mac.update(b"prism-widget-action/2");
        mac.update(&drawing.to_be_bytes());
        for part in [&a.kind, &a.action, &a.payload] {
            mac.update(&(part.len() as u64).to_be_bytes());
            mac.update(part.as_bytes());
        }
        mac
    }

    /// Whether `mac` is `a`'s in `drawing` (compared in constant time).
    fn verify(&self, drawing: u64, a: &WidgetAction, mac: &[u8; MAC_LEN]) -> bool {
        use hmac::Mac;
        self.mac(drawing, a).verify_truncated_left(mac).is_ok()
    }

    /// Merges what the store holds (another instance of the app may have
    /// written it since) and drops what has expired.
    fn sync(&mut self, now: SystemTime) {
        if let Some(stored) = self.store.get(LEDGER) {
            self.ledger.merge(Ledger::parse(&stored));
        }
        self.ledger.prune(secs(now));
    }

    /// Keeps the ledger. If the store fails, this run still has it.
    fn save(&mut self) {
        self.store.set(LEDGER, &self.ledger.to_json());
    }
}

/// Signs the actions of one drawing ([`WidgetLinks::signer`]).
pub(crate) struct Signer<'a> {
    links: &'a WidgetLinks,
    drawing: u64,
}

impl Signer<'_> {
    /// The token of `a`: the drawing's id, then the MAC (48 hex digits).
    pub(crate) fn token(&self, a: &WidgetAction) -> String {
        use hmac::Mac;
        let tag = self.links.mac(self.drawing, a).finalize().into_bytes();
        let mut out = format!("{:016x}", self.drawing);
        for b in &tag[..MAC_LEN] {
            out.push_str(&format!("{b:02x}"));
        }
        out
    }
}

/// A token's drawing id and MAC, if it has the shape of one.
fn parse_token(token: &str) -> Option<(u64, [u8; MAC_LEN])> {
    if token.len() != 16 + 2 * MAC_LEN || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let drawing = u64::from_str_radix(&token[..16], 16).ok()?;
    Some((drawing, hex_mac(&token[16..])?))
}

fn hex_mac(s: &str) -> Option<[u8; MAC_LEN]> {
    if s.len() != 2 * MAC_LEN || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; MAC_LEN];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

fn nanos(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// One drawing of the widgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Drawing {
    /// Unique and increasing (the time it started, in nanoseconds, or one
    /// more than the previous drawing's).
    id: u64,
    /// When the app drew the widgets again (Unix seconds).
    replaced: Option<u64>,
}

/// The drawings whose links a widget may still show, and the links used.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Ledger {
    /// Oldest first: the last one is the current drawing.
    drawings: Vec<Drawing>,
    /// Links that ran: their drawing and MAC.
    used: Vec<(u64, [u8; MAC_LEN])>,
}

impl Ledger {
    /// Takes in another copy: every drawing either knows, replaced at the
    /// earliest time either saw, and every link either saw used.
    fn merge(&mut self, other: Ledger) {
        for d in other.drawings {
            match self.drawings.iter_mut().find(|m| m.id == d.id) {
                Some(m) => {
                    m.replaced = match (m.replaced, d.replaced) {
                        (Some(a), Some(b)) => Some(a.min(b)),
                        (a, b) => a.or(b),
                    }
                }
                None => self.drawings.push(d),
            }
        }
        for u in other.used {
            if !self.used.contains(&u) {
                self.used.push(u);
            }
        }
    }

    /// Orders the drawings, marks all but the newest replaced (at `now` if
    /// no time was known), drops those replaced [`GRACE`] ago or more and
    /// the oldest past [`MAX_DRAWINGS`], and the used links of dropped
    /// drawings.
    fn prune(&mut self, now: u64) {
        self.drawings.sort_by_key(|d| d.id);
        self.drawings.dedup_by_key(|d| d.id);
        let older = self.drawings.len().saturating_sub(1);
        for d in &mut self.drawings[..older] {
            d.replaced.get_or_insert(now);
        }
        self.drawings.retain(|d| {
            d.replaced
                .is_none_or(|r| now < r.saturating_add(GRACE.as_secs()))
        });
        let extra = self.drawings.len().saturating_sub(MAX_DRAWINGS);
        self.drawings.drain(..extra);
        let drawings = &self.drawings;
        self.used
            .retain(|(id, _)| drawings.iter().any(|d| d.id == *id));
    }

    /// `{"v":1,"drawings":[[id,replaced|null],…],"used":[[id,"mac"],…]}`.
    fn to_json(&self) -> String {
        let drawings: Vec<serde_json::Value> = self
            .drawings
            .iter()
            .map(|d| serde_json::json!([d.id, d.replaced]))
            .collect();
        let used: Vec<serde_json::Value> = self
            .used
            .iter()
            .map(|(id, mac)| {
                let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
                serde_json::json!([id, hex])
            })
            .collect();
        serde_json::json!({ "v": VERSION, "drawings": drawings, "used": used }).to_string()
    }

    /// A ledger written by [`to_json`](Self::to_json); anything else is an
    /// empty one (every earlier link then refused: the safe side).
    fn parse(json: &str) -> Ledger {
        let parsed = (|| {
            let v: serde_json::Value = serde_json::from_str(json).ok()?;
            if v.get("v")?.as_u64()? != VERSION {
                return None;
            }
            let mut ledger = Ledger::default();
            for d in v.get("drawings")?.as_array()? {
                let replaced = match d.get(1)? {
                    serde_json::Value::Null => None,
                    r => Some(r.as_u64()?),
                };
                ledger.drawings.push(Drawing {
                    id: d.get(0)?.as_u64()?,
                    replaced,
                });
            }
            for u in v.get("used")?.as_array()? {
                ledger
                    .used
                    .push((u.get(0)?.as_u64()?, hex_mac(u.get(1)?.as_str()?)?));
            }
            Some(ledger)
        })();
        parsed.unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    /// A store two instances of the app share (one file on disk).
    #[derive(Clone, Default)]
    struct Shared(Rc<RefCell<HashMap<String, String>>>);

    impl Storage for Shared {
        fn get(&self, key: &str) -> Option<String> {
            self.0.borrow().get(key).cloned()
        }
        fn set(&mut self, key: &str, value: &str) {
            self.0.borrow_mut().insert(key.into(), value.into());
        }
        fn remove(&mut self, key: &str) {
            self.0.borrow_mut().remove(key);
        }
        fn keys(&self) -> Vec<String> {
            self.0.borrow().keys().cloned().collect()
        }
    }

    const KEY: [u8; 32] = [7; 32];

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
    }

    fn toggle() -> WidgetAction {
        WidgetAction::new("todo", "toggle", "1")
    }

    fn token(links: &WidgetLinks, a: &WidgetAction) -> String {
        links.signer().expect("a drawing").token(a)
    }

    #[test]
    fn a_link_runs_once() {
        let mut links = WidgetLinks::new(KEY, Box::new(Shared::default()));
        assert!(links.signer().is_none(), "no drawing, no link");
        links.begin_drawing(at(0));
        let t = token(&links, &toggle());
        assert_eq!(t.len(), 48);
        assert_eq!(links.check(&toggle(), Some(&t), at(1)), LinkCheck::Run);
        assert_eq!(links.check(&toggle(), Some(&t), at(2)), LinkCheck::Used);
        // The same token written in capitals is the same link.
        let upper = t.to_uppercase();
        assert_eq!(links.check(&toggle(), Some(&upper), at(2)), LinkCheck::Used);
        // Another button of the same drawing still works once.
        let other = WidgetAction::new("todo", "toggle", "2");
        let t2 = token(&links, &other);
        assert_eq!(links.check(&other, Some(&t2), at(3)), LinkCheck::Run);
        assert_eq!(links.check(&other, Some(&t2), at(4)), LinkCheck::Used);
        // The next drawing gives the button a fresh link.
        links.begin_drawing(at(5));
        let fresh = token(&links, &toggle());
        assert_ne!(fresh, t);
        assert_eq!(links.check(&toggle(), Some(&fresh), at(6)), LinkCheck::Run);
        assert_eq!(links.check(&toggle(), Some(&t), at(7)), LinkCheck::Used);
    }

    #[test]
    fn a_link_expires_an_hour_after_its_drawing_was_replaced() {
        let mut links = WidgetLinks::new(KEY, Box::new(Shared::default()));
        links.begin_drawing(at(0));
        let a = WidgetAction::new("todo", "toggle", "1");
        let b = WidgetAction::new("todo", "toggle", "2");
        let (ta, tb) = (token(&links, &a), token(&links, &b));
        links.begin_drawing(at(100));
        let grace = GRACE.as_secs();
        assert_eq!(
            links.check(&a, Some(&ta), at(100 + grace - 1)),
            LinkCheck::Run
        );
        assert_eq!(
            links.check(&b, Some(&tb), at(100 + grace)),
            LinkCheck::Expired
        );
        assert_eq!(
            links.check(&a, Some(&ta), at(100 + grace)),
            LinkCheck::Expired
        );
    }

    #[test]
    fn the_current_drawing_never_expires() {
        // A widget keeps its drawing until the app reloads it: the first
        // tap, a month later, launches the app and runs.
        let store = Shared::default();
        let mut links = WidgetLinks::new(KEY, Box::new(store.clone()));
        links.begin_drawing(at(0));
        let t = token(&links, &toggle());
        let month = 30 * 24 * 3600;
        let mut launched = WidgetLinks::new(KEY, Box::new(store));
        // The app draws its widgets as it starts, then gets the link.
        launched.begin_drawing(at(month));
        assert_eq!(
            launched.check(&toggle(), Some(&t), at(month + 1)),
            LinkCheck::Run
        );
        assert_eq!(
            launched.check(&toggle(), Some(&t), at(month + 2)),
            LinkCheck::Used
        );
    }

    #[test]
    fn forged_tokens_are_refused() {
        let mut links = WidgetLinks::new(KEY, Box::new(Shared::default()));
        links.begin_drawing(at(0));
        let t = token(&links, &toggle());
        let other_drawing = format!(
            "{:016x}{}",
            u64::from_str_radix(&t[..16], 16).unwrap() + 1,
            &t[16..]
        );
        let mut other_key = WidgetLinks::new([8; 32], Box::new(Shared::default()));
        other_key.begin_drawing(at(0));
        for bad in [
            None,
            Some(String::new()),
            Some(t[..47].to_string()),
            Some(format!("{t}0")),
            Some(format!("{}x", &t[..47])),
            Some(format!(
                "{}{}",
                &t[..47],
                if t.ends_with('0') { '1' } else { '0' }
            )),
            Some(other_drawing),
            Some(token(&other_key, &toggle())),
        ] {
            assert_eq!(
                links.check(&toggle(), bad.as_deref(), at(1)),
                LinkCheck::Forged,
                "{bad:?}"
            );
        }
        let wrong = WidgetAction::new("todo", "toggle", "01");
        assert_eq!(links.check(&wrong, Some(&t), at(1)), LinkCheck::Forged);
        // None of these used the link.
        assert_eq!(links.check(&toggle(), Some(&t), at(2)), LinkCheck::Run);
    }

    #[test]
    fn two_instances_cannot_both_run_a_link() {
        let store = Shared::default();
        let mut first = WidgetLinks::new(KEY, Box::new(store.clone()));
        first.begin_drawing(at(0));
        let t = token(&first, &toggle());
        // A second instance read the ledger before the tap.
        let mut second = WidgetLinks::new(KEY, Box::new(store.clone()));
        assert_eq!(first.check(&toggle(), Some(&t), at(1)), LinkCheck::Run);
        assert_eq!(second.check(&toggle(), Some(&t), at(2)), LinkCheck::Used);
        // And a third, started later, after both wrote the ledger.
        second.begin_drawing(at(3));
        let mut third = WidgetLinks::new(KEY, Box::new(store));
        assert_eq!(third.check(&toggle(), Some(&t), at(4)), LinkCheck::Used);
    }

    #[test]
    fn a_burst_of_drawings_keeps_the_newest() {
        let mut links = WidgetLinks::new(KEY, Box::new(Shared::default()));
        links.begin_drawing(at(0));
        let first = token(&links, &toggle());
        for i in 0..MAX_DRAWINGS as u64 {
            links.begin_drawing(at(1 + i));
        }
        assert_eq!(links.ledger.drawings.len(), MAX_DRAWINGS);
        assert_eq!(
            links.check(&toggle(), Some(&first), at(40)),
            LinkCheck::Expired
        );
        let newest = token(&links, &toggle());
        assert_eq!(
            links.check(&toggle(), Some(&newest), at(40)),
            LinkCheck::Run
        );
        // Drawings within one nanosecond still get their own ids.
        let mut fast = WidgetLinks::new(KEY, Box::new(Shared::default()));
        fast.begin_drawing(at(0));
        fast.begin_drawing(at(0));
        let ids: Vec<u64> = fast.ledger.drawings.iter().map(|d| d.id).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids[0] < ids[1]);
    }

    #[test]
    fn the_ledger_round_trips_and_a_damaged_one_refuses_old_links() {
        let store = Shared::default();
        let mut links = WidgetLinks::new(KEY, Box::new(store.clone()));
        links.begin_drawing(at(0));
        let t = token(&links, &toggle());
        links.begin_drawing(at(1));
        assert_eq!(links.check(&toggle(), Some(&t), at(2)), LinkCheck::Run);
        let json = store.get(LEDGER).unwrap();
        assert_eq!(Ledger::parse(&json), links.ledger);
        assert_eq!(Ledger::parse(&Ledger::parse(&json).to_json()), links.ledger);
        for damaged in [
            "",
            "{",
            "[]",
            r#"{"v":2,"drawings":[],"used":[]}"#,
            r#"{"v":1}"#,
        ] {
            assert_eq!(Ledger::parse(damaged), Ledger::default(), "{damaged}");
        }
        // A ledger lost or damaged between runs: the old drawings are gone.
        let mut store = store;
        store.set(LEDGER, "garbage");
        let mut restarted = WidgetLinks::new(KEY, Box::new(store));
        let other = WidgetAction::new("todo", "toggle", "2");
        let old = token(&links, &other);
        assert_eq!(
            restarted.check(&other, Some(&old), at(3)),
            LinkCheck::Expired
        );
    }
}
