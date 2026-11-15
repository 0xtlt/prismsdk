//! Navigation: stacks, tabs, sheets, deep links, back.

use prism_core::prelude::*;
use prism_core::{NodeKind, Patch, UiEvent, normalize_path};

use prism_testing::{Harness, creates, destroys};

struct Page(String);

impl Render for Page {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen(self.0.clone()).child(text(format!("at {}", self.0)))
    }
}

fn page(cx: &mut App, title: String) -> Entity<Page> {
    cx.new_entity(|_| Page(title))
}

fn routes(b: prism_core::RouterBuilder) -> prism_core::RouterBuilder {
    b.route("/", |cx, _| page(cx, "home".into()))
        // Declared first: wins over `/todos/:id`.
        .route("/todos/new", |cx, _| page(cx, "new".into()))
        .route("/todos/:id", |cx, p| {
            let id: Option<u32> = p.parse("id");
            page(cx, format!("todo {id:?}"))
        })
        .route("/users/:user/posts/:post", |cx, p| {
            page(
                cx,
                format!("{} / {}", p.get("user").unwrap(), p.get("post").unwrap()),
            )
        })
        .route("/files/*rest", |cx, p| {
            page(cx, format!("file {:?}", p.get("rest").unwrap()))
        })
        .route("/settings", |cx, _| page(cx, "settings".into()))
        .route("/settings/:section", |cx, p| {
            page(cx, format!("section {}", p.get("section").unwrap()))
        })
}

fn stack_app(path: &str) -> Harness {
    Harness::at(path, |cx| {
        let router = routes(Router::new()).build(cx);
        cx.set_root(router);
    })
}

fn tab_app(path: &str) -> Harness {
    Harness::at(path, |cx| {
        let router = routes(Router::new().tab("Home", "house", "/").tab(
            "Settings",
            "gear",
            "/settings",
        ))
        .build(cx);
        cx.set_root(router);
    })
}

/// Screen titles of the stack with this id.
fn screens(h: &Harness, stack: u64) -> Vec<String> {
    h.host.child_texts(stack)
}

fn stack(h: &Harness) -> u64 {
    h.host.find(|n| n.kind == NodeKind::Stack).expect("a stack")
}

fn path(h: &Harness) -> String {
    h.rt.app.current_path().unwrap_or_default().to_string()
}

#[test]
fn paths_are_normalized() {
    assert_eq!(normalize_path(""), "/");
    assert_eq!(normalize_path("/"), "/");
    assert_eq!(normalize_path("todos/1/"), "/todos/1");
    assert_eq!(normalize_path("//todos//"), "/todos");
    assert_eq!(normalize_path("/a?b=/c"), "/a");
    assert_eq!(normalize_path("/a#/frag"), "/a");
}

