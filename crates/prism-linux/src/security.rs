//! Secrets, the device owner and sign-in on Linux desktops, through
//! freedesktop standards:
//!
//! | | |
//! |---|---|
//! | `secure_storage` | the Secret Service (GNOME Keyring, KWallet…) on the session bus |
//! | `authenticate` | fprintd on the system bus (fingerprint); no desktop standard checks a password for an app |
//! | web sign-in | the default browser, redirected to a loopback server (RFC 8252 §7.3) |
//!
//! Secrets travel over the user's session bus with the Secret Service's
//! `plain` algorithm (the bus is private to the user); the keyring stores
//! them encrypted with the login password.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use prism_core::{BiometryKind, DeviceError, DeviceValue};

const SECRETS: &str = "org.freedesktop.secrets";
const SECRETS_PATH: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const FPRINT: &str = "net.reactivated.Fprint";

type Answer = Result<DeviceValue, DeviceError>;

fn failed(e: impl std::fmt::Display) -> DeviceError {
    DeviceError::Failed(e.to_string())
}

fn object_path(p: &str) -> glib::variant::ObjectPath {
    glib::variant::ObjectPath::try_from(p.to_string()).expect("object path")
}

fn paths(v: &glib::Variant) -> Vec<String> {
    v.iter()
        .filter_map(|c| c.str().map(str::to_string))
        .collect()
}

/// A name is owned or activatable on `bus`.
pub(crate) fn has_name(bus: &gio::DBusConnection, name: &str) -> bool {
    ["ListNames", "ListActivatableNames"].iter().any(|method| {
        bus.call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            method,
            None,
            Some(glib::VariantTy::new("(as)").expect("type")),
            gio::DBusCallFlags::NONE,
            2000,
            None::<&gio::Cancellable>,
        )
        .ok()
        .and_then(|v| v.child_value(0).get::<Vec<String>>())
        .is_some_and(|names| names.iter().any(|n| n == name))
    })
}

fn session_bus() -> Option<gio::DBusConnection> {
    gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).ok()
}

/// fprintd lives on the system bus (`PRISM_FPRINTD_BUS=session` for tests).
fn fprint_bus_type() -> gio::BusType {
    if std::env::var("PRISM_FPRINTD_BUS").is_ok_and(|b| b == "session") {
        gio::BusType::Session
    } else {
        gio::BusType::System
    }
}

/// What this desktop offers: `(secure storage, biometry)`.
pub(crate) fn capabilities() -> (bool, BiometryKind) {
    let secrets = session_bus().is_some_and(|b| has_name(&b, SECRETS));
    let fingerprint = gio::bus_get_sync(fprint_bus_type(), None::<&gio::Cancellable>)
        .ok()
        .filter(|b| has_name(b, FPRINT))
        .and_then(|b| {
            b.call_sync(
                Some(FPRINT),
                "/net/reactivated/Fprint/Manager",
                "net.reactivated.Fprint.Manager",
                "GetDefaultDevice",
                None,
                Some(glib::VariantTy::new("(o)").expect("type")),
                gio::DBusCallFlags::NONE,
                3000,
                None::<&gio::Cancellable>,
            )
            .ok()
        })
        .is_some();
    (
        secrets,
        if fingerprint {
            BiometryKind::Fingerprint
        } else {
            BiometryKind::None
        },
    )
}

async fn call(
    bus: &gio::DBusConnection,
    name: &str,
    path: &str,
    interface: &str,
    method: &str,
    args: Option<glib::Variant>,
) -> Result<glib::Variant, glib::Error> {
    bus.call_future(
        Some(name),
        path,
        interface,
        method,
        args.as_ref(),
        None,
        gio::DBusCallFlags::NONE,
        // Prompts (unlocking the keyring) wait on the user.
        120_000,
    )
    .await
}

