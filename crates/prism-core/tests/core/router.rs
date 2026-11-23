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
        // Declared after `/todos/new`, which still wins: more specific.
        .route("/todos/:id", |cx, p| {
            let id: Option<u32> = p.parse("id");
            page(cx, format!("todo {id:?}"))
        })
        .route("/todos/new", |cx, _| page(cx, "new".into()))
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
    // The query string is kept as it is; the fragment is dropped.
    assert_eq!(normalize_path("/a?b=/c"), "/a?b=/c");
    assert_eq!(normalize_path("a/?x=1&y=2#top"), "/a?x=1&y=2");
    assert_eq!(normalize_path("/a?"), "/a");
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

/// The most specific route wins, whatever the order the routes were added in.
fn ranked_app(path: &str) -> Harness {
    Harness::at(path, |cx| {
        let router = Router::new()
            .route("/", |cx, _| page(cx, "home".into()))
            .route("/*rest", |cx, _| page(cx, "catch-all".into()))
            .route("/files/*rest", |cx, p| {
                page(cx, format!("file {:?}", p.get("rest").unwrap()))
            })
            .route("/files", |cx, _| page(cx, "files".into()))
            .route("/:a/new", |cx, p| {
                page(cx, format!("new in {}", p.get("a").unwrap()))
            })
            .route("/sites/:site", |cx, p| {
                page(cx, format!("site {}", p.get("site").unwrap()))
            })
            .route("/sites/new", |cx, _| page(cx, "new site".into()))
            // Same shape as `/sites/:site`, added after it: never reached.
            .route("/sites/:other", |cx, _| page(cx, "shadowed".into()))
            .build(cx);
        cx.set_root(router);
    })
}

