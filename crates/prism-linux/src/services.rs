//! Linux system surfaces and device APIs, through freedesktop standards:
//!
//! | | |
//! |---|---|
//! | location | GeoClue2 (system D-Bus: `CreateClient`, `LocationUpdated`) |
//! | photos | the desktop's file chooser (`GtkFileChooserNative`, portal-aware) |
//! | clipboard | the display's clipboard |
//! | links | the default app for the URI |
//! | notifications | `GNotification` (freedesktop notifications or the portal), with buttons |
//! | alerts | a modal window |
//! | context menus, menu bar | `GMenu` models with app actions (`app.m<id>`) |
//! | background tasks | run while the app is open |
//! | secrets, fingerprint, sign-in | Secret Service, fprintd, loopback redirect (`security.rs`) |
//!
//! Motion, camera, haptics and the share sheet have no desktop standard:
//! capabilities say so and calls fail with "unavailable".

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use prism_core::menu::{ActionRole, AlertSpec, MenuItemSpec, MenuSpec};
use prism_core::surfaces::PlatformRequest;
use prism_core::{
    Accuracy, Capabilities, DeviceCall, DeviceError, DeviceEvent, DeviceValue, Permission,
    PermissionState, PickedFile, PlatformServices, UiEvent,
};

use crate::imp;
use crate::security::SecretOp;

/// Prefix of secrets that need a fingerprint first (kept in the keyring).
const GATED: &str = "\u{1}prism-gated\u{1}";

const GEOCLUE: &str = "org.freedesktop.GeoClue2";

pub(crate) struct LinuxServices {
    pub app: gtk::Application,
    pub window: gtk::ApplicationWindow,
    caps: Capabilities,
    /// GeoClue clients of running location streams (call id → client path).
    location_streams: Rc<RefCell<HashMap<u64, String>>>,
    /// Delayed notifications not sent yet.
    pending: HashMap<String, glib::SourceId>,
    /// Background tasks: id → (next run, interval).
    pub background: Vec<(String, std::time::Instant, std::time::Duration)>,
    /// Sign-ins waiting for their loopback redirect.
    loopback: crate::security::Loopback,
    /// The app's data directory (push keys).
    data_dir: std::path::PathBuf,
}

fn reply(id: u64, result: Result<DeviceValue, DeviceError>) {
    glib::idle_add_local_once(move || imp::device_reply(id, result));
}

fn event(id: u64, e: DeviceEvent) {
    glib::idle_add_local_once(move || imp::device_event(id, e));
}

fn system_bus() -> Option<gio::DBusConnection> {
    gio::bus_get_sync(gio::BusType::System, None::<&gio::Cancellable>).ok()
}

