//! The tray thread: each tray is a StatusNotifierItem (with its menu) on a
//! session bus connection of its own, served from a GLib main loop on a
//! thread of its own, so a slow panel never blocks GTK's.
//!
//! As in KDE's own items: the item owns the name
//! `org.kde.StatusNotifierItem-<pid>-<n>`, serves `/StatusNotifierItem` and
//! `/MenuBar`, registers with `org.kde.StatusNotifierWatcher`, and registers
//! again whenever a watcher (the panel) starts.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use gtk::gio::{self, prelude::*};
use gtk::glib::{self, Variant, VariantTy};

use super::dbusmenu::{self, Item, Menu};

pub(crate) const ITEM_PATH: &str = "/StatusNotifierItem";
pub(crate) const MENU_PATH: &str = "/MenuBar";
pub(crate) const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
pub(crate) const MENU_INTERFACE: &str = "com.canonical.dbusmenu";
pub(crate) const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

const ITEM_XML: &str = r#"<node>
  <interface name="org.kde.StatusNotifierItem">
    <method name="ContextMenu">
      <arg name="x" type="i" direction="in"/>
      <arg name="y" type="i" direction="in"/>
    </method>
    <method name="Activate">
      <arg name="x" type="i" direction="in"/>
      <arg name="y" type="i" direction="in"/>
    </method>
    <method name="SecondaryActivate">
      <arg name="x" type="i" direction="in"/>
      <arg name="y" type="i" direction="in"/>
    </method>
    <method name="Scroll">
      <arg name="delta" type="i" direction="in"/>
      <arg name="orientation" type="s" direction="in"/>
    </method>
    <method name="ProvideXdgActivationToken">
      <arg name="token" type="s" direction="in"/>
    </method>
    <signal name="NewTitle"/>
    <signal name="NewIcon"/>
    <signal name="NewAttentionIcon"/>
    <signal name="NewOverlayIcon"/>
    <signal name="NewToolTip"/>
    <signal name="NewStatus">
      <arg name="status" type="s"/>
    </signal>
    <property name="Category" type="s" access="read"/>
    <property name="Id" type="s" access="read"/>
    <property name="Title" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="WindowId" type="i" access="read"/>
    <property name="IconThemePath" type="s" access="read"/>
    <property name="Menu" type="o" access="read"/>
    <property name="ItemIsMenu" type="b" access="read"/>
    <property name="IconName" type="s" access="read"/>
    <property name="IconPixmap" type="a(iiay)" access="read"/>
    <property name="OverlayIconName" type="s" access="read"/>
    <property name="OverlayIconPixmap" type="a(iiay)" access="read"/>
    <property name="AttentionIconName" type="s" access="read"/>
    <property name="AttentionIconPixmap" type="a(iiay)" access="read"/>
    <property name="AttentionMovieName" type="s" access="read"/>
    <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
  </interface>
</node>"#;

/// What a tray shows, made on GTK's thread and handed to the tray thread.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tray {
    pub id: String,
    pub title: String,
    pub tooltip: String,
    /// A name in the desktop's icon theme.
    pub icon: String,
    pub items: Vec<Item>,
}

/// What the panel asks of the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// A click on the icon, with the XDG activation token the panel sent
    /// just before (Wayland: lets the window come forward).
    Activate { token: Option<String> },
    /// A click on a menu item: its `MenuSelect` id.
    Select(u64),
}

/// Where actions go; called on the tray thread.
pub(crate) type Sink = Arc<dyn Fn(Action) + Send + Sync>;

/// Shows the tray `key`, or updates it.
pub(crate) fn set(key: String, tray: Tray, sink: Sink) {
    thread().invoke(move || set_here(key, tray, sink));
}

/// Takes the tray `key` off the bus.
pub(crate) fn remove(key: String) {
    thread().invoke(move || remove_here(&key));
}

/// The tray thread's main context, started on first use.
fn thread() -> &'static glib::MainContext {
    static CONTEXT: OnceLock<glib::MainContext> = OnceLock::new();
    CONTEXT.get_or_init(|| {
        let context = glib::MainContext::new();
        let ours = context.clone();
        let (ready, started) = std::sync::mpsc::sync_channel(0);
        std::thread::Builder::new()
            .name("prism-tray".into())
            .spawn(move || {
                // Thread-default for the thread's whole life: D-Bus calls and
                // the objects registered here are dispatched to it.
                let _ = ours.with_thread_default(|| {
                    let main_loop = glib::MainLoop::new(Some(&ours), false);
                    let _ = ready.send(());
                    main_loop.run();
                });
            })
            .expect("prism-linux: tray thread");
        // Once the thread owns the context, `invoke` runs closures there (on
        // a context nobody owns, it would run them on the caller's thread).
        let _ = started.recv();
        context
    })
}

// --- On the tray thread -------------------------------------------------------------

