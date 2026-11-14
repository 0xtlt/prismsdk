//! One codebase, three targets:
//!   mise run web     → http://localhost:3000 (HTML/CSS/JS, no WASM)
//!   mise run macos   → native AppKit window
//!   mise run ios     → native UIKit app in the simulator
//!
//! Four tabs, each with its own navigation stack:
//! - Todos:      `/` and `/todos/:id`, a shared `Store` entity, plus a widget.
//! - Focus:      a timer as a Live Activity (lock screen + Dynamic Island),
//!   and a notification when it ends.
//! - Components: the `prism::ui` gallery, custom vs native controls.
//! - Settings:   live theming (system/light/dark, accent, shape, native controls).

mod focus;
mod widgets;

use prism::prelude::*;
use prism::ui::{self, Theme};

fn main() {
    Application::new()
        .title("Prism")
        .size(460., 720.)
        .run(|cx| {
            ui::set_theme(cx, Theme::system());
            let store = cx.new_entity(|_| Store::new());
            widgets::register(cx, store);
            Router::new()
                .tab("Todos", "checklist", "/")
                .tab("Focus", "timer", "/focus")
                .tab("Components", "square.grid.2x2", "/components")
                .tab("Settings", "gearshape", "/settings")
                .route("/", move |cx, _| cx.new_entity(|cx| Home::new(store, cx)))
                .route("/todos/:id", move |cx, p| {
                    let id = p.parse("id").unwrap_or(0);
                    cx.new_entity(|cx| TodoDetail::new(store, id, cx))
                })
                .route("/components", |cx, _| cx.new_entity(|_| Gallery::default()))
                .route("/settings", |cx, _| cx.new_entity(|_| Settings))
                .route("/focus", |cx, _| cx.new_entity(|_| focus::Focus::default()))
                .build(cx)
        });
}

/// A screen with the theme's background and a scrollable body.
fn page(title: impl Into<String>, cx: &App) -> (prism::Screen, prism::Scroll) {
    let t = Theme::of(cx);
    (screen(title).bg(t.background), scroll().p(20.).gap(20.))
}

// --- Shared state ---------------------------------------------------------------------

pub(crate) struct Todo {
    pub id: u64,
    pub title: String,
    pub done: bool,
}

/// App data, shared by every screen. Not a view: it never renders.
pub(crate) struct Store {
    pub todos: Vec<Todo>,
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

    pub fn open(&self) -> usize {
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

// --- Tab 1: todos -----------------------------------------------------------------------

struct Home {
    counter: Entity<CounterCard>,
    todos: Entity<TodoList>,
    store: Entity<Store>,
}

impl Home {
    fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_: &mut Home, _, _| {});
        Home {
            counter: cx.new(|_| CounterCard::default()),
            todos: cx.new(|cx| {
                cx.observe(&store, |_: &mut TodoList, _, _| {});
                TodoList {
                    store,
                    draft: String::new(),
                }
            }),
            store,
        }
    }
}

impl Render for Home {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let open = self.store.read(cx).open();
        let (screen, body) = page("Prism", cx);
        screen.child(
            body.child(
                text(format!(
                    "Rust UI · running on {} · {open} open todo(s)",
                    prism::BACKEND
                ))
                .text_size(14.)
                .text_color(t.muted),
            )
            .child(self.counter)
            .child(self.todos),
        )
    }
}

#[derive(Default)]
struct CounterCard {
    count: i32,
}

impl Render for CounterCard {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        ui::card().child(
            div()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    text(self.count.to_string())
                        .text_size(40.)
                        .bold()
                        .text_color(t.accent)
                        .accessibility_label(format!("Count: {}", self.count))
                        .min_w(px(64.)),
                )
                .child(
                    div()
                        .flex_row()
                        .gap(8.)
                        .child(
                            ui::button("−1")
                                .secondary()
                                .hint("Decrements the counter")
                                .on_click(cx.listener(|this, _, _| this.count -= 1)),
                        )
                        .child(
                            ui::button("+1").on_click(cx.listener(|this, _, _| this.count += 1)),
                        ),
                ),
        )
    }
}

struct TodoList {
    store: Entity<Store>,
    draft: String,
}