impl LinuxServices {
    pub fn new(
        app: gtk::Application,
        window: gtk::ApplicationWindow,
        data_dir: std::path::PathBuf,
    ) -> Self {
        // GNotification needs the session bus; location needs GeoClue on the
        // system bus.
        let session = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).is_ok();
        let geoclue = system_bus().is_some_and(|bus| {
            bus.call_sync(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "ListActivatableNames",
                None,
                Some(glib::VariantTy::new("(as)").expect("type")),
                gio::DBusCallFlags::NONE,
                -1,
                None::<&gio::Cancellable>,
            )
            .ok()
            .and_then(|v| v.child_value(0).get::<Vec<String>>())
            .is_some_and(|names| names.iter().any(|n| n == GEOCLUE))
        });
        let (secrets, biometry) = crate::security::capabilities();
        LinuxServices {
            caps: Capabilities {
                car: false,
                notifications: session,
                app_menu: true,
                tray: session,
                location: geoclue,
                photos: true,
                clipboard: true,
                secure_storage: secrets,
                biometry,
                // fprintd only: no desktop standard checks a password for an app.
                device_auth: biometry.is_biometric(),
                web_auth: true,
                // UnifiedPush over D-Bus, when a distributor runs (push.rs).
                push: crate::push::distributor().map(|_| prism_core::PushService::UnifiedPush),
                // GtkDropTarget (#111).
                drop_files: true,
                ..Default::default()
            },
            data_dir,
            app,
            window,
            location_streams: Rc::new(RefCell::new(HashMap::new())),
            pending: HashMap::new(),
            background: Vec::new(),
            loopback: Default::default(),
        }
    }

    // --- Device ------------------------------------------------------------------------

    fn device(&mut self, id: u64, call: DeviceCall) {
        match call {
            // GeoClue authorizes when a client starts (its agent asks the
            // user): asking means starting one.
            DeviceCall::RequestPermission(Permission::Location | Permission::LocationAlways)
                if self.caps.location =>
            {
                let desktop_id = self.desktop_id();
                glib::MainContext::default().spawn_local(async move {
                    let state = match geoclue::start_client(&desktop_id, 4).await {
                        Ok((bus, path)) => {
                            geoclue::stop(&bus, &path).await;
                            PermissionState::Granted
                        }
                        Err("denied") => PermissionState::Denied,
                        Err(_) => PermissionState::Unavailable,
                    };
                    reply(id, Ok(DeviceValue::Permission(state)));
                });
            }
            DeviceCall::PermissionStatus(p) | DeviceCall::RequestPermission(p) => {
                let state = match p {
                    Permission::Location | Permission::LocationAlways if self.caps.location => {
                        // GeoClue's agent asks the user when a client starts.
                        PermissionState::NotDetermined
                    }
                    Permission::Notifications if self.caps.notifications => {
                        PermissionState::Granted
                    }
                    // The file chooser needs no permission.
                    Permission::Photos => PermissionState::Granted,
                    _ => PermissionState::Unavailable,
                };
                reply(id, Ok(DeviceValue::Permission(state)));
            }
            DeviceCall::CurrentLocation(a) => self.locate(id, a, false),
            DeviceCall::WatchLocation(a) => self.locate(id, a, true),
            DeviceCall::StopWatch => {
                if let Some(path) = self.location_streams.borrow_mut().remove(&id)
                    && let Some(bus) = system_bus()
                {
                    glib::MainContext::default()
                        .spawn_local(async move { geoclue::stop(&bus, &path).await });
                }
            }
            DeviceCall::PickImages(o) => {
                if o.camera {
                    return reply(id, Err(DeviceError::from_message("unavailable")));
                }
                self.pick_images(id, o.limit);
            }
            DeviceCall::PickFiles(o) => self.pick_files(id, o),
            DeviceCall::ReadClipboard => {
                let clipboard = self.window.clipboard();
                clipboard.read_text_async(None::<&gio::Cancellable>, move |r| {
                    let text = r.ok().flatten().map(|t| t.to_string());
                    reply(id, Ok(DeviceValue::Text(text)));
                });
            }
            DeviceCall::CopyText(t) => {
                self.window.clipboard().set_text(&t);
                reply(id, Ok(DeviceValue::Done));
            }
            // No tabs here: the default handler opens it either way.
            DeviceCall::OpenUrl(url) | DeviceCall::OpenUrlIn(url, _) => {
                let ok = gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>);
                reply(
                    id,
                    ok.map(|_| DeviceValue::Done)
                        .map_err(|e| DeviceError::from_message(&e.to_string())),
                );
            }
            DeviceCall::WatchMotion(_) => event(
                id,
                DeviceEvent::Error(DeviceError::from_message("unavailable")),
            ),
            // No platform passkeys on the Linux desktop (#94).
            DeviceCall::Haptic(_)
            | DeviceCall::Share(_)
            | DeviceCall::SignInWithApple(_)
            | DeviceCall::WebAuthn(_) => reply(id, Err(DeviceError::from_message("unavailable"))),
            DeviceCall::SecureGet { key, prompt, .. } => {
                self.secret(id, key, SecretOp::Get, prompt)
            }
            DeviceCall::SecureSet {
                key,
                value,
                options,
            } => {
                // The keyring has no per-item owner check: gate it with fprintd.
                if options.biometric && !self.caps.device_auth {
                    return reply(id, Err(DeviceError::Unavailable));
                }
                let label = self
                    .window
                    .title()
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                let value = format!("{}{value}", if options.biometric { GATED } else { "" });
                self.secret(id, key, SecretOp::Set { label, value }, None)
            }
            DeviceCall::SecureDelete { key, .. } => self.secret(id, key, SecretOp::Delete, None),
            DeviceCall::BiometryStatus => {
                crate::security::biometry(move |r| imp::device_reply(id, r))
            }
            DeviceCall::Authenticate(r) => {
                crate::security::authenticate(&self.window, r.reason, move |r| {
                    imp::device_reply(id, r)
                })
            }
            DeviceCall::AuthRedirect(_) => reply(id, self.loopback.redirect()),
            DeviceCall::WebAuth(r) => self
                .loopback
                .start(&r.url, &r.redirect, move |r| imp::device_reply(id, r)),
            // No cookies here (secure storage keeps secrets), no live session.
            DeviceCall::SetCookie(_) => reply(id, Err(DeviceError::Unavailable)),
            DeviceCall::Reconnect => {}
        }
    }

    /// A Secret Service operation; biometric items are verified with
    /// fprintd before their value is returned.
    fn secret(&mut self, id: u64, key: String, op: SecretOp, prompt: Option<String>) {
        if !self.caps.secure_storage {
            return reply(id, Err(DeviceError::Unavailable));
        }
        let window = self.window.clone();
        let get = matches!(op, SecretOp::Get);
        crate::security::secret(self.desktop_id(), key, op, move |r| match r {
            Ok(DeviceValue::Text(Some(v))) if get && v.starts_with(GATED) => {
                let value = v[GATED.len()..].to_string();
                let reason = prompt
                    .or_else(|| window.title().map(|t| t.to_string()))
                    .unwrap_or_default();
                crate::security::authenticate(&window, reason, move |r| {
                    imp::device_reply(id, r.map(|_| DeviceValue::Text(Some(value))))
                });
            }
            r => imp::device_reply(id, r),
        });
    }

    fn desktop_id(&self) -> String {
        self.app
            .application_id()
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    /// GeoClue2: a client per call; `stream` keeps it running. Everything
    /// is asynchronous: GeoClue may take seconds to answer.
    fn locate(&mut self, id: u64, accuracy: Accuracy, stream: bool) {
        let fail = move |m: &str| {
            if stream {
                event(id, DeviceEvent::Error(DeviceError::from_message(m)));
            } else {
                reply(id, Err(DeviceError::from_message(m)));
            }
        };
        if !self.caps.location {
            return fail("unavailable");
        }
        // GClueAccuracyLevel (gclue-enums.h): CITY 4, STREET 6, EXACT 8.
        let level: u32 = match accuracy {
            Accuracy::Coarse => 4,
            Accuracy::Balanced => 6,
            Accuracy::Best => 8,
        };
        let desktop_id = self.desktop_id();
        let streams = self.location_streams.clone();
        glib::MainContext::default().spawn_local(async move {
            let (bus, path) = match geoclue::create_client(&desktop_id, level).await {
                Ok(c) => c,
                Err(e) => return fail(e),
            };
            geoclue::on_location(&bus, &path, move |bus, client, location| {
                if stream {
                    event(id, DeviceEvent::Location(location));
                    true
                } else {
                    reply(id, Ok(DeviceValue::Location(location)));
                    let (bus, client) = (bus.clone(), client.to_string());
                    glib::MainContext::default()
                        .spawn_local(async move { geoclue::stop(&bus, &client).await });
                    false
                }
            });
            if stream {
                streams.borrow_mut().insert(id, path.clone());
            }
            if let Err(e) = geoclue::start(&bus, &path).await {
                streams.borrow_mut().remove(&id);
                fail(e);
            }
        });
    }

    /// The desktop's file chooser, images only.
    fn pick_images(&mut self, id: u64, limit: u32) {
        let chooser = gtk::FileChooserNative::new(
            None,
            Some(&self.window),
            gtk::FileChooserAction::Open,
            None,
            None,
        );
        chooser.set_select_multiple(limit != 1);
        let filter = gtk::FileFilter::new();
        filter.add_mime_type("image/*");
        chooser.add_filter(&filter);
        let keep = chooser.clone();
        chooser.connect_response(move |c, response| {
            let _ = &keep;
            if response != gtk::ResponseType::Accept {
                return reply(id, Err(DeviceError::from_message("cancelled")));
            }
            let files = c.files();
            let mut picked = Vec::new();
            for i in 0..files.n_items() {
                if limit > 0 && picked.len() as u32 >= limit {
                    break;
                }
                let Some(file) = files.item(i).and_downcast::<gio::File>() else {
                    continue;
                };
                if let Some(p) = copy_picked(&file) {
                    picked.push(p);
                }
            }
            reply(id, Ok(DeviceValue::Files(picked)));
        });
        chooser.show();
    }

    /// The desktop's file chooser (portal-aware `GtkFileChooserNative`:
    /// `GtkFileDialog` needs GTK 4.10), filtered by MIME types and
    /// extensions.
    fn pick_files(&mut self, id: u64, o: prism_core::PickFiles) {
        let chooser = gtk::FileChooserNative::new(
            None,
            Some(&self.window),
            gtk::FileChooserAction::Open,
            None,
            None,
        );
        chooser.set_select_multiple(o.multiple);
        if !o.types.is_empty() {
            let filter = gtk::FileFilter::new();
            for t in &o.types {
                match t.trim().strip_prefix('.') {
                    Some(ext) => filter.add_suffix(ext),
                    None => filter.add_mime_type(t.trim()),
                }
            }
            chooser.add_filter(&filter);
        }
        let keep = chooser.clone();
        chooser.connect_response(move |c, response| {
            let _ = &keep;
            if response != gtk::ResponseType::Accept {
                return reply(id, Err(DeviceError::from_message("cancelled")));
            }
            let files = c.files();
            let mut picked = Vec::new();
            for i in 0..files.n_items() {
                if !o.multiple && !picked.is_empty() {
                    break;
                }
                let Some(file) = files.item(i).and_downcast::<gio::File>() else {
                    continue;
                };
                if let Some(p) = copy_picked(&file) {
                    picked.push(p);
                }
            }
            reply(id, Ok(DeviceValue::Files(picked)));
        });
        chooser.show();
    }

    // --- Notifications ------------------------------------------------------------------

    fn notify(&mut self, n: prism_core::surfaces::Notification) {
        if let Some(timer) = self.pending.remove(&n.id) {
            timer.remove();
        }
        let notification = gio::Notification::new(&n.title);
        let body = [n.subtitle.clone(), Some(n.body.clone())]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if !body.is_empty() {
            notification.set_body(Some(&body));
        }
        let target = |action: &str, link: Option<&String>| {
            (n.id.as_str(), action, link.map_or("", String::as_str)).to_variant()
        };
        notification.set_default_action_and_target_value(
            "app.prism-notification",
            Some(&target("", n.link.as_ref())),
        );
        for a in &n.actions {
            // GNotification has no text field: replies send no text.
            notification.add_button_with_target_value(
                &a.title,
                "app.prism-notification",
                Some(&target(&a.id, a.link.as_ref())),
            );
        }
        let app = self.app.clone();
        let id = n.id.clone();
        match n.delay {
            Some(delay) if !delay.is_zero() => {
                let pending_id = id.clone();
                let timer = glib::timeout_add_local_once(delay, move || {
                    app.send_notification(Some(&pending_id), &notification)
                });
                self.pending.insert(id, timer);
            }
            _ => app.send_notification(Some(&id), &notification),
        }
    }
}

