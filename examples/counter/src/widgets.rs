//! The "Todos" widget: one description, rendered by WidgetKit on iOS/macOS
//! and by RemoteViews on Android. Re-rendered whenever the store changes.
//! Its ring is a `prism::ui` chart: drawn with SwiftUI paths in WidgetKit,
//! and as a bitmap on Android.
//!
//! The "Todo" widget is configurable: each instance shows the todo picked in
//! the system's « Edit widget » sheet (iOS 17+, macOS 14+), from the todos
//! of the moment. Its check mark is a widget button: a tap marks the todo
//! done (or open again) without opening the app (`widget_action`, handled by
//! `on_widget_action` below).

use prism::prelude::*;

use crate::strings;
use prism::ui::{self, Theme};

use crate::Store;

/// `COUNTER_NO_WIDGETS=1 prism run ios`: the app without widgets or Live
/// Activities, so its bundle has no extension and no App Group.
pub fn no_widgets() -> bool {
    option_env!("COUNTER_NO_WIDGETS") == Some("1")
}

/// Kept so the widgets can be named again in another language.
struct WidgetStore(Entity<Store>);

pub fn register(cx: &mut App, store: Entity<Store>) {
    if no_widgets() {
        return;
    }
    // The focus timer starts Live Activities (focus.rs): the iOS bundle needs
    // the extension for them even without this widget.
    cx.enable_live_activities();
    cx.set_global(WidgetStore(store));
    rename(cx);
    cx.reload_widgets_on(&store);
    // Widget buttons: runs in the app, launched in the background if needed;
    // the widgets are redrawn when the job returns.
    cx.on_widget_action(move |action, cx| async move {
        if action.action == "toggle"
            && let Ok(id) = action.payload.parse::<u64>()
        {
            cx.update_entity(&store, |s, cx| s.toggle(id, cx));
        }
    });
}

/// Registers the widgets with their names in the current language: the
/// gallery shows them (registering a kind again replaces it).
pub fn rename(cx: &mut App) {
    let Some(store) = cx.try_global::<WidgetStore>().map(|w| w.0) else {
        return;
    };
    cx.register_widget(
        Widget::new("todos", strings::widget_todos_name(cx))
            .description(strings::widget_todos_description(cx))
            .families(&[
                WidgetFamily::Small,
                WidgetFamily::Medium,
                WidgetFamily::AccessoryCircular,
                WidgetFamily::AccessoryRectangular,
            ])
            .link("/")
            .render(move |cx, family| {
                let t = Theme::of(cx);
                let s = store.read(cx);
                let open: Vec<(u64, String)> = s
                    .todos
                    .iter()
                    .filter(|t| !t.done)
                    .map(|t| (t.id, t.title.clone()))
                    .collect();
                let total = s.todos.len();
                let count = open.len();
                let done = if total == 0 {
                    0.
                } else {
                    (total - count) as f64 / total as f64
                };
                let percent = format!("{}%", cx.format_number(done * 100., 0));
                match family {
                    // Lock screen: monochrome, tinted by the system.
                    WidgetFamily::AccessoryCircular => {
                        return ui::ring(done)
                            .size(56.)
                            .thickness(6.)
                            .label(count.to_string())
                            .into_element();
                    }
                    WidgetFamily::AccessoryRectangular => {
                        return div()
                            .flex_row()
                            .items_center()
                            .gap(8.)
                            .child(ui::ring(done).size(36.).thickness(4.))
                            .child(
                                div()
                                    .gap(2.)
                                    .child(text(strings::tabs_todos(cx)).bold())
                                    .child(text(strings::widget_open(cx, count as i64))),
                            )
                            .into_element();
                    }
                    _ => {}
                }
                let rows = if family == WidgetFamily::Small { 2 } else { 3 };
                let list = div()
                    .gap(6.)
                    .flex_1()
                    .child(
                        div()
                            .flex_row()
                            .items_center()
                            .gap(6.)
                            .child(image("checklist").text_color(t.accent).text_size(16.))
                            .child(
                                text(strings::tabs_todos(cx))
                                    .semibold()
                                    .text_size(15.)
                                    .text_color(t.text),
                            ),
                    )
                    .child(
                        div()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                text(count.to_string())
                                    .text_size(34.)
                                    .bold()
                                    .text_color(t.accent)
                                    .accessibility_label(strings::home_open(cx, count as i64)),
                            )
                            .when(family == WidgetFamily::Small, |d| {
                                d.child(ui::ring(done).size(40.).thickness(5.))
                            }),
                    )
                    .children(open.into_iter().take(rows).map(|(id, title)| {
                        div()
                            .flex_row()
                            .items_center()
                            .gap(6.)
                            .link(format!("/todos/{id}"))
                            .child(image("circle").text_color(t.muted).text_size(11.))
                            .child(text(title).text_size(13.).text_color(t.text))
                    }));
                if family == WidgetFamily::Small {
                    return list.into_element();
                }
                div()
                    .flex_row()
                    .items_center()
                    .gap(12.)
                    .child(list)
                    .child(
                        ui::ring(done)
                            .size(84.)
                            .thickness(9.)
                            .label(percent)
                            .caption(strings::widget_done(cx)),
                    )
                    .into_element()
            }),
    );
    let todo = todo_widget(cx, store);
    cx.register_widget(todo);
}

/// One todo, picked per instance. The options are the todos themselves: the
/// edit sheet lists the current ones (written again whenever the store
/// changes, through `reload_widgets_on`).
fn todo_widget(cx: &App, store: Entity<Store>) -> Widget {
    let options = store;
    let name = strings::widget_todo_name(cx);
    Widget::new("todo", name.clone())
        .description(strings::widget_todo_description(cx))
        .families(&[WidgetFamily::Small, WidgetFamily::AccessoryRectangular])
        .link("/")
        .parameter(WidgetParameter::new("todo", name).options(move |cx| {
            options
                .read(cx)
                .todos
                .iter()
                .map(|t| WidgetOption::new(t.id.to_string(), t.title.clone()))
                .collect()
        }))
        .render_configured(move |cx, family, config| {
            let t = Theme::of(cx);
            let s = store.read(cx);
            let todo = config
                .get("todo")
                .and_then(|id| s.todos.iter().find(|t| t.id.to_string() == id));
            let Some(todo) = todo else {
                return text(strings::tabs_todos(cx))
                    .text_color(t.muted)
                    .into_element();
            };
            let mark = if todo.done {
                "checkmark.circle.fill"
            } else {
                "circle"
            };
            let link = format!("/todos/{}", todo.id);
            if family == WidgetFamily::AccessoryRectangular {
                return div()
                    .flex_row()
                    .items_center()
                    .gap(6.)
                    .link(link)
                    .child(image(mark))
                    .child(text(todo.title.clone()).bold())
                    .into_element();
            }
            div()
                .gap(8.)
                .flex_1()
                .link(link)
                .child(
                    // Taps here toggle the todo in place (the rest opens it).
                    div()
                        .widget_action("toggle", todo.id.to_string())
                        .accessibility_label(if todo.done {
                            strings::widget_mark_open(cx)
                        } else {
                            strings::widget_mark_done(cx)
                        })
                        .child(image(mark).text_color(t.accent).text_size(24.)),
                )
                .child(
                    text(todo.title.clone())
                        .semibold()
                        .text_size(17.)
                        .text_color(t.text),
                )
                .into_element()
        })
}
