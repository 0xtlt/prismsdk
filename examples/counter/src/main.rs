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
//! - Settings:   live theming (system/light/dark, accent, shape, native controls)
//!   and language (English, French, Arabic right-to-left).

mod focus;
mod menus;
mod strings;
mod widgets;

use prism::prelude::*;
use prism::ui::{self, Theme};
use prism::{Alert, Menu, MenuItem};

fn main() {
    Application::new()
        .title("Prism")
        .size(460., 720.)
        .assets(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"))
        .run(|cx| {
            strings::install(cx);
            let theme = load_theme(cx);
            ui::set_theme(cx, theme);
            let store = cx.new_entity(|cx| Store::load(cx));
            widgets::register(cx, store);
            menus::install(cx, store);
            Router::new()
                .tab("Todos", "checklist", "/")
                .tab("Focus", "timer", "/focus")
                .tab("Components", "square.grid.2x2", "/components")
                .tab("Settings", "gearshape", "/settings")
                .route("/", move |cx, _| cx.new_entity(|cx| Home::new(store, cx)))
                .route("/todos/new", move |cx, _| {
                    cx.new_entity(|_| NewTodo {
                        store,
                        title: String::new(),
                    })
                })
                .route("/todos/:id", move |cx, p| {
                    let id = p.parse("id").unwrap_or(0);
                    cx.new_entity(|cx| TodoDetail::new(store, id, cx))
                })
                .route("/components", |cx, _| cx.new_entity(|_| Gallery::default()))
                .route("/settings", |cx, _| cx.new_entity(|_| Settings))
                .route("/focus", |cx, _| cx.new_entity(|_| focus::Focus::default()))
                .route("/components/list", |cx, _| cx.new_entity(LongList::new))
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
    /// Loads saved todos (one per line: `id<TAB>done<TAB>title`).
    fn load(cx: &mut App) -> Self {
        let todos: Vec<Todo> = match cx.storage().get("todos") {
            Some(saved) => saved
                .lines()
                .filter_map(|line| {
                    let mut parts = line.splitn(3, '\t');
                    Some(Todo {
                        id: parts.next()?.parse().ok()?,
                        done: parts.next()? == "1",
                        title: parts.next()?.to_string(),
                    })
                })
                .collect(),
            None => vec![Todo {
                id: 1,
                title: "Try Prism on the web".into(),
                done: true,
            }],
        };
        let next_id = todos.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        Store { todos, next_id }
    }

    /// Persists and re-renders everything that observes the store.
    fn save(&self, cx: &mut Context<Self>) {
        let data: Vec<String> = self
            .todos
            .iter()
            .map(|t| {
                let title = t.title.replace(['\t', '\n'], " ");
                format!("{}\t{}\t{title}", t.id, if t.done { 1 } else { 0 })
            })
            .collect();
        cx.storage().set("todos", &data.join("\n"));
        cx.notify();
    }

    fn get(&self, id: u64) -> Option<&Todo> {
        self.todos.iter().find(|t| t.id == id)
    }

    pub fn open(&self) -> usize {
        self.todos.iter().filter(|t| !t.done).count()
    }

    pub fn add(&mut self, title: String, cx: &mut Context<Self>) {
        self.todos.push(Todo {
            id: self.next_id,
            title,
            done: false,
        });
        self.next_id += 1;
        self.save(cx);
    }

    pub fn toggle(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(t) = self.todos.iter_mut().find(|t| t.id == id) {
            t.done = !t.done;
            self.save(cx);
        }
    }

    pub fn delete(&mut self, id: u64, cx: &mut Context<Self>) {
        self.todos.retain(|t| t.id != id);
        self.save(cx);
    }
}

// --- Tab 1: todos -----------------------------------------------------------------------

struct Home {
    refreshing: bool,
    synced: u32,
    counter: Entity<CounterCard>,
    todos: Entity<TodoList>,
    store: Entity<Store>,
}

impl Home {
    fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_: &mut Home, _, _| {});
        Home {
            refreshing: false,
            synced: 0,
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
        let mut info = format!(
            "{} · {}",
            cx.t_with("home.info", &[("backend", &prism::BACKEND)]),
            cx.t_count("home.open", open as i64)
        );
        if self.synced > 0 {
            info = format!("{info} · {}", cx.t_count("home.synced", self.synced as i64));
        }
        let today = cx.format_date(strings::now(), DateStyle::Long);
        screen.child(
            body
                // Pull down to "sync" (a fake one-second task).
                .refreshing(self.refreshing)
                .on_refresh(cx.listener(|this, _, cx| {
                    this.refreshing = true;
                    cx.spawn(async move |this, cx| {
                        cx.sleep(std::time::Duration::from_secs(1)).await;
                        cx.update_entity(&this, |h, _| {
                            h.refreshing = false;
                            h.synced += 1;
                        });
                    });
                }))
                .child(text(info).text_size(14.).text_color(t.muted))
                .child(
                    text(cx.t_with("home.today", &[("date", &today)]))
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
        // 0…10 fills the bar; it slides to its new width (skipped with Reduce Motion).
        let fill = (self.count.clamp(0, 10) as f32) / 10.;
        let bar = div()
            .h(px(6.))
            .rounded(3.)
            .bg(t.border)
            .flex_row()
            .child(
                div()
                    .h(px(6.))
                    .w(relative(fill))
                    .rounded(3.)
                    .bg(t.accent)
                    .transition(Transition::spring(350)),
            )
            .accessibility_hidden();
        ui::card().child(bar).child(
            div()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    text(self.count.to_string())
                        .text_size(40.)
                        .bold()
                        .text_color(t.accent)
                        .accessibility_label(cx.t_with("counter.label", &[("count", &self.count)]))
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
            .child(ui::heading(cx.t("todos.heading")))
            .child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .gap(8.)
                    .items_center()
                    .child(
                        text_input()
                            .flex_1()
                            .value(self.draft.clone())
                            .placeholder(cx.t("todos.placeholder"))
                            .accessibility_label("New todo")
                            .on_input(
                                cx.listener(|this, e: &InputEvent, _| this.draft = e.value.clone()),
                            ),
                    )
                    .child(
                        ui::button(cx.t("todos.new"))
                            .secondary()
                            .hint("Opens a form in a sheet")
                            .on_click(std::rc::Rc::new(|_: &ClickEvent, app: &mut App| {
                                app.present("/todos/new")
                            })),
                    )
                    .child(
                        ui::button(cx.t("todos.add")).on_click(cx.listener(|this, _, cx| {
                            let title = this.draft.trim().to_string();
                            if !title.is_empty() {
                                this.store.update(cx, |s, cx| s.add(title, cx));
                                this.draft.clear();
                            }
                        })),
                    ),
            )
            .children(store.todos.iter().map(|todo| {
                let status = if todo.done {
                    cx.t("todo.done")
                } else {
                    cx.t("todo.open")
                };
                let (id, store_e, done) = (todo.id, self.store, todo.done);
                ui::list_row(todo.title.clone())
                    .leading(if todo.done { "✓" } else { "○" })
                    .subtitle(status)
                    .on_click(navigate_to(format!("/todos/{}", todo.id)))
                    .on_swipe_left(std::rc::Rc::new(move |_: &ClickEvent, app: &mut App| {
                        confirm_delete(app, store_e, id)
                    }))
                    .context_menu(
                        Menu::new()
                            .item(
                                MenuItem::new("Open")
                                    .icon("arrow.up.forward.square")
                                    .on_select(navigate_to(format!("/todos/{id}"))),
                            )
                            .item(
                                MenuItem::new(if done { "Mark as open" } else { "Mark as done" })
                                    .icon(if done { "circle" } else { "checkmark.circle" })
                                    .on_select(std::rc::Rc::new(
                                        move |_: &ClickEvent, app: &mut App| {
                                            store_e.update(app, |s, cx| s.toggle(id, cx))
                                        },
                                    )),
                            )
                            .separator()
                            .item(
                                MenuItem::new("Delete…")
                                    .icon("trash")
                                    .destructive()
                                    .on_select(std::rc::Rc::new(
                                        move |_: &ClickEvent, app: &mut App| {
                                            confirm_delete(app, store_e, id)
                                        },
                                    )),
                            ),
                    )
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
                    .child(ui::button("Delete").danger().on_click(
                        cx.listener(move |this, _, cx| confirm_delete(cx, this.store, id)),
                    )),
            ),
        )
    }
}

// --- Tab 2: component gallery ---------------------------------------------------------------

#[derive(Default)]
struct Gallery {
    quote: Option<Result<String, String>>,
    loading: bool,
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
                    .child(ui::heading("Images"))
                    .child(
                        div()
                            .flex_row()
                            .gap(12.)
                            .child(
                                picture("https://images.unsplash.com/photo-1506905925346-21bda4d32df4?w=600&q=70")
                                    .alt("A river between mountains")
                                    .w(px(180.))
                                    .h(px(120.))
                                    .rounded(12.),
                            )
                            .child(picture("prism.png").alt("Prism logo").fit().size(px(120.)).rounded(24.)),
                    )
                    .child(
                        ui::list_row("A list of 10,000 rows")
                            .subtitle("Virtualized: only visible rows exist")
                            .leading("☰")
                            .on_click(navigate_to("/components/list")),
                    ),
            )
            .child(
                ui::card()
                    .child(ui::heading("Network"))
                    .child(match (&self.quote, self.loading) {
                        (_, true) => text("Loading…").text_color(t.muted).into_element(),
                        (Some(Ok(q)), _) => text(format!("“{q}”")).into_element(),
                        (Some(Err(e)), _) => text(format!("Failed: {e}"))
                            .text_color(t.danger)
                            .into_element(),
                        (None, _) => text("Fetch a line of GitHub wisdom over HTTPS.")
                            .text_color(t.muted)
                            .into_element(),
                    })
                    .child(
                        ui::button("Fetch")
                            .secondary()
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, cx| {
                                this.loading = true;
                                cx.spawn(async move |this, cx| {
                                    use prism::http::{Fetch, Request};
                                    let res = cx
                                        .fetch(
                                            Request::get("https://api.github.com/zen")
                                                .header("user-agent", "prism-example"),
                                        )
                                        .await;
                                    cx.update_entity(&this, |g, _| {
                                        g.loading = false;
                                        g.quote = Some(match res {
                                            Ok(r) if r.ok() => Ok(r.text()),
                                            Ok(r) => Err(format!("HTTP {}", r.status)),
                                            Err(e) => Err(e.to_string()),
                                        });
                                    });
                                });
                            })),
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
                update_and_save_theme(app, |th| f(th, e.value))
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
                                    update_and_save_theme(app, |th| th.accent = rgb(hex))
                                }))
                        },
                    ))),
            )
            .child(language_card(cx))
            .child(
                text("Every control here is exposed to VoiceOver and to screen readers on the web, and works with the keyboard.")
                    .text_size(13.)
                    .text_color(t.muted),
            ),
        )
    }
}

