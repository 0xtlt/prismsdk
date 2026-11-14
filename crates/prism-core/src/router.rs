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
//! - iOS: `UINavigationController` (push animation, swipe back, large titles).
//! - macOS: a native header bar with a Back button, plus ⌘[.
//! - Web: real URLs via the History API; server-side rendering of deep links.

use std::rc::Rc;
use std::str::FromStr;

use crate::app::{AnyEntity, App, Context, Entity, Render};
use crate::element::{
    BackEvent, ChangeEvent, ClickEvent, Element, Handler, IntoElement, NavigateEvent,
    ParentElement, div, modal, screen, stack, tab, tabs, text,
};
use crate::style::Styled;

/// Path parameters captured by a route pattern (`/todos/:id`, `/files/*rest`).
#[derive(Clone, Debug, Default)]
pub struct Params {
    values: Vec<(String, String)>,
}

impl Params {
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
}

type Build = Rc<dyn Fn(&mut App, &Params) -> AnyEntity>;

enum Segment {
    Static(String),
    Param(String),
    Rest(String),
}

struct Route {
    segments: Vec<Segment>,
    build: Build,
}

impl Route {
    fn matches(&self, path: &str) -> Option<Params> {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut params = Params::default();
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

/// `/a/b?x=1#y` → `/a/b`; always starts with `/`, no trailing `/`.
pub fn normalize_path(path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or("");
    let trimmed = path.trim_matches('/');
    format!("/{trimmed}")
}

pub(crate) enum NavOp {
    Push(String),
    Replace(String),
    Reset(String),
    Back,
    Present(String),
    Dismiss,
}

struct Entry {
    path: String,
    entity: AnyEntity,
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
}

/// Declares the routes (and optionally tabs), then [`build`](RouterBuilder::build)s the router.
#[derive(Default)]
pub struct RouterBuilder {
    routes: Vec<Route>,
    not_found: Option<Build>,
    tabs: Vec<(TabTitle, String, String)>,
}

impl Router {
    #[allow(clippy::new_ret_no_self)]
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

    pub(crate) fn apply(&mut self, op: NavOp, cx: &mut Context<Self>) {
        match op {
            NavOp::Present(path) => {
                let path = normalize_path(&path);
                self.dismiss(cx);
                let entity = self.open(&path, cx);
                self.modal.push(Entry { path, entity });
            }
            NavOp::Dismiss => self.dismiss(cx),
            // While a sheet is up, navigation happens inside it.
            NavOp::Push(path) if !self.modal.is_empty() => {
                let path = normalize_path(&path);
                if self.modal.last().is_some_and(|e| e.path == path) {
                    return;
                }
                let entity = self.open(&path, cx);
                self.modal.push(Entry { path, entity });
            }
            NavOp::Back if !self.modal.is_empty() => {
                let depth = self.modal.len() - 1;
                self.truncate_modal(depth, cx);
            }
            NavOp::Push(path) => self.push(&path, cx),
            NavOp::Replace(path) => {
                let path = normalize_path(&path);
                let tab = self.selected;
                if let Some(old) = self.tabs[tab].entries.pop() {
                    cx.release_any(old.entity);
                }
                let entity = self.open(&path, cx);
                self.tabs[tab].entries.push(Entry { path, entity });
            }
            NavOp::Reset(path) => self.reset(&path, cx),
            NavOp::Back => {
                let depth = self.depth().saturating_sub(1);
                self.truncate(self.selected, depth, cx)
            }
        }
        cx.notify();
    }

    /// Pushes `path` on the current tab, or switches tab if `path` is a tab's root.
    fn push(&mut self, path: &str, cx: &mut Context<Self>) {
        let path = normalize_path(path);
        if self.tab_bar
            && let Some(i) = self.tabs.iter().position(|t| t.root == path)
        {
            self.selected = i;
            return;
        }
        if self.current_path() == path {
            return;
        }
        let entity = self.open(&path, cx);
        self.tabs[self.selected]
            .entries
            .push(Entry { path, entity });
    }

    /// Keeps the first `depth` screens (at least one) of tab `tab`.
    fn truncate(&mut self, tab: usize, depth: usize, cx: &mut Context<Self>) {
        let Some(t) = self.tabs.get_mut(tab) else {
            return;
        };
        while t.entries.len() > depth.max(1) {
            let old = t.entries.pop().expect("non-empty");
            cx.release_any(old.entity);
        }
    }

    /// Closes the sheet, releasing its screens.
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        for old in self.modal.drain(..) {
            cx.release_any(old.entity);
        }
    }

    /// Keeps the first `depth` screens of the sheet; 0 closes it.
    fn truncate_modal(&mut self, depth: usize, cx: &mut Context<Self>) {
        while self.modal.len() > depth {
            let old = self.modal.pop().expect("non-empty");
            cx.release_any(old.entity);
        }
    }

    /// Whether a sheet is presented.
    pub fn is_presenting(&self) -> bool {
        !self.modal.is_empty()
    }

    fn select(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.selected = index;
        }
    }

