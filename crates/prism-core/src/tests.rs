use std::sync::Arc;

use crate::prelude::*;
use crate::{AppInit, NodeKind, Patch, Runtime, UiEvent};

struct Counter {
    count: i32,
    items: Vec<&'static str>,
}

impl Render for Counter {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(text(format!("Count: {}", self.count)))
            .child(button("+").on_click(cx.listener(|this, _, cx| {
                this.count += 1;
                cx.notify();
            })))
            .child(div().children(self.items.iter().map(|i| text(*i).key(*i))))
    }
}

fn runtime(items: Vec<&'static str>) -> Runtime {
    let init: AppInit = Arc::new(move |cx: &mut App| {
        let items = items.clone();
        let root = cx.new_entity(|_| Counter { count: 0, items });
        cx.set_root(root);
    });
    Runtime::new(&init)
}

fn button_id(patches: &[Patch]) -> u64 {
    patches
        .iter()
        .find_map(|p| match p {
            Patch::Create {
                id,
                kind: NodeKind::Button,
            } => Some(*id),
            _ => None,
        })
        .unwrap()
}

#[test]
fn first_render_creates_and_mounts_tree() {
    let mut rt = runtime(vec!["a"]);
    let patches = rt.render();
    let creates = patches
        .iter()
        .filter(|p| matches!(p, Patch::Create { .. }))
        .count();
    assert_eq!(creates, 5);
    assert!(matches!(
        patches
            .iter()
            .rev()
            .find(|p| matches!(p, Patch::SetRoot { .. })),
        Some(_)
    ));
}

#[test]
fn click_updates_only_the_changed_text() {
    let mut rt = runtime(vec![]);
    let first = rt.render();
    let patches = rt.dispatch(UiEvent::Click(button_id(&first)));
    assert_eq!(
        patches,
        vec![Patch::SetText {
            id: 2,
            text: "Count: 1".into()
        }]
    );
}

#[test]
fn event_without_notify_renders_nothing() {
    let mut rt = runtime(vec![]);
    rt.render();
    assert!(rt.dispatch(UiEvent::Click(999)).is_empty());
}

#[test]
fn keyed_children_are_moved_not_recreated() {
    let mut rt = runtime(vec!["a", "b"]);
    rt.render();
    let root = crate::app::Entity::<Counter>::clone(&first_entity());
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
    assert!(
        patches
            .iter()
            .any(|p| matches!(p, Patch::SetChildren { children, .. } if children.len() == 2))
    );
}

fn first_entity() -> Entity<Counter> {
    // The root entity is the first one created by `runtime`.
    let mut app = App::new();
    app.new_entity(|_| Counter {
        count: 0,
        items: vec![],
    })
}
