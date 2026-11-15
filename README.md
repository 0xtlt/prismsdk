# Prism

Write a UI once in Rust (GPUI-style API), ship it as:

| Target | How it renders | Command |
|---|---|---|
| **Web** | Server-side Rust → HTML/CSS + a small JS client receiving DOM patches over a WebSocket. **No WASM.** | `prism dev web` → http://localhost:3000 |
| **macOS** | Real AppKit views (`NSView`, `NSTextField`, `NSButton`), menu bar, tray | `prism dev macos` |
| **iOS** | Real UIKit views (`UIView`, `UILabel`, `UIButton`, `UITextField`), widgets + Dynamic Island | `prism dev ios` (simulator) / `prism run ios --device "My iPhone"` |
| **Android** | Real Android views (`TextView`, `Button`, `EditText`, `Switch`), home-screen widgets, ongoing notifications | `prism dev android` (emulator or phone) |

## Quick start

```sh
mise run cli                 # installs the `prism` command
prism new my-app && cd my-app
prism dev web                # rebuilds and restarts on every save, on the same screen
prism dev ios                # same, in the iOS simulator
prism run ios --device "My iPhone"
prism build macos            # release .app with widgets and icon
prism publish ios --dry-run  # App Store build, checked (see Shipping to stores)
prism dev android            # the running emulator or a USB phone
prism devices                # simulators, paired iPhones, Android devices
```

`Prism.toml` configures the app: name, `bundle_id`, `version`, `icon` (one 1024×1024 PNG, compiled with `actool` for iOS and macOS), and `[ios] team` / `device` / `simulator`. Real devices are signed with your Apple Development identity and a matching provisioning profile (found automatically, or `PRISM_PROFILE`), then installed with `devicectl`. The bundling scripts and Apple sources are embedded in the CLI, so projects don't need this repository's scripts.

```rust
use prism::prelude::*;

struct Counter { count: i32 }

impl Render for Counter {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().p(24.).gap(12.)
            .child(text(format!("Count: {}", self.count)).text_size(32.).bold())
            .child(button("+1").on_click(cx.listener(|this, _, _| this.count += 1)))
    }
}

fn main() {
    Application::new().title("Counter").run(|cx| cx.new_entity(|_| Counter { count: 0 }));
}
```

## Updating state

- State lives in **entities** (`cx.new(|_| MyView { .. })`), GPUI-style.
- Mutate it in a `cx.listener`: the entity re-renders automatically. Elsewhere, call `cx.notify()`.
- Only notified entities re-render. A clean child entity is neither rendered nor diffed: its mounted views are reused as-is. So split your UI into entities where updates are frequent (an input, a live counter, a list row).
- Derived state: `cx.observe(&other, |this, other, cx| this.total = other.read(cx).total())` runs when `other` changes, then re-renders `this`.
- Each entity renders one root node (a fragment gets wrapped in a `div`).

## Async, network, storage

```rust
// Runs on the UI thread; await timers or background work in between.
cx.spawn(async move |this, cx| {
    cx.sleep(Duration::from_secs(1)).await;
    let quote = cx.background(|| prism::http::Request::get("https://api.github.com/zen").send_blocking()).await;
    cx.update_entity(&this, |this, _| this.quote = quote.ok().map(|r| r.text()));
});

cx.storage().set("todos", &data);   // App Group (iOS), Application Support (macOS), per browser (web)
```

## Responsive

```rust
// Styles, from a width up (like Tailwind): phones get a column, tablets a row.
div().flex_col().gap(12.).md(|s| s.flex_row().gap(24.))
text("Swipe to delete").hidden_from(Breakpoint::Medium)   // phones only
sidebar().visible_from(Breakpoint::Expanded)              // laptops and up

// Structure: anything else.
if cx.breakpoint() >= Breakpoint::Expanded { two_panes() } else { one_pane() }
cx.on_resize(|(w, h), app| { … });                        // every size change
```

| Breakpoint | Width | Typically |
|---|---|---|
| `Compact` | < 600 | phones |
| `Medium` (`md`) | ≥ 600 | tablets portrait, small windows, phones in landscape |
| `Expanded` (`lg`) | ≥ 1024 | iPad landscape / 13", laptops |
| `Large` (`xl`) | ≥ 1440 | desktops |

