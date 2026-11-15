//! The "Todos" widget: one description, rendered by WidgetKit on iOS/macOS
//! and by RemoteViews on Android. Re-rendered whenever the store changes.
//! Its ring is a `prism::ui` chart: drawn with SwiftUI paths in WidgetKit,
//! and as a bitmap on Android.

use prism::prelude::*;

use crate::strings;
use prism::ui::{self, Theme};

use crate::Store;

pub fn register(cx: &mut App, store: Entity<Store>) {
    cx.register_widget(
        Widget::new("todos", "Todos")
            .description("Your open todos at a glance.")
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
    cx.reload_widgets_on(&store);
}
