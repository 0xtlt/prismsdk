//! The "Todos" widget: one description, rendered by WidgetKit on iOS/macOS
//! (and, later, Glance on Android). Re-rendered whenever the store changes.

use prism::prelude::*;
use prism::ui::Theme;

use crate::Store;

pub fn register(cx: &mut App, store: Entity<Store>) {
    cx.register_widget(
        Widget::new("todos", "Todos")
            .description("Your open todos at a glance.")
            .families(&[
                WidgetFamily::Small,
                WidgetFamily::Medium,
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
                let count = open.len();
                if family == WidgetFamily::AccessoryRectangular {
                    // Lock screen: monochrome, tinted by the system.
                    return div()
                        .gap(2.)
                        .child(text("Todos").bold())
                        .child(text(format!("{count} open")))
                        .into_element();
                }
                let rows = if family == WidgetFamily::Small { 2 } else { 3 };
                div()
                    .gap(6.)
                    .child(
                        div()
                            .flex_row()
                            .items_center()
                            .gap(6.)
                            .child(image("checklist").text_color(t.accent).text_size(16.))
                            .child(text("Todos").semibold().text_size(15.).text_color(t.text)),
                    )
                    .child(
                        text(count.to_string())
                            .text_size(34.)
                            .bold()
                            .text_color(t.accent)
                            .accessibility_label(format!("{count} open todos")),
                    )
                    .children(open.into_iter().take(rows).map(|(id, title)| {
                        div()
                            .flex_row()
                            .items_center()
                            .gap(6.)
                            .link(format!("/todos/{id}"))
                            .child(image("circle").text_color(t.muted).text_size(11.))
                            .child(text(title).text_size(13.).text_color(t.text))
                    }))
                    .into_element()
            }),
    );
    cx.reload_widgets_on(&store);
}
