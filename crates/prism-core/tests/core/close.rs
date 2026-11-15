//! Closing a router: signing out releases every screen (#31).

use std::rc::Rc;

use prism_core::prelude::*;

use prism_testing::Harness;

/// What the signed-in part of the app shares.
struct Store;

/// The app's state, kept as a global.
#[derive(Default)]
struct Session {
    store: Option<Entity<Store>>,
    router: Option<Entity<Router>>,
    page: Option<Entity<SignIn>>,
    /// Observers of the store that ran.
    hits: usize,
    /// Tasks the screens spawned.
    tasks: Vec<u64>,
    /// Shared with the app, handed out by a route.
    shared: Option<Entity<Plain>>,
}

struct Plain(&'static str);
impl Render for Plain {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen(self.0)
    }
}

struct Child;
impl Render for Child {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        text("child")
    }
}

/// A screen that creates a child, observes the store and runs a task.
struct Home {
    child: Entity<Child>,
}

impl Home {
    fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_: &mut Home, _, cx| {
            cx.global_mut::<Session>().hits += 1;
        });
        let task = cx
            .spawn(async move |_, _| std::future::pending::<()>().await)
            .cancel_on_release();
        cx.global_mut::<Session>().tasks.push(task.id());
        Home {
            child: cx.new(|_| Child),
        }
    }
}

impl Render for Home {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        screen("Home").child(self.child).child(
            // Releases the router, and this screen with it, from its own handler.
            button("Sign out").on_click(cx.listener(|_: &mut Home, _, cx| sign_out(cx))),
        )
    }
}

struct SignIn;
impl Render for SignIn {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Reading the path once the router is gone must not panic.
        let at = cx.current_path().unwrap_or("no router").to_string();
        div()
            .child(text(format!("signed out ({at})")))
            .child(button("Sign in").on_click(Rc::new(|_: &ClickEvent, cx: &mut App| sign_in(cx))))
    }
}

fn sign_in(cx: &mut App) {
    let store = cx.global_mut::<Session>().store.expect("a store");
    let shared = cx.global_mut::<Session>().shared.expect("a shared screen");
    let router = Router::new()
        .tab("Home", "house", "/")
        .tab("Settings", "gear", "/settings")
        .route("/", move |cx, _| cx.new_entity(|cx| Home::new(store, cx)))
        .route("/settings", move |cx, _| {
            cx.new_entity(|cx| Home::new(store, cx))
        })
        .route("/shared", move |_, _| shared)
        .build(cx);
    if let Some(page) = cx.global_mut::<Session>().page.take() {
        cx.release(page);
    }
    cx.global_mut::<Session>().router = Some(router);
    cx.set_root(router);
}

fn sign_out(cx: &mut App) {
    let router = cx.global_mut::<Session>().router.take().expect("signed in");
    cx.release(router);
    let page = cx.new_entity(|_| SignIn);
    cx.global_mut::<Session>().page = Some(page);
    cx.set_root(page);
}

fn app() -> Harness {
    Harness::at("/", |cx| {
        let store = cx.new_entity(|_| Store);
        let shared = cx.new_entity(|_| Plain("Shared"));
        cx.set_global(Session {
            store: Some(store),
            shared: Some(shared),
            ..Session::default()
        });
        sign_in(cx);
    })
}

fn session(h: &Harness) -> &Session {
    h.rt.app.global::<Session>()
}

fn notify_store(h: &mut Harness) -> usize {
    let store = session(h).store.unwrap();
    let before = session(h).hits;
    store.notify(&mut h.rt.app);
    h.render();
    session(h).hits - before
}

#[test]
fn signing_out_releases_every_screen_with_what_it_made() {
    let mut h = app();
    // The store, the shared screen, the session's router and both tabs'
    // screens with their child.
    let signed_in = h.rt.app.entity_count();
    assert_eq!(signed_in, 2 + 1 + 2 * 2);
    assert_eq!(notify_store(&mut h), 2, "both screens observe the store");
    let tasks = session(&h).tasks.clone();
    assert!(tasks.iter().all(|t| h.rt.is_task_running(*t)));
    // The shared screen is handed out by a route, not created by it.
    h.rt.app.navigate("/shared");
    h.render();

    h.click("Sign out");
    h.host.by_text("signed out (no router)");
    assert_eq!(h.rt.app.current_path(), None);
    // The store, the shared screen (the app's) and the sign-in page.
    assert_eq!(h.rt.app.entity_count(), 3);
    assert_eq!(notify_store(&mut h), 0, "their observers are gone");
    assert!(tasks.iter().all(|t| !h.rt.is_task_running(*t)), "tasks too");

    // Signing in again builds one set of screens, not a second one.
    h.click("Sign in");
    assert_eq!(h.rt.app.entity_count(), signed_in);
    assert_eq!(notify_store(&mut h), 2);
    assert_eq!(h.rt.app.current_path(), Some("/"));
    h.rt.app.navigate("/shared");
    h.render();
    h.host.check();

    // And again: still no duplicates, no panic.
    h.rt.app.navigate("/");
    h.render();
    h.click("Sign out");
    h.click("Sign in");
    assert_eq!(h.rt.app.entity_count(), signed_in);
}

#[test]
fn close_releases_the_screens_and_reset_opens_them_again() {
    let mut h = app();
    let signed_in = h.rt.app.entity_count();
    h.rt.app.present("/settings");
    h.render();
    assert_eq!(
        h.rt.app.entity_count(),
        signed_in + 2,
        "a sheet and its child"
    );
    let router = session(&h).router.unwrap();
    router.update(&mut h.rt.app, |r, cx| {
        r.close(cx);
        cx.notify();
    });
    h.render();
    assert_eq!(h.rt.app.entity_count(), 3, "store, shared screen, router");
    assert!(!router.read(&h.rt.app).is_presenting());
    assert_eq!(notify_store(&mut h), 0);
    // Still the navigator: navigation opens screens again.
    h.rt.app.reset("/");
    h.render();
    assert_eq!(h.rt.app.entity_count(), signed_in);
    assert_eq!(h.rt.app.current_path(), Some("/"));
}

#[test]
fn releasing_an_entity_releases_what_it_created_with_cx_new() {
    struct Parent {
        _child: Entity<Child>,
        _kept: Entity<Child>,
    }
    let mut app = App::new();
    let parent = app.new_entity(|cx| Parent {
        _child: cx.new(|_| Child),
        _kept: cx.new_entity(|_| Child),
    });
    assert_eq!(app.entity_count(), 3);
    app.release(parent);
    // `cx.new` makes a child; `new_entity` an entity of its own.
    assert_eq!(app.entity_count(), 1);
}