#[test]
fn the_most_specific_route_wins() {
    let cases = [
        ("/sites/new", "new site"),
        ("/sites/ada", "site ada"),
        // A static first segment beats a parameter there.
        ("/pages/new", "new in pages"),
        ("/files", "files"),
        ("/files/a/b", r#"file "a/b""#),
        ("/other/path", "catch-all"),
        ("/", "home"),
    ];
    for (p, title) in cases {
        let h = ranked_app(p);
        assert_eq!(screens(&h, stack(&h)).last().unwrap(), title, "{p}");
    }
}

#[test]
fn deep_links_open_on_top_of_home() {
    let h = stack_app("/todos/3?from=widget");
    assert_eq!(screens(&h, stack(&h)), ["home", "todo Some(3)"]);
    assert_eq!(path(&h), "/todos/3?from=widget");
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
    // The other tab is in the bar, but lazy: no screen until selected.
    assert_eq!(h.host.get(tabs_node(&h)).children.len(), 2);
    let all: Vec<String> = h
        .host
        .mounted()
        .into_iter()
        .filter(|id| h.host.get(*id).kind == NodeKind::Screen)
        .map(|id| h.host.get(id).text.clone())
        .collect();
    assert_eq!(all, ["settings", "section privacy"]);
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
    // Lazy tabs: the first visit renders the tab's root screen.
    assert!(creates(&p) > 0);
    h.rt.app.navigate("/");
    let p = h.render().to_vec();
    assert_eq!(creates(&p), 0, "switching tab creates nothing: {p:?}");
    assert_eq!(path(&h), "/todos/9");
    h.rt.app.navigate("/settings");
    let p = h.render().to_vec();
    assert_eq!(creates(&p), 0, "a visited tab stays rendered: {p:?}");
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

// --- Query strings, navigation state, hidden headers (#7, #8) ----------------

/// A "Saved" notice handed to the next screen.
struct Saved(&'static str);

fn query_app(path: &str) -> Harness {
    Harness::at(path, |cx| {
        let router = Router::new()
            .route("/", |cx, _| page(cx, "home".into()))
            .route("/analytics", |cx, p| {
                let rooms = p.query_all("room").join("+");
                let notice = p.state::<Saved>().map_or("-", |s| s.0);
                page(
                    cx,
                    format!(
                        "analytics {} [{rooms}] {notice}",
                        p.query("period").unwrap_or("all")
                    ),
                )
            })
            .route("/items/:id", |cx, p| {
                let notice = p.state::<Saved>().map_or("-", |s| s.0);
                page(cx, format!("item {} {notice}", p.get("id").unwrap()))
            })
            .build(cx);
        cx.set_root(router);
    })
}

#[test]
fn routes_read_the_query_string_with_every_value_decoded() {
    let h = query_app("/analytics?period=7d&room=3&room=4%2B5");
    assert_eq!(screens(&h, stack(&h)), ["home", "analytics 7d [3+4+5] -"]);
    // The stack (the URL on the web) keeps it.
    assert_eq!(
        h.host.get(stack(&h)).text,
        "/analytics?period=7d&room=3&room=4%2B5"
    );
    assert_eq!(path(&h), "/analytics?period=7d&room=3&room=4%2B5");
    let q = h.rt.app.current_query();
    assert_eq!(q[0], ("period".to_string(), "7d".to_string()));
    assert_eq!(q[2].1, "4+5");
}

#[test]
fn query_helpers_round_trip() {
    let q = prism_core::encode_query([("q", "a&b c"), ("é", "1/2"), ("edit", "")]);
    assert_eq!(q, "q=a%26b%20c&%C3%A9=1/2&edit=");
    assert_eq!(
        prism_core::parse_query(&q),
        [
            ("q".to_string(), "a&b c".to_string()),
            ("é".into(), "1/2".into()),
            ("edit".into(), "".into())
        ]
    );
    // `+` is a space; broken escapes stay as they are.
    assert_eq!(prism_core::parse_query("a=x+y&b=%zz&c")[0].1, "x y");
    assert_eq!(prism_core::parse_query("b=%zz")[0].1, "%zz");
    assert_eq!(prism_core::parse_query("c&&")[0], ("c".into(), "".into()));
}

#[test]
fn navigating_to_another_query_pushes_a_screen_back_undoes() {
    let mut h = query_app("/analytics?period=7d");
    h.rt.app.navigate("/analytics?period=30d");
    h.render();
    let s = stack(&h);
    assert_eq!(
        screens(&h, s),
        ["home", "analytics 7d [] -", "analytics 30d [] -"]
    );
    // The same path and query again is a no-op.
    h.rt.app.navigate("/analytics?period=30d");
    assert!(h.render().is_empty());
    // Back (the browser's, a swipe) returns to the previous query.
    h.dispatch(UiEvent::Back(s, 2));
    assert_eq!(path(&h), "/analytics?period=7d");
}

#[test]
fn replace_query_rewrites_the_url_and_keeps_the_screen() {
    let mut h = query_app("/analytics?period=7d");
    let s = stack(&h);
    let screen = h.host.get(s).children[1];
    h.rt.app.replace_query([("period", "30d"), ("room", "2 b")]);
    let p = h.render().to_vec();
    assert_eq!(creates(&p), 0, "no screen is built");
    assert!(destroys(&p).is_empty());
    assert_eq!(h.host.get(s).children[1], screen);
    assert_eq!(h.host.get(s).text, "/analytics?period=30d&room=2%20b");
    assert_eq!(path(&h), "/analytics?period=30d&room=2%20b");
    // An empty list clears it.
    h.rt.app.replace_query(std::iter::empty::<(&str, &str)>());
    h.render();
    assert_eq!(path(&h), "/analytics");
    assert_eq!(h.host.get(s).children.len(), 2);
}

#[test]
fn navigation_state_reaches_only_the_next_screen() {
    let mut h = query_app("/");
    h.rt.app.navigate_with("/items/4", Saved("saved"));
    h.render();
    h.host.by_text("at item 4 saved");
    // A plain navigation afterwards gets nothing.
    h.rt.app.navigate("/items/5");
    h.render();
    h.host.by_text("at item 5 -");
    // After a save: replace the form, with a notice.
    h.rt.app.replace_with("/analytics?period=1d", Saved("done"));
    h.render();
    assert_eq!(
        screens(&h, stack(&h)),
        ["home", "item 4 saved", "analytics 1d [] done"]
    );
    // A value of another type is not handed over.
    h.rt.app.navigate_with("/items/6", 42u8);
    h.render();
    h.host.by_text("at item 6 -");
    // Dropped when nothing is built (already on top): not kept for later.
    h.rt.app.navigate_with("/items/6", Saved("lost"));
    h.render();
    h.rt.app.navigate("/items/7");
    h.render();
    h.host.by_text("at item 7 -");
}

#[test]
fn deep_links_to_a_tab_root_with_a_query_open_it_there() {
    let h = tab_app("/settings?section=privacy");
    let t = tabs_node(&h);
    assert_eq!(h.host.get(t).value, Some(1.));
    assert_eq!(path(&h), "/settings?section=privacy");
    // The tab's root, with its query: no plain root underneath.
    let settings = h.host.get(t).children[1];
    let s = h.host.get(settings).children[0];
    assert_eq!(screens(&h, s), ["settings"]);
}

#[test]
fn a_tab_root_with_a_query_switches_tab_then_pushes() {
    let mut h = tab_app("/");
    h.rt.app.navigate("/settings?section=privacy");
    h.render();
    assert_eq!(h.host.get(tabs_node(&h)).value, Some(1.));
    assert_eq!(path(&h), "/settings?section=privacy");
    h.rt.app.navigate("/settings");
    h.render();
    assert_eq!(path(&h), "/settings?section=privacy", "a tab switch only");
}

#[test]
fn the_header_can_be_hidden_for_every_screen_or_one() {
    struct Bare;
    impl Render for Bare {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            screen("Bare").bare()
        }
    }
    let mut h = Harness::at("/", |cx| {
        let router = routes(Router::new())
            .route("/bare", |cx, _| cx.new_entity(|_| Bare))
            .build(cx);
        cx.set_root(router);
    });
    let s = stack(&h);
    assert_eq!(h.host.get(s).value, Some(0.), "header shown");
    let home = h.host.get(s).children[0];
    assert_eq!(h.host.get(home).value, Some(0.), "a plain screen");
    h.rt.app.navigate("/bare");
    h.render();
    let bare = h.host.get(s).children[1];
    assert_eq!(h.host.get(bare).value, Some(1.), "a bare screen");

    let h = Harness::at("/", |cx| {
        let router = routes(Router::new()).header(false).build(cx);
        cx.set_root(router);
    });
    let flags = h.host.get(stack(&h)).value.unwrap() as u32;
    assert_eq!(flags, prism_core::STACK_NO_HEADER);
}

#[test]
fn a_headerless_sheet_stays_closable() {
    let mut h = Harness::at("/", |cx| {
        let router = routes(Router::new()).header(false).build(cx);
        cx.set_root(router);
    });
    h.rt.app.present("/todos/new");
    h.render();
    let m = modal(&h).expect("a sheet");
    let sheet_stack = h.host.get(m).children[0];
    let flags = h.host.get(sheet_stack).value.unwrap() as u32;
    assert_eq!(
        flags,
        prism_core::STACK_CLOSABLE | prism_core::STACK_NO_HEADER
    );
}

// --- The top screen (#288) and stacks of one screen (#289) ---

/// What the screens saw: `"<name> top|under"` at each render, `"<name>
/// shown"` when their screen became the top one.
#[derive(Default)]
struct Seen(Vec<String>);

struct Watched(String);

impl Render for Watched {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let line = format!(
            "{} {}",
            self.0,
            if cx.is_top_screen() { "top" } else { "under" }
        );
        cx.global_mut::<Seen>().0.push(line);
        screen(self.0.clone())
            .child(text(format!("at {}", self.0)))
            .child(div().link_root("/b").child(text("to b alone")))
    }
}

fn watched(cx: &mut App, name: &str) -> Entity<Watched> {
    let name = name.to_string();
    cx.new_entity(|cx| {
        cx.on_screen_shown(|this: &mut Watched, cx| {
            let line = format!("{} shown", this.0);
            cx.global_mut::<Seen>().0.push(line);
        });
        Watched(name)
    })
}

fn watched_app(path: &str, tabs: bool) -> Harness {
    Harness::at(path, move |cx| {
        let mut b = Router::new();
        if tabs {
            b = b.tab("A", "a", "/").tab("B", "b", "/b");
        }
        let router = b
            .route("/", |cx, _| watched(cx, "a"))
            .route("/b", |cx, _| watched(cx, "b"))
            .route("/b/:n", |cx, p| {
                let name = format!("b{}", p.get("n").unwrap());
                watched(cx, &name)
            })
            .route("/c", |cx, _| watched(cx, "c"))
            .build(cx);
        cx.set_root(router);
    })
}

fn seen(h: &mut Harness) -> Vec<String> {
    std::mem::take(&mut h.rt.app.global_mut::<Seen>().0)
}

#[test]
fn a_screen_knows_it_is_the_top_one() {
    let mut h = watched_app("/", false);
    assert_eq!(seen(&mut h), ["a shown", "a top"]);
    h.rt.app.navigate("/c");
    h.render();
    assert_eq!(seen(&mut h), ["c shown", "c top"]);
    assert!(h.rt.app.top_screen().is_some());
    // A global change re-renders the screen kept under it too.
    h.rt.app.refresh();
    h.render();
    let mut lines = seen(&mut h);
    lines.sort();
    assert_eq!(lines, ["a under", "c top"]);
    // Back reveals `a`: it renders again, knowing it is on top.
    h.rt.app.back();
    h.render();
    assert_eq!(seen(&mut h), ["a shown", "a top"]);
    // A sheet over it is the top screen; closing it shows `a` again.
    h.rt.app.present("/b");
    h.render();
    assert_eq!(seen(&mut h), ["b shown", "b top"]);
    h.rt.app.refresh();
    h.render();
    let mut lines = seen(&mut h);
    lines.sort();
    assert_eq!(lines, ["a under", "b top"]);
    h.rt.app.dismiss();
    h.render();
    assert_eq!(seen(&mut h), ["a shown", "a top"]);
}

#[test]
fn a_tab_screen_is_on_top_while_its_tab_is_selected() {
    let mut h = watched_app("/", true);
    assert_eq!(seen(&mut h), ["a shown", "a top"]);
    let tabs = h.host.find(|n| n.kind == NodeKind::Tabs).unwrap();
    h.dispatch(UiEvent::Change(tabs, 1.));
    let lines = seen(&mut h);
    assert!(lines.contains(&"b shown".to_string()), "{lines:?}");
    assert!(lines.contains(&"b top".to_string()), "{lines:?}");
    h.rt.app.refresh();
    h.render();
    let mut lines = seen(&mut h);
    lines.sort();
    assert_eq!(lines, ["a under", "b top"]);
    // Back to the first tab: `a` is shown again.
    h.dispatch(UiEvent::Change(tabs, 0.));
    assert_eq!(seen(&mut h), ["a shown", "a top"]);
}

#[test]
fn without_a_router_every_entity_is_on_top() {
    struct Solo;
    impl Render for Solo {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            text(cx.is_top_screen().to_string())
        }
    }
    let h = Harness::new(|cx| {
        let e = cx.new_entity(|_| Solo);
        cx.set_root(e);
    });
    h.host.by_text("true");
    assert_eq!(h.rt.app.top_screen(), None);
    assert_eq!(h.rt.app.stack_depth(), 0);
}

