//! Web views (`web_view()`, #343): WebKitGTK 6.0, the GTK 4 build of
//! WebKit, behind the `web-view` feature (it links libwebkitgtk-6.0).
//!
//! Behaves like the `WKWebView` of the Apple backends
//! (`PrismWebView.swift`) and the Windows backend's `WebView2`:
//!
//! - the page talks to the app with `prismHost.postMessage(JSON.stringify(v))`
//!   and receives the app's messages as `message` events on `window`;
//! - only the allowed origins (the config's, else the URL's own) are
//!   heard and posted to; a link to another origin opens in the system
//!   browser, so do new windows (`target="_blank"`, `window.open`) when the
//!   sandbox allows popups and the user asked for them;
//! - a sandbox without `scripts` turns the page's JavaScript off (the
//!   app's scripts still run, as with `allowsContentJavaScript`); one
//!   without `same-origin` gets an ephemeral network session (no cookies
//!   or storage kept); one without `forms` cancels form navigations;
//! - `on_load` hears each page that finished loading.
//!
//! `prismHost` is in the main frame only. WebKitGTK's script message
//! handlers do not say which frame posted, so the handler lives in an
//! isolated script world that only Prism's script, injected in the main
//! frame, runs in: the page's `prismHost` hands it each message as a DOM
//! event, and it posts it with the main frame's origin (read from
//! `location` in its own world, which the page cannot reach).
//!
//! One `WebContext` serves every web view of the app, with one persistent
//! network session, its data in `$XDG_DATA_HOME/<app>/WebKit` and its
//! cache in `$XDG_CACHE_HOME/<app>/WebKit`. Custom URL schemes go on that
//! context ([`context`]).
//!
//! The `WebKitWebView` is made on the main loop's next turn, after the
//! update that created the node: its network session (from the sandbox,
//! in the config, which comes after the URL) is fixed when it is made. A
//! sandbox's `same-origin` that changes later makes a new one.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use prism_core::web_view::{WebViewConfig, message_script};
use prism_core::{NodeId, UiEvent};

use crate::imp::dispatch;
use crate::links;
use crate::webkit::{NetworkSession, Policy, Script, UserContentManager, WebContext, WebView};

/// The script world the message handler lives in: page scripts never run
/// there.
const WORLD: &str = "prism";
/// The handler's name, in [`WORLD`] only.
const HANDLER: &str = "prismHost";
/// In the page's world, main frame: `prismHost`. Takes what it calls at
/// document start, before the page's scripts could replace it.
const PAGE_BRIDGE: &str = r#"(function () {
  if (window.prismHost) return;
  var d = document, E = CustomEvent, send = EventTarget.prototype.dispatchEvent, apply = Reflect.apply;
  Object.defineProperty(window, "prismHost", {
    value: Object.freeze({
      postMessage: function (json) { apply(send, d, [new E("prismhost-post", { detail: String(json) })]); },
    }),
  });
})();"#;

/// In Prism's world, main frame: hands each message to the app with the
/// frame's origin (`location.protocol + "//" + location.host`, as
/// `prism_core::web_view::origin_of` writes it, `file://` too).
const WORLD_BRIDGE: &str = r#"(function () {
  var h = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.prismHost;
  if (!h) return;
  document.addEventListener("prismhost-post", function (e) {
    if (typeof e.detail !== "string") return;
    h.postMessage(location.protocol + "//" + location.host + "\n" + e.detail);
  });
})();"#;

thread_local! {
    /// The app's web context and persistent network session, once made.
    static CONTEXT: RefCell<Option<WebContext>> = const { RefCell::new(None) };
    static SESSION: RefCell<Option<NetworkSession>> = const { RefCell::new(None) };
}

/// The web context every web view of the app shares. App schemes
/// (`app://`) are registered on it (`webkit_web_context_register_uri_scheme`,
/// and its security manager's `register_uri_scheme_as_secure` and
/// `register_uri_scheme_as_cors_enabled`), before a view loads them.
pub fn context() -> WebContext {
    CONTEXT.with(|c| c.borrow_mut().get_or_insert_with(WebContext::new).clone())
}

