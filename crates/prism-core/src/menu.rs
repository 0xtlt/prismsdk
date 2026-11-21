//! Menus, alerts and system tray items — one model, native everywhere.
//!
//! - **Context menus** (`div().context_menu(menu)`): long press on iOS
//!   (`UIContextMenuInteraction`) and Android, right-click / ⌃-click on macOS,
//!   right-click or a long press on Linux, right-click, Shift+F10 or a long
//!   press on Windows, right-click, long touch and the Menu key on the web.
//! - **Alerts & confirmation dialogs** (`cx.show_alert(...)`):
//!   `UIAlertController`, `NSAlert`, Android's `AlertDialog`, a modal GTK
//!   window, a WinUI `ContentDialog`, `<dialog role=alertdialog>`.
//! - **Tray / menu bar extras** (`cx.set_tray(...)`): `NSStatusItem` on macOS,
//!   a StatusNotifierItem on Linux, `Shell_NotifyIcon` on Windows. iOS,
//!   Android and the web report `capabilities().tray == false`.
//! - **App menu** (`cx.set_app_menu(...)`): the macOS menu bar (and Windows
//!   menu bars later), with keyboard shortcuts.
//! - **Pop-up menus** (`cx.show_menu(...)`, `cx.show_menu_on(...)`,
//!   `div().on_click_menu(menu)`, #247): the platform's menu at a point or
//!   below an element (a "+" button, a right-click on a canvas).

use crate::app::App;
use crate::element::{ClickEvent, Handler};
use crate::surfaces::json_str;

/// Visual role of a menu item or alert button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionRole {
    /// A regular item or button.
    #[default]
    Normal,
    /// Shown in red; never the default button.
    Destructive,
    /// Dismisses without doing anything (Escape).
    Cancel,
}

/// An action the platform performs itself (#161), for menu bar menus the
/// app builds: [`MenuItem::system`]. On macOS it goes to the focused view
/// (Cut, Copy, Paste in a text field) or the window (Minimize, Zoom) like
/// the system's own items; elsewhere these items are left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemAction {
    /// Undo in the focused text field (`undo:`, ⌘Z).
    Undo,
    /// Redo in the focused text field (`redo:`, ⇧⌘Z).
    Redo,
    /// `cut:` (⌘X).
    Cut,
    /// `copy:` (⌘C).
    Copy,
    /// `paste:` (⌘V).
    Paste,
    /// Paste without the text's formatting (⌥⇧⌘V).
    PasteAndMatchStyle,
    /// `delete:`.
    Delete,
    /// `selectAll:` (⌘A).
    SelectAll,
    /// Minimize the window (⌘M).
    Minimize,
    /// Zoom the window.
    Zoom,
    /// Enter or leave full screen (⌃⌘F).
    ToggleFullScreen,
    /// Close the window (⌘W).
    CloseWindow,
    /// Hide the app (⌘H).
    Hide,
    /// Hide the other apps (⌥⌘H).
    HideOthers,
    /// Show all apps.
    ShowAll,
    /// The standard About panel.
    About,
    /// The Services submenu.
    Services,
    /// Quit (⌘Q), asking first with unsaved changes.
    Quit,
}

