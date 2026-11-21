use std::sync::Arc;

use crate::prelude::*;
use crate::{AppInit, NodeKind, Patch, Runtime, UiEvent};

fn find_text(patches: &[Patch], text: &str) -> u64 {
    patches
        .iter()
        .find_map(|p| match p {
            Patch::SetText { id, text: t } if t == text => Some(*id),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no node with text {text:?}"))
}

fn boot(init: impl Fn(&mut App) + Send + Sync + 'static) -> (Runtime, Vec<Patch>) {
    let init: AppInit = Arc::new(init);
    let mut rt = Runtime::new(&init);
    let first = rt.render();
    (rt, first)
}

// --- A flat app -------------------------------------------------------------

struct Flat {
    count: i32,
    items: Vec<&'static str>,
}

impl Render for Flat {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(text(format!("Count: {}", self.count)))
            .child(button("+").on_click(cx.listener(|this, _, _| this.count += 1)))
            .child(button("noop").on_click(cx.listener(|_, _, _| {})))
            .child(div().children(self.items.iter().map(|i| text(*i).key(*i))))
    }
}

fn flat(items: Vec<&'static str>) -> (Runtime, Vec<Patch>) {
    boot(move |cx| {
        let items = items.clone();
        let root = cx.new_entity(|_| Flat { count: 0, items });
        cx.set_root(root);
    })
}

#[test]
fn first_render_creates_and_mounts_tree() {
    let (_, patches) = flat(vec!["a"]);
    let creates = patches
        .iter()
        .filter(|p| matches!(p, Patch::Create { .. }))
        .count();
    assert_eq!(creates, 6);
    assert!(patches.iter().any(|p| matches!(p, Patch::SetRoot { .. })));
}

#[test]
fn listener_auto_notifies_and_patches_only_the_text() {
    let (mut rt, first) = flat(vec![]);
    let plus = find_text(&first, "+");
    let patches = rt.dispatch(UiEvent::Click(plus));
    assert_eq!(
        patches,
        vec![Patch::SetText {
            id: find_text(&first, "Count: 0"),
            text: "Count: 1".into()
        }]
    );
}

#[test]
fn unchanged_state_produces_no_patches() {
    let (mut rt, first) = flat(vec![]);
    assert!(
        rt.dispatch(UiEvent::Click(find_text(&first, "noop")))
            .is_empty()
    );
    assert!(rt.dispatch(UiEvent::Click(999)).is_empty());
}

#[test]
fn keyed_children_are_moved_not_recreated() {
    let (mut rt, first) = flat(vec!["a", "b"]);
    let root = first_root::<Flat>(&rt);
    rt.app.update(&root, |this, cx| {
        this.items.reverse();
        cx.notify();
    });
    let patches = rt.render();
    assert!(
        patches
            .iter()
            .all(|p| !matches!(p, Patch::Create { .. } | Patch::Destroy { .. }))
    );
    let a = find_text(&first, "a");
    let b = find_text(&first, "b");
    assert!(
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetChildren { children, .. } if children == &vec![b, a]))
    );
}

fn first_root<T: 'static>(rt: &Runtime) -> Entity<T> {
    Entity::from_id(rt.app.root().expect("root").id())
}

// --- Nested entities --------------------------------------------------------

struct Parent {
    child: Entity<Child>,
    title: String,
    renders: u32,
}

struct Child {
    count: i32,
    renders: u32,
}

impl Render for Parent {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        div()
            .child(text(self.title.clone()))
            .child(button("rename").on_click(cx.listener(|this, _, _| this.title.push('!'))))
            .child(self.child)
    }
}

impl Render for Child {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        div()
            .child(text(format!("child {}", self.count)))
            .child(button("inc").on_click(cx.listener(|this, _, _| this.count += 1)))
    }
}

fn nested() -> (Runtime, Vec<Patch>) {
    boot(|cx| {
        let root = cx.new_entity(|cx| Parent {
            child: cx.new(|_| Child {
                count: 0,
                renders: 0,
            }),
            title: "parent".into(),
            renders: 0,
        });
        cx.set_root(root);
    })
}

fn renders(rt: &Runtime) -> (u32, u32) {
    let parent: Entity<Parent> = first_root(rt);
    let p = rt.app.read(&parent);
    (p.renders, rt.app.read(&p.child).renders)
}

#[test]
fn child_update_does_not_render_parent() {
    let (mut rt, first) = nested();
    assert_eq!(renders(&rt), (1, 1));
    let patches = rt.dispatch(UiEvent::Click(find_text(&first, "inc")));
    assert_eq!(renders(&rt), (1, 2));
    assert_eq!(rt.last_render_count(), 1);
    assert_eq!(
        patches,
        vec![Patch::SetText {
            id: find_text(&first, "child 0"),
            text: "child 1".into()
        }]
    );
}

#[test]
fn parent_update_reuses_clean_child() {
    let (mut rt, first) = nested();
    let patches = rt.dispatch(UiEvent::Click(find_text(&first, "rename")));
    assert_eq!(renders(&rt), (2, 1));
    assert_eq!(patches.len(), 1, "{patches:?}");
}

#[test]
fn child_root_kind_change_is_swapped_in_parent() {
    struct Swap(bool);
    impl Render for Swap {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            if self.0 {
                text("now text").into_element()
            } else {
                div().into_element()
            }
        }
    }
    struct Host(Entity<Swap>);
    impl Render for Host {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div().child(self.0)
        }
    }
    let (mut rt, first) = boot(|cx| {
        let root = cx.new_entity(|cx| Host(cx.new(|_| Swap(false))));
        cx.set_root(root);
    });
    let host: Entity<Host> = first_root(&rt);
    let swap = rt.app.read(&host).0;
    swap.update(&mut rt.app, |s, cx| {
        s.0 = true;
        cx.notify();
    });
    let patches = rt.render();
    // Second View created = the Swap entity's root (the first is the Host's).
    let old_child = first
        .iter()
        .filter_map(|p| match p {
            Patch::Create {
                id,
                kind: NodeKind::View,
            } => Some(*id),
            _ => None,
        })
        .nth(1)
        .unwrap();
    assert!(patches.contains(&Patch::Destroy { id: old_child }));
    assert!(
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetChildren { children, .. } if children.len() == 1 && children[0] != old_child))
    );
}

#[test]
fn observers_rerender_dependents() {
    struct Source(i32);
    impl Render for Source {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            button(format!("src {}", self.0)).on_click(cx.listener(|this, _, _| this.0 += 1))
        }
    }
    struct Mirror {
        seen: i32,
    }
    impl Render for Mirror {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            text(format!("mirror {}", self.seen))
        }
    }
    struct Shell(Entity<Source>, Entity<Mirror>);
    impl Render for Shell {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div().child(self.0).child(self.1)
        }
    }
    let (mut rt, first) = boot(|cx| {
        let root = cx.new_entity(|cx| {
            let source = cx.new(|_| Source(0));
            let mirror = cx.new(|cx| {
                cx.observe(&source, |this: &mut Mirror, src, cx| {
                    this.seen = src.read(cx).0;
                });
                Mirror { seen: 0 }
            });
            Shell(source, mirror)
        });
        cx.set_root(root);
    });
    let patches = rt.dispatch(UiEvent::Click(find_text(&first, "src 0")));
    assert!(patches.contains(&Patch::SetText {
        id: find_text(&first, "src 0"),
        text: "src 1".into()
    }));
    assert!(patches.contains(&Patch::SetText {
        id: find_text(&first, "mirror 0"),
        text: "mirror 1".into()
    }));
    assert_eq!(
        rt.last_render_count(),
        2,
        "only source and mirror re-render"
    );
}

// --- Router -----------------------------------------------------------------

struct Home;
impl Render for Home {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen("Home").child(button("open 7").on_click(navigate_to("/items/7")))
    }
}

struct Item(u64);
impl Render for Item {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen(format!("Item {}", self.0)).child(button("back").on_click(go_back()))
    }
}

fn routed(path: &str) -> (Runtime, Vec<Patch>) {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let router = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| Home))
            .route("/items/:id", |cx, p| {
                let id = p.parse("id").unwrap_or(0);
                cx.new_entity(move |_| Item(id))
            })
            .build(cx);
        cx.set_root(router);
    });
    let mut rt = Runtime::with_path(&init, path);
    let first = rt.render();
    (rt, first)
}

fn stack_of(patches: &[Patch]) -> (u64, usize, String) {
    let id = patches
        .iter()
        .find_map(|p| match p {
            Patch::Create {
                id,
                kind: NodeKind::Stack,
            } => Some(*id),
            _ => None,
        })
        .expect("a stack");
    let depth = patches
        .iter()
        .rev()
        .find_map(|p| match p {
            Patch::SetChildren { id: s, children } if *s == id => Some(children.len()),
            _ => None,
        })
        .unwrap_or(0);
    let path = patches
        .iter()
        .rev()
        .find_map(|p| match p {
            Patch::SetText { id: s, text } if *s == id => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    (id, depth, path)
}

#[test]
fn router_pushes_and_pops() {
    let (mut rt, first) = routed("/");
    let (stack, depth, path) = stack_of(&first);
    assert_eq!((depth, path.as_str()), (1, "/"));

    let pushed = rt.dispatch(UiEvent::Click(find_text(&first, "open 7")));
    assert!(pushed.contains(&Patch::SetText {
        id: stack,
        text: "/items/7".into()
    }));
    assert!(pushed.iter().any(
        |p| matches!(p, Patch::SetChildren { id, children } if *id == stack && children.len() == 2)
    ));
    assert_eq!(rt.app.current_path(), Some("/items/7"));
    find_text(&pushed, "Item 7");

    let popped = rt.dispatch(UiEvent::Click(find_text(&pushed, "back")));
    assert!(popped.contains(&Patch::SetText {
        id: stack,
        text: "/".into()
    }));
    assert!(popped.iter().any(|p| matches!(p, Patch::Destroy { .. })));
}

#[test]
fn deep_link_opens_home_underneath() {
    let (_, first) = routed("/items/3?x=1");
    let (_, depth, path) = stack_of(&first);
    assert_eq!((depth, path.as_str()), (2, "/items/3?x=1"));
    find_text(&first, "Item 3");
}

#[test]
fn native_back_truncates_the_stack() {
    let (mut rt, first) = routed("/items/3");
    let (stack, _, _) = stack_of(&first);
    let patches = rt.dispatch(UiEvent::Back(stack, 1));
    assert!(patches.contains(&Patch::SetText {
        id: stack,
        text: "/".into()
    }));
    assert_eq!(rt.app.current_path(), Some("/"));
    // Going "back" below the root is ignored.
    assert!(rt.dispatch(UiEvent::Back(stack, 0)).is_empty());
}

/// A router on `platform`, counting how many times `/` is built.
fn routed_on(
    platform: crate::Platform,
    path: &str,
) -> (Runtime, Vec<Patch>, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let homes = Arc::new(AtomicUsize::new(0));
    let count = homes.clone();
    let init: AppInit = Arc::new(move |cx: &mut App| {
        let count = count.clone();
        let router = Router::new()
            .route("/", move |cx, _| {
                count.fetch_add(1, Ordering::SeqCst);
                cx.new_entity(|_| Home)
            })
            .route("/items/:id", |cx, p| {
                let id = p.parse("id").unwrap_or(0);
                cx.new_entity(move |_| Item(id))
            })
            .build(cx);
        cx.set_root(router);
    });
    let mut rt = Runtime::with_options(
        &init,
        crate::RuntimeOptions {
            path: path.into(),
            platform: Some(platform),
            ..Default::default()
        },
    );
    let first = rt.render();
    (rt, first, homes)
}

#[test]
fn web_deep_links_build_the_screen_underneath_when_back_reaches_it() {
    use std::sync::atomic::Ordering;
    let (mut rt, first, homes) = routed_on(crate::Platform::Web, "/items/3");
    // Two screens (Back and the browser's history see the depth), but `/`
    // is neither built nor rendered.
    let (stack, depth, _) = stack_of(&first);
    assert_eq!(depth, 2);
    assert_eq!(homes.load(Ordering::SeqCst), 0);
    assert!(
        !first
            .iter()
            .any(|p| matches!(p, Patch::SetText { text, .. } if text == "Home"))
    );
    find_text(&first, "Item 3");
    // Back builds it, once.
    let back = rt.dispatch(UiEvent::Back(stack, 1));
    assert_eq!(homes.load(Ordering::SeqCst), 1);
    assert_eq!(rt.app.current_path(), Some("/"));
    find_text(&back, "Home");
    find_text(&back, "open 7");
    // Navigating away and back reuses it.
    let again = rt.dispatch(UiEvent::Click(find_text(&back, "open 7")));
    rt.dispatch(UiEvent::Click(find_text(&again, "back")));
    assert_eq!(homes.load(Ordering::SeqCst), 1);
    assert_eq!(rt.app.current_path(), Some("/"));
}

#[test]
fn web_deep_links_build_the_screen_underneath_on_app_back() {
    use std::sync::atomic::Ordering;
    let (mut rt, first, homes) = routed_on(crate::Platform::Web, "/items/3");
    let back = rt.dispatch(UiEvent::Click(find_text(&first, "back")));
    assert_eq!(homes.load(Ordering::SeqCst), 1);
    find_text(&back, "Home");
}

#[test]
fn native_deep_links_build_the_screen_underneath_up_front() {
    use std::sync::atomic::Ordering;
    // Native Back shows it (iOS names it in Back, the swipe reveals it).
    let (_, first, homes) = routed_on(crate::Platform::Ios, "/items/3");
    assert_eq!(homes.load(Ordering::SeqCst), 1);
    find_text(&first, "Home");
}

#[test]
fn browser_forward_navigates() {
    let (mut rt, first) = routed("/");
    let (stack, _, _) = stack_of(&first);
    rt.dispatch(UiEvent::Navigate(stack, "/items/9".into()));
    assert_eq!(rt.app.current_path(), Some("/items/9"));
}

#[test]
fn unknown_route_shows_not_found() {
    let (_, first) = routed("/nope");
    find_text(&first, "Nothing at /nope");
}

// --- Accessibility, toggles, tabs, lazy, globals -----------------------------

use crate::{Accessibility, ChangeEvent, Role, lazy, toggle};

fn a11y_of(patches: &[Patch], id: u64) -> Option<Accessibility> {
    patches.iter().find_map(|p| match p {
        Patch::SetAccessibility { id: i, a11y } if *i == id => Some(a11y.clone()),
        _ => None,
    })
}

struct A11yView {
    wifi: bool,
}

impl Render for A11yView {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(text("Settings").heading(1))
            .child(
                div()
                    .on_click(cx.listener(|_, _, _| {}))
                    .child(text("Open"))
                    .child(text("›").accessibility_hidden())
                    .child(text("Profile")),
            )
            .child(
                toggle(self.wifi)
                    .accessibility_label("Wi-Fi")
                    .on_change(cx.listener(|this, e: &ChangeEvent, _| this.wifi = e.checked())),
            )
            .child(text_input().placeholder("Search"))
    }
}

#[test]
fn accessibility_defaults_are_resolved_in_core() {
    let (mut rt, first) = boot(|cx| {
        let root = cx.new_entity(|_| A11yView { wifi: false });
        cx.set_root(root);
    });
    let heading = a11y_of(&first, find_text(&first, "Settings")).unwrap();
    assert_eq!(heading.role, Some(Role::Heading(1)));

    // A clickable div becomes a button named by its visible text.
    let row = first
        .iter()
        .find_map(|p| match p {
            Patch::SetAccessibility { id, a11y } if a11y.role == Some(Role::Button) => {
                Some((*id, a11y.clone()))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(row.1.label.as_deref(), Some("Open, Profile"));

    let toggle_id = first
        .iter()
        .find_map(|p| match p {
            Patch::Create {
                id,
                kind: NodeKind::Toggle,
            } => Some(*id),
            _ => None,
        })
        .unwrap();
    let t = a11y_of(&first, toggle_id).unwrap();
    assert_eq!(
        (t.role, t.checked, t.label.as_deref()),
        (Some(Role::Switch), Some(false), Some("Wi-Fi"))
    );

    let patches = rt.dispatch(UiEvent::Change(toggle_id, 1.));
    assert!(patches.contains(&Patch::SetValue {
        id: toggle_id,
        value: 1.
    }));
    assert_eq!(a11y_of(&patches, toggle_id).unwrap().checked, Some(true));

    let input = first
        .iter()
        .find_map(|p| match p {
            Patch::Create {
                id,
                kind: NodeKind::TextInput,
            } => Some(*id),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        a11y_of(&first, input).unwrap().label.as_deref(),
        Some("Search")
    );
}

struct Sidebar {
    page: usize,
}

impl Render for Sidebar {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let item = |i: usize, name: &str, cx: &mut Context<Self>| {
            div()
                .on_click(cx.listener(move |this, _, _| this.page = i))
                .accessibility_role(Role::Link)
                .accessibility_current(self.page == i)
                .accessibility_selected(self.page == i)
                .child(text(name))
        };
        div()
            .accessibility_role(Role::Navigation)
            .child(item(0, "Home", cx))
            .child(item(1, "Settings", cx))
    }
}

#[test]
fn the_current_item_of_a_navigation_is_described_and_moves() {
    let (mut rt, first) = boot(|cx| {
        let root = cx.new_entity(|_| Sidebar { page: 0 });
        cx.set_root(root);
    });
    let item = |patches: &[Patch], name: &str| {
        patches
            .iter()
            .find_map(|p| match p {
                Patch::SetAccessibility { id, a11y } if a11y.label.as_deref() == Some(name) => {
                    Some((*id, a11y.clone()))
                }
                _ => None,
            })
            .unwrap()
    };
    let (home, a) = item(&first, "Home");
    assert!(a.current && a.selected == Some(true) && a.is_selected());
    let (settings, a) = item(&first, "Settings");
    assert!(!a.current && a.selected == Some(false) && !a.is_selected());

    // Going to Settings moves the state: both items get a new description.
    let patches = rt.dispatch(UiEvent::Click(settings));
    assert!(!a11y_of(&patches, home).unwrap().current);
    assert!(a11y_of(&patches, settings).unwrap().current);
    // Same page again: nothing to send.
    assert!(
        rt.dispatch(UiEvent::Click(settings))
            .iter()
            .all(|p| !matches!(p, Patch::SetAccessibility { .. }))
    );
}

#[test]
fn checked_tabs_and_toggle_buttons_read_as_selected() {
    let checked = |role| Accessibility {
        role: Some(role),
        checked: Some(true),
        ..Default::default()
    };
    assert!(checked(Role::Tab).is_selected());
    assert!(checked(Role::Radio).is_selected());
    assert!(
        !checked(Role::Switch).is_selected(),
        "a switch is on, not selected"
    );
    assert!(!Accessibility::default().is_selected());
}

struct Themed;
#[derive(Default)]
struct Brand(&'static str);

impl Render for Themed {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(lazy(|cx| text(cx.global::<Brand>().0)))
    }
}

#[test]
fn lazy_elements_read_globals_and_globals_rerender() {
    let (mut rt, first) = boot(|cx| {
        cx.set_global(Brand("blue"));
        let root = cx.new_entity(|_| Themed);
        cx.set_root(root);
    });
    let id = find_text(&first, "blue");
    rt.app.set_global(Brand("red"));
    let patches = rt.render();
    assert_eq!(
        patches,
        vec![Patch::SetText {
            id,
            text: "red".into()
        }]
    );
}

fn tabbed(path: &str) -> (Runtime, Vec<Patch>) {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let router = Router::new()
            .tab("Home", "house", "/")
            .tab("Settings", "gear", "/settings")
            .route("/", |cx, _| cx.new_entity(|_| Home))
            .route("/items/:id", |cx, p| {
                let id = p.parse("id").unwrap_or(0);
                cx.new_entity(move |_| Item(id))
            })
            .route("/settings", |cx, _| cx.new_entity(|_| Item(99)))
            .route("/settings/:id", |cx, p| {
                let id = p.parse("id").unwrap_or(0);
                cx.new_entity(move |_| Item(id))
            })
            .build(cx);
        cx.set_root(router);
    });
    let mut rt = Runtime::with_path(&init, path);
    let first = rt.render();
    (rt, first)
}

fn tabs_id(patches: &[Patch]) -> u64 {
    patches
        .iter()
        .find_map(|p| match p {
            Patch::Create {
                id,
                kind: NodeKind::Tabs,
            } => Some(*id),
            _ => None,
        })
        .unwrap()
}

#[test]
fn tabs_keep_one_stack_each() {
    let (mut rt, first) = tabbed("/");
    let tabs = tabs_id(&first);
    assert!(first.contains(&Patch::SetValue {
        id: tabs,
        value: 0.
    }));
    // Both tabs are in the bar from the start; tab 1 renders when selected.
    assert_eq!(count_created(&first, NodeKind::Tab), 2);

    rt.dispatch(UiEvent::Click(find_text(&first, "open 7")));
    assert_eq!(rt.app.current_path(), Some("/items/7"));

    let switched = rt.dispatch(UiEvent::Change(tabs, 1.));
    assert!(switched.contains(&Patch::SetValue {
        id: tabs,
        value: 1.
    }));
    assert_eq!(rt.app.current_path(), Some("/settings"));

    // Back on tab 0, its stack is intact.
    rt.dispatch(UiEvent::Change(tabs, 0.));
    assert_eq!(rt.app.current_path(), Some("/items/7"));
}

#[test]
fn deep_link_selects_the_matching_tab_and_navigating_to_a_root_switches_tab() {
    let (mut rt, first) = tabbed("/settings/5");
    let tabs = tabs_id(&first);
    assert!(first.contains(&Patch::SetValue {
        id: tabs,
        value: 1.
    }));
    assert_eq!(rt.app.current_path(), Some("/settings/5"));
    rt.app.navigate("/");
    rt.render();
    assert_eq!(rt.app.current_path(), Some("/"));
}

// --- Lazy tabs ---------------------------------------------------------------

fn has_text(patches: &[Patch], text: &str) -> bool {
    patches
        .iter()
        .any(|p| matches!(p, Patch::SetText { text: t, .. } if t == text))
}

fn count_created(patches: &[Patch], kind: NodeKind) -> usize {
    patches
        .iter()
        .filter(|p| matches!(p, Patch::Create { kind: k, .. } if *k == kind))
        .count()
}

/// A screen with a counter, to check a tab keeps its state.
struct Tally(u32);
impl Render for Tally {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        screen("Tally").child(
            button(format!("count {}", self.0))
                .on_click(cx.listener(|this: &mut Self, _: &ClickEvent, _| this.0 += 1)),
        )
    }
}

/// The router of [`lazy_tabs`], kept to ask it which tabs are rendered.
struct LazyRouter(Entity<Router>);

fn lazy_tabs(path: &str, eager: bool) -> (Runtime, Vec<Patch>) {
    let init: AppInit = Arc::new(move |cx: &mut App| {
        let mut router = Router::new()
            .tab("Home", "house", "/")
            .tab("Tally", "number", "/tally");
        if eager {
            router = router.eager();
        }
        let router = router
            .tab("Settings", "gear", "/settings")
            .route("/", |cx, _| cx.new_entity(|_| Home))
            .route("/tally", |cx, _| cx.new_entity(|_| Tally(0)))
            .route("/settings", |cx, _| cx.new_entity(|_| Item(99)))
            .route("/settings/:id", |cx, p| {
                let id = p.parse("id").unwrap_or(0);
                cx.new_entity(move |_| Item(id))
            })
            .build(cx);
        cx.set_global(LazyRouter(router));
        cx.set_root(router);
    });
    let mut rt = Runtime::with_path(&init, path);
    let first = rt.render();
    (rt, first)
}

fn rendered(rt: &Runtime, tab: usize) -> bool {
    let router = rt.app.global::<LazyRouter>().0;
    rt.app.read(&router).tab_rendered(tab)
}

#[test]
fn unvisited_tabs_have_an_entry_and_an_empty_panel() {
    let (rt, first) = lazy_tabs("/", false);
    // Every tab is in the bar, with its title and icon...
    assert_eq!(count_created(&first, NodeKind::Tab), 3);
    for title in ["Home", "Tally", "Settings"] {
        find_text(&first, title);
    }
    // ...and a stack each, but only the selected tab has a screen.
    assert_eq!(count_created(&first, NodeKind::Stack), 3);
    assert_eq!(count_created(&first, NodeKind::Screen), 1);
    assert!(!has_text(&first, "count 0"));
    assert!(!has_text(&first, "Item 99"));
    assert!(rendered(&rt, 0));
    assert!(!rendered(&rt, 1));
    assert!(!rendered(&rt, 2));
}

#[test]
fn selecting_a_tab_renders_it_and_it_stays_rendered() {
    let (mut rt, first) = lazy_tabs("/", false);
    let tabs = tabs_id(&first);

    let opened = rt.dispatch(UiEvent::Change(tabs, 1.));
    assert!(opened.contains(&Patch::SetValue {
        id: tabs,
        value: 1.
    }));
    let count = find_text(&opened, "count 0");
    assert_eq!(count_created(&opened, NodeKind::Screen), 1);
    assert!(rendered(&rt, 1));
    assert!(!rendered(&rt, 2));

    // Its state and nodes stay while another tab shows.
    rt.dispatch(UiEvent::Click(count));
    rt.dispatch(UiEvent::Change(tabs, 0.));
    assert!(rendered(&rt, 1));
    let back = rt.dispatch(UiEvent::Change(tabs, 1.));
    assert_eq!(count_created(&back, NodeKind::Screen), 0);
    assert!(!has_text(&back, "count 0"));
    let more = rt.dispatch(UiEvent::Click(count));
    assert_eq!(
        more,
        vec![Patch::SetText {
            id: count,
            text: "count 2".into()
        }]
    );
}

#[test]
fn navigating_to_a_tab_root_renders_that_tab() {
    let (mut rt, _) = lazy_tabs("/", false);
    rt.app.navigate("/settings");
    let patches = rt.render();
    find_text(&patches, "Item 99");
    assert!(rendered(&rt, 2));
}

#[test]
fn deep_links_render_their_tab_only() {
    let (rt, first) = lazy_tabs("/settings/5", false);
    let tabs = tabs_id(&first);
    assert!(first.contains(&Patch::SetValue {
        id: tabs,
        value: 2.
    }));
    // The linked screen and the tab's root underneath.
    find_text(&first, "Item 5");
    find_text(&first, "Item 99");
    assert!(!has_text(&first, "open 7"));
    assert!(!has_text(&first, "count 0"));
    assert!(!rendered(&rt, 0));
    assert!(rendered(&rt, 2));
}

#[test]
fn eager_tabs_render_up_front() {
    let (rt, first) = lazy_tabs("/", true);
    find_text(&first, "count 0");
    // The tabs not marked eager stay lazy.
    assert!(!has_text(&first, "Item 99"));
    assert_eq!(count_created(&first, NodeKind::Screen), 2);
    assert!(rendered(&rt, 1));
    assert!(!rendered(&rt, 2));
}

#[test]
fn on_watchos_the_pages_next_to_the_selected_tab_render_too() {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let router = Router::new()
            .tab("Home", "house", "/")
            .tab("Tally", "number", "/tally")
            .tab("Settings", "gear", "/settings")
            .route("/", |cx, _| cx.new_entity(|_| Home))
            .route("/tally", |cx, _| cx.new_entity(|_| Tally(0)))
            .route("/settings", |cx, _| cx.new_entity(|_| Item(99)))
            .build(cx);
        cx.set_root(router);
    });
    let mut rt = Runtime::with_options(
        &init,
        crate::RuntimeOptions {
            path: "/".into(),
            platform: Some(Platform::Watchos),
            ..Default::default()
        },
    );
    // The Digital Crown shows the next page while it scrolls.
    let first = rt.render();
    find_text(&first, "count 0");
    assert!(!has_text(&first, "Item 99"));
    let next = rt.dispatch(UiEvent::Change(tabs_id(&first), 1.));
    find_text(&next, "Item 99");
}