/// Resolves once with a value (a signal's arguments).
fn oneshot<T: 'static>() -> (Rc<dyn Fn(T)>, impl std::future::Future<Output = T>) {
    type Slot<T> = Rc<RefCell<(Option<T>, Option<std::task::Waker>)>>;
    let slot: Slot<T> = Rc::new(RefCell::new((None, None)));
    let tx = slot.clone();
    let send: Rc<dyn Fn(T)> = Rc::new(move |v| {
        let mut s = tx.borrow_mut();
        if s.0.is_none() {
            s.0 = Some(v);
            if let Some(w) = s.1.take() {
                w.wake();
            }
        }
    });
    let fut = std::future::poll_fn(move |cx| {
        let mut s = slot.borrow_mut();
        match s.0.take() {
            Some(v) => std::task::Poll::Ready(v),
            None => {
                s.1 = Some(cx.waker().clone());
                std::task::Poll::Pending
            }
        }
    });
    (send, fut)
}

// --- Secret Service ---------------------------------------------------------------------------

/// The Secret Service (org.freedesktop.Secret.*.xml), `plain` session.
struct Secrets {
    bus: gio::DBusConnection,
    session: String,
}

impl Secrets {
    async fn open() -> Result<Self, DeviceError> {
        let bus = gio::bus_get_future(gio::BusType::Session)
            .await
            .map_err(|_| DeviceError::Unavailable)?;
        let opened = call(
            &bus,
            SECRETS,
            SECRETS_PATH,
            SERVICE,
            "OpenSession",
            Some(("plain", "".to_variant()).to_variant()),
        )
        .await
        .map_err(|e| failed(format!("Secret Service: {e}")))?;
        let session = opened.child_value(1).str().unwrap_or("/").to_string();
        Ok(Secrets { bus, session })
    }

    fn attributes(app: &str, key: &str) -> HashMap<String, String> {
        HashMap::from([
            ("xdg:schema".to_string(), "dev.prism.Secret".to_string()),
            ("application".to_string(), app.to_string()),
            ("key".to_string(), key.to_string()),
        ])
    }

    /// Runs a prompt (unlocking, creating the keyring); `false` if dismissed.
    // `signal_subscribe` is the GLib 2.74 API (Debian bookworm's GLib).
    #[allow(deprecated)]
    async fn prompt(&self, prompt: &str) -> Result<bool, DeviceError> {
        if prompt == "/" {
            return Ok(true);
        }
        let (done, completed) = oneshot::<bool>();
        let id = self.bus.signal_subscribe(
            Some(SECRETS),
            Some("org.freedesktop.Secret.Prompt"),
            Some("Completed"),
            Some(prompt),
            None,
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, _, params| done(params.child_value(0).get::<bool>() != Some(true)),
        );
        let started = call(
            &self.bus,
            SECRETS,
            prompt,
            "org.freedesktop.Secret.Prompt",
            "Prompt",
            Some(("",).to_variant()),
        )
        .await;
        let ok = match started {
            Ok(_) => completed.await,
            Err(e) => {
                self.bus.signal_unsubscribe(id);
                return Err(failed(e));
            }
        };
        self.bus.signal_unsubscribe(id);
        Ok(ok)
    }

    async fn unlock(&self, objects: &[String]) -> Result<(), DeviceError> {
        let list: Vec<glib::variant::ObjectPath> = objects.iter().map(|p| object_path(p)).collect();
        let r = call(
            &self.bus,
            SECRETS,
            SECRETS_PATH,
            SERVICE,
            "Unlock",
            Some((list,).to_variant()),
        )
        .await
        .map_err(failed)?;
        let prompt = r.child_value(1).str().unwrap_or("/").to_string();
        if self.prompt(&prompt).await? {
            Ok(())
        } else {
            Err(DeviceError::Cancelled)
        }
    }

    /// The items for `key`, unlocked.
    async fn items(&self, app: &str, key: &str) -> Result<Vec<String>, DeviceError> {
        let found = call(
            &self.bus,
            SECRETS,
            SECRETS_PATH,
            SERVICE,
            "SearchItems",
            Some((Self::attributes(app, key),).to_variant()),
        )
        .await
        .map_err(failed)?;
        let (mut unlocked, locked) = (paths(&found.child_value(0)), paths(&found.child_value(1)));
        if !locked.is_empty() {
            self.unlock(&locked).await?;
            unlocked.extend(locked);
        }
        Ok(unlocked)
    }

