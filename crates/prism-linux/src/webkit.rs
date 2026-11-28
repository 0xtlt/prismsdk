//! WebKitGTK 6.0 through its C API (`webkit6-sys`), for `web.rs`: only
//! what Prism's web views call.
//!
//! The `webkit6` crate would be the safe binding, but its `WebView`
//! implements `gtk::Accessible`, which gtk4-rs only binds from GTK 4.10
//! (its `v4_10` feature, which needs GTK 4.10 to build): Prism supports
//! GTK 4.8 (Debian 12), whose libwebkitgtk-6.0 works. So the objects are
//! plain `glib::Object`s (the view a `gtk::Widget`), their signals
//! connected by name, their methods called through the C API, as the
//! accessibility code does for the same reason.

use std::ffi::c_char;
use std::ptr;

use gtk::glib::translate::{FromGlib, FromGlibPtrFull, FromGlibPtrNone, ToGlibPtr, ToGlibPtrMut};
use gtk::glib::{self, Object, Value, gobject_ffi};
use gtk::prelude::*;
use javascriptcore6_sys as jsc;
use webkit6_sys as ffi;

/// A `WebKitWebContext`: web processes and URL schemes, shared by views.
#[derive(Clone)]
pub struct WebContext(Object);

impl WebContext {
    pub fn new() -> WebContext {
        // SAFETY: returns a new reference.
        WebContext(unsafe { Object::from_glib_full(ffi::webkit_web_context_new() as *mut _) })
    }

    /// The C object, for calls not wrapped here (`register_uri_scheme`).
    #[allow(dead_code)]
    pub fn as_ptr(&self) -> *mut ffi::WebKitWebContext {
        self.0.as_ptr() as *mut _
    }
}

/// A `WebKitNetworkSession`: cookies, storage, cache.
#[derive(Clone)]
pub struct NetworkSession(Object);

impl NetworkSession {
    /// Kept in `data` and `cache`.
    pub fn new(data: &str, cache: &str) -> NetworkSession {
        // SAFETY: valid strings; returns a new reference.
        NetworkSession(unsafe {
            Object::from_glib_full(ffi::webkit_network_session_new(
                data.to_glib_none().0,
                cache.to_glib_none().0,
            ) as *mut _)
        })
    }

    /// Kept in memory only, for one view.
    pub fn ephemeral() -> NetworkSession {
        // SAFETY: returns a new reference.
        NetworkSession(unsafe {
            Object::from_glib_full(ffi::webkit_network_session_new_ephemeral() as *mut _)
        })
    }
}

/// A signal handler, until disconnected (the view's handlers go with
/// its node: a view being let go may still emit while it stops).
pub struct Connection(Object, glib::SignalHandlerId);

impl Connection {
    pub fn disconnect(self) {
        self.0.disconnect(self.1);
    }
}

/// When a user script runs, and where.
pub struct Script<'a> {
    pub source: &'a str,
    /// The script world (`None`: the page's).
    pub world: Option<&'a str>,
}

/// A `WebKitUserContentManager`: user scripts and script message handlers.
#[derive(Clone)]
pub struct UserContentManager(Object);

impl UserContentManager {
    pub fn new() -> UserContentManager {
        // SAFETY: returns a new reference.
        UserContentManager(unsafe {
            Object::from_glib_full(ffi::webkit_user_content_manager_new() as *mut _)
        })
    }

    fn ptr(&self) -> *mut ffi::WebKitUserContentManager {
        self.0.as_ptr() as *mut _
    }

    /// A handler `name` in `world`: `window.webkit.messageHandlers.<name>`
    /// there.
    pub fn register_handler(&self, name: &str, world: &str) {
        // SAFETY: valid object and strings.
        unsafe {
            ffi::webkit_user_content_manager_register_script_message_handler(
                self.ptr(),
                name.to_glib_none().0,
                world.to_glib_none().0,
            );
        }
    }

    pub fn unregister_handler(&self, name: &str, world: &str) {
        // SAFETY: valid object and strings (an unknown handler is ignored).
        unsafe {
            ffi::webkit_user_content_manager_unregister_script_message_handler(
                self.ptr(),
                name.to_glib_none().0,
                world.to_glib_none().0,
            );
        }
    }

    /// A script run at document start, in the main frame only.
    pub fn add_main_frame_script(&self, script: Script) {
        let frames = ffi::WEBKIT_USER_CONTENT_INJECT_TOP_FRAME;
        let start = ffi::WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START;
        let none: *const *const c_char = ptr::null();
        // SAFETY: valid strings, no allow or block lists; the manager
        // takes its own reference to the script.
        unsafe {
            let s = match script.world {
                Some(world) => ffi::webkit_user_script_new_for_world(
                    script.source.to_glib_none().0,
                    frames,
                    start,
                    world.to_glib_none().0,
                    none,
                    none,
                ),
                None => ffi::webkit_user_script_new(
                    script.source.to_glib_none().0,
                    frames,
                    start,
                    none,
                    none,
                ),
            };
            ffi::webkit_user_content_manager_add_script(self.ptr(), s);
            ffi::webkit_user_script_unref(s);
        }
    }