// --- Menus --------------------------------------------------------------------------

/// A `GMenu` for a Prism menu; items activate app actions `app.m<id>`.
pub(crate) fn menu_model(app: &gtk::Application, spec: &MenuSpec) -> gio::Menu {
    let menu = gio::Menu::new();
    let mut section = gio::Menu::new();
    for item in &spec.items {
        match item {
            MenuItemSpec::Action {
                id,
                title,
                shortcut,
                role,
                checked,
                enabled,
                ..
            } => {
                let name = format!("m{id}");
                action(app, &name, *id, *enabled);
                if let Some(keys) = shortcut {
                    app.set_accels_for_action(&format!("app.{name}"), &[&accelerator(keys)]);
                }
                let label = match (checked, role) {
                    (Some(true), _) => format!("✓ {title}"),
                    _ => title.clone(),
                };
                section.append(Some(&label), Some(&format!("app.{name}")));
            }
            MenuItemSpec::Separator => {
                menu.append_section(None, &section);
                section = gio::Menu::new();
            }
            // System actions (#161) are AppKit's: GTK fields have their own.
            MenuItemSpec::System { .. } => {}
            MenuItemSpec::Submenu {
                title, menu: sub, ..
            } => {
                let model = menu_model(app, sub);
                section.append_submenu(Some(title), &model);
            }
        }
    }
    menu.append_section(None, &section);
    menu
}

