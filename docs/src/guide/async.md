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

**Background work** runs on a process-wide pool of at most 16 threads (`prism::DEFAULT_BACKGROUND_THREADS`), shared by every window and, on the web, every browser session. More calls at once wait in line, in order, instead of each starting an OS thread. Threads start on demand and stop after 20 seconds idle. Change the cap with `prism::set_background_threads(n)` (at startup, or any time: extra threads stop once the queue is empty) or the `PRISM_BACKGROUND_THREADS` environment variable; `prism::background_threads()` reads it. A panicking job is reported and does not stop the pool (its future never resolves).

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
