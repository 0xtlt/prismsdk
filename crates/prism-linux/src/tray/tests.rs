//! The menu's answers, then a tray served on a real session bus to a mock
//! StatusNotifierWatcher. The bus test needs `DBUS_SESSION_BUS_ADDRESS`
//! (in the dev image: `dbus-run-session -- cargo test -p prismsdk-linux`)
//! and is skipped without it.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use gtk::gio::{self, prelude::*};
use gtk::glib::{self, Variant};

use super::dbusmenu::{Item, Menu};
use super::item::{
    self, Action, ITEM_INTERFACE, ITEM_PATH, MENU_INTERFACE, MENU_PATH, Tray, WATCHER,
};

fn action(id: u64, title: &str) -> Item {
    Item::Action {
        id,
        title: title.into(),
        enabled: true,
        checked: None,
    }
}

fn sample() -> Vec<Item> {
    vec![
        action(41, "Buy_milk"),
        Item::Separator,
        Item::Action {
            id: 42,
            title: "Done".into(),
            enabled: true,
            checked: Some(true),
        },
        Item::Sub {
            title: "More".into(),
            items: vec![action(43, "Deep")],
        },
        Item::Action {
            id: 44,
            title: "Off".into(),
            enabled: false,
            checked: None,
        },
    ]
}

/// `(id, properties, children)` of a `(ia{sv}av)` layout.
fn node(layout: &Variant) -> (i32, glib::VariantDict, Vec<Variant>) {
    let id = layout.child_value(0).get::<i32>().expect("id");
    let props = glib::VariantDict::new(Some(&layout.child_value(1)));
    let children = layout
        .child_value(2)
        .iter()
        .map(|c| c.as_variant().expect("child"))
        .collect();
    (id, props, children)
}

fn label(props: &glib::VariantDict) -> Option<String> {
    props.lookup::<String>("label").ok().flatten()
}

fn layout(menu: &Menu, depth: i32) -> Variant {
    let args = (0i32, depth, Vec::<String>::new()).to_variant();
    let reply = menu.call(7, "GetLayout", &args).reply.expect("GetLayout");
    let reply = reply.expect("arguments");
    assert_eq!(reply.type_().as_str(), "(u(ia{sv}av))");
    assert_eq!(reply.child_value(0).get::<u32>(), Some(7));
    reply.child_value(1)
}

fn click(menu: &Menu, id: i32) -> Result<Vec<u64>, String> {
    let args = (id, "clicked", 0i32.to_variant(), 0u32).to_variant();
    let answer = menu.call(0, "Event", &args);
    answer
        .reply
        .map(|_| answer.clicked)
        .map_err(|e| e.0.to_string())
}

#[test]
fn menu_layout_has_every_item_with_its_properties() {
    let menu = Menu::new(&sample());
    let (root, props, items) = node(&layout(&menu, -1));
    assert_eq!(root, 0);
    assert_eq!(
        props
            .lookup::<String>("children-display")
            .ok()
            .flatten()
            .as_deref(),
        Some("submenu")
    );
    assert_eq!(items.len(), 5);

    let (_, buy, _) = node(&items[0]);
    // An underscore of the title's own is doubled (single ones mark access keys).
    assert_eq!(label(&buy).as_deref(), Some("Buy__milk"));
    assert!(
        buy.lookup_value("enabled", None).is_none(),
        "defaults are left out"
    );

    let (_, separator, _) = node(&items[1]);
    assert_eq!(
        separator.lookup::<String>("type").ok().flatten().as_deref(),
        Some("separator")
    );

    let (_, done, _) = node(&items[2]);
    assert_eq!(
        done.lookup::<String>("toggle-type")
            .ok()
            .flatten()
            .as_deref(),
        Some("checkmark")
    );
    assert_eq!(done.lookup::<i32>("toggle-state").ok().flatten(), Some(1));

    let (_, more, deep) = node(&items[3]);
    assert_eq!(
        more.lookup::<String>("children-display")
            .ok()
            .flatten()
            .as_deref(),
        Some("submenu")
    );
    assert_eq!(deep.len(), 1);
    assert_eq!(label(&node(&deep[0]).1).as_deref(), Some("Deep"));

    let (_, off, _) = node(&items[4]);
    assert_eq!(off.lookup::<bool>("enabled").ok().flatten(), Some(false));

    // One level only: the submenu's items are left out.
    let (_, _, shallow) = node(&layout(&menu, 1));
    assert!(node(&shallow[3]).2.is_empty());
}