fn action(app: &gtk::Application, name: &str, id: u64, enabled: bool) {
    if let Some(existing) = app.lookup_action(name).and_downcast::<gio::SimpleAction>() {
        existing.set_enabled(enabled);
        return;
    }
    let action = gio::SimpleAction::new(name, None);
    action.set_enabled(enabled);
    action.connect_activate(move |_, _| imp::dispatch(UiEvent::MenuSelect(id)));
    app.add_action(&action);
}

impl LinuxServices {
    fn alert(&self, spec: AlertSpec) {
        let dialog = gtk::Window::builder()
            .transient_for(&self.window)
            .modal(true)
            .resizable(false)
            .default_width(360)
            .build();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
        body.set_margin_top(20);
        body.set_margin_bottom(20);
        body.set_margin_start(20);
        body.set_margin_end(20);
        let title = gtk::Label::new(Some(&spec.title));
        title.add_css_class("title-3");
        title.set_wrap(true);
        body.append(&title);
        if let Some(m) = &spec.message {
            let message = gtk::Label::new(Some(m));
            message.set_wrap(true);
            body.append(&message);
        }
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        for (index, (label, role)) in spec.actions.iter().enumerate() {
            let b = gtk::Button::with_label(label);
            match role {
                ActionRole::Destructive => b.add_css_class("destructive-action"),
                ActionRole::Cancel => {}
                _ => b.add_css_class("suggested-action"),
            }
            let (dialog, id) = (dialog.clone(), spec.id);
            b.connect_clicked(move |_| {
                dialog.close();
                imp::dispatch(UiEvent::AlertAction(id, index));
            });
            buttons.append(&b);
        }
        body.append(&buttons);
        dialog.set_child(Some(&body));
        dialog.present();
    }