impl Render for TodoList {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let store = self.store.read(cx);
        ui::card()
            .child(ui::heading("Todos"))
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
                            .accessibility_label("New todo")
                            .on_input(
                                cx.listener(|this, e: &InputEvent, _| this.draft = e.value.clone()),
                            ),
                    )
                    .child(ui::button("Add").on_click(cx.listener(|this, _, cx| {
                        let title = this.draft.trim().to_string();
                        if !title.is_empty() {
                            this.store.update(cx, |s, cx| s.add(title, cx));
                            this.draft.clear();
                        }
                    }))),
            )
            .children(store.todos.iter().map(|todo| {
                ui::list_row(todo.title.clone())
                    .leading(if todo.done { "✓" } else { "○" })
                    .subtitle(if todo.done { "Done" } else { "Open" })
                    .on_click(navigate_to(format!("/todos/{}", todo.id)))
                    .style({
                        let border = t.border;
                        move |s| {
                            s.border_width = 1.;
                            s.border_color = Some(border);
                            s.corner_radius = 8.;
                            s.padding.left = 12.;
                            s.padding.right = 12.;
                        }
                    })
            }))
    }
}

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
        let t = Theme::of(cx);
        let Some(todo) = self.store.read(cx).get(id) else {
            let (screen, body) = page("Not found", cx);
            return screen.child(
                body.child(text(format!("Todo #{id} does not exist.")).text_color(t.muted)),
            );
        };
        let (screen, body) = page(todo.title.clone(), cx);
        screen.child(
            body.child(
                ui::card()
                    .child(
                        text(format!("Todo #{id}"))
                            .text_size(13.)
                            .text_color(t.muted),
                    )
                    .child(ui::heading(todo.title.clone()).level(1))
                    .child(
                        ui::badge(if todo.done { "Done" } else { "Open" }).tone(if todo.done {
                            ui::Variant::Primary
                        } else {
                            ui::Variant::Secondary
                        }),
                    ),
            )
            .child(
                div()
                    .flex_row()
                    .gap(8.)
                    .child(
                        ui::button(if todo.done {
                            "Mark as open"
                        } else {
                            "Mark as done"
                        })
                        .on_click(cx.listener(move |this, _, cx| {
                            this.store.update(cx, |s, cx| s.toggle(id, cx))
                        })),
                    )
                    .child(ui::button("Delete").danger().on_click(cx.listener(
                        move |this, _, cx| {
                            this.store.update(cx, |s, cx| s.delete(id, cx));
                            cx.back();
                        },
                    ))),
            ),
        )
    }
}

// --- Tab 2: component gallery ---------------------------------------------------------------

#[derive(Default)]
struct Gallery {
    wifi: bool,
    bluetooth: bool,
    terms: bool,
    email: String,
    taps: u32,
}

impl Render for Gallery {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let email_error = (!self.email.is_empty() && !self.email.contains('@'))
            .then_some("Enter a valid email address");
        let (screen, body) = page("Components", cx);
        screen.child(
            body.child(
                ui::card()
                    .child(ui::heading("Buttons"))
                    .child(
                        div()
                            .flex_row()
                            .flex_wrap()
                            .gap(8.)
                            .child(
                                ui::button("Primary")
                                    .on_click(cx.listener(|this, _, _| this.taps += 1)),
                            )
                            .child(
                                ui::button("Secondary")
                                    .secondary()
                                    .on_click(cx.listener(|this, _, _| this.taps += 1)),
                            )
                            .child(
                                ui::button("Ghost")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _| this.taps += 1)),
                            )
                            .child(
                                ui::button("Danger")
                                    .danger()
                                    .on_click(cx.listener(|this, _, _| this.taps += 1)),
                            )
                            .child(ui::button("Disabled").disabled(true)),
                    )
                    .child(
                        div()
                            .flex_row()
                            .gap(8.)
                            .items_center()
                            .child(
                                ui::button("Native")
                                    .native()
                                    .on_click(cx.listener(|this, _, _| this.taps += 1)),
                            )
                            .child(
                                ui::button("Square, large")
                                    .large()
                                    .style(|s| s.corner_radius = 0.)
                                    .on_click(cx.listener(|this, _, _| this.taps += 1)),
                            ),
                    )
                    .child(text(format!("Tapped {} time(s)", self.taps)).text_color(t.muted)),
            )
            .child(
                ui::card()
                    .child(ui::heading("Switches"))
                    .child(
                        ui::switch("Wi-Fi (custom)", self.wifi).on_change(
                            cx.listener(|this, e: &ChangeEvent, _| this.wifi = e.checked()),
                        ),
                    )
                    .child(ui::divider())
                    .child(
                        ui::switch("Bluetooth (native)", self.bluetooth)
                            .native()
                            .on_change(
                                cx.listener(|this, e: &ChangeEvent, _| {
                                    this.bluetooth = e.checked()
                                }),
                            ),
                    )
                    .child(ui::divider())
                    .child(ui::checkbox("I accept the terms", self.terms).on_change(
                        cx.listener(|this, e: &ChangeEvent, _| this.terms = e.checked()),
                    )),
            )
            .child(
                ui::card().child(ui::heading("Text field")).child(
                    ui::text_field("Email", self.email.clone())
                        .placeholder("you@example.com")
                        .helper("We never share it.")
                        .error(email_error)
                        .on_input(
                            cx.listener(|this, e: &InputEvent, _| this.email = e.value.clone()),
                        ),
                ),
            )
            .child(
                ui::card()
                    .child(ui::heading("Badges & rows"))
                    .child(
                        div()
                            .flex_row()
                            .gap(6.)
                            .child(ui::badge("New").tone(ui::Variant::Primary))
                            .child(ui::badge("Beta"))
                            .child(ui::badge("Deprecated").tone(ui::Variant::Danger)),
                    )
                    .child(
                        ui::list_row("Storage")
                            .subtitle("12.4 GB used")
                            .detail("64 GB"),
                    )
                    .child(ui::divider())
                    .child(
                        ui::list_row("Open settings")
                            .leading("⚙")
                            .on_click(navigate_to("/settings")),
                    ),
            ),
        )
    }
}

