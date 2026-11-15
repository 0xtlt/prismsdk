//! One codebase, every target:
//!   prism run web | macos | ios | tvos | watchos | android, or `cargo run` on Linux
//!
//! The app lives in this library: `main.rs` calls [`run`], and Android loads
//! it as a shared library (`prism::android_main!`).
//!
//! Four tabs, each with its own navigation stack:
//! - Todos:      `/` and `/todos/:id`, a shared `Store` entity, plus a widget.
//! - Focus:      a timer as a Live Activity (lock screen + Dynamic Island),
//!   and a notification when it ends.
//! - Components: the `prism::ui` gallery, custom vs native controls.
//! - Settings:   live theming (system/light/dark, accent, shape, native controls)
//!   and language (English, French, Arabic right-to-left).

mod about;
mod background;
mod car;
mod device;
mod focus;
mod forms;
mod menus;
mod push;
mod report;
mod security;
mod strings;
mod widgets;

use prism::prelude::*;
use prism::ui::{self, Theme};
use prism::{Alert, Menu, MenuItem};

prism::android_main!(run);

/// Starts the app (every platform).
pub fn run() {
    Application::new()
        .title("Prism")
        .size(460., 720.)
        .assets(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"))
        .run(|cx| {
            strings::install(cx);
            background::install(cx);
            push::install(cx);
            let theme = load_theme(cx);
            ui::set_theme(cx, theme);
            let store = cx.new_entity(|cx| Store::load(cx));
            widgets::register(cx, store);
            car::register(cx, store);
            menus::install(cx, store);
            let focus = focus::install(cx, store);
            Router::new()
                // Titles follow the language (recomputed on every render).
                .tab_with(strings::tabs_todos, "checklist", "/")
                .tab_with(strings::tabs_focus, "timer", "/focus")
                .tab_with(strings::tabs_components, "square.grid.2x2", "/components")
                .tab_with(strings::tabs_settings, "gearshape", "/settings")
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
                .route("/components", |cx, _| cx.new_entity(Gallery::new))
                .route("/settings", |cx, _| {
                    cx.new_entity(|cx| Settings {
                        security: cx.new(security::SecurityDemo::new),
                    })
                })
                .route("/focus", move |_, _| focus)
                .route("/components/list", |cx, _| cx.new_entity(LongList::new))
                .route("/push", |cx, _| cx.new_entity(|_| push::PushScreen))
                .route("/components/report", |cx, p| {
                    cx.new_entity(|cx| report::Report::new(p, cx))
                })
                .build(cx)
        });
}

/// A screen with the theme's background and a scrollable body.
fn page(title: impl Into<String>, cx: &App) -> (prism::Screen, prism::Scroll) {
    let t = Theme::of(cx);
    (
        screen(title).bg(t.background),
        // Watches: every point counts.
        scroll()
            .p(20.)
            .gap(20.)
            .on_device(DeviceKind::Watch, |s| s.p(6.).gap(10.)),
    )
}

// --- Shared state ---------------------------------------------------------------------

#[derive(Clone)]
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
            strings::home_info(cx, prism::BACKEND),
            strings::home_open(cx, open as i64)
        );
        if self.synced > 0 {
            info = format!("{info} · {}", strings::home_synced(cx, self.synced));
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
                    text(background::summary(cx))
                        .text_size(14.)
                        .text_color(t.muted),
                )
                .child(
                    text(strings::home_today(cx, &today))
                        .text_size(14.)
                        .text_color(t.muted),
                )
                // Laptops and up: the counter beside the todo list.
                .child(
                    div()
                        .gap(20.)
                        .lg(|s| s.flex_row().items_start())
                        .child(div().lg(|s| s.w(px(340.))).child(self.counter))
                        .child(div().lg(|s| s.flex_1()).child(self.todos)),
                ),
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
                // On a watch the number sits above the buttons.
                .on_device(DeviceKind::Watch, |s| s.flex_col().items_start().gap(6.))
                .child(
                    div()
                        .flex_row()
                        .items_center()
                        .gap(8.)
                        .child(
                            text(self.count.to_string())
                                .text_size(40.)
                                .on_device(DeviceKind::Watch, |s| s.text_size(30.))
                                .bold()
                                .text_color(t.accent)
                                .accessibility_label(strings::counter_label(cx, self.count))
                                .min_w(px(64.)),
                        )
                        // Ten and up: a star pops in and keeps pulsing.
                        .when(self.count >= 10, |row| {
                            row.child(
                                image("star.fill")
                                    .text_size(24.)
                                    .text_color(t.accent)
                                    .enter(Pose::pop())
                                    .exit(Pose::pop())
                                    .repeat(Pose::new().scale(1.25), Transition::ease(700))
                                    .accessibility_hidden(),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_row()
                        .gap(8.)
                        .child(
                            ui::button("−1")
                                .secondary()
                                .hint(strings::counter_decrement_hint(cx))
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
            .child(ui::heading(strings::todos_heading(cx)))
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
                            .placeholder(strings::todos_placeholder(cx))
                            .accessibility_label(strings::screens_new_todo(cx))
                            .on_input(
                                cx.listener(|this, e: &InputEvent, _| this.draft = e.value.clone()),
                            ),
                    )
                    .child(
                        ui::button(strings::todos_new(cx))
                            .secondary()
                            .hint(strings::todos_new_hint(cx))
                            .on_click(std::rc::Rc::new(|_: &ClickEvent, app: &mut App| {
                                app.present("/todos/new")
                            })),
                    )
                    .child(ui::button(strings::todos_add(cx)).on_click(cx.listener(
                        |this, _, cx| {
                            let title = this.draft.trim().to_string();
                            if !title.is_empty() {
                                cx.with_animation(Transition::spring(400), |cx| {
                                    this.store.update(cx, |s, cx| s.add(title, cx))
                                });
                                this.draft.clear();
                            }
                        },
                    ))),
            )
            // Gestures differ: swipe on phones, press and hold on watches
            // and TVs (watchOS, Wear OS, tvOS, Android TV), right-click on
            // computers and the web.
            .child(
                text(match cx.device_info().kind {
                    _ if cx.platform() == Platform::Web => strings::todos_hint_desktop(cx),
                    // Watches, TVs, headsets (pinch and hold): press and hold.
                    DeviceKind::Watch | DeviceKind::Tv | DeviceKind::Headset => {
                        strings::todos_hint_hold(cx)
                    }
                    DeviceKind::Phone | DeviceKind::Tablet => strings::todos_hint_mobile(cx),
                    _ => strings::todos_hint_desktop(cx),
                })
                .text_size(13.)
                .text_color(t.muted),
            )
            .children(store.todos.iter().map(|todo| {
                let status = if todo.done {
                    strings::todo_done(cx)
                } else {
                    strings::todo_open(cx)
                };
                let (id, store_e, done) = (todo.id, self.store, todo.done);
                let row = ui::list_row(todo.title.clone())
                    .leading(if todo.done { "✓" } else { "○" })
                    .subtitle(status)
                    .on_click(navigate_to(format!("/todos/{}", todo.id)))
                    .on_swipe_left(std::rc::Rc::new(move |_: &ClickEvent, app: &mut App| {
                        confirm_delete(app, store_e, id)
                    }))
                    .context_menu(
                        Menu::new()
                            .item(
                                MenuItem::new(strings::menu_open(cx))
                                    .icon("arrow.up.forward.square")
                                    .on_select(navigate_to(format!("/todos/{id}"))),
                            )
                            .item(
                                MenuItem::new(if done {
                                    strings::todo_mark_open(cx)
                                } else {
                                    strings::todo_mark_done(cx)
                                })
                                .icon(if done { "circle" } else { "checkmark.circle" })
                                .on_select(std::rc::Rc::new(
                                    move |_: &ClickEvent, app: &mut App| {
                                        app.with_animation(Transition::ease(250), |app| {
                                            store_e.update(app, |s, cx| s.toggle(id, cx))
                                        })
                                    },
                                )),
                            )
                            .separator()
                            .item(
                                MenuItem::new(strings::menu_delete(cx))
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
                    });
                // Keyed, so a removed todo is the one that leaves. New rows
                // rise in; removed ones slide away.
                div()
                    .key(format!("todo-{id}"))
                    .enter(Pose::slide(0., 12.))
                    .exit(Pose::slide(-40., 0.))
                    .child(row)
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
            let (screen, body) = page(strings::screens_not_found(cx), cx);
            return screen
                .child(body.child(text(strings::todo_missing(cx, id)).text_color(t.muted)));
        };
        let (screen, body) = page(todo.title.clone(), cx);
        screen.child(
            body.child(
                ui::card()
                    .child(
                        text(strings::todo_number(cx, id))
                            .text_size(13.)
                            .text_color(t.muted),
                    )
                    .child(ui::heading(todo.title.clone()).level(1))
                    .child(
                        ui::badge(if todo.done {
                            strings::todo_done(cx)
                        } else {
                            strings::todo_open(cx)
                        })
                        .tone(if todo.done {
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
                            strings::todo_mark_open(cx)
                        } else {
                            strings::todo_mark_done(cx)
                        })
                        .on_click(cx.listener(move |this, _, cx| {
                            this.store.update(cx, |s, cx| s.toggle(id, cx))
                        })),
                    )
                    .child(ui::button(strings::todo_delete(cx)).danger().on_click(
                        cx.listener(move |this, _, cx| confirm_delete(cx, this.store, id)),
                    )),
            ),
        )
    }
}

// --- Tab 2: component gallery ---------------------------------------------------------------

struct Gallery {
    device: Entity<device::DeviceDemo>,
    forms: Entity<forms::Forms>,
    quote: Option<Result<String, String>>,
    loading: bool,
    wifi: bool,
    bluetooth: bool,
    terms: bool,
    email: String,
    taps: u32,
    /// The highlighted bar of the orders chart.
    day: Option<usize>,
}

impl Gallery {
    fn new(cx: &mut Context<Self>) -> Self {
        Gallery {
            device: cx.new(|_| device::DeviceDemo::default()),
            forms: cx.new(|_| forms::Forms::default()),
            quote: None,
            loading: false,
            wifi: false,
            bluetooth: false,
            terms: false,
            email: String::new(),
            taps: 0,
            day: None,
        }
    }

    /// Stat tiles, a line chart, tappable bars, a donut and rings.
    fn charts(&self, cx: &mut Context<Self>) -> ui::Card {
        let t = Theme::of(cx);
        let days: Vec<String> = strings::charts_days(cx)
            .split(',')
            .map(str::to_string)
            .collect();
        let orders = [12., 18., 9., 24., 21., 30., 16.];
        let caption = match self.day {
            Some(i) => strings::charts_selected(cx, &days[i], orders[i] as i64),
            None => strings::charts_tap_bar(cx),
        };
        let euros = |v: f64| format!("{} €", cx.format_number(v, 0));
        ui::card()
            .child(ui::heading(strings::charts_heading(cx)))
            .child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .gap(10.)
                    .child(
                        ui::stat(strings::charts_revenue(cx), euros(12480.))
                            .delta("+12%", true)
                            .trend([8., 9.5, 9., 11., 10.5, 12., 12.5]),
                    )
                    .child(
                        ui::stat(strings::charts_visitors(cx), cx.format_number(3204., 0))
                            .delta("−4%", false)
                            .trend([40., 38., 41., 36., 35., 37., 33.]),
                    ),
            )
            .child(
                ui::line_chart()
                    .title(strings::charts_revenue(cx))
                    .series(
                        strings::charts_this_week(cx),
                        [1.2, 1.9, 1.6, 2.4, 2.1, 3.1, 2.8],
                    )
                    .series(
                        strings::charts_last_week(cx),
                        [1.0, 1.4, 1.5, 1.7, 1.9, 2.2, 2.0],
                    )
                    .labels(days.clone())
                    .smooth()
                    .area()
                    .points()
                    .unit(" k€")
                    .height(160.),
            )
            .child(ui::heading(strings::charts_orders(cx)).level(3))
            .child(text(caption).text_size(13.).text_color(t.muted))
            .child(
                ui::bar_chart()
                    .bars(days.iter().cloned().zip(orders))
                    .highlight(self.day)
                    .height(150.)
                    .on_select(cx.listener(|this, e: &ChangeEvent, _| {
                        this.day = (this.day != Some(e.index())).then_some(e.index());
                    })),
            )
            .child(ui::heading(strings::charts_budget(cx)).level(3))
            .child(
                ui::pie_chart()
                    .slice(strings::charts_rent(cx), 900.)
                    .slice(strings::charts_food(cx), 420.)
                    .slice(strings::charts_leisure(cx), 240.)
                    .slice(strings::charts_transport(cx), 160.)
                    .donut(euros(1720.))
                    .size(140.),
            )
            .child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .justify_center()
                    .gap(24.)
                    .child(
                        ui::ring(0.72)
                            .label("72%")
                            .caption(strings::charts_goal(cx)),
                    )
                    .child(
                        ui::ring(0.45)
                            .label("9/20")
                            .caption(strings::charts_done(cx))
                            .color(t.success),
                    ),
            )
    }
}

impl Render for Gallery {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let email_error = (!self.email.is_empty() && !self.email.contains('@'))
            .then(|| strings::gallery_email_error(cx));
        let (screen, body) = page(strings::screens_components(cx), cx);
        let charts = self.charts(cx);
        screen.child(
            body.child(self.forms)
            .child(report::card())
            .child(charts)
            .child(
                ui::card()
                    .child(ui::heading(strings::gallery_buttons(cx)))
                    .child(
                        div()
                            .flex_row()
                            .flex_wrap()
                            .gap(8.)
                            .child(
                                ui::button(strings::gallery_primary(cx))
                                    .on_click(cx.listener(|this, _, _| this.taps += 1)),
                            )
                            .child(
                                ui::button(strings::gallery_secondary(cx))
                                    .secondary()
                                    .on_click(cx.listener(|this, _, _| this.taps += 1)),
                            )
                            .child(
                                ui::button(strings::gallery_ghost(cx))
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _| this.taps += 1)),
                            )
                            .child(
                                ui::button(strings::gallery_danger(cx))
                                    .danger()
                                    .on_click(cx.listener(|this, _, _| this.taps += 1)),
                            )
                            .child(ui::button(strings::gallery_disabled(cx)).disabled(true)),
                    )
                    .child(
                        div()
                            .flex_row()
                            .gap(8.)
                            .items_center()
                            .child(
                                ui::button(strings::gallery_native(cx))
                                    .native()
                                    .on_click(cx.listener(|this, _, _| this.taps += 1)),
                            )
                            .child(
                                ui::button(strings::gallery_square_large(cx))
                                    .large()
                                    .style(|s| s.corner_radius = 0.)
                                    .on_click(cx.listener(|this, _, _| this.taps += 1)),
                            ),
                    )
                    .child(text(strings::gallery_taps(cx, self.taps as i64)).text_color(t.muted)),
            )
            .child(
                ui::card()
                    .child(ui::heading(strings::gallery_switches(cx)))
                    .child(
                        ui::switch(strings::gallery_wifi(cx), self.wifi).on_change(
                            cx.listener(|this, e: &ChangeEvent, _| this.wifi = e.checked()),
                        ),
                    )
                    .child(ui::divider())
                    .child(
                        ui::switch(strings::gallery_bluetooth(cx), self.bluetooth)
                            .native()
                            .on_change(
                                cx.listener(|this, e: &ChangeEvent, _| {
                                    this.bluetooth = e.checked()
                                }),
                            ),
                    )
                    .child(ui::divider())
                    .child(ui::checkbox(strings::gallery_terms(cx), self.terms).on_change(
                        cx.listener(|this, e: &ChangeEvent, _| this.terms = e.checked()),
                    )),
            )
            .child(
                ui::card().child(ui::heading(strings::gallery_text_field(cx))).child(
                    ui::text_field(strings::gallery_email(cx), self.email.clone())
                        .placeholder(strings::gallery_email_placeholder(cx))
                        .helper(strings::gallery_email_helper(cx))
                        .error(email_error)
                        .on_input(
                            cx.listener(|this, e: &InputEvent, _| this.email = e.value.clone()),
                        ),
                ),
            )
            .child(
                ui::card()
                    .child(ui::heading(strings::gallery_images(cx)))
                    .child(
                        div()
                            .flex_row()
                            .gap(12.)
                            .child(
                                picture("https://images.unsplash.com/photo-1506905925346-21bda4d32df4?w=600&q=70")
                                    .alt(strings::gallery_river_alt(cx))
                                    .w(px(180.))
                                    .h(px(120.))
                                    .rounded(12.),
                            )
                            .child(picture("prism.png").alt(strings::gallery_logo_alt(cx)).fit().size(px(120.)).rounded(24.)),
                    )
                    .child(
                        ui::list_row(strings::gallery_list(cx, cx.format_number(10_000., 0)))
                            .subtitle(strings::gallery_list_subtitle(cx))
                            .leading("☰")
                            .on_click(navigate_to("/components/list")),
                    ),
            )
            .child(
                ui::card()
                    .child(ui::heading(strings::gallery_network(cx)))
                    .child(match (&self.quote, self.loading) {
                        (_, true) => text(strings::gallery_loading(cx)).text_color(t.muted).into_element(),
                        (Some(Ok(q)), _) => text(format!("“{q}”")).into_element(),
                        (Some(Err(e)), _) => text(strings::gallery_failed(cx, e))
                            .text_color(t.danger)
                            .into_element(),
                        (None, _) => text(strings::gallery_fetch_intro(cx))
                            .text_color(t.muted)
                            .into_element(),
                    })
                    .child(
                        ui::button(strings::gallery_fetch(cx))
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
                                })
                                // Completes even if the user leaves the app meanwhile.
                                .finish_in_background();
                            })),
                    ),
            )
            .child(about::card(cx))
            .child(self.device)
            .child(
                ui::card()
                    .child(ui::heading(strings::gallery_badges(cx)))
                    .child(
                        div()
                            .flex_row()
                            .flex_wrap()
                            .gap(6.)
                            .child(ui::badge(strings::gallery_new(cx)).tone(ui::Variant::Primary))
                            .child(ui::badge(strings::gallery_beta(cx)))
                            .child(ui::badge(strings::gallery_deprecated(cx)).tone(ui::Variant::Danger)),
                    )
                    .child(
                        ui::list_row(strings::gallery_storage(cx))
                            .subtitle(strings::gallery_storage_used(cx, format!("{} GB", cx.format_number(12.4, 1))))
                            .detail(format!("{} GB", cx.format_number(64., 0))),
                    )
                    .child(ui::divider())
                    .child(
                        ui::list_row(strings::gallery_open_settings(cx))
                            .leading("⚙")
                            .on_click(navigate_to("/settings")),
                    ),
            ),
        )
        // Over the scroll view, at the bottom of the screen.
        .child(ui::toaster())
    }
}