    /// Background tasks due now (removed until they reschedule).
    pub fn due_background(&mut self) -> Vec<String> {
        let now = std::time::Instant::now();
        let (due, later): (Vec<_>, Vec<_>) =
            self.background.drain(..).partition(|(_, at, _)| *at <= now);
        self.background = later;
        due.into_iter().map(|(id, _, _)| id).collect()
    }
}

impl PlatformServices for LinuxServices {
    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    fn handle(&mut self, request: PlatformRequest) {
        match request {
            PlatformRequest::Device { id, call } => self.device(id, call),
            PlatformRequest::Notify(n) => {
                if self.caps.notifications {
                    self.notify(n)
                }
            }
            PlatformRequest::CancelNotification(id) => {
                if let Some(timer) = self.pending.remove(&id) {
                    timer.remove();
                }
                self.app.withdraw_notification(&id);
            }
            PlatformRequest::RequestNotificationPermission => {}
            PlatformRequest::RegisterPush => crate::push::register(
                self.data_dir.clone(),
                |r| {
                    glib::idle_add_local_once(move || imp::push_registered(r));
                },
                |m| {
                    glib::idle_add_local_once(move || imp::push_received(m));
                },
            ),
            PlatformRequest::UnregisterPush => crate::push::unregister(&self.data_dir),
            // No Live Activities on Linux desktops.
            PlatformRequest::WatchActivityPushToStart => {}
            PlatformRequest::ShowAlert(spec) => self.alert(spec),
            // The app's whole bar (#161): GTK has no menus of its own.
            PlatformRequest::SetAppMenu(menus) | PlatformRequest::SetMenuBar(menus) => {
                let bar = gio::Menu::new();
                for m in &menus {
                    let model = menu_model(&self.app, m);
                    bar.append_submenu(Some(&m.title), &model);
                }
                self.app.set_menubar(Some(&bar));
                self.window.set_show_menubar(true);
            }
            PlatformRequest::ScheduleBackgroundTask(task) => {
                let every = imp::background_interval(task.every);
                self.background.retain(|(id, _, _)| *id != task.id);
                self.background
                    .push((task.id, std::time::Instant::now() + every, every));
            }
            PlatformRequest::SetTray(spec) => crate::tray::set(self, spec),
            PlatformRequest::RemoveTray(id) => crate::tray::remove(&id),
            // No car screens on Linux.
            PlatformRequest::SetCar(_) => {}
            // Fonts are registered by the backend, which draws texts again
            // (imp.rs, `load_font`); stylesheets are a web thing.
            PlatformRequest::LoadFont(_)
            | PlatformRequest::AddStylesheet(_)
            | PlatformRequest::RemoveStylesheet(_)
            // The window's title and its close question are the backend's
            // (imp.rs), which has the window.
            | PlatformRequest::SetTitle(_)
            | PlatformRequest::SetUnsavedChanges(_) => {}
            // No widgets or Live Activities on Linux desktops.
            PlatformRequest::ReloadWidgets(_)
            | PlatformRequest::StartActivity { .. }
            | PlatformRequest::UpdateActivity { .. }
            | PlatformRequest::EndActivity { .. } => {}
        }
    }
}

