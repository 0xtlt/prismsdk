//! Web views (`web_view()`, #62): WinUI's `WebView2` control, the
//! Chromium engine of the Evergreen WebView2 Runtime (part of Windows 11;
//! nothing is installed with the app). Its WinRT projection,
//! `Microsoft.Web.WebView2.Core.dll`, goes next to the app with the
//! Windows App SDK runtime (`prism_build::winappsdk`).
//!
//! Behaves like the `WKWebView` of the Apple backends
//! (`PrismWebView.swift`):
//!
//! - the page talks to the app with `prismHost.postMessage(JSON.stringify(v))`
//!   (a script added at document creation, in the main frame only, over
//!   `chrome.webview.postMessage`), and receives the app's messages as
//!   `message` events on `window`;
//! - only the allowed origins (the config's, else the URL's own) are
//!   heard and posted to; a link to another origin opens in the default
//!   browser, so do new windows (`target="_blank"`, `window.open`) when
//!   the sandbox allows popups;
//! - a sandbox without `scripts` turns JavaScript off; one without
//!   `same-origin` browses InPrivate (no cookies or storage kept); one
//!   without `forms` submits no form, in any frame, as WebKit cancels
//!   `formSubmitted` navigations: WebView2 does not tell them apart, so a
//!   script at document creation stops `submit` events, `form.submit()`
//!   and `requestSubmit()`, and every POST of a document (a frame's too)
//!   is answered 403 before it leaves, scripts on or off;
//! - `on_load` hears each page that finished loading.
//!
//! One `CoreWebView2Environment` serves every web view of the app, its
//! data (cookies, cache) in `%LOCALAPPDATA%\<app>\WebView2`, not next to
//! the executable. WebView2's async operations are awaited by polling
//! them on the UI thread (`when_done`): their callbacks may come on
//! another thread.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use prism_core::web_view::{WebViewConfig, origin_of};
use prism_core::{NodeId, UiEvent};
use windows::Foundation::TypedEventHandler;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SW_SHOWNORMAL, SetTimer};
use windows::core::{PCWSTR, w};
use windows_core::{HSTRING, Result};
use windows_future::{AsyncStatus, IAsyncOperation};

use crate::imp::dispatch;
use crate::widgets::Win;
use crate::win::{log, wide};
use crate::winui::bindings::Microsoft::Web::WebView2::Core::{
    CoreWebView2, CoreWebView2Environment, CoreWebView2EnvironmentOptions,
    CoreWebView2NavigationCompletedEventArgs, CoreWebView2NavigationStartingEventArgs,
    CoreWebView2NewWindowRequestedEventArgs, CoreWebView2ProcessFailedEventArgs,
    CoreWebView2WebMessageReceivedEventArgs, CoreWebView2WebResourceContext,
    CoreWebView2WebResourceRequestSourceKinds, CoreWebView2WebResourceRequestedEventArgs,
};
use crate::winui::h;
use crate::winui::xaml::Controls::{CoreWebView2InitializedEventArgs, WebView2};

/// `prismHost`, in the main frame of every page (WebView2 runs the
/// script in frames too: they are left without it).
const BRIDGE: &str = r#"(function () {
  if (window !== window.top) return;
  var w = window.chrome && window.chrome.webview;
  if (!w || window.prismHost) return;
  Object.defineProperty(window, "prismHost", {
    value: Object.freeze({ postMessage: function (json) { w.postMessage(String(json)); } }),
  });
})();"#;

/// In every frame of a sandbox without `forms`: no form is submitted.
const NO_FORMS: &str = r#"(function () {
  var stop = function (e) { e.preventDefault(); e.stopImmediatePropagation(); };
  window.addEventListener("submit", stop, true);
  var none = function () {};
  try {
    Object.defineProperty(HTMLFormElement.prototype, "submit", { value: none });
    Object.defineProperty(HTMLFormElement.prototype, "requestSubmit", { value: none });
  } catch (e) {}
})();"#;

/// A web view node: its control and what it shows.
pub struct Web {
    pub element: WebView2,
    state: Rc<RefCell<State>>,
}

#[derive(Default)]
struct State {
    id: NodeId,
    url: String,
    config: WebViewConfig,
    /// Initialisation asked for (it waits for the environment).
    started: bool,
    /// The `CoreWebView2` is ready.
    core: Option<CoreWebView2>,
    /// The bridge script's id, while installed.
    bridge: Option<HSTRING>,
    /// The script stopping forms, while installed.
    no_forms: Option<HSTRING>,
    /// The page finished loading (messages go now).
    loaded: bool,
    /// Messages posted before the page loaded.
    queue: Vec<String>,
    /// InPrivate (a sandbox without `same-origin`), as created.
    private: bool,
}