    pub fn remove_all_scripts(&self) {
        // SAFETY: valid object.
        unsafe { ffi::webkit_user_content_manager_remove_all_scripts(self.ptr()) }
    }

    /// `f` gets what handler `name` was posted, if it is a string.
    pub fn connect_message(&self, name: &str, f: impl Fn(String) + 'static) -> Connection {
        let id = self.0.connect_local(
            &format!("script-message-received::{name}"),
            false,
            move |args| {
                let value = args.get(1)?.get::<Object>().ok()?;
                let value = value.as_ptr() as *mut jsc::JSCValue;
                // SAFETY: the signal's JSCValue, alive for the call;
                // `jsc_value_to_string` returns a new string.
                let text = unsafe {
                    (jsc::jsc_value_is_string(value) != 0)
                        .then(|| glib::GString::from_glib_full(jsc::jsc_value_to_string(value)))
                };
                if let Some(text) = text {
                    f(text.to_string());
                }
                None
            },
        );
        Connection(self.0.clone(), id)
    }
}

/// What a navigation policy decision is about.
pub struct Navigation {
    pub uri: String,
    /// A form submitted (or submitted again).
    pub form: bool,
    /// A click on a link.
    pub link: bool,
    pub user_gesture: bool,
    pub redirect: bool,
}

/// The kinds of policy decisions Prism makes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Navigation,
    NewWindow,
    Other,
}

/// A `WebKitPolicyDecision`.
pub struct PolicyDecision(Object);

impl PolicyDecision {
    /// The navigation it decides, for a navigation or a new window.
    pub fn navigation(&self) -> Option<Navigation> {
        // SAFETY: a type check, then calls on the decision's own action
        // and request (owned by the decision, alive for the call).
        unsafe {
            let ty = glib::Type::from_glib(ffi::webkit_navigation_policy_decision_get_type());
            if !self.0.type_().is_a(ty) {
                return None;
            }
            let action = ffi::webkit_navigation_policy_decision_get_navigation_action(
                self.0.as_ptr() as *mut _,
            );
            if action.is_null() {
                return None;
            }
            let request = ffi::webkit_navigation_action_get_request(action);
            let uri: Option<String> = if request.is_null() {
                None
            } else {
                FromGlibPtrNone::from_glib_none(ffi::webkit_uri_request_get_uri(request))
            };
            let kind = ffi::webkit_navigation_action_get_navigation_type(action);
            Some(Navigation {
                uri: uri.unwrap_or_default(),
                form: kind == ffi::WEBKIT_NAVIGATION_TYPE_FORM_SUBMITTED
                    || kind == ffi::WEBKIT_NAVIGATION_TYPE_FORM_RESUBMITTED,
                link: kind == ffi::WEBKIT_NAVIGATION_TYPE_LINK_CLICKED,
                user_gesture: ffi::webkit_navigation_action_is_user_gesture(action) != 0,
                redirect: ffi::webkit_navigation_action_is_redirect(action) != 0,
            })
        }
    }

    /// Refuses it.
    pub fn ignore(&self) {
        // SAFETY: a WebKitPolicyDecision.
        unsafe { ffi::webkit_policy_decision_ignore(self.0.as_ptr() as *mut _) }
    }
}

/// A `WebKitWebView`.
#[derive(Clone)]
pub struct WebView(gtk::Widget);

/// A value of `object`'s own type (a property of a WebKit type takes no
/// plain `GObject` value).
fn object_value(object: &Object) -> Value {
    let mut v = Value::from_type(object.type_());
    // SAFETY: a value of the object's type.
    unsafe { gobject_ffi::g_value_set_object(v.to_glib_none_mut().0, object.as_ptr()) };
    v
}

/// An enum argument of a signal.
fn enum_arg(args: &[Value], i: usize) -> i32 {
    // SAFETY: the signal's argument `i` is an enum.
    args.get(i)
        .map(|v| unsafe { gobject_ffi::g_value_get_enum(v.to_glib_none().0) })
        .unwrap_or(-1)
}

impl WebView {
    /// A view in `context` and `session`, with `content`, the page's
    /// JavaScript on or off, and no window opened by a script on its own.
    pub fn new(
        context: &WebContext,
        session: &NetworkSession,
        content: &UserContentManager,
        scripts: bool,
    ) -> WebView {
        // SAFETY: returns a new reference.
        let settings: Object =
            unsafe { Object::from_glib_full(ffi::webkit_settings_new() as *mut _) };
        settings.set_property("enable-javascript-markup", scripts);
        settings.set_property("javascript-can-open-windows-automatically", false);
        // SAFETY: registers the type (libwebkitgtk is linked).
        let ty = unsafe { glib::Type::from_glib(ffi::webkit_web_view_get_type()) };
        let view = Object::with_mut_values(
            ty,
            &mut [
                ("web-context", object_value(&context.0)),
                ("network-session", object_value(&session.0)),
                ("user-content-manager", object_value(&content.0)),
                ("settings", object_value(&settings)),
            ],
        );
        WebView(view.downcast().expect("WebKitWebView is a GtkWidget"))
    }