struct Served {
    /// The bus name the item registers with.
    name: String,
    tray: Tray,
    menu: Menu,
    revision: u32,
    sink: Sink,
    /// The last XDG activation token, for the next activation.
    token: Option<String>,
    bus: Option<Bus>,
    removed: bool,
}

struct Bus {
    conn: gio::DBusConnection,
    objects: Vec<gio::RegistrationId>,
    /// `NameOwnerChanged` of the watcher; unsubscribes when dropped.
    _watcher: gio::SignalSubscription,
}

thread_local! {
    static SERVED: RefCell<HashMap<String, Rc<RefCell<Served>>>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u32> = const { Cell::new(1) };
}

fn set_here(key: String, tray: Tray, sink: Sink) {
    if let Some(served) = SERVED.with(|s| s.borrow().get(&key).cloned()) {
        return update(&served, tray);
    }
    let n = NEXT.with(|n| n.replace(n.get() + 1));
    let served = Rc::new(RefCell::new(Served {
        name: format!("org.kde.StatusNotifierItem-{}-{n}", std::process::id()),
        menu: Menu::new(&tray.items),
        tray,
        revision: 0,
        sink,
        token: None,
        bus: None,
        removed: false,
    }));
    SERVED.with(|s| s.borrow_mut().insert(key.clone(), served.clone()));
    glib::MainContext::ref_thread_default().spawn_local(async move {
        if let Err(e) = start(&served, &key).await
            && !served.borrow().removed
        {
            eprintln!("prism-linux: tray {key:?} not shown: {e}");
        }
    });
}

fn update(served: &Rc<RefCell<Served>>, tray: Tray) {
    let mut s = served.borrow_mut();
    let old = std::mem::replace(&mut s.tray, tray);
    let menu_changed = old.items != s.tray.items;
    if menu_changed {
        s.menu = s.menu.next(&s.tray.items);
        s.revision += 1;
    }
    let Some(bus) = &s.bus else {
        // Not on the bus yet: the panel reads the new values when it is.
        return;
    };
    let emit = |path: &str, interface: &str, signal: &str, args: Option<&Variant>| {
        let _ = bus.conn.emit_signal(None, path, interface, signal, args);
    };
    if old.title != s.tray.title {
        emit(ITEM_PATH, ITEM_INTERFACE, "NewTitle", None);
    }
    if old.icon != s.tray.icon {
        emit(ITEM_PATH, ITEM_INTERFACE, "NewIcon", None);
    }
    if old.tooltip != s.tray.tooltip {
        emit(ITEM_PATH, ITEM_INTERFACE, "NewToolTip", None);
    }
    if menu_changed {
        let args = (s.revision, 0i32).to_variant();
        emit(MENU_PATH, MENU_INTERFACE, "LayoutUpdated", Some(&args));
    }
}

fn remove_here(key: &str) {
    let Some(served) = SERVED.with(|s| s.borrow_mut().remove(key)) else {
        return;
    };
    let mut s = served.borrow_mut();
    s.removed = true;
    if let Some(bus) = s.bus.take() {
        for id in bus.objects {
            let _ = bus.conn.unregister_object(id);
        }
        // The bus then drops the item's name, and the watcher the item.
        bus.conn.close(gio::Cancellable::NONE, |_| ());
    }
}

/// Connects, serves the item, takes its name and registers it.
async fn start(served: &Rc<RefCell<Served>>, key: &str) -> Result<(), glib::Error> {
    let address =
        gio::dbus_address_get_for_bus_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    let conn = gio::DBusConnection::for_address_future(
        &address,
        gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
            | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
        None,
    )
    .await?;
    if served.borrow().removed {
        conn.close(gio::Cancellable::NONE, |_| ());
        return Ok(());
    }
    let objects = serve(&conn, served)?;
    let name = served.borrow().name.clone();
    // The panel (re)started: register again.
    let watcher = conn.subscribe_to_signal(
        Some(DBUS),
        Some(DBUS),
        Some("NameOwnerChanged"),
        Some(DBUS_PATH),
        Some(WATCHER),
        gio::DBusSignalFlags::NONE,
        {
            let name = name.clone();
            move |signal| {
                let owner = signal
                    .parameters
                    .get::<(String, String, String)>()
                    .map(|a| a.2);
                if owner.is_some_and(|o| !o.is_empty()) {
                    let (conn, name) = (signal.connection.clone(), name.clone());
                    glib::MainContext::ref_thread_default().spawn_local(async move {
                        if let Err(e) = register(&conn, &name).await {
                            eprintln!("prism-linux: tray {name} not registered: {e}");
                        }
                    });
                }
            }
        },
    );
    served.borrow_mut().bus = Some(Bus {
        conn: conn.clone(),
        objects,
        _watcher: watcher,
    });
    // DO_NOT_QUEUE: the name has the pid and a counter, so it is free.
    let owned = conn
        .call_future(
            Some(DBUS),
            DBUS_PATH,
            DBUS,
            "RequestName",
            Some(&(name.as_str(), 4u32).to_variant()),
            Some(VariantTy::new("(u)").expect("type")),
            gio::DBusCallFlags::NONE,
            -1,
        )
        .await?;
    if owned.get::<(u32,)>() != Some((1,)) {
        return Err(glib::Error::new(
            gio::IOErrorEnum::Exists,
            &format!("{name} is taken"),
        ));
    }
    if let Err(e) = register(&conn, &name).await
        && !served.borrow().removed
    {
        // No StatusNotifierWatcher (no panel showing trays): the item
        // registers when one starts.
        eprintln!("prism-linux: tray {key:?} not shown until a panel shows trays: {e}");
    }
    Ok(())
}