/// The app's persistent network session (cookies, storage, cache), in its
/// data and cache directories.
fn session(title: &str) -> NetworkSession {
    SESSION.with(|s| {
        s.borrow_mut()
            .get_or_insert_with(|| {
                let data = crate::imp::data_dir(title).join("WebKit");
                let cache: PathBuf = glib::user_cache_dir()
                    .join(title.replace('/', "-"))
                    .join("WebKit");
                NetworkSession::new(&data.to_string_lossy(), &cache.to_string_lossy())
            })
            .clone()
    })
}

/// A web view node: a box holding the `WebKitWebView` once made.
pub struct Web {
    pub root: gtk::Box,
    state: Rc<RefCell<State>>,
}

struct State {
    id: NodeId,
    /// The app's title (where its data goes).
    title: String,
    root: glib::WeakRef<gtk::Box>,
    url: String,
    config: WebViewConfig,
    view: Option<WebView>,
    /// The view's network session is ephemeral (as made).
    ephemeral: bool,
    /// The page finished loading (messages go now).
    loaded: bool,
    /// The current load failed (no `on_load`).
    failed: bool,
    /// Messages posted before the page loaded.
    queue: Vec<String>,
}

impl State {
    /// A sandbox without `same-origin`.
    fn wants_ephemeral(&self) -> bool {
        self.config.sandbox.is_some_and(|s| !s.same_origin)
    }

    fn blocks_forms(&self) -> bool {
        self.config.sandbox.is_some_and(|s| !s.forms)
    }

    fn popups(&self) -> bool {
        self.config.sandbox.is_none_or(|s| s.popups)
    }

    /// Whether the app talks to a page of `origin` (none: only with `*`).
    fn allows(&self, origin: Option<&str>) -> bool {
        match origin {
            Some(origin) => self.config.allows(&self.url, origin),
            None => self
                .config
                .allowed_origins(&self.url)
                .iter()
                .any(|o| o == "*"),
        }
    }
}

impl Web {
    pub fn new(id: NodeId, title: &str) -> Web {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let state = Rc::new(RefCell::new(State {
            id,
            title: title.to_string(),
            root: root.downgrade(),
            url: String::new(),
            config: WebViewConfig::default(),
            view: None,
            ephemeral: false,
            loaded: false,
            failed: false,
            queue: Vec::new(),
        }));
        Web { root, state }
    }

    /// The URL (`set_text`): loaded on the next turn, with the config.
    pub fn load(&self, url: &str) {
        {
            let mut st = self.state.borrow_mut();
            if st.url == url && st.view.is_some() {
                return;
            }
            st.url = url.to_string();
            st.loaded = false;
        }
        schedule(&self.state);
    }

    /// The options (`WebViewConfig`, encoded).
    pub fn configure(&self, config: &str) {
        let c = WebViewConfig::decode(config);
        let mut st = self.state.borrow_mut();
        let messages_changed = st.config.messages != c.messages;
        st.config = c;
        let Some(view) = st.view.clone() else { return };
        if st.wants_ephemeral() != st.ephemeral {
            // Another network session: a new view.
            drop(st);
            self.teardown();
            schedule(&self.state);
            return;
        }
        view.set_scripts(st.config.scripts());
        if messages_changed && let Some(ucm) = view.user_content_manager() {
            bridge(&ucm, st.config.messages);
        }
    }

    /// `cx.post_web_message`: now if the page is loaded, else once it is.
    pub fn post(&self, json: &str) {
        let view = {
            let st = self.state.borrow();
            st.view.clone().filter(|_| st.loaded)
        };
        match view {
            Some(view) => deliver(&self.state, &view, json),
            None => self.state.borrow_mut().queue.push(json.to_string()),
        }
    }

