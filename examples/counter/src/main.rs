//! One codebase, three targets:
//!   mise run web     → http://localhost:3000 (HTML/CSS/JS, no WASM)
//!   mise run macos   → native AppKit window
//!   mise run ios     → native UIKit app in the simulator
//!
//! Each card is its own entity: clicking "+1" re-renders only `CounterCard`;
//! typing re-renders only `TodoList`. `Header` observes the todo list to show
//! a live total, without the two knowing about each other's internals.

use prism::prelude::*;

const ACCENT: u32 = 0x5B5BD6;
const INK: u32 = 0x1C1C28;
const MUTED: u32 = 0x6E6E80;

// --- Root -------------------------------------------------------------------

struct App {
    header: Entity<Header>,
    counter: Entity<CounterCard>,
    todos: Entity<TodoList>,
}

impl Render for App {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .bg(rgb(0xF4F4F8))
            .p(24.)
            .gap(20.)
            .child(self.header)
            .child(self.counter)
            .child(self.todos)
    }
}

// --- Header: derived state via `observe` -------------------------------------

struct Header {
    open_todos: usize,
}

impl Render for Header {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .gap(4.)
            .child(text("Prism").text_size(28.).bold().text_color(rgb(INK)))
            .child(
                text(format!(
                    "Rust UI · running on {} · {} open todo(s)",
                    prism::BACKEND,
                    self.open_todos
                ))
                .text_size(14.)
                .text_color(rgb(MUTED)),
            )
    }
}

// --- Counter ------------------------------------------------------------------

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

// --- Todos ----------------------------------------------------------------------

struct Todo {
    id: u64,
    title: String,
    done: bool,
}

struct TodoList {
    draft: String,
    todos: Vec<Todo>,
    next_id: u64,
}

impl TodoList {
    fn open(&self) -> usize {
        self.todos.iter().filter(|t| !t.done).count()
    }
}

impl Render for TodoList {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(button("Add").on_click(cx.listener(|this, _, _| {
                        let title = this.draft.trim().to_string();
                        if !title.is_empty() {
                            this.todos.push(Todo {
                                id: this.next_id,
                                title,
                                done: false,
                            });
                            this.next_id += 1;
                            this.draft.clear();
                        }
                    }))),
            )
            .children(self.todos.iter().map(|todo| {
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
                    .on_click(cx.listener(move |this, _, _| {
                        if let Some(t) = this.todos.iter_mut().find(|t| t.id == id) {
                            t.done = !t.done;
                        }
                    }))
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
                    .child(
                        button("Delete").on_click(
                            cx.listener(move |this, _, _| this.todos.retain(|t| t.id != id)),
                        ),
                    )
            }))
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

fn main() {
    Application::new()
        .title("Prism Counter")
        .size(440., 640.)
        .run(|cx| {
            cx.new_entity(|cx| {
                let todos = cx.new(|_| TodoList {
                    draft: String::new(),
                    todos: vec![Todo {
                        id: 1,
                        title: "Try Prism on the web".into(),
                        done: true,
                    }],
                    next_id: 2,
                });
                let header = cx.new(|cx| {
                    cx.observe(&todos, |this: &mut Header, todos, cx| {
                        this.open_todos = todos.read(cx).open();
                    });
                    Header {
                        open_todos: todos.read(cx).open(),
                    }
                });
                App {
                    header,
                    counter: cx.new(|_| CounterCard::default()),
                    todos,
                }
            })
        });
}
