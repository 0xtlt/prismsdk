//! One codebase, three targets:
//!   mise run web     → http://localhost:3000 (HTML/CSS/JS, no WASM)
//!   mise run macos   → native AppKit window
//!   mise run ios     → native UIKit app in the simulator
//!
//! Two routes, `/` and `/todos/:id`, rendered as a native navigation stack:
//! UINavigationController on iOS, a header bar on macOS, real URLs on the web.
//! A shared `Store` entity holds the todos; screens observe it.

use prism::prelude::*;

const ACCENT: u32 = 0x5B5BD6;
const INK: u32 = 0x1C1C28;
const MUTED: u32 = 0x6E6E80;
const PAGE: u32 = 0xF4F4F8;

fn main() {
    Application::new()
        .title("Prism")
        .size(440., 680.)
        .run(|cx| {
            let store = cx.new_entity(|_| Store::new());
            Router::new()
                .route("/", move |cx, _| cx.new_entity(|cx| Home::new(store, cx)))
                .route("/todos/:id", move |cx, p| {
                    let id = p.parse("id").unwrap_or(0);
                    cx.new_entity(|cx| TodoDetail::new(store, id, cx))
                })
                .build(cx)
        });
}

// --- Shared state -----------------------------------------------------------------

struct Todo {
    id: u64,
    title: String,
    done: bool,
}

/// App data, shared by every screen. Not a view: it never renders.
struct Store {
    todos: Vec<Todo>,
    next_id: u64,
}

impl Store {
    fn new() -> Self {
        Store {
            todos: vec![Todo {
                id: 1,
                title: "Try Prism on the web".into(),
                done: true,
            }],
            next_id: 2,
        }
    }

    fn get(&self, id: u64) -> Option<&Todo> {
        self.todos.iter().find(|t| t.id == id)
    }

    fn open(&self) -> usize {
        self.todos.iter().filter(|t| !t.done).count()
    }

    fn add(&mut self, title: String, cx: &mut Context<Self>) {
        self.todos.push(Todo {
            id: self.next_id,
            title,
            done: false,
        });
        self.next_id += 1;
        cx.notify();
    }

    fn toggle(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(t) = self.todos.iter_mut().find(|t| t.id == id) {
            t.done = !t.done;
            cx.notify();
        }
    }

    fn delete(&mut self, id: u64, cx: &mut Context<Self>) {
        self.todos.retain(|t| t.id != id);
        cx.notify();
    }
}

// --- Screen: home ----------------------------------------------------------------

struct Home {
    header: Entity<Header>,
    counter: Entity<CounterCard>,
    todos: Entity<TodoList>,
}

impl Home {
    fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        Home {
            header: cx.new(|cx| {
                cx.observe(&store, |this: &mut Header, store, cx| {
                    this.open_todos = store.read(cx).open();
                });
                Header {
                    open_todos: store.read(cx).open(),
                }
            }),
            counter: cx.new(|_| CounterCard::default()),
            todos: cx.new(|cx| {
                // Re-render the list whenever the store changes.
                cx.observe(&store, |_: &mut TodoList, _, _| {});
                TodoList {
                    store,
                    draft: String::new(),
                }
            }),
        }
    }
}

impl Render for Home {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen("Prism")
            .bg(rgb(PAGE))
            .p(20.)
            .gap(20.)
            .child(self.header)
            .child(self.counter)
            .child(self.todos)
    }
}

struct Header {
    open_todos: usize,
}

impl Render for Header {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        text(format!(
            "Rust UI · running on {} · {} open todo(s)",
            prism::BACKEND,
            self.open_todos
        ))
        .text_size(14.)
        .text_color(rgb(MUTED))
    }
}

#[derive(Default)]
struct CounterCard {
    count: i32,
}

impl Render for CounterCard {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        card()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                text(self.count.to_string())
                    .text_size(40.)
                    .bold()
                    .text_color(rgb(ACCENT))
                    .min_w(px(64.)),
            )
            .child(
                div()
                    .flex_row()
                    .gap(8.)
                    // Listeners re-render their entity automatically: no notify().
                    .child(button("−1").on_click(cx.listener(|this, _, _| this.count -= 1)))
                    .child(button("+1").on_click(cx.listener(|this, _, _| this.count += 1)))
                    .child(button("Reset").on_click(cx.listener(|this, _, _| this.count = 0))),
            )
    }
}