Rules are resolved in the core for the current window, so every backend agrees and the web needs no media queries: the iOS window (rotation, Split View, iPad), the macOS window (resize) and the browser (resize, plus a cookie so the server's first render already fits) report their size, and the app re-renders when it crosses a breakpoint. On wide browser windows the tab bar becomes a sidebar.

## Platform-specific UI

```rust
text("Swipe left to delete").only(Platform::MOBILE)       // iOS, Android
text("Right-click for more").except(Platform::MOBILE)     // macOS, Windows, Linux, web
apple_pay_button().only(Platform::APPLE)
div().p(16.).on(Platform::Web, |s| s.p(24.)).md(|s| s.flex_row())   // combine with breakpoints

match cx.platform() {
    Platform::Ios | Platform::Android => mobile_onboarding(),
    Platform::Web => landing_page(),
    _ => desktop_welcome(),
}
```

Platforms: `Ios`, `Android`, `Macos`, `Windows`, `Linux`, `Web`; sets with `|` or `Platform::MOBILE`, `DESKTOP`, `APPLE`, `NATIVE` (all but web). Excluded elements are left out of the tree (no hidden view). `cx.platform()` is the runtime platform (`Web` for the web backend, even on a phone browser: use breakpoints for screen size); `prism::PLATFORM` is the same at compile time. For code that only compiles on one platform, use `cfg`: `#[cfg(target_os = "ios")]`, `#[cfg(feature = "web")]` (your app's feature), or per-platform files with `#[cfg_attr(target_os = "ios", path = "share_ios.rs")]`.

## Device information

```rust
let d = cx.device_info();
if d.is_tablet() && d.is_landscape() { two_columns() } else { one_column() }
if d.os.version.at_least(18, 0) { new_feature() }
d.screen.diagonal            // Some(6.3) inches
if let Some(b) = &d.browser { /* web: "Safari", 18, WebKit, standalone (installed) */ }

div().flex_col().landscape(|s| s.flex_row())                // style rules, like breakpoints
    .posture(Posture::Tabletop, |s| s.justify_end())
    .on_device(DeviceKind::Tablet, |s| s.p(32.))
ui::dual_pane(list, detail)   // one pane per side of a foldable's hinge, side by side on tablets
cx.on_device_info_change(|d, cx| …);   // the tree already re-renders on rotation, fold, split view
```

| Field | iOS / iPadOS | macOS | Web |
|---|---|---|---|
| `kind` (phone, tablet, desktop, tv) | idiom | desktop | user agent + touch (iPads in desktop mode included) |
| `os`, `model`, `model_name` | `iOS 18.2`, `iPhone17,1`, `iPhone 16 Pro` | `macOS 15.1`, `Mac15,6` | the visitor's OS, Android model, client hints |
| `screen` (pt, scale, inches, ppi, Hz, cutout) | screen pixels ÷ the model's ppi (table generated from Xcode's simulator profiles: `scripts/apple-models.py`) | `CGDisplayScreenSize` | estimated inches |
| `orientation`, `rotation` | interface orientation | screen shape | `screen.orientation` |
| `posture`, `hinge_angle`, `fold` (Flat, Book, Tabletop, Closed) | `UIHingeInteraction` + division reserved regions (iOS 27.1+, iPhone Duo) | – | `devicePosture`, viewport segments |
| `window_mode` (fullscreen, split, slide over, windowed) | Split View / Stage Manager | fullscreen or windowed | installed app or browser tab |
| `browser` | `None` | `None` | always set |

On the web the server reads the `User-Agent` for the first HTML, then the page reports touch, screen, orientation, posture and client hints (and remembers them in a cookie for the next visit). Android: types ready, backend to come.

## Data: local, synced, SQL

```rust
cx.storage().set("draft", &text);              // this device (web: this browser, on the server)
cx.cloud_storage().set("theme", "dark");       // the user's devices: iCloud key-value store
cx.on_cloud_change(|keys, app| { … });         // another device changed something

// SQL with sqlx (`prism` feature `db`)
let db = prism::db::Db::open(cx, "app.db", &["CREATE TABLE notes (id INTEGER PRIMARY KEY, text TEXT)"])?;
cx.set_global(db);
cx.spawn(async move |this, cx| {
    let notes: Vec<(i64, String)> = Db::of(&cx)
        .run(&cx, |pool| async move { sqlx::query_as("SELECT id, text FROM notes").fetch_all(&pool).await })
        .await?;
});
```

| | `storage` | `cloud_storage` | `prism::db` (SQLite) |
|---|---|---|---|
| iOS | App Group container | iCloud KVS (`[ios] icloud = true`; signed builds) | on the device |
| macOS | Application Support | iCloud KVS (needs a Mac provisioning profile) | on the device |
| Web | server, per browser | server, per browser | **on the server, shared by every user**: the app runs there, so this is your backend |
| Android | app files | backed up by Android Backup (key/value), restored on a new device or a reinstall | on the device |

The database runs queries on a small Tokio runtime and hands results back to the UI thread. Migrations are SQL statements applied once each, in order (`PRAGMA user_version`). On the web the same sqlx code can point at Postgres instead of SQLite. `cx.capabilities().cloud_sync` says whether `cloud_storage` really syncs (iCloud signed in); `cloud_backup` that it is backed up without live sync (Android: `BackupAgent` + `BackupManager.dataChanged()` after each change; Block Store needs the Play services library, so it is not used); elsewhere it is local storage. Secrets go in [`secure_storage`](#security-secrets-biometrics-sign-in), which is never synced or backed up.

## Device APIs

```rust
cx.spawn(async move |this, cx| {
    if cx.request_permission(Permission::Location).await.is_granted() {
        let here = cx.current_location().await?;            // Location { latitude, longitude, accuracy, … }
    }
    let photos = cx.pick_images(PickImages::new().limit(3)).await?;  // picture(photo.path) shows them
    let photo = cx.take_photo().await?;
    let text = cx.read_clipboard().await;
});
let gyro = cx.watch_motion(Duration::from_millis(50), |m, app| { /* m.rotation_rate, m.attitude */ });
let follow = cx.watch_location(Accuracy::Best, |l, app| { … });
cx.stop_watching(gyro);
cx.haptic(Haptic::Success);   cx.copy_text("Hi");   cx.share(Share::text("Look").url(url));   cx.open_url(url);
```

| | iOS | macOS | Web |
|---|---|---|---|
| Permissions | system prompts (location, camera, photos, microphone, motion, notifications) | same (bundled app) | Permissions API, `getUserMedia` |
| Location | CoreLocation | CoreLocation (bundled app) | `navigator.geolocation` |
| Motion | CoreMotion (device only) | — | `devicemotion` |
| Photos / camera | PHPicker (no permission needed) / camera | open panel / — | `<input type=file>`, uploaded to the session |
| Haptics, clipboard, share, URLs | UIFeedbackGenerator, UIPasteboard, share sheet | trackpad, NSPasteboard, sharing picker | `vibrate`, Clipboard API, Web Share |

Usage texts go in `Prism.toml`; `prism` writes them to Info.plist (iOS refuses to ask without them):

```toml
[permissions]
location = "Shows todos near you."
camera = "Adds a photo to a todo."
```

`cx.capabilities()` says what exists (`location`, `motion`, `camera`, `photos`, `haptics`, `share`); unsupported calls fail with `DeviceError::Unavailable`.

## Security: secrets, biometrics, sign-in

```rust
cx.spawn(async move |this, cx| {
    // Secrets: never in plain files, never synced or backed up.
    cx.secure_storage().set("refresh_token", &token).await?;
    let token = cx.secure_storage().get("refresh_token").await?;            // Option<String>
    cx.secure_storage().set_with("pin", "1234", SecureOptions::new().biometric()).await?;  // read = Face ID…
    cx.secure_storage().delete("pin").await?;

    // Is it the owner? Face ID, Touch ID, Optic ID, a fingerprint, then the passcode.
    match cx.authenticate("Unlock your notes").await {
        Ok(()) => unlock(),
        Err(DeviceError::Cancelled) => {}
        Err(e) => show(e),
    }
    let b = cx.biometry().await;            // kind (FaceId, Fingerprint…), enrolled, passcode, locked_out

    // Sign in with Apple, and OAuth 2 / OpenID Connect with PKCE (Google, Microsoft, Auth0, Keycloak…).
    let apple = cx.sign_in_with_apple(AppleSignIn::new().email().full_name()).await?;
    let tokens = prism::oauth::OAuth::google("1234-abc.apps.googleusercontent.com").sign_in(&cx).await?;
    let email = tokens.id_claims().and_then(|c| c["email"].as_str().map(String::from));
});
```

| | `secure_storage` | `authenticate` (`capabilities().biometry`) | Sign in with Apple | `oauth` sign-in browser → redirect |
|---|---|---|---|---|
| iOS, iPadOS | Keychain (this device only; `biometric`: `SecAccessControl` `.biometryCurrentSet`, else `.userPresence`) | LocalAuthentication: Face ID / Touch ID, then the passcode | AuthenticationServices | `ASWebAuthenticationSession` → `prism-<app>:/oauth` (or the provider's scheme) |
| visionOS | Keychain | Optic ID, then the passcode | AuthenticationServices | `ASWebAuthenticationSession` |
| macOS | login keychain; data protection keychain with a provisioning profile | Touch ID, then the password | needs a Mac provisioning profile | `ASWebAuthenticationSession` |
| watchOS | Keychain (`biometric` = `.userPresence`) | passcode / wrist detection (no biometric policy on the watch) | AuthenticationServices | `ASWebAuthenticationSession` |
| tvOS | Keychain | — (the SDK marks `LAContext` unavailable) | AuthenticationServices | `ASWebAuthenticationSession` (tvOS 16+) |
| Android | Keystore: AES-256-GCM key in `AndroidKeyStore`, ciphertext in `files/secure` (`biometric`: an RSA key needing the owner wraps the value's key) | `BiometricPrompt` (framework, no androidx), then the PIN (Android 11+ with keys) | — | Custom Tab (protocol extra, no androidx) or the browser → deep link |
| Linux | Secret Service over D-Bus (GNOME Keyring, KWallet) | fprintd (fingerprint readers) | — | default browser → loopback `http://127.0.0.1:<port>/callback` |
| Web | on the server, per browser (the `HttpOnly` session cookie), AES-256-GCM at rest | WebAuthn platform authenticator with user verification, checked on the server | — | popup → `<base>/_prism/auth/callback` |

- **Biometric secrets** need the owner every time they are read; new fingers or faces make them unreadable (Apple, Android: store them again). Setting one never prompts.
- **Face ID** needs a usage text: `face_id = "…"` under `[permissions]` in Prism.toml (`NSFaceIDUsageDescription`). Without it the call fails with a message saying so instead of crashing.
- **Sign in with Apple** needs the `com.apple.developer.applesignin` entitlement: `[ios] sign_in_with_apple = true` takes it from the provisioning profile (enable the capability for the App ID). `capabilities().sign_in_with_apple` is true only in bundles built with it. Verify `identity_token` (or exchange `authorization_code`) on your server.
- **OAuth**: authorization code + PKCE (S256), random `state` checked on return, `nonce` and audience checked in the ID token. Public clients only on devices (no secret); `.code_only()` hands the code and verifier to your server instead. `OAuth::oidc(issuer, client_id)` discovers the endpoints. Google requires the reversed client id as the scheme for iOS clients (`OAuth::google` does it on Apple); for Android it only accepts App Links or Credential Manager (Play services), so use a provider that allows custom schemes, or bounce through your server.
- **Web secrets** are a server-side store, because the app runs there: the browser never sees them. Set `PRISM_SECRET_KEY` (64 hex digits) in production; otherwise a key is generated in the data directory. WebAuthn needs a secure context (`https://` or `localhost`); the first `authenticate` registers this browser's platform credential. When a popup or WebAuthn needs a fresh tap (Safari), the page shows a "Continue" button.
- **Linux** has no desktop standard for checking a password on behalf of an app (polkit authorizes system actions, not apps): `authenticate` uses fprintd, otherwise `Unavailable`. Biometric secrets are checked by the app with fprintd before the keyring hands them out.
- Test sign-in without real accounts: `python3 scripts/mock-oidc.py` (a local OpenID provider checking PKCE); `scripts/mock-fprintd.py` stands in for a fingerprint reader (`PRISM_FPRINTD_BUS=session`).

## Background execution

```rust
cx.on_lifecycle(|phase, app| if phase == Lifecycle::Background { save(app) });

// Finishes even if the user leaves the app (iOS: beginBackgroundTask, ~30 s).
cx.spawn(async move |this, cx| upload(&cx).await).finish_in_background();

// Periodic work, even when the app is closed (the system picks the moment).
cx.register_background_task(
    BackgroundTask::refresh("sync").every(Duration::from_secs(15 * 60)),
    |cx| async move { sync(&cx).await },
);
```

| | iOS | macOS | Web |
|---|---|---|---|
| Lifecycle | active / inactive / background | active / inactive / hidden or minimized | page focus / visibility |
| `finish_in_background` | `beginBackgroundTask` | keeps running | keeps running (server) |
| Periodic tasks | `BGTaskScheduler`; the app is relaunched without UI if needed | `NSBackgroundActivityScheduler` | timer while a page is open |

`prism build/run` declares the tasks in Info.plist (`BGTaskSchedulerPermittedIdentifiers`, `UIBackgroundModes`). The iOS Simulator cannot run `BGTaskScheduler`: use a device, or `PRISM_RUN_BACKGROUND=sync` to run a task at launch and `PRISM_BACKGROUND_INTERVAL=5` to shorten intervals (macOS, web).

## Dialogs, menus, tray

```rust
cx.show_alert(Alert::new("Delete “Milk”?").message("This cannot be undone.")
    .destructive("Delete", on_delete).cancel("Cancel"));          // UIAlertController, NSAlert, <dialog>
cx.present("/todos/new");                                          // sheet: page sheet, NSWindow sheet, <dialog>
div().context_menu(Menu::new().item(MenuItem::new("Open").icon("arrow.up.forward.square")))
cx.set_tray(Tray::new("focus").title("25:00").menu(menu));         // macOS menu bar (NSStatusItem)
cx.set_app_menu(vec![Menu::titled("File").item(MenuItem::new("New").shortcut("cmd+n"))]);
```

Context menus open with a long press (iOS), right click or ⌃-click (macOS), right click / Shift+F10 (web), and from VoiceOver.

## Images, long lists, gestures

```rust
picture("https://…/photo.jpg").alt("A river").fill()   // cached on disk; or picture("logo.png") from assets/
VirtualList::new(10_000, 56., |i| row(i))               // only the visible rows exist
scroll().on_refresh(handler).refreshing(self.loading)   // pull to refresh (UIRefreshControl)
div().on_swipe(SwipeDirection::Left, handler)           // swipe gestures
```

## Languages, text size, motion

One JSON file per language in `locales/` (`en.json`, `fr.json`, `ar.json`…), flat or nested, plurals as `{"one": …, "other": …}`. `build.rs` turns them into one function per key:

```rust
// build.rs
prism_build::translations("locales").fallback("en").generate();   // .strict() to fail instead of warn

// src/main.rs
mod strings { prism::include_strings!(); }
cx.set_translations(strings::translations());
text(strings::home_title(cx, &name));            // {name} → argument; a typo is a compile error
text(strings::todos(cx, n));                     // CLDR plurals: one/few/many/other…
text(cx.format_date(now, DateStyle::Long));      // Foundation formatters on Apple, Accept-Language on the web
cx.set_locale(Some(Locale::new("ar")));          // or follow the system
div().w(relative(fill)).animate(250)             // UIView/NSAnimationContext/CSS transitions
```

- **Build-time checks**: a key missing in a language, an unknown key, a placeholder the reference text lacks, or a plural without `other` is a cargo warning (`warning: counter: fr.json: missing "todos.add"`).
- **Native packaging**: `prism build/run` declares the languages to iOS/macOS (`CFBundleLocalizations` + `.lproj`), so iOS Settings offers a per-app language.
- `cx.t("key")` / `cx.t_with` / `cx.t_count` remain for dynamic keys.
- **Right-to-left** languages mirror layouts on every backend (native frames and chrome, `dir=rtl` + logical CSS).
- **Dynamic Type**: native text follows the iOS text size live; the web uses `rem`.
- **Reduce Motion** turns transitions off. **iOS keyboard**: screens shrink above it and the focused field stays visible.

## Motion

Transitions, transforms, enter/exit animations and loops, native on every backend: UIKit and Core Animation (iOS, tvOS, visionOS), AppKit (macOS), property animators (Android), SwiftUI (watchOS), CSS and the Web Animations API (web), GTK's frame clock (Linux).

```rust
div().spring(400)                                   // animate this element's changes (frame, colors, opacity, transform)
div().transition(Transition::ease_out(250).delay(100))
div().offset(0., -4.).scale(1.05).rotate(3.)        // visual only: the layout does not move

// Appear and disappear (keyed children: the removed one leaves)
div().key(id).enter(Pose::slide(0., 12.)).exit(Pose::slide(-40., 0.))
row.stagger(index, 40)                              // cascading entrances, 40 ms apart
image("heart.fill").repeat(Pose::new().scale(1.2), Transition::ease(700))   // pulse forever

// Animate everything one update changes (SwiftUI's withAnimation)
cx.with_animation(Transition::spring(400), |cx| store.update(cx, |s, cx| s.delete(id, cx)));
```

- **Curves**: `ease`, `ease_in`, `ease_out`, `linear`, `spring` (a slight overshoot), each with `.delay(ms)`.
- **Poses**: `Pose::fade()`, `Pose::slide(x, y)`, `Pose::pop()`, or `Pose::new().opacity(…).offset(…).scale(…).rotate(…)`; unset fields keep the element's values.
- **Exits** animate a picture of the removed view (snapshot, overlay, clone), so its subtree can be freed at once.
- Offsets follow the layout direction (mirrored right to left). **Reduce Motion** (system setting, `prefers-reduced-motion`, GTK's animations switch) turns all of it off.
- `prism::ui` components take `.enter()`, `.exit()` and `.transition()` too.

## Charts and vector drawing

`prism::ui` has charts, drawn natively with vector shapes: UIKit Bézier paths (iOS, tvOS, visionOS), Core Animation shape layers (macOS), `Canvas` (Android), SwiftUI (watchOS), SVG (web), cairo (Linux).

```rust
ui::line_chart()
    .series("This week", [1.2, 1.9, 1.6, 2.4, 2.1, 3.1, 2.8])
    .series("Last week", [1.0, 1.4, 1.5, 1.7, 1.9, 2.2, 2.0])
    .labels(["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"])
    .smooth().area().points().unit(" k€")

ui::bar_chart()
    .bars([("Mon", 12.), ("Tue", 18.), ("Wed", 9.)])
    .highlight(self.day)                                        // the others fade
    .on_select(cx.listener(|this, e, _| this.day = Some(e.index())))

ui::pie_chart().slice("Rent", 900.).slice("Food", 420.).donut("1 320 €")
ui::ring(0.72).label("72%").caption("Goal")                     // circular progress
ui::stat("Revenue", "12 480 €").delta("+12%", true).trend([8., 9.5, 9., 11.])
ui::sparkline([3., 5., 4., 8.])
```

- **Themed**: series use `Theme::chart_color(i)` (the accent first), adaptive in dark mode; axis labels use the app's number format.
- **Axes** pick round ticks (1, 2, 2.5, 5 × 10ⁿ). Curves never overshoot the data (monotone interpolation).
- **Bars** are native elements: tappable, read one by one by screen readers, and animated when values change. Other charts read as one element with their values.
- Charts mirror in right-to-left languages, like the layout around them.

Draw your own with `shape()`: coordinates are fractions of the frame, so drawings follow the layout size.

```rust
shape()
    .inset(2., 2.)                                              // points kept free for strokes
    .path(Path::smooth([(0., 1.), (0.5, 0.2), (1., 0.6)]).stroke(theme.accent, 2.).round())
    .path(Path::sector(0.5, 0.5, 0.5, 0.3, 0., 120.).fill(theme.success))   // donut slice, degrees
    .size(px(120.))
    .label("Trend: up")                                         // decorative unless labelled
```

`Path` also has `line`, `arc`, `circle`, `rect`, `move_to`/`line_to`/`curve_to`/`arc_to`/`close`, `.fill`, `.stroke`, `.dashed`.

### Charts in widgets, Live Activities and car screens

The same charts and shapes work in `Widget::render`, `Activity::lock_screen`/`compact`/`expanded` and car screens:

```rust
Widget::new("todos", "Todos").families(&[WidgetFamily::Medium, WidgetFamily::AccessoryCircular])
    .render(move |cx, family| ui::ring(done).label("2/3").size(56.).thickness(6.))
Activity::new("focus").lock_screen(div().flex_row().child(ui::ring(today / 4.).size(48.)).child(timer(ends)))
CarItem::new("Today").drawing(ui::ring(0.6).drawing(cx))       // also ui::sparkline(…).drawing(cx), any Drawing
```

| Surface | iOS / macOS / visionOS | Android |
|---|---|---|
| Widgets (all sizes) | SwiftUI `Path`s at the node's frame (`PrismRenderer.swift`) | a bitmap per drawing in `RemoteViews` (`PrismSurface.java`) |
| Lock-screen widgets (`Accessory*`) | `widgetRenderingMode`: in vibrant and accented modes colored paths are white and `widgetAccentable`, gray ones (grids, tracks) recede | — (no lock-screen widgets) |
| Live Activity, Dynamic Island | SwiftUI paths, every region | ongoing notification: bitmaps |
| Car screens | CarPlay list/grid images (`UIImage` with day and night variants) | Android Auto / Automotive row and grid images (`CarIcon` bitmap, day or night) |

- **Snapshots** carry the drawing unresolved (fractions of the frame), so each renderer draws it at the size it lays out. Numbers are short (`.25`, opaque colors without alpha): a sparkline snapshot is ~0.5 KB, a ring with its label ~0.3 KB, a 7-point smooth line chart with its axes ~1.8 KB.
- **Static variant**: while a surface renders, `cx.in_snapshot()` is true. The bar chart then draws its bars in its shape (nothing is tappable there), and overlays (a ring's label) use negative margins, which snapshots now keep.
- **Light/dark and right to left** follow the host: adaptive colors resolve with the widget's color scheme (Android 12+: day and night bitmaps via `RemoteViews.setIcon(…, night)`), and drawings flip with the layout direction.
- **Limits**, enforced:
  - ActivityKit: 4 KB for a Live Activity's static and dynamic data together. Content over it has its drawings rounded to two decimals, then left out, with a log line (`ActivityContent::payload_size`).
  - WidgetKit timelines read the snapshot from the App Group container, so there is no payload limit there; the extension's memory budget is the constraint, and vector paths cost almost nothing.
  - Android widgets: the bitmaps and icons of one `RemoteViews` may not exceed screen width × height × 4 × 1.5 bytes (`AppWidgetManager.updateAppWidget`, an error from Android 17), shared by every size of the widget.
  - Android notifications: the system warns above 2 000 000 bytes of custom views and strips them above 5 000 000 (`config_notificationWarnRemoteViewSizeBytes`, `…Strip…`). Prism keeps the collapsed and expanded views under 2 MB together.
  - Past a budget, bitmaps are drawn at a lower resolution (and left out below a quarter of it). RemoteViews do not report their size: a drawing's bitmap is made for the size estimated from the host's bounds, fixed sizes and measured text, then stretched to its real frame.
- **Watch complications**: this repository has no watchOS widget extension yet, so there are none; the watch app itself draws charts with SwiftUI.
## Forms and feedback

Native form controls everywhere: dragging, menus, calendars and keyboards behave like the platform's own.

```rust
slider(self.volume).range(0., 100.).step(1.)
    .accessibility_label("Volume")
    .on_change(cx.listener(|this, e, _| this.volume = e.value))
picker(["Small", "Medium", "Large"], self.size).placeholder("Choose…")
    .on_change(cx.listener(|this, e, _| this.size = Some(e.index())))
date_picker(self.due).min(SystemTime::now())                  // .time(), .date_time()
    .on_change(cx.listener(|this, e, _| this.due = e.date()))
text_input().secure(true).return_key(ReturnKey::Go)
    .on_submit(cx.listener(|this, _, cx| this.sign_in(cx)))
text_input().keyboard(Keyboard::Email).capitalize(Capitalize::None).autocorrect(false)
text_area().value(&self.notes).max_h(px(160.))                // grows, then scrolls
spinner().large()

// Themed (label, value, accent), with `.native()` for the bare control
ui::slider("Volume", v).range(0., 100.).step(1.).format(|v| format!("{v:.0} %"))
ui::select("Size", sizes, self.size)
ui::date_field("Due", self.due)
ui::text_field("Password", &self.password).secure().on_submit(…)
ui::text_area("Notes", &self.notes).max_lines(5)
ui::spinner().label("Syncing…")

// Toasts: put `ui::toaster()` once, last in the screen it covers
ui::show_toast(cx, ui::toast("Saved").action("Undo", undo));  // .tone(Variant::Danger), .duration(…)
```

| | Slider | Picker | Date / time | Text area | Spinner |
|---|---|---|---|---|---|
| iOS, visionOS | `UISlider` | `UIButton` + `UIMenu` | `UIDatePicker` (compact) | `UITextView` | `UIActivityIndicatorView` |
| tvOS | focusable track, ◀ ▶ on the remote | `UIButton` + `UIMenu` | focusable field, ◀ ▶ by day / 15 min / hour | `UITextView` | `UIActivityIndicatorView` |
| macOS | `NSSlider` | `NSPopUpButton` | `NSDatePicker` | `NSTextView` | spinning `NSProgressIndicator` |
| Web | `<input type=range>` | `<select>` | `<input type=date / time / datetime-local>` | `<textarea>` | CSS |
| Linux | `GtkScale` | `GtkDropDown` | `GtkCalendar` in a popover | `GtkTextView` | `GtkSpinner` |
| Android | `SeekBar` | `Spinner` | `DatePickerDialog` / `TimePickerDialog` | multi-line `EditText` | indeterminate `ProgressBar` |
| watchOS | SwiftUI `Slider` (Digital Crown) | SwiftUI `Picker` | wheels (`DatePicker`) in a sheet | vertical `TextField` | `ProgressView` |

- **Values**: sliders report `e.value` while dragging (snapped to the step), pickers `e.index()`, date pickers `e.date()` (an instant, shown in the device's time zone).
- **Text inputs**: `secure` (password entry), `keyboard` (email, number, decimal, phone, URL), `return_key` (Done, Go, Next, Search, Send), `capitalize`, `autocorrect`, and `on_submit` for the return key. Each maps to the platform's own setting where it has one (no keyboard types on macOS, Linux or the watch (number has no watch equivalent); no return-key label on macOS or Linux; tvOS has no multi-line editing, so text areas are one line there).
- **Accessibility**: sliders are adjustable (VoiceOver swipe up/down, TalkBack, arrow keys) and read their value (`accessibility_value("40 %")` overrides it); pickers read their label and choice; spinners are busy indicators. Toasts are announced politely (`Role::Status`: ARIA live region, VoiceOver and TalkBack announcements, `gtk_accessible_announce`).
- **Right to left**: sliders fill from the right, menus and calendars follow the language.
- **Tint**: `text_color` tints a control (`ui::*` uses the theme's accent).

- **Platform notes**: tvOS has no `UISlider`, `UIDatePicker` or editable `UITextView` (`API_UNAVAILABLE(tvos)`), hence the focus-driven stand-ins. watchOS sliders always step (a continuous one moves by a tenth of its range), and dates open the system wheels in a sheet. `GtkCalendar` has no min/max, so out-of-range days snap back. Before GTK 4.14, Linux has no `gtk_accessible_announce`: the status text changes but may not be spoken. Android announces through a polite live region (`announceForAccessibility` is deprecated in API 36), and the date dialog's header follows the device language, not the app's.
- **Dev hooks for forms**: macOS `PRISM_TAP=<label>`, `PRISM_TYPE=<text>` (`\n` is Return), `PRISM_INCREMENT=<label>` (VoiceOver increment), `PRISM_A11Y_LOG=1`; Linux `PRISM_CLICK` opens pickers and date pickers, `fill:<placeholder>=<text>`, `fill:@slider=40`, `submit:<placeholder>`, open popovers saved as `out-N.png`; watchOS `PRISM_SCROLL=<pt>`, `PRISM_PRESENT=<label>` (opens a date picker), `PRISM_DEBUG_A11Y=1`. The watch's `ImageRenderer` snapshot can't draw native controls: use `xcrun simctl io <udid> screenshot`.

**Overlays**: `div().absolute().bottom(16.).left(16.).right(16.)` takes an element out of the flex flow and places it at its insets from the parent (`.top()`, `.inset(0.)`…), like CSS `position: absolute` (taffy on native). Later siblings draw on top; `left`/`right` are the leading and trailing edges.

## Accessibility

Accessibility is built in, not opt-in. The core resolves one description per node (role, label, hint, value, checked, disabled, hidden) and each backend maps it to the platform API:

| | Web | iOS | macOS |
|---|---|---|---|
| API | ARIA + semantic HTML (`<main>`, `<h1>`, `<button>`, `role=switch`, `tablist`) | `UIAccessibility` traits, labels, values | `NSAccessibility` roles, labels, press action |
| Keyboard | Tab/Enter/Space on custom buttons, arrow keys in tab bars, visible focus | — | Full Keyboard Access, Space/Return, focus ring |
| Navigation | focus moves to the new screen's title; document title follows | native | window title follows |

Defaults are automatic: a clickable `div` becomes a button named after its text ("Try Prism on the web, Done"), inputs use their placeholder, toggles expose their state. Refine with:

```rust
text("Settings").heading(1)
div().on_click(..).accessibility_label("Open profile").accessibility_hint("Shows your account")
text("›").accessibility_hidden()
```

## Components (`prism::ui`)

A themed, accessible component library: `button`, `switch`, `checkbox`, `text_field`, `card`, `heading`, `badge`, `list_row`, `divider`, form controls and toasts (see [Forms](#forms-and-feedback)), and charts (see [Charts](#charts-and-vector-drawing)).

Three levels of customization:

1. **Theme**: `ui::set_theme(cx, Theme::dark().accent(rgb(0x0A84FF)).radius(0.))`. It also switches the native chrome (bars, native controls) to dark.
2. **Per instance**: `ui::button("Delete").danger().large()`, plus `.style(|s| s.corner_radius = 0.)` for anything else.
3. **Replace a component app-wide**: `Theme::light().override_component(|b: ui::Button, cx| my_button(b, cx))`, or implement `ui::Component` for your own and register it with `prism_ui::component!(MyThing)`.

Custom or native: `ui::button("Save").native()` / `ui::switch(..).native()` (also `ui::slider`, `ui::select`, `ui::date_field`, `ui::text_area`) per instance, or `Theme::prefer_native(true)` everywhere (UIButton/UISwitch, NSButton/NSSwitch, `<button>`/`<input role=switch>`). Raw native primitives remain available: `button()`, `toggle()`, `text_input()`.

## Widgets, Live Activities, Dynamic Island, notifications

System surfaces are described with the **same elements as the UI**, plus `image("sf.symbol")`, `progress(0.4)`, `timer(until)` (counts down by itself, even when the app is not running) and `div().link("/path")` (deep link everywhere):

```rust
// A home-screen widget, re-rendered whenever `store` changes.
cx.register_widget(
    Widget::new("todos", "Todos")
        .families(&[WidgetFamily::Small, WidgetFamily::Medium])
        .link("/")
        .render(move |cx, family| text(format!("{} open", store.read(cx).open()))),
);
cx.reload_widgets_on(&store);

// A Live Activity: lock screen + Dynamic Island (compact, minimal, expanded).
let id = cx.start_activity(
    Activity::new("focus")
        .summary("Focus session, 25 minutes")
        .compact(image("brain.head.profile"), timer(ends))
        .minimal(image("timer"))
        .lock_screen(div().flex_row().child(text("Focus")).child(timer(ends)))
        .link("/focus"),
);
cx.update_activity(id, …);  cx.end_activity(id, None, true);

// A local notification that opens a screen when tapped, with action buttons.
cx.send_notification(
    Notification::new("Done").body("Take a break").link("/focus").after(total)
        .action(NotificationAction::new("again", "5 more minutes").link("/focus"))
        .action(NotificationAction::new("note", "Add a todo…").reply("What's next?")),
);
cx.on_notification_response(|r, app| match r.action.as_deref() { Some("note") => add(app, r.text.clone()), _ => {} });
```

| | iOS | macOS | Web | Android (planned) |
|---|---|---|---|---|
| Widgets | WidgetKit | WidgetKit (`mise run macos-app`) | — | Glance |
| Live Activities | lock screen + **Dynamic Island** | menu bar item with live countdown | in-page live banner (`aria-live`) | Live Updates notification |
| Notifications | UserNotifications | UserNotifications (bundled app) | Notification API / toast | NotificationCompat |

How it works: Prism resolves the elements into a static snapshot (compact JSON, under ActivityKit's 4 KB limit). A **generic SwiftUI renderer** shipped with Prism (`platform/apple/Extension`) draws it in a WidgetKit extension. `scripts/ios-run.sh` generates that extension from the app's registered widgets, then builds and signs it with the App Group. The Rust app talks to WidgetKit/ActivityKit/UserNotifications through a small Swift bridge linked automatically (`crates/prism-apple-build`). Platforms implement `PlatformServices`; apps query `cx.capabilities()` instead of checking the OS.

**Light & dark**: colors can be adaptive, `adaptive(rgb(0xFFFFFF), rgb(0x1C1C22))`, resolved natively everywhere: CSS `light-dark()`, dynamic `UIColor`/`NSColor`, SwiftUI color scheme in widgets and Live Activities. `Theme::system()` makes the whole component library follow the system appearance live.

## Remote push notifications

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
cx.on_activity_push_token(|t, app| { /* t.activity (None: push-to-start), t.token */ });
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
// .urgency(..), .live_activity(&Activity::…content_state_json(), end) / .start_live_activity(kind, ..)
```

| | Service | How | Silent pushes | Verified |
|---|---|---|---|---|
| iOS, iPadOS | APNs | `registerForRemoteNotifications`; app delegate in Rust | `content-available` → `on_background_push` (~30 s) | simulator: real sandbox token, `simctl push` shown in the foreground, Live Activity and push-to-start tokens |
| tvOS, visionOS | APNs | same UIKit delegate | yes | compiled |
| macOS | APNs | `NSApplication` delegate (bridge) | `didReceiveRemoteNotification` | compiled (needs a Mac provisioning profile) |
| watchOS | APNs | `WKApplication` delegate | `didReceiveRemoteNotification:fetchCompletionHandler:` | compiled |
| Android | FCM (opt-in) or UnifiedPush | `FirebaseMessagingService` / UnifiedPush connector (AND_3) | data messages | Firebase bundled without Gradle and initialized on the emulator; UnifiedPush built (its decryption is Linux's, verified) |
| Web | Web Push | service worker `<base>/prism-sw.js`, `pushManager.subscribe` with VAPID | – (browsers always show one) | service worker, session events, click → focus/open |
| Linux | UnifiedPush over D-Bus | `org.unifiedpush.Connector2` on the session bus | yes, while the app runs | end to end with a local distributor |

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
- **Test in the simulator:** `xcrun simctl push <device> dev.prism.counter examples/counter/push/hello.apns` (an `.apns` file is the payload; `cargo run -p prism-push --example push -- apns-payload '{"title":"Hi","link":"/push"}'` writes one). The simulator delivers alerts (foreground banner, `Received`), but rejects pure silent pushes (`UNErrorDomain 1401`, "no user visible content") and turns `content-available` into a background fetch without the payload: test silent pushes on a device.
- **Android, FCM:** Firebase normally needs Gradle. Here the CLI fetches `firebase-messaging` and its dependencies from Google's Maven (Play services client libraries, Kotlin, AndroidX: about 75 artifacts, 2 dex files), merges their manifests (`scripts/android-manifest-merge.py`), and turns `google-services.json` into the resources Firebase reads. Firebase is Apache 2.0; the Play services client libraries are under the Android SDK license, redistributable inside apps (what every Gradle build ships). FCM needs Google Play services on the device, recent enough for the bundled `firebase-messaging` (26.0.2 requires Play services 26.12; older devices and emulator images fall back to UnifiedPush); auto-init is off, so a token is requested only by `register_for_push`. Prism sends **data messages**, which reach the app in the foreground and the background; Rust shows visible ones.
- **Android, UnifiedPush:** an open standard with no library: the app broadcasts `REGISTER` to a distributor (ntfy, NextPush, Sunup…), receives `NEW_ENDPOINT`/`MESSAGE` broadcasts, and decrypts the Web Push message in Rust (RFC 8291). The subscription carries the device's keys, so the server sends Web Push to the endpoint (`Service::UnifiedPush`). No distributor installed and no FCM: `capabilities().push` is `None`.
- **Web:** the page registers the service worker on `register_for_push` (it needs a VAPID key; generate one with `cargo run -p prism-push --example push -- vapid`). Pushes reach open pages as `Received`; with no visible page the worker shows a notification, and a click focuses the app (or opens it on the push's `link`) and reports `Opened`. Browsers require every push to show something: `silent` only skips the sound.
- **Linux:** there is no desktop push service. Prism speaks UnifiedPush's D-Bus protocol (`org.unifiedpush.Distributor2`), so a distributor such as KUnifiedPush delivers Web Push messages while the app runs (Prism apps are not D-Bus-activatable services). `crates/prism-linux/examples/fake_distributor.rs` is a local distributor for tests.
- **Pushes shown by Prism** (UnifiedPush, FCM data, Linux) become local notifications when the app is not in the foreground; tapping one is an `Opened` push.

`prism-push` is plain Rust on RustCrypto (`p256`, AES-GCM, HKDF) and ring (RS256 for Google's OAuth), with an HTTP/2 `reqwest` client (rustls). It is tested against RFC 8291's example (intermediate values, encryption and decryption), RFC 8292's VAPID token, and RFC 7515's ES256 and RS256 signatures. Every service also builds plain `HttpRequest`s for servers with another HTTP client. Without its `send` feature it is pure Rust: the Android and Linux receivers use it to decrypt.

## In the car (CarPlay, Android Auto, Android Automotive)

Cars only show the platform's templates, so a car screen is described with them: lists in sections, grids, panes (rows and up to two actions) and tabs. They are rebuilt whenever the app changes, like widgets; the car only receives what differs.

```rust
cx.register_car(move |cx| {
    CarTemplate::list("Todos").section(CarSection::new().items(
        store.read(cx).todos.iter().map(|t| {
            let id = t.id;
            CarItem::new(&t.title).icon("circle").browsable()
                .on_select(move |cx| cx.car_push(move |cx| detail(cx, id)))
        }),
    ))
});
// In a handler: cx.car_push(…), cx.car_pop(), cx.car_pop_to_root(); cx.car_connected().
```

- **CarPlay:**
  - a template scene (`CPListTemplate`, `CPGridTemplate`, `CPInformationTemplate`, `CPTabBarTemplate`), updated in place;
  - the car can start the app without its phone window;
  - CarPlay only shows apps whose provisioning profile carries an entitlement Apple grants on request (`com.apple.developer.carplay-driving-task`), even in the simulator. Builds declare the scene; the entitlement comes from your profile.
- **Android Auto and Android Automotive:**
  - Google's Car App Library (IoT category), fetched from Maven by `scripts/android-deps.py` (no Gradle);
  - the phone APK serves Android Auto. A build for a car running Android (`PRISM_ANDROID_AUTOMOTIVE=1`, automatic when the device is one) opens the car screens from the car's launcher;
  - tabs show their first tab (`TabTemplate` needs Car API level 6);
  - test on the Automotive emulator (`system-images;android-35-ext15;android-automotive;arm64-v8a`, device `automotive_1408p_landscape_with_google_apis`), or with Android Auto's Desktop Head Unit and a phone signed in to Play.

## Navigation

A URL-style router, rendered with each platform's native navigation:

```rust
Application::new().title("Prism").run(|cx| {
    let store = cx.new_entity(|_| Store::new());
    Router::new()
        .route("/", move |cx, _| cx.new_entity(|cx| Home::new(store, cx)))
        .route("/todos/:id", move |cx, p| {
            let id = p.parse("id").unwrap_or(0);
            cx.new_entity(|cx| TodoDetail::new(store, id, cx))
        })
        .build(cx)
});

// A route's view returns a screen; its title goes in the navigation bar.
screen("Details").child(...)

// Anywhere:
cx.navigate("/todos/2");   cx.back();   cx.replace("/login");   cx.reset("/");
div().on_click(navigate_to("/todos/2"))   // handler helpers: navigate_to, go_back
```

| | Stack | Back | Deep links |
|---|---|---|---|
| **iOS** | `UINavigationController` (large titles, push animation) | system button + edge swipe | `PRISM_PATH=/todos/1` |
| **macOS** | header bar (title, Back button) | Back button, Go ▸ Back ⌘[ | `PRISM_PATH=/todos/1` |
| **Web** | header bar | header button + browser Back/Forward (History API) | any URL, server-rendered |

**Tabs**: `.tab("Todos", "checklist", "/")` per tab (SF Symbol name; a glyph on the web). Each tab keeps its own stack. `navigate()` to a tab's root path switches tab. iOS: `UITabBarController`. macOS: segmented control. Web: ARIA tablist, URL follows the tab.

**Scrolling**: `scroll()` (`UIScrollView` / `NSScrollView` / `overflow:auto`). A screen starting with a scroll view lets it run under the bars, so iOS large titles collapse natively.

Patterns: `/`, `/todos`, `/todos/:id`, `/files/*rest`. Unknown paths show a "Not found" screen (or `.not_found(...)`). Opening a deep link puts `/` underneath so Back works.

## Web: inside your server, behind a CDN

Prism's web backend is a library. Mount it in Axum next to your API (`examples/axum-server`):

```rust
let prism = Application::new().title("Shop").web_app(|cx| routes(cx)).base_path("/app");
let app = axum::Router::new()
    .route("/api/orders", get(orders))
    .nest("/app", prism.router());          // or .merge(prism.router()) at the root
axum::serve(listener, app).await?;
```

Other servers (actix, hyper, …): `prism-web` without default features has no server dependency. `WebApp::handle(&http::Request)` answers pages and static files, and `WebApp::connect(uri, headers, send)` runs a live session over any WebSocket (`session.receive(text)` for incoming messages). Each session is a thread that sleeps until a message, a timer or async work wakes it.

| Path | Cache-Control | |
|---|---|---|
| pages | `private, no-store` | rendered per browser, sets the session cookie |
| `/_prism/client.<hash>.js`, `/_prism/prism.<hash>.css` | `public, max-age=31536000, immutable` | ETag, `304` |
| `/assets/logo.<hash>.png` (`picture("logo.png")`) | `public, max-age=31536000, immutable` | unhashed `/assets/logo.png`: revalidated |
| `/files/*` (uploads) | `private` | per browser |
| `/ws` | – | proxy as a WebSocket |

`prism build web` also writes `public/` (hashed files plus `manifest.json`). Upload it to a CDN and set `PRISM_ASSET_URL=https://cdn.example.com`, or let the CDN cache `/_prism/*` and `/assets/*` from the server. Settings: `PRISM_HOST`, `PRISM_PORT`, `PRISM_BASE_PATH`, `PRISM_ASSET_URL`, `PRISM_DATA_DIR` (or `WebApp::base_path` / `asset_url` / `data_dir`).

### Docker (Linux)

```sh
docker build -f docker/web.Dockerfile --build-arg PACKAGE=my-app -t my-app .
docker run -p 3000:3000 -v my-app-data:/data my-app
```

`prism build web --docker` runs the same build from any project in the workspace.

This is a multi-stage build on Rust 1.99 / Debian bookworm. It produces the server binary, `public/` (the hashed files for a CDN) and the app's assets, in about 160 MB with the counter example. The image is tested with the same jsdom suites as the macOS build.

## Shipping to stores

`prism build <target>` makes store packages, and `prism publish <target>` uploads them after checking them locally. `--dry-run` builds and checks everything and prints the requests or commands it would send, without using the network. CI runs every dry run.

| Target | Package | Command | Sent to |
|---|---|---|---|
| Android | `.aab` (App Bundle) | `prism build android --aab` | Google Play (`prism publish android`) |
| iOS, iPadOS, tvOS, visionOS, watchOS | `.ipa` | `prism build ios --store` (`tvos`, `visionos`, `watchos`) | App Store Connect (`prism publish ios`…) |
| macOS (Mac App Store) | `.pkg` | `prism build macos --store` | App Store Connect (`prism publish macos`) |
| macOS (direct download) | notarized `.dmg` | `prism build macos --direct` | Apple's notary service (`prism publish macos --direct`) |
| Linux | `.deb` | `prism build linux` | your repository or download page |
| Web | Docker image | `prism build web --docker` | your registry or server |

Versions come from `Prism.toml`. `[app] version` is the version people see. `[app] build` is the build number: `CFBundleVersion` on Apple platforms, `versionCode` on Android (or `[android] version_code`), and the Debian revision on Linux. Each upload needs a higher build number.

```toml
[app]
version = "1.2.0"
build = 7
category = "public.app-category.productivity"   # LSApplicationCategoryType (the Mac App Store needs one)
uses_encryption = false                          # answers App Store export compliance
description = "Counts things"                    # Linux packages

[android]
track = "internal"                               # internal, alpha, beta, production

[linux]
maintainer = "Jane Doe <jane@example.com>"
```

### Google Play

- **The bundle:** `aapt2 link --proto-format` writes the manifest and resources as protobuf. They go into the base module zip (`manifest/`, `dex/`, `res/`, `resources.pb`, `assets/`, `lib/`, `root/`), and `bundletool build-bundle` assembles it. bundletool 1.18.3 is downloaded once into `~/.cache/prism` and checked against a pinned SHA-256 (`PRISM_BUNDLETOOL` uses another jar). `BundleConfig.json` keeps native libraries uncompressed and 16 KB aligned (`PAGE_ALIGNMENT_16K`), as Android 15+ devices with 16 KB pages need.
- **Signing:** jarsigner signs the bundle with your upload key (`PRISM_ANDROID_KEYSTORE`, `PRISM_ANDROID_KEYSTORE_PASS`, `PRISM_ANDROID_KEY_ALIAS`, and `PRISM_ANDROID_KEY_PASS` if the key's password differs). Play rejects the debug key, so `prism publish` refuses it. Create an upload key once: `keytool -genkeypair -keystore upload.jks -alias upload -keyalg RSA -keysize 2048 -validity 10000`. Enroll in Play App Signing, then keep the keystore out of the repository.
- **Testing like Play does:** `prism run android --aab` builds the bundle, then `bundletool build-apks --local-testing --connected-device` and `install-apks` install the split APKs (base, ABI, language, density) that Play would serve to that device.
- **Upload:** `prism publish android [--track beta] [--rollout 0.1 | --draft] [--notes "…"]` uses the Google Play Developer API. It creates an edit (`edits.insert`), uploads the bundle (`edits.bundles.upload`), sets the track's release (`edits.tracks.update`) and commits the edit (`edits.commit`). If any step fails, it deletes the edit. Credentials come from a service account JSON key: `PRISM_PLAY_SERVICE_ACCOUNT` (a path or the JSON itself) or `--service-account`. Invite the service account in the Play Console (Users and permissions) with release rights.
- **Checks before the upload:** package name, `versionCode`, `targetSdkVersion` ≥ 36 (Play's rule since August 31, 2026), not debuggable, signed and not with the debug key, release status and rollout, and the service account key (it signs its token request).
- **First release:** the app must already exist in the Play Console. Until it has been published once, Play only accepts draft releases (`--draft`).

### App Store Connect (iOS, iPadOS, tvOS, visionOS, watchOS, macOS)

- **Store builds** (`--store`) use the device SDK. They are signed with an "Apple Distribution" identity (or the older "iPhone Distribution" or "3rd Party Mac Developer Application"), with App Store provisioning profiles: one for the app and one for its widget extension (`<bundle id>.widgets`). The profiles are found in `~/Library/MobileDevice/Provisioning Profiles` and Xcode's folder, or set them with `PRISM_PROFILE` / `PRISM_WIDGETS_PROFILE`, and on macOS `PRISM_MAC_PROFILE` / `PRISM_MAC_WIDGETS_PROFILE`. `PRISM_IDENTITY` picks the identity, `PRISM_TEAM` the team, and `PRISM_KEYCHAIN` the keychain that holds them (CI). The build records the SDK and Xcode in Info.plist (`DT*` keys), as App Store Connect expects, and adds every iPad orientation (needed for multitasking).
- **Icons:** one 1024 px PNG (`[app] icon`) becomes each platform's icon. iOS and watchOS get a single size, macOS every size, visionOS a two-layer solid image stack. tvOS gets brand assets: layered icons at 400×240 and 1280×768 and the top shelf images, with the icon filling each.
- **iOS family:** the `.ipa` is `Payload/<App>.app` (with `PlugIns/PrismWidgets.appex`), zipped. Untested: Xcode wraps watch-only apps in an iOS stub app, which Prism does not make, so App Store Connect may refuse a watchOS `.ipa` (the dry run warns).
- **Mac App Store:** the app is sandboxed (`com.apple.security.app-sandbox`, network client, user-selected files). `[permissions]` add the matching entitlements: camera, microphone, location, photos. The profile is embedded, and productbuild makes a `.pkg` installing into `/Applications`, signed with `PRISM_INSTALLER_IDENTITY` ("3rd Party Mac Developer Installer"). The app group becomes `<TEAM>.<bundle id>`, the form macOS accepts.
- **Upload:** `prism publish ios` (or `tvos`, `visionos`, `watchos`, `macos`) runs `xcrun altool --upload-package <ipa|pkg> --api-key … --api-issuer … --p8-file-path …`. It uses an App Store Connect API key (Users and Access › Integrations): `PRISM_ASC_KEY_ID`, `PRISM_ASC_ISSUER_ID`, and `PRISM_ASC_KEY` (the `AuthKey_<id>.p8`). Before uploading, it checks the bundle id, version and build, the `DT*` keys, the icon, the macOS category, the signature (`codesign --verify --deep --strict`) and identity, the profiles (App Store type, matching bundle ids), the sandbox (macOS) and the key, by signing an App Store Connect token with it.

### macOS outside the store (Developer ID)

`prism build macos --direct` signs the app and its widget extension with "Developer ID Application" under the hardened runtime (`--options runtime`) with a secure timestamp, puts them in a `.dmg` (with an Applications shortcut), and signs the disk image. `prism publish macos --direct` checks all of that, then runs `xcrun notarytool submit <dmg> --wait`, `xcrun stapler staple` and `stapler validate`. It authenticates with the same API key or a `notarytool store-credentials` profile (`PRISM_NOTARY_PROFILE`). Without a provisioning profile, iCloud stays off (`PRISM_MAC_PROFILE` embeds one).

### Linux

`prism build linux` makes a `.deb`: the release binary and its assets in `/usr/lib/<name>`, a launcher in `/usr/bin`, a desktop entry and icon named after the bundle id (the window uses it as its GTK application id, so desktops match it to its launcher), and AppStream metadata. Dependencies come from `dpkg-shlibdeps`. On Linux it builds directly. Elsewhere it builds inside the `docker/linux-dev.Dockerfile` image, which it builds if needed, with its own target dir under `target/linux`. Install the package with `sudo apt install ./<name>_<version>-<build>_<arch>.deb`.

### Web

`prism build web --docker` builds `docker/web.Dockerfile` (see [Docker](#docker-linux)) into an image tagged `<name>:<version>`. The build context is the Cargo workspace, without `target/` and local data, so path dependencies must be inside the workspace. Push the image to your registry yourself.

### Trying it without accounts

`scripts/test-signing.sh <dir> <bundle id>` creates a throwaway keychain with a self-signed identity, plus provisioning profiles signed by it. It prints the `PRISM_*` variables to export. Apple refuses all of this, so it is only for exercising the builds and the dry runs. With them, `prism publish ios --dry-run` and the others build real packages, check them, and show the upload commands. Identity problems appear as warnings in a dry run and as errors for a real upload. productbuild only signs packages with an identity the system trusts, so the test `.pkg` is unsigned (`PRISM_INSTALLER_IDENTITY=-`).

### Secrets for CI

| Secret | For |
|---|---|
| `ANDROID_UPLOAD_KEYSTORE` (base64), `ANDROID_KEYSTORE_PASS`, `ANDROID_KEY_ALIAS` | `PRISM_ANDROID_KEYSTORE` (decoded to a file), `PRISM_ANDROID_KEYSTORE_PASS`, `PRISM_ANDROID_KEY_ALIAS` |
| `PLAY_SERVICE_ACCOUNT_JSON` | `PRISM_PLAY_SERVICE_ACCOUNT` |
| `APPLE_CERTIFICATES_P12` (base64), `APPLE_CERTIFICATES_PASSWORD` | Apple Distribution, Mac Installer Distribution and Developer ID Application identities, imported into a temporary keychain (`PRISM_KEYCHAIN`) |
| `APPLE_PROFILES` (base64 tarball) | App Store profiles for the app and `.widgets`, per platform (`PRISM_PROFILE`, `PRISM_WIDGETS_PROFILE`, `PRISM_MAC_PROFILE`, `PRISM_MAC_WIDGETS_PROFILE`) |
| `ASC_KEY_ID`, `ASC_ISSUER_ID`, `ASC_KEY_P8` | `PRISM_ASC_KEY_ID`, `PRISM_ASC_ISSUER_ID`, `PRISM_ASC_KEY` (uploads and notarization) |
| `APPLE_TEAM_ID` | `PRISM_TEAM` |

The `release` jobs in `.github/workflows/ci.yml` run every dry run with throwaway keys. To publish for real, add these secrets and drop `--dry-run`, ideally in a separate workflow that runs on tags.

## Performance

Measured on the example app (release build, Apple silicon):

| | Idle CPU | Memory |
|---|---|---|
| macOS, 460×720 / 1200×800 window | ~0 % | 42 MB / 49 MB |
| iOS (simulator) | ~0.2 % | 30–35 MB |
| Web server, per open session | ~0 % (sleeps until a message or timer) | ~0.3 MB |

- **Updates are local**: changing one row of a 5,000-row list re-renders that row only (1 patch, ~20 µs) and touches no other native view (`cargo test -p prism-native --release -- --ignored --nocapture bench`).
- **GPU-composited views**: backgrounds, borders, corners and pictures are layer properties, not per-view bitmaps (macOS graphics memory 28 MB → 0.4 MB in a large window).
- **Web**: one database pool per file for the whole server (not per visitor); sessions sleep until a message, a timer or finished background work.
- **Release profile**: thin LTO and stripped symbols (binaries −24 %).

## Architecture

```
            your app (Render entities)
                      │ render()
                      ▼
 prism-core   Element tree ──► Reconciler ──► Vec<Patch> ◄── UiEvent (click, input)
                                                 │
        ┌────────────────────────┬───────────────┴──────────────┐
        ▼                        ▼                              ▼
   prism-web                prism-native (taffy flexbox, Widgets trait)
   SSR HTML + JSON              ├── prism-macos  (AppKit)
   patches over WS              └── prism-ios    (UIKit)
```

- **`prism-core`**: styles, element builders, entities/`Context`, reconciler, `Backend` trait, `Runtime`. No platform code.
- **`prism-native`**: turns patches into calls on a 10-method `Widgets` trait and runs flexbox layout with taffy. Shared by every native backend.
- **`prism-web`**: std-only HTTP server + tungstenite. One `Runtime` per browser session (LiveView model).
- **`prism-macos`** / **`prism-ios`**: `Widgets` implementations over objc2.
- **`prism-ui`**: component library on top of the core (themes, overrides, native escape hatch).
- **`prism`**: facade; picks the backend from features (`web`, `native`) and the target OS.
- **`prism-push`**: for your server: sends pushes to APNs, FCM and Web Push / UnifiedPush (see [Remote push](#remote-push-notifications)).

Layout is a flexbox subset (column by default, like React Native) and maps 1:1 to CSS on the web and to taffy on native, so all targets lay out the same way.

### Adding a platform (Windows…)

1. New crate `prism-<os>` implementing `prism_native::Widgets` (create/style/measure/place native views).
2. An entry point that builds a `Runtime`, feeds `runtime.render()` / `runtime.dispatch(event)` into a `NativeRenderer`, and calls `layout()` on resize.
3. One `cfg` line in `crates/prism/src/lib.rs`.

## Development

```sh
mise install        # Rust 1.99 + macOS/iOS targets
mise run test       # unit tests
mise run check      # type-check host, web and iOS-simulator builds
mise run web        # the example, restarting on save (also: macos, ios, android, macos-app, device)
```

- `prism dev` keeps the current screen across restarts (`PRISM_DEV_STATE`); the web client reconnects by itself.
- `PRISM_PORT=8080`, `PRISM_SIM="iPhone 17"` (simulators are slimmed with `simslim` when installed), `PRISM_LOCALE=ar`, `PRISM_TEXT_SCALE=1.5`.
- macOS dev hooks: `PRISM_CLICK="+1|Dark mode" PRISM_SCROLL=600 PRISM_DELAY=2 PRISM_SNAPSHOT=out.png cargo run -p counter` presses controls by title or accessibility label, scrolls, waits, writes a PNG of the window, and quits.
- CI (`.github/workflows/ci.yml`): format, tests, clippy for macOS / iOS simulator / iOS device / web, then release bundles and a web smoke test. The `release-*` jobs build the store packages and run every `prism publish --dry-run` with throwaway keys. The Linux job builds and installs the `.deb`.

## tvOS

`prism run tvos` (simulator) / `prism build tvos`: the UIKit backend, adapted to the Siri Remote.

- Clickable views take the focus (raised with a shadow) and react to the select press; buttons use the primary action.
- Toggles are check-mark buttons (`UISwitch` does not exist on tvOS).
- The whole UI is zoomed by the system's body text size ÷ 17, so phone-sized styles read from a sofa. The app sees the viewport in Prism points (1920 ÷ zoom), so breakpoints keep working. `PRISM_ZOOM` overrides it.
- Not on tvOS (the SDK marks them unavailable): notifications with text, widgets, Live Activities, motion, photo/camera pickers, haptics, clipboard, share sheet, pull to refresh. Location gives one position at a time. `cx.capabilities()` reports all of this, and `.only(Platform::TV)` / `Platform::Tvos` adapt the UI.
- Dev hooks on iOS and tvOS: `PRISM_CLICK="+1|Save"` presses by title or label, `PRISM_SNAPSHOT=/abs/out.png` saves the window and quits.

## watchOS

`prism run watchos` (simulator) / `prism build watchos`: a watch-only app (`WKApplication`, `WKWatchOnly`).

watchOS has no UIKit views (`UIView` is `API_UNAVAILABLE(watchos)` in the SDK), so the app runs in Rust and a SwiftUI host draws it (`platform/apple/Watch`).

- Layout is prism-native's flexbox, as on iOS and macOS. Swift measures text (UIKit string drawing) and places each node at its frame, so layouts match the other platforms.
- Navigation stacks are `NavigationStack`, tabs are vertical pages (Digital Crown), toggles are SwiftUI toggles.
- Styles are zoomed like tvOS (body text size ÷ 17); `cx.device_info()` reports the watch (model and screen from Xcode's profiles).
- Device APIs: location (Core Location), motion (Core Motion), the photo picker (PhotosUI's `PhotosPicker`), haptics (`WKInterfaceDevice.play`), sharing (`ShareLink`), links and the microphone permission.
- Also on the watch: notifications with actions, alerts, and context menus (press and hold opens a dialog, since SwiftUI's `contextMenu` is deprecated on watchOS). It also handles swipes, scroll events, pull to refresh, the scene lifecycle, Dynamic Type and iCloud key-value storage.
- Background tasks use WatchKit's `scheduleBackgroundRefresh`, because `BGTaskScheduler` is `API_UNAVAILABLE(watchos)`.
- What the watch lacks (SDK): camera, clipboard. `capabilities` says so.
- Devices and store builds use Rust's `aarch64-apple-watchos` (arm64 watches, watchOS 26+), so their minimum version is 26.0. Older `arm64_32` watches would need Rust's tier-3 `arm64_32-apple-watchos` target, which Prism does not build.
- Dev hooks: `PRISM_CLICK`, `PRISM_SNAPSHOT` (SwiftUI `ImageRenderer`: text fields render as a placeholder), `PRISM_DEBUG_LAYOUT=1` (logs text frames).

## visionOS

`prism run visionos` (simulator) / `prism build visionos`: the UIKit backend in a window, like an iPad app. `cx.platform()` is `Platform::Visionos` and `cx.device_info().kind` is `Headset`; `Platform::SPATIAL` selects it in `.only()`.

- Eyes and pinches arrive as touches, so controls, gestures and menus work as on iOS.
- Widgets (WidgetKit, visionOS 26+): the minimum version is 26 (`XROS_DEPLOYMENT_TARGET`).
- Not on visionOS (unavailable in the SDK): `UIScreen` (so the window gives the size and scale), haptics, the camera for apps, Live Activities (no ActivityKit), lock-screen widget families, and "always" location.
- The simulator runtime comes from Xcode › Settings › Components, or `xcodebuild -downloadPlatform visionOS`.

## Linux (GTK 4)

`cargo run` on Linux opens a native GTK 4 window: `GtkLabel`, `GtkButton`, `GtkEntry`, `GtkSwitch`, `GtkProgressBar`, `GtkPicture` and `GtkScrolledWindow`. Navigation stacks get a header with a Back button, tabs a tab bar, and modals their own window.

- Layout is prism-native's flexbox: a small GTK layout manager puts each widget at its frame, so screens look like on macOS. Styles become generated CSS.
- Wrapping labels report their longest word as minimum width.
- Storage goes to `$XDG_DATA_HOME/<title>`; the language comes from `LANG`. `cx.device_info()` reports the distribution's `VERSION_ID` and the monitor (size in mm, refresh rate).
- Develop and test in Docker: `docker build -f docker/linux-dev.Dockerfile -t prism-linux-dev docker`, then run the app under `xvfb-run` with `PRISM_SNAPSHOT`/`PRISM_CLICK` (see CI).
- Accessibility labels use GTK's C API (`gtk_accessible_update_property_value`, available in every GTK 4); gtk-rs only binds it from 4.10.
- Location uses GeoClue2 over the system bus, asynchronously; `request_permission` starts a client, so the desktop's agent asks the user. Also available: the file chooser for photos, the clipboard, opening links with the default app.
- Notifications use `GNotification`, with buttons; the dev image's D-Bus monitor shows them. Alerts are modal windows. Context menus open on right-click or press and hold (`GtkPopoverMenu`). `set_app_menu` becomes the menu bar, with shortcuts.
- The tray uses StatusNotifierItem through `ksni`, checked against a test watcher. Also supported: swipes (touchpad), scroll events, refresh with F5 / Ctrl+R, lifecycle (focus, minimized) and background tasks while the app runs. Text size follows GNOME's scaling factor; reduced motion follows "animations off".
- Secrets use the Secret Service (D-Bus), fingerprints fprintd, sign-in the default browser with a loopback redirect (see [Security](#security-secrets-biometrics-sign-in)).
- No desktop standard exists for motion, camera, haptics or a share sheet, so `capabilities` reports them as unavailable.

## Android

`prism run android` / `prism dev android` / `prism build android`: an APK built with the SDK's own tools (`javac`, `d8`, `aapt2`, `zipalign`, `apksigner`), without Gradle, then installed with `adb` on the running emulator or a USB phone (`--device <serial>`; `prism devices` lists them).

The app runs in Rust, and a small Java host (`platform/android`) creates real Android views and places them at the frames Rust computes.

- **Layout:** prism-native's flexbox in dp; the zoom is the screen density, and Java measures text. Navigation stacks get a header with Back (also the system back gesture), tabs a bottom bar, and modals a full-screen dialog.
- **Look:** light/dark follows the system or `cx.set_appearance` (the header, bars and system bars included). Right-to-left languages mirror the layout, the header and the tab bar.
- **Setup (mise):** Java 21, the Android SDK (`ANDROID_HOME`), `cargo-ndk`, the `aarch64-linux-android` target. Install the SDK packages and create an emulator:
  ```sh
  sdkmanager "platform-tools" "emulator" "platforms;android-36" "build-tools;37.0.0" "ndk;30.0.16248370" "system-images;android-36;google_apis;arm64-v8a"
  avdmanager create avd -n Pixel_9 -k "system-images;android-36;google_apis;arm64-v8a" -d pixel_9
  emulator -avd Pixel_9 &
  ```
- **The app crate:** its code lives in its library, and `main.rs` calls it. Android loads a shared library, through `prism::android_main!(run)` in `lib.rs`.
- **Device APIs:**
  - location (`LocationManager`), motion (rotation vector, gyroscope);
  - photos (system photo picker), camera (`ACTION_IMAGE_CAPTURE`);
  - haptics (`VibrationEffect`), clipboard;
  - share sheet (files through a small content provider), links;
  - secrets (Keystore), `BiometricPrompt`, sign-in in a Custom Tab, `cloud_storage` backed up (see [Security](#security-secrets-biometrics-sign-in)).
  Permissions come from `Prism.toml`'s `[permissions]`. Deep links and sign-in redirects arrive through `PrismLinkActivity`, which hands them to the running app.
- **Notifications:** actions and text replies; the first one asks for the permission (Android 13+), as on iOS. Alerts are `AlertDialog`s. Context menus (`PopupMenu`) open on press and hold. Also supported: swipes, scroll events, deep links (`prism-<package>:/path`) and the lifecycle.
- **Background tasks:** JobScheduler, even with the app closed; the library then loads without the Activity and runs the task without a screen.
- **Live Activities:** an ongoing notification drawing the lock-screen content, with timers counting natively (`Chronometer`).
- **Widgets:** home-screen widgets (`AppWidgetProvider`), one per `register_widget`, with small/medium layouts chosen by size (Android 12+). They are redrawn from the last snapshot while the app is closed, charts as bitmaps (see [Charts in widgets](#charts-in-widgets-live-activities-and-car-screens)). Both surfaces render `SNode` snapshots with `RemoteViews` (`PrismSurface.java`).
- **Release:** `PRISM_RELEASE=1` or `prism build android`. Sign with `PRISM_ANDROID_KEYSTORE`, `PRISM_ANDROID_KEYSTORE_PASS` and `PRISM_ANDROID_KEY_ALIAS`; the default is the SDK's debug key. Libraries are aligned for 16 KB pages. `prism build android --aab` makes an App Bundle for Google Play, and `prism publish android` uploads it (see [Shipping to stores](#shipping-to-stores)).
- **Wear OS and Android TV:** the same APK runs there; `cx.device_info().kind` says `Watch` or `Tv`.
  - *Watches:* dark, compact header, icon-only tabs, margins inside a round screen. Back is the swipe to the right. The app is standalone (no phone needed).
  - *TVs:* the remote moves the focus; the focused view rises with a ring. Select clicks, and holding it opens the context menu. Android TV's safe area applies, and the app gets a launcher banner made from the icon.
  - *Emulators:* `system-images;android-34;android-wear;arm64-v8a` (device `wearos_large_round`) and `system-images;android-34;android-tv;arm64-v8a` (device `tv_1080p`).
- **Not on Android:** tray and menu bar (desktop surfaces).

## Status

Primitives: `div`, `text`, `button`, `text_input` (secure, keyboards, submit), `text_area`, `toggle`, `slider`, `picker`, `date_picker`, `spinner`, `scroll`, `picture`, `shape`, `VirtualList`, `screen`, `Router` (stacks, tabs, sheets). Component library `prism::ui`. Accessibility on all backends. Light/dark appearance.
Widgets, Live Activities (Dynamic Island), car screens (CarPlay, Android Auto, Automotive), notifications with actions, remote push (APNs, FCM, Web Push, UnifiedPush) with a server-side sender, background tasks, device APIs (location, motion, photos, camera, haptics, clipboard, share), secure storage, biometrics, Sign in with Apple, OAuth/OIDC sign-in, alerts, context menus, tray and app menus, charts (also in widgets), async tasks, HTTP, storage, i18n/RTL, Dynamic Type, animations, `prism` CLI.
Platforms: web, macOS, iOS/iPadOS, tvOS, watchOS, visionOS, Linux (GTK 4), Android (phones, tablets, Wear OS, Android TV). Not yet: Windows (the `Widgets` trait and `PlatformServices` are the extension points).
