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
    assert_eq!((depth, path.as_str()), (2, "/items/3"));
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
    find_text(&first, "No route for /nope");
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
    // Both tabs' root screens exist from the start (native tab bars need them).
    find_text(&first, "Item 99");

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
