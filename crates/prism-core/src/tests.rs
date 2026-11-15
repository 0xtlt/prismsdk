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
                .inset(2., 3.)
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
