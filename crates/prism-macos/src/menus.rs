//! AppKit menus and alerts from Prism's platform-neutral specs.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSApplication, NSControlStateValueOff, NSControlStateValueOn,
    NSEventModifierFlags, NSImage, NSMenu, NSMenuItem, NSWindow,
};
use objc2_foundation::NSString;
use prism_core::UiEvent;
use prism_core::menu::{ActionRole, AlertSpec, MenuItemSpec, MenuSpec, SystemAction};

/// `"cmd+shift+n"` → ("n", ⌘⇧).
pub fn parse_shortcut(s: &str) -> (String, NSEventModifierFlags) {
    let mut mask = NSEventModifierFlags::empty();
    let mut key = String::new();
    for part in s.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command" | "meta" | "super" => mask |= NSEventModifierFlags::Command,
            "shift" => mask |= NSEventModifierFlags::Shift,
            "alt" | "option" | "opt" => mask |= NSEventModifierFlags::Option,
            "ctrl" | "control" => mask |= NSEventModifierFlags::Control,
            other => key = other.to_string(),
        }
    }
    (key, mask)
}

fn item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    }
}

/// Builds an `NSMenu`; chosen actions call `onMenuItem:` on `target`.
pub fn build_menu(mtm: MainThreadMarker, spec: &MenuSpec, target: &AnyObject) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setTitle(&NSString::from_str(&spec.title));
    menu.setAutoenablesItems(false);
    for it in &spec.items {
        match it {
            MenuItemSpec::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
            MenuItemSpec::System {
                action,
                title,
                shortcut,
            } => menu.addItem(&system_item(
                mtm,
                *action,
                title,
                shortcut.as_deref(),
                target,
            )),
            MenuItemSpec::Submenu {
                title,
                icon,
                menu: sub,
            } => {
                let entry = item(mtm, title, None, "");
                entry.setImage(symbol(icon).as_deref());
                entry.setSubmenu(Some(&build_menu(mtm, sub, target)));
                menu.addItem(&entry);
            }
            MenuItemSpec::Action {
                id,
                title,
                icon,
                shortcut,
                role,
                checked,
                enabled,
            } => {
                let (key, mask) = shortcut
                    .as_deref()
                    .map(parse_shortcut)
                    .unwrap_or((String::new(), NSEventModifierFlags::empty()));
                let entry = item(mtm, title, Some(sel!(onMenuItem:)), &key);
                if !key.is_empty() {
                    entry.setKeyEquivalentModifierMask(mask);
                }
                entry.setTag(*id as isize);
                unsafe { entry.setTarget(Some(target)) };
                entry.setEnabled(*enabled);
                entry.setImage(symbol(icon).as_deref());
                if let Some(on) = checked {
                    entry.setState(if *on {
                        NSControlStateValueOn
                    } else {
                        NSControlStateValueOff
                    });
                }
                if *role == ActionRole::Destructive {
                    // macOS has no red menu items; the title conveys it.
                    entry.setToolTip(Some(&NSString::from_str("Destructive action")));
                }
                menu.addItem(&entry);
            }
        }
    }
    menu
}

/// A system item (#161): the action AppKit sends along the responder chain
/// (the focused field, the window), like its own menus' items; Quit goes to
/// Prism, which asks first with unsaved changes.
fn system_item(
    mtm: MainThreadMarker,
    action: SystemAction,
    title: &str,
    shortcut: Option<&str>,
    target: &AnyObject,
) -> Retained<NSMenuItem> {
    let selector = match action {
        SystemAction::Undo => sel!(undo:),
        SystemAction::Redo => sel!(redo:),
        SystemAction::Cut => sel!(cut:),
        SystemAction::Copy => sel!(copy:),
        SystemAction::Paste => sel!(paste:),
        SystemAction::PasteAndMatchStyle => sel!(pasteAsPlainText:),
        SystemAction::Delete => sel!(delete:),
        SystemAction::SelectAll => sel!(selectAll:),
        SystemAction::Minimize => sel!(performMiniaturize:),
        SystemAction::Zoom => sel!(performZoom:),
        SystemAction::ToggleFullScreen => sel!(toggleFullScreen:),
        SystemAction::CloseWindow => sel!(performClose:),
        SystemAction::Hide => sel!(hide:),
        SystemAction::HideOthers => sel!(hideOtherApplications:),
        SystemAction::ShowAll => sel!(unhideAllApplications:),
        SystemAction::About => sel!(orderFrontStandardAboutPanel:),
        SystemAction::Quit => sel!(prismQuit:),
        SystemAction::Services => {
            // A submenu AppKit fills.
            let entry = item(mtm, title, None, "");
            let services = NSMenu::new(mtm);
            services.setTitle(&NSString::from_str(title));
            entry.setSubmenu(Some(&services));
            NSApplication::sharedApplication(mtm).setServicesMenu(Some(&services));
            return entry;
        }
    };
    let (key, mask) = shortcut
        .map(parse_shortcut)
        .unwrap_or((String::new(), NSEventModifierFlags::empty()));
    let entry = item(mtm, title, Some(selector), &key);
    if !key.is_empty() {
        entry.setKeyEquivalentModifierMask(mask);
    }
    if action == SystemAction::Quit {
        unsafe { entry.setTarget(Some(target)) };
    }
    entry
}

