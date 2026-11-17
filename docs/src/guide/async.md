# Async, network, storage

```rust
// Runs on the UI thread; await timers or background work in between.
cx.spawn(async move |this, cx| {
    cx.sleep(Duration::from_secs(1)).await;
    let quote = cx.background(|| prism::http::Request::get("https://api.github.com/zen").send_blocking()).await;
    cx.update_entity(&this, |this, _| this.quote = quote.ok().map(|r| r.text()));
});

cx.storage().set("todos", &data);   // App Group (iOS), Application Support (macOS), per browser (web)
```

**HTTP** (`prism::http`, #250): requests follow up to 10 redirects, and a redirect to another origin (scheme, host, port) drops `Authorization`, `Proxy-Authorization`, `Cookie` and every header given with `.secret_header(name, value)`: an API token never reaches a third-party host. `.redirects(Redirects::None | SameOrigin | Follow(n))` changes that (a redirect not followed is the response). `.max_body(bytes)` fails past that size, from the `Content-Length` or as the body arrives. `send_stream_blocking(|chunk, progress| …)` and `cx.fetch_stream(request, on_chunk)` hand the body over in chunks, on the background thread (write a large export to disk; send `progress.fraction()` to the UI through a `prism::channel()` for a bar); returning `false` stops it.

```rust
use prism::http::{FetchStream, Redirects, Request};
let req = Request::get(url).secret_header("X-Api-Key", key).max_body(50 << 20);
let mut file = std::fs::File::create(&path)?;
cx.fetch_stream(req, move |chunk, _| file.write_all(chunk).is_ok()).await?;
```

**Background work** runs on a process-wide pool of at most 16 threads (`prism::DEFAULT_BACKGROUND_THREADS`), shared by every window and, on the web, every browser session. More calls at once wait in line, in order, instead of each starting an OS thread. Threads start on demand and stop after 20 seconds idle. Change the cap with `prism::set_background_threads(n)` (at startup, or any time: extra threads stop once the queue is empty) or the `PRISM_BACKGROUND_THREADS` environment variable; `prism::background_threads()` reads it. A panicking job is reported and does not stop the pool (its future never resolves).

**Threads the app does not own**: a blocking WebSocket client, a file watcher or a device SDK callback hands its values to the UI with `prism::channel()`. The `Sender` is `Send + Clone` and never blocks; sending wakes the UI thread (the same signal that resumes a task after background work), which polls the task awaiting `rx.recv()`: no polling loop, and no pool thread held for the life of the session. `recv()` returns `None` once every sender is dropped; `send` fails once the receiver is gone (its task ended or was cancelled), so the thread can stop. `try_recv()` and `drain()` take what is queued without waiting.

```rust
let (tx, mut rx) = prism::channel::<Event>();
std::thread::spawn(move || {
    for ev in socket {
        if tx.send(ev).is_err() { break }                      // the screen is gone
    }
});
cx.spawn(async move |this, cx| {
    while let Some(ev) = rx.recv().await {                     // on the UI thread
        cx.update_entity(&this, |this, _| this.apply(ev));
    }
})
.cancel_on_release();
```

**Tasks tied to a screen**: `cx.spawn` tasks run on after their entity is released (`update_entity` then returns `None`). Add `.cancel_on_release()` to stop them with it, for example when the user leaves the screen: the future is dropped, and background work it was waiting for that has not started yet is skipped. `.cancel_with(&entity)` ties any task to another entity.

```rust
cx.spawn(async move |this, cx| {
    loop {
        let orders = cx.background(|| api::orders()).await;    // on the pool
        cx.update_entity(&this, |this, _| this.orders = orders);
        cx.sleep(Duration::from_secs(30)).await;
    }
})
.cancel_on_release();                                          // stops when the screen closes
```

**On the web**, data loaded this way is not in the server-rendered first paint unless the app sets `WebApp::first_paint_wait(timeout)`: the page then waits, up to `timeout`, for the `background` work its first render started. See [Web](../platforms/web.md#data-in-the-first-paint).
