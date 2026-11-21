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
//!
//! Push answers "unavailable": WNS needs package identity (`push`). There
//! is no location, motion, camera, haptics or share sheet yet: capabilities
//! say so and calls fail with "unavailable".
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

use crate::pickers::{self, Pick};
use crate::security::{self, Answer, Loopback};
use crate::win::{log, wide};
use crate::{clipboard, notify, shell, taskbar, tray};

/// Prefix of secrets that need Windows Hello first.
const GATED: &str = "\u{1}prism-gated\u{1}";

pub struct WinServices {
    pub hwnd: HWND,
    caps: Capabilities,
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
        let hello = security::hello_at_start();
        crate::imp::trace("Windows Hello");
        let notifications = notify::supported();
        notify::start(data);
        WinServices {
            hwnd,
            caps: Capabilities {
                notifications,
                tray: true,
                app_menu: true,
                photos: true,
                clipboard: true,
                secure_storage: true,
                biometry: hello.kind,
                device_auth: hello.usable(),
                web_auth: true,
                // WebView2 (`web.rs`), Explorer drops and drags between
                // views (`pointer.rs`).
                web_view: true,
                drop_files: true,
                drag_and_drop: true,
                ..Default::default()
            },
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

    fn unavailable(&mut self, what: &'static str, why: &str) {
        if self.told.insert(what) {
            log!("{what}: unavailable on Windows ({why})");
        }
    }

    fn device(&mut self, id: u64, call: DeviceCall) {
        match call {
            DeviceCall::PermissionStatus(p) | DeviceCall::RequestPermission(p) => {
                let state = match p {
                    Permission::Notifications if self.caps.notifications => notify::permission(),
                    // The file dialog needs no permission.
                    Permission::Photos => PermissionState::Granted,
                    _ => PermissionState::Unavailable,
                };
                reply(id, Ok(DeviceValue::Permission(state)));
            }
            DeviceCall::PickImages(o) => {
                if o.camera {
                    self.unavailable("the camera", "not supported yet");
                    return reply(id, Err(DeviceError::Unavailable));
                }
                self.pick(id, Pick::Images { limit: o.limit });
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
                // Credential Manager has no per-item owner check: Hello
                // guards the value when it is read.
                if options.biometric && !self.caps.device_auth {
                    return reply(id, Err(DeviceError::Unavailable));
                }
                let value = format!("{}{value}", if options.biometric { GATED } else { "" });
                reply(
                    id,
                    security::secret_set(&self.app, &key, &value).map(|_| DeviceValue::Done),
                );
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
                if !r.passcode && self.caps.biometry == prism_core::BiometryKind::None {
                    // Hello without a sensor is its PIN only.
                    return reply(id, Err(DeviceError::Unavailable));
                }
                security::authenticate(self.hwnd, &r.reason, move |r| reply(id, r));
            }
            DeviceCall::AuthRedirect(_) => reply(id, self.loopback.redirect()),
            DeviceCall::WebAuth(r) => self
                .loopback
                .start(&r.url, &r.redirect, move |r| reply(id, r)),
            DeviceCall::WatchLocation(_) => {
                self.unavailable("location", "not supported yet");
                event(id, DeviceEvent::Error(DeviceError::Unavailable));
            }
            DeviceCall::WatchMotion(_) => {
                event(id, DeviceEvent::Error(DeviceError::Unavailable));
            }
            DeviceCall::CurrentLocation(_) => {
                self.unavailable("location", "not supported yet");
                reply(id, Err(DeviceError::Unavailable));
            }
            DeviceCall::StopWatch | DeviceCall::Reconnect => {}
            // No haptics or share sheet yet; Apple's sign-in and passkeys
            // are other platforms'; cookies are the web's.
            DeviceCall::Haptic(_)
            | DeviceCall::Share(_)
            | DeviceCall::SignInWithApple(_)
            | DeviceCall::WebAuthn(_)
            | DeviceCall::SetCookie(_) => reply(id, Err(DeviceError::Unavailable)),
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

    /// A secret; one stored with `biometric` needs Windows Hello first.
    fn secret_get(&mut self, id: u64, key: String, prompt: Option<String>) {
        match security::secret_get(&self.app, &key) {
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
                self.unavailable(
                    "push",
                    "Windows push notifications (WNS) need package identity: an MSIX \
                     package, or a sparse package for an unpackaged app",
                );
                crate::imp::defer(Box::new(|s| {
                    let patches = s.runtime.push_registered(Err(DeviceError::Unavailable));
                    crate::imp::commit(s, patches);
                }));
            }
            PlatformRequest::UnregisterPush => {}
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
