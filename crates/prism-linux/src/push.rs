//! Remote push on Linux: UnifiedPush over D-Bus (spec DBUS_0.3.0). There is
//! no desktop push service; a distributor (KUnifiedPush, ntfy's desktop
//! client…) owns `org.unifiedpush.Distributor.<name>` on the session bus and
//! relays Web Push messages to us. The app answers on its own connection at
//! `/org/unifiedpush/Connector` (`org.unifiedpush.Connector2`) while it runs
//! (no D-Bus activation: Prism apps are not D-Bus services).
//!
//! Messages are RFC 8291 encrypted: decrypted here with this device's keys
//! (`prism-push`), kept in the app's data directory.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use gtk::gio;
use gtk::glib;
use prism_core::{DeviceError, PushEvent, PushMessage, PushService, PushSubscription};

const PREFIX: &str = "org.unifiedpush.Distributor.";
const DISTRIBUTOR_PATH: &str = "/org/unifiedpush/Distributor";
const DISTRIBUTOR: &str = "org.unifiedpush.Distributor2";
const CONNECTOR_PATH: &str = "/org/unifiedpush/Connector";
const CONNECTOR_XML: &str = r#"<node>
  <interface name="org.unifiedpush.Connector2">
    <method name="Message"><arg type="a{sv}" direction="in"/><arg type="a{sv}" direction="out"/></method>
    <method name="NewEndpoint"><arg type="a{sv}" direction="in"/><arg type="a{sv}" direction="out"/></method>
    <method name="Unregistered"><arg type="a{sv}" direction="in"/><arg type="a{sv}" direction="out"/></method>
  </interface>
</node>"#;

thread_local! {
    static REGISTERED: RefCell<Option<gio::RegistrationId>> = const { RefCell::new(None) };
}

fn session() -> Option<gio::DBusConnection> {
    gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).ok()
}

/// The distributor to use: `UNIFIEDPUSH_DISTRIBUTOR` when it runs, else the
/// only (or first) one on the session bus.
pub(crate) fn distributor() -> Option<String> {
    let bus = session()?;
    let names = bus
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "ListNames",
            None,
            Some(glib::VariantTy::new("(as)").expect("type")),
            gio::DBusCallFlags::NONE,
            2000,
            None::<&gio::Cancellable>,
        )
        .ok()?
        .child_value(0)
        .get::<Vec<String>>()?;
    let mut found: Vec<String> = names
        .into_iter()
        .filter(|n| n.starts_with(PREFIX))
        .collect();
    found.sort();
    if let Ok(chosen) = std::env::var("UNIFIEDPUSH_DISTRIBUTOR") {
        let chosen = if chosen.starts_with(PREFIX) {
            chosen
        } else {
            format!("{PREFIX}{chosen}")
        };
        if found.contains(&chosen) {
            return Some(chosen);
        }
    }
    found.into_iter().next()
}

/// Our registration token (random, made once and kept private in the data
/// directory).
fn token(dir: &Path) -> Result<String, DeviceError> {
    let bytes = prism_push::secret_file::load_or_create(
        &dir.join("prism-push-token"),
        |b| std::str::from_utf8(b).is_ok_and(|t| !t.trim().is_empty()),
        || {
            // 16 random bytes, hex (UUID-sized).
            let keys = prism_push::webpush::ReceiverKeys::generate();
            let t: String = keys.auth.iter().map(|b| format!("{b:02x}")).collect();
            t.into_bytes()
        },
    )
    .map_err(|e| DeviceError::Failed(format!("push token: {e}")))?;
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
}

/// This device's Web Push keys, made once and kept private.
fn keys(dir: &Path) -> Result<prism_push::webpush::ReceiverKeys, DeviceError> {
    prism_push::webpush::ReceiverKeys::load_or_create(&dir.join("prism-push-keys"))
        .map_err(|e| DeviceError::Failed(format!("push keys: {e}")))
}

fn dict(v: &glib::Variant) -> glib::VariantDict {
    glib::VariantDict::new(Some(&v.child_value(0)))
}

fn empty_reply() -> glib::Variant {
    glib::Variant::tuple_from_iter([glib::VariantDict::new(None).end()])
}

