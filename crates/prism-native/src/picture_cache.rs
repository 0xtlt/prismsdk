//! Decoded pictures kept for the nodes that show them (#343), shared by the
//! backends that decode pictures themselves (macOS, iOS, Windows).
//!
//! - A decoded image stays while at least one live node shows its source.
//! - Once no node shows it, it waits in a small least-recently-used pool
//!   bounded by bytes ([`UNSHOWN_BYTES`]): going back to a screen shows its
//!   pictures again without decoding them, and an app that shows a new
//!   `picture_data` after each gesture keeps a few images, not all of them.
//! - A node may also hold several sources at once ([`hold`]: a drawing's
//!   images), each counted as shown while it does: a drawing whose images
//!   are larger than the pool is not decoded again at every paint.
//! - Each node's current source is known, so a load that finishes after
//!   the node moved on to another source never reaches it ([`loaded`]
//!   names only the nodes still showing the source).
//!
//! The cache only keeps the books: backends load and decode, and keep
//! showing a node's previous image until its new one is ready ([`Show`]).
//!
//! [`loaded`]: PictureCache::loaded
//! [`hold`]: PictureCache::hold

use std::collections::{HashMap, HashSet};

use prism_core::NodeId;

/// Bytes of decoded images no node shows that are kept for when a node
/// shows them again (64 MB: three 2880×1800 images, hundreds of
/// thumbnails). Beyond it the least recently used go first. Images a node
/// shows are never counted nor evicted.
pub const UNSHOWN_BYTES: usize = 64 << 20;

/// The bytes a decoded image takes: `width × height` pixels of 4 bytes.
pub fn decoded_bytes(width: f64, height: f64) -> usize {
    (width.max(0.) * height.max(0.) * 4.) as usize
}

/// What a backend does when a node shows a source ([`PictureCache::show`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Show<I> {
    /// Decoded already: show it now.
    Ready(I),
    /// Not decoded and not loading: start loading it, then call
    /// [`PictureCache::loaded`]. The view keeps its previous image until
    /// then.
    Load,
    /// A load of this source runs already: wait for it, keeping the
    /// previous image.
    Wait,
    /// No source (an empty one): show nothing, now.
    Clear,
}

struct Entry<I> {
    image: I,
    /// Decoded bytes, plus the source's own length (a `data:` URL is as
    /// big as its encoded image).
    cost: usize,
    /// Live nodes showing it.
    viewers: usize,
    /// When it was last shown, released or stored (a tick of `clock`).
    used: u64,
}

/// Decoded images by source, the nodes that show them, and the loads
/// running. `I` is the platform's image handle (cheap to clone).
pub struct PictureCache<I> {
    entries: HashMap<String, Entry<I>>,
    /// Each node's current source.
    shown: HashMap<NodeId, String>,
    /// The sources nodes hold while they show several (a drawing's
    /// images), each counted as a viewer.
    held: HashMap<NodeId, Vec<String>>,
    loading: HashSet<String>,
    /// Bytes of the entries no node shows.
    unshown: usize,
    budget: usize,
    clock: u64,
}

impl<I: Clone> Default for PictureCache<I> {
    fn default() -> Self {
        PictureCache::new(UNSHOWN_BYTES)
    }
}

