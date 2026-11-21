# Remote push notifications

The app registers and gets a subscription to give your server; the server sends with the `prism-push` crate. Payloads use the same keys everywhere (`title`, `subtitle`, `body`, `link`, custom `data`), so a tap opens `link` on every platform.

```rust
// At startup: a push can launch the app.
cx.on_push_token(|sub, app| upload(app, sub.to_json()));   // the first token, and every refresh
cx.on_push(|push, app| match push.event {
    PushEvent::Received => {}                               // arrived while the app is open
    PushEvent::Opened => {}                                 // tapped (its `link` is already open)
    PushEvent::Background => {}                             // silent (data only)
});
cx.on_background_push(|push, cx| async move { sync(&cx).await });  // the system waits for it

cx.spawn(async move |_, cx| {
    let sub = cx.register_for_push().await?;   // asks to show notifications first
    // sub.service: Apns / Fcm / WebPush / UnifiedPush
});

// Live Activities updated from the server (iOS): their tokens.
cx.start_activity(Activity::new("focus").push_updates() /* … */);
cx.on_activity_push_token(|t, app| { /* t.activity (None: push-to-start), t.kind, t.token */ });
```

On the server (Axum, a worker…):

```rust
use prism_push::{Push, Sender, Subscription, apns::ApnsKey, fcm::ServiceAccount, webpush::VapidKey};

let sender = Sender::new()
    .apns(ApnsKey::from_p8(&p8, "KEYID12345", "TEAMID1234")?)   // token-based .p8 key, HTTP/2
    .fcm(ServiceAccount::from_json(&service_account_json)?)     // FCM HTTP v1, OAuth (RS256)
    .vapid(VapidKey::from_base64(&vapid_private)?, "mailto:ops@example.com");
let sub = Subscription::from_json(&json_from_the_app)?;
match sender.send(&sub, &Push::new().title("Order shipped").body("Tomorrow").link("/orders/42")).await {
    Err(prism_push::PushError::Gone) => forget(sub),   // uninstalled, token changed
    r => r?,
}
// Also: Push::new().silent().data("sync", "inbox"), .badge(3), .collapse_id(..), .ttl(..),
// .urgency(..), .live_activity(&Activity::…content_state_json(), end) / .start_live_activity(kind, id, ..)
```

**Live Activities from the server** (iOS). `on_activity_push_token` reports three kinds of token, each again whenever iOS changes it:

- `activity: None`: the push-to-start token (iOS 17.2+). Send it `Push::new().title(..).start_live_activity("delivery", id, &Activity::…content_state_json())` to start an activity of that kind.
- `activity: Some(id)` for an activity the app started with `.push_updates()`: `id` is what `start_activity` returned.
- `activity: Some(id)` for an activity a push started: `id` is the one the push gave it. The push carries it in the activity's attributes (`"attributes": {"kind": "delivery", "id": "42"}`, a decimal string), so the token tells your server which of the activities it started this is. Send that token `Push::live_activity(.., end)` to update or end it. Pick ids that the app's own activities will not use: they count from 1 at each launch. A timestamp or a random number works.

Prism registers the activities it did not start in this run: ones started by a push, and ones from an earlier run of the app. It picks them up at launch and as they start. iOS wakes the app in the background when a push starts one, which gives it time to report the token. The app can then `update_activity` or `end_activity` them with the same id. If one of the app's running activities already has the id, or the push gave none, the activity gets a new id, and its token arrives with that id and its `kind`.