// --- Tab 3: settings (live theming) ---------------------------------------------------------

struct Settings {
    security: Entity<security::SecurityDemo>,
}

/// A translated name.
type Label = fn(&App) -> String;

const ACCENTS: [(Label, u32); 4] = [
    (strings::settings_accents_indigo, 0x5B5BD6),
    (strings::settings_accents_blue, 0x0A84FF),
    (strings::settings_accents_green, 0x2E9E5B),
    (strings::settings_accents_orange, 0xE8730C),
];

impl Render for Settings {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let (screen, body) = page(strings::screens_settings(cx), cx);
        // Globals re-render the whole app when they change.
        let update = |f: fn(&mut Theme, f64)| {
            std::rc::Rc::new(move |e: &ChangeEvent, app: &mut App| {
                update_and_save_theme(app, |th| f(th, e.value))
            })
        };
        // Phones: one column. Tablets and up: a two-column grid.
        let grid = div()
            .gap(20.)
            .md(|s| s.flex_row().flex_wrap().items_start());
        screen.child(
            body.child(
                grid.child(cell(self.security))
                    .child(cell(
                        ui::card()
                            .child(ui::heading(strings::settings_appearance(cx)))
                            .child(appearance_picker(&t, cx))
                            .child(ui::divider())
                            .child(
                                ui::switch(strings::settings_native(cx), t.prefer_native)
                                    .on_change(update(|th, v| th.prefer_native = v != 0.)),
                            )
                            .child(ui::divider())
                            .child(
                                ui::switch(strings::settings_square(cx), t.radius == 0.).on_change(
                                    update(|th, v| th.radius = if v != 0. { 0. } else { 10. }),
                                ),
                            ),
                    ))
                    .child(cell(
                        ui::card()
                            .child(ui::heading(strings::settings_accent(cx)))
                            .child(div().flex_row().gap(12.).children(ACCENTS.iter().map(
                                |&(name, hex)| {
                                    let selected = t.accent == rgb(hex);
                                    div()
                                        .size(px(40.))
                                        .rounded(20.)
                                        .bg(rgb(hex))
                                        .when(selected, |d| d.border(3.).border_color(t.text))
                                        .accessibility_role(Role::Button)
                                        .accessibility_label(name(cx))
                                        .accessibility_checked(selected)
                                        .on_click(std::rc::Rc::new(
                                            move |_: &ClickEvent, app: &mut App| {
                                                update_and_save_theme(app, |th| {
                                                    th.accent = rgb(hex)
                                                })
                                            },
                                        ))
                                },
                            ))),
                    ))
                    .child(cell(language_card(cx)))
                    .child(cell(push::card(cx))),
            )
            .child(
                text(strings::settings_a11y_note(cx))
                    .text_size(13.)
                    .text_color(t.muted),
            ),
        )
    }
}

