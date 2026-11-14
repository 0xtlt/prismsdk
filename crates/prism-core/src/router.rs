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
    BackEvent, ClickEvent, Element, Handler, IntoElement, NavigateEvent, ParentElement, div,
    screen, stack, text,
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
}

struct Entry {
    path: String,
    entity: AnyEntity,
}

/// The navigation state: a stack of screens, each one an entity built from a route.
pub struct Router {
    routes: Vec<Route>,
    not_found: Option<Build>,
    entries: Vec<Entry>,
}

/// Declares the routes, then [`build`](RouterBuilder::build)s the router.
#[derive(Default)]
pub struct RouterBuilder {
    routes: Vec<Route>,
    not_found: Option<Build>,
}

impl Router {
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> RouterBuilder {
        RouterBuilder::default()
    }

    /// Path of the visible screen.
    pub fn current_path(&self) -> &str {
        self.entries.last().map_or("/", |e| e.path.as_str())
    }

    /// Number of screens in the stack.
    pub fn depth(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn apply(&mut self, op: NavOp, cx: &mut Context<Self>) {
        match op {
            NavOp::Push(path) => self.push(&path, cx),
            NavOp::Replace(path) => {
                let path = normalize_path(&path);
                if let Some(old) = self.entries.pop() {
                    cx.release_any(old.entity);
                }
                let entity = self.open(&path, cx);
                self.entries.push(Entry { path, entity });
            }
            NavOp::Reset(path) => self.reset(&path, cx),
            NavOp::Back => self.truncate(self.entries.len().saturating_sub(1), cx),
        }
        cx.notify();
    }

    fn push(&mut self, path: &str, cx: &mut Context<Self>) {
        let path = normalize_path(path);
        if self.current_path() == path {
            return;
        }
        let entity = self.open(&path, cx);
        self.entries.push(Entry { path, entity });
    }

    /// Keeps the first `depth` screens (at least one).
    fn truncate(&mut self, depth: usize, cx: &mut Context<Self>) {
        while self.entries.len() > depth.max(1) {
            let old = self.entries.pop().expect("non-empty");
            cx.release_any(old.entity);
        }
    }

    /// A deep link to `/a/b` opens `/` underneath, so "back" has somewhere to go.
    fn reset(&mut self, path: &str, cx: &mut Context<Self>) {
        for old in self.entries.drain(..) {
            cx.release_any(old.entity);
        }
        let path = normalize_path(path);
        if path != "/" && self.routes.iter().any(|r| r.matches("/").is_some()) {
            let home = self.open("/", cx);
            self.entries.push(Entry {
                path: "/".into(),
                entity: home,
            });
        }
        let entity = self.open(&path, cx);
        self.entries.push(Entry { path, entity });
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
        let router = app.new_entity(|_| Router {
            routes: self.routes,
            not_found: self.not_found,
            entries: Vec::new(),
        });
        app.set_navigator(router);
        router.update(app, |r, cx| r.reset(&initial, cx));
        router
    }
}

impl Render for Router {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        stack()
            .path(self.current_path())
            .on_back(cx.listener(|this, e: &BackEvent, cx| this.truncate(e.depth, cx)))
            .on_navigate(cx.listener(|this, e: &NavigateEvent, cx| this.push(&e.path, cx)))
            .children(self.entries.iter().map(|e| Element::Entity(e.entity)))
    }
}

struct NotFound {
    path: String,
}

impl Render for NotFound {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen("Not found").child(
            div()
                .p(24.)
                .child(text(format!("No route for {}", self.path))),
        )
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