    /// `cx.reload_web_view`.
    pub fn reload(&self) {
        let view = {
            let mut st = self.state.borrow_mut();
            st.loaded = false;
            st.view.clone()
        };
        match view {
            Some(view)
                if view
                    .uri()
                    .is_some_and(|u| !u.is_empty() && u != "about:blank") =>
            {
                view.reload()
            }
            _ => schedule(&self.state),
        }
    }

    /// Stops the page and lets the view go (the node is gone, or its
    /// network session changes).
    pub fn teardown(&self) {
        let Some(view) = self.state.borrow_mut().view.take() else {
            return;
        };
        self.state.borrow_mut().loaded = false;
        view.stop_loading();
        if let Some(ucm) = view.user_content_manager() {
            ucm.remove_all_scripts();
            ucm.unregister_handler(HANDLER, WORLD);
        }
        self.root.remove(view.widget());
    }
}

/// Whether `w` is a web page (its keys are its own while it types text).
pub fn is_page(w: &gtk::Widget) -> bool {
    WebView::is(w)
}

/// Loads the URL on the main loop's next turn (once, however many calls
/// this turn), making the view first if needed.
fn schedule(state: &Rc<RefCell<State>>) {
    thread_local! {
        static PENDING: RefCell<Vec<Weak<RefCell<State>>>> = const { RefCell::new(Vec::new()) };
        static SCHEDULED: Cell<bool> = const { Cell::new(false) };
    }
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if !p.iter().any(|w| w.as_ptr() == Rc::as_ptr(state)) {
            p.push(Rc::downgrade(state));
        }
    });
    if SCHEDULED.replace(true) {
        return;
    }
    glib::idle_add_local_once(|| {
        SCHEDULED.set(false);
        for state in PENDING.with(|p| std::mem::take(&mut *p.borrow_mut())) {
            if let Some(state) = state.upgrade() {
                perform_load(&state);
            }
        }
    });
}

fn perform_load(state: &Rc<RefCell<State>>) {
    let view = match state.borrow().view.clone() {
        Some(view) => view,
        None => match make_view(state) {
            Some(view) => view,
            None => return,
        },
    };
    let url = state.borrow().url.clone();
    if !url.is_empty() {
        view.load_uri(&url);
    }
}

