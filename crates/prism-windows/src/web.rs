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
//! - the app's own schemes (#343, `Application::web_scheme("app", …)`) are
//!   WebView2 custom schemes (`CoreWebView2CustomSchemeRegistration`, with
//!   an authority, secure, requests from its own pages allowed): every
//!   request to `app://…`, in any frame and of any kind, is answered by
//!   the app's handler on a background thread (`WebResourceRequested` with
//!   a deferral). The page is at `app://main/…`, its origin `app://main`.
//!   Nothing is registered when the app serves no scheme.
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
use windows::Storage::Streams::IRandomAccessStream;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::IStream;
use windows::Win32::System::WinRT::{
    BSOS_DEFAULT, CreateRandomAccessStreamOverStream, CreateStreamOverRandomAccessStream,
};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use windows_core::{HSTRING, Result};
use windows_future::{AsyncStatus, IAsyncOperation};

use crate::imp::dispatch;
use crate::links;
use crate::widgets::Win;
use crate::win::log;
use crate::winui::bindings::Microsoft::Web::WebView2::Core::{
    CoreWebView2, CoreWebView2CustomSchemeRegistration, CoreWebView2Environment,
    CoreWebView2EnvironmentOptions, CoreWebView2NavigationCompletedEventArgs,
    CoreWebView2NavigationStartingEventArgs, CoreWebView2NewWindowRequestedEventArgs,
    CoreWebView2ProcessFailedEventArgs, CoreWebView2WebMessageReceivedEventArgs,
    CoreWebView2WebResourceContext, CoreWebView2WebResourceRequestSourceKinds,
    CoreWebView2WebResourceRequestedEventArgs,
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
        register_schemes(&options);
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
    // The app's schemes (#343): every request, in any frame.
    for scheme in prism_core::web_view::web_schemes() {
        let pattern = h(&format!("{scheme}://*"));
        let all = core.AddWebResourceRequestedFilter2(
            &pattern,
            CoreWebView2WebResourceContext::All,
            CoreWebView2WebResourceRequestSourceKinds::All,
        );
        if all.is_err() {
            core.AddWebResourceRequestedFilter(&pattern, CoreWebView2WebResourceContext::All)?;
        }
    }
    core.WebResourceRequested(&TypedEventHandler::<
        CoreWebView2,
        CoreWebView2WebResourceRequestedEventArgs,
    >::new(move |_, a| {
        let (Some(a), Some(state)) = (a.as_ref(), state_of(id)) else {
            return Ok(());
        };
        let document = a.ResourceContext()? == CoreWebView2WebResourceContext::Document;
        let post = a
            .Request()?
            .Method()?
            .to_string()
            .eq_ignore_ascii_case("POST");
        if !(state.borrow().blocks_forms() && document && post) {
            if is_app_scheme(&a.Request()?.Uri()?.to_string()) {
                serve_scheme(id, a)?;
            }
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
            if crate::perf::on() {
                log!("web view {id}: a link to {uri} opens in the browser (another origin)");
            }
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
        // The blank page shown for no URL is not one the app loaded.
        let reports = {
            let st = state.borrow();
            st.config.loads && !st.url.is_empty()
        };
        if reports {
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
        // Never a WebView2 window of its own: the browser, if allowed and
        // asked for by the user (WebView2 has no popup blocker: a script
        // could open the browser on its own).
        a.SetHandled(true)?;
        let popups =
            state_of(id).is_some_and(|s| s.borrow().config.sandbox.is_none_or(|s| s.popups));
        let uri = a.Uri()?.to_string();
        match links::new_window(&uri, popups, a.IsUserInitiated()?) {
            Ok(()) => {
                if crate::perf::on() {
                    log!(
                        "web view {id}: a new window for {uri} opens in the browser (user gesture)"
                    );
                }
                open_externally(&uri);
            }
            Err(why) => log!(
                "web view {id}: a new window for {uri} not opened: {}",
                why.reason()
            ),
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

/// Registers the app's schemes (#343) on the environment's options:
/// `app://main/…` URLs with an authority (so `app://main` is an origin),
/// secure, and fetchable from the scheme's own pages (`app://*`: WebView2
/// checks the `Origin` of every request that has one, same-origin POSTs
/// included). Other origins are refused, opaque ones too.
fn register_schemes(options: &CoreWebView2EnvironmentOptions) {
    let schemes = prism_core::web_view::web_schemes();
    if schemes.is_empty() {
        return;
    }
    let r = (|| -> Result<()> {
        let list = options.CustomSchemeRegistrations()?;
        for scheme in &schemes {
            let reg = CoreWebView2CustomSchemeRegistration::CreateInstance(&h(scheme))?;
            reg.SetHasAuthorityComponent(true)?;
            reg.SetTreatAsSecure(1)?;
            reg.AllowedOrigins()?.Append(&h(&format!("{scheme}://*")))?;
            list.Append(&reg)?;
        }
        Ok(())
    })();
    if let Err(e) = r {
        log!("web views: the app's schemes {schemes:?} are not registered: {e}");
    }
}

/// Whether `url` is of a scheme the app serves (#343).
fn is_app_scheme(url: &str) -> bool {
    url.split_once(':')
        .is_some_and(|(s, _)| prism_core::web_view::is_web_scheme(&s.to_ascii_lowercase()))
}

/// Answers a request to the app's scheme (#343): the app's handler runs on
/// a background thread while WebView2 waits (a deferral); the answer is
/// set on the UI thread.
fn serve_scheme(id: NodeId, a: &CoreWebView2WebResourceRequestedEventArgs) -> Result<()> {
    let request = a.Request()?;
    let mut r =
        prism_core::SchemeRequest::new(request.Method()?.to_string(), request.Uri()?.to_string());
    let headers = request.Headers()?.First()?;
    while headers.HasCurrent()? {
        let kv = headers.Current()?;
        r.headers
            .push((kv.Key()?.to_string(), kv.Value()?.to_string()));
        headers.MoveNext()?;
    }
    if let Ok(content) = request.Content() {
        r.body = read_stream(&content);
    }
    let deferral = a.GetDeferral()?;
    let args = a.clone();
    let answer: std::sync::Arc<std::sync::Mutex<Option<prism_core::SchemeResponse>>> =
        Default::default();
    let filled = answer.clone();
    prism_core::web_view::serve_web_request(r, move |response| {
        *filled.lock().unwrap_or_else(|e| e.into_inner()) = Some(response);
    });
    when_ready(move || {
        let Some(response) = answer.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return false;
        };
        if let Err(e) = respond(&args, &response) {
            log!("web view {id}: an answer of the app's scheme was not sent: {e}");
        }
        let _ = deferral.Complete();
        true
    });
    Ok(())
}

/// Sets `response` as the answer to `args`'s request.
fn respond(
    args: &CoreWebView2WebResourceRequestedEventArgs,
    response: &prism_core::SchemeResponse,
) -> Result<()> {
    let Some(env) = ENV.with(|e| e.borrow().clone()) else {
        return Ok(());
    };
    let mut headers = String::new();
    for (name, value) in response.header_list() {
        headers.push_str(&format!("{name}: {value}\r\n"));
    }
    let stream: Option<IRandomAccessStream> =
        match unsafe { SHCreateMemStream(Some(&response.body)) } {
            Some(s) => Some(unsafe { CreateRandomAccessStreamOverStream(&s, BSOS_DEFAULT) }?),
            None => None,
        };
    args.SetResponse(&env.CreateWebResourceResponse(
        stream.as_ref(),
        i32::from(response.status),
        &h(response.reason()),
        &h(&headers),
    )?)
}

/// A request's body, read whole.
fn read_stream(content: &IRandomAccessStream) -> Vec<u8> {
    let Ok(stream) = (unsafe { CreateStreamOverRandomAccessStream::<_, IStream>(content) }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let mut read = 0u32;
        let hr = unsafe { stream.Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut read)) };
        if hr.is_err() || read == 0 {
            break;
        }
        out.extend_from_slice(&buf[..read as usize]);
    }
    out
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

/// Loads `url`; no URL (none given, or one `web_view` refused) shows a
/// blank page instead of the previous one, as an `<iframe>` without `src`.
fn navigate(core: &CoreWebView2, url: &str) {
    let url = if url.is_empty() { "about:blank" } else { url };
    if let Err(e) = core.Navigate(&h(url)) {
        log!("web view: cannot load {url}: {e}");
    }
}

/// Delivers `json` to the page as a `message` event, if it shows an
/// allowed origin, and still shows it when the script runs.
fn deliver(state: &Rc<RefCell<State>>, core: &CoreWebView2, json: &str) {
    let source = core.Source().map(|s| s.to_string()).unwrap_or_default();
    let origin = page_origin(&source);
    if !state.borrow().allows(origin.as_deref()) {
        return;
    }
    // `json` is valid JSON (checked by Prism), so a JavaScript literal.
    let script = prism_core::web_view::message_script(json, origin.as_deref());
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

/// Web links, mail and phone numbers open in their app, web pages with
/// `PRISM_BROWSER` when it is set (as `open_url` does).
fn open_externally(url: &str) {
    if !links::external_scheme(url) {
        return;
    }
    if let Err(e) = crate::services::open(url) {
        log!("web view: {url} not opened: {e}");
    }
}

// --- Awaiting WinRT operations on the UI thread ------------------------------------------

type Waiting = Box<dyn FnMut() -> bool>;

thread_local! {
    static PENDING: RefCell<Vec<Waiting>> = const { RefCell::new(Vec::new()) };
    static POLLING: Cell<usize> = const { Cell::new(0) };
}

/// Runs `ready` on the UI thread until it answers true (polled with the
/// async operations).
fn when_ready(ready: impl FnMut() -> bool + 'static) {
    PENDING.with(|p| p.borrow_mut().push(Box::new(ready)));
    if POLLING.get() == 0 {
        let timer = unsafe { SetTimer(None::<HWND>, 0, 20, Some(poll)) };
        POLLING.set(timer);
    }
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