/// Serves the connector object, then asks the distributor for an endpoint.
/// Answers arrive through `on_registered` and `on_message`.
pub(crate) fn register(
    dir: PathBuf,
    on_registered: impl Fn(Result<PushSubscription, DeviceError>) + 'static,
    on_message: impl Fn(PushMessage) + 'static,
) {
    let Some(bus) = session() else {
        return on_registered(Err(DeviceError::Unavailable));
    };
    let Some(name) = distributor() else {
        return on_registered(Err(DeviceError::Failed(
            "no UnifiedPush distributor on the session bus".into(),
        )));
    };
    // The token, and the keys the endpoint will be announced with, are
    // kept before a distributor is asked for one.
    let token = match token(&dir).and_then(|t| keys(&dir).map(|_| t)) {
        Ok(t) => t,
        Err(e) => return on_registered(Err(e)),
    };
    let on_registered = std::rc::Rc::new(on_registered);
    if REGISTERED.with(|r| r.borrow().is_none()) {
        let node = gio::DBusNodeInfo::for_xml(CONNECTOR_XML).expect("connector XML");
        let iface = node
            .lookup_interface("org.unifiedpush.Connector2")
            .expect("interface");
        let (dir, ours, done) = (dir.clone(), token.clone(), on_registered.clone());
        let id = bus
            .register_object(CONNECTOR_PATH, &iface)
            .method_call(move |_, _sender, _, _, method, params, invocation| {
                let args = dict(&params);
                let token = args.lookup::<String>("token").ok().flatten();
                // Calls for another (older) token are ignored.
                if token.as_deref() != Some(ours.as_str()) {
                    return invocation.return_value(Some(&empty_reply()));
                }
                match method {
                    "NewEndpoint" => {
                        if let Ok(Some(endpoint)) = args.lookup::<String>("endpoint") {
                            done(keys(&dir).map(|k| {
                                let mut sub =
                                    PushSubscription::new(PushService::UnifiedPush, endpoint);
                                sub.p256dh = Some(k.p256dh());
                                sub.auth = Some(k.auth());
                                sub
                            }));
                        }
                        invocation.return_value(Some(&empty_reply()));
                    }
                    "Unregistered" => {
                        done(Err(DeviceError::Failed(
                            "unregistered by the distributor".into(),
                        )));
                        invocation.return_value(Some(&empty_reply()));
                    }
                    "Message" => {
                        let body = args.lookup::<Vec<u8>>("message").ok().flatten();
                        let id = args.lookup::<String>("id").ok().flatten();
                        let plain = body.map(|b| {
                            keys(&dir).and_then(|k| {
                                k.decrypt(&b)
                                    .map_err(|e| DeviceError::Failed(e.to_string()))
                            })
                        });
                        match plain {
                            Some(Ok(plain)) => {
                                on_message(message(&String::from_utf8_lossy(&plain)))
                            }
                            Some(Err(e)) => {
                                eprintln!("prism-linux: push message not decrypted: {e}")
                            }
                            None => {}
                        }
                        // The id acknowledges the message (Connector2).
                        let reply = glib::VariantDict::new(None);
                        if let Some(id) = id {
                            reply.insert("id", id);
                        }
                        invocation
                            .return_value(Some(&glib::Variant::tuple_from_iter([reply.end()])));
                    }
                    _ => invocation.return_value(Some(&empty_reply())),
                }
            })
            .build();
        match id {
            Ok(id) => REGISTERED.with(|r| *r.borrow_mut() = Some(id)),
            Err(e) => return on_registered(Err(DeviceError::Failed(e.to_string()))),
        }
    }
    let args = glib::VariantDict::new(None);
    args.insert(
        "service",
        bus.unique_name().map(|n| n.to_string()).unwrap_or_default(),
    );
    args.insert("token", token);
    args.insert(
        "description",
        glib::application_name()
            .map(|n| n.to_string())
            .unwrap_or_default(),
    );
    glib::MainContext::default().spawn_local(async move {
        let result = bus
            .call_future(
                Some(&name),
                DISTRIBUTOR_PATH,
                DISTRIBUTOR,
                "Register",
                Some(&glib::Variant::tuple_from_iter([args.end()])),
                Some(glib::VariantTy::new("(a{sv})").expect("type")),
                gio::DBusCallFlags::NONE,
                10_000,
            )
            .await;
        match result {
            Ok(v) => {
                let out = dict(&v);
                if out.lookup::<String>("success").ok().flatten().as_deref()
                    == Some("REGISTRATION_FAILED")
                {
                    let reason = out
                        .lookup::<String>("reason")
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    on_registered(Err(DeviceError::Failed(format!(
                        "registration failed: {reason}"
                    ))));
                }
                // Success: the endpoint comes through NewEndpoint.
            }
            Err(e) => on_registered(Err(DeviceError::Failed(e.to_string()))),
        }
    });
}

pub(crate) fn unregister(dir: &Path) {
    let (Some(bus), Some(name)) = (session(), distributor()) else {
        return;
    };
    let Ok(token) = token(dir) else {
        return;
    };
    let args = glib::VariantDict::new(None);
    args.insert("token", token);
    bus.call(
        Some(&name),
        DISTRIBUTOR_PATH,
        DISTRIBUTOR,
        "Unregister",
        Some(&glib::Variant::tuple_from_iter([args.end()])),
        None,
        gio::DBusCallFlags::NONE,
        5000,
        None::<&gio::Cancellable>,
        |_| {},
    );
    let _ = std::fs::remove_file(dir.join("prism-push-token"));
}

/// A decrypted message: Prism's payload JSON, or plain text as the body.
fn message(text: &str) -> PushMessage {
    let mut m = PushMessage::from_payload(PushEvent::Received, text)
        .map(|(m, _)| m)
        .unwrap_or_else(|| {
            let mut m = PushMessage::new(PushEvent::Received);
            m.body = Some(text.to_string());
            m
        });
    m.service = Some(PushService::UnifiedPush);
    m
}
