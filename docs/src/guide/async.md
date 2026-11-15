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