struct TodoList {
    store: Entity<Store>,
    draft: String,
}

impl Render for TodoList {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        card()
            .gap(12.)
            .child(text("Todos").text_size(17.).semibold().text_color(rgb(INK)))
            .child(
                div()
                    .flex_row()
                    .gap(8.)
                    .items_center()
                    .child(
                        text_input()
                            .flex_1()
                            .value(self.draft.clone())
                            .placeholder("What needs doing?")
                            .on_input(
                                cx.listener(|this, e: &InputEvent, _| this.draft = e.value.clone()),
                            ),
                    )
                    .child(button("Add").on_click(cx.listener(|this, _, cx| {
                        let title = this.draft.trim().to_string();
                        if !title.is_empty() {
                            this.store.update(cx, |s, cx| s.add(title, cx));
                            this.draft.clear();
                        }
                    }))),
            )
            .children(store.todos.iter().map(|todo| {
                let id = todo.id;
                div()
                    .key(id.to_string())
                    .flex_row()
                    .items_center()
                    .gap(10.)
                    .py(8.)
                    .px(10.)
                    .rounded(8.)
                    .bg(if todo.done {
                        rgb(0xEDEDF7)
                    } else {
                        rgb(0xFFFFFF)
                    })
                    .border(1.)
                    .border_color(rgb(0xE2E2EA))
                    // Tapping a row opens its detail screen.
                    .on_click(navigate_to(format!("/todos/{id}")))
                    .child(
                        text(if todo.done { "✓" } else { "○" })
                            .text_color(rgb(ACCENT))
                            .bold(),
                    )
                    .child(text(todo.title.clone()).flex_1().text_color(if todo.done {
                        rgb(0x8E8EA0)
                    } else {
                        rgb(INK)
                    }))
                    .child(text("›").text_color(rgb(MUTED)))
            }))
    }
}

// --- Screen: todo detail -------------------------------------------------------------

struct TodoDetail {
    store: Entity<Store>,
    id: u64,
}

impl TodoDetail {
    fn new(store: Entity<Store>, id: u64, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_: &mut TodoDetail, _, _| {});
        TodoDetail { store, id }
    }
}

impl Render for TodoDetail {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let id = self.id;
        let Some(todo) = self.store.read(cx).get(id) else {
            return screen("Not found")
                .bg(rgb(PAGE))
                .p(20.)
                .child(text(format!("Todo #{id} does not exist.")).text_color(rgb(MUTED)));
        };
        screen(todo.title.clone())
            .bg(rgb(PAGE))
            .p(20.)
            .gap(16.)
            .child(
                card()
                    .gap(6.)
                    .child(
                        text(format!("Todo #{id}"))
                            .text_size(13.)
                            .text_color(rgb(MUTED)),
                    )
                    .child(
                        text(todo.title.clone())
                            .text_size(22.)
                            .bold()
                            .text_color(rgb(INK)),
                    )
                    .child(
                        text(if todo.done { "✓ Done" } else { "○ Open" })
                            .text_color(rgb(ACCENT))
                            .semibold(),
                    ),
            )
            .child(
                div()
                    .flex_row()
                    .gap(8.)
                    .child(
                        button(if todo.done {
                            "Mark as open"
                        } else {
                            "Mark as done"
                        })
                        .on_click(cx.listener(move |this, _, cx| {
                            this.store.update(cx, |s, cx| s.toggle(id, cx))
                        })),
                    )
                    .child(button("Delete").on_click(cx.listener(move |this, _, cx| {
                        this.store.update(cx, |s, cx| s.delete(id, cx));
                        cx.back();
                    }))),
            )
    }
}

fn card() -> prism::Div {
    div()
        .bg(rgb(0xFFFFFF))
        .rounded(14.)
        .p(16.)
        .border(1.)
        .border_color(rgb(0xE2E2EA))
}
