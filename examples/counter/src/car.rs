//! In the car (CarPlay, Android Auto): the todos as the platform's list and
//! pane templates, rebuilt whenever the store changes.

use prism::prelude::*;

use crate::{Store, strings};

pub fn register(cx: &mut App, store: Entity<Store>) {
    cx.register_car(move |cx| {
        let todos = store.read(cx).todos.clone();
        let item = |cx: &mut App, t: &crate::Todo| {
            let id = t.id;
            CarItem::new(&t.title)
                .detail(if t.done {
                    strings::todo_done(cx)
                } else {
                    strings::todo_open(cx)
                })
                .icon(if t.done {
                    "checkmark.circle.fill"
                } else {
                    "circle"
                })
                .browsable()
                .on_select(move |cx| cx.car_push(move |cx| detail(cx, store, id)))
        };
        let open: Vec<CarItem> = todos
            .iter()
            .filter(|t| !t.done)
            .map(|t| item(cx, t))
            .collect();
        let done: Vec<CarItem> = todos
            .iter()
            .filter(|t| t.done)
            .map(|t| item(cx, t))
            .collect();
        CarTemplate::list(strings::tabs_todos(cx))
            .section(CarSection::new().header(strings::todo_open(cx)).items(open))
            .section(CarSection::new().header(strings::todo_done(cx)).items(done))
    });
}

/// One todo: its status and an action, like the phone's detail screen.
fn detail(cx: &mut App, store: Entity<Store>, id: u64) -> CarTemplate {
    let Some(todo) = store.read(cx).todos.iter().find(|t| t.id == id).cloned() else {
        return CarTemplate::pane(strings::todo_missing(cx, id as i64));
    };
    let status = if todo.done {
        strings::todo_done(cx)
    } else {
        strings::todo_open(cx)
    };
    let toggle = if todo.done {
        strings::todo_mark_open(cx)
    } else {
        strings::todo_mark_done(cx)
    };
    CarTemplate::pane(todo.title.clone())
        .row(strings::todo_number(cx, id as i64), status)
        .button(
            CarButton::new(toggle)
                .style(CarButtonStyle::Primary)
                .on_select(move |cx| {
                    store.update(cx, |s, cx| s.toggle(id, cx));
                    cx.car_pop();
                }),
        )
}
