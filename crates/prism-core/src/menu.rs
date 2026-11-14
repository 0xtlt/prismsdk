//! Menus, alerts and system tray items — one model, native everywhere.
//!
//! - **Context menus** (`div().context_menu(menu)`): long press on iOS
//!   (`UIContextMenuInteraction`), right-click / ⌃-click on macOS, right-click,
//!   long touch and the Menu key on the web.
//! - **Alerts & confirmation dialogs** (`cx.show_alert(...)`):
//!   `UIAlertController`, `NSAlert`, `<dialog role=alertdialog>`.
//! - **Tray / menu bar extras** (`cx.set_tray(...)`): `NSStatusItem` on macOS;
//!   the system tray on Windows and Linux later. iOS and the web report
//!   `capabilities().tray == false`.
//! - **App menu** (`cx.set_app_menu(...)`): the macOS menu bar (and Windows
//!   menu bars later), with keyboard shortcuts.

use crate::app::App;
use crate::element::{ClickEvent, Handler};
use crate::surfaces::json_str;

/// Visual role of a menu item or alert button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionRole {
    #[default]
    Normal,
    /// Shown in red; never the default button.
    Destructive,
    /// Dismisses without doing anything (Escape).
    Cancel,
}

/// One entry of a [`Menu`].
#[derive(Clone)]
pub enum MenuItem {
    Action {
        title: String,
        /// SF Symbol name (Apple platforms).
        icon: Option<String>,
        /// `"cmd+shift+n"` style shortcut (menu bar menus).
        shortcut: Option<String>,
        role: ActionRole,
        checked: Option<bool>,
        enabled: bool,
        handler: Option<Handler<ClickEvent>>,
    },
    Separator,
    Submenu {
        title: String,
        icon: Option<String>,
        menu: Menu,
    },
}

impl MenuItem {
    pub fn new(title: impl Into<String>) -> Self {
        MenuItem::Action {
            title: title.into(),
            icon: None,
            shortcut: None,
            role: ActionRole::Normal,
            checked: None,
            enabled: true,
            handler: None,
        }
    }

    pub fn icon(mut self, symbol: impl Into<String>) -> Self {
        match &mut self {
            MenuItem::Action { icon, .. } | MenuItem::Submenu { icon, .. } => {
                *icon = Some(symbol.into())
            }
            MenuItem::Separator => {}
        }
        self
    }
    pub fn shortcut(mut self, keys: impl Into<String>) -> Self {
        if let MenuItem::Action { shortcut, .. } = &mut self {
            *shortcut = Some(keys.into());
        }
        self
    }
    pub fn destructive(mut self) -> Self {
        if let MenuItem::Action { role, .. } = &mut self {
            *role = ActionRole::Destructive;
        }
        self
    }
    pub fn checked(mut self, on: bool) -> Self {
        if let MenuItem::Action { checked, .. } = &mut self {
            *checked = Some(on);
        }
        self
    }
    pub fn enabled(mut self, on: bool) -> Self {
        if let MenuItem::Action { enabled, .. } = &mut self {
            *enabled = on;
        }
        self
    }
    pub fn on_select(mut self, h: Handler<ClickEvent>) -> Self {
        if let MenuItem::Action { handler, .. } = &mut self {
            *handler = Some(h);
        }
        self
    }

    pub fn submenu(title: impl Into<String>, menu: Menu) -> Self {
        MenuItem::Submenu {
            title: title.into(),
            icon: None,
            menu,
        }
    }
}

/// An ordered list of menu items, optionally titled (menu bar menus).
#[derive(Clone, Default)]
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
}

impl Menu {
    pub fn new() -> Self {
        Menu::default()
    }

    /// A titled menu, for the app menu bar (`File`, `View`…).
    pub fn titled(title: impl Into<String>) -> Self {
        Menu {
            title: title.into(),
            items: Vec::new(),
        }
    }

    pub fn item(mut self, item: MenuItem) -> Self {
        self.items.push(item);
        self
    }

