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
