//! The app's widget code in its own WidgetKit extension (#331), shared by
//! the UIKit and AppKit backends.
//!
//! An app whose widgets refresh without it (`Widget::refresh`) ships its
//! binary twice: as the app, and as the widget extension's executable, with
//! the extension's Swift linked in (`prism-apple-build`, the
//! `prism_widget_extension` cfg of the backend). In the `.appex`, the app's
//! `main` reaches `Application::run`, and the backend calls
//! [`run`](crate::apple::widget_extension::run) instead of starting its UI:
//! it hands the process to WidgetKit (NSExtensionMain, which finds the
//! widget bundle Swift recorded as the `@main` entry).
//!
//! Each time WidgetKit asks for a timeline, the extension's provider calls
//! `prism_widget_refresh(kind, token)` on the main thread: a runtime without
//! a window, started from the app's `init` on the first request, runs the
//! widget's refresh hook, writes the drawings it brings to the App Group
//! (without reloading the timelines: the extension is building one), then
//! releases the timeline (`done(token)`, Swift's
//! `prism_widget_refresh_done`), which is built from them.
//!
//! The runtime draws only the widgets whose hook succeeded
//! ([`App::set_widgets_refresh_only`](prism_core::App::set_widgets_refresh_only)):
//! the others keep the drawings the app wrote. Everything else the app's
//! code asks the platform for (notifications, background tasks, Live
//! Activities…) is ignored here: the app does it when it runs.

use std::cell::{Cell, RefCell};
use std::ffi::{CString, c_char};
use std::time::Instant;

use prism_core::{PlatformRequest, Runtime, WakeUi};

/// The runtime of the extension and the timelines waiting on it.
struct Extension {
    runtime: Runtime,
    /// (refresh task, token) of each timeline waiting.
    waiting: Vec<(u64, u64)>,
}

/// How to make the runtime, and the Swift function that releases a
/// waiting timeline ([`run`]).
type Setup = (Box<dyn Fn() -> Runtime>, fn(u64));

thread_local! {
    static EXTENSION: RefCell<Option<Extension>> = const { RefCell::new(None) };
    static SETUP: RefCell<Option<Setup>> = const { RefCell::new(None) };
    static NEXT_WAKE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Whether this process is an app extension (the binary runs from a
/// `.appex` bundle): the widget extension of an app with refresh hooks.
pub fn is_app_extension() -> bool {
    objc2_foundation::NSBundle::mainBundle()
        .bundlePath()
        .to_string()
        .trim_end_matches('/')
        .ends_with(".appex")
}

/// Main thread: hands the process to WidgetKit. `make` makes the runtime
/// (from the app's `init`, with the backend's storage and environment, and
/// [`wake_ui`]) on the first refresh; `done` is the Swift function that
/// releases a waiting timeline. Only for a binary with the extension's
/// Swift linked in. Never returns.
pub fn run(make: impl Fn() -> Runtime + 'static, done: fn(u64)) -> ! {
    unsafe extern "C" {
        fn NSExtensionMain(argc: i32, argv: *mut *mut c_char) -> i32;
        fn _NSGetArgc() -> *mut i32;
        fn _NSGetArgv() -> *mut *mut *mut c_char;
    }
    SETUP.with(|s| *s.borrow_mut() = Some((Box::new(make), done)));
    // What a Swift-only extension's entry point is.
    let code = unsafe { NSExtensionMain(*_NSGetArgc(), *_NSGetArgv()) };
    std::process::exit(code)
}

/// The runtime's `wake_ui`: polls it on the main thread.
pub fn wake_ui() -> WakeUi {
    std::sync::Arc::new(|| dispatch2::DispatchQueue::main().exec_async(tick))
}

/// `PrismWidgetKit.swift`, main thread: WidgetKit asks widget `kind` for a
/// timeline; the provider waits for `prism_widget_refresh_done(token)`,
/// called at once when there is no refresh to wait for.
///
/// # Safety
///
/// `kind` is a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prism_widget_refresh(kind: *const c_char, token: u64) {
    let kind = unsafe { std::ffi::CStr::from_ptr(kind) }
        .to_string_lossy()
        .into_owned();
    let started = EXTENSION.with(|e| {
        let mut e = e.borrow_mut();
        if e.is_none() {
            let runtime = SETUP.with(|s| s.borrow().as_ref().map(|(make, _)| make()))?;
            *e = Some(Extension {
                runtime,
                waiting: Vec::new(),
            });
        }
        let x = e.as_mut()?;
        let task = x.runtime.refresh_widget(&kind);
        if let Some(task) = task {
            x.waiting.push((task, token));
        }
        Some(task.is_some())
    });
    if started != Some(true) {
        release(&[token]);
    }
    tick();
}

/// Polls the runtime, then wakes the main thread again when its next timer
/// is due (a hook's `sleep`, its timeout).
fn tick() {
    let Some(at) = poll() else { return };
    if NEXT_WAKE.with(|w| w.get().is_some_and(|w| w <= at)) {
        return;
    }
    NEXT_WAKE.with(|w| w.set(Some(at)));
    let delay = at.saturating_duration_since(Instant::now());
    let when = dispatch2::DispatchTime::try_from(delay).unwrap_or(dispatch2::DispatchTime::NOW);
    let _ = dispatch2::DispatchQueue::main().after(when, || {
        NEXT_WAKE.with(|w| w.set(None));
        tick();
    });
}

/// Runs the app's ready tasks, writes the drawings they brought, and
/// releases the timelines whose refresh ended. Returns when the next timer
/// is due.
fn poll() -> Option<Instant> {
    let (ended, next) = EXTENSION.with(|e| {
        let Ok(mut e) = e.try_borrow_mut() else {
            return (Vec::new(), None);
        };
        let Some(x) = e.as_mut() else {
            return (Vec::new(), None);
        };
        x.runtime.poll_headless();
        for request in x.runtime.take_requests() {
            handle(request);
        }
        let (ended, running): (Vec<_>, Vec<_>) = x
            .waiting
            .drain(..)
            .partition(|(task, _)| !x.runtime.is_task_running(*task));
        x.waiting = running;
        let ended: Vec<u64> = ended.into_iter().map(|(_, token)| token).collect();
        (ended, x.runtime.next_timer())
    });
    release(&ended);
    next
}

fn release(tokens: &[u64]) {
    let Some(done) = SETUP.with(|s| s.borrow().as_ref().map(|(_, done)| *done)) else {
        return;
    };
    for token in tokens {
        done(*token);
    }
}

/// The extension's part of the platform: the drawings, written for the
/// timeline being built.
fn handle(request: PlatformRequest) {
    unsafe extern "C" {
        fn prism_widgets_save(kind: *const c_char, json: *const c_char) -> bool;
    }
    if let PlatformRequest::ReloadWidgets(snaps) = request {
        for w in snaps {
            let (Ok(kind), Ok(json)) = (CString::new(w.kind.clone()), CString::new(w.to_json()))
            else {
                continue;
            };
            if !unsafe { prism_widgets_save(kind.as_ptr(), json.as_ptr()) } {
                eprintln!(
                    "prism: widget extension: could not write widget {:?} (App Group missing?)",
                    w.kind
                );
            }
        }
    }
}