#[test]
fn menu_clicks_report_their_item() {
    let menu = Menu::new(&sample());
    let (_, _, items) = node(&layout(&menu, -1));
    let id = |i: usize| node(&items[i]).0;
    assert_eq!(click(&menu, id(0)), Ok(vec![41]));
    assert_eq!(click(&menu, id(2)), Ok(vec![42]));
    let deep = node(&node(&items[3]).2[0]).0;
    assert_eq!(click(&menu, deep), Ok(vec![43]));
    // Separators, submenus and disabled items report nothing.
    assert_eq!(click(&menu, id(1)), Ok(vec![]));
    assert_eq!(click(&menu, id(3)), Ok(vec![]));
    assert_eq!(click(&menu, id(4)), Ok(vec![]));
    // Hovering is not clicking.
    let hover = (id(0), "hovered", 0i32.to_variant(), 0u32).to_variant();
    assert!(menu.call(0, "Event", &hover).clicked.is_empty());
    assert!(click(&menu, 999).is_err());
}

#[test]
fn a_new_menu_refuses_the_old_ids() {
    let old = Menu::new(&sample());
    let (_, _, old_items) = node(&layout(&old, -1));
    let new = old.next(&sample()[1..]);
    let (_, _, new_items) = node(&layout(&new, -1));
    let old_ids: Vec<i32> = old_items.iter().map(|i| node(i).0).collect();
    for item in &new_items {
        assert!(!old_ids.contains(&node(item).0));
    }
    assert!(click(&new, old_ids[0]).is_err());
    assert_eq!(click(&new, node(&new_items[1]).0), Ok(vec![42]));
}

#[test]
fn menu_group_calls() {
    let menu = Menu::new(&sample());
    let (_, _, items) = node(&layout(&menu, -1));
    let first = node(&items[0]).0;

    let args = (vec![first, 999], vec!["label".to_string()]).to_variant();
    let reply = menu
        .call(0, "GetGroupProperties", &args)
        .reply
        .unwrap()
        .unwrap();
    assert_eq!(reply.type_().as_str(), "(a(ia{sv}))");
    let entries = reply.child_value(0);
    assert_eq!(entries.n_children(), 1);
    assert_eq!(
        entries.child_value(0).child_value(0).get::<i32>(),
        Some(first)
    );

    let args = (first, "label").to_variant();
    let reply = menu.call(0, "GetProperty", &args).reply.unwrap().unwrap();
    assert_eq!(
        reply
            .child_value(0)
            .as_variant()
            .and_then(|v| v.get::<String>())
            .as_deref(),
        Some("Buy__milk")
    );

    let data = 0i32.to_variant();
    let events = vec![
        (first, "clicked".to_string(), data.clone(), 0u32),
        (999, "clicked".to_string(), data, 0u32),
    ];
    let answer = menu.call(0, "EventGroup", &(events,).to_variant());
    assert_eq!(answer.clicked, vec![41]);
    assert_eq!(
        answer.reply.unwrap().unwrap().get::<(Vec<i32>,)>(),
        Some((vec![999],))
    );

    let reply = menu
        .call(0, "AboutToShowGroup", &(vec![0, 999],).to_variant())
        .reply
        .unwrap()
        .unwrap();
    assert_eq!(
        reply.get::<(Vec<i32>, Vec<i32>)>(),
        Some((vec![], vec![999]))
    );
    let reply = menu
        .call(0, "AboutToShow", &(0i32,).to_variant())
        .reply
        .unwrap()
        .unwrap();
    assert_eq!(reply.get::<(bool,)>(), Some((false,)));
}

// --- On a session bus ---------------------------------------------------------------

const WATCHER_XML: &str = r#"<node>
  <interface name="org.kde.StatusNotifierWatcher">
    <method name="RegisterStatusNotifierItem">
      <arg name="service" type="s" direction="in"/>
    </method>
    <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
    <property name="ProtocolVersion" type="i" access="read"/>
  </interface>
</node>"#;

/// Runs the test thread's main context until `done` (or 10 s).
fn pump(context: &glib::MainContext, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        if done() {
            return true;
        }
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(10));
    }
    done()
}

fn connect() -> gio::DBusConnection {
    let address = gio::dbus_address_get_for_bus_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .expect("session bus address");
    gio::DBusConnection::for_address_sync(
        &address,
        gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
            | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
        None,
        gio::Cancellable::NONE,
    )
    .expect("session bus")
}

fn bus_call(conn: &gio::DBusConnection, method: &str, args: Variant) -> Variant {
    conn.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        method,
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        5000,
        gio::Cancellable::NONE,
    )
    .expect(method)
}