#[test]
fn a_reset_makes_visited_tabs_lazy_again() {
    let (mut rt, first) = lazy_tabs("/", false);
    let tabs = tabs_id(&first);
    rt.dispatch(UiEvent::Change(tabs, 1.));
    rt.app.reset("/");
    rt.render();
    assert!(rendered(&rt, 0));
    assert!(!rendered(&rt, 1));
}

// --- Surfaces: widgets, live activities, notifications, links ----------------

use crate::{
    Activity, Notification, PlatformRequest, Widget, WidgetFamily, adaptive, image, progress,
    surfaces::snapshot_one,
};

#[test]
fn snapshots_serialize_compact_json_with_adaptive_colors() {
    let mut app = App::new();
    let node = snapshot_one(
        &mut app,
        div()
            .flex_row()
            .gap(8.)
            .bg(adaptive(rgb(0xFFFFFF), rgb(0x000000)))
            .link("/todos/1")
            .child(image("checkmark.circle.fill").text_color(rgb(0x00FF00)))
            .child(text("Hi \"you\"").bold())
            .child(progress(0.5)),
    );
    let json = node.to_json();
    assert_eq!(
        json,
        r##"{"k":"v","d":1,"g":8,"bg":["#FFFFFFFF","#000000FF"],"l":"/todos/1","c":[{"k":"i","fg":"#00FF00FF","s":"checkmark.circle.fill","ah":1},{"k":"t","fw":3,"s":"Hi \"you\""},{"k":"p","v":0.5}]}"##
    );
}