impl SystemAction {
    /// The English title, used when the item has none of its own.
    pub fn default_title(self) -> &'static str {
        match self {
            SystemAction::Undo => "Undo",
            SystemAction::Redo => "Redo",
            SystemAction::Cut => "Cut",
            SystemAction::Copy => "Copy",
            SystemAction::Paste => "Paste",
            SystemAction::PasteAndMatchStyle => "Paste and Match Style",
            SystemAction::Delete => "Delete",
            SystemAction::SelectAll => "Select All",
            SystemAction::Minimize => "Minimize",
            SystemAction::Zoom => "Zoom",
            SystemAction::ToggleFullScreen => "Enter Full Screen",
            SystemAction::CloseWindow => "Close",
            SystemAction::Hide => "Hide",
            SystemAction::HideOthers => "Hide Others",
            SystemAction::ShowAll => "Show All",
            SystemAction::About => "About",
            SystemAction::Services => "Services",
            SystemAction::Quit => "Quit",
        }
    }

    /// The usual shortcut (`"cmd+shift+z"`), if any.
    pub fn default_shortcut(self) -> Option<&'static str> {
        Some(match self {
            SystemAction::Undo => "cmd+z",
            SystemAction::Redo => "cmd+shift+z",
            SystemAction::Cut => "cmd+x",
            SystemAction::Copy => "cmd+c",
            SystemAction::Paste => "cmd+v",
            SystemAction::PasteAndMatchStyle => "cmd+alt+shift+v",
            SystemAction::SelectAll => "cmd+a",
            SystemAction::Minimize => "cmd+m",
            SystemAction::ToggleFullScreen => "cmd+ctrl+f",
            SystemAction::CloseWindow => "cmd+w",
            SystemAction::Hide => "cmd+h",
            SystemAction::HideOthers => "cmd+alt+h",
            SystemAction::Quit => "cmd+q",
            _ => return None,
        })
    }
}

/// One entry of a [`Menu`].
#[derive(Clone)]
pub enum MenuItem {
    /// A selectable item.
    Action {
        /// Its label.
        title: String,
        /// SF Symbol name (Apple platforms).
        icon: Option<String>,
        /// `"cmd+shift+n"` style shortcut (menu bar menus).
        shortcut: Option<String>,
        /// The shortcut is only shown, never bound (#247): see
        /// [`MenuItem::shortcut_hint`].
        shortcut_hint: bool,
        /// How it is drawn (normal, destructive, cancel).
        role: ActionRole,
        /// `Some` shows a check mark (on) or none (off); `None` is not checkable.
        checked: Option<bool>,
        /// Disabled items are shown dimmed and cannot be chosen.
        enabled: bool,
        /// Runs when the item is chosen; an item without one is disabled.
        handler: Option<Handler<ClickEvent>>,
    },
    /// A line between groups of items.
    Separator,
    /// An item opening a nested menu.
    Submenu {
        /// Its label.
        title: String,
        /// SF Symbol name (Apple platforms).
        icon: Option<String>,
        /// The nested menu.
        menu: Menu,
    },
    /// An action the platform performs itself (#161): see [`SystemAction`].
    System {
        /// Which one.
        action: SystemAction,
        /// Its label (`None`: the English default).
        title: Option<String>,
        /// Its shortcut (`None`: the usual one).
        shortcut: Option<String>,
    },
}

impl MenuItem {
    /// An item the platform performs (#161), titled in English unless given
    /// [`title`](Self::title): `MenuItem::system(SystemAction::Copy).title("Copier")`.
    pub fn system(action: SystemAction) -> Self {
        MenuItem::System {
            action,
            title: None,
            shortcut: None,
        }
    }

    /// Sets the label of a [`system`](Self::system) item (its translation).
    pub fn title(mut self, label: impl Into<String>) -> Self {
        match &mut self {
            MenuItem::System { title, .. } => *title = Some(label.into()),
            MenuItem::Action { title, .. } | MenuItem::Submenu { title, .. } => {
                *title = label.into()
            }
            MenuItem::Separator => {}
        }
        self
    }
    /// An enabled action item titled `title`, without a handler yet.
    pub fn new(title: impl Into<String>) -> Self {
        MenuItem::Action {
            title: title.into(),
            icon: None,
            shortcut: None,
            shortcut_hint: false,
            role: ActionRole::Normal,
            checked: None,
            enabled: true,
            handler: None,
        }
    }

