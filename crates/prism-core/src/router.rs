//! Navigation: a URL-style router rendered as a native stack.
//!
//! ```ignore
//! Router::new()
//!     .route("/", |cx, _| cx.new_entity(|_| Home::default()))
//!     .route("/todos/:id", |cx, p| cx.new_entity(|_| Detail::new(p.parse("id").unwrap_or(0))))
//!     .build(cx)
//! ```
//!
//! Anywhere in the app: `cx.navigate("/todos/2")`, `cx.back()`, `cx.replace(..)`,
//! `cx.reset(..)`. Each route's view should return a [`screen`](crate::screen)
//! (its title goes in the native navigation bar).
//!
//! Paths keep their query string (`/analytics?period=7d&room=3`): a route
//! reads it with [`Params::query`], a screen rewrites it in place with
//! `cx.replace_query(..)`, and `cx.navigate_with(path, value)` hands a typed
//! value to the next screen ([`Params::state`]).
//!
//! - iOS: `UINavigationController` (push animation, swipe back, large titles).
//! - macOS: a native header bar with a Back button, plus ⌘[.
//! - Web: real URLs via the History API; server-side rendering of deep links.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::str::FromStr;

use crate::app::{AnyEntity, App, Context, Entity, Render};
use crate::element::{
    BackEvent, ChangeEvent, ClickEvent, Element, Handler, IntoElement, NavigateEvent,
    ParentElement, div, modal, screen, stack, tab, tabs, text,
};
use crate::style::Styled;

/// What a route is opened with: the path parameters captured by its pattern
/// (`/todos/:id`, `/files/*rest`), the query string (`?period=7d`), and the
/// value passed with [`App::navigate_with`], if any.
#[derive(Clone, Default)]
pub struct Params {
    values: Vec<(String, String)>,
    /// Decoded `name=value` pairs, in order (a name may repeat).
    query: Vec<(String, String)>,
    state: Option<Rc<dyn Any>>,
}

impl std::fmt::Debug for Params {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Params")
            .field("values", &self.values)
            .field("query", &self.query)
            .field("state", &self.state.is_some())
            .finish()
    }
}

impl Params {
    /// The value captured for `name` (`id` in `/todos/:id`).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// `p.parse::<u64>("id")`
    pub fn parse<T: FromStr>(&self, name: &str) -> Option<T> {
        self.get(name)?.parse().ok()
    }

    /// The first value of query parameter `name`, decoded (`7d` for
    /// `?period=7d`; `""` for `?edit`).
    pub fn query(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Every value of query parameter `name`, in order (`?room=1&room=3`).
    pub fn query_all(&self, name: &str) -> Vec<&str> {
        self.query
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// `p.parse_query::<u32>("room")`: the first value, parsed.
    pub fn parse_query<T: FromStr>(&self, name: &str) -> Option<T> {
        self.query(name)?.parse().ok()
    }

    /// Every query parameter, decoded, in order.
    pub fn query_pairs(&self) -> &[(String, String)] {
        &self.query
    }

    /// The value handed over with [`App::navigate_with`] (or
    /// [`App::replace_with`]), if it is a `T`. Only the screen built by that
    /// navigation gets it: a reload or a deep link opens without it.
    pub fn state<T: 'static>(&self) -> Option<&T> {
        self.state.as_deref()?.downcast_ref()
    }
}

/// Splits `/a/b?x=1` into `("/a/b", "x=1")`.
pub(crate) fn split_query(path: &str) -> (&str, &str) {
    path.split_once('?').unwrap_or((path, ""))
}

/// `+` and `%XX` escapes decoded (invalid escapes are kept as they are).
fn decode_component(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = |b: u8| (b as char).to_digit(16);
                match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                        continue;
                    }
                    _ => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Decodes a query string (`period=7d&room=3&room=4&edit`) into pairs.
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (decode_component(k), decode_component(v))
        })
        .collect()
}