impl<I: Clone> PictureCache<I> {
    /// A cache keeping at most `budget` bytes of images no node shows.
    pub fn new(budget: usize) -> Self {
        PictureCache {
            entries: HashMap::new(),
            shown: HashMap::new(),
            held: HashMap::new(),
            loading: HashSet::new(),
            unshown: 0,
            budget,
            clock: 0,
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Node `node` shows `src` now (its previous source, if any, is
    /// released). Says what the view does: show a decoded image, start a
    /// load, wait for one, or clear itself for an empty source.
    pub fn show(&mut self, node: NodeId, src: &str) -> Show<I> {
        if src.is_empty() {
            self.forget(node);
            return Show::Clear;
        }
        let previous = self.shown.insert(node, src.to_string());
        let now = self.tick();
        let show = match self.entries.get_mut(src) {
            Some(e) => {
                if e.viewers == 0 {
                    self.unshown -= e.cost;
                }
                e.viewers += 1;
                e.used = now;
                Show::Ready(e.image.clone())
            }
            None if self.loading.contains(src) => Show::Wait,
            None => {
                self.loading.insert(src.to_string());
                Show::Load
            }
        };
        if let Some(previous) = previous {
            self.release(&previous);
        }
        show
    }

    /// Node `node` is gone (or shows no picture any more): its source, and
    /// the ones it held, are released.
    pub fn forget(&mut self, node: NodeId) {
        if let Some(src) = self.shown.remove(&node) {
            self.release(&src);
        }
        for src in self.held.remove(&node).into_iter().flatten() {
            self.release(&src);
        }
    }

    /// Node `node` shows all of `sources` now (a drawing's images), instead
    /// of what it held before: they count as shown, so they are never
    /// evicted while it does, however large. Nothing is loaded here: the
    /// backend decodes them as it draws ([`insert`](Self::insert), or
    /// [`start_load`](Self::start_load) then [`loaded`](Self::loaded)).
    /// No sources releases what it held.
    pub fn hold<'a>(&mut self, node: NodeId, sources: impl IntoIterator<Item = &'a str>) {
        let mut sources: Vec<String> = sources
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        sources.sort();
        sources.dedup();
        let now = self.tick();
        for src in &sources {
            if let Some(e) = self.entries.get_mut(src) {
                if e.viewers == 0 {
                    self.unshown -= e.cost;
                }
                e.viewers += 1;
                e.used = now;
            }
        }
        let previous = if sources.is_empty() {
            self.held.remove(&node)
        } else {
            self.held.insert(node, sources)
        };
        for src in previous.into_iter().flatten() {
            self.release(&src);
        }
    }

    /// How many nodes show or hold `src`.
    fn viewers(&self, src: &str) -> usize {
        self.shown.values().filter(|s| s.as_str() == src).count()
            + self
                .held
                .values()
                .filter(|h| h.iter().any(|s| s == src))
                .count()
    }

    fn release(&mut self, src: &str) {
        let now = self.tick();
        if let Some(e) = self.entries.get_mut(src) {
            e.viewers -= 1;
            if e.viewers == 0 {
                e.used = now;
                self.unshown += e.cost;
            }
        }
        self.evict();
    }

    /// A load of `src` finished: `image` and its decoded bytes
    /// ([`decoded_bytes`]), or `None` when it could not be loaded or
    /// decoded (nothing is stored: showing the source again retries).
    /// Returns the nodes showing `src` now, which show the image (or
    /// nothing, on a failure); nodes that moved on to another source are
    /// not among them.
    pub fn loaded(&mut self, src: &str, image: Option<(I, usize)>) -> Vec<NodeId> {
        self.loading.remove(src);
        let nodes: Vec<NodeId> = self
            .shown
            .iter()
            .filter(|(_, s)| s.as_str() == src)
            .map(|(n, _)| *n)
            .collect();
        if let Some((image, bytes)) = image {
            let viewers = self.viewers(src);
            self.store(src, image, bytes, viewers);
        }
        nodes
    }

    /// Stores `image` for `src` without a load ([`Show::Load`]) first: an
    /// image decoded for something other than a node (a drawing's image).
    pub fn insert(&mut self, src: &str, image: I, bytes: usize) {
        self.loading.remove(src);
        let viewers = self.viewers(src);
        self.store(src, image, bytes, viewers);
    }

    fn store(&mut self, src: &str, image: I, bytes: usize, viewers: usize) {
        let used = self.tick();
        let entry = Entry {
            image,
            cost: bytes + src.len(),
            viewers,
            used,
        };
        if viewers == 0 {
            self.unshown += entry.cost;
        }
        if let Some(old) = self.entries.insert(src.to_string(), entry)
            && old.viewers == 0
        {
            self.unshown -= old.cost;
        }
        self.evict();
    }

    /// The least recently used images no node shows go until the rest fit
    /// the budget.
    fn evict(&mut self) {
        while self.unshown > self.budget {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, e)| e.viewers == 0)
                .min_by_key(|(_, e)| e.used)
                .map(|(src, _)| src.clone());
            let Some(src) = oldest else { break };
            if let Some(e) = self.entries.remove(&src) {
                self.unshown -= e.cost;
            }
        }
    }