async fn register(conn: &gio::DBusConnection, name: &str) -> Result<(), glib::Error> {
    conn.call_future(
        Some(WATCHER),
        "/StatusNotifierWatcher",
        WATCHER,
        "RegisterStatusNotifierItem",
        Some(&(name,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        -1,
    )
    .await
    .map(|_| ())
}

fn interface(xml: &str, name: &str) -> gio::DBusInterfaceInfo {
    gio::DBusNodeInfo::for_xml(xml)
        .ok()
        .and_then(|node| node.lookup_interface(name))
        .expect("prism-linux: tray introspection data")
}

/// Serves the item and its menu. The handlers hold the tray until
/// `remove_here` unregisters them.
fn serve(
    conn: &gio::DBusConnection,
    served: &Rc<RefCell<Served>>,
) -> Result<Vec<gio::RegistrationId>, glib::Error> {
    let item = conn
        .register_object(ITEM_PATH, &interface(ITEM_XML, ITEM_INTERFACE))
        .method_call({
            let served = served.clone();
            move |_, _, _, _, method, args, call| item_call(&served, method, &args, call)
        })
        .property({
            let served = served.clone();
            move |_, _, _, _, name| item_property(&served.borrow().tray, name)
        })
        .build()?;
    let menu = conn
        .register_object(MENU_PATH, &interface(dbusmenu::XML, MENU_INTERFACE))
        .method_call({
            let served = served.clone();
            move |_, _, _, _, method, args, call| {
                let (answer, sink) = {
                    let s = served.borrow();
                    (s.menu.call(s.revision, method, &args), s.sink.clone())
                };
                match answer.reply {
                    Ok(reply) => call.return_value(reply.as_ref()),
                    Err((error, message)) => call.return_dbus_error(error, &message),
                }
                for id in answer.clicked {
                    sink(Action::Select(id));
                }
            }
        })
        .property(|_, _, _, _, name| dbusmenu::property(name))
        .build();
    match menu {
        Ok(menu) => Ok(vec![item, menu]),
        Err(e) => {
            let _ = conn.unregister_object(item);
            Err(e)
        }
    }
}

fn item_call(
    served: &Rc<RefCell<Served>>,
    method: &str,
    args: &Variant,
    call: gio::DBusMethodInvocation,
) {
    match method {
        "Activate" | "SecondaryActivate" | "Scroll" => {
            // A token is for the activation that follows it.
            let (token, sink) = {
                let mut s = served.borrow_mut();
                (s.token.take(), s.sink.clone())
            };
            call.return_value(None);
            if method == "Activate" {
                sink(Action::Activate { token });
            }
        }
        "ProvideXdgActivationToken" => {
            served.borrow_mut().token = args.get::<(String,)>().map(|t| t.0);
            call.return_value(None);
        }
        // Panels then show the menu at `Menu` themselves.
        _ => call.return_dbus_error(
            "org.freedesktop.DBus.Error.UnknownMethod",
            "the menu is com.canonical.dbusmenu at /MenuBar",
        ),
    }
}

/// No pixmaps: the icon is a name in the theme (`a(iiay)`).
fn no_pixmaps() -> Vec<(i32, i32, Vec<u8>)> {
    Vec::new()
}

fn item_property(tray: &Tray, name: &str) -> Variant {
    match name {
        "Category" => "ApplicationStatus".to_variant(),
        "Id" => tray.id.to_variant(),
        "Title" => tray.title.to_variant(),
        "Status" => "Active".to_variant(),
        "WindowId" => 0i32.to_variant(),
        "Menu" => glib::variant::ObjectPath::try_from(MENU_PATH)
            .expect("object path")
            .to_variant(),
        "ItemIsMenu" => false.to_variant(),
        "IconName" => tray.icon.to_variant(),
        "IconPixmap" | "OverlayIconPixmap" | "AttentionIconPixmap" => no_pixmaps().to_variant(),
        "ToolTip" => (
            String::new(),
            no_pixmaps(),
            tray.tooltip.clone(),
            String::new(),
        )
            .to_variant(),
        // IconThemePath, OverlayIconName, AttentionIconName, AttentionMovieName.
        _ => "".to_variant(),
    }
}