/// Encodes pairs as a query string (no leading `?`), escaping what must be:
/// `encode_query([("q", "a&b c")])` → `q=a%26b%20c`. Build links with it:
/// `format!("/analytics?{}", encode_query([("period", "7d")]))`.
pub fn encode_query<K: AsRef<str>, V: AsRef<str>>(
    pairs: impl IntoIterator<Item = (K, V)>,
) -> String {
    fn escape(s: &str, out: &mut String) {
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() || b"-._~!$'()*,;:@/?".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    let mut out = String::new();
    for (k, v) in pairs {
        if !out.is_empty() {
            out.push('&');
        }
        escape(k.as_ref(), &mut out);
        out.push('=');
        escape(v.as_ref(), &mut out);
    }
    out
}

type Build = Rc<dyn Fn(&mut App, &Params) -> AnyEntity>;

enum Segment {
    Static(String),
    Param(String),
    Rest(String),
}

struct Route {
    /// The pattern as declared, for the debug builds' warnings.
    #[cfg(debug_assertions)]
    pattern: String,
    segments: Vec<Segment>,
    build: Build,
}

impl Route {
    /// How specific the route is at each position: a static segment (0)
    /// beats a parameter (1), which beats the end of the pattern (2), which
    /// beats a `*rest` (3). Comparing two routes' keys orders them most
    /// specific first: `/sites/new` before `/sites/:site`, `/files` before
    /// `/files/*rest`. Two patterns of different lengths without a `*rest`
    /// never match the same path, so the end only matters against a rest.
    fn specificity(&self) -> Vec<u8> {
        let mut key: Vec<u8> = self
            .segments
            .iter()
            .map(|seg| match seg {
                Segment::Static(_) => 0,
                Segment::Param(_) => 1,
                Segment::Rest(_) => 3,
            })
            .collect();
        if !matches!(self.segments.last(), Some(Segment::Rest(_))) {
            key.push(2);
        }
        key
    }

    /// Whether every path this route matches is matched by `earlier` too
    /// (same shape: the same static segments, parameters and rest at the
    /// same places), so `earlier`, ranked first, always wins.
    #[cfg(debug_assertions)]
    fn shadowed_by(&self, earlier: &Route) -> bool {
        self.segments.len() == earlier.segments.len()
            && self
                .segments
                .iter()
                .zip(&earlier.segments)
                .all(|(a, b)| match (a, b) {
                    (Segment::Static(a), Segment::Static(b)) => a == b,
                    (Segment::Param(_), Segment::Param(_))
                    | (Segment::Rest(_), Segment::Rest(_)) => true,
                    _ => false,
                })
    }

    fn matches(&self, path: &str) -> Option<Params> {
        let (path, query) = split_query(path);
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut params = Params {
            query: parse_query(query),
            ..Params::default()
        };
        for (i, seg) in self.segments.iter().enumerate() {
            match seg {
                Segment::Rest(name) => {
                    params
                        .values
                        .push((name.clone(), parts[i.min(parts.len())..].join("/")));
                    return Some(params);
                }
                Segment::Static(s) => {
                    if parts.get(i) != Some(&s.as_str()) {
                        return None;
                    }
                }
                Segment::Param(name) => params.values.push((name.clone(), (*parts.get(i)?).into())),
            }
        }
        (parts.len() == self.segments.len()).then_some(params)
    }
}

/// `/a/b/?x=1#y` → `/a/b?x=1`: always starts with `/`, no trailing `/`,
/// the query string kept as it is (an empty one dropped), the fragment
/// dropped.
pub fn normalize_path(path: &str) -> String {
    let path = path.split('#').next().unwrap_or("");
    let (path, query) = split_query(path);
    let trimmed = path.trim_matches('/');
    if query.is_empty() {
        format!("/{trimmed}")
    } else {
        format!("/{trimmed}?{query}")
    }
}

pub(crate) enum NavOp {
    Push(String),
    /// A link opened from outside while the app runs: like a deep link at
    /// launch, it opens in the tab its path belongs to, `/` included.
    Link(String),
    Replace(String),
    Reset(String),
    Back,
    Present(String),
    Dismiss,
    /// The visible screen's query string becomes this (no new screen).
    ReplaceQuery(String),
    /// Handed to the screen the next operation builds.
    State(Rc<dyn Any>),
}

struct Entry {
    path: String,
    /// `None`: a screen under a deep link on the web, not built yet. It is
    /// built when Back reaches it ([`Router::build_top`]).
    entity: Option<AnyEntity>,
}

/// A tab's title: fixed, or computed at each render (translations).
#[derive(Clone)]
enum TabTitle {
    Fixed(String),
    Dynamic(Rc<dyn Fn(&App) -> String>),
}

impl TabTitle {
    fn resolve(&self, app: &App) -> String {
        match self {
            TabTitle::Fixed(s) => s.clone(),
            TabTitle::Dynamic(f) => f(app),
        }
    }
}

/// A tab and its own navigation stack.
struct TabState {
    title: TabTitle,
    icon: String,
    root: String,
    entries: Vec<Entry>,
    // --- Lazy tabs ---
    /// Rendered from the start ([`RouterBuilder::eager`]), not on first
    /// selection.
    eager: bool,
    /// Whether the tab's screens are in the tree: it was selected since the
    /// last reset (it then stays, state and nodes kept), or it is eager.
    shown: bool,
}

/// The navigation state: one stack of screens per tab (a single implicit tab
/// when no tab bar is declared). Screens are entities built from routes.
pub struct Router {
    routes: Vec<Route>,
    not_found: Option<Build>,
    tabs: Vec<TabState>,
    selected: usize,
    tab_bar: bool,
    /// Screens presented modally (a sheet with its own stack), if any.
    modal: Vec<Entry>,
    /// Whether stacks show their header bar (`RouterBuilder::header`).
    header: bool,
    /// The value for the next screen built (`navigate_with`).
    next_state: RefCell<Option<Rc<dyn Any>>>,
    // --- Closing the router (#31) ---
    /// Screens a route returned but did not create (`.route("/focus", move
    /// |_, _| focus)`): shared with the app, never released by the router.
    shared: RefCell<std::collections::HashSet<crate::app::EntityId>>,
    // --- Bar options (#30) ---
    /// Tint of the tab bar and the header bars (`RouterBuilder::tint`).
    tint: Option<crate::style::Color>,
}

/// Declares the routes (and optionally tabs), then [`build`](RouterBuilder::build)s the router.
#[derive(Default)]
pub struct RouterBuilder {
    routes: Vec<Route>,
    not_found: Option<Build>,
    tabs: Vec<(TabTitle, String, String)>,
    /// `Some(false)`: no header bar ([`RouterBuilder::header`]).
    header: Option<bool>,
    // --- Bar options (#30) ---
    tint: Option<crate::style::Color>,
    // --- Lazy tabs ---
    /// Indices of the tabs declared [`eager`](RouterBuilder::eager).
    eager: Vec<usize>,
}

impl Router {
    #[allow(clippy::new_ret_no_self)]
    /// Starts declaring routes.
    pub fn new() -> RouterBuilder {
        RouterBuilder::default()
    }

    fn entries(&self) -> &Vec<Entry> {
        &self.tabs[self.selected].entries
    }

    /// Path of the visible screen.
    pub fn current_path(&self) -> &str {
        self.entries().last().map_or("/", |e| e.path.as_str())
    }

    /// Number of screens in the visible stack.
    pub fn depth(&self) -> usize {
        self.entries().len()
    }

    /// Index of the selected tab.
    pub fn selected_tab(&self) -> usize {
        self.selected
    }

    /// Whether tab `index` has its screens in the tree: tabs render on
    /// their first selection (then stay), or up front when declared
    /// [`eager`](RouterBuilder::eager). An unrendered tab shows only its
    /// tab-bar entry.
    pub fn tab_rendered(&self, index: usize) -> bool {
        self.tabs
            .get(index)
            .is_some_and(|t| t.shown || index == self.selected)
    }

    pub(crate) fn apply(&mut self, op: NavOp, cx: &mut Context<Self>) {
        if let NavOp::State(state) = op {
            *self.next_state.borrow_mut() = Some(state);
            return;
        }
        self.apply_op(op, cx);
        // Only the screen this operation built gets the value.
        self.next_state.borrow_mut().take();
    }

    /// Sets the query string of the visible screen (of the sheet, if one
    /// is up), keeping the screen.
    fn replace_query(&mut self, query: &str) {
        let entries = if self.modal.is_empty() {
            &mut self.tabs[self.selected].entries
        } else {
            &mut self.modal
        };
        if let Some(top) = entries.last_mut() {
            let (path, _) = split_query(&top.path);
            top.path = if query.is_empty() {
                path.to_string()
            } else {
                format!("{path}?{query}")
            };
        }
    }

    fn apply_op(&mut self, op: NavOp, cx: &mut Context<Self>) {
        match op {
            NavOp::State(_) => {}
            NavOp::ReplaceQuery(query) => self.replace_query(&query),
            NavOp::Present(path) => {
                let path = normalize_path(&path);
                self.dismiss(cx);
                let entity = self.open(&path, cx);
                self.modal.push(Entry {
                    path,
                    entity: Some(entity),
                });
            }
            NavOp::Dismiss => self.dismiss(cx),
            // While a sheet is up, navigation happens inside it.
            NavOp::Push(path) | NavOp::Link(path) if !self.modal.is_empty() => {
                let path = normalize_path(&path);
                if self.modal.last().is_some_and(|e| e.path == path) {
                    return;
                }
                let entity = self.open(&path, cx);
                self.modal.push(Entry {
                    path,
                    entity: Some(entity),
                });
            }
            NavOp::Back if !self.modal.is_empty() => {
                let depth = self.modal.len() - 1;
                self.truncate_modal(depth, cx);
            }
            NavOp::Push(path) => self.push(&path, false, cx),
            NavOp::Link(path) => self.push(&path, true, cx),
            NavOp::Replace(path) => {
                let path = normalize_path(&path);
                let tab = self.selected;
                let owner = self.owning_tab(&path, false).unwrap_or(tab);
                // A path of another tab opens there, selecting it; this tab
                // keeps at least its root.
                if (owner == tab || self.tabs[tab].entries.len() > 1)
                    && let Some(old) = self.tabs[tab].entries.pop()
                {
                    release_screen(&self.shared, old.entity, cx);
                }
                if owner == tab {
                    let entity = self.open(&path, cx);
                    self.tabs[tab].entries.push(Entry {
                        path,
                        entity: Some(entity),
                    });
                } else {
                    self.push(&path, false, cx);
                }
            }
            NavOp::Reset(path) => self.reset(&path, cx),
            NavOp::Back => {
                let depth = self.depth().saturating_sub(1);
                self.truncate(self.selected, depth, cx)
            }
        }
        cx.notify();
    }

    /// The tab whose root best matches `path` (its query aside): the
    /// longest root that is `path` or one of its parents. A `/` root is
    /// every path's parent only for a `link` (a deep link, at launch or
    /// while the app runs: it has no tab of its own to stay in); within the
    /// app, a path under no other tab's root (a screen several tabs show,
    /// `/items/7`) stays in the selected tab, as on iOS. None without a tab
    /// bar, or when no root matches.
    fn owning_tab(&self, path: &str, link: bool) -> Option<usize> {
        if !self.tab_bar {
            return None;
        }
        let path = split_query(path).0;
        self.tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                path == t.root
                    || (t.root == "/" && link)
                    || (t.root != "/" && path.starts_with(&format!("{}/", t.root)))
            })
            .max_by_key(|(_, t)| t.root.len())
            .map(|(i, _)| i)
    }

    /// Pushes `path` on the tab it belongs to ([`owning_tab`](Self::owning_tab)),
    /// selecting that tab: the tab bar follows the route, whether a deep
    /// `link`, the app or the user changed it. A tab's root only switches
    /// to its tab (keeping the screens pushed there); with a query the tab
    /// doesn't show, it is pushed there too.
    fn push(&mut self, path: &str, link: bool, cx: &mut Context<Self>) {
        let path = normalize_path(path);
        if let Some(i) = self.owning_tab(&path, link) {
            self.selected = i;
            if path == self.tabs[i].root {
                return;
            }
        }
        if self.current_path() == path {
            return;
        }
        let entity = self.open(&path, cx);
        self.tabs[self.selected].entries.push(Entry {
            path,
            entity: Some(entity),
        });
    }

    /// Keeps the first `depth` screens (at least one) of tab `tab`.
    fn truncate(&mut self, tab: usize, depth: usize, cx: &mut Context<Self>) {
        let Some(t) = self.tabs.get_mut(tab) else {
            return;
        };
        while t.entries.len() > depth.max(1) {
            let old = t.entries.pop().expect("non-empty");
            release_screen(&self.shared, old.entity, cx);
        }
        self.build_top(tab, cx);
    }

    /// Builds the top screen of tab `tab` if it is still a pending one (a
    /// screen under a deep link that Back just reached).
    fn build_top(&mut self, tab: usize, cx: &mut Context<Self>) {
        let Some(path) = self.tabs[tab]
            .entries
            .last()
            .filter(|e| e.entity.is_none())
            .map(|e| e.path.clone())
        else {
            return;
        };
        let entity = self.open(&path, cx);
        if let Some(top) = self.tabs[tab].entries.last_mut() {
            top.entity = Some(entity);
        }
    }

    /// Closes the sheet, releasing its screens.
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        for old in self.modal.drain(..) {
            release_screen(&self.shared, old.entity, cx);
        }
    }

    /// Keeps the first `depth` screens of the sheet; 0 closes it.
    fn truncate_modal(&mut self, depth: usize, cx: &mut Context<Self>) {
        while self.modal.len() > depth {
            let old = self.modal.pop().expect("non-empty");
            release_screen(&self.shared, old.entity, cx);
        }
    }

    /// Whether a sheet is presented.
    pub fn is_presenting(&self) -> bool {
        !self.modal.is_empty()
    }

    /// Whether a declared route matches `path` (otherwise it opens on the
    /// not-found screen, the app's own or Prism's).
    pub fn has_route(&self, path: &str) -> bool {
        let path = normalize_path(path);
        self.routes.iter().any(|r| r.matches(&path).is_some())
    }

    /// Whether the visible screen is the not-found screen (no route matches
    /// its path). The web answers such pages with `404 Not Found`.
    pub fn shows_not_found(&self) -> bool {
        let path = match self.modal.last() {
            Some(e) => e.path.as_str(),
            None => self.current_path(),
        };
        !self.has_route(path)
    }

    fn select(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.selected = index;
        }
    }

    /// Lazy tabs: marks the selected tab rendered (it stays so). On watchOS
    /// the tabs are pages the Digital Crown scrolls through, which shows the
    /// next page during the swipe, before the selection changes: the tabs
    /// next to the selected one render too.
    fn show_selected_tab(&mut self, platform: crate::Platform) {
        let near = if platform == crate::Platform::Watchos {
            1
        } else {
            0
        };
        let (from, to) = (self.selected.saturating_sub(near), self.selected + near);
        for t in self.tabs.iter_mut().take(to + 1).skip(from) {
            t.shown = true;
        }
    }

    /// Opens `path` in the tab whose root best matches it. Each tab's root
    /// screen sits underneath, so "back" has somewhere to go. On the web the
    /// one under `path` is not built until Back reaches it: it would be
    /// hidden, yet load its data and ship its HTML with every deep link.
    fn reset(&mut self, path: &str, cx: &mut Context<Self>) {
        let full = normalize_path(path);
        // Tabs and their roots are matched on the path, without the query.
        let path = split_query(&full).0.to_string();
        let tab = self.owning_tab(&path, true).unwrap_or(0);
        for i in 0..self.tabs.len() {
            // Fresh screens: only eager tabs (and the selected one) render.
            self.tabs[i].shown = self.tabs[i].eager;
            for old in std::mem::take(&mut self.tabs[i].entries) {
                release_screen(&self.shared, old.entity, cx);
            }
            let root = self.tabs[i].root.clone();
            let has_root_route = self.routes.iter().any(|r| r.matches(&root).is_some());
            if i == tab && path != root && has_root_route && cx.platform().is_web() {
                self.tabs[i].entries.push(Entry {
                    path: root,
                    entity: None,
                });
            } else if i != tab || (path != root && has_root_route) {
                let entity = self.open(&root, cx);
                self.tabs[i].entries.push(Entry {
                    path: root,
                    entity: Some(entity),
                });
            }
        }
        if path != self.tabs[tab].root || self.tabs[tab].entries.is_empty() {
            let entity = self.open(&full, cx);
            self.tabs[tab].entries.push(Entry {
                path: full,
                entity: Some(entity),
            });
        }
        self.selected = tab;
    }

    fn open(&self, path: &str, app: &mut App) -> AnyEntity {
        let before = app.last_entity_id();
        let entity = self.build_screen(path, app);
        // Built before: the app keeps it (and releases it).
        if entity.id() <= before {
            self.shared.borrow_mut().insert(entity.id());
        }
        entity
    }

    /// Releases every screen of every tab and the sheet: their entities,
    /// the entities they created with `cx.new`, their observers and the
    /// tasks tied to them. Screens a route hands out without creating them
    /// (`.route("/focus", move |_, _| focus)`) stay. Call it before dropping
    /// the router (signing out), or just release the router entity
    /// (`cx.release(router)`), which closes it. Until then it shows no
    /// screen; `cx.reset(path)` opens one again.
    pub fn close(&mut self, cx: &mut App) {
        for entity in self.take_screens() {
            cx.release_any(entity);
        }
    }

    /// Empties every stack and the sheet, returning the screens the router
    /// owns (what releasing it must release).
    pub(crate) fn take_screens(&mut self) -> Vec<AnyEntity> {
        let shared = self.shared.borrow();
        self.tabs
            .iter_mut()
            .flat_map(|t| t.entries.drain(..))
            .chain(self.modal.drain(..))
            .filter_map(|e| e.entity)
            .filter(|e| !shared.contains(&e.id()))
            .collect()
    }

    fn build_screen(&self, path: &str, app: &mut App) -> AnyEntity {
        let state = self.next_state.borrow_mut().take();
        for route in &self.routes {
            if let Some(mut params) = route.matches(path) {
                params.state = state;
                return (route.build)(app, &params);
            }
        }
        let params = Params {
            query: parse_query(split_query(path).1),
            state,
            ..Params::default()
        };
        match &self.not_found {
            Some(build) => build(app, &params),
            None => {
                let path = path.to_string();
                app.new_entity(|_| NotFound { path }).into_any()
            }
        }
    }
}