    /// The decoded image of `src`, if kept.
    pub fn get(&self, src: &str) -> Option<&I> {
        self.entries.get(src).map(|e| &e.image)
    }

    /// The decoded image of `src`, if kept, made the most recently used:
    /// for images drawn without a node (a drawing's), the last to go.
    pub fn touch(&mut self, src: &str) -> Option<&I> {
        self.clock += 1;
        let e = self.entries.get_mut(src)?;
        e.used = self.clock;
        Some(&e.image)
    }

    /// Whether a load of `src` runs.
    pub fn is_loading(&self, src: &str) -> bool {
        self.loading.contains(src)
    }

    /// Whether `src` is decoded or loading.
    pub fn knows(&self, src: &str) -> bool {
        self.entries.contains_key(src) || self.loading.contains(src)
    }

    /// Marks `src` as loading, for a load started without a node (a
    /// drawing's image); `false` if it is decoded or loading already.
    pub fn start_load(&mut self, src: &str) -> bool {
        if self.knows(src) {
            return false;
        }
        self.loading.insert(src.to_string())
    }

    /// The source node `node` shows now.
    pub fn source(&self, node: NodeId) -> Option<&str> {
        self.shown.get(&node).map(String::as_str)
    }

    /// Bytes of the images no node shows (at most the budget).
    pub fn unshown_bytes(&self) -> usize {
        self.unshown
    }

    /// How many images are kept, shown or not.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no image is kept.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: usize = 1 << 20;

    fn node(n: u64) -> NodeId {
        n
    }

    /// Shows `src` in `n`, loading it (with `bytes`) when asked to.
    fn show_loaded(c: &mut PictureCache<&'static str>, n: u64, src: &'static str, bytes: usize) {
        if c.show(node(n), src) == Show::Load {
            c.loaded(src, Some((src, bytes)));
        }
    }

    #[test]
    fn a_drawing_keeps_all_its_images_while_it_holds_them() {
        // Images larger than the pool, drawn by one node.
        let mut c = PictureCache::new(10 * MB);
        c.hold(node(1), ["a", "b", "c", "b"]);
        for src in ["a", "b", "c"] {
            c.insert(src, src, 20 * MB);
        }
        // All kept, though they are 60 MB with a 10 MB pool: none is
        // decoded again at the next paint.
        for src in ["a", "b", "c"] {
            assert!(c.touch(src).is_some(), "{src}");
        }
        assert_eq!(c.unshown_bytes(), 0);
        // Another picture coming and going does not evict them.
        show_loaded(&mut c, 2, "d", 20 * MB);
        c.forget(node(2));
        assert!(c.get("d").is_none(), "d does not fit the pool");
        assert_eq!(c.len(), 3);
        // Decoded before the hold: counted as shown from then on.
        c.insert("e", "e", 20 * MB);
        assert!(c.get("e").is_none());
        c.insert("e", "e", MB);
        c.hold(node(1), ["a", "e"]);
        assert_eq!(c.unshown_bytes(), 0, "b and c went, e is held");
        assert!(c.get("b").is_none() && c.get("c").is_none());
        // The drawing goes: its images join the pool, which keeps what fits.
        c.forget(node(1));
        assert!(c.get("a").is_none());
        assert_eq!(c.get("e"), Some(&"e"));
        assert!(c.unshown_bytes() <= 10 * MB);
        // A node holding nothing more releases everything.
        c.hold(node(3), ["e"]);
        assert_eq!(c.unshown_bytes(), 0);
        c.hold(node(3), []);
        assert_eq!(c.unshown_bytes(), MB + 1);
    }