impl State {
    /// A sandbox without `forms`.
    fn blocks_forms(&self) -> bool {
        self.config.sandbox.is_some_and(|s| !s.forms)
    }

    fn allows(&self, origin: Option<&str>) -> bool {
        let Some(origin) = origin else {
            return self
                .config
                .allowed_origins(&self.url)
                .iter()
                .any(|o| o == "*");
        };
        self.config.allows(&self.url, origin)
    }
}

impl Web {
    pub fn new(id: NodeId) -> Result<Web> {
        let element = WebView2::new()?;
        let state = Rc::new(RefCell::new(State {
            id,
            ..State::default()
        }));
        WEBS.with(|w| w.borrow_mut().insert(id, Rc::downgrade(&state)));
        WEB_ELEMENTS.with(|e| e.borrow_mut().insert(id, element.clone()));
        element.CoreWebView2Initialized(&TypedEventHandler::<
            WebView2,
            CoreWebView2InitializedEventArgs,
        >::new(move |s, a| {
            let failed = a.as_ref().and_then(|a| a.Exception().ok()).filter(|e| e.is_err());
            if let Some(e) = failed {
                log!("web view {id}: WebView2 could not start ({e}); is the WebView2 Runtime installed?");
                return Ok(());
            }
            if let Some(s) = s.as_ref() {
                initialized(id, s)?;
            }
            Ok(())
        }))?;
        Ok(Web { element, state })
    }

    /// The options (`WebViewConfig`, encoded).
    pub fn configure(&self, config: &str) -> Result<()> {
        let c = WebViewConfig::decode(config);
        let mut st = self.state.borrow_mut();
        let messages_changed = st.config.messages != c.messages;
        st.config = c;
        if let Some(core) = st.core.clone() {
            let private = st.config.sandbox.is_some_and(|s| !s.same_origin);
            if private != st.private {
                log!(
                    "web view {}: a sandbox's same-origin changes after the page started: \
                     kept as it was",
                    st.id
                );
            }
            settings(&core, &st.config)?;
            drop(st);
            if messages_changed {
                bridge(&self.state, &core)?;
            }
            forms(&self.state, &core)?;
        }
        Ok(())
    }
}

impl Drop for Web {
    fn drop(&mut self) {
        let _ = self.element.Close();
    }
}

thread_local! {
    /// The web views, by node.
    static WEBS: RefCell<HashMap<NodeId, std::rc::Weak<RefCell<State>>>> =
        RefCell::new(HashMap::new());
    /// The app's environment, once created, and whether it is coming.
    static ENV: RefCell<Option<CoreWebView2Environment>> = const { RefCell::new(None) };
    static ENV_STARTED: Cell<bool> = const { Cell::new(false) };
    /// Web views waiting for the environment.
    static WAITING: RefCell<Vec<NodeId>> = const { RefCell::new(Vec::new()) };
}

fn state_of(id: NodeId) -> Option<Rc<RefCell<State>>> {
    WEBS.with(|w| w.borrow().get(&id).and_then(|s| s.upgrade()))
}

impl Win {
    /// Web view node `id` shows its URL (`set_text`): loads it, starting
    /// WebView2 first if needed.
    pub fn show_web(&mut self, id: NodeId) {
        let Some(url) = self.scene.get(id).map(|n| n.text.clone()) else {
            return;
        };
        let Some(state) = state_of(id) else { return };
        let core = {
            let mut st = state.borrow_mut();
            if st.url == url && st.started {
                return;
            }
            st.url = url.clone();
            st.loaded = false;
            st.core.clone()
        };
        match core {
            Some(core) => navigate(&core, &url),
            None => {
                // After this update: its config (sandbox) comes with it.
                let data = crate::system::data_dir(&self.app_title).join("WebView2");
                crate::imp::defer(Box::new(move |_| start(id, &data)));
            }
        }
    }
}