    pub fn widget(&self) -> &gtk::Widget {
        &self.0
    }

    fn ptr(&self) -> *mut ffi::WebKitWebView {
        self.0.as_ptr() as *mut _
    }

    /// Whether `w` is a `WebKitWebView`.
    pub fn is(w: &gtk::Widget) -> bool {
        // SAFETY: registers the type (libwebkitgtk is linked).
        let ty = unsafe { glib::Type::from_glib(ffi::webkit_web_view_get_type()) };
        w.type_().is_a(ty)
    }

    pub fn load_uri(&self, uri: &str) {
        // SAFETY: valid view and string.
        unsafe { ffi::webkit_web_view_load_uri(self.ptr(), uri.to_glib_none().0) }
    }

    pub fn reload(&self) {
        // SAFETY: valid view.
        unsafe { ffi::webkit_web_view_reload(self.ptr()) }
    }

    pub fn stop_loading(&self) {
        // SAFETY: valid view.
        unsafe { ffi::webkit_web_view_stop_loading(self.ptr()) }
    }

    /// The URL of the page shown (or being loaded).
    pub fn uri(&self) -> Option<String> {
        // SAFETY: valid view; the string is the view's.
        unsafe { FromGlibPtrNone::from_glib_none(ffi::webkit_web_view_get_uri(self.ptr())) }
    }

    pub fn set_background_color(&self, rgba: &gtk::gdk::RGBA) {
        // SAFETY: valid view and color (copied).
        unsafe { ffi::webkit_web_view_set_background_color(self.ptr(), rgba.to_glib_none().0) }
    }

    /// Runs `script` in the main frame's page world, without waiting for
    /// its result.
    pub fn run_script(&self, script: &str) {
        // SAFETY: valid view and script (with its length), no callback.
        unsafe {
            ffi::webkit_web_view_evaluate_javascript(
                self.ptr(),
                script.as_ptr() as *const c_char,
                script.len() as _,
                ptr::null(),
                ptr::null(),
                ptr::null_mut(),
                None,
                ptr::null_mut(),
            )
        }
    }

    /// Turns the page's JavaScript on or off (from the next page).
    pub fn set_scripts(&self, on: bool) {
        // SAFETY: valid view; the settings are the view's.
        let settings: Option<Object> = unsafe {
            FromGlibPtrNone::from_glib_none(
                ffi::webkit_web_view_get_settings(self.ptr()) as *mut gobject_ffi::GObject
            )
        };
        if let Some(s) = settings {
            s.set_property("enable-javascript-markup", on);
        }
    }

    pub fn user_content_manager(&self) -> Option<UserContentManager> {
        // SAFETY: valid view; the manager is the view's.
        let ucm: Option<Object> = unsafe {
            FromGlibPtrNone::from_glib_none(ffi::webkit_web_view_get_user_content_manager(
                self.ptr(),
            ) as *mut gobject_ffi::GObject)
        };
        ucm.map(UserContentManager)
    }

    /// A page started loading (`true`) or finished (`false`).
    pub fn connect_load(&self, f: impl Fn(&WebView, bool) + 'static) -> Connection {
        let view = self.0.downgrade();
        let id = self.0.connect_local("load-changed", false, move |args| {
            let view = WebView(view.upgrade()?);
            match enum_arg(args, 1) {
                ffi::WEBKIT_LOAD_STARTED => f(&view, true),
                ffi::WEBKIT_LOAD_FINISHED => f(&view, false),
                _ => {}
            }
            None
        });
        self.connection(id)
    }

    /// A load failed (WebKit then shows its error page).
    pub fn connect_load_failed(&self, f: impl Fn() + 'static) -> Connection {
        let id = self.0.connect_local("load-failed", false, move |_| {
            f();
            Some(false.to_value())
        });
        self.connection(id)
    }

    /// A policy decision: `f` returns whether it decided (else WebKit's
    /// default: allowed).
    pub fn connect_decide_policy(
        &self,
        f: impl Fn(&PolicyDecision, Policy) -> bool + 'static,
    ) -> Connection {
        let id = self.0.connect_local("decide-policy", false, move |args| {
            let decision = PolicyDecision(args.get(1)?.get::<Object>().ok()?);
            let kind = match enum_arg(args, 2) {
                ffi::WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION => Policy::Navigation,
                ffi::WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION => Policy::NewWindow,
                _ => Policy::Other,
            };
            Some(f(&decision, kind).to_value())
        });
        self.connection(id)
    }

    /// The page's process ended without the app asking.
    pub fn connect_process_died(&self, f: impl Fn(&WebView) + 'static) -> Connection {
        let view = self.0.downgrade();
        let id = self
            .0
            .connect_local("web-process-terminated", false, move |args| {
                if enum_arg(args, 1) != ffi::WEBKIT_WEB_PROCESS_TERMINATED_BY_API
                    && let Some(view) = view.upgrade()
                {
                    f(&WebView(view));
                }
                None
            });
        self.connection(id)
    }

    fn connection(&self, id: glib::SignalHandlerId) -> Connection {
        Connection(self.0.clone().upcast(), id)
    }
}