// --- Tab 3: settings (live theming) ---------------------------------------------------------

struct Settings;

const ACCENTS: [(&str, u32); 4] = [
    ("Indigo", 0x5B5BD6),
    ("Blue", 0x0A84FF),
    ("Green", 0x2E9E5B),
    ("Orange", 0xE8730C),
];

impl Render for Settings {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let (screen, body) = page("Settings", cx);
        // Globals re-render the whole app when they change.
        let update = |f: fn(&mut Theme, f64)| {
            std::rc::Rc::new(move |e: &ChangeEvent, app: &mut App| {
                ui::update_theme(app, |th| f(th, e.value))
            })
        };
        screen.child(
            body.child(
                ui::card()
                    .child(ui::heading("Appearance"))
                    .child(appearance_picker(&t))
                    .child(ui::divider())
                    .child(
                        ui::switch("Use native controls", t.prefer_native)
                            .on_change(update(|th, v| th.prefer_native = v != 0.)),
                    )
                    .child(ui::divider())
                    .child(ui::switch("Square corners", t.radius == 0.).on_change(update(
                        |th, v| th.radius = if v != 0. { 0. } else { 10. },
                    ))),
            )
            .child(
                ui::card()
                    .child(ui::heading("Accent color"))
                    .child(div().flex_row().gap(12.).children(ACCENTS.iter().map(
                        |&(name, hex)| {
                            let selected = t.accent == rgb(hex);
                            div()
                                .size(px(40.))
                                .rounded(20.)
                                .bg(rgb(hex))
                                .when(selected, |d| d.border(3.).border_color(t.text))
                                .accessibility_role(Role::Button)
                                .accessibility_label(name)
                                .accessibility_checked(selected)
                                .on_click(std::rc::Rc::new(move |_: &ClickEvent, app: &mut App| {
                                    ui::update_theme(app, |th| th.accent = rgb(hex))
                                }))
                        },
                    ))),
            )
            .child(
                text("Every control here is exposed to VoiceOver and to screen readers on the web, and works with the keyboard.")
                    .text_size(13.)
                    .text_color(t.muted),
            ),
        )
    }
}

/// System / Light / Dark, as a group of toggle buttons.
fn appearance_picker(t: &Theme) -> prism::Div {
    let current = if t.follows_system {
        0
    } else if t.dark {
        2
    } else {
        1
    };
    div()
        .flex_row()
        .gap(8.)
        .accessibility_role(Role::Group)
        .accessibility_label("Appearance")
        .children(
            ["System", "Light", "Dark"]
                .into_iter()
                .enumerate()
                .map(move |(i, name)| {
                    ui::button(name)
                        .variant(if i == current {
                            ui::Variant::Primary
                        } else {
                            ui::Variant::Secondary
                        })
                        .small()
                        .on_click(std::rc::Rc::new(move |_: &ClickEvent, app: &mut App| {
                            ui::update_theme(app, |th| {
                                let keep =
                                    (th.accent, th.radius, th.prefer_native, th.follows_system);
                                *th = match i {
                                    0 => Theme::system(),
                                    1 => Theme::light(),
                                    _ => Theme::dark(),
                                };
                                th.radius = keep.1;
                                th.prefer_native = keep.2;
                                // Keep a custom accent, unless it was the theme default.
                                if !keep.3
                                    && keep.0 != Theme::light().accent
                                    && keep.0 != Theme::dark().accent
                                {
                                    th.accent = keep.0;
                                }
                            })
                        }))
                }),
        )
}
