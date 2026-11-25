//! Windows' system services (`PlatformServices`):
//!
//! | | |
//! |---|---|
//! | files, folders, photos, saving | the shell's file dialogs (`pickers.rs`) |
//! | notifications | Windows App SDK app notifications (`notify.rs`, `toast.rs`) |
//! | badges | an overlay on the taskbar button (`taskbar.rs`) |
//! | secrets, Windows Hello, sign-in | Credential Manager, `UserConsentVerifier`, loopback redirect (`security.rs`) |
//! | tray | `Shell_NotifyIcon` and a popup menu (`tray.rs`) |
//! | menu bar | a WinUI `MenuBar` with accelerators (`menubar.rs`) |
//! | alerts | `ContentDialog` (`alert.rs`) |
//! | clipboard | text, HTML, RTF, pictures, files, custom types (`clipboard.rs`) |
//! | links, reveal in Explorer | `ShellExecuteW`, `SHOpenFolderAndSelectItems` |
//! | background tasks | timers while the app runs (like on Linux) |
//! | location | `Geolocator` (`location.rs`) |
//! | taking a photo | the Camera app's capture UI (`camera.rs`) |
//! | share sheet | the Share dialog for the window (`share.rs`) |
//! | passkeys | `webauthn.dll` (`webauthn.rs`, `passkey.rs`) |
//! | haptics | a vibration device, where the PC has one (`haptics.rs`) |
//! | motion | accelerometer, gyrometer, inclinometer, where the PC has them (`sensors.rs`) |
//!
//! What the PC has is asked in the background from startup (`probe.rs`),
//! never waited for: each capability turns true when its check answers
//! (the app re-renders), a call made before then tries the API itself,
//! and a call to what the PC lacks fails with "unavailable". Push is WNS
//! in an MSIX (`push.rs`), "unavailable" without package identity.
//!
//! Modal UI (dialogs, the tray's menu) starts from the services window
//! (`shell.rs`), never while the app's state is in use; answers come back
//! through `imp::defer`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use prism_core::surfaces::PlatformRequest;
use prism_core::{
    Capabilities, DeviceCall, DeviceError, DeviceEvent, DeviceValue, Permission, PermissionState,
    PickedFile, PlatformServices,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use crate::device::{Found, may};
use crate::pickers::{self, Pick};
use crate::security::{self, Answer, Loopback};
use crate::win::{log, wide};
use crate::{
    camera, clipboard, haptics, location, notify, sensors, shell, taskbar, tray, webauthn,
};

/// Prefix of secrets that need Windows Hello first.
const GATED: &str = "\u{1}prism-gated\u{1}";

pub struct WinServices {
    pub hwnd: HWND,
    caps: Capabilities,
    /// What the startup checks found so far (`probe.rs`).
    found: Found,
    /// The app's id (its data directory's name): names its secrets.
    app: String,
    /// Where picked files are copied.
    picked: PathBuf,
    /// Sign-ins waiting for their loopback redirect.
    loopback: Loopback,
    /// Background tasks: id → timer.
    background: HashMap<String, usize>,
    /// Requests logged once as unavailable.
    told: HashSet<&'static str>,
    /// File dialogs' filter names in the app's language (set by `imp`
    /// before each device call).
    pub filter_names: pickers::FilterNames,
}

/// Stores a secret that is not sealed: marked when reading it needs
/// Windows Hello (`can_authenticate`: Hello can verify the user).
fn store(
    app: &str,
    key: &str,
    value: &str,
    options: prism_core::security::SecureOptions,
    can_authenticate: bool,
) -> Answer {
    let gated = options.app_gate(can_authenticate)?;
    let value = format!("{}{value}", if gated { GATED } else { "" });
    security::secret_set(app, key, &value).map(|_| DeviceValue::Done)
}

/// Answers device call `id`.
fn reply(id: u64, result: Answer) {
    crate::imp::defer(Box::new(move |s| {
        let patches = s.runtime.device_reply(id, result);
        crate::imp::commit(s, patches);
    }));
}

fn event(id: u64, e: DeviceEvent) {
    crate::imp::defer(Box::new(move |s| {
        let patches = s.runtime.device_event(id, e);
        crate::imp::commit(s, patches);
    }));
}

/// `PRISM_BACKGROUND_INTERVAL=<seconds>` shortens background intervals
/// (development), as on Linux.
fn background_interval(every: std::time::Duration) -> std::time::Duration {
    std::env::var("PRISM_BACKGROUND_INTERVAL")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .map_or(every, std::time::Duration::from_secs_f64)
}

impl WinServices {
    pub fn new(hwnd: HWND, data: &std::path::Path) -> WinServices {
        shell::create();
        let notifications = notify::supported();
        notify::start(data);
        crate::push::start();
        // With `PRISM_DEVICE_REPORT` only: waits for every check.
        crate::probe::report(hwnd, data);
        // The checks' answers so far; the later ones are published.
        let found = crate::probe::open();
        let base = Capabilities {
            notifications,
            tray: true,
            app_menu: true,
            photos: true,
            clipboard: true,
            secure_storage: true,
            web_auth: true,
            // WebView2 (`web.rs`), Explorer drops and drags between
            // views (`pointer.rs`).
            web_view: true,
            drop_files: true,
            drag_and_drop: true,
            push: crate::push::service(),
            ..Default::default()
        };
        WinServices {
            hwnd,
            caps: found.apply(base),
            found,
            app: data
                .file_name()
                .map_or_else(|| "prism".into(), |n| n.to_string_lossy().into_owned()),
            picked: data.join("picked"),
            loopback: Loopback::default(),
            background: HashMap::new(),
            told: HashSet::new(),
            filter_names: pickers::FilterNames::default(),
        }
    }

    /// Takes what the startup checks found since; true when the
    /// capabilities changed (then given to the app: `probe.rs`).
    pub fn take_found(&mut self) -> bool {
        self.found = crate::probe::found();
        let caps = self.found.apply(self.caps);
        let changed = caps != self.caps;
        self.caps = caps;
        changed
    }

    fn unavailable(&mut self, what: &'static str, why: &str) {
        if self.told.insert(what) {
            log!("{what}: unavailable on Windows ({why})");
        }
    }

    fn device(&mut self, id: u64, call: DeviceCall) {
        match call {
            DeviceCall::RequestPermission(Permission::Location | Permission::LocationAlways)
                if may(self.found.location) =>
            {
                location::request(move |state| reply(id, Ok(DeviceValue::Permission(state))));
            }
            DeviceCall::PermissionStatus(p) | DeviceCall::RequestPermission(p) => {
                let request = matches!(call, DeviceCall::RequestPermission(_));
                let state = match p {
                    Permission::Notifications if self.caps.notifications => notify::permission(),
                    // The file dialog needs no permission.
                    Permission::Photos => PermissionState::Granted,
                    // Desktop apps have no background limit: one setting.
                    Permission::Location | Permission::LocationAlways
                        if may(self.found.location) =>
                    {
                        location::permission()
                    }
                    Permission::Camera if may(self.found.camera) => camera::permission(request),
                    // Windows' motion sensors ask no one.
                    Permission::Motion if may(self.found.motion) => PermissionState::Granted,
                    _ => PermissionState::Unavailable,
                };
                reply(id, Ok(DeviceValue::Permission(state)));
            }
            DeviceCall::PickImages(o) => {
                if !o.camera {
                    return self.pick(id, Pick::Images { limit: o.limit });
                }
                if !may(self.found.camera) {
                    self.unavailable("the camera", "no camera, or no Windows Camera app");
                    return reply(id, Err(DeviceError::Unavailable));
                }
                camera::take_photo(self.hwnd, self.picked.clone(), move |r| reply(id, r));
            }
            DeviceCall::PickFiles(o) => self.pick(
                id,
                Pick::Files {
                    types: o.types,
                    multiple: o.multiple,
                },
            ),
            DeviceCall::PickFolder(o) => self.pick(id, Pick::Folder { title: o.title }),
            DeviceCall::SaveFile(f) => self.save(id, f),
            DeviceCall::RevealPath(p) => reveal(&p),
            DeviceCall::CopyText(text) => reply(
                id,
                clipboard::write_text(self.hwnd, &text).map(|_| DeviceValue::Done),
            ),
            DeviceCall::WriteClipboard(item) => reply(
                id,
                clipboard::write_item(self.hwnd, &item).map(|_| DeviceValue::Done),
            ),
            DeviceCall::ReadClipboard => {
                reply(id, Ok(DeviceValue::Text(clipboard::read_text(self.hwnd))))
            }
            DeviceCall::ReadClipboardItem => {
                let item = clipboard::read_item(self.hwnd);
                reply(
                    id,
                    Ok(if item.is_empty() {
                        DeviceValue::Text(None)
                    } else {
                        DeviceValue::Clipboard(item)
                    }),
                )
            }
            DeviceCall::OpenUrl(url) | DeviceCall::OpenUrlIn(url, _) => {
                reply(id, open(&url).map(|_| DeviceValue::Done))
            }
            DeviceCall::SecureGet { key, prompt, .. } => self.secret_get(id, key, prompt),
            DeviceCall::SecureSet {
                key,
                value,
                options,
            } => {
                if options.biometric && !options.confirm_owner {
                    // Sealed with a key only Windows Hello gives back (S22):
                    // storing asks the user too. Nothing is stored unless
                    // it is sealed.
                    let app = self.app.clone();
                    std::thread::spawn(move || {
                        let r = security::sealed_set(&app, &key, &value);
                        shell::on_ui(move || reply(id, r.map(|_| DeviceValue::Done)));
                    });
                    return;
                }
                // `confirm_owner`: stored as it is (Credential Manager has
                // no per-item owner check), Hello asked when it is read.
                let app = self.app.clone();
                match self.found.hello {
                    Some((_, usable)) => reply(id, store(&app, &key, &value, options, usable)),
                    // Hello's check has not answered: asked here, off the
                    // UI thread.
                    None => {
                        std::thread::spawn(move || {
                            let usable = security::hello().usable();
                            let r = store(&app, &key, &value, options, usable);
                            shell::on_ui(move || reply(id, r));
                        });
                    }
                }
            }
            DeviceCall::SecureDelete { key, .. } => reply(
                id,
                security::secret_delete(&self.app, &key).map(|_| DeviceValue::Done),
            ),
            DeviceCall::BiometryStatus => {
                std::thread::spawn(move || {
                    let fields = security::hello().fields();
                    shell::on_ui(move || reply(id, Ok(DeviceValue::Fields(fields))));
                });
            }
            DeviceCall::Authenticate(r) => {
                if !r.passcode {
                    // Hello's consent check lets the user pick the PIN, even
                    // with a sensor: it cannot promise biometrics only.
                    return reply(id, Err(DeviceError::Unavailable));
                }
                security::authenticate(self.hwnd, &r.reason, move |r| reply(id, r));
            }
            DeviceCall::AuthRedirect(_) => reply(id, self.loopback.redirect()),
            DeviceCall::WebAuth(r) => self
                .loopback
                .start(&r.url, &r.redirect, move |r| reply(id, r)),
            DeviceCall::WatchLocation(accuracy) => {
                if !may(self.found.location) {
                    self.unavailable("location", "no location provider, or it is turned off");
                    return event(id, DeviceEvent::Error(DeviceError::Unavailable));
                }
                location::watch(id, accuracy, event);
            }
            DeviceCall::WatchMotion(interval) => {
                if !may(self.found.motion) {
                    self.unavailable("motion", "no accelerometer or gyrometer");
                    return event(id, DeviceEvent::Error(DeviceError::Unavailable));
                }
                sensors::watch(id, interval, event);
            }
            DeviceCall::CurrentLocation(accuracy) => {
                if !may(self.found.location) {
                    self.unavailable("location", "no location provider, or it is turned off");
                    return reply(id, Err(DeviceError::Unavailable));
                }
                // Asks first where Windows asks, as other platforms do;
                // only a refusal stops here.
                location::request(move |state| {
                    if state == PermissionState::Denied {
                        return reply(id, Err(DeviceError::Denied));
                    }
                    std::thread::spawn(move || {
                        let r = location::current(accuracy).map(DeviceValue::Location);
                        shell::on_ui(move || reply(id, r));
                    });
                });
            }
            DeviceCall::StopWatch => {
                location::stop(id);
                sensors::stop(id);
            }
            DeviceCall::Reconnect => {}
            // No browser id natively: nothing to renew.
            DeviceCall::RenewBrowserId => reply(id, Ok(DeviceValue::Done)),
            // No store here (#306): the Microsoft Store's `StoreContext`
            // needs an app the Store distributes; an app sells through its
            // own (`cx.set_store`), which never reaches the backend.
            DeviceCall::StoreProducts(_)
            | DeviceCall::Purchase(_)
            | DeviceCall::StoreEntitlements
            | DeviceCall::RestorePurchases
            | DeviceCall::WatchEntitlements
            | DeviceCall::StoreReturnUrl
            | DeviceCall::StoreCheckout { .. } => reply(id, Err(DeviceError::Unavailable)),
            // Haptics and sharing answer nothing, as on Apple platforms.
            DeviceCall::Haptic(h) => {
                if may(self.found.haptics) {
                    haptics::play(h);
                }
            }
            DeviceCall::Share(content) => {
                if may(self.found.share) {
                    crate::share::share(self.hwnd, &self.app, content);
                } else {
                    self.unavailable("sharing", "this Windows has no Share dialog");
                }
            }
            DeviceCall::WebAuthn(r) => {
                if !may(self.found.passkeys) {
                    self.unavailable("passkeys", "no webauthn.dll (Windows 10 1903 or later)");
                    return reply(id, Err(DeviceError::Unavailable));
                }
                webauthn::ceremony(self.hwnd, r, move |r| reply(id, r));
            }
            // Apple's sign-in is Apple's; cookies are the web's.
            DeviceCall::SignInWithApple(_) | DeviceCall::SetCookie(_) => {
                reply(id, Err(DeviceError::Unavailable))
            }
        }
    }

    /// A file dialog, from the message loop; the files are copied (or a
    /// folder's read) off the UI thread.
    fn pick(&mut self, id: u64, pick: Pick) {
        let (owner, dir) = (self.hwnd, self.picked.clone());
        let names = self.filter_names.clone();
        shell::later(move || {
            let paths = match pickers::open(owner, &pick, &names) {
                Ok(p) => p,
                Err(e) => return reply(id, Err(e)),
            };
            let folder = matches!(pick, Pick::Folder { .. });
            std::thread::spawn(move || {
                let files: Vec<PickedFile> = if folder {
                    paths
                        .first()
                        .map(|root| pickers::folder_files(root))
                        .unwrap_or_default()
                } else {
                    paths
                        .iter()
                        .filter_map(|p| pickers::copy_picked(p, &dir))
                        .collect()
                };
                shell::on_ui(move || reply(id, Ok(DeviceValue::Files(files))));
            });
        });
    }

    /// The save dialog (#210), then the file written off the UI thread.
    fn save(&mut self, id: u64, f: prism_core::SaveFile) {
        let owner = self.hwnd;
        let names = self.filter_names.clone();
        shell::later(move || {
            let path = match pickers::save(owner, &f.file_name(), &names) {
                Ok(p) => p,
                Err(e) => return reply(id, Err(e)),
            };
            std::thread::spawn(move || {
                let mime = f.mime_type();
                let r = f
                    .write_to(&path)
                    .map(|size| {
                        DeviceValue::Files(vec![PickedFile {
                            name: path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            path: path.to_string_lossy().into_owned(),
                            mime,
                            size,
                        }])
                    })
                    .map_err(|e| DeviceError::Failed(e.to_string()));
                shell::on_ui(move || reply(id, r));
            });
        });
    }

    /// A secret. A `biometric` one is opened with the app's Hello key, a
    /// `confirm_owner` one needs Windows Hello's consent first; both ask
    /// the user.
    fn secret_get(&mut self, id: u64, key: String, prompt: Option<String>) {
        match security::secret_get(&self.app, &key) {
            Ok(Some(v)) if crate::seal::is_sealed(&v) => {
                // Hello's key prompt has its own text: `prompt` is unused.
                let app = self.app.clone();
                std::thread::spawn(move || {
                    let r = security::sealed_get(&app, &key, &v);
                    shell::on_ui(move || reply(id, r.map(|v| DeviceValue::Text(Some(v)))));
                });
            }
            Ok(Some(v)) if v.starts_with(GATED) => {
                let value = v[GATED.len()..].to_string();
                let reason = prompt.unwrap_or_else(|| self.app.clone());
                security::authenticate(self.hwnd, &reason, move |r| {
                    reply(id, r.map(|_| DeviceValue::Text(Some(value))))
                });
            }
            r => reply(id, r.map(DeviceValue::Text)),
        }
    }

    fn schedule(&mut self, task: prism_core::background::BackgroundTask) {
        if let Some(t) = self.background.remove(&task.id) {
            shell::cancel(t);
        }
        let every = background_interval(task.every);
        let ms = every.as_millis().clamp(1, u32::MAX as u128) as u32;
        let id = task.id.clone();
        let timer = shell::timer(ms, move || {
            crate::imp::defer(Box::new(move |s| {
                s.services.background.remove(&id);
                // The runtime sends the task again for its next run.
                s.runtime.run_background_task(&id);
                let patches = s.runtime.poll();
                crate::imp::commit(s, patches);
            }));
        });
        self.background.insert(task.id, timer);
    }
}

impl Drop for WinServices {
    fn drop(&mut self) {
        tray::remove_all();
        notify::quit();
        crate::push::quit();
        location::stop_all();
        sensors::stop_all();
        webauthn::cancel();
        for (_, t) in self.background.drain() {
            shell::cancel(t);
        }
    }
}

impl PlatformServices for WinServices {
    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    fn handle(&mut self, request: PlatformRequest) {
        match request {
            PlatformRequest::Device { id, call } => self.device(id, call),
            PlatformRequest::Notify(n) => {
                if self.caps.notifications {
                    notify::notify(n);
                }
            }
            PlatformRequest::CancelNotification(id) => notify::cancel(&id),
            PlatformRequest::RequestNotificationPermission => {
                if self.caps.notifications {
                    notify::register();
                }
            }
            PlatformRequest::SetBadge(n) => taskbar::set_badge(self.hwnd, n),
            PlatformRequest::ShowAlert(spec) => crate::imp::defer(Box::new(move |s| {
                crate::alert::show(&s.renderer.widgets, spec)
            })),
            PlatformRequest::SetTray(spec) => tray::set(spec),
            PlatformRequest::RemoveTray(id) => tray::remove(&id),
            // The app's whole bar (#161): Windows has no menus of its own.
            PlatformRequest::SetAppMenu(menus) | PlatformRequest::SetMenuBar(menus) => {
                crate::imp::defer(Box::new(move |s| crate::menubar::set(s, menus)))
            }
            PlatformRequest::RegisterPush => {
                if let Some(why) = crate::push::missing() {
                    self.unavailable("push", why);
                }
                crate::push::register();
            }
            PlatformRequest::UnregisterPush => crate::push::unregister(),
            PlatformRequest::ScheduleBackgroundTask(task) => self.schedule(task),
            // Handled by the window (`imp::commit`), or nothing to do on a
            // desktop: widgets, Live Activities, cars, stylesheets.
            _ => {}
        }
    }
}

/// Opens a URL (or a file, a folder) with its default app. `PRISM_BROWSER`
/// (a development hook) opens web pages with that command instead, for a
/// test PC whose default browser must not change: a browser's path, or a
/// command line (`"C:\…\chrome.exe" --user-data-dir=…`), the URL appended.
pub fn open(url: &str) -> Result<(), DeviceError> {
    if let Some(browser) = crate::win::env("PRISM_BROWSER")
        && (url.starts_with("http://") || url.starts_with("https://"))
    {
        return browser_command(&browser)
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| DeviceError::Failed(format!("PRISM_BROWSER: {e}")));
    }
    let target = wide(url);
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(target.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // > 32: success.
    if r.0 as isize > 32 {
        Ok(())
    } else {
        Err(DeviceError::Failed(format!(
            "ShellExecute: {}",
            r.0 as isize
        )))
    }
}

/// `PRISM_BROWSER`'s program and arguments: an existing file as it is,
/// else a command line whose program may be quoted.
fn browser_command(line: &str) -> std::process::Command {
    let line = line.trim();
    if std::path::Path::new(line).is_file() {
        return std::process::Command::new(line);
    }
    let (program, args) = match line.strip_prefix('"').and_then(|l| l.split_once('"')) {
        Some(quoted) => quoted,
        None => line.split_once(' ').unwrap_or((line, "")),
    };
    let mut command = std::process::Command::new(program);
    command.args(args.split_whitespace());
    command
}

/// Shows `path` selected in Explorer (#210); else opens its folder.
fn reveal(path: &std::path::Path) {
    let p = wide(&path.to_string_lossy());
    unsafe {
        let pidl = ILCreateFromPathW(PCWSTR(p.as_ptr()));
        if !pidl.is_null() {
            let r = SHOpenFolderAndSelectItems(pidl, None, 0);
            ILFree(Some(pidl));
            if r.is_ok() {
                return;
            }
        }
    }
    if let Some(folder) = path.parent() {
        let _ = open(&folder.to_string_lossy());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn browser_command_line() {
        let c = super::browser_command(r#""C:\Program Files\B\b.exe" --a=1  --b"#);
        assert_eq!(c.get_program(), r"C:\Program Files\B\b.exe");
        assert_eq!(c.get_args().collect::<Vec<_>>(), ["--a=1", "--b"]);
        let c = super::browser_command(r"C:\b.exe --x");
        assert_eq!(c.get_program(), r"C:\b.exe");
        assert_eq!(c.get_args().collect::<Vec<_>>(), ["--x"]);
    }
}