impl RouterBuilder {
    /// Adds a route. Patterns: `/`, `/todos`, `/todos/:id`, `/files/*rest`.
    ///
    /// The most specific route that matches a path wins, whatever the order
    /// the routes were added in: segment by segment from the left, a static
    /// segment beats a `:param`, which beats a `*rest` (`/todos/new` beats
    /// `/todos/:id`, `/files` beats `/files/*rest`). Routes of the same shape
    /// (`/a/:x` and `/a/:y`) keep their order: the first one added wins, and
    /// debug builds warn about the other.
    pub fn route<V: Render>(
        mut self,
        pattern: &str,
        build: impl Fn(&mut App, &Params) -> Entity<V> + 'static,
    ) -> Self {
        let segments = pattern
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                if let Some(name) = s.strip_prefix(':') {
                    Segment::Param(name.into())
                } else if let Some(name) = s.strip_prefix('*') {
                    Segment::Rest(name.into())
                } else {
                    Segment::Static(s.into())
                }
            })
            .collect();
        self.routes.push(Route {
            #[cfg(debug_assertions)]
            pattern: pattern.into(),
            segments,
            build: Rc::new(move |app, params| build(app, params).into_any()),
        });
        self
    }

    /// Adds a tab to a native tab bar. `icon` is an SF Symbol name (Apple
    /// platforms); `root` is the path of the tab's first screen. Each tab keeps
    /// its own navigation stack.
    pub fn tab(mut self, title: impl Into<String>, icon: impl Into<String>, root: &str) -> Self {
        self.tabs.push((
            TabTitle::Fixed(title.into()),
            icon.into(),
            normalize_path(root),
        ));
        self
    }

    /// Like [`tab`](Self::tab), with a title computed at each render, so it
    /// follows the language: `.tab_with(|cx| strings::tabs_todos(cx), "checklist", "/")`.
    pub fn tab_with(
        mut self,
        title: impl Fn(&App) -> String + 'static,
        icon: impl Into<String>,
        root: &str,
    ) -> Self {
        self.tabs.push((
            TabTitle::Dynamic(Rc::new(title)),
            icon.into(),
            normalize_path(root),
        ));
        self
    }

    /// Renders the tab declared just before up front, instead of on its
    /// first selection: `.tab("Player", "play", "/player").eager()`.
    ///
    /// Tabs are lazy by default: an unvisited tab shows its tab-bar entry
    /// (title and icon) with an empty panel, and its screens render when it
    /// is first selected, then stay (their state and nodes are kept when
    /// switching away). A page then ships only the visible tab (less HTML
    /// and a smaller first update on the web, fewer native views). Make a
    /// tab eager when switching to it must show content without waiting
    /// for a render (a remote web session's round trip), or when its views
    /// must exist while hidden. Without a tab before it, it does nothing.
    pub fn eager(mut self) -> Self {
        if let Some(last) = self.tabs.len().checked_sub(1) {
            self.eager.push(last);
        }
        self
    }

    /// Shows or hides the header bar (title and Back) of every stack:
    /// `.header(false)` for an app that draws its own chrome. Back stays
    /// available natively (swipe, system Back, browser Back, ⌘[) and through
    /// `cx.back()`. One screen can hide it with [`Screen::bare`](crate::Screen::bare).
    pub fn header(mut self, shown: bool) -> Self {
        self.header = Some(shown);
        self
    }

    /// The tint of the tab bar's selected tab and of the header bars'
    /// buttons (Back included): `UITabBar` / `UINavigationBar` `tintColor`,
    /// the selected segment on macOS, accent colors on Linux, Android and
    /// the web; not yet on Windows, which keeps the system's accent color.
    /// Without it they take the app's accent color
    /// ([`App::set_accent_color`], which `prism_ui::set_theme` sets), else
    /// the platform's.
    pub fn tint(mut self, color: crate::style::Color) -> Self {
        self.tint = Some(color);
        self
    }

    /// The screen shown when no route matches.
    pub fn not_found<V: Render>(
        mut self,
        build: impl Fn(&mut App, &Params) -> Entity<V> + 'static,
    ) -> Self {
        self.not_found = Some(Rc::new(move |app, params| build(app, params).into_any()));
        self
    }

    /// Creates the router entity, makes it the app's navigator, and opens the
    /// initial path (the request URL on the web, `PRISM_PATH` or `/` natively).
    pub fn build(mut self, app: &mut App) -> Entity<Router> {
        // Most specific first; a stable sort keeps the order added between
        // routes of the same shape.
        self.routes.sort_by_key(Route::specificity);
        #[cfg(debug_assertions)]
        for (i, route) in self.routes.iter().enumerate() {
            if let Some(earlier) = self.routes[..i].iter().find(|e| route.shadowed_by(e)) {
                eprintln!(
                    "prism: route {:?} is never reached: {:?}, added before it, matches the same paths",
                    route.pattern, earlier.pattern
                );
            }
        }
        let initial = app.initial_path().to_string();
        let tab_bar = !self.tabs.is_empty();
        let tabs = if tab_bar {
            self.tabs
                .into_iter()
                .enumerate()
                .map(|(i, (title, icon, root))| TabState {
                    title,
                    icon,
                    root,
                    entries: Vec::new(),
                    eager: self.eager.contains(&i),
                    shown: false,
                })
                .collect()
        } else {
            vec![TabState {
                title: TabTitle::Fixed(String::new()),
                icon: String::new(),
                root: "/".into(),
                entries: Vec::new(),
                eager: true,
                shown: true,
            }]
        };
        let router = app.new_entity(|_| Router {
            routes: self.routes,
            not_found: self.not_found,
            tabs,
            selected: 0,
            tab_bar,
            modal: Vec::new(),
            header: self.header.unwrap_or(true),
            next_state: RefCell::new(None),
            shared: RefCell::default(),
            tint: self.tint,
        });
        app.set_navigator(router);
        router.update(app, |r, cx| r.reset(&initial, cx));
        router
    }
}