/// `cmd+shift+n` → `<Control><Shift>n` (Command maps to Control on Linux).
fn accelerator(keys: &str) -> String {
    let mut out = String::new();
    let mut key = String::new();
    for part in keys.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command" | "ctrl" | "control" | "meta" => out += "<Control>",
            "shift" => out += "<Shift>",
            "alt" | "option" | "opt" => out += "<Alt>",
            "super" => out += "<Super>",
            other => key = other.to_string(),
        }
    }
    // Single characters by their GDK key name (`,` → `comma`).
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        use gtk::glib::translate::FromGlib;
        // SAFETY: plain conversion of a code point to a key value.
        let keyval =
            unsafe { gtk::gdk::Key::from_glib(gtk::gdk::ffi::gdk_unicode_to_keyval(c as u32)) };
        if let Some(name) = keyval.name() {
            key = name.to_string();
        }
    }
    out + &key
}

/// GeoClue2 over the system bus (org.freedesktop.GeoClue2.*.xml).
mod geoclue {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gtk::prelude::*;
    use gtk::{gio, glib};
    use prism_core::Location;

    use super::GEOCLUE;

    /// GeoClue may wait on its agent or location sources.
    const TIMEOUT_MS: i32 = 10_000;

    type Handler = Rc<dyn Fn(&gio::DBusConnection, &str, Location) -> bool>;

    async fn call(
        bus: &gio::DBusConnection,
        path: &str,
        interface: &str,
        method: &str,
        args: Option<glib::Variant>,
        reply: Option<&str>,
    ) -> Result<glib::Variant, glib::Error> {
        bus.call_future(
            Some(GEOCLUE),
            path,
            interface,
            method,
            args.as_ref(),
            reply.map(|r| glib::VariantTy::new(r).expect("type")),
            gio::DBusCallFlags::NONE,
            TIMEOUT_MS,
        )
        .await
    }