    /// Opens `path` in the tab whose root best matches it. Each tab's root
    /// screen sits underneath, so "back" has somewhere to go.
    fn reset(&mut self, path: &str, cx: &mut Context<Self>) {
        let path = normalize_path(path);
        let tab = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.root == "/" || path == t.root || path.starts_with(&format!("{}/", t.root))
            })
            .max_by_key(|(_, t)| t.root.len())
            .map_or(0, |(i, _)| i);
        for i in 0..self.tabs.len() {
            for old in std::mem::take(&mut self.tabs[i].entries) {
                cx.release_any(old.entity);
            }
            let root = self.tabs[i].root.clone();
            let has_root_route = self.routes.iter().any(|r| r.matches(&root).is_some());
            if i != tab || (path != root && has_root_route) {
                let entity = self.open(&root, cx);
                self.tabs[i].entries.push(Entry { path: root, entity });
            }
        }
        if path != self.tabs[tab].root || self.tabs[tab].entries.is_empty() {
            let entity = self.open(&path, cx);
            self.tabs[tab].entries.push(Entry { path, entity });
        }
        self.selected = tab;
    }

    fn open(&self, path: &str, app: &mut App) -> AnyEntity {
        for route in &self.routes {
            if let Some(params) = route.matches(path) {
                return (route.build)(app, &params);
            }
        }
        match &self.not_found {
            Some(build) => build(app, &Params::default()),
            None => {
                let path = path.to_string();
                app.new_entity(|_| NotFound { path }).into_any()
            }
        }
    }
}

impl RouterBuilder {
    /// Adds a route. Patterns: `/`, `/todos`, `/todos/:id`, `/files/*rest`.
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
    pub fn build(self, app: &mut App) -> Entity<Router> {
        let initial = app.initial_path().to_string();
        let tab_bar = !self.tabs.is_empty();
        let tabs = if tab_bar {
            self.tabs
                .into_iter()
                .map(|(title, icon, root)| TabState {
                    title,
                    icon,
                    root,
                    entries: Vec::new(),
                })
                .collect()
        } else {
            vec![TabState {
                title: TabTitle::Fixed(String::new()),
                icon: String::new(),
                root: "/".into(),
                entries: Vec::new(),
            }]
        };
        let router = app.new_entity(|_| Router {
            routes: self.routes,
            not_found: self.not_found,
            tabs,
            selected: 0,
            tab_bar,
            modal: Vec::new(),
        });
        app.set_navigator(router);
        router.update(app, |r, cx| r.reset(&initial, cx));
        router
    }
}

fn tab_stack(t: &TabState, index: usize, cx: &Context<Router>) -> crate::element::Stack {
    stack()
        .path(t.entries.last().map_or("/", |e| e.path.as_str()))
        .on_back(cx.listener(move |this, e: &BackEvent, cx| this.truncate(index, e.depth, cx)))
        .on_navigate(cx.listener(|this, e: &NavigateEvent, cx| this.push(&e.path, cx)))
        .children(t.entries.iter().map(|e| Element::Entity(e.entity)))
}

/// The sheet: a closable stack; closing it (button, swipe, Escape) dismisses.
fn modal_sheet(entries: &[Entry], cx: &Context<Router>) -> Element {
    if entries.is_empty() {
        return Element::Empty;
    }
    modal()
        .key("modal")
        .on_dismiss(cx.listener(|this, _: &BackEvent, cx| this.dismiss(cx)))
        .child(
            stack()
                .closable(true)
                .path(entries.last().map_or("/", |e| e.path.as_str()))
                .on_back(cx.listener(|this, e: &BackEvent, cx| this.truncate_modal(e.depth, cx)))
                .on_navigate(cx.listener(|this, e: &NavigateEvent, cx| {
                    this.apply(NavOp::Push(e.path.clone()), cx)
                }))
                .children(entries.iter().map(|e| Element::Entity(e.entity))),
        )
        .into_element()
}

impl Render for Router {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let sheet = modal_sheet(&self.modal, cx);
        if !self.tab_bar {
            return tab_stack(&self.tabs[0], 0, cx).child(sheet).into_element();
        }
        tabs(self.selected)
            .path(self.current_path())
            .on_change(cx.listener(|this, e: &ChangeEvent, _| this.select(e.index())))
            .children(self.tabs.iter().enumerate().map(|(i, t)| {
                tab(t.title.resolve(cx), t.icon.clone())
                    .key(i.to_string())
                    .child(tab_stack(t, i, cx))
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
