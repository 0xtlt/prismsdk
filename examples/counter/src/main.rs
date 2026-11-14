//! One codebase, three targets:
//!   mise run web     → http://localhost:3000 (HTML/CSS/JS, no WASM)
//!   mise run macos   → native AppKit window
//!   mise run ios     → native UIKit app in the simulator

use prism::prelude::*;

const ACCENT: u32 = 0x5B5BD6;

struct Todo {
    id: u64,
    title: String,
    done: bool,
}

struct CounterApp {
    count: i32,
    draft: String,
    todos: Vec<Todo>,
    next_id: u64,
}

impl CounterApp {
    fn new() -> Self {
        CounterApp {
            count: 0,
            draft: String::new(),
            todos: vec![Todo {
                id: 1,
                title: "Try Prism on the web".into(),
                done: true,
            }],
            next_id: 2,
        }
    }
}

impl Render for CounterApp {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let remaining = self.todos.iter().filter(|t| !t.done).count();

        div()
            .bg(rgb(0xF4F4F8))
            .p(24.)
            .gap(20.)
            .child(
                div()
                    .gap(4.)
                    .child(
                        text("Prism")
                            .text_size(28.)
                            .bold()
                            .text_color(rgb(0x1C1C28)),
                    )
                    .child(
                        text(format!("Rust UI · running on {}", prism::BACKEND))
                            .text_size(14.)
                            .text_color(rgb(0x6E6E80)),
                    ),
            )
            .child(
                card()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        text(format!("{}", self.count))
                            .text_size(40.)
                            .bold()
                            .text_color(rgb(ACCENT))
                            .min_w(px(64.)),
                    )
                    .child(
                        div()
                            .flex_row()
                            .gap(8.)
                            .child(button("−1").on_click(cx.listener(|this, _, cx| {
                                this.count -= 1;
                                cx.notify();
                            })))
                            .child(button("+1").on_click(cx.listener(|this, _, cx| {
                                this.count += 1;
                                cx.notify();
                            })))
                            .child(button("Reset").on_click(cx.listener(|this, _, cx| {
                                this.count = 0;
                                cx.notify();
                            }))),
                    ),
            )
            .child(
                card()
                    .gap(12.)
                    .child(
                        text(format!("Todos · {remaining} left"))
                            .text_size(17.)
                            .semibold()
                            .text_color(rgb(0x1C1C28)),
                    )
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
                                    .on_input(cx.listener(|this, e: &InputEvent, cx| {
                                        this.draft = e.value.clone();
                                        cx.notify();
                                    })),
                            )
                            .child(button("Add").on_click(cx.listener(|this, _, cx| {
                                let title = this.draft.trim().to_string();
                                if !title.is_empty() {
                                    this.todos.push(Todo {
                                        id: this.next_id,
                                        title,
                                        done: false,
                                    });
                                    this.next_id += 1;
                                    this.draft.clear();
                                    cx.notify();
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
                            .on_click(cx.listener(move |this, _, cx| {
                                if let Some(t) = this.todos.iter_mut().find(|t| t.id == id) {
                                    t.done = !t.done;
                                    cx.notify();
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
                                rgb(0x1C1C28)
                            }))
                            .child(button("Delete").on_click(cx.listener(move |this, _, cx| {
                                this.todos.retain(|t| t.id != id);
                                cx.notify();
                            })))
                    })),
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

fn main() {
    Application::new()
        .title("Prism Counter")
        .size(440., 640.)
        .run(|cx| cx.new_entity(|_| CounterApp::new()));
}