#[test]
fn navigate_root_leaves_one_screen_without_building_the_root() {
    let mut h = watched_app("/c", false);
    let s = stack(&h);
    assert_eq!(screens(&h, s), ["a", "c"]);
    seen(&mut h);
    h.rt.app.navigate_root("/b");
    let p = h.render().to_vec();
    assert_eq!(screens(&h, s), ["b"]);
    assert_eq!(h.rt.app.stack_depth(), 1);
    assert_eq!(path(&h), "/b");
    // `a` and `c` are released; no root screen is built underneath.
    assert_eq!(destroys(&p).len(), 2 * 4);
    assert_eq!(seen(&mut h), ["b shown", "b top"]);
    // Pushed over it, then back to it alone: the same screen, kept.
    let bottom = h.host.get(s).children[0];
    h.rt.app.navigate("/b/1");
    h.render();
    assert_eq!(h.rt.app.stack_depth(), 2);
    seen(&mut h);
    h.rt.app.navigate_root("/b");
    h.render();
    assert_eq!(screens(&h, s), ["b"]);
    assert_eq!(h.host.get(s).children[0], bottom);
    assert_eq!(seen(&mut h), ["b shown", "b top"]);
    // Back has nowhere to go.
    h.rt.app.back();
    h.render();
    assert_eq!(screens(&h, s), ["b"]);
}

