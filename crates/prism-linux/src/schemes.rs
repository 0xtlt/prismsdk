//! The app's own schemes in web views (#343, `Application::web_scheme`):
//! `app://main/…` served by the app's handler, from Rust.
//!
//! Each scheme is registered on the shared web context when it is made
//! (`web::context`, before the first view), as secure and CORS-enabled.
//! A request is read on the main loop (its body without blocking it),
//! answered by the handler on a background thread
//! (`prism_core::web_view::serve_web_request`, a panic answers 500), and
//! finished back on the main loop: once, whatever happened to the page
//! meanwhile (WebKit drops the answer of a load it cancelled).
//!
//! WebKitGTK reports the page at `app://main/…`, so its origin is
//! `app://main`, as on Apple platforms and Windows.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use gtk::glib;
use gtk::prelude::*;
use prism_core::web_view::{SchemeRequest, SchemeResponse, serve_web_request, web_schemes};

use crate::webkit::{self, WebContext};

thread_local! {
    /// Requests the handler is answering, by number: their references
    /// stay on the main thread, the background thread only has the
    /// number.
    static PENDING: RefCell<HashMap<u64, webkit::SchemeRequest>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u64> = const { Cell::new(0) };
}

/// Registers the app's schemes on `context` (none if it serves none).
pub fn register(context: &WebContext) {
    for scheme in web_schemes() {
        context.register_scheme(&scheme, received);
    }
}

/// A page asked for a URL of one of the app's schemes (main loop).
fn received(request: webkit::SchemeRequest) {
    let mut r = SchemeRequest::new(request.method(), request.uri());
    r.headers = request.headers();
    let Some(body) = request.body() else {
        answer(request, r);
        return;
    };
    // Read on the main loop without blocking it (WebKit hands over a
    // stream: a memory one, or one fed from the page's process).
    glib::MainContext::default().spawn_local(async move {
        loop {
            match body
                .read_bytes_future(64 * 1024, glib::Priority::DEFAULT)
                .await
            {
                Ok(chunk) if chunk.is_empty() => break,
                Ok(chunk) => r.body.extend_from_slice(&chunk),
                Err(e) => {
                    eprintln!(
                        "prism-linux: web view: the body of {} {} was not read: {e}",
                        r.method, r.url
                    );
                    request.fail("the request's body could not be read");
                    return;
                }
            }
        }
        answer(request, r);
    });
}

/// Hands `r` to the handler, off the main thread; its answer finishes
/// `request` on the main loop.
fn answer(request: webkit::SchemeRequest, r: SchemeRequest) {
    let id = NEXT.get();
    NEXT.set(id + 1);
    PENDING.with(|p| p.borrow_mut().insert(id, request));
    serve_web_request(r, move |response| {
        glib::MainContext::default().invoke(move || finish(id, response));
    });
}

/// The handler answered request `id` (main loop).
fn finish(id: u64, response: SchemeResponse) {
    // Taken out first: no borrow while WebKit runs.
    let request = PENDING.with(|p| p.borrow_mut().remove(&id));
    if let Some(request) = request {
        request.finish(response);
    }
}
