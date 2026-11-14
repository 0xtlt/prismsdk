//! Menus outside the window: a menu bar extra / tray item with the open-todo
//! count, and app menus with keyboard shortcuts. Both are no-ops where the
//! platform has neither (iOS, web), as reported by `cx.capabilities()`.

use std::rc::Rc;

use prism::prelude::*;

use crate::{Store, strings};

/// Kept so menus can be rebuilt in another language.
struct MenuStore(Entity<Store>);

fn go(path: &'static str) -> prism::Handler<ClickEvent> {
    Rc::new(move |_: &ClickEvent, app: &mut App| app.navigate(path))
}

pub fn install(cx: &mut App, store: Entity<Store>) {
    cx.set_global(MenuStore(store));
    refresh(cx);
    if cx.capabilities().tray {
        cx.on_notify(&store, move |app| update_tray(app, store));
    }
}

/// Builds the app menus and the tray in the current language.
pub fn refresh(cx: &mut App) {
    let Some(store) = cx.try_global::<MenuStore>().map(|m| m.0) else {
        return;
    };
    if cx.capabilities().app_menu {
        cx.set_app_menu(vec![
            Menu::titled(strings::menu_file(cx)).item(
                MenuItem::new(strings::menu_new_todo(cx))
                    .shortcut("cmd+n")
                    .on_select(Rc::new(|_: &ClickEvent, app: &mut App| {
                        app.present("/todos/new")
                    })),
            ),
            Menu::titled(strings::menu_view(cx))
                .item(
                    MenuItem::new(strings::tabs_todos(cx))
                        .shortcut("cmd+1")
                        .on_select(go("/")),
                )
                .item(
                    MenuItem::new(strings::tabs_focus(cx))
                        .shortcut("cmd+2")
                        .on_select(go("/focus")),
                )
                .item(
                    MenuItem::new(strings::tabs_components(cx))
                        .shortcut("cmd+3")
                        .on_select(go("/components")),
                )
                .item(
                    MenuItem::new(strings::tabs_settings(cx))
                        .shortcut("cmd+,")
                        .on_select(go("/settings")),
                ),
        ]);
    }
    if cx.capabilities().tray {
        update_tray(cx, store);
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
    let mut menu = Menu::new()
        .item(MenuItem::new(strings::menu_open_todos(cx, open.len() as i64)).enabled(false));
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
        .item(
            MenuItem::new(strings::menu_new_todo(cx))
                .icon("plus")
                .on_select(Rc::new(|_: &ClickEvent, app: &mut App| {
                    app.present("/todos/new")
                })),
        )
        .item(
            MenuItem::new(strings::menu_focus_session(cx))
                .icon("timer")
                .on_select(go("/focus")),
        );
    cx.set_tray(
        Tray::new("todos")
            .icon("checklist")
            .title(open.len().to_string())
            .tooltip(strings::menu_open_todos(cx, open.len() as i64))
            .menu(menu),
    );
}