    /// Sets the SF Symbol shown next to the title (Apple platforms).
    pub fn icon(mut self, symbol: impl Into<String>) -> Self {
        match &mut self {
            MenuItem::Action { icon, .. } | MenuItem::Submenu { icon, .. } => {
                *icon = Some(symbol.into())
            }
            MenuItem::Separator | MenuItem::System { .. } => {}
        }
        self
    }
    /// Sets a keyboard shortcut such as `"cmd+shift+n"` (menu bar menus).
    pub fn shortcut(mut self, keys: impl Into<String>) -> Self {
        match &mut self {
            MenuItem::System { shortcut, .. } => *shortcut = Some(keys.into()),
            MenuItem::Action {
                shortcut,
                shortcut_hint,
                ..
            } => {
                *shortcut = Some(keys.into());
                *shortcut_hint = false;
            }
            MenuItem::Separator | MenuItem::Submenu { .. } => {}
        }
        self
    }

    /// Shows `keys` (`"cmd+b"`) next to the item without binding them
    /// (#247): the key keeps going to the focused view, which handles it
    /// itself (⌘B in a text field, ⌘D on a canvas, ⌘= for a zoom the view
    /// does), and the menu still documents it. Choosing the item runs its
    /// handler as usual. Written like [`shortcut`](Self::shortcut).
    pub fn shortcut_hint(mut self, keys: impl Into<String>) -> Self {
        if let MenuItem::Action {
            shortcut,
            shortcut_hint,
            ..
        } = &mut self
        {
            *shortcut = Some(keys.into());
            *shortcut_hint = true;
        }
        self
    }
    /// Draws the item as destructive (red).
    pub fn destructive(mut self) -> Self {
        if let MenuItem::Action { role, .. } = &mut self {
            *role = ActionRole::Destructive;
        }
        self
    }
    /// Makes the item checkable, with a check mark when `on`.
    pub fn checked(mut self, on: bool) -> Self {
        if let MenuItem::Action { checked, .. } = &mut self {
            *checked = Some(on);
        }
        self
    }
    /// Enables or disables the item.
    pub fn enabled(mut self, on: bool) -> Self {
        if let MenuItem::Action { enabled, .. } = &mut self {
            *enabled = on;
        }
        self
    }
    /// Sets the handler run when the item is chosen.
    pub fn on_select(mut self, h: Handler<ClickEvent>) -> Self {
        if let MenuItem::Action { handler, .. } = &mut self {
            *handler = Some(h);
        }
        self
    }

    /// An item opening `menu` as a submenu.
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
    /// Title shown in the menu bar (empty for context menus and trays).
    pub title: String,
    /// The entries, in order.
    pub items: Vec<MenuItem>,
}

impl Menu {
    /// An empty, untitled menu (context menus, trays).
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

    /// Appends an item.
    pub fn item(mut self, item: MenuItem) -> Self {
        self.items.push(item);
        self
    }

    /// Shorthand for an action item.
    pub fn action(self, title: impl Into<String>, h: Handler<ClickEvent>) -> Self {
        self.item(MenuItem::new(title).on_select(h))
    }

    /// Appends a separator.
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
            MenuItem::System {
                action,
                title,
                shortcut,
            } => MenuItemSpec::System {
                action: *action,
                title: title
                    .clone()
                    .unwrap_or_else(|| action.default_title().to_string()),
                shortcut: shortcut
                    .clone()
                    .or_else(|| action.default_shortcut().map(str::to_string)),
            },
            MenuItem::Submenu { title, icon, menu } => MenuItemSpec::Submenu {
                title: title.clone(),
                icon: icon.clone(),
                menu: compile_items(&menu.items, next, handlers, title),
            },
            MenuItem::Action {
                title,
                icon,
                shortcut,
                shortcut_hint,
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
                    shortcut_hint: *shortcut_hint && shortcut.is_some(),
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
    /// Title shown in the menu bar (empty for context menus and trays).
    pub title: String,
    /// The entries, in order.
    pub items: Vec<MenuItemSpec>,
}

/// One entry of a [`MenuSpec`], as platforms see it.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuItemSpec {
    /// A selectable item.
    Action {
        /// Reported with `UiEvent::MenuSelect(id)` when chosen.
        id: u64,
        /// Its label.
        title: String,
        /// SF Symbol name (Apple platforms).
        icon: Option<String>,
        /// `"cmd+shift+n"` style shortcut.
        shortcut: Option<String>,
        /// The shortcut is only shown next to the title, never bound
        /// (#247): backends leave the key to the focused view.
        shortcut_hint: bool,
        /// How it is drawn.
        role: ActionRole,
        /// `Some` for checkable items: whether the check mark shows.
        checked: Option<bool>,
        /// `false` when disabled or without a handler.
        enabled: bool,
    },
    /// An action the platform performs itself (#161). Backends without
    /// such actions leave it out.
    System {
        /// Which one.
        action: SystemAction,
        /// Its label.
        title: String,
        /// Its shortcut.
        shortcut: Option<String>,
    },
    /// A line between groups of items.
    Separator,
    /// An item opening a nested menu.
    Submenu {
        /// Its label.
        title: String,
        /// SF Symbol name (Apple platforms).
        icon: Option<String>,
        /// The nested menu.
        menu: MenuSpec,
    },
}