    async fn get(&self, app: &str, key: &str) -> Result<Option<String>, DeviceError> {
        let items = self.items(app, key).await?;
        if items.is_empty() {
            return Ok(None);
        }
        let list: Vec<glib::variant::ObjectPath> = items.iter().map(|p| object_path(p)).collect();
        let secrets = call(
            &self.bus,
            SECRETS,
            SECRETS_PATH,
            SERVICE,
            "GetSecrets",
            Some((list, object_path(&self.session)).to_variant()),
        )
        .await
        .map_err(failed)?;
        // a{o(oayays)}: the first item's value.
        let dict = secrets.child_value(0);
        let value = dict
            .iter()
            .next()
            .map(|entry| entry.child_value(1).child_value(2))
            .and_then(|v| v.get::<Vec<u8>>());
        Ok(value.map(|v| String::from_utf8_lossy(&v).into_owned()))
    }

    /// The default collection (created if the keyring has none), unlocked.
    async fn collection(&self) -> Result<String, DeviceError> {
        let alias = call(
            &self.bus,
            SECRETS,
            SECRETS_PATH,
            SERVICE,
            "ReadAlias",
            Some(("default",).to_variant()),
        )
        .await
        .map_err(failed)?;
        let mut path = alias.child_value(0).str().unwrap_or("/").to_string();
        if path == "/" {
            let props = HashMap::from([(
                "org.freedesktop.Secret.Collection.Label".to_string(),
                "Login".to_variant(),
            )]);
            let created = call(
                &self.bus,
                SECRETS,
                SECRETS_PATH,
                SERVICE,
                "CreateCollection",
                Some((props, "default").to_variant()),
            )
            .await
            .map_err(failed)?;
            path = created.child_value(0).str().unwrap_or("/").to_string();
            let prompt = created.child_value(1).str().unwrap_or("/").to_string();
            if !self.prompt(&prompt).await? {
                return Err(DeviceError::Cancelled);
            }
            if path == "/" {
                let again = call(
                    &self.bus,
                    SECRETS,
                    SECRETS_PATH,
                    SERVICE,
                    "ReadAlias",
                    Some(("default",).to_variant()),
                )
                .await
                .map_err(failed)?;
                path = again.child_value(0).str().unwrap_or("/").to_string();
            }
        }
        if path == "/" {
            return Err(failed("Secret Service: no keyring"));
        }
        self.unlock(std::slice::from_ref(&path)).await?;
        Ok(path)
    }

    async fn set(&self, app: &str, label: &str, key: &str, value: &str) -> Result<(), DeviceError> {
        let collection = self.collection().await?;
        let props = HashMap::from([
            (
                "org.freedesktop.Secret.Item.Label".to_string(),
                format!("{label}: {key}").to_variant(),
            ),
            (
                "org.freedesktop.Secret.Item.Attributes".to_string(),
                Self::attributes(app, key).to_variant(),
            ),
        ]);
        let secret = (
            object_path(&self.session),
            Vec::<u8>::new(),
            value.as_bytes().to_vec(),
            "text/plain; charset=utf8".to_string(),
        );
        let created = call(
            &self.bus,
            SECRETS,
            &collection,
            "org.freedesktop.Secret.Collection",
            "CreateItem",
            Some((props, secret, true).to_variant()),
        )
        .await
        .map_err(failed)?;
        let prompt = created.child_value(1).str().unwrap_or("/").to_string();
        if self.prompt(&prompt).await? {
            Ok(())
        } else {
            Err(DeviceError::Cancelled)
        }
    }

    async fn delete(&self, app: &str, key: &str) -> Result<(), DeviceError> {
        for item in self.items(app, key).await? {
            let r = call(
                &self.bus,
                SECRETS,
                &item,
                "org.freedesktop.Secret.Item",
                "Delete",
                None,
            )
            .await
            .map_err(failed)?;
            let prompt = r.child_value(0).str().unwrap_or("/").to_string();
            self.prompt(&prompt).await?;
        }
        Ok(())
    }