/// Releases a screen the router built (not one shared with the app).
fn release_screen(
    shared: &RefCell<std::collections::HashSet<crate::app::EntityId>>,
    entity: Option<AnyEntity>,
    app: &mut App,
) {
    if let Some(entity) = entity
        && !shared.borrow().contains(&entity.id())
    {
        app.release_any(entity);
    }
}

fn tab_stack(
    t: &TabState,
    index: usize,
    header: bool,
    cx: &Context<Router>,
) -> crate::element::Stack {
    stack()
        .header(header)
        .path(t.entries.last().map_or("/", |e| e.path.as_str()))
        .on_back(cx.listener(move |this, e: &BackEvent, cx| this.truncate(index, e.depth, cx)))
        .on_navigate(cx.listener(|this, e: &NavigateEvent, cx| this.push(&e.path, false, cx)))
        // Lazy tabs: no screens until the tab is first selected.
        .children(
            t.entries
                .iter()
                .filter(|_| t.shown)
                .map(|e| match e.entity {
                    Some(entity) => Element::Entity(entity),
                    // Not built yet (under a deep link, web): an empty,
                    // hidden screen, so the stack's depth (Back, the
                    // browser's history) stays right.
                    None => screen(String::new()).key("pending").into_element(),
                }),
        )
}