    #[test]
    fn a_shown_image_stays_whatever_the_budget() {
        let mut c = PictureCache::new(0);
        assert_eq!(c.show(node(1), "a"), Show::Load);
        assert_eq!(c.loaded("a", Some(("A", 20 * MB))), vec![node(1)]);
        // Shown: kept, though the budget is 0.
        assert_eq!(c.get("a"), Some(&"A"));
        assert_eq!(c.unshown_bytes(), 0);
        // Another node shows it from memory.
        assert_eq!(c.show(node(2), "a"), Show::Ready("A"));
        c.forget(node(1));
        assert_eq!(c.get("a"), Some(&"A"), "node 2 still shows it");
        c.forget(node(2));
        assert_eq!(c.get("a"), None, "no node shows it and it does not fit");
        assert!(c.is_empty());
    }

    #[test]
    fn changing_a_source_releases_the_previous_one() {
        let mut c = PictureCache::new(30 * MB);
        // A new 20 MB image after each gesture, in the same node.
        for (i, src) in ["g1", "g2", "g3", "g4", "g5"].into_iter().enumerate() {
            show_loaded(&mut c, 1, src, 20 * MB);
            assert_eq!(c.source(node(1)), Some(src));
            assert!(c.unshown_bytes() <= 30 * MB, "after {i}");
        }
        // The shown one, and the previous one (it fits the budget).
        assert_eq!(c.len(), 2);
        assert!(c.get("g5").is_some() && c.get("g4").is_some());
    }

    #[test]
    fn removing_a_node_releases_its_source() {
        let mut c = PictureCache::new(MB);
        show_loaded(&mut c, 1, "big", 2 * MB);
        show_loaded(&mut c, 2, "small", 1000);
        c.forget(node(1));
        assert_eq!(c.get("big"), None, "over the budget once unshown");
        c.forget(node(2));
        assert!(c.get("small").is_some(), "fits the budget");
        // Forgetting twice, or a node that showed nothing, is harmless.
        c.forget(node(2));
        c.forget(node(9));
        assert_eq!(c.unshown_bytes(), 1000 + "small".len());
    }

    #[test]
    fn unshown_images_go_least_recently_used_first() {
        let mut c = PictureCache::new(3 * MB);
        for (n, src) in [(1, "a"), (2, "b"), (3, "c")] {
            show_loaded(&mut c, n, src, MB - 10);
        }
        // Released in the order b, a, c.
        c.forget(node(2));
        c.forget(node(1));
        c.forget(node(3));
        assert_eq!(c.len(), 3);
        // Going back to `b` shows it from memory and makes it recent.
        assert_eq!(c.show(node(4), "b"), Show::Ready("b"));
        c.forget(node(4));
        // A fourth image: `a` (the least recently used) goes.
        show_loaded(&mut c, 5, "d", MB - 10);
        c.forget(node(5));
        assert_eq!(c.get("a"), None);
        assert!(c.get("b").is_some() && c.get("c").is_some() && c.get("d").is_some());
        assert!(c.unshown_bytes() <= 3 * MB);
    }

    #[test]
    fn the_source_counts_in_the_bytes() {
        // A `data:` URL is as big as its encoded image.
        let src: &'static str = Box::leak("x".repeat(2000).into_boxed_str());
        let mut c = PictureCache::new(1500);
        show_loaded(&mut c, 1, src, 0);
        c.forget(node(1));
        assert!(c.is_empty());
    }

    #[test]
    fn one_load_per_source() {
        let mut c = PictureCache::new(MB);
        assert_eq!(c.show(node(1), "a"), Show::Load);
        assert_eq!(c.show(node(2), "a"), Show::Wait);
        assert!(c.is_loading("a"));
        let mut nodes = c.loaded("a", Some(("A", 10)));
        nodes.sort();
        assert_eq!(nodes, vec![node(1), node(2)]);
        assert!(!c.is_loading("a"));
    }

