# prism-push

Send push notifications from your server to apps built with [Prism](https://github.com/0xtlt/prismsdk):

- **APNs** (iOS, iPadOS, macOS, tvOS, visionOS, watchOS): token-based authentication with a `.p8` key, HTTP/2, Live Activity updates.
- **FCM HTTP v1** (Android): service-account OAuth (RS256).
- **Web Push** (browsers) and **UnifiedPush**: RFC 8291 `aes128gcm` encryption and VAPID (RFC 8292).

The app side (`cx.register_for_push()`, `on_push`) is in the `prism` crate: it hands you the subscription to store, and this crate sends to it.

Without the default `send` feature it only builds payloads, JWTs and Web Push encryption/decryption in pure Rust (what the Android and Linux receivers use).

```sh
cargo run -p prismsdk-push --example push   # VAPID keys, .apns test files, encrypt and send
```

See the [Remote push notifications](https://0xtlt.github.io/prismsdk/guide/push.html) chapter.

Licensed under MIT OR Apache-2.0.