#[test]
fn snapshots_resolve_svg_current_color_against_the_text_color() {
    // #51: widget renderers read hex colors; `currentColor` becomes the
    // node's text color, else the adaptive label color.
    let icon =
        r#"<svg width="4" height="4"><rect width="4" height="4" fill="currentColor"/></svg>"#;
    let mut app = App::new();
    let tinted = snapshot_one(
        &mut app,
        div().child(crate::svg(icon).text_color(rgb(0x00FF00))),
    );
    let json = tinted.to_json();
    assert!(json.contains(r#""s":"i0,0\nf#00FF00|"#), "{json}");
    let plain = snapshot_one(&mut app, div().child(crate::svg(icon)));
    let json = plain.to_json();
    assert!(json.contains("f#000000/#FFFFFF|"), "{json}");
    // `tint` recolors an SVG that hard-codes its color.
    let fixed = r##"<svg width="4" height="4"><rect width="4" height="4" fill="#123456"/></svg>"##;
    let t = snapshot_one(&mut app, div().child(crate::svg(fixed).tint(rgb(0xFF0000))));
    assert!(t.to_json().contains("f#FF0000|"), "{}", t.to_json());
}

#[test]
fn snapshots_carry_shapes_compactly_with_margins() {
    use crate::{Drawing, Path, shape};
    let mut app = App::new();
    let line = Path::smooth([(0., 1.), (0.5, 0.25), (1., 0.6)])
        .stroke(adaptive(rgb(0x5B5BD6), rgb(0x8B8BFF)), 2.)
        .round()
        .dashed(4., 2.);
    let ring = Path::arc(0.5, 0.5, 0.5, 0., 270.).stroke(rgba(0xFF0000, 0.5), 8.);
    let node = snapshot_one(
        &mut app,
        div().child(
            shape()
                .stroke_inset(2., 3.)
                .path(line.clone())
                .path(ring.clone())
                .h(px(40.))
                .w_full()
                .with_style(|s| s.margin.top = -40.)
                .label("Trend"),
        ),
    );
    let json = node.to_json();
    let sh = &node.children[0];
    assert!(json.contains(r#""k":"sh""#), "{json}");
    assert!(json.contains(r#""m":[-40,0,0,0]"#), "{json}");
    assert!(json.contains(r#""al":"Trend""#), "{json}");
    // Opaque colors lose their alpha, numbers their leading zero.
    assert!(
        json.contains(r#"s#5B5BD6/#8B8BFF w2 r d4,2|M 0 1 C .167 .75 .333 .25 .5 .25"#),
        "{json}"
    );
    assert!(json.contains("s#FF000080 w8|A .5 .5 .5 0 270"), "{json}");
    // The compact form still decodes to the same drawing (3 decimals).
    let compact = super::surfaces::compact_drawing(&sh.text, 3);
    let (a, b) = (Drawing::decode(&compact), Drawing::decode(&sh.text));
    assert_eq!(a.inset, (2., 3.));
    assert_eq!(a.paths.len(), 2);
    for (pa, pb) in a.paths.iter().zip(&b.paths) {
        assert_eq!(
            (pa.fill, pa.stroke, pa.width, pa.dash),
            (pb.fill, pb.stroke, pb.width, pb.dash)
        );
        assert_eq!(pa.segments.len(), pb.segments.len());
    }
    assert!(compact.len() < sh.text.len());
}

/// #281: an ended timer says what the app means instead of 0:00.
#[test]
fn an_ended_timer_shows_its_after_text() {
    use crate::{timer, timer_text};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    assert_eq!(timer_text(now - 10., "Departed"), "Departed");
    assert_eq!(timer_text(now - 10., ""), "0:00");
    assert_eq!(timer_text(now + 125.2, "Departed"), "2:05");

    let mut app = App::new();
    let past = SystemTime::now() - Duration::from_secs(10);
    let soon = SystemTime::now() + Duration::from_secs(90);
    let node = snapshot_one(
        &mut app,
        div()
            .child(timer(past).after("Departed"))
            .child(timer(soon).after("Departed"))
            .child(timer(past)),
    );
    let [ended, counting, plain] = &node.children[..] else {
        panic!("three timers");
    };
    // The text the snapshot carries already says it.
    assert_eq!(ended.text, "Departed");
    assert!(ended.to_json().contains(r#""s":"Departed","v":"#));
    assert!(ended.to_json().contains(r#","ph":"Departed""#));
    // Still counting: the time left, and the text for later.
    assert!(counting.text.starts_with("1:"), "{}", counting.text);
    assert!(counting.to_json().contains(r#","ph":"Departed""#));
    // Without one: 0:00 and no key.
    assert_eq!(plain.text, "0:00");
    assert!(!plain.to_json().contains(r#""ph""#));
    // `placeholder` is a timer's only.
    let input = snapshot_one(&mut app, crate::text_input().placeholder("Name"));
    assert!(input.placeholder.is_empty());
}

#[test]
fn components_know_when_they_render_a_snapshot() {
    let mut app = App::new();
    assert!(!app.in_snapshot());
    let node = snapshot_one(
        &mut app,
        crate::lazy(|cx| text(if cx.in_snapshot() { "static" } else { "live" })),
    );
    assert_eq!(node.text, "static");
    assert!(!app.in_snapshot());
}

#[test]
fn live_activities_stay_under_activitykit_limit() {
    use crate::surfaces::{ACTIVITY_PAYLOAD_LIMIT, ActivityContent};
    use crate::{Path, shape};
    let wavy = |n: usize| {
        let pts: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                (
                    i as f32 / (n - 1) as f32,
                    (i as f32 * 0.7).sin() * 0.45 + 0.5,
                )
            })
            .collect();
        shape()
            .path(Path::smooth(pts).stroke(rgb(0x123456), 2.))
            .h(px(40.))
            .w_full()
    };
    let mut app = App::new();
    // Small: unchanged.
    app.start_activity(Activity::new("small").summary("s").lock_screen(wavy(10)));
    // Large: rounded or stripped, but always under the limit.
    app.start_activity(
        Activity::new("large")
            .summary("l")
            .lock_screen(div().child(wavy(120)).child(wavy(120)))
            .expanded(wavy(60), wavy(60), wavy(60), wavy(60)),
    );
    let contents: Vec<(String, ActivityContent)> = app
        .take_requests()
        .into_iter()
        .filter_map(|r| match r {
            PlatformRequest::StartActivity { kind, content, .. } => Some((kind, content)),
            _ => None,
        })
        .collect();
    assert_eq!(contents.len(), 2);
    for (kind, c) in &contents {
        assert!(
            c.payload_size(kind) <= ACTIVITY_PAYLOAD_LIMIT,
            "{kind}: {}",
            c.payload_size(kind)
        );
    }
    let small = contents[0].1.lock_screen.as_ref().unwrap();
    assert!(
        small.to_json().contains("C .037 "),
        "small drawings keep 3 decimals"
    );
}

struct Counter2 {
    n: u32,
}

impl Render for Counter2 {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().child(button("+").on_click(cx.listener(|this, _, _| this.n += 1)))
    }
}

#[test]
fn widgets_rerender_from_state_once_per_event() {
    let (mut rt, first) = boot(|cx| {
        let root = cx.new_entity(|_| Counter2 { n: 0 });
        cx.register_widget(
            Widget::new("count", "Count")
                .families(&[WidgetFamily::Small])
                .render(move |cx, _| text(format!("n={}", root.read(cx).n))),
        );
        cx.reload_widgets_on(&root);
        cx.set_root(root);
    });
    let reqs = rt.take_requests();
    assert!(
        matches!(&reqs[..], [PlatformRequest::ReloadWidgets(w)] if w[0].families[0].1.contains("n=0"))
    );
    assert!(rt.take_requests().is_empty(), "nothing new");
    rt.dispatch(UiEvent::Click(find_text(&first, "+")));
    let reqs = rt.take_requests();
    assert!(
        matches!(&reqs[..], [PlatformRequest::ReloadWidgets(w)] if w[0].families[0].1.contains("n=1"))
    );
}

// --- Configurable widgets ---------------------------------------------------

use crate::surfaces::WidgetSnapshot;
use std::cell::RefCell;
use std::rc::Rc;

/// Rooms an app could add at runtime, and the widget showing one of them.
fn rooms_widget(rooms: Rc<RefCell<Vec<(&'static str, &'static str)>>>) -> Widget {
    Widget::new("reading", "Reading")
        .families(&[WidgetFamily::Small])
        .parameter(
            WidgetParameter::new("place", "Place")
                .options(move |_| {
                    rooms
                        .borrow()
                        .iter()
                        .map(|(id, title)| WidgetOption::new(*id, *title))
                        .collect()
                })
                .default("all"),
        )
        .parameter(WidgetParameter::new("metric", "Measure").options(|_| {
            vec![
                WidgetOption::new("co2", "CO2"),
                WidgetOption::new("temp", "Temperature"),
            ]
        }))
        .render_configured(|_, _, config| {
            text(format!(
                "{}/{}",
                config.get("place").unwrap_or("-"),
                config.get("metric").unwrap_or("-")
            ))
        })
}

fn widget_snapshots(app: &mut App) -> Vec<WidgetSnapshot> {
    app.take_requests()
        .into_iter()
        .find_map(|r| match r {
            PlatformRequest::ReloadWidgets(snaps) => Some(snaps),
            _ => None,
        })
        .expect("a widget reload")
}

fn config(place: &str, metric: &str) -> WidgetConfig {
    WidgetConfig::new()
        .with("place", place)
        .with("metric", metric)
}

#[test]
fn widget_parameters_are_described_for_the_bundler() {
    let rooms = Rc::new(RefCell::new(vec![("all", "Everywhere")]));
    let mut app = App::new();
    app.register_widget(rooms_widget(rooms));
    app.register_widget(Widget::new("plain", "Plain"));
    assert_eq!(
        app.describe_widgets(),
        concat!(
            r#"[{"kind":"reading","name":"Reading","description":"","families":["systemSmall"],"#,
            r#""parameters":[{"id":"place","title":"Place"},{"id":"metric","title":"Measure"}]},"#,
            r#"{"kind":"plain","name":"Plain","description":"","families":["systemSmall","systemMedium"],"parameters":[]}]"#
        )
    );
}

#[test]
fn small_option_sets_render_every_configuration() {
    let rooms = Rc::new(RefCell::new(vec![("all", "Everywhere"), ("1", "Kitchen")]));
    let mut app = App::new();
    app.register_widget(rooms_widget(rooms));
    let snaps = widget_snapshots(&mut app);
    let snap = &snaps[0];
    // Explicit default for "place", the first option for "metric".
    assert_eq!(snap.default, config("all", "co2"));
    assert!(snap.families[0].1.contains("all/co2"));
    // 2 × 2 combinations: the default plus the three others.
    let mut others: Vec<_> = snap.configs.iter().map(|(c, _)| c.clone()).collect();
    others.sort_by_key(WidgetConfig::to_json);
    assert_eq!(
        others,
        vec![
            config("1", "co2"),
            config("1", "temp"),
            config("all", "temp")
        ]
    );
    for (c, fams) in &snap.configs {
        let label = format!("{}/{}", c.get("place").unwrap(), c.get("metric").unwrap());
        assert!(fams[0].1.contains(&label), "{label} in {}", fams[0].1);
    }
    assert!(
        snap.families_for(&config("1", "temp"))[0]
            .1
            .contains("1/temp")
    );
    // An unknown configuration falls back to the default's snapshot.
    assert!(
        snap.families_for(&config("9", "temp"))[0]
            .1
            .contains("all/co2")
    );
    // The extension reads the options and the configurations from the JSON.
    let json = snap.to_json();
    assert!(json.contains(
        r#""parameters":[{"id":"place","default":"all","options":[{"id":"all","title":"Everywhere"},{"id":"1","title":"Kitchen"}]},"#
    ));
    assert!(json.contains(r#""default":{"metric":"co2","place":"all"}"#));
    assert!(json.contains(r#"{"values":{"metric":"temp","place":"1"},"families":{"systemSmall":"#));
    assert!(
        crate::push::Json::parse(&json).is_some(),
        "valid JSON: {json}"
    );
}

#[test]
fn static_widgets_keep_their_snapshot_format() {
    let mut app = App::new();
    app.register_widget(Widget::new("plain", "Plain").render(|_, _| text("hi")));
    let snaps = widget_snapshots(&mut app);
    let json = snaps[0].to_json();
    assert!(
        !json.contains("parameters") && !json.contains("configs"),
        "{json}"
    );
    assert!(snaps[0].configs.is_empty() && snaps[0].default.is_empty());
}

#[test]
fn large_option_sets_render_the_default_and_the_recorded_configurations() {
    let many: Vec<(&'static str, &'static str)> = (0..20)
        .map(|i| -> (&'static str, &'static str) {
            let id: &'static str = Box::leak(i.to_string().into_boxed_str());
            (id, id)
        })
        .collect();
    let rooms = Rc::new(RefCell::new(many));
    let mut app = App::new();
    app.register_widget(rooms_widget(rooms));
    // Without a record: only the default ("all" is no option here: the first is).
    let snaps = widget_snapshots(&mut app);
    assert_eq!(snaps[0].default, config("0", "co2"));
    assert!(snaps[0].configs.is_empty());

    let recorded = Rc::new(RefCell::new(vec![
        config("5", "temp"),
        // A removed option falls back to the default; an unknown parameter is dropped.
        WidgetConfig::new()
            .with("place", "gone")
            .with("metric", "temp")
            .with("x", "y"),
        config("5", "temp"),
    ]));
    let source = recorded.clone();
    app.set_widget_configurations_source(move |kind| {
        assert_eq!(kind, "reading");
        source.borrow().clone()
    });
    app.reload_widgets();
    let snaps = widget_snapshots(&mut app);
    let configs: Vec<_> = snaps[0].configs.iter().map(|(c, _)| c.clone()).collect();
    assert_eq!(configs, vec![config("5", "temp"), config("0", "temp")]);
    assert!(snaps[0].configs[0].1[0].1.contains("5/temp"));

    // Never more than MAX_CONFIGURATIONS per kind.
    *recorded.borrow_mut() = (0..20).map(|i| config(&i.to_string(), "temp")).collect();
    app.reload_widgets();
    let snaps = widget_snapshots(&mut app);
    assert_eq!(snaps[0].configs.len() + 1, Widget::MAX_CONFIGURATIONS);
}

#[test]
fn options_are_read_again_on_each_reload() {
    let rooms = Rc::new(RefCell::new(vec![("all", "Everywhere")]));
    let mut app = App::new();
    app.register_widget(rooms_widget(rooms.clone()));
    let snaps = widget_snapshots(&mut app);
    assert_eq!(snaps[0].parameters[0].2.len(), 1);
    rooms.borrow_mut().push(("2", "Bedroom"));
    app.reload_widgets();
    let snaps = widget_snapshots(&mut app);
    assert_eq!(
        snaps[0].parameters[0].2,
        vec![
            WidgetOption::new("all", "Everywhere"),
            WidgetOption::new("2", "Bedroom")
        ]
    );
    assert!(
        snaps[0].families_for(&config("2", "temp"))[0]
            .1
            .contains("2/temp")
    );
}

#[test]
fn becoming_active_reloads_widgets_for_new_configurations() {
    let many: Vec<(&'static str, &'static str)> = (0..20)
        .map(|i| -> (&'static str, &'static str) {
            let id: &'static str = Box::leak(format!("r{i}").into_boxed_str());
            (id, id)
        })
        .collect();
    let rooms = Rc::new(RefCell::new(many));
    let recorded = Rc::new(RefCell::new(Vec::<WidgetConfig>::new()));
    let mut app = App::new();
    let source = recorded.clone();
    app.set_widget_configurations_source(move |_| source.borrow().clone());
    app.register_widget(rooms_widget(rooms));
    widget_snapshots(&mut app);
    // Nothing new: no reload.
    app.set_lifecycle(Lifecycle::Background);
    app.set_lifecycle(Lifecycle::Active);
    assert!(app.take_requests().is_empty());
    // A widget added on the home screen while the app was away.
    recorded.borrow_mut().push(config("r3", "temp"));
    app.set_lifecycle(Lifecycle::Background);
    app.set_lifecycle(Lifecycle::Active);
    let snaps = widget_snapshots(&mut app);
    assert!(
        snaps[0].families_for(&config("r3", "temp"))[0]
            .1
            .contains("r3/temp")
    );
}

#[test]
fn recorded_configurations_parse_from_the_extension_json() {
    let list = WidgetConfig::list_from_json(
        r#"[{"place":"1","metric":"co2"},{"metric":"temp"},"junk",{"n":3}]"#,
    );
    assert_eq!(
        list,
        vec![
            config("1", "co2"),
            WidgetConfig::new().with("metric", "temp"),
            WidgetConfig::new(),
        ]
    );
    assert!(WidgetConfig::list_from_json("not json").is_empty());
    assert_eq!(
        config("1", "co2").to_json(),
        r#"{"metric":"co2","place":"1"}"#
    );
}

#[test]
fn render_without_configuration_still_works_with_parameters() {
    // `render(cx, family)` ignores the configuration: every instance looks the same.
    let mut app = App::new();
    app.register_widget(
        Widget::new("w", "W")
            .families(&[WidgetFamily::Small])
            .parameter(
                WidgetParameter::new("p", "P")
                    .options(|_| vec![WidgetOption::new("a", "A"), WidgetOption::new("b", "B")]),
            )
            .render(|_, _| text("same")),
    );
    let snaps = widget_snapshots(&mut app);
    assert_eq!(snaps[0].configs.len(), 1);
    assert!(
        snaps[0].families_for(&WidgetConfig::new().with("p", "b"))[0]
            .1
            .contains("same")
    );
}

#[test]
fn activities_and_notifications_become_requests() {
    let mut app = App::new();
    let id = app.start_activity(
        Activity::new("focus")
            .summary("Focus · 25:00")
            .compact(image("timer"), text("25:00"))
            .lock_screen(text("Focus")),
    );
    app.send_notification(Notification::new("Done").body("Take a break").link("/"));
    app.end_activity(id, None, true);
    let reqs = app.take_requests();
    match &reqs[0] {
        PlatformRequest::StartActivity { kind, content, .. } => {
            assert_eq!(kind, "focus");
            let json = content.to_json();
            assert!(
                json.contains(r#""cl":{"k":"i""#) && json.contains(r#""sm":"Focus · 25:00""#),
                "{json}"
            );
            assert!(json.len() < 4096, "fits ActivityKit's 4 KB limit");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(&reqs[1], PlatformRequest::Notify(n) if n.link.as_deref() == Some("/")));
    assert!(matches!(
        &reqs[2],
        PlatformRequest::EndActivity {
            immediately: true,
            ..
        }
    ));
}

#[test]
fn links_navigate_in_app_and_deep_links_open_screens() {
    struct Linker;
    impl Render for Linker {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            screen("Home").child(div().link("/items/4").child(text("go")))
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let r = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| Linker))
            .route("/items/:id", |cx, p| {
                let id = p.parse("id").unwrap_or(0);
                cx.new_entity(move |_| Item(id))
            })
            .build(cx);
        cx.set_root(r);
    });
    let mut rt = Runtime::new(&init);
    let first = rt.render();
    let row = first
        .iter()
        .find_map(|p| match p {
            Patch::SetAccessibility { id, a11y } if a11y.role == Some(Role::Link) => Some(*id),
            _ => None,
        })
        .expect("a link role");
    rt.dispatch(UiEvent::Click(row));
    assert_eq!(rt.app.current_path(), Some("/items/4"));
    rt.open_link("/items/9");
    assert_eq!(rt.app.current_path(), Some("/items/9"));
}

#[test]
fn a_link_follows_its_node_and_is_its_own_element_on_the_web() {
    struct Row(Option<&'static str>);
    impl Render for Row {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            let row = div().child(text("Order"));
            div()
                .child(button("next").on_click(cx.listener(|this: &mut Row, _, _| {
                    this.0 = match this.0 {
                        None => Some("/orders/42"),
                        Some("/orders/42") => Some("/orders/43"),
                        Some(_) => None,
                    }
                })))
                .child(match self.0 {
                    Some(path) => row.link(path),
                    None => row,
                })
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Row(Some("/orders/42")));
        cx.set_root(root);
    });
    for platform in [crate::Platform::Web, crate::Platform::Ios] {
        let mut rt = Runtime::with_options(
            &init,
            crate::RuntimeOptions {
                path: "/".into(),
                platform: Some(platform),
                ..Default::default()
            },
        );
        let first = rt.render();
        // Right after its `Create` (the web picks the element there).
        let at = first
            .iter()
            .position(|p| matches!(p, Patch::SetLink { href: Some(h), .. } if h == "/orders/42"))
            .expect("the link is sent");
        let Patch::SetLink { id: row, .. } = first[at] else {
            unreachable!()
        };
        assert_eq!(
            first[at - 1],
            Patch::Create {
                id: row,
                kind: NodeKind::View
            }
        );
        let next = first
            .iter()
            .find_map(|p| match p {
                Patch::SetText { id, text } if text == "next" => Some(*id),
                _ => None,
            })
            .unwrap();
        // Another destination: the same node, its new link.
        let changed = rt.dispatch(UiEvent::Click(next));
        assert!(
            changed.contains(&Patch::SetLink {
                id: row,
                href: Some("/orders/43".into())
            }),
            "{changed:?}"
        );
        assert!(!changed.iter().any(|p| matches!(p, Patch::Create { .. })));
        // No link any more: a new `<div>` on the web, the same view natively.
        let removed = rt.dispatch(UiEvent::Click(next));
        let destroyed = removed.contains(&Patch::Destroy { id: row });
        let unlinked = removed.contains(&Patch::SetLink {
            id: row,
            href: None,
        });
        if platform == crate::Platform::Web {
            assert!(destroyed && !unlinked, "{removed:?}");
            assert!(!removed.iter().any(|p| matches!(p, Patch::SetLink { .. })));
        } else {
            assert!(unlinked && !destroyed, "{removed:?}");
        }
    }
}

// --- Async tasks, timers, background work, storage ---------------------------

use crate::{FileStorage, Storage};
use std::time::{Duration, Instant};

struct Ticker {
    ticks: u32,
    loaded: Option<String>,
}

impl Render for Ticker {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(text(format!("ticks {}", self.ticks)))
            .child(text(self.loaded.clone().unwrap_or_default()))
            .child(button("go").on_click(cx.listener(|_, _, cx| {
                cx.spawn(async move |this, cx| {
                    for _ in 0..2 {
                        cx.sleep(Duration::from_millis(20)).await;
                        cx.update_entity(&this, |t, _| t.ticks += 1);
                    }
                    let data = cx.background(|| "from a thread".to_string()).await;
                    cx.update_entity(&this, |t, _| t.loaded = Some(data));
                });
            })))
    }
}

/// Drives the runtime like a backend would: poll when work is due.
fn run_until(rt: &mut Runtime, mut done: impl FnMut(&Runtime) -> bool) -> Vec<Patch> {
    let start = Instant::now();
    let mut all = Vec::new();
    while !done(rt) {
        assert!(start.elapsed() < Duration::from_secs(2), "timed out");
        std::thread::sleep(Duration::from_millis(5));
        all.extend(rt.poll());
    }
    all
}

#[test]
fn tasks_sleep_update_state_and_await_background_work() {
    let (mut rt, first) = boot(|cx| {
        let root = cx.new_entity(|_| Ticker {
            ticks: 0,
            loaded: None,
        });
        cx.set_root(root);
    });
    let root: Entity<Ticker> = first_root(&rt);
    rt.dispatch(UiEvent::Click(find_text(&first, "go")));
    assert!(rt.next_timer().is_some(), "a timer is pending");
    let patches = run_until(&mut rt, |rt| rt.app.read(&root).loaded.is_some());
    assert_eq!(rt.app.read(&root).ticks, 2);
    assert!(patches.contains(&Patch::SetText {
        id: find_text(&first, "ticks 0"),
        text: "ticks 2".into()
    }));
    assert!(
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetText { text, .. } if text == "from a thread"))
    );
    assert!(!rt.has_tasks());
}

/// Loads in two steps of background work from its first render, next to a
/// clock that ticks forever.
struct Loader {
    /// How long each step of background work takes.
    step: Duration,
    started: bool,
    loaded: Option<u32>,
}

impl Render for Loader {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.started {
            self.started = true;
            let step = self.step;
            cx.spawn(async move |this, cx| {
                let first = cx
                    .background(move || {
                        std::thread::sleep(step);
                        1
                    })
                    .await;
                let second = cx
                    .background(move || {
                        std::thread::sleep(step);
                        first + 1
                    })
                    .await;
                cx.update_entity(&this, |l, _| l.loaded = Some(second));
            });
            cx.spawn(async move |_, cx| {
                loop {
                    cx.sleep(Duration::from_millis(5)).await;
                }
            });
        }
        text(match self.loaded {
            Some(n) => format!("loaded {n}"),
            None => "loading".into(),
        })
    }
}

fn loader(step: Duration) -> Runtime {
    let init: AppInit = Arc::new(move |cx: &mut App| {
        let root = cx.new_entity(|_| Loader {
            step,
            started: false,
            loaded: None,
        });
        cx.set_root(root);
    });
    Runtime::new(&init)
}

fn shows(patches: &[Patch], text: &str) -> bool {
    patches
        .iter()
        .any(|p| matches!(p, Patch::SetText { text: t, .. } if t == text))
}

#[test]
fn a_settled_render_waits_for_the_background_work_it_started() {
    let mut rt = loader(Duration::from_millis(30));
    let start = Instant::now();
    let patches = rt.render_settled(Duration::from_secs(5));
    // Both steps ran; the clock, a timer, did not hold the render up.
    assert!(shows(&patches, "loaded 2"), "{patches:?}");
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    assert!(rt.has_tasks(), "the clock still runs");
}

#[test]
fn a_settled_render_stops_at_its_deadline() {
    let mut rt = loader(Duration::from_millis(400));
    let start = Instant::now();
    let patches = rt.render_settled(Duration::from_millis(50));
    assert!(start.elapsed() < Duration::from_millis(350));
    assert!(shows(&patches, "loading") && !shows(&patches, "loaded 2"));
    // What was still loading arrives later, as without a wait.
    let start = Instant::now();
    let mut later = Vec::new();
    while !shows(&later, "loaded 2") {
        assert!(start.elapsed() < Duration::from_secs(5), "timed out");
        std::thread::sleep(Duration::from_millis(5));
        later.extend(rt.poll());
    }
}

#[test]
fn a_settled_render_without_a_wait_is_a_render() {
    let mut rt = loader(Duration::from_millis(30));
    let patches = rt.render_settled(Duration::ZERO);
    assert!(shows(&patches, "loading") && !shows(&patches, "loaded 2"));
}

#[test]
fn cancelled_tasks_never_run() {
    let mut app = App::new();
    let flag = std::rc::Rc::new(std::cell::Cell::new(false));
    let f = flag.clone();
    let handle = app.spawn(|cx| async move {
        cx.sleep(Duration::from_millis(1)).await;
        f.set(true);
    });
    handle.cancel();
    std::thread::sleep(Duration::from_millis(5));
    crate::task::poll_tasks(&mut app);
    assert!(!flag.get());
}

#[test]
fn file_storage_round_trips_odd_keys() {
    let dir = std::env::temp_dir().join(format!("prism-storage-{}", std::process::id()));
    let mut s = FileStorage::new(&dir);
    s.set("todos/v1 é", "a\nb");
    assert_eq!(s.get("todos/v1 é").as_deref(), Some("a\nb"));
    assert_eq!(s.keys(), vec!["todos/v1 é".to_string()]);
    s.remove("todos/v1 é");
    assert!(s.get("todos/v1 é").is_none());
    let _ = std::fs::remove_dir_all(dir);
}

// --- Menus, alerts, tray, sheets ----------------------------------------------

use crate::{Alert, Menu, MenuItem, Tray, menu::MenuItemSpec};

struct Menus2 {
    picked: Vec<&'static str>,
}

impl Render for Menus2 {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(
                div().child(text("row")).context_menu(
                    Menu::new()
                        .item(
                            MenuItem::new("Pin")
                                .icon("pin")
                                .on_select(cx.listener(|t, _, _| t.picked.push("pin"))),
                        )
                        .separator()
                        .item(
                            MenuItem::new("Delete")
                                .destructive()
                                .on_select(cx.listener(|t, _, _| t.picked.push("delete"))),
                        ),
                ),
            )
            .child(button("ask").on_click(cx.listener(|_, _, cx| {
                let root = cx.entity();
                cx.show_alert(
                    Alert::new("Delete?")
                        .message("This cannot be undone.")
                        .destructive(
                            "Delete",
                            std::rc::Rc::new(move |_: &ClickEvent, app: &mut App| {
                                root.update(app, |t, cx| {
                                    t.picked.push("alert-delete");
                                    cx.notify();
                                })
                            }),
                        )
                        .cancel("Cancel"),
                );
            })))
    }
}

#[test]
fn context_menus_alerts_and_trays_dispatch_to_handlers() {
    let (mut rt, first) = boot(|cx| {
        let root = cx.new_entity(|_| Menus2 { picked: vec![] });
        cx.set_root(root);
        cx.set_tray(Tray::new("status").icon("bolt").menu(Menu::new().action(
            "Quit",
            std::rc::Rc::new(move |_: &ClickEvent, app: &mut App| {
                root.update(app, |t, _| t.picked.push("tray-quit"))
            }),
        )));
    });
    let root: Entity<Menus2> = first_root(&rt);

    // Context menu.
    let spec = first
        .iter()
        .find_map(|p| match p {
            Patch::SetMenu { menu: Some(m), .. } => Some(m.clone()),
            _ => None,
        })
        .expect("a context menu");
    let ids: Vec<u64> = spec
        .items
        .iter()
        .filter_map(|i| match i {
            MenuItemSpec::Action { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    rt.dispatch(UiEvent::MenuSelect(ids[1]));
    assert_eq!(rt.app.read(&root).picked, vec!["delete"]);

    // Tray menu.
    let reqs = rt.take_requests();
    let tray_id = reqs
        .iter()
        .find_map(|r| match r {
            PlatformRequest::SetTray(t) => match &t.menu.items[0] {
                MenuItemSpec::Action { id, .. } => Some(*id),
                _ => None,
            },
            _ => None,
        })
        .expect("a tray");
    rt.dispatch(UiEvent::MenuSelect(tray_id));
    assert_eq!(rt.app.read(&root).picked, vec!["delete", "tray-quit"]);

    // Alert: index 0 = Delete.
    rt.dispatch(UiEvent::Click(find_text(&first, "ask")));
    let alert = rt
        .take_requests()
        .into_iter()
        .find_map(|r| match r {
            PlatformRequest::ShowAlert(a) => Some(a),
            _ => None,
        })
        .expect("an alert");
    assert_eq!(alert.actions.len(), 2);
    rt.dispatch(UiEvent::AlertAction(alert.id, 0));
    assert_eq!(rt.app.read(&root).picked.last(), Some(&"alert-delete"));
}

#[test]
fn sheets_present_navigate_and_dismiss() {
    let (mut rt, first) = routed("/");
    rt.app.present("/items/1");
    let patches = rt.render();
    let modal_id = patches
        .iter()
        .find_map(|p| match p {
            Patch::Create {
                id,
                kind: NodeKind::Modal,
            } => Some(*id),
            _ => None,
        })
        .expect("a sheet");
    find_text(&patches, "Item 1");
    // Navigation inside the sheet, then back.
    rt.app.navigate("/items/2");
    let pushed = rt.render();
    find_text(&pushed, "Item 2");
    assert_eq!(
        rt.app.current_path(),
        Some("/"),
        "the main stack did not move"
    );
    rt.app.back();
    rt.render();
    // Swipe-down / Escape: the platform reports a dismissal.
    let closed = rt.dispatch(UiEvent::Back(modal_id, 0));
    assert!(closed.contains(&Patch::Destroy { id: modal_id }));
    let _ = first;
}

// --- Virtual lists, swipes, refresh, pictures --------------------------------

use crate::{SwipeDirection, VirtualList, picture};

struct Big {
    list: Entity<VirtualList>,
    swiped: u32,
    refreshing: bool,
}

impl Render for Big {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(self.list)
            .child(
                div()
                    .child(text("swipe me"))
                    .on_swipe(SwipeDirection::Left, cx.listener(|t, _, _| t.swiped += 1)),
            )
            .child(
                scroll()
                    .refreshing(self.refreshing)
                    .on_refresh(cx.listener(|t, _, _| t.refreshing = true))
                    .child(picture("https://example.com/a.png").alt("A cat")),
            )
    }
}

#[test]
fn virtual_lists_render_only_visible_rows() {
    let (mut rt, first) = boot(|cx| {
        let list = VirtualList::new(cx, 100_000, 50., |i, _| text(format!("Row {i}")));
        let root = cx.new_entity(|_| Big {
            list,
            swiped: 0,
            refreshing: false,
        });
        cx.set_root(root);
    });
    let rows = |p: &[Patch]| {
        p.iter()
            .filter(|p| matches!(p, Patch::SetText { text, .. } if text.starts_with("Row ")))
            .count()
    };
    let initial = rows(&first);
    assert!(
        initial > 10 && initial < 40,
        "{initial} rows rendered of 100 000"
    );
    let scroll_id = first
        .iter()
        .find_map(|p| match p {
            Patch::SetScrollEvents { id, on: true } => Some(*id),
            _ => None,
        })
        .unwrap();
    // The platform reports the real viewport; then a tiny scroll inside the
    // same window renders nothing.
    rt.dispatch(UiEvent::Scroll(scroll_id, 0., 800.));
    assert!(
        rt.dispatch(UiEvent::Scroll(scroll_id, 10., 800.))
            .is_empty()
    );
    // Jump far down: new rows appear, the far ones are destroyed.
    let patches = rt.dispatch(UiEvent::Scroll(scroll_id, 2_500_000., 800.));
    assert!(
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetText { text, .. } if text == "Row 50000"))
    );
    assert!(patches.iter().any(|p| matches!(p, Patch::Destroy { .. })));
}

#[test]
fn swipes_refresh_and_pictures() {
    let (mut rt, first) = boot(|cx| {
        let list = VirtualList::new(cx, 0, 50., |i, _| text(format!("Row {i}")));
        let root = cx.new_entity(|_| Big {
            list,
            swiped: 0,
            refreshing: false,
        });
        cx.set_root(root);
    });
    let root: Entity<Big> = first_root(&rt);
    let swipe_id = first
        .iter()
        .find_map(|p| match p {
            Patch::SetSwipe {
                id,
                left: true,
                right: false,
            } => Some(*id),
            _ => None,
        })
        .unwrap();
    rt.dispatch(UiEvent::Swipe(swipe_id, SwipeDirection::Left));
    rt.dispatch(UiEvent::Swipe(swipe_id, SwipeDirection::Right));
    assert_eq!(rt.app.read(&root).swiped, 1);

    // The refreshable scroll is the one marked clickable.
    let refresh_id = first
        .iter()
        .filter_map(|p| match p {
            Patch::Create {
                id,
                kind: NodeKind::Scroll,
            } => Some(*id),
            _ => None,
        })
        .find(|id| {
            first.contains(&Patch::SetClickable {
                id: *id,
                clickable: true,
            })
        })
        .unwrap();
    let patches = rt.dispatch(UiEvent::Click(refresh_id));
    assert!(patches.contains(&Patch::SetValue {
        id: refresh_id,
        value: 1.
    }));

    let pic = first
        .iter()
        .find_map(|p| match p {
            Patch::Create {
                id,
                kind: NodeKind::Picture,
            } => Some(*id),
            _ => None,
        })
        .unwrap();
    assert!(first.contains(&Patch::SetText {
        id: pic,
        text: "https://example.com/a.png".into()
    }));
    assert_eq!(
        a11y_of(&first, pic).unwrap().label.as_deref(),
        Some("A cat")
    );
}

#[test]
fn notification_actions_reach_the_handler_and_open_links() {
    use crate::surfaces::{NotificationAction, NotificationResponse};
    use std::sync::Mutex;
    struct Plain;
    impl Render for Plain {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            screen("Plain")
        }
    }
    let got: Arc<Mutex<Vec<NotificationResponse>>> = Default::default();
    let seen = got.clone();
    let init: AppInit = Arc::new(move |cx: &mut App| {
        let seen = seen.clone();
        cx.on_notification_response(move |r, _| seen.lock().unwrap().push(r.clone()));
        let r = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| Plain))
            .route("/later", |cx, _| cx.new_entity(|_| Plain))
            .build(cx);
        cx.set_root(r);
    });
    let n = crate::Notification::new("Timer done")
        .id("focus")
        .action(NotificationAction::new("snooze", "Snooze"))
        .action(NotificationAction::new("reply", "Reply").reply("Message…"))
        .action(NotificationAction::new("open", "Open").link("/later"));
    let json = n.to_json();
    assert!(json.contains(r#""actions":[{"id":"snooze""#), "{json}");
    assert!(json.contains(r#""reply":"Message…""#));
    assert!(json.contains(r#""link":"/later","#) || json.contains(r#""link":"/later"}"#));

    let mut rt = Runtime::new(&init);
    rt.render();
    let response = NotificationResponse {
        id: "focus".into(),
        action: Some("open".into()),
        text: None,
    };
    rt.notification_response(response.clone(), Some("/later".into()));
    assert_eq!(got.lock().unwrap().as_slice(), &[response]);
    assert_eq!(rt.app.current_path(), Some("/later"));
}

#[test]
fn tab_titles_follow_the_language() {
    struct Page;
    impl Render for Page {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            screen("Page")
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        cx.set_translations(
            crate::Translations::new()
                .add("en", &[("tab", "Settings")])
                .add("fr", &[("tab", "Réglages")]),
        );
        let r = Router::new()
            .tab_with(|cx| cx.t("tab"), "gearshape", "/")
            .route("/", |cx, _| cx.new_entity(|_| Page))
            .build(cx);
        cx.set_root(r);
    });
    let mut rt = Runtime::new(&init);
    let first = rt.render();
    assert!(
        first
            .iter()
            .any(|p| matches!(p, Patch::SetText { text, .. } if text == "Settings"))
    );
    rt.app.set_locale(Some(crate::Locale::new("fr")));
    let patches = rt.render();
    assert!(
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetText { text, .. } if text == "Réglages")),
        "{patches:?}"
    );
}

#[test]
fn background_tasks_and_lifecycle() {
    use crate::{BackgroundTask, Lifecycle, PlatformRequest};
    use std::sync::Mutex;
    struct Plain;
    impl Render for Plain {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    let log: Arc<Mutex<Vec<String>>> = Default::default();
    let seen = log.clone();
    let init: AppInit = Arc::new(move |cx: &mut App| {
        let (a, b) = (seen.clone(), seen.clone());
        cx.on_lifecycle(move |phase, _| a.lock().unwrap().push(format!("{phase:?}")));
        cx.register_background_task(
            BackgroundTask::refresh("sync").every(std::time::Duration::from_secs(60)),
            move |cx| {
                let b = b.clone();
                async move {
                    cx.sleep(std::time::Duration::from_millis(30)).await;
                    b.lock().unwrap().push("synced".into());
                }
            },
        );
        let root = cx.new_entity(|_| Plain);
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    rt.render();
    let scheduled = rt.take_requests();
    assert!(
        scheduled
            .iter()
            .any(|r| matches!(r, PlatformRequest::ScheduleBackgroundTask(t) if t.id == "sync"))
    );
    assert_eq!(
        rt.app.describe_background(),
        r#"[{"id":"sync","kind":"refresh"}]"#
    );

    rt.set_lifecycle(Lifecycle::Background);
    assert_eq!(rt.app.lifecycle(), Lifecycle::Background);
    assert!(rt.run_background_task("nope").is_none());
    let task = rt.run_background_task("sync").unwrap();
    assert!(rt.is_task_running(task) && rt.wants_background_time());
    // Rescheduled for its next run.
    assert!(
        rt.take_requests()
            .iter()
            .any(|r| matches!(r, PlatformRequest::ScheduleBackgroundTask(_)))
    );
    std::thread::sleep(std::time::Duration::from_millis(40));
    rt.poll();
    assert!(!rt.is_task_running(task) && !rt.wants_background_time());
    assert_eq!(*log.lock().unwrap(), ["Background", "synced"]);
}

#[test]
fn device_calls_resolve_and_streams_deliver() {
    use crate::PlatformRequest;
    use crate::device::*;
    struct Here {
        text: String,
        tilt: f64,
    }
    impl Render for Here {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            text(format!("{} {}", self.text, self.tilt))
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|cx: &mut Context<Here>| {
            cx.spawn(async move |this, cx| {
                let state = cx.request_permission(Permission::Location).await;
                let text = match cx.current_location().await {
                    Ok(l) if state.is_granted() => format!("{:.2},{:.2}", l.latitude, l.longitude),
                    Ok(_) => "no permission".into(),
                    Err(e) => e.to_string(),
                };
                cx.update_entity(&this, |h, _| h.text = text);
            });
            let me = cx.entity();
            cx.watch_motion(std::time::Duration::from_millis(50), move |m, app| {
                if let Ok(m) = m {
                    me.update(app, |h, cx| {
                        h.tilt = m.attitude[0];
                        cx.notify();
                    });
                }
            });
            Here {
                text: "…".into(),
                tilt: 0.,
            }
        });
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    rt.render();
    let calls: Vec<(u64, DeviceCall)> = rt
        .take_requests()
        .into_iter()
        .filter_map(|r| match r {
            PlatformRequest::Device { id, call } => Some((id, call)),
            _ => None,
        })
        .collect();
    let (perm, _) = calls
        .iter()
        .find(|(_, c)| matches!(c, DeviceCall::RequestPermission(Permission::Location)))
        .unwrap()
        .clone();
    let (motion, _) = calls
        .iter()
        .find(|(_, c)| matches!(c, DeviceCall::WatchMotion(_)))
        .unwrap()
        .clone();
    rt.device_reply(perm, Ok(DeviceValue::Permission(PermissionState::Granted)));
    let loc = rt
        .take_requests()
        .into_iter()
        .find_map(|r| match r {
            PlatformRequest::Device {
                id,
                call: DeviceCall::CurrentLocation(_),
            } => Some(id),
            _ => None,
        })
        .expect("asks for the location after the permission");
    let patches = rt.device_reply(
        loc,
        Ok(DeviceValue::Location(Location {
            latitude: 48.8566,
            longitude: 2.3522,
            ..Default::default()
        })),
    );
    find_text(&patches, "48.86,2.35 0");
    let patches = rt.device_event(
        motion,
        DeviceEvent::Motion(Motion {
            attitude: [0.5, 0., 0.],
            ..Default::default()
        }),
    );
    find_text(&patches, "48.86,2.35 0.5");
}

/// #112: progress while picked files reach the app, and a cancel.
#[test]
fn picked_files_report_their_progress_and_can_be_cancelled() {
    use crate::PlatformRequest;
    use crate::device::*;
    struct Files {
        text: String,
    }
    impl Render for Files {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            text(self.text.clone())
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|cx: &mut Context<Files>| {
            let me = cx.entity();
            cx.spawn(async move |this, cx| {
                let files = cx
                    .pick_files_with_progress(PickFiles::new(), move |p, app| {
                        let pct = (p.fraction() * 100.).round();
                        me.update(app, |f, cx| {
                            f.text = format!("{pct}%");
                            cx.notify();
                        });
                    })
                    .await;
                let text = match files {
                    Ok(f) if f.is_empty() => "none".to_string(),
                    Ok(f) => f[0].name.clone(),
                    Err(e) => e.to_string(),
                };
                cx.update_entity(&this, |f, _| f.text = text);
            });
            Files { text: "…".into() }
        });
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    rt.render();
    let pick = |rt: &mut Runtime| {
        rt.take_requests()
            .into_iter()
            .find_map(|r| match r {
                PlatformRequest::Device {
                    id,
                    call: DeviceCall::PickFiles(_),
                } => Some(id),
                _ => None,
            })
            .unwrap()
    };
    let id = pick(&mut rt);
    find_text(
        &rt.device_event(id, DeviceEvent::Upload { sent: 0, total: 4 }),
        "0%",
    );
    find_text(
        &rt.device_event(id, DeviceEvent::Upload { sent: 3, total: 4 }),
        "75%",
    );
    let file = PickedFile {
        path: "/files/a.txt".into(),
        name: "a.txt".into(),
        mime: "text/plain".into(),
        size: 4,
    };
    find_text(
        &rt.device_reply(id, Ok(DeviceValue::Files(vec![file]))),
        "a.txt",
    );
    // Late events are ignored once the files are the app's.
    assert!(
        rt.device_event(id, DeviceEvent::Upload { sent: 4, total: 4 })
            .is_empty()
    );
    assert_eq!(
        UploadProgress {
            upload: Upload(1),
            sent: 0,
            total: 0
        }
        .fraction(),
        1.
    );

    // A second pick, cancelled half way: the page is told to stop.
    let init2: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|cx: &mut Context<Files>| {
            cx.spawn(async move |this, cx| {
                let n = cx
                    .pick_files_with_progress(PickFiles::new(), |p, app| {
                        if p.sent == 5 {
                            app.cancel_upload(p.upload);
                        }
                    })
                    .await
                    .map(|f| f.len());
                cx.update_entity(&this, |f, _| f.text = format!("{n:?}"));
            });
            Files { text: "…".into() }
        });
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init2);
    rt.render();
    let id = pick(&mut rt);
    rt.device_event(id, DeviceEvent::Upload { sent: 5, total: 10 });
    assert!(rt.take_requests().into_iter().any(|r| matches!(
        r,
        PlatformRequest::Device { id: stop, call: DeviceCall::StopWatch } if stop == id
    )));
    // The page answers « cancelled »: no files.
    find_text(&rt.device_reply(id, Err(DeviceError::Cancelled)), "Ok(0)");
}

/// #111: a drop target's types reach the backend, and its handler gets
/// only the dropped files it accepts.
#[test]
fn drop_targets_get_the_files_they_accept() {
    use crate::PickedFile;
    struct Zone {
        on: bool,
        got: Vec<String>,
    }
    impl Render for Zone {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            let zone = div().child(text(self.got.join(",")));
            if self.on {
                zone.on_drop_files(
                    ["image/*", ".pdf"],
                    cx.listener(|this, e: &FileDropEvent, _| {
                        this.got = e.files.iter().map(|f| f.name.clone()).collect();
                    }),
                )
            } else {
                zone
            }
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Zone {
            on: true,
            got: Vec::new(),
        });
        cx.set_global(root);
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    let patches = rt.render();
    let (id, types) = patches
        .iter()
        .find_map(|p| match p {
            Patch::SetDropTarget { id, types } => Some((*id, types.clone())),
            _ => None,
        })
        .expect("a drop target");
    assert_eq!(types, Some(vec!["image/*".to_string(), ".pdf".to_string()]));
    let file = |name: &str, mime: &str| PickedFile {
        path: format!("/tmp/{name}"),
        name: name.into(),
        mime: mime.into(),
        size: 1,
    };
    let patches = rt.dispatch(UiEvent::DropFiles(
        id,
        vec![
            file("cat.png", "image/png"),
            file("notes.txt", "text/plain"),
            file("doc.PDF", "application/octet-stream"),
        ],
    ));
    find_text(&patches, "cat.png,doc.PDF");
    // Nothing it accepts: the handler is not called.
    assert!(
        rt.dispatch(UiEvent::DropFiles(id, vec![file("a.txt", "text/plain")]))
            .is_empty()
    );
    // No longer a drop target.
    let root = *rt.app.global::<Entity<Zone>>();
    rt.app.update(&root, |z, cx| {
        z.on = false;
        cx.notify();
    });
    let patches = rt.render_dirty();
    assert!(patches.contains(&Patch::SetDropTarget { id, types: None }));
}

/// #94: ceremonies of the app's own relying party: the server's options
/// (wrapper removed), the credential's JSON back.
#[test]
fn webauthn_ceremonies_pass_the_servers_options_through() {
    use crate::PlatformRequest;
    use crate::device::*;
    struct Keys {
        out: String,
    }
    impl Render for Keys {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            text(self.out.clone())
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|cx: &mut Context<Keys>| {
            cx.spawn(async move |this, cx| {
                let bad = cx.webauthn_get("[1]").await;
                let r = cx
                    .webauthn_create(r#"{"publicKey":{"challenge":"AQID"}}"#)
                    .await;
                let out = format!("{} | {}", bad.is_err(), r.unwrap_or_else(|e| e.to_string()));
                cx.update_entity(&this, |k, _| k.out = out);
            });
            Keys { out: "…".into() }
        });
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    rt.render();
    let (id, options) = rt
        .take_requests()
        .into_iter()
        .find_map(|r| match r {
            PlatformRequest::Device {
                id,
                call: DeviceCall::WebAuthn(w),
            } => Some((id, w)),
            _ => None,
        })
        .expect("one ceremony: the bad options never leave the app");
    assert!(options.create);
    assert_eq!(options.options, r#"{"challenge":"AQID"}"#);
    let patches = rt.device_reply(id, Ok(DeviceValue::Text(Some(r#"{"id":"x"}"#.into()))));
    find_text(&patches, r#"true | {"id":"x"}"#);
}

/// #203: a sign-in in a field's suggestions is a conditional `get`.
#[test]
fn webauthn_autofill_asks_for_a_conditional_sign_in() {
    use crate::PlatformRequest;
    use crate::device::*;
    struct Plain;
    impl Render for Plain {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|cx: &mut Context<Plain>| {
            cx.spawn(async move |_, cx| {
                let _ = cx
                    .webauthn_autofill(r#"{"challenge":"AQID","rpId":"example.com"}"#)
                    .await;
            });
            Plain
        });
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    rt.render();
    let request = rt
        .take_requests()
        .into_iter()
        .find_map(|r| match r {
            PlatformRequest::Device {
                call: DeviceCall::WebAuthn(w),
                ..
            } => Some(w),
            _ => None,
        })
        .expect("a ceremony");
    assert!(request.conditional && !request.create);
    assert_eq!(
        request.options,
        r#"{"challenge":"AQID","rpId":"example.com"}"#
    );
}

/// #161: the app's whole menu bar, with system items next to its own.
#[test]
fn the_menu_bar_mixes_system_and_app_items() {
    use crate::PlatformRequest;
    use crate::menu::{Menu, MenuItem, MenuItemSpec, SystemAction};
    struct Plain;
    impl Render for Plain {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let undo: Handler<ClickEvent> = std::rc::Rc::new(|_, _| {});
        cx.set_menu_bar(vec![
            Menu::titled("App").item(MenuItem::system(SystemAction::Quit).title("Quitter")),
            Menu::titled("Édition")
                .item(MenuItem::new("Annuler").shortcut("cmd+z").on_select(undo))
                .item(MenuItem::system(SystemAction::Copy).title("Copier"))
                .item(MenuItem::system(SystemAction::Zoom)),
        ]);
        let root = cx.new_entity(|_| Plain);
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    rt.render();
    let bar = rt
        .take_requests()
        .into_iter()
        .find_map(|r| match r {
            PlatformRequest::SetMenuBar(m) => Some(m),
            _ => None,
        })
        .expect("the bar");
    assert_eq!(bar.len(), 2);
    assert_eq!(
        bar[0].items[0],
        MenuItemSpec::System {
            action: SystemAction::Quit,
            title: "Quitter".into(),
            shortcut: Some("cmd+q".into()),
        }
    );
    let edit = &bar[1].items;
    assert!(
        matches!(&edit[0], MenuItemSpec::Action { shortcut: Some(k), enabled: true, .. } if k == "cmd+z")
    );
    assert_eq!(
        edit[2],
        MenuItemSpec::System {
            action: SystemAction::Zoom,
            title: "Zoom".into(),
            shortcut: None,
        },
        "English title and no shortcut by default"
    );
    assert!(
        bar[1]
            .to_json()
            .contains(r#""t":"system","action":"Copy","title":"Copier""#)
    );
}

/// #247: a shortcut hint is shown, not bound; `shortcut` binds again.
#[test]
fn shortcut_hints_are_marked_for_backends() {
    use crate::menu::{Menu, MenuItem, MenuItemSpec};
    let h: Handler<ClickEvent> = std::rc::Rc::new(|_, _| {});
    let menu = Menu::new()
        .item(
            MenuItem::new("Bold")
                .shortcut_hint("cmd+b")
                .on_select(h.clone()),
        )
        .item(
            MenuItem::new("Find")
                .shortcut_hint("cmd+f")
                .shortcut("cmd+f")
                .on_select(h),
        )
        .item(MenuItem::new("Plain").shortcut_hint("cmd+p"));
    let spec = menu.compile(0, &mut Vec::new());
    let hints: Vec<bool> = spec
        .items
        .iter()
        .map(|i| {
            matches!(
                i,
                MenuItemSpec::Action {
                    shortcut_hint: true,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(hints, [true, false, true]);
    assert!(spec.to_json().contains(r#""shortcut":"cmd+b","hint":true"#));
    assert!(
        spec.to_json()
            .contains(r#""shortcut":"cmd+f","hint":false"#)
    );
}

/// #159: clicks carry their keys and count; key releases reach on_key_up
/// and on_global_key_up.
#[test]
fn clicks_say_keys_and_count_and_key_ups_are_heard() {
    struct Row {
        log: Vec<String>,
    }
    impl Render for Row {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .on_global_key_up(cx.listener(|this, e: &KeyEvent, _| {
                    this.log.push(format!("global up {}", e.key))
                }))
                .child(
                    div()
                        .on_click(cx.listener(|this, e: &ClickEvent, _| {
                            this.log.push(format!(
                                "click {} {}",
                                e.count,
                                if e.modifiers.shift { "shift" } else { "-" }
                            ))
                        }))
                        .on_key_up(cx.listener(|this, e: &KeyEvent, _| {
                            this.log.push(format!("up {}", e.key))
                        }))
                        .child(text(self.log.join(","))),
                )
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Row { log: Vec::new() });
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    let patches = rt.render();
    // The row asked for key reports (key ups come with them).
    let row = patches
        .iter()
        .find_map(|p| match p {
            Patch::SetKeyEvents {
                id,
                local: true,
                global: false,
            } => Some(*id),
            _ => None,
        })
        .expect("the row reports keys");
    let shift = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    let click = ClickEvent {
        position: Some(Point::new(1., 2.)),
        modifiers: shift,
        count: 2,
        button: crate::PointerButton::Primary,
        text_offset: None,
    };
    find_text(
        &rt.dispatch(UiEvent::ClickWith(row, click)),
        "click 2 shift",
    );
    find_text(
        &rt.dispatch(UiEvent::ClickAt(row, Point::new(0., 0.))),
        "click 2 shift,click 1 -",
    );
    let up = KeyEvent::new(" ");
    find_text(
        &rt.dispatch(UiEvent::KeyUp(Some(row), up.clone())),
        "click 2 shift,click 1 -,up  ,global up  ",
    );
    assert!(ClickEvent.count == 1 && !ClickEvent.is_double());
}

/// #158: unsaved changes reach the backend once per change; the question's
/// texts are built in.
#[test]
fn unsaved_changes_reach_the_backend_on_change() {
    use crate::PlatformRequest;
    struct Plain;
    impl Render for Plain {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Plain);
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    rt.render();
    rt.take_requests();
    rt.app.set_unsaved_changes(true);
    rt.app.set_unsaved_changes(true);
    assert!(rt.app.has_unsaved_changes());
    rt.app.set_unsaved_changes(false);
    let seen: Vec<bool> = rt
        .take_requests()
        .into_iter()
        .filter_map(|r| match r {
            PlatformRequest::SetUnsavedChanges(on) => Some(on),
            _ => None,
        })
        .collect();
    assert_eq!(seen, [true, false]);
    let fr = crate::Locale::new("fr-FR");
    assert_eq!(
        crate::i18n::chrome_text(None, &fr, "unsaved_title"),
        "Quitter sans enregistrer ?"
    );
    assert_eq!(
        crate::i18n::chrome_text(None, &crate::Locale::new("ja"), "cancel"),
        "キャンセル"
    );
    assert_eq!(rt.app.chrome_text("discard"), "Discard Changes");
    // File dialogs name their filters in the app's language.
    assert_eq!(
        crate::i18n::chrome_text(None, &fr, "all_files"),
        "Tous les fichiers"
    );
    assert_eq!(crate::i18n::chrome_text(None, &fr, "files"), "Fichiers");
    assert_eq!(
        crate::i18n::chrome_text(None, &crate::Locale::new("de"), "images"),
        "Bilder"
    );
}

/// #156: on_dismiss says how the user closed the overlay.
#[test]
fn dismissals_say_how() {
    struct Picker {
        open: bool,
        log: Vec<String>,
    }
    impl Render for Picker {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            div().child(text(self.log.join(","))).child(
                dialog(text("Colors"))
                    .open(self.open)
                    .on_dismiss(cx.listener(|this, e: &BackEvent, _| {
                        this.log.push(e.reason.name().to_string());
                    })),
            )
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Picker {
            open: true,
            log: Vec::new(),
        });
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    let patches = rt.render();
    let overlay = patches
        .iter()
        .find_map(|p| match p {
            Patch::Create {
                id,
                kind: NodeKind::Overlay,
            } => Some(*id),
            _ => None,
        })
        .unwrap();
    find_text(
        &rt.dispatch(UiEvent::Dismiss(overlay, DismissReason::Escape)),
        "escape",
    );
    find_text(
        &rt.dispatch(UiEvent::Dismiss(overlay, DismissReason::Outside)),
        "escape,outside",
    );
    // A backend that does not say: unknown.
    find_text(
        &rt.dispatch(UiEvent::Back(overlay, 0)),
        "escape,outside,unknown",
    );
    assert_eq!(DismissReason::from_name("swipe"), DismissReason::Swipe);
    assert_eq!(DismissReason::from_name("??"), DismissReason::Unknown);
}

/// #154: the app's title goes to the backend once per change.
#[test]
fn the_apps_title_reaches_the_backend_on_change() {
    use crate::PlatformRequest;
    struct Plain;
    impl Render for Plain {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        cx.set_title("Orders");
        cx.set_title("Orders");
        let root = cx.new_entity(|_| Plain);
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    rt.render();
    let titles = |rt: &mut Runtime| -> Vec<Option<String>> {
        rt.take_requests()
            .into_iter()
            .filter_map(|r| match r {
                PlatformRequest::SetTitle(t) => Some(t),
                _ => None,
            })
            .collect()
    };
    assert_eq!(titles(&mut rt), [Some("Orders".to_string())]);
    assert_eq!(rt.app.title(), Some("Orders"));
    rt.app.set_title("  ");
    rt.app.clear_title();
    assert_eq!(titles(&mut rt), [None]);
    assert_eq!(rt.app.title(), None);
}

/// #136: the buttons that may drag become pointer bits.
#[test]
fn drag_buttons_become_pointer_bits() {
    use crate::events::pointer_mask::{DRAG, DRAG_MIDDLE, DRAG_SECONDARY};
    struct Pad(&'static [PointerButton]);
    impl Render for Pad {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            let d = div().on_drag(cx.listener(|_, _: &DragEvent, _| {}));
            if self.0.is_empty() {
                d
            } else {
                d.drag_buttons(self.0)
            }
        }
    }
    for (buttons, mask) in [
        (&[][..], DRAG),
        (&[PointerButton::Middle][..], DRAG_MIDDLE),
        (
            &[PointerButton::Primary, PointerButton::Secondary][..],
            DRAG | DRAG_SECONDARY,
        ),
        (&[PointerButton::Other(4)][..], 0),
    ] {
        let init: AppInit = Arc::new(move |cx: &mut App| {
            let root = cx.new_entity(|_| Pad(buttons));
            cx.set_root(root);
        });
        let mut rt = Runtime::new(&init);
        let patches = rt.render();
        let got = patches
            .iter()
            .find_map(|p| match p {
                Patch::SetPointerEvents { mask, .. } => Some(*mask),
                _ => None,
            })
            .unwrap_or(0);
        assert_eq!(got, mask, "{buttons:?}");
    }
}

/// #243: a long-press drag sends its hold once, only for a node that drags,
/// and 0 when it goes back to dragging on movement.
#[test]
fn drag_after_long_press_sends_its_hold() {
    use std::sync::atomic::{AtomicU8, Ordering};
    // 0: no delay, 1: a 400 ms hold, 2: a hold without a drag handler.
    static MODE: AtomicU8 = AtomicU8::new(1);
    struct Card;
    impl Render for Card {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            let hold = std::time::Duration::from_millis(400);
            match MODE.load(Ordering::SeqCst) {
                0 => div().on_drag(cx.listener(|_, _: &DragEvent, _| {})),
                1 => div()
                    .on_drag(cx.listener(|_, _: &DragEvent, _| {}))
                    .drag_after_long_press(hold),
                _ => div().drag_after_long_press(hold),
            }
        }
    }
    let delays = |patches: &[Patch]| -> Vec<u32> {
        patches
            .iter()
            .filter_map(|p| match p {
                Patch::SetDragDelay { ms, .. } => Some(*ms),
                _ => None,
            })
            .collect()
    };
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Card);
        cx.set_root(root);
    });
    MODE.store(1, Ordering::SeqCst);
    let mut rt = Runtime::new(&init);
    let patches = rt.render();
    // After the pointer bits, so backends know it drags.
    let pointer = patches
        .iter()
        .position(|p| matches!(p, Patch::SetPointerEvents { .. }));
    let delay = patches
        .iter()
        .position(|p| matches!(p, Patch::SetDragDelay { .. }));
    assert!(pointer < delay, "{patches:?}");
    assert_eq!(delays(&patches), [400]);
    rt.app.refresh();
    assert_eq!(
        delays(&rt.render()),
        [] as [u32; 0],
        "unchanged: not sent again"
    );
    MODE.store(0, Ordering::SeqCst);
    rt.app.refresh();
    assert_eq!(delays(&rt.render()), [0]);
    MODE.store(2, Ordering::SeqCst);
    rt.app.refresh();
    assert_eq!(delays(&rt.render()), [] as [u32; 0], "nothing drags it");
}

/// #133: wheel and pinch handlers set their pointer bits and get their events.
#[test]
fn wheel_and_magnify_reach_their_element() {
    use crate::events::pointer_mask::{MAGNIFY, WHEEL};
    struct Canvas {
        pan: Point,
        zoom: f32,
    }
    impl Render for Canvas {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .on_wheel(cx.listener(|this, e: &WheelEvent, _| {
                    this.pan = Point::new(this.pan.x - e.delta.x, this.pan.y - e.delta.y)
                }))
                .on_magnify(cx.listener(|this, e: &MagnifyEvent, _| this.zoom *= e.scale))
                .child(text(format!("{} {} {}", self.pan.x, self.pan.y, self.zoom)))
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Canvas {
            pan: Point::new(0., 0.),
            zoom: 1.,
        });
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    let patches = rt.render();
    let id = patches
        .iter()
        .find_map(|p| match p {
            Patch::SetPointerEvents { id, mask } if *mask == WHEEL | MAGNIFY => Some(*id),
            _ => None,
        })
        .expect("wheel and pinch bits");
    let wheel = WheelEvent {
        delta: Point::new(10., -4.),
        phase: GesturePhase::Move,
        precise: true,
        ..WheelEvent::default()
    };
    find_text(&rt.dispatch(UiEvent::Wheel(id, wheel)), "-10 4 1");
    let pinch = MagnifyEvent {
        scale: 2.,
        position: Point::new(0., 0.),
        phase: GesturePhase::None,
    };
    find_text(&rt.dispatch(UiEvent::Magnify(id, pinch)), "-10 4 2");
    assert_eq!(
        GesturePhase::from_name("momentum"),
        Some(GesturePhase::Momentum)
    );
}

#[test]
fn cloud_storage_and_external_changes() {
    use std::sync::Mutex;
    struct Plain;
    impl Render for Plain {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    let seen: Arc<Mutex<Vec<String>>> = Default::default();
    let log = seen.clone();
    let init: AppInit = Arc::new(move |cx: &mut App| {
        cx.cloud_storage().set("theme", "dark");
        let log = log.clone();
        cx.on_cloud_change(move |keys, app| {
            let value = app.cloud_storage().get(&keys[0]).unwrap_or_default();
            log.lock().unwrap().push(format!("{}={value}", keys[0]));
        });
        let root = cx.new_entity(|_| Plain);
        cx.set_root(root);
    });
    let mut rt = Runtime::with_options(
        &init,
        crate::RuntimeOptions {
            path: "/".into(),
            data_dir: Some("/tmp/prism-user".into()),
            ..Default::default()
        },
    );
    rt.render();
    assert_eq!(rt.app.cloud_storage().get("theme").as_deref(), Some("dark"));
    // Natively the shared directory is the user's.
    assert_eq!(
        rt.app.shared_data_dir(),
        Some(std::path::Path::new("/tmp/prism-user"))
    );
    rt.app.cloud_storage().set("theme", "light"); // as if another device wrote it
    rt.cloud_changed(&["theme".to_string()]);
    assert_eq!(*seen.lock().unwrap(), ["theme=light"]);
}

#[test]
fn responsive_styles_follow_the_window() {
    use crate::style::{Breakpoint, FlexDirection};
    struct Page;
    impl Render for Page {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex_col()
                .gap(8.)
                .md(|s| s.flex_row().gap(24.))
                .child(text(format!("{:?}", cx.breakpoint())))
                .child(text("phones only").hidden_from(Breakpoint::Medium))
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Page);
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    let first = rt.render();
    let style_of = |patches: &[Patch], dir: FlexDirection| {
        patches.iter().any(|p| matches!(p, Patch::SetStyle { style, .. } if style.direction == dir && style.responsive.is_empty()))
    };
    assert!(style_of(&first, FlexDirection::Column));
    find_text(&first, "Compact");
    // Same class: nothing to re-render.
    assert!(rt.set_viewport(500., 800.).is_empty());
    // A tablet: the row layout applies, the phone-only text hides.
    let patches = rt.set_viewport(820., 1180.);
    assert!(style_of(&patches, FlexDirection::Row), "{patches:?}");
    assert!(
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetStyle { style, .. } if style.display_none))
    );
    find_text(&patches, "Medium");
    assert_eq!(rt.app.viewport(), (820., 1180.));
    // A side bar covers 230 of a 830 window (#28): the content area is 600
    // wide, still Medium; 240 of it falls under Medium.
    let none = crate::Edges::default();
    assert!(
        rt.set_window(830., 1180., crate::Edges { left: 230., ..none })
            .iter()
            .all(|p| !matches!(p, Patch::SetText { .. })),
    );
    assert_eq!(rt.app.viewport(), (600., 1180.));
    assert_eq!(rt.app.window_size(), (830., 1180.));
    assert_eq!(rt.app.safe_area().left, 230.);
    let patches = rt.set_window(830., 1180., crate::Edges { left: 240., ..none });
    find_text(&patches, "Compact");
    assert_eq!(rt.app.viewport(), (590., 1180.));
    // `set_viewport`: no insets.
    rt.set_viewport(830., 1180.);
    assert_eq!(rt.app.safe_area(), none);
    assert_eq!(rt.app.viewport(), rt.app.window_size());
}

#[test]
fn the_safe_area_re_renders_and_resize_handlers_see_the_content_area() {
    use std::sync::Mutex;
    struct Insets;
    impl Render for Insets {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            text(format!("top {}", cx.safe_area().top))
        }
    }
    let seen: Arc<Mutex<Vec<(f32, f32)>>> = Arc::default();
    let log = seen.clone();
    let init: AppInit = Arc::new(move |cx: &mut App| {
        let root = cx.new_entity(|_| Insets);
        cx.set_root(root);
        let log = log.clone();
        cx.on_resize(move |size, _| log.lock().unwrap().push(size));
    });
    let mut rt = Runtime::new(&init);
    find_text(&rt.render(), "top 0");
    let top = crate::Edges {
        top: 59.,
        bottom: 34.,
        ..crate::Edges::default()
    };
    // Same window, insets only: re-rendered.
    rt.set_viewport(390., 844.);
    find_text(&rt.set_window(390., 844., top), "top 59");
    assert_eq!(seen.lock().unwrap().last().copied(), Some((390., 751.)));
    // Nothing changed: nothing to do.
    let n = seen.lock().unwrap().len();
    assert!(rt.set_window(390., 844., top).is_empty());
    assert_eq!(seen.lock().unwrap().len(), n);
}

#[test]
fn platform_specific_elements_and_styles() {
    use crate::Platform;
    struct Page;
    impl Render for Page {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(text("swipe").only(Platform::MOBILE))
                .child(text("right-click").except(Platform::MOBILE))
                .child(text("ios only").only(Platform::Ios))
                .child(
                    text(cx.platform().name())
                        .p(4.)
                        .on(Platform::Web, |s| s.p(24.)),
                )
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Page);
        cx.set_root(root);
    });
    let texts = |platform| {
        let mut rt = Runtime::with_options(
            &init,
            crate::RuntimeOptions {
                path: "/".into(),
                platform: Some(platform),
                ..Default::default()
            },
        );
        rt.render()
    };
    let has = |patches: &[Patch], t: &str| {
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetText { text, .. } if text == t))
    };
    let web = texts(Platform::Web);
    assert!(has(&web, "right-click") && has(&web, "web"));
    assert!(!has(&web, "swipe") && !has(&web, "ios only"));
    let padded = |patches: &[Patch], v: f32| {
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetStyle { style, .. } if style.padding.top == v))
    };
    assert!(padded(&web, 24.) && !padded(&web, 4.));
    let ios = texts(Platform::Ios);
    assert!(has(&ios, "swipe") && has(&ios, "ios only") && !has(&ios, "right-click"));
    assert!(padded(&ios, 4.));
    assert!((Platform::Ios | Platform::Web).contains(Platform::Web));
    assert!(!Platform::NATIVE.contains(Platform::Web));
}

#[test]
fn keyboard_toolbars_follow_their_fields_on_touch_platforms() {
    // #242: a field's own bar comes right after it ("keyboard field"); a
    // container's bar serves every field inside; desktops leave both out.
    use crate::Platform;
    struct Page;
    impl Render for Page {
        fn render(&mut self, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(text_input().key("title").keyboard_toolbar(button("Bold")))
                .child(text_area())
                .keyboard_toolbar(button("Done"))
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Page);
        cx.set_root(root);
    });
    let render = |platform| {
        let mut rt = Runtime::with_options(
            &init,
            crate::RuntimeOptions {
                path: "/".into(),
                platform: Some(platform),
                ..Default::default()
            },
        );
        rt.render()
    };
    let bars = |patches: &[Patch]| -> Vec<(crate::NodeId, String)> {
        patches
            .iter()
            .filter_map(|p| match p {
                Patch::SetConfig { id, config } if config.starts_with("keyboard") => {
                    Some((*id, config.clone()))
                }
                _ => None,
            })
            .collect()
    };
    for platform in [Platform::Ios, Platform::Android, Platform::Web] {
        let patches = render(platform);
        let found = bars(&patches);
        assert_eq!(found.len(), 2, "{platform:?}");
        assert_eq!(found[0].1, "keyboard field");
        assert_eq!(found[1].1, "keyboard");
        let c = crate::controls::OverlayConfig::decode(&found[0].1);
        assert_eq!(c.kind, crate::OverlayKind::Keyboard);
        assert!(c.field_only);
        // Always open: the platform shows it with the field's focus.
        assert!(patches.iter().any(
            |p| matches!(p, Patch::SetValue { id, value } if *id == found[0].0 && *value == 1.)
        ));
        // The field, its own bar, the text area, the shared bar.
        let order = patches
            .iter()
            .rev()
            .find_map(|p| match p {
                Patch::SetChildren { children, .. } if children.len() == 4 => {
                    Some(children.clone())
                }
                _ => None,
            })
            .expect("the row's children");
        assert_eq!(order[1], found[0].0);
        assert_eq!(order[3], found[1].0);
        assert!(
            patches
                .iter()
                .any(|p| matches!(p, Patch::SetText { text, .. } if text == "Bold"))
        );
    }
    for platform in [
        Platform::Macos,
        Platform::Linux,
        Platform::Tvos,
        Platform::Watchos,
        Platform::Visionos,
    ] {
        let patches = render(platform);
        assert!(bars(&patches).is_empty(), "{platform:?}");
        assert!(
            !patches.iter().any(
                |p| matches!(p, Patch::SetText { text, .. } if text == "Bold" || text == "Done")
            )
        );
    }
}

#[test]
fn device_info_drives_rendering_and_styles() {
    use crate::style::FlexDirection;
    use crate::{DeviceInfo, DeviceKind, Fold, Orientation, Posture};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Page;
    impl Render for Page {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            let d = cx.device_info();
            div()
                .flex_col()
                .landscape(|s| s.flex_row())
                .posture(Posture::Tabletop, |s| s.gap(40.))
                .on_device(DeviceKind::Tablet, |s| s.p(32.))
                .child(text(format!("{} {}", d.kind.name(), d.orientation.name())))
                .child(text(if d.fold.is_some() { "hinge" } else { "flat" }))
        }
    }
    let changes = Arc::new(AtomicUsize::new(0));
    let seen = changes.clone();
    let init: AppInit = Arc::new(move |cx: &mut App| {
        let seen = seen.clone();
        cx.on_device_info_change(move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        let root = cx.new_entity(|_| Page);
        cx.set_root(root);
    });
    let mut rt = Runtime::with_options(
        &init,
        crate::RuntimeOptions {
            platform: Some(crate::Platform::Ios),
            ..Default::default()
        },
    );
    let first = rt.render();
    // Before the platform reports: guessed from the platform and window.
    find_text(&first, "phone portrait");
    let style = |patches: &[Patch], f: &dyn Fn(&crate::Style) -> bool| {
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetStyle { style, .. } if f(style)))
    };
    assert!(style(&first, &|s| s.direction == FlexDirection::Column));

    // An iPad in landscape: the row layout and the tablet padding apply.
    let mut ipad = DeviceInfo::guess(crate::Platform::Ios, (1180., 820.));
    ipad.kind = DeviceKind::Tablet;
    let patches = rt.set_device_info(ipad.clone());
    find_text(&patches, "tablet landscape");
    assert!(style(&patches, &|s| s.direction == FlexDirection::Row
        && s.padding.top == 32.));
    assert_eq!(changes.load(Ordering::SeqCst), 1);
    // The same report again changes nothing.
    assert!(rt.set_device_info(ipad.clone()).is_empty());
    assert_eq!(changes.load(Ordering::SeqCst), 1);

    // A foldable half open on a table.
    let mut fold = ipad;
    fold.kind = DeviceKind::Phone;
    fold.orientation = Orientation::Portrait;
    fold.posture = Posture::Tabletop;
    fold.fold = Some(Fold {
        y: 400.,
        width: 400.,
        height: 20.,
        ..Default::default()
    });
    let patches = rt.set_device_info(fold);
    find_text(&patches, "hinge");
    assert!(style(&patches, &|s| s.gap == 40. && s.padding.top == 0.));
    assert!(rt.app.device_info().is_folded());
    assert_eq!(changes.load(Ordering::SeqCst), 2);
}

#[test]
fn user_agents_describe_the_visitor_device() {
    use crate::DeviceKind;
    use crate::device_info::{Engine, OsName, Version, parse_user_agent};
    let iphone = parse_user_agent(
        "Mozilla/5.0 (iPhone; CPU iPhone OS 18_2 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/131.0.6778.73 Mobile/15E148 Safari/604.1",
        None,
    );
    assert_eq!(iphone.kind, DeviceKind::Phone);
    assert_eq!(iphone.os.name, OsName::Ios);
    assert_eq!(iphone.os.version, Version::new(18, 2, 0));
    let b = iphone.browser.as_ref().unwrap();
    // Chrome on iOS still uses WebKit.
    assert_eq!(
        (b.name.as_str(), b.version.major, b.engine),
        ("Chrome", 131, Engine::WebKit)
    );
    assert!(iphone.touch && iphone.is_browser());

    let mac_ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15";
    let mac = parse_user_agent(mac_ua, Some(false));
    assert_eq!(
        (mac.kind, mac.os.name),
        (DeviceKind::Desktop, OsName::Macos)
    );
    assert_eq!(mac.browser.as_ref().unwrap().name, "Safari");
    // An iPad asks for the desktop site: touch gives it away.
    let ipad = parse_user_agent(mac_ua, Some(true));
    assert_eq!(
        (ipad.kind, ipad.os.name),
        (DeviceKind::Tablet, OsName::Ipados)
    );
    assert!(ipad.os.version.at_least(18, 1));

    let fold = parse_user_agent(
        "Mozilla/5.0 (Linux; Android 14; SM-F956B) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/26.0 Chrome/122.0.0.0 Mobile Safari/537.36",
        None,
    );
    assert_eq!(
        (fold.kind, fold.os.name),
        (DeviceKind::Phone, OsName::Android)
    );
    assert_eq!(fold.model.as_deref(), Some("SM-F956B"));
    assert_eq!(fold.browser.as_ref().unwrap().name, "Samsung Internet");

    let tab = parse_user_agent(
        "Mozilla/5.0 (Linux; Android 13; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        None,
    );
    assert_eq!(tab.kind, DeviceKind::Tablet);

    let edge = parse_user_agent(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.2903.70",
        None,
    );
    let b = edge.browser.as_ref().unwrap();
    assert_eq!(
        (edge.os.name, b.name.as_str(), b.engine),
        (OsName::Windows, "Edge", Engine::Blink)
    );
    assert!(!edge.touch);

    let firefox = parse_user_agent(
        "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0",
        None,
    );
    assert_eq!(firefox.browser.unwrap().engine, Engine::Gecko);

    assert_eq!(Version::parse("10_15_7").to_string(), "10.15.7");
    assert!(Version::parse("18.10") > Version::parse("18.9"));
    // The model table comes from Xcode's simulator profiles.
    let pro = crate::device_info::apple_model("iPhone17,1").unwrap();
    assert_eq!(pro.name, "iPhone 16 Pro");
    let (_, ppi) = pro.screen(2622, 1206).unwrap();
    assert_eq!(
        crate::device_info::diagonal_inches(1206., 2622., ppi),
        Some(6.3)
    );
    let duo = crate::device_info::apple_model("iPhone19,4").unwrap();
    assert!(duo.is_foldable() && !pro.is_foldable());
    let (outer, _) = duo.screen(1398, 2034).unwrap();
    assert!(duo.is_smallest(outer));
}

#[test]
fn car_templates_stack_select_and_back() {
    use crate::PlatformRequest;
    struct Plain;
    impl Render for Plain {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    struct Count(i32);
    let init: AppInit = Arc::new(|cx: &mut App| {
        cx.set_global(Count(0));
        cx.register_car(|cx| {
            let n = cx.global::<Count>().0;
            CarTemplate::list("Home").section(
                CarSection::new()
                    .item(CarItem::new(format!("Count {n}")).on_select(|cx| {
                        let n = cx.global::<Count>().0;
                        cx.set_global(Count(n + 1));
                    }))
                    .item(
                        CarItem::new("Details")
                            .browsable()
                            .drawing(crate::Drawing {
                                inset: (2., 2.),
                                paths: vec![
                                    crate::Path::arc(0.5, 0.5, 0.5, 0., 180.)
                                        .stroke(rgb(0xFF0000), 4.),
                                ],
                            })
                            .on_select(|cx| {
                                cx.car_push(|_| CarTemplate::pane("Details").row("Speed", "Fast"));
                            }),
                    ),
            )
        });
        let root = cx.new_entity(|_| Plain);
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    rt.render();
    let car = |rt: &mut Runtime| {
        rt.take_requests().into_iter().find_map(|r| match r {
            PlatformRequest::SetCar(json) => Some(json),
            _ => None,
        })
    };
    // Nothing until a car screen shows the app.
    assert!(car(&mut rt).is_none());
    rt.car_connected(true);
    let json = car(&mut rt).expect("templates when connected");
    assert!(json.contains(r#""title":"Count 0""#), "{json}");
    assert!(json.contains(r#""browsable":true"#), "{json}");
    // Charts in the image slot travel as compact drawings.
    assert!(json.contains(r#""drawing":null"#), "{json}");
    assert!(
        json.contains(r#""drawing":"i2,2\ns#FF0000 w4|A .5 .5 .5 0 180\n""#),
        "{json}"
    );
    // Unchanged: nothing new.
    assert!(car(&mut rt).is_none());
    // Item 0 increments; the list is rebuilt.
    rt.car_select(0);
    assert!(car(&mut rt).unwrap().contains("Count 1"));
    // Item 1 pushes a pane.
    rt.car_select(1);
    let json = car(&mut rt).unwrap();
    assert!(
        json.contains(r#""t":"pane""#) && json.contains(r#"["Speed","Fast"]"#),
        "{json}"
    );
    // The host went back to the root.
    rt.car_back(0);
    assert!(!car(&mut rt).unwrap().contains("pane"));
}

// --- Motion -----------------------------------------------------------------

struct Grow {
    big: bool,
}

impl Render for Grow {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(text(if self.big { "big" } else { "small" }).enter(Pose::fade()))
            .child(button("grow").on_click(cx.listener(|this, _, cx| {
                cx.with_animation(Transition::spring(300), |_| {});
                this.big = !this.big;
            })))
            .child(button("plain").on_click(cx.listener(|this, _, _| this.big = !this.big)))
    }
}

fn styles(patches: &[Patch]) -> Vec<(u64, Option<Transition>)> {
    patches
        .iter()
        .filter_map(|p| match p {
            Patch::SetStyle { id, style } => Some((*id, style.transition)),
            _ => None,
        })
        .collect()
}

#[test]
fn with_animation_animates_one_update_only() {
    let (mut rt, first) = boot(|cx| {
        let root = cx.new_entity(|_| Grow { big: false });
        cx.set_root(root);
    });
    // Animations are part of the style.
    assert!(first.iter().any(|p| matches!(p,
        Patch::SetStyle { style, .. } if style.animations.enter == Some(Pose::fade()))));
    let animated = rt.dispatch(UiEvent::Click(find_text(&first, "grow")));
    let t = Some(Transition::spring(300));
    assert!(!styles(&animated).is_empty());
    assert!(styles(&animated).iter().all(|(_, tr)| *tr == t));
    // The next update is instant again: the animated nodes lose the transition.
    let plain = rt.dispatch(UiEvent::Click(find_text(&first, "plain")));
    assert!(!styles(&plain).is_empty());
    assert!(styles(&plain).iter().all(|(_, tr)| tr.is_none()));
    assert!(styles(&rt.dispatch(UiEvent::Click(find_text(&first, "plain")))).is_empty());
}

#[test]
fn push_registration_messages_and_background_jobs() {
    use crate::{
        Capabilities, Lifecycle, NotificationResponse, PlatformRequest, PushEvent, PushMessage,
        PushService, PushSubscription,
    };
    use std::sync::Mutex;
    struct Plain;
    impl Render for Plain {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    let log: Arc<Mutex<Vec<String>>> = Default::default();
    let seen = log.clone();
    let init: AppInit = Arc::new(move |cx: &mut App| {
        let (a, b, c, d) = (seen.clone(), seen.clone(), seen.clone(), seen.clone());
        cx.on_push_token(move |s, _| a.lock().unwrap().push(format!("token {}", s.token)));
        cx.on_push(move |p, _| {
            b.lock().unwrap().push(format!(
                "{:?} {}",
                p.event,
                p.title.clone().unwrap_or_default()
            ))
        });
        cx.on_background_push(move |p, cx| {
            let c = c.clone();
            async move {
                cx.sleep(std::time::Duration::from_millis(20)).await;
                c.lock()
                    .unwrap()
                    .push(format!("synced {}", p.get("sync").unwrap_or("")));
            }
        });
        cx.spawn(async move |cx| {
            let r = cx.register_for_push().await;
            d.lock()
                .unwrap()
                .push(format!("awaited {:?}", r.map(|s| s.service)));
        });
        let root = cx.new_entity(|_| Plain);
        cx.set_root(root);
    });
    let caps = Capabilities {
        push: Some(PushService::UnifiedPush),
        ..Default::default()
    };
    let mut rt = Runtime::with_capabilities(&init, "/", caps);
    rt.render();
    assert!(
        rt.take_requests()
            .iter()
            .any(|r| matches!(r, PlatformRequest::RegisterPush))
    );
    let mut sub = PushSubscription::new(PushService::UnifiedPush, "https://up.example/1");
    sub.p256dh = Some("BCV".into());
    sub.auth = Some("BTB".into());
    rt.push_registered(Ok(sub.clone()));
    // The same token again is not news.
    rt.push_registered(Ok(sub));
    assert_eq!(
        rt.app.push_subscription().unwrap().token,
        "https://up.example/1"
    );

    // In the background, a visible push becomes a notification; its tap is
    // an `Opened` push.
    rt.set_lifecycle(Lifecycle::Background);
    let (m, _) = PushMessage::from_payload(
        PushEvent::Received,
        r#"{"title":"Hi","body":"There","id":"m1"}"#,
    )
    .unwrap();
    rt.push_received(m, true);
    let notify = rt
        .take_requests()
        .into_iter()
        .find_map(|r| match r {
            PlatformRequest::Notify(n) => Some(n),
            _ => None,
        })
        .expect("shown as a notification");
    assert_eq!(notify.id, "prism-push:m1");
    rt.notification_response(
        NotificationResponse {
            id: notify.id,
            action: None,
            text: None,
        },
        None,
    );

    // A silent push runs the background job.
    let (silent, is_silent) =
        PushMessage::from_payload(PushEvent::Received, r#"{"data":{"sync":"inbox"}}"#).unwrap();
    assert!(is_silent && silent.event == PushEvent::Background);
    let task = rt.run_background_push(silent).unwrap();
    assert!(rt.is_task_running(task));
    std::thread::sleep(std::time::Duration::from_millis(30));
    rt.poll();
    assert!(!rt.is_task_running(task));
    assert_eq!(
        *log.lock().unwrap(),
        [
            "token https://up.example/1",
            "awaited Ok(UnifiedPush)",
            "Received Hi",
            "Opened Hi",
            "Background ",
            "synced inbox"
        ]
    );
}

#[test]
fn push_is_unavailable_without_the_capability() {
    use std::sync::Mutex;
    let out: Arc<Mutex<Option<String>>> = Default::default();
    let o = out.clone();
    let init: AppInit = Arc::new(move |cx: &mut App| {
        let o = o.clone();
        cx.spawn(async move |cx| {
            let r = cx.register_for_push().await;
            *o.lock().unwrap() = Some(format!("{r:?}"));
        });
    });
    let mut rt = Runtime::new(&init);
    rt.poll();
    assert_eq!(out.lock().unwrap().as_deref(), Some("Err(Unavailable)"));
}

// --- Form controls ----------------------------------------------------------

struct Form {
    volume: f64,
    size: Option<usize>,
    submitted: u32,
}

impl Render for Form {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(
                slider(self.volume)
                    .range(0., 10.)
                    .step(0.5)
                    .accessibility_label("Volume")
                    .on_change(cx.listener(|this, e: &ChangeEvent, _| this.volume = e.value)),
            )
            .child(
                picker(["S", "M"], self.size)
                    .on_change(cx.listener(|this, e: &ChangeEvent, _| this.size = Some(e.index()))),
            )
            .child(
                text_input()
                    .secure(true)
                    .return_key(ReturnKey::Go)
                    .on_submit(cx.listener(|this, _, _| this.submitted += 1)),
            )
            .child(text(format!("submitted {}", self.submitted)))
    }
}

fn node_of(patches: &[Patch], kind: NodeKind) -> u64 {
    patches
        .iter()
        .find_map(|p| match p {
            Patch::Create { id, kind: k } if *k == kind => Some(*id),
            _ => None,
        })
        .expect("node created")
}

#[test]
fn controls_get_their_config_before_their_value_and_a_spoken_value() {
    let (mut rt, first) = boot(|cx| {
        let root = cx.new_entity(|_| Form {
            volume: 2.5,
            size: None,
            submitted: 0,
        });
        cx.set_root(root);
    });
    let s = node_of(&first, NodeKind::Slider);
    let pos = |pred: &dyn Fn(&Patch) -> bool| first.iter().position(pred).unwrap();
    let config =
        pos(&|p| matches!(p, Patch::SetConfig { id, config } if *id == s && config == "0 10 0.5"));
    let value = pos(&|p| matches!(p, Patch::SetValue { id, value } if *id == s && *value == 2.5));
    assert!(config < value);
    assert!(
        first
            .iter()
            .any(|p| matches!(p, Patch::SetAccessibility { id, a11y }
        if *id == s && a11y.role == Some(Role::Slider) && a11y.value.as_deref() == Some("2.5")))
    );
    let input = node_of(&first, NodeKind::TextInput);
    assert!(
        first
            .iter()
            .any(|p| matches!(p, Patch::SetConfig { id, config }
        if *id == input && config == "secure return=go"))
    );

    let picker = node_of(&first, NodeKind::Picker);
    let patches = rt.dispatch(UiEvent::Change(picker, 1.));
    assert!(
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetAccessibility { id, a11y }
        if *id == picker && a11y.value.as_deref() == Some("M")))
    );
    let patches = rt.dispatch(UiEvent::Submit(input));
    assert!(
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetText { text, .. } if text == "submitted 1"))
    );
}

#[test]
fn corners_combine_and_mirror() {
    use crate::Corners;
    assert_eq!(Corners::default(), Corners::ALL);
    assert_eq!(Corners::TOP | Corners::BOTTOM, Corners::ALL);
    assert!(Corners::TOP.contains(Corners::TOP_TRAILING));
    assert!(!Corners::TOP.contains(Corners::BOTTOM_LEADING));
    assert_eq!(Corners::LEADING.mirrored(), Corners::TRAILING);
    assert_eq!(Corners::TOP_LEADING.mirrored(), Corners::TOP_TRAILING);
    assert_eq!(Corners::BOTTOM_TRAILING.mirrored(), Corners::BOTTOM_LEADING);
    assert_eq!(Corners::TOP.mirrored(), Corners::TOP);
    assert_eq!(Corners::from_bits(0xFF), Corners::ALL);
    let mut d = div().rounded_bottom(6.);
    let s = d.style_mut().clone();
    assert_eq!((s.corner_radius, s.rounded_corners), (6., Corners::BOTTOM));
}

// --- Quiet globals, tasks tied to their entity (#23) ---------------------------

#[derive(Default)]
struct Debounce(Vec<f32>);

struct Shown;

impl Render for Shown {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(text("shown"))
    }
}

#[test]
fn quiet_globals_change_without_re_rendering() {
    let (mut rt, _) = boot(|cx| {
        let root = cx.new_entity(|_| Shown);
        cx.set_root(root);
    });
    rt.app.update_global_quiet(|d: &mut Debounce| d.0.push(0.5));
    rt.app.global_mut::<Debounce>().0.push(0.7);
    assert!(!rt.app.has_dirty(), "nothing re-renders");
    assert_eq!(rt.app.global::<Debounce>().0, vec![0.5, 0.7]);
    rt.app.update_global(|d: &mut Debounce| d.0.clear());
    assert!(
        rt.app.has_dirty(),
        "update_global still re-renders everything"
    );
}

struct Poller;

impl Render for Poller {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[test]
fn tasks_tied_to_an_entity_stop_when_it_is_released() {
    let mut app = App::new();
    let (scoped, free) = (
        std::rc::Rc::new(std::cell::Cell::new(0u32)),
        std::rc::Rc::new(std::cell::Cell::new(0u32)),
    );
    let (s, f) = (scoped.clone(), free.clone());
    let poller = app.new_entity(|_| Poller);
    let (scoped_task, free_task) = poller.update(&mut app, |_, cx| {
        let a = cx
            .spawn(async move |_, cx| {
                loop {
                    s.set(s.get() + 1);
                    cx.sleep(Duration::from_millis(1)).await;
                }
            })
            .cancel_on_release();
        let b = cx.spawn(async move |_, cx| {
            loop {
                f.set(f.get() + 1);
                cx.sleep(Duration::from_millis(1)).await;
            }
        });
        (a.id(), b.id())
    });
    crate::task::poll_tasks(&mut app);
    assert_eq!((scoped.get(), free.get()), (1, 1));
    assert!(app.executor.is_running(scoped_task));
    app.release(poller);
    assert!(
        !app.executor.is_running(scoped_task),
        "cancelled with its entity"
    );
    assert!(app.executor.is_running(free_task), "plain cx.spawn runs on");
    std::thread::sleep(Duration::from_millis(3));
    crate::task::poll_tasks(&mut app);
    assert_eq!((scoped.get(), free.get()), (1, 2));
}

#[test]
fn a_task_can_release_its_own_entity_and_stop() {
    let (mut rt, _) = boot(|cx| {
        let root = cx.new_entity(|_| Shown);
        cx.set_root(root);
    });
    let polls = std::rc::Rc::new(std::cell::Cell::new(0u32));
    let p = polls.clone();
    let owner = rt.app.new_entity(|_| Poller);
    let task = rt
        .app
        .spawn(|cx| async move {
            loop {
                p.set(p.get() + 1);
                // Releasing the owner from inside its own task cancels it.
                cx.update(|app| app.release(owner));
                cx.sleep(Duration::from_millis(1)).await;
            }
        })
        .cancel_with(&owner)
        .id();
    rt.poll();
    assert!(!rt.is_task_running(task));
    std::thread::sleep(Duration::from_millis(3));
    rt.poll();
    assert_eq!(polls.get(), 1, "never polled again");
}

// --- Shapes position like any element (#15), fitted scroll views (#16) --------

#[test]
fn a_shape_takes_the_styled_inset_and_keeps_its_stroke_inset() {
    use crate::{Drawing, Length, Path, shape};
    let mut sh = shape()
        .absolute()
        .inset(0.)
        .stroke_inset(2., 3.)
        .path(Path::line([(0., 0.), (1., 1.)]).stroke(rgb(0x000000), 1.));
    let s = sh.style_mut();
    assert_eq!(
        (s.inset.top, s.inset.left),
        (Length::Px(0.), Length::Px(0.))
    );
    let crate::Element::Node(n) = sh.into_element() else {
        panic!("a node")
    };
    assert_eq!(Drawing::decode(&n.text).inset, (2., 3.));
}

#[test]
fn fitted_scroll_views_size_to_their_content() {
    use crate::Length;
    let mut fitted = scroll().fit().max_h(px(200.));
    let s = fitted.style_mut();
    assert_eq!((s.flex_grow, s.flex_shrink), (0., 1.));
    assert_eq!((s.flex_basis, s.max_height), (Length::Auto, px(200.)));
    let mut plain = scroll();
    let s = plain.style_mut();
    assert_eq!((s.flex_grow, s.flex_basis), (1., Length::Px(0.)));
}

struct Loads {
    status: &'static str,
}

impl Loads {
    fn new(cx: &mut Context<Self>) -> Self {
        // A first load, started by the screen's constructor.
        cx.spawn(async move |this, cx| {
            cx.update_entity(&this, |this, _| this.status = "loading");
        })
        .cancel_on_release();
        Loads { status: "idle" }
    }
}

impl Render for Loads {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen("Loads").child(text(self.status))
    }
}

#[test]
fn tasks_spawned_by_a_screen_opened_by_navigation_start_at_once() {
    let (mut rt, _) = boot(|cx| {
        let r = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| Shown))
            .route("/loads", |cx, _| cx.new_entity(Loads::new))
            .build(cx);
        cx.set_root(r);
    });
    rt.app.navigate("/loads");
    let patches = rt.poll();
    assert!(
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetText { text, .. } if text == "loading")),
        "the task ran before the screen was first shown: {patches:?}"
    );
}

#[test]
fn custom_breakpoints_and_any_width_rules_re_render_when_crossed() {
    use crate::style::{Breakpoint, FlexDirection};
    struct Page;
    impl Render for Page {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex_col()
                .at_width(768., |s| s.flex_row())
                .child(text(format!("{:?}", cx.breakpoint())))
        }
    }
    let (mut rt, first) = boot(|cx| {
        cx.set_breakpoints([480., 900., 1280.]);
        let root = cx.new_entity(|_| Page);
        cx.set_root(root);
    });
    let has_dir = |patches: &[Patch], dir: FlexDirection| {
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetStyle { style, .. } if style.direction == dir))
    };
    assert!(has_dir(&first, FlexDirection::Column));
    find_text(&first, "Compact");
    assert_eq!(rt.app.breakpoints(), [480., 900., 1280.]);
    // 500 is Medium with these breakpoints (Compact by default).
    find_text(&rt.set_viewport(500., 800.), "Medium");
    assert!(rt.set_viewport(700., 800.).is_empty(), "nothing crossed");
    // 768: the `.at_width` rule's threshold re-renders by itself.
    let patches = rt.set_viewport(800., 800.);
    assert!(has_dir(&patches, FlexDirection::Row), "{patches:?}");
    assert_eq!(rt.app.breakpoint(), Breakpoint::Medium);
    find_text(&rt.set_viewport(1300., 800.), "Large");
    assert!(has_dir(&rt.set_viewport(600., 800.), FlexDirection::Column));
    assert_eq!(
        Breakpoint::for_width_in(899., [480., 900., 1280.]),
        Breakpoint::Medium
    );
}

#[test]
fn own_width_rules_wait_for_the_backend() {
    use crate::style::{FlexDirection, StyleContext};
    let mut d = div()
        .gap(4.)
        .when_width_at_least(600., |s| s.gap(24.))
        .when_width_at_least(300., |s| s.flex_row())
        .md(|s| s.p(10.));
    let s = d.style_mut().clone();
    let resolved = s.resolve(StyleContext {
        breakpoint: crate::Breakpoint::Medium,
        ..Default::default()
    });
    // Window rules are applied; own-width rules are kept for the backend.
    assert_eq!(resolved.padding.top, 10.);
    assert_eq!(resolved.own_width_thresholds(), [300., 600.]);
    assert_eq!(resolved.direction, FlexDirection::Column);
    let narrow = resolved.resolve_own_width(200.);
    assert!(narrow.responsive.is_empty());
    assert_eq!((narrow.direction, narrow.gap), (FlexDirection::Column, 4.));
    let mid = resolved.resolve_own_width(300.);
    assert_eq!((mid.direction, mid.gap), (FlexDirection::Row, 4.));
    let wide = resolved.resolve_own_width(1000.);
    assert_eq!(
        (wide.direction, wide.gap, wide.padding.top),
        (FlexDirection::Row, 24., 10.)
    );
}

#[test]
fn layout_builders_set_their_fields() {
    use crate::style::{Align, Position};
    let mut d = div()
        .grid()
        .cols(2)
        .col_span(2)
        .row_span(0)
        .row_start(3)
        .gap_x(3.)
        .gap_y(5.)
        .items_baseline()
        .self_baseline()
        .aspect_ratio(-1.)
        .flex_grow(2.5)
        .flex_shrink(-1.)
        .sticky();
    let s = d.style_mut().clone();
    assert!(s.grid);
    assert_eq!(s.grid_columns, [repeat(2, minmax(0f32, fr(1.)))]);
    assert_eq!((s.col_span, s.row_span, s.row_start), (2, 1, Some(3)));
    assert_eq!((s.gap_x, s.gap_y), (Some(3.), Some(5.)));
    assert_eq!(
        (s.align_items, s.align_self),
        (Align::Baseline, Some(Align::Baseline))
    );
    assert_eq!(s.aspect_ratio, None, "nonsense ratios are ignored");
    assert_eq!((s.flex_grow, s.flex_shrink), (2.5, 0.));
    assert_eq!(s.position, Position::Sticky);
    let mut f = div().fixed();
    assert_eq!(f.style_mut().position, Position::Fixed);
    let mut sx = scroll_x();
    let s = sx.style_mut().clone();
    assert_eq!(s.scroll_axis, ScrollAxis::Horizontal);
    assert_eq!((s.flex_grow, s.direction), (0., crate::FlexDirection::Row));
    // Zoom scales point tracks and gaps, not fractions.
    let z = crate::Style {
        grid_columns: vec![px(10.).into(), minmax(20., fr(1.)), auto_fill(30.)],
        gap_x: Some(2.),
        ..Default::default()
    }
    .scaled(2.);
    assert_eq!(
        z.grid_columns,
        [GridTrack::Px(20.), minmax(40., fr(1.)), auto_fill(60.)]
    );
    assert_eq!(z.gap_x, Some(4.));
    assert_eq!(
        auto_fit(minmax(172, fr(1.))).css(),
        "repeat(auto-fit,minmax(172px,1fr))"
    );
}

// --- Custom window chrome (#67) ---------------------------------------------

#[test]
fn window_chrome_defaults_to_the_standard_title_bar() {
    let config = crate::AppConfig::default();
    assert_eq!(config.window_chrome, WindowChrome::Standard);
    assert!(!config.window_chrome.is_custom());
    let custom = WindowChrome::Custom {
        traffic_lights: Some(Point::new(16., 13.)),
    };
    assert!(custom.is_custom());
    assert!(
        WindowChrome::Custom {
            traffic_lights: None
        }
        .is_custom()
    );
}

#[test]
fn window_drag_reaches_the_backend_as_a_style() {
    struct Bar {
        drag: bool,
    }
    impl Render for Bar {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            let bar = div().h(40.).child(
                button("Tab").on_click(cx.listener(|this: &mut Bar, _, _| this.drag = !this.drag)),
            );
            if self.drag { bar.window_drag() } else { bar }
        }
    }
    let (mut rt, first) = boot(|cx| {
        let root = cx.new_entity(|_| Bar { drag: true });
        cx.set_root(root);
    });
    let dragged: Vec<u64> = first
        .iter()
        .filter_map(|p| match p {
            Patch::SetStyle { id, style } if style.window_drag => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(dragged.len(), 1, "only the bar drags the window");
    let bar = dragged[0];
    // The button inside keeps its own style and its clicks.
    let tab = find_text(&first, "Tab");
    assert_ne!(tab, bar);
    let patches = rt.dispatch(UiEvent::Click(tab));
    let restyled = patches.iter().find_map(|p| match p {
        Patch::SetStyle { id, style } if *id == bar => Some(style.window_drag),
        _ => None,
    });
    assert_eq!(restyled, Some(false), "turning it off restyles the bar");
    assert!(!div().style_mut().window_drag);
    assert!(div().window_drag().style_mut().window_drag);
}

#[test]
fn fullscreen_and_window_controls_re_render() {
    struct Title;
    impl Render for Title {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            let lead = cx.window_controls().map_or(12., |c| c.right() + 12.);
            text(format!("fs {} lead {lead}", cx.is_fullscreen()))
        }
    }
    let (mut rt, first) = boot(|cx| {
        let root = cx.new_entity(|_| Title);
        cx.set_root(root);
    });
    find_text(&first, "fs false lead 12");
    let lights = WindowControls {
        x: 16.,
        y: 13.,
        width: 52.,
        height: 14.,
    };
    assert_eq!((lights.right(), lights.bottom()), (68., 27.));
    find_text(
        &rt.set_window_chrome(false, Some(lights)),
        "fs false lead 80",
    );
    // Nothing changed: nothing to do.
    assert!(rt.set_window_chrome(false, Some(lights)).is_empty());
    // Full screen hides the traffic lights: the title bar drops their room.
    find_text(&rt.set_window_chrome(true, None), "fs true lead 12");
    assert!(rt.app.is_fullscreen());
    assert_eq!(rt.app.window_controls(), None);
}

#[test]
fn an_explicit_zero_padding_is_set_and_survives_scaling() {
    use crate::{Edges, Style, Styled, text_area, text_input};
    fn style<S: Styled>(mut el: S) -> Style {
        el.style_mut().clone()
    }
    // No padding given: the control keeps its own inset.
    assert!(!style(text_input()).has_padding());
    // `.p(0.)` and every side setter: a real zero inset (#69).
    for s in [
        style(text_input().p(0.)),
        style(text_input().px(0.)),
        style(text_input().py(0.)),
        style(text_area().pt(0.)),
        style(text_area().pr(0.)),
        style(text_area().pb(0.)),
        style(text_area().pl(0.)),
    ] {
        assert!(s.has_padding(), "{s:?}");
        assert_eq!(s.padding, Edges::default());
        assert!(s.scaled(2.).has_padding(), "kept when zoomed");
    }
    // A padding set directly counts when it isn't zero.
    let direct = Style {
        padding: Edges::all(4.),
        ..Style::default()
    };
    assert!(direct.has_padding());
}

#[test]
fn reversed_directions_and_order_work_in_refinements() {
    use crate::style::{FlexDirection, StyleContext};
    let mut d = div()
        .flex_row()
        .order(2)
        .md(|s| s.flex_row_reverse().order(-1))
        .lg(|s| s.flex_col_reverse().order(0));
    let s = d.style_mut().clone();
    let at = |breakpoint| {
        s.resolve(StyleContext {
            breakpoint,
            ..Default::default()
        })
    };
    let compact = at(crate::Breakpoint::Compact);
    assert_eq!((compact.direction, compact.order), (FlexDirection::Row, 2));
    let medium = at(crate::Breakpoint::Medium);
    assert_eq!(
        (medium.direction, medium.order),
        (FlexDirection::RowReverse, -1)
    );
    let large = at(crate::Breakpoint::Large);
    assert_eq!(
        (large.direction, large.order),
        (FlexDirection::ColumnReverse, 0)
    );
    assert!(FlexDirection::RowReverse.is_row() && FlexDirection::RowReverse.is_reverse());
    assert!(!FlexDirection::ColumnReverse.is_row() && FlexDirection::ColumnReverse.is_reverse());
    assert!(!FlexDirection::Row.is_reverse());
}

#[test]
fn snapshots_resolve_order_and_reversed_directions() {
    let mut app = App::new();
    let node = snapshot_one(
        &mut app,
        div()
            .flex_row_reverse()
            .child(text("a"))
            .child(text("b").order(-1))
            .child(text("c")),
    );
    // Sorted by order (b, a, c), then reversed: renderers draw in order.
    assert_eq!(
        node.to_json(),
        r#"{"k":"v","d":1,"jc":"e","c":[{"k":"t","s":"c"},{"k":"t","s":"a"},{"k":"t","s":"b"}]}"#
    );
    let col = snapshot_one(
        &mut app,
        div()
            .flex_col_reverse()
            .justify_end()
            .child(text("a"))
            .child(text("b")),
    );
    assert_eq!(
        col.to_json(),
        r#"{"k":"v","c":[{"k":"t","s":"b"},{"k":"t","s":"a"}]}"#
    );
}

/// #210: `save_file` sends the file and answers where it went, `None` when
/// the user cancels; `reveal_path` is a call of its own.
#[test]
fn saved_files_answer_where_they_went() {
    use crate::PlatformRequest;
    use crate::device::*;
    struct Saver {
        text: String,
    }
    impl Render for Saver {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            text(self.text.clone())
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|cx: &mut Context<Saver>| {
            cx.spawn(async move |this, cx| {
                let first = cx
                    .save_file(SaveFile::bytes(b"%PDF".to_vec(), "report.pdf"))
                    .await;
                let path = match &first {
                    Ok(Some(SavedFile {
                        path: Some(p),
                        name,
                    })) => format!("{} {name}", p.display()),
                    other => format!("{other:?}"),
                };
                cx.update_entity(&this, |s, cx| {
                    s.text = path;
                    cx.notify();
                });
                let second = cx
                    .save_file(SaveFile::path("/data/cache/q3.csv").name("Q3.csv"))
                    .await;
                cx.update_entity(&this, |s, cx| {
                    s.text = format!("second {second:?}");
                    cx.notify();
                });
            });
            Saver { text: "…".into() }
        });
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    rt.render();
    let saved = |rt: &mut Runtime| {
        rt.take_requests()
            .into_iter()
            .find_map(|r| match r {
                PlatformRequest::Device {
                    id,
                    call: DeviceCall::SaveFile(f),
                } => Some((id, f)),
                _ => None,
            })
            .unwrap()
    };
    let (id, f) = saved(&mut rt);
    assert_eq!(f.source, SaveSource::Bytes(b"%PDF".to_vec()));
    assert_eq!(
        (f.file_name(), f.mime_type()),
        ("report.pdf".into(), "application/pdf".into())
    );
    let file = PickedFile {
        path: "/Users/me/Documents/report 2.pdf".into(),
        name: "report 2.pdf".into(),
        mime: "application/pdf".into(),
        size: 4,
    };
    find_text(
        &rt.device_reply(id, Ok(DeviceValue::Files(vec![file]))),
        "/Users/me/Documents/report 2.pdf report 2.pdf",
    );
    let (id, f) = saved(&mut rt);
    assert_eq!(f.source, SaveSource::Path("/data/cache/q3.csv".into()));
    assert_eq!(
        (f.file_name(), f.mime_type()),
        ("Q3.csv".into(), "text/csv".into())
    );
    find_text(
        &rt.device_reply(id, Err(DeviceError::Cancelled)),
        "second Ok(None)",
    );

    let mut app = App::new();
    app.reveal_path("/tmp/report.pdf");
    assert!(matches!(
        app.take_requests().as_slice(),
        [PlatformRequest::Device { call: DeviceCall::RevealPath(p), .. }] if p.as_os_str() == "/tmp/report.pdf"
    ));
}

#[test]
fn save_file_names_are_file_names() {
    use crate::device::SaveFile;
    let name = |n: &str| SaveFile::bytes(Vec::new(), n).file_name();
    assert_eq!(name("../../etc/passwd"), "_.._etc_passwd");
    assert_eq!(name(" .hidden "), "hidden");
    assert_eq!(name("a\nb.txt"), "a_b.txt");
    assert_eq!(name(""), "file");
    assert_eq!(
        SaveFile::bytes(Vec::new(), "x.bin")
            .mime("application/x-thing")
            .mime_type(),
        "application/x-thing"
    );
    assert_eq!(
        SaveFile::bytes(Vec::new(), "NOTES.TXT").mime_type(),
        "text/plain"
    );
    assert_eq!(
        SaveFile::bytes(Vec::new(), "noext").mime_type(),
        "application/octet-stream"
    );
}

/// #239: the badge is a request of its own.
#[test]
fn the_badge_is_set_on_its_own() {
    let mut app = App::new();
    app.set_badge(Some(5));
    app.set_badge(None);
    let r = app.take_requests();
    assert!(matches!(
        r.as_slice(),
        [
            crate::PlatformRequest::SetBadge(Some(5)),
            crate::PlatformRequest::SetBadge(None)
        ]
    ));
}

/// #225: a folder answer, natively (the folder first) and on the web
/// (names starting with the folder's).
#[test]
fn picked_folders_keep_their_tree() {
    use crate::device::{FOLDER_MIME, PickedFile, PickedFolder};
    let f = |path: &str, name: &str, mime: &str| PickedFile {
        path: path.into(),
        name: name.into(),
        mime: mime.into(),
        size: 1,
    };
    let native = PickedFolder::from_files(vec![
        f("/Users/me/Export", "Export", FOLDER_MIME),
        f("/Users/me/Export/a.md", "a.md", "text/markdown"),
        f("/Users/me/Export/notes/b.md", "notes/b.md", "text/markdown"),
        f("/Users/me/Export/.git/HEAD", ".git/HEAD", "text/plain"),
    ])
    .unwrap();
    assert_eq!(native.name, "Export");
    assert_eq!(native.path.as_deref(), Some("/Users/me/Export"));
    let names: Vec<_> = native.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["a.md", "notes/b.md"], "hidden ones left out");
    // Copied for the app: no path.
    let copied = PickedFolder::from_files(vec![f("", "Export", FOLDER_MIME)]).unwrap();
    assert_eq!((copied.path, copied.files.len()), (None, 0));
    let web = PickedFolder::from_files(vec![
        f("/files/1", "Export/a.md", "text/markdown"),
        f("/files/2", "Export/notes/b.md", "text/markdown"),
        f("/files/3", "Export/.DS_Store", ""),
    ])
    .unwrap();
    assert_eq!(web.name, "Export");
    let names: Vec<_> = web.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["a.md", "notes/b.md"]);
    assert_eq!(PickedFolder::from_files(Vec::new()), None);
}

/// #223: every cursor has its CSS (and GTK) name, and back.
#[test]
fn cursors_round_trip_through_their_css_names() {
    use crate::decoration::Cursor as C;
    for c in [
        C::ResizeN,
        C::ResizeS,
        C::ResizeE,
        C::ResizeW,
        C::ResizeNe,
        C::ResizeNw,
        C::ResizeSe,
        C::ResizeSw,
        C::ResizeNeSw,
        C::ResizeNwSe,
        C::ZoomIn,
        C::ZoomOut,
        C::ResizeHorizontal,
    ] {
        assert_eq!(C::from_css(c.css()), Some(c), "{}", c.css());
    }
    assert_eq!(C::ResizeNwSe.css(), "nwse-resize");
    assert_eq!(C::ZoomIn.css(), "zoom-in");
}

/// #246: a clipboard item keeps its types in order, one data per type; the
/// image is the PNG first; files come from `text/uri-list`.
#[test]
fn clipboard_items_hold_several_types() {
    use crate::ClipboardItem;
    let item = ClipboardItem::new()
        .text("caption")
        .image(b"jpeg".to_vec(), "image/jpeg")
        .image(b"png".to_vec(), "Image/PNG")
        .custom("application/x-notes", br#"{"b":1}"#.to_vec())
        .text("Caption");
    assert_eq!(
        item.types(),
        [
            "text/plain",
            "image/jpeg",
            "image/png",
            "application/x-notes"
        ]
    );
    assert_eq!(item.as_text().as_deref(), Some("Caption"));
    assert_eq!(item.as_html(), None);
    let image = item.as_image().unwrap();
    assert_eq!(
        (image.bytes.as_slice(), image.mime.as_str()),
        (&b"png"[..], "image/png")
    );
    assert_eq!(item.get("application/x-notes"), Some(&br#"{"b":1}"#[..]));
    assert!(!item.has("text/rtf"));
    let files = ClipboardItem::new()
        .custom(ClipboardItem::URI_LIST, "# copied\r\nfile:///Users/me/My%20Shot.png\r\nfile://localhost/tmp/a.txt\r\nhttps://x.dev/\r\n");
    assert_eq!(
        files.files(),
        [
            std::path::PathBuf::from("/Users/me/My Shot.png"),
            std::path::PathBuf::from("/tmp/a.txt")
        ]
    );
    assert!(ClipboardItem::is_text("text/html", b"<b>x</b>"));
    assert!(!ClipboardItem::is_text("image/png", b"png"));
}

/// #246: `read_clipboard_item` takes the platform's files (one per type),
/// an item, or plain text; `None` when empty. `write_clipboard` sends the
/// whole item.
#[test]
fn clipboard_items_are_read_and_written() {
    use crate::device::*;
    use crate::{ClipboardItem, PlatformRequest};
    struct Paster {
        text: String,
    }
    impl Render for Paster {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            text(self.text.clone())
        }
    }
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|cx: &mut Context<Paster>| {
            cx.spawn(async move |this, cx| {
                for round in 0..4 {
                    let item = cx.read_clipboard_item().await;
                    let shown = match &item {
                        Some(i) => format!(
                            "{round}: {} {:?} {:?}",
                            i.types().join(","),
                            i.as_text(),
                            i.as_image().map(|img| img.bytes.len())
                        ),
                        None => format!("{round}: none"),
                    };
                    cx.update_entity(&this, |s, cx| {
                        s.text = shown;
                        cx.notify();
                    });
                }
            });
            Paster { text: "…".into() }
        });
        cx.set_root(root);
    });
    let mut rt = Runtime::new(&init);
    rt.render();
    let read = |rt: &mut Runtime| {
        rt.take_requests()
            .into_iter()
            .find_map(|r| match r {
                PlatformRequest::Device {
                    id,
                    call: DeviceCall::ReadClipboardItem,
                } => Some(id),
                _ => None,
            })
            .unwrap()
    };
    let dir = std::env::temp_dir().join(format!("prism-clip-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = |n: &str, ty: &str, data: &[u8]| {
        let path = dir.join(n);
        std::fs::write(&path, data).unwrap();
        PickedFile {
            path: path.to_string_lossy().into_owned(),
            name: ty.into(),
            mime: ty.into(),
            size: data.len() as u64,
        }
    };
    let id = read(&mut rt);
    let files = vec![
        file("0", "image/png", b"\x89PNG"),
        file("1", "text/plain", "légende".as_bytes()),
    ];
    find_text(
        &rt.device_reply(id, Ok(DeviceValue::Files(files))),
        "0: image/png,text/plain Some(\"légende\") Some(4)",
    );
    let id = read(&mut rt);
    find_text(&rt.device_reply(id, Ok(DeviceValue::Text(None))), "1: none");
    let id = read(&mut rt);
    find_text(
        &rt.device_reply(id, Ok(DeviceValue::Text(Some("hi".into())))),
        "2: text/plain Some(\"hi\") None",
    );
    let id = read(&mut rt);
    find_text(
        &rt.device_reply(
            id,
            Ok(DeviceValue::Clipboard(
                ClipboardItem::new().html("<b>x</b>"),
            )),
        ),
        "3: text/html None None",
    );
    let _ = std::fs::remove_dir_all(&dir);

    let mut app = App::new();
    app.write_clipboard(ClipboardItem::new().text("a").html("<i>a</i>"));
    assert!(matches!(
        app.take_requests().as_slice(),
        [PlatformRequest::Device { call: DeviceCall::WriteClipboard(i), .. }] if i.types() == ["text/plain", "text/html"]
    ));
}

// --- Widgets drawn ahead (#281) ----------------------------------------------

/// Departures every five minutes from now, the next one drawn as a countdown.
fn departures_widget(departures: Vec<std::time::SystemTime>) -> Widget {
    let mut shown = departures.clone();
    shown.sort();
    shown.dedup();
    Widget::new("next", "Next departure")
        .families(&[WidgetFamily::Small])
        .render(move |cx, _| {
            let now = cx.now();
            let left = shown.iter().filter(|t| **t > now).count();
            match shown.iter().find(|t| **t > now) {
                Some(t) => div()
                    .child(text(format!("{left} left")))
                    .child(timer(*t).after("Departed"))
                    .into_element(),
                None => text("No more departures").into_element(),
            }
        })
        .ahead(move |_, _| departures.clone())
}

#[test]
fn a_widget_is_drawn_ahead_for_each_instant_with_that_now() {
    use crate::element::unix_seconds;
    use std::time::{Duration, SystemTime};
    let now = SystemTime::now();
    let at = |mins: u64| now + Duration::from_secs(mins * 60);
    // Unsorted, a duplicate, one already gone.
    let departures = vec![at(10), at(5), at(15), at(10), now - Duration::from_secs(60)];
    let mut app = App::new();
    app.register_widget(departures_widget(departures));
    let snaps = widget_snapshots(&mut app);
    let snap = &snaps[0];
    // Now: three departures to come, the next in five minutes.
    assert!(
        snap.families[0].1.contains("3 left"),
        "{}",
        snap.families[0].1
    );
    let entries = snap.ahead_for(&WidgetConfig::new());
    let instants: Vec<_> = entries.iter().map(|e| e.at).collect();
    assert_eq!(
        instants,
        [at(5), at(10), at(15)],
        "sorted, once, past ones dropped"
    );
    // Each is drawn with `cx.now()` at its instant: what has gone is left out.
    assert!(entries[0].families[0].1.contains("2 left"));
    assert!(entries[1].families[0].1.contains("1 left"));
    assert!(entries[2].families[0].1.contains("No more departures"));
    let next = format!(r#""v":{}"#, unix_seconds(at(10)).round() as i64);
    assert!(
        entries[0].families[0].1.contains(&next),
        "the countdown moves on"
    );
    // Outside a drawing, `now` is the clock again.
    assert!(app.now() >= now && app.now() < now + Duration::from_secs(60));

    let json = snap.to_json();
    let first = format!(
        r#","ahead":[{{"at":{},"families":{{"systemSmall":"#,
        (unix_seconds(at(5)) * 1000.).round() / 1000.
    );
    assert!(json.contains(&first), "{json}");
    assert_eq!(json.matches(r#"{"at":"#).count(), 3);
}

#[test]
fn widgets_drawn_ahead_keep_the_soonest_instants() {
    use std::time::{Duration, SystemTime};
    let now = SystemTime::now();
    let many: Vec<_> = (1..=40)
        .rev()
        .map(|m| now + Duration::from_secs(m * 60))
        .collect();
    let mut app = App::new();
    app.register_widget(departures_widget(many));
    let snaps = widget_snapshots(&mut app);
    let entries = snaps[0].ahead_for(&WidgetConfig::new());
    assert_eq!(entries.len(), Widget::MAX_AHEAD);
    assert_eq!(entries[0].at, now + Duration::from_secs(60));
    assert_eq!(entries[15].at, now + Duration::from_secs(16 * 60));
}

/// Apps that don't draw ahead write the same snapshot as before.
#[test]
fn a_widget_not_drawn_ahead_has_no_entries() {
    let mut app = App::new();
    app.register_widget(
        Widget::new("plain", "Plain")
            .families(&[WidgetFamily::Small])
            .render(|_, _| text("hi")),
    );
    // Instants all gone: nothing ahead either.
    app.register_widget(departures_widget(vec![
        std::time::SystemTime::now() - std::time::Duration::from_secs(1),
    ]));
    for snap in widget_snapshots(&mut app) {
        assert!(snap.ahead.is_empty());
        assert!(!snap.to_json().contains("ahead"), "{}", snap.to_json());
    }
}

/// Each configuration is drawn ahead for its own instants, with its values.
#[test]
fn configurations_are_drawn_ahead_each_for_their_own_instants() {
    use std::time::{Duration, SystemTime};
    let now = SystemTime::now();
    let mut app = App::new();
    app.register_widget(
        Widget::new("line", "Line")
            .families(&[WidgetFamily::Small])
            .parameter(
                WidgetParameter::new("line", "Line")
                    .options(|_| vec![WidgetOption::new("a", "A"), WidgetOption::new("b", "B")]),
            )
            .render_configured(|cx, _, config| {
                let mins = cx
                    .now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    / 60;
                text(format!("{}@{}", config.get("line").unwrap_or("-"), mins))
            })
            .ahead(move |_, config| {
                let gap = if config.get("line") == Some("a") {
                    2
                } else {
                    3
                };
                vec![now + Duration::from_secs(gap * 60)]
            }),
    );
    let snaps = widget_snapshots(&mut app);
    let snap = &snaps[0];
    let mins = |m: u64| {
        (now + Duration::from_secs(m * 60))
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / 60
    };
    let a = WidgetConfig::new().with("line", "a");
    let b = WidgetConfig::new().with("line", "b");
    assert_eq!(snap.default, a);
    assert!(
        snap.ahead_for(&a)[0].families[0]
            .1
            .contains(&format!("a@{}", mins(2)))
    );
    assert!(
        snap.ahead_for(&b)[0].families[0]
            .1
            .contains(&format!("b@{}", mins(3)))
    );
    assert!(
        snap.ahead_for(&WidgetConfig::new().with("line", "z"))
            .is_empty()
    );
    // The default's at the top, the others' with their configuration.
    let json = snap.to_json();
    let (top, configs) = json.split_once(r#""configs":"#).unwrap();
    assert_eq!(top.matches(r#"{"at":"#).count(), 1, "{json}");
    assert!(configs.contains(r#"{"values":{"line":"b"},"families":{"systemSmall":"#));
    assert_eq!(configs.matches(r#"{"at":"#).count(), 1, "{json}");
    assert!(configs.contains(&format!("b@{}", mins(3))));
}

// --- Impossible control events (S13) -----------------------------------------

/// What the controls of [`Guarded`] reported, in order.
struct Guarded {
    log: Vec<String>,
}

impl Render for Guarded {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let change = |name: &'static str| {
            cx.listener(move |this: &mut Self, e: &ChangeEvent, _| {
                this.log.push(format!("{name} {}", e.value));
            })
        };
        let click = |name: &'static str| {
            cx.listener(move |this: &mut Self, _: &ClickEvent, _| this.log.push(name.into()))
        };
        let day = 86_400.;
        div()
            .child(
                button("Pay")
                    .accessibility_disabled(true)
                    .on_click(click("pay")),
            )
            .child(button("Go").on_click(click("go")))
            .child(
                text_input()
                    .accessibility_disabled(true)
                    .on_input(cx.listener(|this: &mut Self, e: &InputEvent, _| {
                        this.log.push(format!("typed {}", e.value));
                    }))
                    .on_submit(click("submit")),
            )
            .child(
                slider(2.)
                    .range(0., 10.)
                    .step(0.5)
                    .on_change(change("slider")),
            )
            .child(picker(["S", "M"], None).on_change(change("picker")))
            .child(toggle(false).on_change(change("toggle")))
            .child(radio_group(["a", "b"], None).on_change(change("radio")))
            .child(
                date_picker(crate::unix_time(0.))
                    .min(crate::unix_time(0.))
                    .max(crate::unix_time(day))
                    .on_change(change("date")),
            )
            .child(date_picker_opt(None).on_change(change("any date")))
            .child(scroll().child(text("no on_scroll")))
            .child(div().track_layout().child(text("tracked")))
            .child(div().child(text("untracked")))
    }
}

fn nth_of(patches: &[Patch], kind: NodeKind, n: usize) -> u64 {
    patches
        .iter()
        .filter_map(|p| match p {
            Patch::Create { id, kind: k } if *k == kind => Some(*id),
            _ => None,
        })
        .nth(n)
        .expect("node created")
}

/// S13 (security review): a modified page can report events its controls
/// could never produce. The dispatcher drops clicks and typing on disabled
/// controls, changes out of a control's range (or snaps them, for a slider
/// or a bounded date picker), events of unknown nodes, and layout and
/// scroll reports of nodes that do not report them, or not finite.
#[test]
fn impossible_control_events_never_reach_the_app() {
    let (mut rt, first) = boot(|cx| {
        let root = cx.new_entity(|_| Guarded { log: Vec::new() });
        cx.set_root(root);
    });
    let pay = find_text(&first, "Pay");
    let field = node_of(&first, NodeKind::TextInput);
    let slider = node_of(&first, NodeKind::Slider);
    let picker = node_of(&first, NodeKind::Picker);
    let toggle = node_of(&first, NodeKind::Toggle);
    let radio = node_of(&first, NodeKind::RadioGroup);
    let date = nth_of(&first, NodeKind::DatePicker, 0);
    let any_date = nth_of(&first, NodeKind::DatePicker, 1);
    let scroll = node_of(&first, NodeKind::Scroll);
    let unknown = 1_000_000;
    let position = Point::new(1., 1.);
    for event in [
        // Disabled controls.
        UiEvent::Click(pay),
        UiEvent::ClickAt(pay, position),
        UiEvent::ClickWith(pay, ClickEvent::default()),
        UiEvent::Input(field, "typed".into()),
        UiEvent::Submit(field),
        // Values no control reports.
        UiEvent::Change(slider, f64::INFINITY),
        UiEvent::Change(picker, 2.),
        UiEvent::Change(picker, -1.),
        UiEvent::Change(picker, 0.5),
        UiEvent::Change(toggle, 7.),
        UiEvent::Change(radio, 5.),
        UiEvent::Change(date, f64::NAN),
        UiEvent::Change(any_date, 1e300),
        // Nodes that are not there.
        UiEvent::Click(unknown),
        UiEvent::Change(unknown, 1.),
    ] {
        rt.dispatch(event);
    }
    let root = first_root::<Guarded>(&rt);
    let log = |rt: &Runtime| rt.app.read(&root).log.clone();
    assert_eq!(log(&rt), Vec::<String>::new());

    // Valid events still arrive; a slider's or a bounded picker's value is
    // brought within its range.
    let go = find_text(&first, "Go");
    for event in [
        UiEvent::Click(go),
        UiEvent::Change(slider, 2.5),
        UiEvent::Change(slider, 3.3),
        UiEvent::Change(slider, 99.),
        UiEvent::Change(picker, 1.),
        UiEvent::Change(toggle, 1.),
        UiEvent::Change(radio, 0.),
        UiEvent::Change(date, -86_400.),
        UiEvent::Change(any_date, f64::NAN),
    ] {
        rt.dispatch(event);
    }
    assert_eq!(
        log(&rt),
        [
            "go",
            "slider 2.5",
            "slider 3.5",
            "slider 10",
            "picker 1",
            "toggle 1",
            "radio 0",
            "date 0",
            "any date NaN"
        ]
    );

    // Layout and scroll reports: kept only from the nodes that report them.
    let frame = Rect::new(0., 0., 10., 10.);
    let tracked = first
        .iter()
        .find_map(|p| match p {
            Patch::SetLayoutEvents { id, on: true } => Some(*id),
            _ => None,
        })
        .expect("the tracked view");
    let untracked = find_text(&first, "untracked");
    rt.dispatch(UiEvent::Layout(unknown, frame));
    rt.dispatch(UiEvent::Layout(untracked, frame));
    rt.dispatch(UiEvent::Layout(
        tracked,
        Rect::new(0., 0., f32::INFINITY, 1.),
    ));
    rt.dispatch(UiEvent::Scroll(unknown, 5., 100.));
    rt.dispatch(UiEvent::Scroll(scroll, 5., 100.));
    assert!(rt.app.frames.is_empty());
    assert!(rt.app.scroll_offsets.is_empty());
    rt.dispatch(UiEvent::Layout(tracked, frame));
    assert_eq!(rt.app.frames.get(&tracked), Some(&frame));
}
