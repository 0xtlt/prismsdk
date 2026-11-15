# Images, long lists, gestures

```rust
picture("https://…/photo.jpg").alt("A river").fill()   // cached on disk; or picture("logo.png") from assets/
VirtualList::new(10_000, 56., |i| row(i))               // only the visible rows exist
scroll().on_refresh(handler).refreshing(self.loading)   // pull to refresh (UIRefreshControl)
div().on_swipe(SwipeDirection::Left, handler)           // swipe gestures
```