/// The sheet: a closable stack; closing it (button, swipe, Escape) dismisses.
fn modal_sheet(
    entries: &[Entry],
    header: bool,
    tint: Option<crate::style::Color>,
    cx: &Context<Router>,
) -> Element {
    if entries.is_empty() {
        return Element::Empty;
    }
    modal()
        .key("modal")
        .on_dismiss(cx.listener(|this, _: &BackEvent, cx| this.dismiss(cx)))
        .child(
            stack()
                .closable(true)
                .header(header)
                .tint(tint)
                .path(entries.last().map_or("/", |e| e.path.as_str()))
                .on_back(cx.listener(|this, e: &BackEvent, cx| this.truncate_modal(e.depth, cx)))
                .on_navigate(cx.listener(|this, e: &NavigateEvent, cx| {
                    this.apply(NavOp::Push(e.path.clone()), cx)
                }))
                .children(entries.iter().filter_map(|e| e.entity).map(Element::Entity)),
        )
        .into_element()
}

impl Render for Router {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // The router's tint, else the app's accent color (#30).
        let tint = self.tint.or_else(|| cx.accent_color());
        let sheet = modal_sheet(&self.modal, self.header, tint, cx);
        // Lazy tabs: the selected tab renders, and stays rendered.
        self.show_selected_tab(cx.platform());
        if !self.tab_bar {
            return tab_stack(&self.tabs[0], 0, self.header, cx)
                .tint(tint)
                .child(sheet)
                .into_element();
        }
        tabs(self.selected)
            .tint(tint)
            .path(self.current_path())
            .on_change(cx.listener(|this, e: &ChangeEvent, _| this.select(e.index())))
            .children(self.tabs.iter().enumerate().map(|(i, t)| {
                tab(t.title.resolve(cx), t.icon.clone())
                    .key(i.to_string())
                    .child(tab_stack(t, i, self.header, cx).tint(tint))
            }))
            .child(sheet)
            .into_element()
    }
}

struct NotFound {
    path: String,
}

impl Render for NotFound {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        screen(cx.chrome_text("not_found")).child(div().p(24.).child(text(
            cx.chrome_text("no_route").replace("{path}", &self.path),
        )))
    }
}

/// A click handler that navigates: `button("Open").on_click(navigate_to("/todos/1"))`.
pub fn navigate_to(path: impl Into<String>) -> Handler<ClickEvent> {
    let path = path.into();
    Rc::new(move |_: &ClickEvent, app: &mut App| app.navigate(path.clone()))
}

/// A click handler that goes back.
pub fn go_back() -> Handler<ClickEvent> {
    Rc::new(|_: &ClickEvent, app: &mut App| app.back())
}
