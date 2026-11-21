//! Tray icons (`cx.set_tray`): the StatusNotifierItem protocol (KDE, GNOME
//! with the AppIndicator extension, most Linux panels), with the menu over
//! `com.canonical.dbusmenu`. Prism serves both itself, over GDBus, on a
//! thread of their own (item.rs). Menu items come back to the app as
//! `UiEvent::MenuSelect`; a click on the icon brings the window forward.

mod dbusmenu;
mod item;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use gtk::glib;
use gtk::prelude::*;
use prism_core::UiEvent;
use prism_core::menu::{MenuItemSpec, MenuSpec, TraySpec};

use dbusmenu::Item;
use item::{Action, Tray};

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

thread_local! {
    /// The window a click on the icon brings forward.
    static WINDOW: glib::WeakRef<gtk::ApplicationWindow> = glib::WeakRef::new();
}

/// Runs on the tray thread: the app hears it on GTK's.
fn act(action: Action) {
    glib::MainContext::default().invoke(move || match action {
        Action::Select(id) => crate::imp::dispatch(UiEvent::MenuSelect(id)),
        Action::Activate { token } => {
            let Some(window) = WINDOW.with(|w| w.upgrade()) else {
                return;
            };
            if let Some(token) = token {
                // Wayland: the panel's activation token lets it come forward.
                window.set_startup_id(&token);
            }
            window.present();
        }
    });
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

pub(crate) fn set(services: &mut crate::services::LinuxServices, spec: TraySpec) {
    WINDOW.with(|w| w.set(Some(&services.window)));
    let tray = Tray {
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
    item::set(spec.id, tray, Arc::new(act));
}

pub(crate) fn remove(id: &str) {
    item::remove(id.to_string());
}
