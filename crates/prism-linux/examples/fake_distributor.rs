//! A local UnifiedPush distributor for tests (D-Bus spec DBUS_0.3.0), no
//! server: it owns `org.unifiedpush.Distributor.fake`, answers `Register`
//! with a made-up endpoint (`NewEndpoint`), then sends one encrypted Web Push
//! message to the app, as a real distributor relays one from its server.
//!
//! ```sh
//! dbus-run-session -- sh -c '
//!   cargo run -p prism-linux --example fake_distributor -- ~/.local/share/Prism/prism-push-keys &
//!   PRISM_PATH=/settings PRISM_CLICK="Enable push" PRISM_AFTER=4 PRISM_SNAPSHOT=/tmp/push.png xvfb-run -a counter'
//! ```
//!
//! The message is encrypted for the keys in the file given (the app's), with
//! `prism-push`: the app decrypts it like any UnifiedPush message.

#[cfg(target_os = "linux")]
fn main() {
    use gtk::{gio, glib};

    const XML: &str = r#"<node><interface name="org.unifiedpush.Distributor2">
      <method name="Register"><arg type="a{sv}" direction="in"/><arg type="a{sv}" direction="out"/></method>
      <method name="Unregister"><arg type="a{sv}" direction="in"/><arg type="a{sv}" direction="out"/></method>
    </interface></node>"#;
    let keys_path = std::env::args()
        .nth(1)
        .expect("usage: fake_distributor <app keys file>");
    let main = glib::MainLoop::new(None, false);
    let bus =
        gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).expect("session bus");
    let node = gio::DBusNodeInfo::for_xml(XML).unwrap();
    let iface = node
        .lookup_interface("org.unifiedpush.Distributor2")
        .unwrap();
    let _id = bus
        .register_object("/org/unifiedpush/Distributor", &iface)
        .method_call(move |conn, _sender, _, _, method, params, invocation| {
            let args = glib::VariantDict::new(Some(&params.child_value(0)));
            let token = args.lookup::<String>("token").ok().flatten().unwrap_or_default();
            let service = args.lookup::<String>("service").ok().flatten().unwrap_or_default();
            eprintln!("fake-distributor: {method} service={service} token={token}");
            let out = glib::VariantDict::new(None);
            if method == "Register" {
                out.insert("success", "REGISTRATION_SUCCEEDED");
            }
            invocation.return_value(Some(&glib::Variant::tuple_from_iter([out.end()])));
            if method != "Register" {
                return;
            }
            let keys_path = keys_path.clone();
            let conn = conn.clone();
            glib::MainContext::default().spawn_local(async move {
                let call = |method: &'static str, dict: glib::VariantDict| {
                    conn.call_future(
                        Some(&service),
                        "/org/unifiedpush/Connector",
                        "org.unifiedpush.Connector2",
                        method,
                        Some(&glib::Variant::tuple_from_iter([dict.end()])),
                        None,
                        gio::DBusCallFlags::NONE,
                        5000,
                    )
                };
                let d = glib::VariantDict::new(None);
                d.insert("token", &token);
                d.insert("endpoint", "https://push.example.invalid/up/fake");
                eprintln!("fake-distributor: NewEndpoint {:?}", call("NewEndpoint", d).await.map(|_| ()));
                glib::timeout_future(std::time::Duration::from_millis(800)).await;
                // The app made its keys when it built the subscription.
                let keys = std::fs::read(&keys_path)
                    .ok()
                    .and_then(|b| prism_push::webpush::ReceiverKeys::from_bytes(&b))
                    .expect("the app's keys");
                let payload = br#"{"title":"Order shipped","body":"Through UnifiedPush over D-Bus","link":"/push"}"#;
                let body = prism_push::webpush::encrypt(
                    payload,
                    keys.secret.public_key().to_sec1_bytes().as_ref(),
                    &keys.auth,
                )
                .unwrap();
                let d = glib::VariantDict::new(None);
                d.insert("token", &token);
                d.insert("message", body);
                d.insert("id", "m1");
                eprintln!("fake-distributor: Message {:?}", call("Message", d).await.map(|v| v.to_string()));
            });
        })
        .build()
        .expect("register the distributor object");
    // RequestName directly (gio's owner callbacks panic when the bus goes away).
    let owned = bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "RequestName",
        Some(&("org.unifiedpush.Distributor.fake", 0u32).into()),
        Some(glib::VariantTy::new("(u)").unwrap()),
        gio::DBusCallFlags::NONE,
        -1,
        None::<&gio::Cancellable>,
    );
    eprintln!("fake-distributor: owns org.unifiedpush.Distributor.fake ({owned:?})");
    main.run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fake_distributor: Linux only (D-Bus)");
}
