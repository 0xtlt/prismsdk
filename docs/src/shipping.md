# Shipping

`prism build <target>` makes release artifacts (optimized, thin LTO, stripped
symbols).

| Target | `prism build` gives | Notes |
|---|---|---|
| `web` | the server binary and `public/` (hashed static files) | run it anywhere; see below |
| `macos` | a `.app` with widgets and icon | signed ad hoc by default (`PRISM_IDENTITY` for your identity) |
| `ios`, `tvos`, `watchos`, `visionos` | an `.app` | real devices: `--device`, your team and a provisioning profile |
| `android` | a signed APK, aligned for 16 KB pages | release key: `PRISM_ANDROID_KEYSTORE`, `PRISM_ANDROID_KEYSTORE_PASS`, `PRISM_ANDROID_KEY_ALIAS` (default: the SDK's debug key) |
| `linux` | a `.deb` | see [Linux](#linux) |
| `windows` | a `.zip` of the app's folder, or an `.msix` (`--msix`) | signed with `PRISM_WINDOWS_CERTIFICATE` (a `.pfx`) and `PRISM_WINDOWS_CERTIFICATE_PASS`; see [Windows](#windows) |

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

## Shipping to stores

`prism build <target>` makes store packages, and `prism publish <target>` uploads them after checking them locally. `--dry-run` builds and checks everything and prints the requests or commands it would send, without using the network. CI runs every dry run.

| Target | Package | Command | Sent to |
|---|---|---|---|
| Android | `.aab` (App Bundle) | `prism build android --aab` | Google Play (`prism publish android`) |
| iOS, iPadOS, tvOS, visionOS, watchOS | `.ipa` | `prism build ios --store` (`tvos`, `visionos`, `watchos`) | App Store Connect (`prism publish ios`…) |
| macOS (Mac App Store) | `.pkg` | `prism build macos --store` | App Store Connect (`prism publish macos`) |
| macOS (direct download) | notarized `.dmg` | `prism build macos --direct` | Apple's notary service (`prism publish macos --direct`) |
| Linux | `.deb` | `prism build linux` | your repository or download page |
| Windows | `.zip` (the app's folder) or `.msix` | `prism build windows` (`--msix`) | your download page (no Store submission yet) |
| Web | Docker image | `prism build web --docker` | your registry or server |

Versions come from `Prism.toml`. `[app] version` is the version people see. `[app] build` is the build number: `CFBundleVersion` on Apple platforms, `versionCode` on Android (or `[android] version_code`), the Debian revision on Linux, and the fourth part of the executable's version on Windows. Each upload needs a higher build number.

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

These values end up in file names, launchers, manifests and package metadata, so `prism` checks them before building. `package` takes letters, digits, `_` and `-`. `bundle_id` takes two or more dot-separated words of those characters, at most 255 bytes (`com.example.app`). On Linux, `name`, `description` and `linux.maintainer` are one line each, and `build` takes letters, digits, `.`, `+` and `~`.

### Google Play

- **The bundle:** `aapt2 link --proto-format` writes the manifest and resources as protobuf. They go into the base module zip (`manifest/`, `dex/`, `res/`, `resources.pb`, `assets/`, `lib/`, `root/`), and `bundletool build-bundle` assembles it. bundletool 1.18.3 is downloaded once into `~/.cache/prism` and checked against a pinned SHA-256, after the download and before each run (`PRISM_BUNDLETOOL` uses another jar, unchecked). `BundleConfig.json` keeps native libraries uncompressed and 16 KB aligned (`PAGE_ALIGNMENT_16K`), as Android 15+ devices with 16 KB pages need.
- **Signing:** jarsigner signs the bundle with your upload key (`PRISM_ANDROID_KEYSTORE`, `PRISM_ANDROID_KEYSTORE_PASS`, `PRISM_ANDROID_KEY_ALIAS`, and `PRISM_ANDROID_KEY_PASS` if the key's password differs). Play rejects the debug key, so `prism publish` refuses it, and a bundle with an entry left unsigned (changed after signing) or signed by more than one key (`jarsigner -verify -strict`). Create an upload key once: `keytool -genkeypair -keystore upload.jks -alias upload -keyalg RSA -keysize 2048 -validity 10000`. Enroll in Play App Signing, then keep the keystore out of the repository.
- **Testing like Play does:** `prism run android --aab` builds the bundle, then `bundletool build-apks --local-testing --connected-device` and `install-apks` install the split APKs (base, ABI, language, density) that Play would serve to that device.
- **Upload:** `prism publish android [--track beta] [--rollout 0.1 | --draft] [--notes "…"]` uses the Google Play Developer API. It creates an edit (`edits.insert`), uploads the bundle (`edits.bundles.upload`), sets the track's release (`edits.tracks.update`) and commits the edit (`edits.commit`). If any step fails, it deletes the edit. Credentials come from a service account JSON key: `PRISM_PLAY_SERVICE_ACCOUNT` (a path or the JSON itself) or `--service-account <path>` (a path only: a command line is visible to other users of the machine). Invite the service account in the Play Console (Users and permissions) with release rights.
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

### Windows

`prism build windows` makes `target/windows/<name>-<version>-windows-x64.zip`:
one folder with the release executable (its icon, version information and
manifest inside, no console window), the Windows App SDK runtime (54 MB:
the app is self-contained, nothing to install) and the app's assets. People
unzip it and run the executable.

`prism build windows --msix` makes an `.msix` instead (5.9 MB for the
counter): the executable and assets only, with a manifest made from
`Prism.toml` that depends on the Windows App Runtime framework package,
which the user's PC must have (its redistributable installer, or the
Store). The package is packed in plain Rust, on any OS.

Both are signed when `PRISM_WINDOWS_CERTIFICATE` names your code-signing
certificate (a `.pfx`), with its password in `PRISM_WINDOWS_CERTIFICATE_PASS`
(never in `Prism.toml`) and an optional `PRISM_WINDOWS_TIMESTAMP_URL`:
the executable before zipping or packing, then the package. `signtool`
signs on Windows, `osslsigncode` elsewhere. The MSIX's publisher is the
certificate's subject. Windows installs only signed packages whose
certificate it trusts; an unsigned zip runs, with a SmartScreen warning on
its first run once downloaded.

It builds on Windows, and from macOS or Linux with MinGW-w64 or in Docker.
Not yet: Store submission, an installer for the zip. See [Windows ›
Running and shipping](platforms/windows.md#running-and-shipping) and
[MSIX](platforms/windows.md#msix).

### Web

`prism build web --docker` builds `docker/web.Dockerfile` (see [Docker](platforms/web.md#docker-linux)) into an image tagged `<name>:<version>`. The build context is the Cargo workspace, without `target/` and local data, so path dependencies must be inside the workspace. Push the image to your registry yourself.

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
