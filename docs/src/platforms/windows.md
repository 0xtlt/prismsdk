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
embeds the manifest that registers it. Builds with the GNU (MinGW) and the
MSVC toolchains.

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
- **Services:** the clipboard (text), links opened with the default app
  (`cx.open_url`); every other device call answers « unavailable ».
- Storage goes to `%LOCALAPPDATA%\<title>`; the language comes from the
  user's locale, the text size from Settings › Accessibility › Text size
  (Prism scales the text itself: XAML's own text scaling is off), reduced
  motion from « Animation effects ».

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
writes it): on Windows it copies the runtime (`runtimes-framework/<arch>`,
`resources.pri`) next to the executable and embeds the app manifest
(Windows 10/11 compatibility, per-monitor DPI v2, UTF-8, common controls
v6, and the `activatableClass` registrations of the runtime: reg-free
WinRT, what the SDK's MSBuild targets generate). GNU builds compile it with
`windres` (MinGW's binutils, or `WINDRES`); MSVC builds pass it to the
linker (`/MANIFESTINPUT`). A manifest activated at run time is not enough:
WinRT only reads the executable's own.

The application object is created by hand (COM aggregation in `outer.rs`)
so the Windows 11 control styles (`XamlControlsResources`) load without a
XAML compiler.

Footprint (counter, x64): the release executable is about 10 MB like with
any backend; the runtime next to it is 205 files, 54 MB (3.4 MB of them
translations). A debug executable carries the bindings' debug information
(about 375 MB). The package cache takes 270 MB once per target directory.

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
| `PRISM_TRACE` | logs the startup phases |

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
- **Services:** notifications, the tray, the menu
  bar, badges, pickers of photos, files and folders, secure storage,
  biometrics, sign-in, background tasks, push and run-time fonts.
- **Packaging:** `prism build windows` (the runtime and `resources.pri`
  next to the exe in a zip, icon and version info in the manifest; or an
  MSIX with the Windows App Runtime as a framework dependency instead of
  the 54 MB); MSVC's `/MANIFESTINPUT` path is untested; arm64 and x86
  untested.
- **Accessibility:** clickable views are buttons for UI Automation (their
  own role, a link or a tab, is not mapped yet: that needs custom automation
  peers); high contrast and Narrator testing.
- **Performance:** a screen of 650 nodes shows its first frame in about
  1 s (Direct2D: 0.55 s): each node is two or three XAML elements, measured
  through XAML.