    fn error(e: glib::Error) -> &'static str {
        eprintln!("prism-linux: GeoClue: {e}");
        if e.to_string().contains("AccessDenied") {
            "denied"
        } else {
            "unavailable"
        }
    }

    /// A client for this app at `level` (GClueAccuracyLevel), not started.
    pub async fn create_client(
        desktop_id: &str,
        level: u32,
    ) -> Result<(gio::DBusConnection, String), &'static str> {
        let bus = gio::bus_get_future(gio::BusType::System)
            .await
            .map_err(|_| "unavailable")?;
        let created = call(
            &bus,
            "/org/freedesktop/GeoClue2/Manager",
            "org.freedesktop.GeoClue2.Manager",
            "CreateClient",
            None,
            Some("(o)"),
        )
        .await
        .map_err(error)?;
        let path = created
            .child_value(0)
            .str()
            .map(str::to_string)
            .ok_or("unavailable")?;
        for (name, value) in [
            ("DesktopId", desktop_id.to_variant()),
            ("RequestedAccuracyLevel", level.to_variant()),
        ] {
            call(
                &bus,
                &path,
                "org.freedesktop.DBus.Properties",
                "Set",
                Some(("org.freedesktop.GeoClue2.Client", name, value).to_variant()),
                None,
            )
            .await
            .map_err(error)?;
        }
        Ok((bus, path))
    }

    /// Creates and starts a client (GeoClue asks its agent).
    pub async fn start_client(
        desktop_id: &str,
        level: u32,
    ) -> Result<(gio::DBusConnection, String), &'static str> {
        let (bus, path) = create_client(desktop_id, level).await?;
        start(&bus, &path).await?;
        Ok((bus, path))
    }

    pub async fn start(bus: &gio::DBusConnection, path: &str) -> Result<(), &'static str> {
        call(
            bus,
            path,
            "org.freedesktop.GeoClue2.Client",
            "Start",
            None,
            None,
        )
        .await
        .map(|_| ())
        .map_err(error)
    }

    pub async fn stop(bus: &gio::DBusConnection, path: &str) {
        let _ = call(
            bus,
            path,
            "org.freedesktop.GeoClue2.Client",
            "Stop",
            None,
            None,
        )
        .await;
    }

    /// Calls `f` with each new location until it returns `false`.
    // `signal_subscribe` is the GLib 2.74 API (the image's GLib).
    #[allow(deprecated)]
    pub fn on_location(
        bus: &gio::DBusConnection,
        path: &str,
        f: impl Fn(&gio::DBusConnection, &str, Location) -> bool + 'static,
    ) {
        let f: Handler = Rc::new(f);
        let subscription: Rc<RefCell<Option<gio::SignalSubscriptionId>>> =
            Rc::new(RefCell::new(None));
        let sub = subscription.clone();
        let client = path.to_string();
        let handle = bus.signal_subscribe(
            Some(GEOCLUE),
            Some("org.freedesktop.GeoClue2.Client"),
            Some("LocationUpdated"),
            Some(path),
            None,
            gio::DBusSignalFlags::NONE,
            move |bus, _, _, _, _, params| {
                let Some(location_path) = params.child_value(1).str().map(str::to_string) else {
                    return;
                };
                let (bus, client, sub, f) = (bus.clone(), client.clone(), sub.clone(), f.clone());
                glib::MainContext::default().spawn_local(async move {
                    let Some(location) = read_location(&bus, &location_path).await else {
                        return;
                    };
                    if !f(&bus, &client, location)
                        && let Some(id) = sub.borrow_mut().take()
                    {
                        bus.signal_unsubscribe(id);
                    }
                });
            },
        );
        *subscription.borrow_mut() = Some(handle);
    }

    /// The properties of a `Location` object (unknown values per
    /// org.freedesktop.GeoClue2.Location.xml: altitude = -DBL_MAX, speed and
    /// heading = -1).
    pub async fn read_location(bus: &gio::DBusConnection, path: &str) -> Option<Location> {
        let all = call(
            bus,
            path,
            "org.freedesktop.DBus.Properties",
            "GetAll",
            Some(("org.freedesktop.GeoClue2.Location",).to_variant()),
            None,
        )
        .await
        .ok()?;
        let props = all.child_value(0);
        let dict = glib::VariantDict::new(Some(&props));
        let num = |k: &str| dict.lookup::<f64>(k).ok().flatten();
        let timestamp = dict
            .lookup_value("Timestamp", None)
            .and_then(|v| v.get::<(u64, u64)>())
            .map_or(0., |(s, us)| s as f64 + us as f64 / 1e6);
        Some(Location {
            latitude: num("Latitude")?,
            longitude: num("Longitude")?,
            altitude: num("Altitude").filter(|a| *a > f64::MIN),
            accuracy: num("Accuracy").unwrap_or(0.),
            speed: num("Speed").filter(|s| *s >= 0.),
            heading: num("Heading").filter(|h| *h >= 0.),
            timestamp,
        })
    }
}

/// Copies a picked file under the cache directory, like the other backends.
fn copy_picked(file: &gio::File) -> Option<PickedFile> {
    copy_picked_path(file.path()?)
}

/// [`copy_picked`] for a local path (any thread: dropped files are copied
/// off the main loop, #111).
pub(crate) fn copy_picked_path(src: std::path::PathBuf) -> Option<PickedFile> {
    let dir = glib::user_cache_dir().join("prism-picked");
    std::fs::create_dir_all(&dir).ok()?;
    let name = src.file_name()?.to_string_lossy().to_string();
    let ext = src
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    let unique = format!(
        "{}.{ext}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let dest = dir.join(unique);
    let size = std::fs::copy(&src, &dest).ok()?;
    let mime = gio::content_type_guess(Some(&src), None::<&[u8]>).0;
    let mime = gio::content_type_get_mime_type(&mime)
        .map_or("application/octet-stream".into(), |m| m.to_string());
    Some(PickedFile {
        path: dest.to_string_lossy().to_string(),
        name,
        mime,
        size,
    })
}
