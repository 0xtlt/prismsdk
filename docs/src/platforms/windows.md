# Windows

`cargo run` on Windows opens a native window (`prism-windows`) whose content
is real WinUI 3: a XAML island (`DesktopWindowXamlSource`) of the Windows
App SDK in a plain Win32 window. Prism's controls are WinUI controls
(`Button`, `TextBox`, `ToggleSwitch`, `Slider`, `ComboBox`,
`CalendarDatePicker`, `SelectorBar`…), its text is `TextBlock`s, its boxes
are `Border`s and Composition shadows, all laid out by prism-native's
flexbox like on the other platforms.

The Windows App SDK is **self-contained**: nothing is installed on the
user's machine. The build fetches it from nuget.org once and puts its
runtime next to the executable (about 54 MB, x64); `prism_build::link_app()`
embeds the manifest that registers it, with the app's icon and version.
Builds with the GNU (MinGW) and the MSVC toolchains, and cross-compiles
from macOS and Linux. `prism run windows` and `prism dev windows` run the
app; `prism build windows` ships it as a zip ([Running and
shipping](#running-and-shipping)).

The milestone: the `counter` example runs on every tab, its smoke test
passes in a desktop session, and UI Automation presses its buttons. Some
kinds and services come next (see [Not yet](#not-yet)).

## How it works

- **Layout** is prism-native's flexbox; each node is a few XAML elements
  placed at its frame in `Canvas`es (`Canvas.Left/Top`, width, height), as
  the other native backends place their views. A pure model of the tree
  (`scene.rs`) tells which page a stack or tab bar shows and finds nodes by
  label for the smoke hooks.
- **Styles:** a `Border` draws the background (solid, `LinearGradientBrush`,
  `RadialGradientBrush`, `AcrylicBrush` for a backdrop blur, and a conic
  gradient's pixels in a `WriteableBitmap` through an `ImageBrush`), the
  border and per-corner radii; dashed and dotted borders are a stroked
  `Rectangle`; sides of different colors are one `Path` each, split at the
  corners like CSS; drop shadows are Composition `DropShadow`s masked by the
  box's rounded rectangle (exact offset, blur, spread and color), inset
  shadows a `DropShadow` cast by a frame around the box's inside, clipped to
  it; opacity and transforms are the element's Composition facades
  (`Opacity`, `Translation`, `Rotation`, `Scale`); `overflow: hidden`
  clips, to the rounded corners too (a Composition geometric clip).
- **Text** is `TextBlock`s: fonts and weights per run (`Run` inlines),
  colors, underline and strikethrough, inline Segoe Fluent Icons, CSS line
  heights, letter spacing, wrapping, line clamps with an ellipsis,
  alignment. The UI font is Segoe UI Variable (Segoe UI on Windows 10).
  Text is measured by XAML itself on a scratch `TextBlock` set up by the
  same code, so what is measured is what is drawn, wrapping included.
- **Controls** are WinUI's, with their own hover, pressed, focus and
  disabled states, animations, keyboard handling and Windows 11 look:
  buttons, text boxes (`PasswordBox` for secure fields), toggles, check
  boxes, radio buttons, sliders, progress bars and rings, combo boxes
  (`picker`), `CalendarDatePicker` and `TimePicker` (`date_picker`), symbol
  images (`FontIcon`, Segoe Fluent Icons for common SF Symbol names),
  pictures (`Image`, decoded by WIC) and shapes (`Path`). A button or a
  field the app styles (a background, a border, `.border(0.)`) keeps the
  real control with its chrome made transparent (lightweight styling): the
  node's `Border` draws the app's look under it.
- **Clickable views** (`on_click`, `ui::button`, list rows) hold their
  children in a chrome-less WinUI `Button`: Invoke, Enter and Space, the
  focus rectangle and the UI Automation peer come from WinUI; hover and
  pressed refinements are Prism's.
- **Navigation:** a `SelectorBar` (the Windows 11 top tabs) for a tab bar,
  and a header per stack with Back (`NavigationBackButtonNormalStyle`), the
  title and the screen's bar buttons.
- **Overlays:** dialogs and sheets have `ContentDialog`'s look: the smoke
  over the window (the page blurred for `Backdrop::Blur`), a centered panel
  with its edge, 8 px corners and deep shadow, scaling down from 1.05 and
  fading in (the reverse to close). They take the keyboard focus (Tab
  cycles inside) and give it back, and close on Escape or a click on the
  smoke unless `fixed`; a bare dialog has no panel, shadow or animation.
  Windows has no sheet: a sheet is a larger dialog. Popovers (and dialogs
  placed beside their anchor) are WinUI `Flyout`s on the anchor: the
  acrylic presenter, its shadow and entry animation, placement from the
  side and alignment (flipped without room), light dismiss, Escape and the
  focus are WinUI's; `fixed` and `manual` ones ignore a click outside,
  `keep_focus` ones are transient and let the click through.
  `cx.show_alert` is a `ContentDialog` (Windows 11's style): the first two
  actions are its primary and secondary buttons, the cancel action its
  close button (Escape); alerts wait for the one before.
- **Animations:** transitions of opacity and transforms, enter and exit
  poses and loops (`repeat`) are Composition keyframe animations on the
  element's facades (`UIElement::StartAnimation`), with the transition's
  own curve and delay; an interrupted one goes on from where it is. Frame
  changes step on each rendered frame (`CompositionTarget::Rendering`). A
  node with an exit pose stays in place until it has animated out. With
  « Animation effects » off, nothing animates (WinUI's own controls follow
  the setting too).
- **Text fields** (`text_input`, `text_area`) are `TextBox`es: typing, IME,
  spell checking, selection, undo and the clipboard are WinUI's. The
  backend reports edits (`Typed` tells the app's echo from the user's
  typing), Enter (`on_submit`), the selection (`on_select`) and focus
  changes.
- **Input:** XAML's: pointer, touch, pen, wheel and touchpads (with
  `ScrollViewer`'s inertia), keyboard focus with Tab, access keys. Key
  presses also go to Prism's key listeners first (`on_key_down`,
  `consume_keys`). Context menus (`set_menu`) and pop-up menus
  (`show_menu`) are `MenuFlyout`s.
- **Accessibility:** WinUI's own UI Automation peers (Button, Edit,
  CheckBox, Slider, ComboBox, the tab bar's list items…), with Prism's
  label, hint, heading level, live region (alerts, status) and tooltip as
  `AutomationProperties`.
- **Appearance:** light and dark follow the system (`AppsUseLightTheme`) or
  the app (`RequestedTheme` on the XAML root, a dark title bar), with the
  system accent color in the controls; Mica behind what the app does not
  paint (`PRISM_NO_MICA=1` turns it off).
- **Windows and DPI:** per-monitor DPI v2 (XAML scales by itself),
  resizing, a minimum size, the title (`cx.set_title`), a confirmation on
  close with unsaved changes.
- **Services:** see [Services](#services) below.
- Storage goes to `%LOCALAPPDATA%\<title>`; the language comes from the
  user's locale, the text size from Settings › Accessibility › Text size
  (Prism scales the text itself: XAML's own text scaling is off), reduced
  motion from « Animation effects ».

## Services

| | |
|---|---|
| Files, folders, photos | the shell's common item dialogs (`IFileOpenDialog`, folders with `FOS_PICKFOLDERS`, pictures from the Pictures folder with an image filter), modal to the window; picked files are copied under the app's data directory, a folder's files are read in place |
| Saving a file | `IFileSaveDialog` (Windows asks before replacing); `reveal_path` selects the file in Explorer (`SHOpenFolderAndSelectItems`) |
| Notifications | Windows App SDK app notifications (`AppNotificationManager`), unpackaged and self-contained: title, subtitle, body, buttons (destructive ones in the critical style), reply fields, sound or silent, replace and withdraw by id (the tag), threads (the group); delays are timers in the app |
| Clicks on notifications | `on_notification_response` with the button and a reply's text, while the app runs or after Windows starts it for the click; the window comes forward unless the button is a background one |
| Badges | an overlay on the taskbar button: the number on an accent-colored disc (`ITaskbarList3::SetOverlayIcon`), from `cx.set_badge` or a notification's badge |
| Secure storage | Credential Manager (`CredWriteW`): generic credentials of the user named `Prism/<app>/<key>`, values longer than 2,560 bytes split over several; `biometric` values need Windows Hello when read |
| Biometrics | Windows Hello (`UserConsentVerifier`, its prompt owned by the window), which also takes the Hello PIN; `capabilities().biometry` is the face camera, fingerprint or iris reader the Windows Biometric Framework sees. Without Hello set up, `authenticate` answers `Unavailable` |
| Sign-in (`prism::oauth`) | the default browser and a loopback redirect, `http://127.0.0.1:<port>/callback` (RFC 8252), like on Linux |
| Tray | `Shell_NotifyIcon`: the SF Symbol's Segoe Fluent Icons glyph in the taskbar's color (else the app's icon), the tooltip, and a Win32 popup menu on a click (dark when apps are); it comes back when Explorer restarts and goes when the app quits |
| Menu bar | `cx.set_app_menu` / `set_menu_bar` put a WinUI `MenuBar` across the top of the window (the app's view starts under it): access keys (Alt+F), `KeyboardAccelerator`s that work with the menus closed, shortcut hints shown only, checkable items, submenus; platform actions do Windows' own (Cut, Copy, Paste, Select All, Undo, Redo in the focused field; Minimize, Zoom, Close, Quit with Alt+F4) |
| Alerts | `ContentDialog`: the first action is the accent button unless it is destructive, Cancel is the close button (Escape), more than three actions are listed under the message |
| Clipboard | text, HTML (`HTML Format`), RTF, pictures (`PNG` and `CF_DIBV5`, read back from either or from a plain DIB), files (`CF_HDROP` as `text/uri-list`), other MIME types as formats of their name |
| Run-time fonts | written to `prism-fonts\` next to the executable and named by `ms-appx:///prism-fonts/…#Family`: XAML loads font files from the app's folder only (not absolute paths, `file:` URIs or `AddFontResourceEx`), so an app installed in a read-only folder cannot load fonts at run time |
| Background tasks | timers while the app runs, like on Linux (`PRISM_BACKGROUND_INTERVAL` shortens them) |
| Push | « unavailable »: Windows push notifications (WNS) need package identity (an MSIX or a sparse package), so an unpackaged app gets no channel |

**Notification registration.** `AppNotificationManager::Register` gives the
unpackaged app an identity for notifications, per user: its display name and
the COM server a click starts (the executable), under
`HKCU\Software\Classes\AppUserModelId` and `\CLSID`. It runs the first time
the app notifies or asks about notifications, then a second after every
startup (a marker in the app's data directory), so a click on an earlier notification
reaches the running app rather than starting a second one. When the app quits it unregisters its COM server
(`Unregister`), so a later click starts it again; an uninstaller removes the
rest (`UnregisterAll`, which leaves the key naming the app by its path,
`AppUserModelId\C:.…exe`, and Windows' own settings for it under
`HKCU\Software\Microsoft\Windows\CurrentVersion\Notifications\Settings`).
With `PRISM_NOTIFICATIONS_CLEANUP=1` the app does all of that itself on quit
and also clears its notifications: tests leave nothing behind.

## The Windows App SDK

`prismsdk-windows`' build script (only when building for Windows):

1. fetches the component packages `Microsoft.WindowsAppSDK.Foundation`,
   `.InteractiveExperiences` and `.WinUI` (version 2.7.0) from nuget.org
   with `curl` and `tar` (both part of Windows 10 and 11), once, into
   `PRISM_WINAPPSDK_DIR`, else `<target dir>/winappsdk/2.7.0`;
2. generates the WinUI projection from their `.winmd` metadata with
   `windows-bindgen` (the types in `crates/prism-windows/winui-types.txt`,
   against the `windows` crate).

The app's own build script calls `prism_build::link_app()` (`prism new`
writes it). On Windows it:

- copies the runtime (`runtimes-framework/<arch>`, `resources.pri`) next to
  the executable;
- embeds the app's resources, written by `prism_build::winres` as one
  `.res` file, without a resource compiler:
  - the manifest: the app's identity (its bundle id and version), Windows
    10/11 compatibility, `asInvoker`, per-monitor DPI v2, UTF-8, common
    controls v6, and the `activatableClass` registrations of the runtime
    (reg-free WinRT, what the SDK's MSBuild targets generate);
  - the icon and the version information (see [What the executable
    says](#what-the-executable-says));
- links release builds as Windows programs, which open no console (see
  [No console](#no-console)).

Each toolchain links the `.res` its own way. GNU (MinGW): `windres` (its
binutils: `x86_64-w64-mingw32-windres`, else `windres`, or `WINDRES`) turns
it into an object named like MinGW's default manifest, found first with
`-B`, so the executable gets one manifest, not two (GNU `ld` cannot read a
`.res`). MSVC: `link.exe` takes the `.res` itself (it runs `cvtres`), with
`/MANIFEST:NO` so it adds no manifest of its own. GNU with LLVM
(`*-gnullvm`): `lld` takes the `.res` itself. A manifest activated at run
time is not enough: WinRT only reads the executable's own.

The application object is created by hand (COM aggregation in `outer.rs`)
so the Windows 11 control styles (`XamlControlsResources`) load without a
XAML compiler.

Footprint (counter, x64): the release executable is about 10 MB like with
any backend; the runtime next to it is 205 files, 54 MB (3.4 MB of them
translations). A debug executable carries the bindings' debug information
(about 375 MB). The package cache takes 270 MB once per target directory.

## Running and shipping

On a Windows PC, `prism run windows` builds the app (debug) and starts it;
`prism dev windows` also rebuilds and restarts it on every save, on the
same screen. Windows locks a running program's executable, so `prism dev`
stops the old version before it builds the new one. Both pass `Prism.toml`'s
name, version, publisher and icon to the build, like `prism build`. Off
Windows they stop with a message: build the zip there instead.

### No console

Release builds are Windows programs (the GUI subsystem): no console window
opens with the app. `prism_build::link_app()` links them so
(`-Wl,--subsystem,windows` for GNU, `/SUBSYSTEM:WINDOWS
/ENTRY:mainCRTStartup` for MSVC, what Rust's `#![windows_subsystem =
"windows"]` passes), so apps need no attribute. Debug builds stay console
programs, so `cargo run` and `prism run windows` print the app's logs in the
terminal; a double-clicked debug build opens a console beside its window.
`PRISM_WINDOWS_SUBSYSTEM=windows` or `console` chooses for any profile. A
release build started by `prism run windows --release` inherits `prism`'s
output handles, so its logs still arrive when that output is a pipe or a
file (CI, SSH, `| tee`); for logs in an interactive console window, run a
debug build or set `PRISM_WINDOWS_SUBSYSTEM=console`.

### `prism build windows`

1. Builds the app in release, its executable carrying its icon, version
   information and manifest.
2. Writes the folder `target/windows/<Name>/`: `<package>.exe`, the Windows
   App SDK runtime (207 files, `resources.pri` among them) and the app's
   `assets/` (the folder the app declares, `PRISM_DESCRIBE=assets`), and
   nothing else: no debug symbols, no build files.
3. Zips it: `target/windows/<package>-<version>-windows-<arch>.zip`. The
   counter's is 26.6 MB, 67.5 MB unzipped (its 10.4 MB executable, the
   runtime and two assets), built on Windows or cross-compiled from a Mac.

People unzip it anywhere and run the executable: nothing to install, no
administrator, on Windows 10 (1809 and later) and 11. The app finds its
pictures in the `assets` folder next to it (`PRISM_ASSETS_DIR` wins) and
keeps its data in `%LOCALAPPDATA%\<title>`.

### What the executable says

| | From | Shown by |
|---|---|---|
| Icon | `[app] icon`: the 1024×1024 PNG, scaled to 16, 20, 24, 32, 40, 48, 64 (32-bit images with alpha) and 256 pixels (PNG), or an `.ico` as it is | Explorer, the taskbar, Alt+Tab, the title bar |
| `FileDescription`, `ProductName` | `[app] name` | Task Manager, the taskbar's menus, Properties › Details |
| `ProductVersion`, `FileVersion` | `[app] version`; the binary version is its three numbers and `[app] build` | Properties › Details |
| `CompanyName` | `[windows] publisher`, else the first author of the `Cargo.toml` | Properties › Details, Task Manager's details |
| `Comments` | `[app] description` | Properties › Details |
| `InternalName`, `OriginalFilename` | the package (`counter`, `counter.exe`) | Properties › Details |
| Manifest identity | `[app] bundle_id` and the binary version; the name as its description | side-by-side activation |

`prism` passes them to cargo as `PRISM_APP_NAME`, `PRISM_VERSION`,
`PRISM_BUILD`, `PRISM_BUNDLE_ID`, `PRISM_PUBLISHER`, `PRISM_DESCRIPTION` and
`PRISM_ICON`; a plain `cargo build` uses the package's own name, version,
authors and description, without an icon. The window takes the icon from
the executable (`LoadImageW` of icon group 1).

### Toolchains and cross-compiling

| Builds on | How | Needs |
|---|---|---|
| Windows | `cargo build` with the default toolchain; `--target <triple>` for another | MSVC Build Tools, or MinGW-w64 on `PATH` |
| macOS or Linux with MinGW-w64 | cross-compiles for `x86_64-pc-windows-gnu` (`--target` for another) | `brew install mingw-w64` or `apt install gcc-mingw-w64-x86-64`, then `rustup target add x86_64-pc-windows-gnu` |
| macOS or Linux without | in the `prism-windows-cross` Docker image (Rust and MinGW-w64, built once from `docker/windows-cross.Dockerfile`), with its own target directory, `target/windows-cross` | Docker |

A cross-compiled app cannot be asked for its assets (it does not run
there): the project's `assets/` folder goes in the zip. MSVC targets build
only on Windows.

### Architectures and MSVC

- **x64:** built, run and shipped with the GNU toolchain (WinLibs GCC) on a
  Windows 11 PC.
- **arm64:** built and zipped on the x64 PC with `prism build windows
  --target aarch64-pc-windows-gnullvm` (a user-level `rustup target add`):
  an arm64 executable with its resources, linked by `lld`, the runtime from
  `runtimes-framework/win-arm64`, a `-arm64` zip (24 MB for `prism new`'s
  app). Every app needs a C compiler for the target, even for `cargo
  check`: `ring` (rustls, under `prism-http`) has C and assembly. With
  [llvm-mingw](https://github.com/mstorsjo/llvm-mingw) unpacked anywhere,
  set `CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_LINKER` and
  `CC_aarch64_pc_windows_gnullvm` to its `aarch64-w64-mingw32-clang.exe`
  and `AR_aarch64_pc_windows_gnullvm` to its `llvm-ar.exe`; do not put it on
  `PATH` beside another MinGW, whose x64 build scripts its
  `x86_64-w64-mingw32-gcc` would then link. `aarch64-pc-windows-msvc` needs
  MSVC's ARM64 tools. Not run on an arm64 PC yet; x86 is untested.
- **MSVC:** the same `.res`, `/MANIFEST:NO`, `/SUBSYSTEM:WINDOWS` and
  `/ENTRY:mainCRTStartup`, as documented for `link.exe`; the CI's `windows`
  job (MSVC) builds the counter with WinUI and runs `prism build windows`,
  but no PC has run an MSVC build yet.

### MSIX

`prism build windows` makes no MSIX yet. A package that depends on the
Windows App Runtime framework package, instead of carrying the 54 MB
runtime, needs:

1. an `AppxManifest.xml`: the identity (the bundle id, the signing
   certificate's subject as publisher, the four-part version), a
   `PackageDependency` on the Windows App Runtime framework package of the
   SDK's release, a desktop `Application` with `runFullTrust`, and logos
   (44 and 150 pixels, from the icon);
2. an executable without the runtime's class registrations in its manifest,
   and no runtime next to it (`PRISM_WINAPPSDK_NO_COPY=1`): the framework
   package registers the classes;
3. the package's own `resources.pri` (`makepri.exe`);
4. packing: `makeappx.exe pack` from the Windows SDK, or `makemsix` (the
   open-source MSIX SDK, also on macOS and Linux), which writes the zip, its
   `[Content_Types].xml` and its `AppxBlockMap.xml` (SHA-256 of each 64 KB
   block);
5. signing: `signtool.exe sign` with a certificate whose subject is the
   manifest's publisher and that the user's PC trusts (the Microsoft Store
   signs the packages it distributes);
6. on the user's PC, the Windows App Runtime installed (its redistributable
   installer; the Store installs it with a Store app).

Packing could be done in plain Rust; signing (an Authenticode PKCS #7
signature over the package's digest) needs `signtool` or a signing service.
The zip needs none of it, nor code signing, but a downloaded unsigned
executable gets a SmartScreen warning on its first run; sign it with
`signtool` and a code-signing certificate before zipping to avoid it.

## Desktop session

WinUI renders through DWM, so only in a signed-in desktop session. Started
from SSH (session 0, services), the app builds and lays its tree out, logs
that it cannot show it, and `PRISM_SNAPSHOT` fails. Run it in the user's
own session with `scripts/windows/run-in-session.ps1` (the user must be
signed in, on screen or switched away from).

## Development hooks

The same as the other backends, in a desktop session:

| Variable | |
|---|---|
| `PRISM_PATH` | the first route |
| `PRISM_CLICK` | `+1\|Save`: presses by label (a control's title, an accessibility label, a tab, `Back`, a bar button, a clickable view showing it; buttons through their UI Automation peer); `fill:Placeholder=text` types into a field; `submit:Placeholder` presses Enter in it; `menu:Title` opens a context menu; `scroll:600` scrolls first (a popover then opens on its anchor in view); `key:Escape` (`Tab`, `Enter`) presses a key |
| `PRISM_SCROLL` | scrolls the visible scroll views to that offset |
| `PRISM_SNAPSHOT` | saves the window's XAML (`RenderTargetBitmap`) as PNG, and what DWM shows of the window (`PrintWindow`: Mica, shadows, open flyouts) as `<name>-window.png`, then quits |
| `PRISM_DELAY`, `PRISM_AFTER` | seconds before the clicks (1) and between them and the snapshot (0.7) |
| `PRISM_WINDOW` | the window's size, `460x720` |
| `PRISM_APPEARANCE` | `light` or `dark` in place of the system's appearance |
| `PRISM_REDUCE_MOTION` | `1` (or `0`) in place of « Animation effects » |
| `PRISM_TRACE` | logs the startup phases, and each notification shown |
| `PRISM_TRAY_MENU` | `<id>`: that tray item's menu opens by itself, as after a click |
| `PRISM_NOTIFICATIONS_CLEANUP` | `1`: on quit, the app removes its notification registration and its notifications |
| `PRISM_BACKGROUND_INTERVAL` | seconds between background task runs |

Every run logs when WinUI is ready, the first layout and the first frame
(from the process's creation), and the memory at the first frame and at
the snapshot.

`scripts/windows-smoke.sh` runs the counter on its six routes on a Windows
machine (`scripts/windows-remote.sh`, see Contributing › Windows), through
`run-in-session.ps1` when the build has the WinUI runtime next to it, and
checks the PNGs here. `scripts/windows/uia-dump.ps1 -Invoke "+1"`, run in
the session, presses the counter's button through UI Automation.

## Not yet

- **Controls:** web views, videos and 3D scenes are empty boxes; rich
  inputs edit plain text; links in text are colored, not `Hyperlink`s;
  drags and drops between views (`locate_drag` gets no pointer input),
  pointer and wheel events for `on_pointer` / `on_wheel`, swipes. A rounded
  clip with only some corners rounded rounds them all but those on one
  side; dialogs and sheets have no UI Automation dialog role yet.
- **Services:** push (WNS needs package identity), location, motion, the
  camera, the share sheet (`DataTransferManager`), haptics and passkeys
  answer « unavailable »; run-time fonts need a writable app folder;
  notification banners and the Hello prompt are untested on screen (the test
  PC shows no banners from any app and has no Hello device: delivery, clicks
  and the « unavailable » answer are tested).
- **Packaging:** an MSIX with the Windows App Runtime as a framework
  dependency ([MSIX](#msix)); an installer (Start menu shortcut, file
  associations, uninstall entry): the zip is a portable app; code signing;
  MSVC, arm64 and x86 builds not run on a PC yet ([Architectures and
  MSVC](#architectures-and-msvc)).
- **Accessibility:** clickable views are buttons for UI Automation (their
  own role, a link or a tab, is not mapped yet: that needs custom automation
  peers); high contrast and Narrator testing.
- **Performance:** a screen of 650 nodes shows its first frame in about
  1 s (Direct2D: 0.55 s): each node is two or three XAML elements, measured
  through XAML.