    async fn close(self) {
        let _ = call(
            &self.bus,
            SECRETS,
            &self.session,
            "org.freedesktop.Secret.Session",
            "Close",
            None,
        )
        .await;
    }
}

/// `get` / `set` / `delete` a secret of app `app`, then `reply`.
pub(crate) enum SecretOp {
    Get,
    Set { label: String, value: String },
    Delete,
}

pub(crate) fn secret(app: String, key: String, op: SecretOp, reply: impl FnOnce(Answer) + 'static) {
    glib::MainContext::default().spawn_local(async move {
        let secrets = match Secrets::open().await {
            Ok(s) => s,
            Err(e) => return reply(Err(e)),
        };
        let r = match op {
            SecretOp::Get => secrets.get(&app, &key).await.map(DeviceValue::Text),
            SecretOp::Set { label, value } => secrets
                .set(&app, &label, &key, &value)
                .await
                .map(|_| DeviceValue::Done),
            SecretOp::Delete => secrets.delete(&app, &key).await.map(|_| DeviceValue::Done),
        };
        secrets.close().await;
        reply(r);
    });
}

// --- fprintd ----------------------------------------------------------------------------------

const DEVICE: &str = "net.reactivated.Fprint.Device";

async fn fprint_device() -> Result<(gio::DBusConnection, String), DeviceError> {
    let bus = gio::bus_get_future(fprint_bus_type())
        .await
        .map_err(|_| DeviceError::Unavailable)?;
    let r = call(
        &bus,
        FPRINT,
        "/net/reactivated/Fprint/Manager",
        "net.reactivated.Fprint.Manager",
        "GetDefaultDevice",
        None,
    )
    .await
    .map_err(|_| DeviceError::Unavailable)?;
    let path = r.child_value(0).str().unwrap_or("/").to_string();
    Ok((bus, path))
}

/// `kind`, `enrolled` (fingers of this user), `passcode` (none: no
/// desktop standard).
pub(crate) fn biometry(reply: impl FnOnce(Answer) + 'static) {
    glib::MainContext::default().spawn_local(async move {
        let mut f = std::collections::BTreeMap::new();
        f.insert("passcode".to_string(), "0".to_string());
        match fprint_device().await {
            Ok((bus, path)) => {
                let fingers = call(
                    &bus,
                    FPRINT,
                    &path,
                    DEVICE,
                    "ListEnrolledFingers",
                    Some(("",).to_variant()),
                )
                .await
                .ok()
                .and_then(|v| v.child_value(0).get::<Vec<String>>())
                .unwrap_or_default();
                f.insert("kind".into(), "fingerprint".into());
                f.insert(
                    "enrolled".into(),
                    (if fingers.is_empty() { "0" } else { "1" }).into(),
                );
            }
            Err(_) => {
                f.insert("kind".into(), "none".into());
                f.insert("enrolled".into(), "0".into());
            }
        }
        reply(Ok(DeviceValue::Fields(f)));
    });
}