/// Starts node `id`'s WebView2 once the app's environment is there.
fn start(id: NodeId, data: &std::path::Path) {
    let Some(state) = state_of(id) else { return };
    if std::mem::replace(&mut state.borrow_mut().started, true) {
        return;
    }
    if let Some(env) = ENV.with(|e| e.borrow().clone()) {
        ensure(id, &env);
        return;
    }
    WAITING.with(|w| w.borrow_mut().push(id));
    if ENV_STARTED.replace(true) {
        return;
    }
    let r = (|| -> Result<IAsyncOperation<CoreWebView2Environment>> {
        let _ = std::fs::create_dir_all(data);
        let options = CoreWebView2EnvironmentOptions::new()?;
        CoreWebView2Environment::CreateWithOptionsAsync(
            &h(""),
            &h(&data.display().to_string()),
            &options,
        )
    })();
    match r {
        Ok(op) => when_done(op, |env| match env {
            Ok(env) => {
                ENV.with(|e| *e.borrow_mut() = Some(env.clone()));
                for id in WAITING.with(|w| std::mem::take(&mut *w.borrow_mut())) {
                    ensure(id, &env);
                }
            }
            Err(e) => log!(
                "web views: the WebView2 Runtime did not start ({e}). It is part of \
                 Windows 11; on Windows 10 it comes with Microsoft Edge"
            ),
        }),
        Err(e) => log!("web views: WebView2 environment: {e}"),
    }
}

/// Creates node `id`'s `CoreWebView2` in `env` (InPrivate for a sandbox
/// without `same-origin`); `CoreWebView2Initialized` follows.
fn ensure(id: NodeId, env: &CoreWebView2Environment) {
    let Some(state) = state_of(id) else { return };
    let element = WEB_ELEMENTS.with(|e| e.borrow().get(&id).cloned());
    let Some(element) = element else { return };
    let private = state
        .borrow()
        .config
        .sandbox
        .is_some_and(|s| !s.same_origin);
    state.borrow_mut().private = private;
    let r = (|| -> Result<()> {
        if private {
            let options = env.CreateCoreWebView2ControllerOptions()?;
            options.SetIsInPrivateModeEnabled(true)?;
            element.EnsureCoreWebView2WithEnvironmentAndOptionsAsync(env, &options)?;
        } else {
            element.EnsureCoreWebView2WithEnvironmentAsync(env)?;
        }
        Ok(())
    })();
    if let Err(e) = r {
        log!("web view {id}: {e}");
    }
}

thread_local! {
    /// The controls, by node (to start them from the environment's
    /// callback).
    static WEB_ELEMENTS: RefCell<HashMap<NodeId, WebView2>> = RefCell::new(HashMap::new());
}

/// The node is gone.
pub fn forget(id: NodeId) {
    WEB_ELEMENTS.with(|e| e.borrow_mut().remove(&id));
    WEBS.with(|w| w.borrow_mut().remove(&id));
    WAITING.with(|w| w.borrow_mut().retain(|i| *i != id));
}