    /// Shorthand for an action item.
    pub fn action(self, title: impl Into<String>, h: Handler<ClickEvent>) -> Self {
        self.item(MenuItem::new(title).on_select(h))
    }

    pub fn separator(self) -> Self {
        self.item(MenuItem::Separator)
    }

    /// Assigns ids (`base + n`) to actions, registers their handlers, and
    /// returns the platform-facing description.
    pub(crate) fn compile(
        &self,
        base: u64,
        handlers: &mut Vec<(u64, Handler<ClickEvent>)>,
    ) -> MenuSpec {
        let mut next = base;
        compile_items(&self.items, &mut next, handlers, &self.title)
    }
}

fn compile_items(
    items: &[MenuItem],
    next: &mut u64,
    handlers: &mut Vec<(u64, Handler<ClickEvent>)>,
    title: &str,
) -> MenuSpec {
    let items = items
        .iter()
        .map(|item| match item {
            MenuItem::Separator => MenuItemSpec::Separator,
            MenuItem::Submenu { title, icon, menu } => MenuItemSpec::Submenu {
                title: title.clone(),
                icon: icon.clone(),
                menu: compile_items(&menu.items, next, handlers, title),
            },
            MenuItem::Action {
                title,
                icon,
                shortcut,
                role,
                checked,
                enabled,
                handler,
            } => {
                *next += 1;
                if let Some(h) = handler {
                    handlers.push((*next, h.clone()));
                }
                MenuItemSpec::Action {
                    id: *next,
                    title: title.clone(),
                    icon: icon.clone(),
                    shortcut: shortcut.clone(),
                    role: *role,
                    checked: *checked,
                    enabled: *enabled && handler.is_some(),
                }
            }
        })
        .collect();
    MenuSpec {
        title: title.to_string(),
        items,
    }
}

/// A menu as platforms see it: no closures, actions identified by id. When
/// one is chosen the backend reports `UiEvent::MenuSelect(id)`.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuSpec {
    pub title: String,
    pub items: Vec<MenuItemSpec>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuItemSpec {
    Action {
        id: u64,
        title: String,
        icon: Option<String>,
        shortcut: Option<String>,
        role: ActionRole,
        checked: Option<bool>,
        enabled: bool,
    },
    Separator,
    Submenu {
        title: String,
        icon: Option<String>,
        menu: MenuSpec,
    },
}

impl MenuSpec {
    pub fn to_json(&self) -> String {
        let items: Vec<String> = self.items.iter().map(MenuItemSpec::to_json).collect();
        format!(
            r#"{{"title":{},"items":[{}]}}"#,
            json_str(&self.title),
            items.join(",")
        )
    }
}