| | Service | How | Silent pushes | Verified |
|---|---|---|---|---|
| iOS, iPadOS | APNs | `registerForRemoteNotifications`; app delegate in Rust | `content-available` → `on_background_push` (~30 s) | simulator: real sandbox token, `simctl push` shown in the foreground, Live Activity and push-to-start tokens |
| tvOS, visionOS | APNs | same UIKit delegate | yes | compiled |
| macOS | APNs | `NSApplication` delegate (bridge) | `didReceiveRemoteNotification` | compiled (needs a Mac provisioning profile) |
| watchOS | APNs | `WKApplication` delegate | `didReceiveRemoteNotification:fetchCompletionHandler:` | compiled |
| Android | FCM (opt-in) or UnifiedPush | `FirebaseMessagingService` / UnifiedPush connector (AND_3) | data messages | Firebase bundled without Gradle and initialized on the emulator; UnifiedPush built (its decryption is Linux's, verified) |
| Web | Web Push | service worker `<base>/prism-sw.js`, `pushManager.subscribe` with VAPID | – (browsers always show one) | service worker, session events, click → focus/open |
| Linux | UnifiedPush over D-Bus | `org.unifiedpush.Connector2` on the session bus | yes, while the app runs | end to end with a local distributor |
| Windows | — (WNS needs package identity: an MSIX or a sparse package) | `register_for_push` fails with `Unavailable` | — | — |

`cx.capabilities().push` says which service `register_for_push` will use (`None`: unavailable, and the call fails with `Unavailable`).

**Configuration** (`Prism.toml`):

```toml
[push]
enabled = true
apns_environment = "development"            # "production" for TestFlight / App Store
google_services = "google-services.json"    # Android: FCM (otherwise UnifiedPush only)
vapid_public_key = "BKv…"                   # Web Push (or PRISM_VAPID_PUBLIC_KEY), and UnifiedPush's VAPID
```

- **Apple:** `prism` adds the `aps-environment` entitlement (`com.apple.developer.aps-environment` on macOS), the `remote-notification` background mode and `PrismApsEnvironment` (the token's sandbox flag). Devices need the Push Notifications capability on the App ID (the profile's value wins). In the simulator the entitlement lives in the binary's entitlement sections only: the simulator refuses to launch an ad hoc signature that claims it. macOS restricts it: it needs `PRISM_MAC_PROFILE`, otherwise `capabilities().push` is `None`. Slimmed simulators keep the push daemons (`simslim … --except store`).
- **Test in the simulator:** `xcrun simctl push <device> dev.prism.counter examples/counter/push/hello.apns` (an `.apns` file is the payload; `cargo run -p prismsdk-push --example push -- apns-payload '{"title":"Hi","link":"/push"}'` writes one). The simulator delivers alerts (foreground banner, `Received`), but rejects pure silent pushes (`UNErrorDomain 1401`, "no user visible content") and turns `content-available` into a background fetch without the payload: test silent pushes on a device. `simctl push` cannot start a Live Activity either: a push-to-start payload shows up as a plain alert. Send push-to-start pushes through APNs.
- **Android, FCM:** Firebase normally needs Gradle. Here the CLI fetches `firebase-messaging` and its dependencies from Google's Maven (Play services client libraries, Kotlin, AndroidX: about 75 artifacts, 2 dex files), merges their manifests (`crates/prism-cli/platform/scripts/android-manifest-merge.py`), and turns `google-services.json` into the resources Firebase reads. Firebase is Apache 2.0; the Play services client libraries are under the Android SDK license, redistributable inside apps (what every Gradle build ships). FCM needs Google Play services on the device, recent enough for the bundled `firebase-messaging` (26.0.2 requires Play services 26.12; older devices and emulator images fall back to UnifiedPush); auto-init is off, so a token is requested only by `register_for_push`. Prism sends **data messages**, which reach the app in the foreground and the background; Rust shows visible ones.
- **Android, UnifiedPush:** an open standard with no library: the app broadcasts `REGISTER` to a distributor (ntfy, NextPush, Sunup…), receives `NEW_ENDPOINT`/`MESSAGE` broadcasts, and decrypts the Web Push message in Rust (RFC 8291). The subscription carries the device's keys, so the server sends Web Push to the endpoint (`Service::UnifiedPush`). No distributor installed and no FCM: `capabilities().push` is `None`.
- **Web:** the page registers the service worker on `register_for_push` (it needs a VAPID key; generate one with `cargo run -p prismsdk-push --example push -- vapid`). Pushes reach open pages as `Received`; with no visible page the worker shows a notification, and a click focuses the app (or opens it on the push's `link`) and reports `Opened`. Browsers require every push to show something: `silent` only skips the sound.
- **Linux:** there is no desktop push service. Prism speaks UnifiedPush's D-Bus protocol (`org.unifiedpush.Distributor2`), so a distributor such as KUnifiedPush delivers Web Push messages while the app runs (Prism apps are not D-Bus-activatable services). `crates/prism-linux/examples/fake_distributor.rs` is a local distributor for tests.
- **Pushes shown by Prism** (UnifiedPush, FCM data, Linux) become local notifications when the app is not in the foreground; tapping one is an `Opened` push.

`prism-push` is plain Rust on RustCrypto (`p256`, AES-GCM, HKDF) and ring (RS256 for Google's OAuth), with an HTTP/2 `reqwest` client (rustls). It is tested against RFC 8291's example (intermediate values, encryption and decryption), RFC 8292's VAPID token, and RFC 7515's ES256 and RS256 signatures. Every service also builds plain `HttpRequest`s for servers with another HTTP client. Without its `send` feature it is pure Rust: the Android and Linux receivers use it to decrypt.

**Endpoints chosen by subscribers.** A Web Push or UnifiedPush endpoint comes from whoever subscribed, so `Sender` posts only to public HTTPS servers on port 443: it checks every address a name resolves to before connecting (a name with any private, loopback, link-local or reserved address is refused, IPv4 written as IPv6 included), never follows redirects, ignores `HTTPS_PROXY` and similar variables, and reads at most 16 KiB of an answer. `Subscription::from_json` refuses endpoints that are not HTTP(S) URLs or carry credentials. A self-hosted UnifiedPush distributor on your network needs `Sender::new().trust_origin("http://ntfy.internal:8080")?` (exactly that scheme, host and port); an egress proxy needs `.proxy("http://proxy:3128")?`, and then the proxy must refuse private destinations itself. `webpush::request`, for your own HTTP client, refuses non-public endpoints too; your client must also check resolved addresses and not follow redirects.