fn call(
    conn: &gio::DBusConnection,
    name: &str,
    path: &str,
    iface: &str,
    method: &str,
    args: Option<Variant>,
) -> Result<Variant, glib::Error> {
    conn.call_sync(
        Some(name),
        path,
        iface,
        method,
        args.as_ref(),
        None,
        gio::DBusCallFlags::NONE,
        5000,
        gio::Cancellable::NONE,
    )
}

fn property(conn: &gio::DBusConnection, name: &str, prop: &str) -> Variant {
    let reply = call(
        conn,
        name,
        ITEM_PATH,
        "org.freedesktop.DBus.Properties",
        "Get",
        Some((ITEM_INTERFACE, prop).to_variant()),
    )
    .expect(prop);
    reply.child_value(0).as_variant().expect("v")
}

fn tray(title: &str, items: Vec<Item>) -> Tray {
    Tray {
        id: "prism-test".into(),
        title: title.into(),
        tooltip: format!("{title} open"),
        icon: "checklist".into(),
        items,
    }
}

#[test]
fn serves_a_tray_to_the_watcher() {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        eprintln!("skipped: no session bus (run under dbus-run-session)");
        return;
    }
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| on_the_bus(&context))
        .expect("thread-default context");
}

fn on_the_bus(context: &glib::MainContext) {
    let (sent, actions) = mpsc::channel();
    let sink: item::Sink = Arc::new(move |a: Action| {
        let _ = sent.send(a);
    });
    let expect = |want: Action| {
        assert_eq!(actions.recv_timeout(Duration::from_secs(5)), Ok(want));
    };

    // The tray starts before any panel: it takes its name, fails to
    // register, and waits for one.
    item::set("test".into(), tray("2", sample()), sink.clone());
    let watcher = connect();
    let prefix = format!("org.kde.StatusNotifierItem-{}-", std::process::id());
    let ours = || {
        let names = bus_call(&watcher, "ListNames", ().to_variant());
        let names = names.get::<(Vec<String>,)>().expect("names").0;
        names.into_iter().find(|n| n.starts_with(&prefix))
    };
    assert!(
        pump(context, || ours().is_some()),
        "the item takes its name"
    );
    let name = ours().unwrap();
    let has_owner = |name: &str| {
        bus_call(&watcher, "NameHasOwner", (name,).to_variant()).get::<(bool,)>() == Some((true,))
    };
    std::thread::sleep(Duration::from_millis(300));

    // A watcher: records the items that register.
    let registered: Rc<RefCell<Vec<String>>> = Rc::default();
    let info = gio::DBusNodeInfo::for_xml(WATCHER_XML).unwrap();
    let iface = info.lookup_interface(WATCHER).unwrap();
    let _object = watcher
        .register_object("/StatusNotifierWatcher", &iface)
        .method_call({
            let registered = registered.clone();
            move |_, _, _, _, _, args, call| {
                registered
                    .borrow_mut()
                    .push(args.get::<(String,)>().unwrap().0);
                call.return_value(None);
            }
        })
        .property(|_, _, _, _, name| match name {
            "IsStatusNotifierHostRegistered" => true.to_variant(),
            _ => 0i32.to_variant(),
        })
        .build()
        .unwrap();
    let own = || bus_call(&watcher, "RequestName", (WATCHER, 4u32).to_variant());
    assert_eq!(own().get::<(u32,)>(), Some((1,)));
    assert!(
        pump(context, || registered.borrow().len() == 1),
        "the item registers once a watcher runs"
    );
    assert_eq!(registered.borrow()[0], name);

    // Its properties, every one of the right type (GDBus checks them).
    let client = connect();
    let all = call(
        &client,
        &name,
        ITEM_PATH,
        "org.freedesktop.DBus.Properties",
        "GetAll",
        Some((ITEM_INTERFACE,).to_variant()),
    )
    .unwrap();
    assert_eq!(all.child_value(0).n_children(), 16);
    assert_eq!(
        property(&client, &name, "Id").get::<String>().as_deref(),
        Some("prism-test")
    );
    assert_eq!(
        property(&client, &name, "Title").get::<String>().as_deref(),
        Some("2")
    );
    assert_eq!(
        property(&client, &name, "IconName")
            .get::<String>()
            .as_deref(),
        Some("checklist")
    );
    assert_eq!(
        property(&client, &name, "Status")
            .get::<String>()
            .as_deref(),
        Some("Active")
    );
    assert_eq!(
        property(&client, &name, "ItemIsMenu").get::<bool>(),
        Some(false)
    );
    assert_eq!(
        property(&client, &name, "Menu")
            .get::<glib::variant::ObjectPath>()
            .as_deref(),
        Some(MENU_PATH)
    );
    let tip = property(&client, &name, "ToolTip");
    assert_eq!(tip.type_().as_str(), "(sa(iiay)ss)");
    assert_eq!(
        tip.child_value(2).get::<String>().as_deref(),
        Some("2 open")
    );

    // A click on the icon, with the token the panel sends first.
    let item_call = |method: &str, args: Variant| {
        call(
            &client,
            &name,
            ITEM_PATH,
            ITEM_INTERFACE,
            method,
            Some(args),
        )
    };
    item_call("ProvideXdgActivationToken", ("token-1",).to_variant()).unwrap();
    item_call("Activate", (10i32, 20i32).to_variant()).unwrap();
    expect(Action::Activate {
        token: Some("token-1".into()),
    });
    item_call("Activate", (10i32, 20i32).to_variant()).unwrap();
    expect(Action::Activate { token: None });
    // The panel shows the menu itself.
    assert!(item_call("ContextMenu", (0i32, 0i32).to_variant()).is_err());

    // The menu, then a click on its first item.
    let menu_call = |method: &str, args: Variant| {
        call(
            &client,
            &name,
            MENU_PATH,
            MENU_INTERFACE,
            method,
            Some(args),
        )
    };
    let reply = menu_call(
        "GetLayout",
        (0i32, -1i32, Vec::<String>::new()).to_variant(),
    )
    .unwrap();
    assert_eq!(reply.child_value(0).get::<u32>(), Some(0));
    let (_, _, items) = node(&reply.child_value(1));
    assert_eq!(items.len(), 5);
    let first = node(&items[0]).0;
    let clicked = |id: i32| {
        menu_call(
            "Event",
            (id, "clicked", 0i32.to_variant(), 0u32).to_variant(),
        )
    };
    clicked(first).unwrap();
    expect(Action::Select(41));
    let version = call(
        &client,
        &name,
        MENU_PATH,
        "org.freedesktop.DBus.Properties",
        "Get",
        Some((MENU_INTERFACE, "Version").to_variant()),
    )
    .unwrap();
    assert_eq!(
        version
            .child_value(0)
            .as_variant()
            .and_then(|v| v.get::<u32>()),
        Some(3)
    );

    // An update: new title and menu, announced with signals.
    let signals: Rc<RefCell<Vec<String>>> = Rc::default();
    let _subscription =
        client.subscribe_to_signal(None, None, None, None, None, gio::DBusSignalFlags::NONE, {
            let signals = signals.clone();
            move |s| {
                if s.object_path == ITEM_PATH || s.object_path == MENU_PATH {
                    signals.borrow_mut().push(format!(
                        "{} {}",
                        s.signal_name,
                        s.parameters.print(false)
                    ));
                }
            }
        });
    item::set(
        "test".into(),
        tray("1", sample()[1..].to_vec()),
        sink.clone(),
    );
    assert!(
        pump(context, || signals.borrow().len() >= 3),
        "{:?}",
        signals.borrow()
    );
    let seen = signals.borrow().clone();
    assert!(seen.contains(&"NewTitle ()".to_string()), "{seen:?}");
    assert!(seen.contains(&"NewToolTip ()".to_string()), "{seen:?}");
    assert!(
        seen.contains(&"LayoutUpdated (1, 0)".to_string()),
        "{seen:?}"
    );
    assert!(!seen.iter().any(|s| s.starts_with("NewIcon")), "{seen:?}");
    assert_eq!(
        property(&client, &name, "Title").get::<String>().as_deref(),
        Some("1")
    );
    // The old menu's ids are refused, the new ones work.
    assert!(clicked(first).is_err());
    let reply = menu_call(
        "GetLayout",
        (0i32, -1i32, Vec::<String>::new()).to_variant(),
    )
    .unwrap();
    assert_eq!(reply.child_value(0).get::<u32>(), Some(1));
    let (_, _, items) = node(&reply.child_value(1));
    clicked(node(&items[1]).0).unwrap();
    expect(Action::Select(42));

    // The panel restarts: the item registers again.
    bus_call(&watcher, "ReleaseName", (WATCHER,).to_variant());
    assert_eq!(own().get::<(u32,)>(), Some((1,)));
    assert!(
        pump(context, || registered.borrow().len() == 2),
        "the item registers with the new watcher"
    );
    assert_eq!(registered.borrow()[1], name);

    // Removed: its name leaves the bus.
    item::remove("test".into());
    assert!(
        pump(context, || !has_owner(&name)),
        "the item's name is released"
    );
    assert!(actions.try_recv().is_err(), "no other action");
}