/// System / English / Français / العربية, plus formatting samples.
fn language_card(cx: &App) -> ui::Card {
    let t = Theme::of(cx);
    let current = cx.locale_choice().map(|l| l.language().to_string());
    let now = strings::now();
    let sample = cx.t_with(
        "settings.sample",
        &[
            ("number", &cx.format_number(1234567.891, 2)),
            ("date", &cx.format_date(now, DateStyle::Medium)),
            ("time", &cx.format_time(now)),
        ],
    );
    let choices = std::iter::once((None, cx.t("settings.system"))).chain(
        strings::LANGUAGES
            .iter()
            .map(|(tag, name)| (Some(*tag), name.to_string())),
    );
    ui::card()
        .child(ui::heading(cx.t("settings.language")))
        .child(
            div()
                .flex_row()
                .flex_wrap()
                .gap(8.)
                .accessibility_role(Role::Group)
                .accessibility_label(cx.t("settings.language"))
                .children(choices.map(move |(tag, name)| {
                    ui::button(name)
                        .variant(if tag == current.as_deref() {
                            ui::Variant::Primary
                        } else {
                            ui::Variant::Secondary
                        })
                        .small()
                        .on_click(std::rc::Rc::new(move |_: &ClickEvent, app: &mut App| {
                            strings::choose(app, tag)
                        }))
                })),
        )
        .child(text(sample).text_size(13.).text_color(t.muted))
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
                            update_and_save_theme(app, |th| {
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

/// Theme saved as `mode;accent;radius;native` (mode: system | light | dark).
fn load_theme(cx: &mut App) -> Theme {
    let saved = cx.storage().get("theme").unwrap_or_default();
    let mut parts = saved.split(';');
    let mut theme = match parts.next() {
        Some("light") => Theme::light(),
        Some("dark") => Theme::dark(),
        _ => Theme::system(),
    };
    if let Some(hex) = parts.next().and_then(|h| u32::from_str_radix(h, 16).ok()) {
        theme.accent = rgb(hex);
    }
    if let Some(r) = parts.next().and_then(|r| r.parse().ok()) {
        theme.radius = r;
    }
    theme.prefer_native = parts.next() == Some("1");
    theme
}

/// Edits the theme and remembers it.
fn update_and_save_theme(cx: &mut App, f: impl FnOnce(&mut Theme)) {
    ui::update_theme(cx, f);
    let t = Theme::of(cx);
    let mode = if t.follows_system {
        "system"
    } else if t.dark {
        "dark"
    } else {
        "light"
    };
    // Only fixed (non-adaptive) accents are custom choices worth saving.
    let accent = if t.accent.dark.is_none() {
        format!(
            "{:06x}",
            ((t.accent.r * 255.) as u32) << 16
                | ((t.accent.g * 255.) as u32) << 8
                | (t.accent.b * 255.) as u32
        )
    } else {
        String::new()
    };
    let value = format!(
        "{mode};{accent};{};{}",
        t.radius,
        if t.prefer_native { 1 } else { 0 }
    );
    cx.storage().set("theme", &value);
}

/// Asks before deleting (a native alert), then goes back if needed.
fn confirm_delete(cx: &mut App, store: Entity<Store>, id: u64) {
    let title = store
        .read(cx)
        .get(id)
        .map(|t| t.title.clone())
        .unwrap_or_default();
    cx.show_alert(
        Alert::new(format!("Delete “{title}”?"))
            .message("This cannot be undone.")
            .destructive(
                "Delete",
                std::rc::Rc::new(move |_: &ClickEvent, app: &mut App| {
                    store.update(app, |s, cx| s.delete(id, cx));
                    if app.current_path() == Some(&format!("/todos/{id}")) {
                        app.back();
                    }
                }),
            )
            .cancel("Cancel"),
    );
}

// --- Sheet: new todo -----------------------------------------------------------------

struct NewTodo {
    store: Entity<Store>,
    title: String,
}

impl Render for NewTodo {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (screen, body) = page("New todo", cx);
        let empty = self.title.trim().is_empty();
        screen.child(
            body.child(
                ui::card()
                    .child(
                        ui::text_field("Title", self.title.clone())
                            .placeholder("What needs doing?")
                            .on_input(
                                cx.listener(|this, e: &InputEvent, _| this.title = e.value.clone()),
                            ),
                    )
                    .child(
                        ui::button("Add todo")
                            .full_width()
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, cx| {
                                let title = this.title.trim().to_string();
                                this.store.update(cx, |s, cx| s.add(title, cx));
                                cx.dismiss();
                            })),
                    ),
            ),
        )
    }
}

// --- Screen: 10,000-row virtualized list ---------------------------------------------

struct LongList {
    list: Entity<VirtualList>,
}

impl LongList {
    fn new(cx: &mut Context<Self>) -> Self {
        let list = VirtualList::new(cx, 10_000, 56., |i, cx| {
            let t = Theme::of(cx);
            div()
                .flex_row()
                .items_center()
                .gap(12.)
                .px(20.)
                .h_full()
                .child(
                    text(format!("{}", i + 1))
                        .text_color(t.accent)
                        .bold()
                        .min_w(px(56.)),
                )
                .child(text(format!("Row number {} of 10,000", i + 1)).text_color(t.text))
        });
        LongList { list }
    }
}

impl Render for LongList {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        screen("10,000 rows").bg(t.background).child(self.list)
    }
}