/// The `CoreWebView2` of node `id` is ready: settings, bridge, events,
/// then the page.
fn initialized(id: NodeId, element: &WebView2) -> Result<()> {
    let Some(state) = state_of(id) else {
        return Ok(());
    };
    let core = element.CoreWebView2()?;
    settings(&core, &state.borrow().config)?;
    bridge(&state, &core)?;
    forms(&state, &core)?;
    // A sandbox without `forms` posts no document (a frame's included).
    let frames = core.AddWebResourceRequestedFilter2(
        &h("*"),
        CoreWebView2WebResourceContext::Document,
        CoreWebView2WebResourceRequestSourceKinds::All,
    );
    if frames.is_err() {
        // An older runtime: the main frame's documents.
        core.AddWebResourceRequestedFilter(&h("*"), CoreWebView2WebResourceContext::Document)?;
    }
    core.WebResourceRequested(&TypedEventHandler::<
        CoreWebView2,
        CoreWebView2WebResourceRequestedEventArgs,
    >::new(move |_, a| {
        let (Some(a), Some(state)) = (a.as_ref(), state_of(id)) else {
            return Ok(());
        };
        if !state.borrow().blocks_forms()
            || !a
                .Request()?
                .Method()?
                .to_string()
                .eq_ignore_ascii_case("POST")
        {
            return Ok(());
        }
        let uri = a.Request()?.Uri()?.to_string();
        log!("web view {id}: a form posting to {uri} held back (sandbox without forms)");
        if let Some(env) = ENV.with(|e| e.borrow().clone()) {
            a.SetResponse(&env.CreateWebResourceResponse(
                None::<&windows::Storage::Streams::IRandomAccessStream>,
                403,
                &h("Forbidden"),
                &h(""),
            )?)?;
        }
        Ok(())
    }))?;
    // The main frame stays on the allowed origins: a link elsewhere opens
    // in the browser.
    core.NavigationStarting(&TypedEventHandler::<
        CoreWebView2,
        CoreWebView2NavigationStartingEventArgs,
    >::new(move |_, a| {
        let (Some(a), Some(state)) = (a.as_ref(), state_of(id)) else {
            return Ok(());
        };
        let uri = a.Uri()?.to_string();
        let web = matches!(uri.split(':').next(), Some("http" | "https"));
        if a.IsUserInitiated()? && web && !state.borrow().allows(page_origin(&uri).as_deref()) {
            a.SetCancel(true)?;
            open_externally(&uri);
            return Ok(());
        }
        state.borrow_mut().loaded = false;
        Ok(())
    }))?;
    core.NavigationCompleted(&TypedEventHandler::<
        CoreWebView2,
        CoreWebView2NavigationCompletedEventArgs,
    >::new(move |s, a| {
        let (Some(core), Some(state)) = (s.as_ref(), state_of(id)) else {
            return Ok(());
        };
        let ok = a.as_ref().and_then(|a| a.IsSuccess().ok()).unwrap_or(false);
        if !ok {
            return Ok(());
        }
        let pending = {
            let mut st = state.borrow_mut();
            st.loaded = true;
            std::mem::take(&mut st.queue)
        };
        for json in pending {
            deliver(&state, core, &json);
        }
        if state.borrow().config.loads {
            let url = core.Source()?.to_string();
            dispatch(UiEvent::WebLoad(id, url));
        }
        Ok(())
    }))?;
    core.WebMessageReceived(&TypedEventHandler::<
        CoreWebView2,
        CoreWebView2WebMessageReceivedEventArgs,
    >::new(move |_, a| {
        let (Some(a), Some(state)) = (a.as_ref(), state_of(id)) else {
            return Ok(());
        };
        let st = state.borrow();
        if !st.config.messages {
            return Ok(());
        }
        let Some(origin) = page_origin(&a.Source()?.to_string()) else {
            return Ok(());
        };
        if !st.allows(Some(&origin)) {
            return Ok(());
        }
        // `prismHost` posts strings of JSON; anything else is not ours.
        let Ok(json) = a.TryGetWebMessageAsString().map(|s| s.to_string()) else {
            return Ok(());
        };
        if serde_json::from_str::<serde_json::Value>(&json).is_err() {
            return Ok(());
        }
        dispatch(UiEvent::WebMessage(id, json, origin));
        Ok(())
    }))?;
    core.NewWindowRequested(&TypedEventHandler::<
        CoreWebView2,
        CoreWebView2NewWindowRequestedEventArgs,
    >::new(move |_, a| {
        let Some(a) = a.as_ref() else { return Ok(()) };
        // Never a WebView2 window of its own: the browser, if allowed.
        a.SetHandled(true)?;
        let popups =
            state_of(id).is_some_and(|s| s.borrow().config.sandbox.is_none_or(|s| s.popups));
        if popups {
            open_externally(&a.Uri()?.to_string());
        }
        Ok(())
    }))?;
    core.ProcessFailed(&TypedEventHandler::<
        CoreWebView2,
        CoreWebView2ProcessFailedEventArgs,
    >::new(move |s, _| {
        // The page's process died: load it again.
        if let Some(core) = s.as_ref() {
            let _ = core.Reload();
        }
        Ok(())
    }))?;
    let url = state.borrow().url.clone();
    state.borrow_mut().core = Some(core.clone());
    if !url.is_empty() {
        navigate(&core, &url);
    }
    Ok(())
}

/// The origin of the page at `url`: `file://` for a local file (as
/// WebKit reports it), none for `about:blank` or `data:`.
fn page_origin(url: &str) -> Option<String> {
    origin_of(url).or_else(|| {
        url.get(..5)
            .is_some_and(|s| s.eq_ignore_ascii_case("file:"))
            .then(|| "file://".to_string())
    })
}

/// JavaScript, `postMessage` from the page.
fn settings(core: &CoreWebView2, c: &WebViewConfig) -> Result<()> {
    let s = core.Settings()?;
    s.SetIsScriptEnabled(c.scripts())?;
    s.SetIsWebMessageEnabled(c.messages)?;
    Ok(())
}

/// Installs the script stopping forms (or takes it away) as the sandbox
/// says.
fn forms(state: &Rc<RefCell<State>>, core: &CoreWebView2) -> Result<()> {
    let block = state.borrow().blocks_forms();
    let installed = state.borrow().no_forms.is_some();
    if block == installed {
        return Ok(());
    }
    if let Some(old) = state.borrow_mut().no_forms.take() {
        core.RemoveScriptToExecuteOnDocumentCreated(&old)?;
        return Ok(());
    }
    let op = core.AddScriptToExecuteOnDocumentCreatedAsync(&h(NO_FORMS))?;
    // Installed from now; the page shown is told now too.
    let _ = core.ExecuteScriptAsync(&h(NO_FORMS));
    let weak = Rc::downgrade(state);
    when_done(op, move |id| {
        if let (Ok(id), Some(state)) = (id, weak.upgrade()) {
            state.borrow_mut().no_forms = Some(id);
        }
    });
    Ok(())
}