/// Verifies a finger with fprintd while a small window explains why.
// `signal_subscribe` is the GLib 2.74 API (Debian bookworm's GLib).
#[allow(deprecated)]
pub(crate) fn authenticate(
    parent: &gtk::ApplicationWindow,
    reason: String,
    reply: impl FnOnce(Answer) + 'static,
) {
    let parent = parent.clone();
    glib::MainContext::default().spawn_local(async move {
        let (bus, path) = match fprint_device().await {
            Ok(d) => d,
            Err(e) => return reply(Err(e)),
        };
        // "" = the user running the app.
        if let Err(e) = call(
            &bus,
            FPRINT,
            &path,
            DEVICE,
            "Claim",
            Some(("",).to_variant()),
        )
        .await
        {
            return reply(Err(if e.to_string().contains("PermissionDenied") {
                DeviceError::Denied
            } else {
                failed(e)
            }));
        }
        let (finish, outcome) = oneshot::<Answer>();
        let window = verify_window(&parent, &reason, finish.clone());
        let on_status = finish.clone();
        let sub = bus.signal_subscribe(
            Some(FPRINT),
            Some(DEVICE),
            Some("VerifyStatus"),
            Some(&path),
            None,
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, _, params| {
                let result = params.child_value(0).str().unwrap_or_default().to_string();
                let done = params.child_value(1).get::<bool>().unwrap_or(true);
                match result.as_str() {
                    "verify-match" => on_status(Ok(DeviceValue::Done)),
                    "verify-no-match" if done => on_status(Err(DeviceError::Denied)),
                    "verify-disconnected" | "verify-unknown-error" => {
                        on_status(Err(failed(format!("fprintd: {result}"))))
                    }
                    // Retry scans: keep waiting.
                    _ if done => on_status(Err(DeviceError::Denied)),
                    _ => {}
                }
            },
        );
        let r = match call(
            &bus,
            FPRINT,
            &path,
            DEVICE,
            "VerifyStart",
            Some(("any",).to_variant()),
        )
        .await
        {
            Ok(_) => outcome.await,
            Err(e) => Err(if e.to_string().contains("NoEnrolledPrints") {
                DeviceError::Unavailable
            } else {
                failed(e)
            }),
        };
        bus.signal_unsubscribe(sub);
        window.close();
        let _ = call(&bus, FPRINT, &path, DEVICE, "VerifyStop", None).await;
        let _ = call(&bus, FPRINT, &path, DEVICE, "Release", None).await;
        reply(r);
    });
}

fn verify_window(
    parent: &gtk::ApplicationWindow,
    reason: &str,
    finish: Rc<dyn Fn(Answer)>,
) -> gtk::Window {
    let dialog = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .default_width(340)
        .build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_top(20);
    body.set_margin_bottom(20);
    body.set_margin_start(20);
    body.set_margin_end(20);
    let icon = gtk::Image::from_icon_name("auth-fingerprint-symbolic");
    icon.set_pixel_size(48);
    body.append(&icon);
    let title = gtk::Label::new(Some(reason));
    title.add_css_class("title-3");
    title.set_wrap(true);
    body.append(&title);
    body.append(&gtk::Label::new(Some("Touch the fingerprint reader")));
    let cancel = gtk::Button::with_label("Cancel");
    cancel.set_halign(gtk::Align::End);
    let f = finish.clone();
    cancel.connect_clicked(move |_| f(Err(DeviceError::Cancelled)));
    body.append(&cancel);
    dialog.set_child(Some(&body));
    dialog.connect_close_request(move |_| {
        finish(Err(DeviceError::Cancelled));
        glib::Propagation::Proceed
    });
    dialog.present();
    dialog
}

// --- Loopback redirect (RFC 8252 §7.3) --------------------------------------------------------

/// Listeners waiting for a sign-in, by redirect URI.
#[derive(Default)]
pub(crate) struct Loopback {
    listeners: HashMap<String, std::net::TcpListener>,
}

impl Loopback {
    /// `http://127.0.0.1:<port>/callback`, served by a fresh listener.
    pub fn redirect(&mut self) -> Answer {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(failed)?;
        let port = listener.local_addr().map_err(failed)?.port();
        let uri = format!("http://127.0.0.1:{port}/callback");
        self.listeners.insert(uri.clone(), listener);
        Ok(DeviceValue::Text(Some(uri)))
    }

    /// Opens `url` in the default browser and waits (on a thread) for the
    /// redirect to `redirect`; `reply` runs on the main thread.
    pub fn start(
        &mut self,
        url: &str,
        redirect: &str,
        reply: impl FnOnce(Answer) + Send + 'static,
    ) {
        let Some(listener) = self.listeners.remove(redirect) else {
            return reply(Err(failed(
                "unknown redirect URI (ask auth_redirect first)",
            )));
        };
        if let Err(e) = gio::AppInfo::launch_default_for_uri(url, None::<&gio::AppLaunchContext>) {
            return reply(Err(failed(format!("no browser: {e}"))));
        }
        std::thread::spawn(move || {
            let r = wait_for_redirect(&listener, std::time::Duration::from_secs(600));
            glib::MainContext::default().invoke(move || reply(r));
        });
    }
}

