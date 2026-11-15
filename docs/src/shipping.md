# Shipping

`prism build <target>` makes release artifacts (optimized, thin LTO, stripped
symbols).

| Target | `prism build` gives | Notes |
|---|---|---|
| `web` | the server binary and `public/` (hashed static files) | run it anywhere; see below |
| `macos` | a `.app` with widgets and icon | signed ad hoc by default (`PRISM_IDENTITY` for your identity) |
| `ios`, `tvos`, `watchos`, `visionos` | an `.app` | real devices: `--device`, your team and a provisioning profile |
| `android` | a signed APK, aligned for 16 KB pages | release key: `PRISM_ANDROID_KEYSTORE`, `PRISM_ANDROID_KEYSTORE_PASS`, `PRISM_ANDROID_KEY_ALIAS` (default: the SDK's debug key) |

## Web servers

The web app is a server: run the binary behind any reverse proxy, or
[mount it in your own Axum server](platforms/web.md). Settings come from
`PRISM_HOST`, `PRISM_PORT`, `PRISM_BASE_PATH`, `PRISM_ASSET_URL` and
`PRISM_DATA_DIR` (or `WebApp::base_path` / `asset_url` / `data_dir`).

- **CDN:** upload `public/` and set `PRISM_ASSET_URL=https://cdn.example.com`,
  or let the CDN cache `/_prism/*` and `/assets/*` from the server (they are
  `immutable`). Pages are `private, no-store`; proxy `/ws` as a WebSocket.
- **Docker:** `docker/web.Dockerfile` builds a small Debian image with the
  server, `public/` and the app's assets:

  ```sh
  docker build -f docker/web.Dockerfile --build-arg PACKAGE=my-app -t my-app .
  docker run -p 3000:3000 -v my-app-data:/data my-app
  ```
- **Data:** `prism::db` and `cx.storage()` live on the server
  (`PRISM_DATA_DIR`): keep that directory on a volume.

## App stores

- **Apple:** set `[app] bundle_id`, `version` and `icon`, and `[ios] team` in
  `Prism.toml`. The bundle declares languages, permissions (usage texts),
  background tasks, App Groups for widgets and iCloud when enabled. CarPlay
  needs an entitlement Apple grants on request.
- **Android:** the APK is built with the SDK's tools (no Gradle) and signed
  with your keystore; Wear OS and Android TV run the same APK.

## Releasing Prism itself

The crates are published to crates.io in dependency order; see
[Releasing](releasing.md).