/// Installs `prismHost` (or takes it away) as the config says.
fn bridge(state: &Rc<RefCell<State>>, core: &CoreWebView2) -> Result<()> {
    if let Some(old) = state.borrow_mut().bridge.take() {
        core.RemoveScriptToExecuteOnDocumentCreated(&old)?;
    }
    if !state.borrow().config.messages {
        return Ok(());
    }
    let op = core.AddScriptToExecuteOnDocumentCreatedAsync(&h(BRIDGE))?;
    let weak = Rc::downgrade(state);
    when_done(op, move |id| {
        if let (Ok(id), Some(state)) = (id, weak.upgrade()) {
            state.borrow_mut().bridge = Some(id);
        }
    });
    Ok(())
}

fn navigate(core: &CoreWebView2, url: &str) {
    if url.is_empty() {
        return;
    }
    if let Err(e) = core.Navigate(&h(url)) {
        log!("web view: cannot load {url}: {e}");
    }
}

/// Delivers `json` to the page as a `message` event, if it shows an
/// allowed origin.
fn deliver(state: &Rc<RefCell<State>>, core: &CoreWebView2, json: &str) {
    let source = core.Source().map(|s| s.to_string()).unwrap_or_default();
    if !state.borrow().allows(page_origin(&source).as_deref()) {
        return;
    }
    // `json` is valid JSON (checked by Prism), so a JavaScript literal.
    let script =
        format!("window.dispatchEvent(new MessageEvent(\"message\", {{ data: {json} }}));");
    let _ = core.ExecuteScriptAsync(&h(&script));
}

/// `cx.post_web_message`: now if the page is loaded, else once it is.
pub fn post(web: &Web, json: &str) {
    let (core, loaded) = {
        let st = web.state.borrow();
        (st.core.clone(), st.loaded)
    };
    match core {
        Some(core) if loaded => deliver(&web.state, &core, json),
        _ => web.state.borrow_mut().queue.push(json.to_string()),
    }
}

/// `cx.reload_web_view`.
pub fn reload(web: &Web) {
    let (core, url) = {
        let mut st = web.state.borrow_mut();
        st.loaded = false;
        (st.core.clone(), st.url.clone())
    };
    if let Some(core) = core {
        let shown = core.Source().map(|s| s.to_string()).unwrap_or_default();
        if shown.is_empty() || shown == "about:blank" {
            navigate(&core, &url);
        } else if let Err(e) = core.Reload() {
            log!("web view: reload: {e}");
        }
    }
}

/// Web links, mail and phone numbers open in their app.
fn open_externally(url: &str) {
    let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "mailto" | "tel") {
        return;
    }
    let target = wide(url);
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(target.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

// --- Awaiting WinRT operations on the UI thread ------------------------------------------

type Waiting = Box<dyn FnMut() -> bool>;

thread_local! {
    static PENDING: RefCell<Vec<Waiting>> = const { RefCell::new(Vec::new()) };
    static POLLING: Cell<usize> = const { Cell::new(0) };
}

/// Runs `then` with `op`'s result once it completes, on the UI thread.
fn when_done<T: windows_core::RuntimeType + 'static>(
    op: IAsyncOperation<T>,
    then: impl FnOnce(Result<T>) + 'static,
) {
    let mut then = Some(then);
    PENDING.with(|p| {
        p.borrow_mut().push(Box::new(move || {
            let status = op.Status().unwrap_or(AsyncStatus::Error);
            if status == AsyncStatus::Started {
                return false;
            }
            if let Some(f) = then.take() {
                f(op.GetResults());
            }
            true
        }))
    });
    if POLLING.get() == 0 {
        let timer = unsafe { SetTimer(None::<HWND>, 0, 20, Some(poll)) };
        POLLING.set(timer);
    }
}

unsafe extern "system" fn poll(_: HWND, _: u32, _: usize, _: u32) {
    let mut list = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
    list.retain_mut(|f| !f());
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        // Those added meanwhile go after.
        list.append(&mut p);
        *p = list;
        if p.is_empty() {
            unsafe {
                let _ = KillTimer(None::<HWND>, POLLING.replace(0));
            }
        }
    });
}
