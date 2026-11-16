//! Tray icons (`cx.set_tray`): the StatusNotifierItem protocol (KDE, GNOME
//! with the AppIndicator extension, most Linux panels), through `ksni`.
//! Menu items come back to the app as `UiEvent::MenuSelect`.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::glib;
use ksni::blocking::TrayMethods;
use prism_core::UiEvent;
use prism_core::menu::{MenuItemSpec, MenuSpec, TraySpec};

/// A menu item the tray thread can own.
#[derive(Clone)]
enum Item {
    Action {
        id: u64,
        title: String,
        enabled: bool,
        checked: Option<bool>,
    },
    Separator,
    Sub {
        title: String,
        items: Vec<Item>,
    },
}

fn items(spec: &MenuSpec) -> Vec<Item> {
    spec.items
        .iter()
        // System actions (#161) are AppKit's: not in a tray.
        .filter(|i| !matches!(i, MenuItemSpec::System { .. }))
        .map(|i| match i {
            MenuItemSpec::Action {
                id,
                title,
                enabled,
                checked,
                ..
            } => Item::Action {
                id: *id,
                title: title.clone(),
                enabled: *enabled,
                checked: *checked,
            },
            MenuItemSpec::Separator | MenuItemSpec::System { .. } => Item::Separator,
            MenuItemSpec::Submenu { title, menu, .. } => Item::Sub {
                title: title.clone(),
                items: items(menu),
            },
        })
        .collect()
}

struct PrismTray {
    id: String,
    title: String,
    tooltip: String,
    icon: String,
    items: Vec<Item>,
}

/// Menu choices run on GTK's thread.
fn select(id: u64) {
    glib::MainContext::default().invoke(move || crate::imp::dispatch(UiEvent::MenuSelect(id)));
}

fn ksni_items(items: &[Item]) -> Vec<ksni::MenuItem<PrismTray>> {
    items
        .iter()
        .map(|i| match i.clone() {
            Item::Action {
                id,
                title,
                enabled,
                checked: Some(on),
            } => ksni::menu::CheckmarkItem {
                label: title,
                enabled,
                checked: on,
                activate: Box::new(move |_| select(id)),
                ..Default::default()
            }
            .into(),
            Item::Action {
                id,
                title,
                enabled,
                checked: None,
            } => ksni::menu::StandardItem {
                label: title,
                enabled,
                activate: Box::new(move |_| select(id)),
                ..Default::default()
            }
            .into(),
            Item::Separator => ksni::MenuItem::Separator,
            Item::Sub { title, items } => ksni::menu::SubMenu {
                label: title,
                submenu: ksni_items(&items),
                ..Default::default()
            }
            .into(),
        })
        .collect()
}

impl ksni::Tray for PrismTray {
    fn id(&self) -> String {
        self.id.clone()
    }
    fn title(&self) -> String {
        self.title.clone()
    }
    fn icon_name(&self) -> String {
        self.icon.clone()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.tooltip.clone(),
            ..Default::default()
        }
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        ksni_items(&self.items)
    }
}

thread_local! {
    static TRAYS: RefCell<HashMap<String, ksni::blocking::Handle<PrismTray>>> = RefCell::new(HashMap::new());
}

/// An icon the desktop's theme has: the SF Symbol's name or its first word
/// (`timer`, `bell.fill` → `bell`), else a generic application icon.
fn icon_name(symbol: Option<&str>) -> String {
    let theme = gtk::gdk::Display::default().map(|d| gtk::IconTheme::for_display(&d));
    let candidates: Vec<String> = symbol
        .map(|s| {
            let first = s.split('.').next().unwrap_or(s);
            vec![
                s.to_string(),
                format!("{first}-symbolic"),
                first.to_string(),
            ]
        })
        .unwrap_or_default();
    candidates
        .into_iter()
        .find(|c| theme.as_ref().is_some_and(|t| t.has_icon(c)))
        .unwrap_or_else(|| "application-x-executable".into())
}

pub(crate) fn set(_services: &mut crate::services::LinuxServices, spec: TraySpec) {
    let tray = PrismTray {
        id: format!("prism-{}", spec.id),
        title: spec.title.clone().unwrap_or_default(),
        tooltip: spec
            .tooltip
            .clone()
            .or(spec.title.clone())
            .unwrap_or_default(),
        icon: icon_name(spec.icon.as_deref()),
        items: items(&spec.menu),
    };
    TRAYS.with(|t| {
        let mut trays = t.borrow_mut();
        if let Some(handle) = trays.get(&spec.id) {
            handle.update(move |old| *old = tray);
            return;
        }
        match tray.spawn() {
            Ok(handle) => {
                trays.insert(spec.id.clone(), handle);
            }
            // No StatusNotifierWatcher (no panel supporting trays).
            Err(e) => eprintln!("prism-linux: tray {:?} not shown: {e}", spec.id),
        }
    });
}

pub(crate) fn remove(id: &str) {
    TRAYS.with(|t| {
        if let Some(handle) = t.borrow_mut().remove(id) {
            let _ = handle.shutdown();
        }
    });
}