#[test]
fn link_root_follows_the_link_alone_in_the_stack() {
    let mut h = watched_app("/c", false);
    let s = stack(&h);
    let c = *h.host.get(s).children.last().unwrap();
    // The `c` screen's link (the screen under it has one too).
    fn link_in(h: &Harness, id: u64) -> Option<u64> {
        let n = h.host.get(id);
        if n.clickable && h.host.child_texts(id).iter().any(|t| t == "to b alone") {
            return Some(id);
        }
        n.children.iter().find_map(|c| link_in(h, *c))
    }
    let link = link_in(&h, c).expect("the link");
    h.dispatch(UiEvent::Click(link));
    h.render();
    assert_eq!(screens(&h, s), ["b"]);
    assert_eq!(path(&h), "/b");
}

#[test]
fn navigate_root_opens_in_the_tab_the_path_belongs_to() {
    let mut h = watched_app("/c", true);
    assert_eq!(h.rt.app.stack_depth(), 2);
    h.rt.app.navigate_root("/b/2");
    h.render();
    let tabs = h.host.find(|n| n.kind == NodeKind::Tabs).unwrap();
    assert_eq!(h.host.get(tabs).value, Some(1.));
    assert_eq!(path(&h), "/b/2");
    assert_eq!(h.rt.app.stack_depth(), 1);
    // The first tab keeps its screens.
    h.dispatch(UiEvent::Change(tabs, 0.));
    assert_eq!(path(&h), "/c");
    assert_eq!(h.rt.app.stack_depth(), 2);
}