/// A grid cell: full width on phones, half a row from tablets up.
fn cell(content: impl IntoElement) -> prism::Div {
    div()
        .md(|s| {
            s.with_style(|s| {
                s.flex_basis = relative(0.4);
                s.flex_grow = 1.;
            })
        })
        .child(content)
}

/// System / English / Français / العربية, plus formatting samples.
fn language_card(cx: &App) -> ui::Card {
    let t = Theme::of(cx);
    let current = cx.locale_choice().map(|l| l.language().to_string());
    let now = strings::now();
    let sample = strings::settings_sample(
        cx,
        cx.format_number(1234567.891, 2),
        cx.format_date(now, DateStyle::Medium),
        cx.format_time(now),
    );
    // The reference language first, each in its own language.
    let mut langs = strings::LANGUAGES.to_vec();
    langs.sort_by_key(|l| (*l != strings::FALLBACK, *l));
    let choices = std::iter::once((None, strings::settings_system(cx))).chain(
        langs
            .into_iter()
            .map(|l| (Some(l), strings::native_name(cx, l))),
    );
    ui::card()
        .child(ui::heading(strings::settings_language(cx)))
        .child(
            div()
                .flex_row()
                .flex_wrap()
                .gap(8.)
                .accessibility_role(Role::Group)
                .accessibility_label(strings::settings_language(cx))
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
fn appearance_picker(t: &Theme, cx: &App) -> prism::Div {
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
        .accessibility_label(strings::settings_appearance(cx))
        .children(
            [
                strings::settings_system(cx),
                strings::settings_light(cx),
                strings::settings_dark(cx),
            ]
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
                            let keep = (th.accent, th.radius, th.prefer_native, th.follows_system);
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
        Alert::new(strings::todo_delete_title(cx, &title))
            .message(strings::todo_delete_message(cx))
            .destructive(
                strings::todo_delete(cx),
                std::rc::Rc::new(move |_: &ClickEvent, app: &mut App| {
                    // The row slides out, the ones below move up.
                    app.with_animation(Transition::spring(400), |app| {
                        store.update(app, |s, cx| s.delete(id, cx))
                    });
                    if app.current_path() == Some(&format!("/todos/{id}")) {
                        app.back();
                    }
                }),
            )
            .cancel(strings::todo_cancel(cx)),
    );
}

// --- Sheet: new todo -----------------------------------------------------------------

struct NewTodo {
    store: Entity<Store>,
    title: String,
}

impl Render for NewTodo {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (screen, body) = page(strings::screens_new_todo(cx), cx);
        let empty = self.title.trim().is_empty();
        screen.child(
            body.child(
                ui::card()
                    .child(
                        ui::text_field(strings::todos_title_label(cx), self.title.clone())
                            .placeholder(strings::todos_placeholder(cx))
                            .on_input(
                                cx.listener(|this, e: &InputEvent, _| this.title = e.value.clone()),
                            ),
                    )
                    .child(
                        ui::button(strings::todos_add_todo(cx))
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
                    text(cx.format_number((i + 1) as f64, 0))
                        .text_color(t.accent)
                        .bold()
                        .min_w(px(56.)),
                )
                .child(
                    text(strings::list_row(
                        cx,
                        cx.format_number((i + 1) as f64, 0),
                        cx.format_number(10_000., 0),
                    ))
                    .text_color(t.text),
                )
        });
        LongList { list }
    }
}

impl Render for LongList {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        screen(strings::screens_rows(cx, cx.format_number(10_000., 0)))
            .bg(t.background)
            .child(self.list)
    }
}