impl MenuSpec {
    /// The JSON form, for hosts that receive menus as text.
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
    /// The JSON of this item (see [`MenuSpec::to_json`]).
    pub fn to_json(&self) -> String {
        let opt = |s: &Option<String>| s.as_deref().map_or("null".into(), json_str);
        match self {
            MenuItemSpec::Separator => r#"{"t":"sep"}"#.into(),
            MenuItemSpec::System {
                action,
                title,
                shortcut,
            } => format!(
                r#"{{"t":"system","action":{},"title":{},"shortcut":{}}}"#,
                json_str(&format!("{action:?}")),
                json_str(title),
                opt(shortcut)
            ),
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
                shortcut_hint,
                role,
                checked,
                enabled,
            } => format!(
                r#"{{"t":"act","id":{id},"title":{},"icon":{},"shortcut":{},"hint":{shortcut_hint},"role":"{}","checked":{},"enabled":{enabled}}}"#,
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

/// How an [`Alert`] is presented.
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
    /// A centered alert titled `title`, without buttons yet.
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

    /// Sets the text shown under the title.
    pub fn message(mut self, m: impl Into<String>) -> Self {
        self.message = Some(m.into());
        self
    }

    /// Adds a regular button running `h`.
    pub fn action(mut self, title: impl Into<String>, h: Handler<ClickEvent>) -> Self {
        self.actions
            .push((title.into(), ActionRole::Normal, Some(h)));
        self
    }

    /// Adds a destructive (red) button running `h`.
    pub fn destructive(mut self, title: impl Into<String>, h: Handler<ClickEvent>) -> Self {
        self.actions
            .push((title.into(), ActionRole::Destructive, Some(h)));
        self
    }

    /// Adds the button that dismisses the alert without doing anything.
    pub fn cancel(mut self, title: impl Into<String>) -> Self {
        self.actions.push((title.into(), ActionRole::Cancel, None));
        self
    }
}

/// An alert as platforms see it. The chosen button is reported with
/// `UiEvent::AlertAction(id, index)`.
#[derive(Clone, Debug, PartialEq)]
pub struct AlertSpec {
    /// Identifies the alert in `UiEvent::AlertAction`.
    pub id: u64,
    /// The title.
    pub title: String,
    /// The text under the title.
    pub message: Option<String>,
    /// Centered alert or action sheet.
    pub style: AlertStyle,
    /// The buttons (title and role), in order.
    pub actions: Vec<(String, ActionRole)>,
}

impl AlertSpec {
    /// The JSON form, for hosts that receive alerts as text.
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
    /// Sets the menu opened by clicking the item.
    pub fn menu(mut self, m: Menu) -> Self {
        self.menu = m;
        self
    }
}

/// A tray item as platforms see it (`PlatformRequest::SetTray`).
#[derive(Clone, Debug, PartialEq)]
pub struct TraySpec {
    /// Identifies the item across updates.
    pub id: String,
    /// SF Symbol name (template image).
    pub icon: Option<String>,
    /// Short text next to the icon.
    pub title: Option<String>,
    /// Help text, also the accessibility label.
    pub tooltip: Option<String>,
    /// The menu opened by clicking the item.
    pub menu: MenuSpec,
}

// ---------------------------------------------------------------------------
// App-level registry (alerts, trays, app menu)
// ---------------------------------------------------------------------------

/// Ids of menus owned by the app (tray, app menu) live above this; ids of
/// context menus are derived from node ids below it (`node << 16`, so up to
/// 2^36 nodes). 2^52 keeps every id exact in a JavaScript number (2^53):
/// the web page sends the ids of the app menus' shortcuts back (#170).
pub(crate) const APP_MENU_BASE: u64 = 1 << 52;

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
            MenuItemSpec::Separator | MenuItemSpec::System { .. } => 0,
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

    /// Removes the tray item set with this id.
    pub fn remove_tray(&mut self, id: impl Into<String>) {
        self.surfaces
            .requests
            .push(crate::PlatformRequest::RemoveTray(id.into()));
    }

    /// The whole menu bar, defined by the app (#161): no menu of Prism's own
    /// (its English application, Edit and Go menus). The first menu is the
    /// application menu (macOS titles it with the app's name). Build Edit
    /// and Window menus with [`MenuItem::system`] items next to the app's
    /// own: an app item with the shortcut `cmd+z` takes ⌘Z, even from a
    /// focused text field, so an app with its own history hears it. On the
    /// web, the shortcuts of its items are the app's too (see
    /// [`set_app_menu`](Self::set_app_menu)). macOS only shows a menu bar.
    pub fn set_menu_bar(&mut self, menus: Vec<Menu>) {
        let specs = menus
            .iter()
            .enumerate()
            .map(|(i, m)| self.menus.compile(&format!("app-menu:{i}"), m))
            .collect();
        self.surfaces
            .requests
            .push(crate::PlatformRequest::SetMenuBar(specs));
    }

    /// Sets the app's menu bar menus (`File`, `View`…), shown between the
    /// app menu and the standard Edit/Window menus. On the web the
    /// shortcuts of their items are the app's (#161): the page takes them
    /// before the browser (⌘Z in a field, ⌘S, ⌘K) and runs the item.
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

    /// Opens `menu` as the platform's pop-up menu at `at`, in points from
    /// the top-left corner of the window's content (#247), after the next
    /// render: `NSMenu` on macOS, `UIMenu` on iOS, iPadOS and visionOS
    /// (17.4+), a `PopupMenu` on Android, a `GtkPopoverMenu` on Linux, a
    /// `MenuFlyout` on Windows, the page's own menu on the web. Nothing on tvOS and watchOS. Its items
    /// run their handlers like a context menu's; showing another pop-up
    /// menu replaces this one's.
    pub fn show_menu(&mut self, menu: Menu, at: crate::events::Point) {
        let spec = self.menus.compile("popup", &menu);
        self.ui_commands.push(crate::app::UiCommand::ShowMenu(
            crate::app::MenuTarget::Window,
            spec,
            Some(at),
        ));
    }

    /// Opens `menu` over the element named `id` (`.id(id)`, #247): at `at`
    /// in the element's coordinates (a [`ClickEvent::position`], a right
    /// click on a canvas), or below it when `None` (a "+" button), as the
    /// platform places menus under their control. See
    /// [`show_menu`](Self::show_menu); nothing when no element has this id.
    pub fn show_menu_on(
        &mut self,
        id: impl Into<String>,
        menu: Menu,
        at: Option<crate::events::Point>,
    ) {
        let spec = self.menus.compile("popup", &menu);
        self.ui_commands.push(crate::app::UiCommand::ShowMenu(
            crate::app::MenuTarget::Named(id.into()),
            spec,
            at,
        ));
    }

    /// `Div::on_click_menu`: the menu below the clicked node.
    pub(crate) fn show_menu_below(&mut self, node: crate::reconcile::NodeId, menu: &Menu) {
        let spec = self.menus.compile("popup", menu);
        self.ui_commands.push(crate::app::UiCommand::ShowMenu(
            crate::app::MenuTarget::Node(node),
            spec,
            None,
        ));
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
