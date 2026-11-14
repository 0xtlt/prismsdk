//! Menus outside the window: a menu bar extra / tray item with the open-todo
//! count, and app menus with keyboard shortcuts. Both are no-ops where the
//! platform has neither (iOS, web), as reported by `cx.capabilities()`.

use std::rc::Rc;

use prism::prelude::*;

use crate::Store;

fn go(path: &'static str) -> prism::Handler<ClickEvent> {
    Rc::new(move |_: &ClickEvent, app: &mut App| app.navigate(path))
}

pub fn install(cx: &mut App, store: Entity<Store>) {
    if cx.capabilities().app_menu {
        cx.set_app_menu(vec![
            Menu::titled("File").item(MenuItem::new("New Todo…").shortcut("cmd+n").on_select(
                Rc::new(|_: &ClickEvent, app: &mut App| app.present("/todos/new")),
            )),
            Menu::titled("View")
                .item(MenuItem::new("Todos").shortcut("cmd+1").on_select(go("/")))
                .item(
                    MenuItem::new("Focus")
                        .shortcut("cmd+2")
                        .on_select(go("/focus")),
                )
                .item(
                    MenuItem::new("Components")
                        .shortcut("cmd+3")
                        .on_select(go("/components")),
                )
                .item(
                    MenuItem::new("Settings")
                        .shortcut("cmd+,")
                        .on_select(go("/settings")),
                ),
        ]);
    }
    if cx.capabilities().tray {
        update_tray(cx, store);
        cx.on_notify(&store, move |app| update_tray(app, store));
    }
}

/// The menu bar item: open todos at a glance, quick actions.
fn update_tray(cx: &mut App, store: Entity<Store>) {
    let open: Vec<(u64, String)> = store
        .read(cx)
        .todos
        .iter()
        .filter(|t| !t.done)
        .map(|t| (t.id, t.title.clone()))
        .collect();
    let mut menu =
        Menu::new().item(MenuItem::new(format!("{} open todo(s)", open.len())).enabled(false));
    for (id, title) in open.iter().take(5) {
        let id = *id;
        menu = menu.item(
            MenuItem::new(title.clone())
                .icon("circle")
                .on_select(Rc::new(move |_: &ClickEvent, app: &mut App| {
                    store.update(app, |s, cx| s.toggle(id, cx))
                })),
        );
    }
    menu = menu
        .separator()
        .item(MenuItem::new("New Todo…").icon("plus").on_select(Rc::new(
            |_: &ClickEvent, app: &mut App| app.present("/todos/new"),
        )))
        .item(
            MenuItem::new("Start a focus session")
                .icon("timer")
                .on_select(go("/focus")),
        );
    cx.set_tray(
        Tray::new("todos")
            .icon("checklist")
            .title(open.len().to_string())
            .tooltip(format!("{} open todos", open.len()))
            .menu(menu),
    );
}