impl MenuItemSpec {
    pub fn to_json(&self) -> String {
        let opt = |s: &Option<String>| s.as_deref().map_or("null".into(), json_str);
        match self {
            MenuItemSpec::Separator => r#"{"t":"sep"}"#.into(),
            MenuItemSpec::Submenu { title, icon, menu } => format!(
                r#"{{"t":"sub","title":{},"icon":{},"menu":{}}}"#,
                json_str(title),
                opt(icon),
                menu.to_json()
            ),
            MenuItemSpec::Action {
                id,
                title,
                icon,
                shortcut,
                role,
                checked,
                enabled,
            } => format!(
                r#"{{"t":"act","id":{id},"title":{},"icon":{},"shortcut":{},"role":"{}","checked":{},"enabled":{enabled}}}"#,
                json_str(title),
                opt(icon),
                opt(shortcut),
                match role {
                    ActionRole::Normal => "normal",
                    ActionRole::Destructive => "destructive",
                    ActionRole::Cancel => "cancel",
                },
                checked.map_or("null".into(), |c| c.to_string()),
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Alerts
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlertStyle {
    /// A centered alert.
    #[default]
    Alert,
    /// A confirmation dialog anchored to the bottom (action sheet on iOS).
    ActionSheet,
}

/// A native alert or confirmation dialog.
pub struct Alert {
    title: String,
    message: Option<String>,
    style: AlertStyle,
    actions: Vec<(String, ActionRole, Option<Handler<ClickEvent>>)>,
}

impl Alert {
    pub fn new(title: impl Into<String>) -> Self {
        Alert {
            title: title.into(),
            message: None,
            style: AlertStyle::Alert,
            actions: Vec::new(),
        }
    }

    /// A confirmation dialog (action sheet on iOS).
    pub fn sheet(title: impl Into<String>) -> Self {
        Alert {
            style: AlertStyle::ActionSheet,
            ..Alert::new(title)
        }
    }

    pub fn message(mut self, m: impl Into<String>) -> Self {
        self.message = Some(m.into());
        self
    }

    pub fn action(mut self, title: impl Into<String>, h: Handler<ClickEvent>) -> Self {
        self.actions
            .push((title.into(), ActionRole::Normal, Some(h)));
        self
    }

    pub fn destructive(mut self, title: impl Into<String>, h: Handler<ClickEvent>) -> Self {
        self.actions
            .push((title.into(), ActionRole::Destructive, Some(h)));
        self
    }

    pub fn cancel(mut self, title: impl Into<String>) -> Self {
        self.actions.push((title.into(), ActionRole::Cancel, None));
        self
    }
}

/// An alert as platforms see it. The chosen button is reported with
/// `UiEvent::AlertAction(id, index)`.
#[derive(Clone, Debug, PartialEq)]
pub struct AlertSpec {
    pub id: u64,
    pub title: String,
    pub message: Option<String>,
    pub style: AlertStyle,
    pub actions: Vec<(String, ActionRole)>,
}

impl AlertSpec {
    pub fn to_json(&self) -> String {
        let actions: Vec<String> = self
            .actions
            .iter()
            .map(|(t, r)| {
                format!(
                    r#"{{"title":{},"role":"{}"}}"#,
                    json_str(t),
                    match r {
                        ActionRole::Normal => "normal",
                        ActionRole::Destructive => "destructive",
                        ActionRole::Cancel => "cancel",
                    }
                )
            })
            .collect();
        format!(
            r#"{{"id":{},"title":{},"message":{},"sheet":{},"actions":[{}]}}"#,
            self.id,
            json_str(&self.title),
            self.message.as_deref().map_or("null".into(), json_str),
            self.style == AlertStyle::ActionSheet,
            actions.join(",")
        )
    }
}

// ---------------------------------------------------------------------------
// Tray / menu bar extras
// ---------------------------------------------------------------------------

/// A system tray icon (macOS menu bar extra; Windows/Linux tray later).
#[derive(Clone)]
pub struct Tray {
    id: String,
    icon: Option<String>,
    title: Option<String>,
    tooltip: Option<String>,
    menu: Menu,
}

impl Tray {
    /// `id` identifies the item across updates (`set_tray` again to update it).
    pub fn new(id: impl Into<String>) -> Self {
        Tray {
            id: id.into(),
            icon: None,
            title: None,
            tooltip: None,
            menu: Menu::new(),
        }
    }
    /// SF Symbol name (template image, adapts to light/dark menu bars).
    pub fn icon(mut self, symbol: impl Into<String>) -> Self {
        self.icon = Some(symbol.into());
        self
    }
    /// Short text next to the icon (counts, timers).
    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }
    /// Help text, also the accessibility label.
    pub fn tooltip(mut self, t: impl Into<String>) -> Self {
        self.tooltip = Some(t.into());
        self
    }
    pub fn menu(mut self, m: Menu) -> Self {
        self.menu = m;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TraySpec {
    pub id: String,
    pub icon: Option<String>,
    pub title: Option<String>,
    pub tooltip: Option<String>,
    pub menu: MenuSpec,
}

// ---------------------------------------------------------------------------
// App-level registry (alerts, trays, app menu)
// ---------------------------------------------------------------------------

/// Ids of menus owned by the app (tray, app menu) live above this; ids of
/// context menus are derived from node ids below it.
pub(crate) const APP_MENU_BASE: u64 = 1 << 62;

#[derive(Default)]
pub(crate) struct Menus {
    pub handlers: Vec<(u64, Handler<ClickEvent>)>,
    /// Handler ranges per owner (tray id, "app-menu"), replaced on update.
    pub owners: Vec<(String, u64, u64)>,
    pub next: u64,
    pub alerts: Vec<(u64, Vec<Option<Handler<ClickEvent>>>)>,
    pub next_alert: u64,
}

impl Menus {
    fn compile(&mut self, owner: &str, menu: &Menu) -> MenuSpec {
        // Drop the previous handlers of this owner.
        if let Some(pos) = self.owners.iter().position(|(o, ..)| o == owner) {
            let (_, lo, hi) = self.owners.remove(pos);
            self.handlers.retain(|(id, _)| *id <= lo || *id > hi);
        }
        let base = APP_MENU_BASE + self.next;
        let mut handlers = Vec::new();
        let spec = menu.compile(base, &mut handlers);
        let hi = base + count_actions(&spec);
        self.next = hi - APP_MENU_BASE;
        self.owners.push((owner.to_string(), base, hi));
        self.handlers.extend(handlers);
        spec
    }
}

fn count_actions(spec: &MenuSpec) -> u64 {
    spec.items
        .iter()
        .map(|i| match i {
            MenuItemSpec::Action { .. } => 1,
            MenuItemSpec::Submenu { menu, .. } => count_actions(menu),
            MenuItemSpec::Separator => 0,
        })
        .sum()
}

impl App {
    /// Shows a native alert or confirmation dialog.
    pub fn show_alert(&mut self, alert: Alert) {
        self.menus.next_alert += 1;
        let id = self.menus.next_alert;
        let spec = AlertSpec {
            id,
            title: alert.title,
            message: alert.message,
            style: alert.style,
            actions: alert
                .actions
                .iter()
                .map(|(t, r, _)| (t.clone(), *r))
                .collect(),
        };
        self.menus
            .alerts
            .push((id, alert.actions.into_iter().map(|(_, _, h)| h).collect()));
        self.surfaces
            .requests
            .push(crate::PlatformRequest::ShowAlert(spec));
    }

    /// Adds or updates a tray / menu bar item (no-op where unsupported).
    pub fn set_tray(&mut self, tray: Tray) {
        let menu = self.menus.compile(&format!("tray:{}", tray.id), &tray.menu);
        self.surfaces
            .requests
            .push(crate::PlatformRequest::SetTray(TraySpec {
                id: tray.id,
                icon: tray.icon,
                title: tray.title,
                tooltip: tray.tooltip,
                menu,
            }));
    }

    pub fn remove_tray(&mut self, id: impl Into<String>) {
        self.surfaces
            .requests
            .push(crate::PlatformRequest::RemoveTray(id.into()));
    }

    /// Sets the app's menu bar menus (`File`, `View`…), shown between the
    /// app menu and the standard Edit/Window menus.
    pub fn set_app_menu(&mut self, menus: Vec<Menu>) {
        let specs = menus
            .iter()
            .enumerate()
            .map(|(i, m)| self.menus.compile(&format!("app-menu:{i}"), m))
            .collect();
        self.surfaces
            .requests
            .push(crate::PlatformRequest::SetAppMenu(specs));
    }

    /// Runs the handler of an app-level menu item; `false` if unknown.
    pub(crate) fn select_menu(&mut self, id: u64) -> bool {
        let handler = self
            .menus
            .handlers
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, h)| h.clone());
        match handler {
            Some(h) => {
                h(&ClickEvent, self);
                true
            }
            None => false,
        }
    }

    pub(crate) fn alert_action(&mut self, alert: u64, index: usize) {
        let Some(pos) = self.menus.alerts.iter().position(|(id, _)| *id == alert) else {
            return;
        };
        let (_, handlers) = self.menus.alerts.remove(pos);
        if let Some(Some(h)) = handlers.get(index) {
            let h: Handler<ClickEvent> = h.clone();
            h(&ClickEvent, self);
        }
    }
}