fn symbol(name: &Option<String>) -> Option<Retained<NSImage>> {
    name.as_deref().and_then(|n| {
        NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(n), None)
    })
}

/// The main menu: app menu, the app's own menus, then Edit and Go.
pub fn install_main_menu(
    mtm: MainThreadMarker,
    title: &str,
    target: &AnyObject,
    app_menus: &[MenuSpec],
) {
    let bar = NSMenu::new(mtm);
    let add = |menu: Retained<NSMenu>| {
        let holder = NSMenuItem::new(mtm);
        holder.setSubmenu(Some(&menu));
        bar.addItem(&holder);
    };
    let app_menu = NSMenu::new(mtm);
    app_menu.addItem(&item(mtm, &format!("Hide {title}"), Some(sel!(hide:)), "h"));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    // Quitting asks first while the app has unsaved changes (#158).
    let quit = item(mtm, &format!("Quit {title}"), Some(sel!(prismQuit:)), "q");
    unsafe { quit.setTarget(Some(target)) };
    app_menu.addItem(&quit);
    add(app_menu);
    for spec in app_menus {
        add(build_menu(mtm, spec, target));
    }
    let edit = NSMenu::new(mtm);
    edit.setTitle(&NSString::from_str("Edit"));
    for (t, a, k) in [
        ("Undo", sel!(undo:), "z"),
        // ⇧⌘Z: a capital Z means Shift (#161).
        ("Redo", sel!(redo:), "Z"),
        ("Cut", sel!(cut:), "x"),
        ("Copy", sel!(copy:), "c"),
        ("Paste", sel!(paste:), "v"),
        ("Select All", sel!(selectAll:), "a"),
    ] {
        edit.addItem(&item(mtm, t, Some(a), k));
    }
    add(edit);
    let go = NSMenu::new(mtm);
    go.setTitle(&NSString::from_str("Go"));
    let back = item(mtm, "Back", Some(sel!(onBackMenu:)), "[");
    unsafe { back.setTarget(Some(target)) };
    go.addItem(&back);
    add(go);
    NSApplication::sharedApplication(mtm).setMainMenu(Some(&bar));
}

/// The whole menu bar, defined by the app (#161): its menus only, the first
/// being the application menu (titled with the app's name by macOS). A menu
/// holding Minimize is the Window menu (AppKit lists the windows there).
pub fn install_menu_bar(mtm: MainThreadMarker, target: &AnyObject, menus: &[MenuSpec]) {
    let bar = NSMenu::new(mtm);
    let app = NSApplication::sharedApplication(mtm);
    for spec in menus {
        let menu = build_menu(mtm, spec, target);
        // System items follow the focus (Copy is enabled in a field with a
        // selection); app items keep the state the app gave them.
        menu.setAutoenablesItems(true);
        let windows = spec.items.iter().any(|i| {
            matches!(
                i,
                MenuItemSpec::System {
                    action: SystemAction::Minimize,
                    ..
                }
            )
        });
        if windows {
            app.setWindowsMenu(Some(&menu));
        }
        let holder = NSMenuItem::new(mtm);
        holder.setSubmenu(Some(&menu));
        bar.addItem(&holder);
    }
    app.setMainMenu(Some(&bar));
}

/// Shows an alert as a sheet on `window`; reports the chosen action index.
pub fn show_alert(mtm: MainThreadMarker, window: &NSWindow, spec: &AlertSpec, report: fn(UiEvent)) {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&spec.title));
    if let Some(m) = &spec.message {
        alert.setInformativeText(&NSString::from_str(m));
    }
    // AppKit order: the first button is the default (rightmost); Cancel last.
    let mut order: Vec<usize> = (0..spec.actions.len())
        .filter(|i| spec.actions[*i].1 != ActionRole::Cancel)
        .collect();
    order.extend((0..spec.actions.len()).filter(|i| spec.actions[*i].1 == ActionRole::Cancel));
    let mut destructive = false;
    for i in &order {
        let (title, role) = &spec.actions[*i];
        let button = alert.addButtonWithTitle(&NSString::from_str(title));
        match role {
            ActionRole::Destructive => {
                button.setHasDestructiveAction(true);
                destructive = true;
            }
            ActionRole::Cancel => button.setKeyEquivalent(&NSString::from_str("\u{1b}")),
            ActionRole::Normal => {}
        }
    }
    if destructive {
        alert.setAlertStyle(NSAlertStyle::Warning);
    }
    let id = spec.id;
    let handler = block2::RcBlock::new(move |response: isize| {
        // NSAlertFirstButtonReturn = 1000.
        let n = (response - 1000).max(0) as usize;
        if let Some(index) = order.get(n) {
            report(UiEvent::AlertAction(id, *index));
        }
    });
    alert.beginSheetModalForWindow_completionHandler(window, Some(&handler));
}