/// Makes the node's `WebKitWebView` (in the app's network session, or an
/// ephemeral one), with its bridge and handlers.
fn make_view(state: &Rc<RefCell<State>>) -> Option<WebView> {
    let (root, ephemeral, config, title) = {
        let st = state.borrow();
        (
            st.root.upgrade()?,
            st.wants_ephemeral(),
            st.config.clone(),
            st.title.clone(),
        )
    };
    let session = if ephemeral {
        NetworkSession::ephemeral()
    } else {
        session(&title)
    };
    let ucm = UserContentManager::new();
    let view = WebView::new(&context(), &session, &ucm, config.scripts());
    let w = view.widget();
    w.set_hexpand(true);
    w.set_vexpand(true);
    // The node's background shows while the page loads, and behind a page
    // without one (as an `<iframe>`).
    view.set_background_color(&gdk::RGBA::new(0., 0., 0., 0.));
    bridge(&ucm, config.messages);

    let weak = Rc::downgrade(state);
    ucm.connect_message(HANDLER, move |body| {
        let Some(state) = weak.upgrade() else { return };
        let Some((origin, json)) = links::parse_message(&body) else {
            return;
        };
        let st = state.borrow();
        if !st.config.messages || !st.allows(Some(origin)) {
            return;
        }
        let event = UiEvent::WebMessage(st.id, json.to_string(), origin.to_string());
        drop(st);
        dispatch(event);
    });

    let weak = Rc::downgrade(state);
    view.connect_decide_policy(move |decision, kind| {
        let (Some(state), Some(nav)) = (weak.upgrade(), decision.navigation()) else {
            return false;
        };
        let st = state.borrow();
        if nav.form && st.blocks_forms() {
            log::form(st.id, &nav.uri);
            decision.ignore();
            return true;
        }
        match kind {
            // In any frame: WebKitGTK does not say which one.
            Policy::Navigation => {
                let user = !nav.redirect && (nav.form || nav.link || nav.user_gesture);
                if links::leaves(&nav.uri, user, |o| st.allows(o)) {
                    decision.ignore();
                    drop(st);
                    open_externally(&nav.uri);
                    return true;
                }
                false
            }
            // Never a window of the view's own: the browser, if allowed
            // and asked for by the user.
            Policy::NewWindow => {
                decision.ignore();
                match links::new_window(&nav.uri, st.popups(), nav.user_gesture) {
                    Ok(()) => {
                        drop(st);
                        open_externally(&nav.uri);
                    }
                    Err(why) => log::new_window(st.id, &nav.uri, why),
                }
                true
            }
            Policy::Other => false,
        }
    });

    let weak = Rc::downgrade(state);
    view.connect_load(move |view, started| {
        let Some(state) = weak.upgrade() else { return };
        if started {
            let mut st = state.borrow_mut();
            st.loaded = false;
            st.failed = false;
            return;
        }
        let (pending, loads, id) = {
            let mut st = state.borrow_mut();
            if st.failed {
                return;
            }
            st.loaded = true;
            (std::mem::take(&mut st.queue), st.config.loads, st.id)
        };
        for json in pending {
            deliver(&state, view, &json);
        }
        if loads {
            dispatch(UiEvent::WebLoad(id, view.uri().unwrap_or_default()));
        }
    });
    let weak = Rc::downgrade(state);
    view.connect_load_failed(move || {
        if let Some(state) = weak.upgrade() {
            state.borrow_mut().failed = true;
        }
    });
    // The page's process died: load it again.
    view.connect_process_died(WebView::reload);

    root.append(view.widget());
    let mut st = state.borrow_mut();
    st.view = Some(view.clone());
    st.ephemeral = ephemeral;
    Some(view)
}

/// Installs the bridge (or takes it away). For the next page: the shown
/// one keeps what it has.
fn bridge(ucm: &UserContentManager, messages: bool) {
    ucm.remove_all_scripts();
    ucm.unregister_handler(HANDLER, WORLD);
    if !messages {
        return;
    }
    ucm.register_handler(HANDLER, WORLD);
    ucm.add_main_frame_script(Script {
        source: PAGE_BRIDGE,
        world: None,
    });
    ucm.add_main_frame_script(Script {
        source: WORLD_BRIDGE,
        world: Some(WORLD),
    });
}

/// Delivers `json` to the page as a `message` event, if it shows an
/// allowed origin, and still shows it when the script runs.
fn deliver(state: &Rc<RefCell<State>>, view: &WebView, json: &str) {
    let uri = view.uri().unwrap_or_default();
    let origin = links::page_origin(&uri);
    if !state.borrow().allows(origin.as_deref()) {
        return;
    }
    // `json` is valid JSON (checked by Prism), so a JavaScript literal.
    let script = message_script(json, origin.as_deref());
    view.run_script(&script);
}

/// Web links, mail and phone numbers open in their app.
fn open_externally(url: &str) {
    if !links::external_scheme(url) {
        return;
    }
    if let Err(e) = gio::AppInfo::launch_default_for_uri(url, None::<&gio::AppLaunchContext>) {
        eprintln!("prism-linux: web view: {url} not opened: {e}");
    }
}

mod log {
    use prism_core::NodeId;

    pub fn form(id: NodeId, uri: &str) {
        eprintln!(
            "prism-linux: web view {id}: a form sent to {uri} held back (sandbox without forms)"
        );
    }

    pub fn new_window(id: NodeId, uri: &str, why: crate::links::Refused) {
        eprintln!(
            "prism-linux: web view {id}: a new window for {uri} not opened: {}",
            why.reason()
        );
    }
}