fn wait_for_redirect(listener: &std::net::TcpListener, timeout: std::time::Duration) -> Answer {
    use std::io::{Read, Write};
    let _ = listener.set_nonblocking(true);
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let Ok((mut stream, _)) = listener.accept() else {
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..n]);
        let target = request
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("GET "))
            .and_then(|l| l.split(' ').next())
            .unwrap_or("");
        if !target.starts_with("/callback") {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            continue;
        }
        let page = "<!doctype html><meta charset=utf-8><title>Signed in</title><p style=\"font:16px system-ui;margin:40px;text-align:center\">Signed in. You can close this tab and return to the app.</p>";
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{page}",
                page.len()
            )
            .as_bytes(),
        );
        return Ok(DeviceValue::Text(Some(format!("http://127.0.0.1{target}"))));
    }
    Err(DeviceError::Cancelled)
}

/// The ignored ones run against a real session bus (`dbus-run-session`, an
/// unlocked gnome-keyring, `scripts/mock-fprintd.py` with
/// `PRISM_FPRINTD_BUS=session`): `cargo test -p prismsdk-linux -- --ignored`.
#[cfg(test)]
mod tests {
    use super::*;

    fn run<T: 'static>(f: impl FnOnce(Rc<dyn Fn(T)>)) -> T {
        let (tx, rx) = oneshot::<T>();
        f(tx);
        glib::MainContext::default().block_on(rx)
    }

    #[test]
    #[ignore = "needs a Secret Service on the session bus"]
    fn secret_service_round_trip() {
        let app = "dev.prism.test".to_string();
        let op = |op: SecretOp| {
            let app = app.clone();
            run(move |done| secret(app, "token".into(), op, move |r| done(r)))
        };
        let set = SecretOp::Set {
            label: "Prism test".into(),
            value: "s3cret ✓".into(),
        };
        assert_eq!(op(set), Ok(DeviceValue::Done));
        assert_eq!(
            op(SecretOp::Get),
            Ok(DeviceValue::Text(Some("s3cret ✓".into())))
        );
        assert_eq!(op(SecretOp::Delete), Ok(DeviceValue::Done));
        assert_eq!(op(SecretOp::Get), Ok(DeviceValue::Text(None)));
        assert!(capabilities().0);
    }

    #[test]
    #[ignore = "needs the mock fprintd (PRISM_FPRINTD_BUS=session)"]
    fn fprintd_status() {
        let status = run(|done| biometry(move |r| done(r)));
        let Ok(DeviceValue::Fields(f)) = status else {
            panic!("{status:?}")
        };
        assert_eq!(f["kind"], "fingerprint");
        assert_eq!(f["enrolled"], "1");
        assert_eq!(capabilities().1, BiometryKind::Fingerprint);
    }

    #[test]
    fn loopback_catches_the_redirect() {
        let mut lb = Loopback::default();
        let Ok(DeviceValue::Text(Some(uri))) = lb.redirect() else {
            panic!()
        };
        assert!(uri.starts_with("http://127.0.0.1:") && uri.ends_with("/callback"));
        let listener = lb.listeners.remove(&uri).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n").unwrap();
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(b"GET /callback?code=abc&state=xyz HTTP/1.1\r\nHost: x\r\n\r\n")
                .unwrap();
            let mut page = String::new();
            let _ = s.read_to_string(&mut page);
            page
        });
        let r = wait_for_redirect(&listener, std::time::Duration::from_secs(10));
        assert_eq!(
            r,
            Ok(DeviceValue::Text(Some(
                "http://127.0.0.1/callback?code=abc&state=xyz".into()
            )))
        );
        assert!(client.join().unwrap().contains("200 OK"));
    }
}