    #[test]
    fn a_stale_load_never_reaches_a_node_that_moved_on() {
        let mut c = PictureCache::new(100 * MB);
        // A → B → C; C finishes first, then A, then B.
        assert_eq!(c.show(node(1), "A"), Show::Load);
        assert_eq!(c.show(node(1), "B"), Show::Load);
        assert_eq!(c.show(node(1), "C"), Show::Load);
        assert_eq!(c.loaded("C", Some(("C", 10))), vec![node(1)]);
        assert_eq!(c.loaded("A", Some(("A", 10))), Vec::<NodeId>::new());
        assert_eq!(c.loaded("B", Some(("B", 10))), Vec::<NodeId>::new());
        assert_eq!(c.source(node(1)), Some("C"));
        // The late images wait unshown (going back is instant).
        assert_eq!(c.show(node(1), "B"), Show::Ready("B"));
    }

    #[test]
    fn a_load_pending_when_the_node_shows_a_cached_source_is_stale() {
        let mut c = PictureCache::new(100 * MB);
        show_loaded(&mut c, 2, "cached", 10);
        assert_eq!(c.show(node(1), "slow"), Show::Load);
        assert_eq!(c.show(node(1), "cached"), Show::Ready("cached"));
        assert_eq!(c.loaded("slow", Some(("slow", 10))), Vec::<NodeId>::new());
    }

    #[test]
    fn a_failed_load_names_its_nodes_and_is_retried() {
        let mut c: PictureCache<&str> = PictureCache::new(MB);
        assert_eq!(c.show(node(1), "broken"), Show::Load);
        assert_eq!(c.loaded("broken", None), vec![node(1)]);
        assert!(c.is_empty());
        assert_eq!(c.show(node(1), "broken"), Show::Load);
    }

    #[test]
    fn an_empty_source_clears_and_releases() {
        let mut c = PictureCache::new(0);
        show_loaded(&mut c, 1, "a", 10);
        assert_eq!(c.show(node(1), ""), Show::Clear);
        assert_eq!(c.source(node(1)), None);
        assert!(c.is_empty());
    }

    #[test]
    fn showing_the_same_source_again_keeps_it() {
        let mut c = PictureCache::new(0);
        show_loaded(&mut c, 1, "a", 10);
        assert_eq!(c.show(node(1), "a"), Show::Ready("a"));
        assert_eq!(c.get("a"), Some(&"a"));
        assert_eq!(c.unshown_bytes(), 0);
    }

    #[test]
    fn images_stored_without_a_node_are_unshown() {
        let mut c = PictureCache::new(MB);
        assert!(c.start_load("drawing"));
        assert!(!c.start_load("drawing"), "loading already");
        c.insert("drawing", "D", 100);
        assert!(!c.start_load("drawing"), "decoded already");
        assert_eq!(c.unshown_bytes(), 100 + "drawing".len());
        // A node showing it pins it.
        assert_eq!(c.show(node(1), "drawing"), Show::Ready("D"));
        assert_eq!(c.unshown_bytes(), 0);
        // Stored again (decoded again) while shown: still pinned.
        c.insert("drawing", "D2", 200);
        assert_eq!(c.unshown_bytes(), 0);
        c.forget(node(1));
        assert_eq!(c.unshown_bytes(), 200 + "drawing".len());
    }

    #[test]
    fn touched_images_go_last() {
        let mut c = PictureCache::new(2 * MB);
        c.insert("a", "A", MB - 100);
        c.insert("b", "B", MB - 100);
        assert_eq!(c.touch("a"), Some(&"A"));
        assert_eq!(c.touch("missing"), None);
        c.insert("c", "C", MB - 100);
        assert_eq!(c.get("b"), None, "the least recently used");
        assert!(c.get("a").is_some() && c.get("c").is_some());
    }

    #[test]
    fn decoded_bytes_are_four_per_pixel() {
        assert_eq!(decoded_bytes(2880., 1800.), 2880 * 1800 * 4);
        assert_eq!(decoded_bytes(-1., 10.), 0);
    }
}
