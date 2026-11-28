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

use gtk::glib::ffi::gpointer;
use gtk::glib::translate::{FromGlib, FromGlibPtrFull, FromGlibPtrNone, ToGlibPtr, ToGlibPtrMut};
use gtk::glib::{self, Object, Value, gobject_ffi};
use gtk::{gio, prelude::*};
use javascriptcore6_sys as jsc;
use soup3_sys as soup;
use webkit6_sys as ffi;

/// A `WebKitWebContext`: web processes and URL schemes, shared by views.
#[derive(Clone)]
pub struct WebContext(Object);

impl WebContext {
    pub fn new() -> WebContext {
        // SAFETY: returns a new reference.
        WebContext(unsafe { Object::from_glib_full(ffi::webkit_web_context_new() as *mut _) })
    }

    fn ptr(&self) -> *mut ffi::WebKitWebContext {
        self.0.as_ptr() as *mut _
    }

    /// Serves `scheme` with `f` in every view of the context (#343): its
    /// URLs are secure (as `https:`) and fetchable with CORS. For views
    /// made after: call it before the first.
    pub fn register_scheme(&self, scheme: &str, f: fn(SchemeRequest)) {
        unsafe extern "C" fn received(request: *mut ffi::WebKitURISchemeRequest, data: gpointer) {
            // SAFETY: `data` is the boxed `fn` given below, alive as long
            // as the context; `request` is alive for the call, and
            // `from_glib_none` takes a reference of Prism's own.
            unsafe {
                let f = *(data as *const fn(SchemeRequest));
                f(SchemeRequest(Object::from_glib_none(
                    request as *mut gobject_ffi::GObject,
                )));
            }
        }
        unsafe extern "C" fn free(data: gpointer) {
            // SAFETY: the box made below, freed once, by the context.
            drop(unsafe { Box::from_raw(data as *mut fn(SchemeRequest)) });
        }
        let data = Box::into_raw(Box::new(f)) as gpointer;
        // SAFETY: valid context and strings; the context owns `data` and
        // frees it with `free`. The security manager is the context's.
        unsafe {
            ffi::webkit_web_context_register_uri_scheme(
                self.ptr(),
                scheme.to_glib_none().0,
                Some(received),
                data,
                Some(free),
            );
            let security = ffi::webkit_web_context_get_security_manager(self.ptr());
            if !security.is_null() {
                ffi::webkit_security_manager_register_uri_scheme_as_secure(
                    security,
                    scheme.to_glib_none().0,
                );
                ffi::webkit_security_manager_register_uri_scheme_as_cors_enabled(
                    security,
                    scheme.to_glib_none().0,
                );
            }
        }
    }
}

/// A `WebKitURISchemeRequest`: a page asked for a URL of one of the app's
/// schemes. Holds a reference until answered ([`finish`](Self::finish),
/// [`fail`](Self::fail)), which consumes it: answered once.
pub struct SchemeRequest(Object);

impl SchemeRequest {
    fn ptr(&self) -> *mut ffi::WebKitURISchemeRequest {
        self.0.as_ptr() as *mut _
    }

    /// The URL asked for (`app://main/index.html`).
    pub fn uri(&self) -> String {
        // SAFETY: valid request; the string is the request's.
        let uri: Option<String> = unsafe {
            FromGlibPtrNone::from_glib_none(ffi::webkit_uri_scheme_request_get_uri(self.ptr()))
        };
        uri.unwrap_or_default()
    }

    /// `GET`, `POST`…
    pub fn method(&self) -> String {
        // SAFETY: valid request; the string is the request's.
        let method: Option<String> = unsafe {
            FromGlibPtrNone::from_glib_none(ffi::webkit_uri_scheme_request_get_http_method(
                self.ptr(),
            ))
        };
        method
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "GET".into())
    }

    /// The request's headers, in order.
    pub fn headers(&self) -> Vec<(String, String)> {
        unsafe extern "C" fn each(name: *const c_char, value: *const c_char, data: gpointer) {
            // SAFETY: `data` is the vector below, borrowed for the walk;
            // the strings are the headers', alive for the call.
            unsafe {
                let out = &mut *(data as *mut Vec<(String, String)>);
                let text = |p: *const c_char| -> String {
                    if p.is_null() {
                        String::new()
                    } else {
                        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
                    }
                };
                out.push((text(name), text(value)));
            }
        }
        let mut out: Vec<(String, String)> = Vec::new();
        // SAFETY: valid request; its headers (if any) are its own, walked
        // now, with `out` alive for the walk.
        unsafe {
            let headers = ffi::webkit_uri_scheme_request_get_http_headers(self.ptr());
            if !headers.is_null() {
                soup::soup_message_headers_foreach(
                    headers,
                    Some(each),
                    &mut out as *mut Vec<(String, String)> as gpointer,
                );
            }
        }
        out
    }

    /// The body (a `fetch` POST's), to read: none without one.
    pub fn body(&self) -> Option<gio::InputStream> {
        // SAFETY: valid request; returns a new reference, or null.
        unsafe {
            let stream = ffi::webkit_uri_scheme_request_get_http_body(self.ptr());
            (!stream.is_null()).then(|| gio::InputStream::from_glib_full(stream))
        }
    }

    /// Answers with `response` (its status, type, headers and body).
    pub fn finish(self, response: prism_core::SchemeResponse) {
        let (status, reason) = (u32::from(response.status), response.reason());
        let headers = response.header_list();
        // The type also goes apart: WebKit takes the MIME type and charset
        // from it, the page's `fetch` sees the header.
        let content_type = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.clone());
        let len = response.body.len() as i64;
        let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(response.body));
        // SAFETY: valid request; the response takes its own reference to
        // the stream; the headers are given to the response (transfer
        // full); the response is let go once the request holds it.
        unsafe {
            let list = soup::soup_message_headers_new(soup::SOUP_MESSAGE_HEADERS_RESPONSE);
            for (name, value) in &headers {
                soup::soup_message_headers_append(
                    list,
                    name.to_glib_none().0,
                    value.to_glib_none().0,
                );
            }
            let r = ffi::webkit_uri_scheme_response_new(
                stream.upcast_ref::<gio::InputStream>().to_glib_none().0,
                len,
            );
            ffi::webkit_uri_scheme_response_set_status(r, status, reason.to_glib_none().0);
            if let Some(t) = content_type {
                ffi::webkit_uri_scheme_response_set_content_type(r, t.to_glib_none().0);
            }
            ffi::webkit_uri_scheme_response_set_http_headers(r, list);
            ffi::webkit_uri_scheme_request_finish_with_response(self.ptr(), r);
            gobject_ffi::g_object_unref(r as *mut gobject_ffi::GObject);
        }
    }

    /// Fails the request (WebKit reports a network error to the page).
    pub fn fail(self, message: &str) {
        let error = glib::Error::new(gio::IOErrorEnum::Failed, message);
        // SAFETY: valid request and error (copied by WebKit).
        unsafe {
            let e: *const glib::ffi::GError = error.to_glib_none().0;
            ffi::webkit_uri_scheme_request_finish_error(self.ptr(), e as *mut _);
        }
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