#[test]
fn params_static_segments_and_rest() {
    let cases = [
        ("/todos/new", "new"),
        ("/todos/42", "todo Some(42)"),
        ("/todos/abc", "todo None"),
        ("/users/ada/posts/7", "ada / 7"),
        ("/files/a/b/c.txt", r#"file "a/b/c.txt""#),
        ("/files", r#"file """#),
        ("/settings/", "settings"),
    ];
    for (p, title) in cases {
        let h = stack_app(p);
        let s = stack(&h);
        assert_eq!(screens(&h, s).last().unwrap(), title, "{p}");
        assert_eq!(h.host.get(s).text, normalize_path(p), "{p}");
    }
    // Too many segments: no match.
    let h = stack_app("/todos/1/extra");
    h.host.by_text("Nothing at /todos/1/extra");
}

#[test]
fn deep_links_open_on_top_of_home() {
    let h = stack_app("/todos/3?from=widget");
    assert_eq!(screens(&h, stack(&h)), ["home", "todo Some(3)"]);
    assert_eq!(path(&h), "/todos/3");
}

#[test]
fn pushing_the_current_path_again_does_nothing() {
    let mut h = stack_app("/todos/1");
    h.rt.app.navigate("/todos/1/");
    assert!(h.render().is_empty());
    assert_eq!(screens(&h, stack(&h)), ["home", "todo Some(1)"]);
}

#[test]
fn back_pops_and_releases_the_screen() {
    let mut h = stack_app("/");
    h.rt.app.navigate("/todos/1");
    h.render();
    h.rt.app.navigate("/todos/2");
    h.render();
    let s = stack(&h);
    assert_eq!(screens(&h, s), ["home", "todo Some(1)", "todo Some(2)"]);
    let top = h.host.get(s).children[2];
    h.rt.app.back();
    let p = h.render().to_vec();
    assert!(destroys(&p).contains(&top));
    assert_eq!(screens(&h, s), ["home", "todo Some(1)"]);
    // Native back to the root in one step (long press on Back).
    h.dispatch(UiEvent::Back(s, 1));
    assert_eq!(screens(&h, s), ["home"]);
    // Back at the root is ignored, from the app and from the platform.
    h.rt.app.back();
    h.render();
    assert!(h.dispatch(UiEvent::Back(s, 0)).is_empty());
    assert_eq!(screens(&h, s), ["home"]);
    // A stale report deeper than the stack changes nothing.
    assert!(h.dispatch(UiEvent::Back(s, 5)).is_empty());
}

#[test]
fn replace_swaps_the_top_screen() {
    let mut h = stack_app("/todos/1");
    let s = stack(&h);
    let old = h.host.get(s).children[1];
    h.rt.app.replace("/todos/2");
    let p = h.render().to_vec();
    // The old screen and its content.
    assert_eq!(destroys(&p).len(), 2);
    assert!(destroys(&p).contains(&old));
    assert_eq!(screens(&h, s), ["home", "todo Some(2)"]);
    assert_eq!(path(&h), "/todos/2");
}

#[test]
fn reset_rebuilds_the_stack_with_home_underneath() {
    let mut h = stack_app("/");
    for p in ["/todos/1", "/todos/2", "/todos/3"] {
        h.rt.app.navigate(p);
        h.render();
    }
    h.rt.app.reset("/settings");
    h.render();
    assert_eq!(screens(&h, stack(&h)), ["home", "settings"]);
    h.rt.app.reset("/");
    h.render();
    assert_eq!(screens(&h, stack(&h)), ["home"]);
}

#[test]
fn custom_not_found_screen() {
    let h = Harness::at("/missing", |cx| {
        let router = Router::new()
            .route("/", |cx, _| page(cx, "home".into()))
            .not_found(|cx, _| page(cx, "404".into()))
            .build(cx);
        cx.set_root(router);
    });
    assert_eq!(screens(&h, stack(&h)), ["home", "404"]);
}

#[test]
fn not_found_follows_the_language() {
    let h = Harness::with_runtime(
        |init| {
            prism_core::Runtime::with_options(
                init,
                prism_core::RuntimeOptions {
                    path: "/nope".into(),
                    environment: prism_core::Environment {
                        locale: Locale::new("fr-FR"),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
        },
        |cx| {
            let router = Router::new()
                .route("/", |cx, _| page(cx, "home".into()))
                .build(cx);
            cx.set_root(router);
        },
    );
    h.host.by_text("Rien à l’adresse /nope");
    assert_eq!(screens(&h, stack(&h)), ["home", "Introuvable"]);
}

// --- Tabs --------------------------------------------------------------------

fn tabs_node(h: &Harness) -> u64 {
    h.host.find(|n| n.kind == NodeKind::Tabs).unwrap()
}

#[test]
fn tabs_carry_titles_icons_and_selection() {
    let h = tab_app("/");
    let tabs = tabs_node(&h);
    assert_eq!(h.host.get(tabs).value, Some(0.));
    let tab_nodes = &h.host.get(tabs).children;
    assert_eq!(tab_nodes.len(), 2);
    let (home, settings) = (h.host.get(tab_nodes[0]), h.host.get(tab_nodes[1]));
    assert_eq!((home.text.as_str(), home.icon.as_str()), ("Home", "house"));
    assert_eq!(
        (settings.text.as_str(), settings.icon.as_str()),
        ("Settings", "gear")
    );
    // Tabs label themselves with their title.
    assert_eq!(home.a11y.label.as_deref(), Some("Home"));
    assert_eq!(home.a11y.role, Some(Role::TabPanel));
    assert_eq!(h.host.get(tabs).a11y.role, Some(Role::TabList));
}

#[test]
fn deep_links_pick_the_tab_with_the_longest_matching_root() {
    let h = tab_app("/settings/privacy");
    assert_eq!(h.host.get(tabs_node(&h)).value, Some(1.));
    assert_eq!(path(&h), "/settings/privacy");
    // The other tab still has its root screen.
    let all: Vec<String> = h
        .host
        .mounted()
        .into_iter()
        .filter(|id| h.host.get(*id).kind == NodeKind::Screen)
        .map(|id| h.host.get(id).text.clone())
        .collect();
    assert_eq!(all, ["home", "settings", "section privacy"]);
    // A path under no other tab's root opens in the "/" tab.
    let h = tab_app("/todos/4");
    assert_eq!(h.host.get(tabs_node(&h)).value, Some(0.));
}

#[test]
fn navigating_to_a_tab_root_switches_tab_and_keeps_stacks() {
    let mut h = tab_app("/");
    h.rt.app.navigate("/todos/9");
    h.render();
    h.rt.app.navigate("/settings");
    let p = h.render().to_vec();
    let tabs = tabs_node(&h);
    assert!(p.contains(&Patch::SetValue {
        id: tabs,
        value: 1.
    }));
    assert_eq!(creates(&p), 0, "switching tab creates nothing: {p:?}");
    h.rt.app.navigate("/");
    h.render();
    assert_eq!(path(&h), "/todos/9");
}

#[test]
fn out_of_range_tab_selection_is_ignored() {
    let mut h = tab_app("/");
    let tabs = tabs_node(&h);
    h.dispatch(UiEvent::Change(tabs, 7.));
    assert_eq!(h.host.get(tabs).value, Some(0.));
    assert_eq!(path(&h), "/");
}

// --- Sheets --------------------------------------------------------------------

fn modal(h: &Harness) -> Option<u64> {
    h.host.find(|n| n.kind == NodeKind::Modal)
}

#[test]
fn a_sheet_has_its_own_closable_stack() {
    let mut h = stack_app("/");
    h.rt.app.present("/todos/new");
    h.render();
    let m = modal(&h).expect("a sheet");
    let sheet_stack = h.host.get(m).children[0];
    assert_eq!(h.host.get(sheet_stack).kind, NodeKind::Stack);
    assert_eq!(screens(&h, sheet_stack), ["new"]);
    // Pushing inside the sheet, then pushing the same path again.
    h.rt.app.navigate("/todos/1");
    h.render();
    h.rt.app.navigate("/todos/1");
    assert!(h.render().is_empty());
    assert_eq!(screens(&h, sheet_stack), ["new", "todo Some(1)"]);
    // The main stack is untouched underneath.
    assert_eq!(path(&h), "/");
    // Back inside the sheet pops it; back on its first screen closes it.
    h.rt.app.back();
    h.render();
    assert_eq!(screens(&h, sheet_stack), ["new"]);
    h.rt.app.back();
    let p = h.render().to_vec();
    assert!(destroys(&p).contains(&m));
    assert!(modal(&h).is_none());
}

#[test]
fn presenting_again_replaces_the_sheet() {
    let mut h = stack_app("/");
    h.rt.app.present("/todos/1");
    h.render();
    h.rt.app.present("/todos/2");
    h.render();
    let m = modal(&h).unwrap();
    let sheet_stack = h.host.get(m).children[0];
    assert_eq!(screens(&h, sheet_stack), ["todo Some(2)"]);
    h.rt.app.dismiss();
    h.render();
    assert!(modal(&h).is_none());
    // Dismissing with nothing presented is harmless.
    h.rt.app.dismiss();
    h.render();
    assert_eq!(screens(&h, stack(&h)), ["home"]);
}

#[test]
fn the_platform_can_pop_inside_a_sheet() {
    let mut h = stack_app("/");
    h.rt.app.present("/todos/new");
    h.render();
    h.rt.app.navigate("/todos/1");
    h.render();
    h.rt.app.navigate("/todos/2");
    h.render();
    let m = modal(&h).unwrap();
    let sheet_stack = h.host.get(m).children[0];
    h.dispatch(UiEvent::Back(sheet_stack, 1));
    assert_eq!(screens(&h, sheet_stack), ["new"]);
    // Swipe down: the modal reports a dismissal.
    h.dispatch(UiEvent::Back(m, 0));
    assert!(modal(&h).is_none());
}

#[test]
fn links_from_widgets_and_notifications_open_screens() {
    let mut h = tab_app("/");
    h.open_link("/todos/5");
    assert_eq!(path(&h), "/todos/5");
    h.host.by_text("at todo Some(5)");
    // A tab's root switches to that tab.
    h.open_link("/settings");
    assert_eq!(h.host.get(tabs_node(&h)).value, Some(1.));
    h.open_link("/settings/about");
    assert_eq!(path(&h), "/settings/about");
}

#[test]
fn sheets_over_tabs() {
    let mut h = tab_app("/");
    h.rt.app.present("/todos/new");
    h.render();
    let m = modal(&h).expect("a sheet over the tab bar");
    assert_eq!(h.host.get(tabs_node(&h)).value, Some(0.));
    h.dispatch(UiEvent::Back(m, 0));
    assert!(modal(&h).is_none());
}
