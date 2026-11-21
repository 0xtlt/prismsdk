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
  the other native backends place their views: a `Canvas`, its content (a
  `TextBlock`, a control), and only what its style needs (a `Border` for a
  box, shadows, a children `Canvas` once it has children). A pure model of
  the tree (`scene.rs`) tells which page a stack or tab bar shows and finds
  nodes by label for the smoke hooks. Text is measured through XAML (what
  is measured is what is drawn), once per text and width: a text that
  fits on its lines is not laid out again. New content of a scroll view
  out of the window waits `Collapsed` until the first frame is shown, then
  joins a batch per frame (see [Performance](#performance)).
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
  clips, to the rounded corners too (a Composition geometric clip: a
  rounded rectangle, or a Direct2D path when only some corners are
  rounded, opposite ones or three, so each corner is exact).
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
  pictures (`Image`, decoded by WIC, clipped to their view's rounded
  corners) and shapes (`Path`). A button or a
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
  side and alignment (flipped without room), light dismiss and Escape are
  WinUI's; on closing, the focus goes back to what had it before, else to
  the anchor (not to the page's first element, which would scroll the page
  to its top); `fixed` and `manual` ones ignore a click outside,
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
  changes. A one-line field is as wide as its placeholder or text (120 at
  least, like macOS), and a `leading_icon` is its Segoe Fluent Icons glyph
  in the field's first 24 DIPs, the text after it. Other people's carets and selections
  (`remote_carets`) and a selection group's highlighted parts are drawn on
  a canvas over the field, from its own geometry
  (`GetRectFromCharacterIndex`), measured again after typing, resizing
  and scrolling.
- **Rich inputs** (`rich_input`) are `RichEditBox`es: each run's weight,
  italic, underline (single, thick, double, dotted, dashed),
  strikethrough, colors, background, font and size are RichEdit character
  formats, applied in place when the app's runs hold the text shown (the
  echo of typing: caret and undo untouched). Text without a color of its
  own takes the theme's text color, again when the theme changes
  (RichEdit's automatic color is black, even in a dark theme). Edits reach the app as plain
  text, like on macOS; Return submits, Shift+Return breaks the line,
  pastes are plain text. A chip is a picture embedded in the text
  (`InsertImage`: its Segoe Fluent icon and label in a rounded box, drawn
  with Direct2D at the window's scale, on the baseline), one character the
  caret jumps over and Backspace deletes whole, like macOS's
  `NSTextAttachment`. The editing hooks (#217) are RichEdit's events:
  copies and cuts (`CopyingToClipboard`, `CuttingToClipboard`) and pastes
  go to the app with the clipboard's text, HTML and types; Ctrl- and
  Alt-clicks report the character under the pointer, every click too with
  `on_click` (presses are heard with `handledEventsToo`); the selection's
  rectangle comes from `ITextRange::GetRect`. Remote carets and the
  group's highlights are drawn over it like over a text box. A mouse drag
  from one field of a `text_flow` into another selects across them
  (#264).
- **Links in text** are `Hyperlink`s in the `TextBlock`: Tab reaches them,
  Enter or a click reports the link, UI Automation sees links; they keep
  the run's own color and underline.
- **Web views** are WinUI's `WebView2`, the Chromium engine of the
  Evergreen WebView2 Runtime (part of Windows 11; with Edge on Windows 10).
  The app's environment keeps its data in `%LOCALAPPDATA%\<app>\WebView2`.
  As on Apple platforms and Android, the page posts with
  `prismHost.postMessage(JSON.stringify(value))` (main frame only), gets
  the app's messages as `message` events on `window`, and only the allowed
  origins are heard and posted to; a link to another origin, and a new
  window when the sandbox allows popups, open in the default browser. A
  sandbox without `scripts` turns JavaScript off; one without
  `same-origin` browses InPrivate; one without `forms` submits no form,
  as WebKit cancels `formSubmitted` navigations: WebView2 does not tell
  them apart, so a script at document creation in every frame stops
  `submit` events, `form.submit()` and `requestSubmit()`, and every POST
  of a document (a frame's too) is answered 403 before it leaves
  (`WebResourceRequested`), scripts on or off. `on_load` hears each
  loaded page.
- **Videos** are `MediaPlayerElement`s with the transport controls
  (play, scrubber, volume, full window), a poster, autoplay, looping and
  muted; their height follows the video's ratio once it opens;
  `on_ended` hears the end. Request headers (`Video::header`) go with
  every request: Media Foundation's HTTP source takes none, so the player
  reads a stream of the backend's (`MediaSource::CreateFromStream`) whose
  reads are HTTP range requests with the headers, a megabyte at a time
  (redirects to another origin drop them); HLS and DASH go through an
  `AdaptiveMediaSource` whose `HttpClient` sends them with every
  playlist, manifest and segment request (checked with HLS and DASH test
  streams behind a header).
- **3D scenes** are drawn by prism-native's OpenGL renderer (the GTK
  backend's: `prism_native::gl3d`) in a WGL OpenGL 3.3 core context of the
  app's, shared by every scene, and shown by a `SwapChainPanel`: WinUI
  composes with DirectX, so the GL context shares Direct3D 11 textures
  (`WGL_NV_DX_interop2`, in AMD, NVIDIA and Intel drivers) and each frame
  goes to the panel's swap chain without leaving the GPU. A driver without
  the extension (or `PRISM_3D_READBACK=1`) reads each frame back into a
  `WriteableBitmap`. OpenGL starts after the window's first frame (the GPU
  driver takes a while to load), and frames follow XAML's. Drag
  to turn (right or Shift drag: pan), two fingers or the wheel zoom, a
  click taps
  (`SceneTapEvent`); frames are drawn while something moves, none at
  rest. Textures decode with WIC off the UI thread.
- **Input:** XAML's: pointer, touch, pen, wheel and touchpads (with
  `ScrollViewer`'s inertia), keyboard focus with Tab, access keys. Key
  presses also go to Prism's key listeners first (`on_key_down`,
  `consume_keys`). Context menus (`set_menu`) and pop-up menus
  (`show_menu`) are `MenuFlyout`s. Pointer events reach Prism's
  listeners: hover and moves (`on_pointer_*`), drags with any button past
  the system's drag distance (a touch on a node that drags after a long
  press waits for it; the view captures the pointer, so a clickable view
  does not click and a scroll view stops panning), the wheel and
  precision touchpads (`on_wheel`), pinches (`on_magnify`: two touches on
  the element, which keeps its touches with a manipulation mode of its own
  (`Scale`) so the scroll views around do not take them, a drag following
  the fingers' middle as on iOS; and Ctrl + wheel, how precision
  touchpads send their pinches), middle, back and forward clicks
  (`on_aux_click`), quick horizontal flicks (`on_swipe_*`, any pointer,
  like iOS's `UISwipeGestureRecognizer`, Android's fling and GTK's
  `GestureSwipe`). By touch, a scroll view takes a finger as it lands, so
  a swiping row keeps horizontal translations on rails
  (`System | TranslateX | TranslateRailsX`): a horizontal flick stays its
  own, a vertical pan still scrolls the list, a tap still clicks. WinUI's
  `SwipeControl` reveals swipe items, which Prism's direction-only swipes
  have none of. Drags between views (`draggable`,
  `on_drop_payload`) find their target with prism-native's `locate_drag`,
  with a see-through preview and autoscroll. Files dragged from Explorer
  land on `on_drop_files` targets (copies in the app's `<data>\picked`,
  with the picked files): as on macOS, the drag's files are read when it
  enters, and only a target that accepts one of them (MIME type or
  extension) shows the copy cursor and its `drop_hover` style; folders are
  left out.
- **Accessibility:** WinUI's own UI Automation peers for its controls
  (Button, Edit, CheckBox, Slider, ComboBox, the tab bar's list items…);
  the views Prism draws itself have custom automation peers (`peers.rs`:
  COM aggregation of `Button`, `Canvas` and their peers, as a C++ or C#
  app derives from them) with their own role, mapped as WinUI maps its
  controls and Edge the same ARIA roles (`access.rs`):

  | Prism | UI Automation |
  |---|---|
  | `Button` (a clickable view) | `Button`, Invoke; pressed (`accessibility_checked`): Toggle |
  | `Link` | `Hyperlink`, Invoke |
  | `Checkbox`, `Switch` | `CheckBox`, `Button` read « toggle switch » (WinUI's words for `ToggleSwitch`, in the user's language); Toggle |
  | `Tab`, `TabList`, `TabPanel` | `TabItem` (SelectionItem), `Tab`, `Group`; the tab bar's own items (WinUI's `SelectorBar`) are tab items too |
  | `List`, `ListItem` | `List`, `ListItem` (SelectionItem when selected or current) |
  | `Radio`, `RadioGroup` | `RadioButton` (SelectionItem), `Group` |
  | `Slider`, `ProgressBar`, `ComboBox` | the same control types; their value through the Value pattern |
  | `Group`, `Navigation`, `Main`, `Alert` | `Group`; landmarks for `Navigation` and `Main` |
  | `Heading(n)`, `Text`, `Image` | `Text` with its heading level, `Text`, `Image` |
  | `Dialog`, a sheet or dialog shown | `Window` with `IsDialog` |
  | `Table`, `Row`, `ColumnHeader`, `Cell` (a [`table`](../guide/tables.md)) | `Table` with the Grid and Table patterns (row and column counts, the cell at a row and column, the column headers); `DataItem` rows; `HeaderItem` and `DataItem` cells with the GridItem and TableItem patterns (row, column, table, column header: `table.rs`) |

  Selected, current, checked, disabled and value are states the reader
  reads (and hears change: property-changed events); a container with a
  role is an element even without a name. A sheet or dialog is a dialog
  window named by its label (else its first heading), and the rest of the
  app leaves the UI Automation tree while it is open, so a reader stays
  inside (Tab already cycles inside); popovers are WinUI flyouts, read as
  WinUI's own. A status's new value is announced (`RaiseNotificationEvent`,
  after what the reader is saying; an alert's in its place). Prism's label,
  hint, heading level, live region and tooltip are `AutomationProperties`;
  tooltips are WinUI's (`ToolTipService`), and `tooltip_when_truncated`
  shows a text's full content while XAML trims it (`IsTextTrimmed`). A view
  lifted with `z_index` is drawn above its siblings (`Canvas.ZIndex`) but
  keeps its place in the source for UI Automation and Tab.
- **High contrast:** under a contrast theme (Settings › Accessibility ›
  Contrast themes) the app's colors give way to the theme's system colors,
  as WinUI's controls do theirs and as Edge forces a page's (`contrast.rs`):
  fills become the window's color (a clickable view's, the button face);
  strong fills stay visible, colored ones (accents, badges) and selected
  items as the highlight, gray ones (a switch's knob) in the text's color,
  except in disabled views; text is the window text (the highlight text on
  a highlight, the link color in a link, the button text in a button, the
  gray text when disabled); borders take the text's color; shadows go, so
  a card or a chip shown by its fill or its shadow gets a 1-pixel outline,
  as WinUI's cards do; drawings keep their colors where they show on the
  theme's background; pictures, videos, web pages and 3D scenes keep
  theirs. The window is the theme's color (no Mica), and WinUI's controls
  take the theme's lightness. The theme is read at start
  (`SPI_GETHIGHCONTRAST`, `GetSysColor`) and again when it changes
  (`WM_SETTINGCHANGE`, `WM_SYSCOLORCHANGE`). `PRISM_HIGH_CONTRAST` stands
  in for the system's setting (below).
- **Appearance:** light and dark follow the system (`AppsUseLightTheme`) or
  the app (`RequestedTheme` on the XAML root, a dark title bar), with the
  system accent color in the controls; Mica behind what the app does not
  paint (`PRISM_NO_MICA=1` turns it off).
- **Windows and DPI:** per-monitor DPI v2 (XAML scales by itself; a move
  to a monitor of another DPI resizes the window to keep its size in
  DIPs), resizing, a minimum size, the title (`cx.set_title`), a
  confirmation on close with unsaved changes. `PRISM_SCALE=1.5` runs the
  app at 150 % on any screen, through the same path as a real DPI (see
  [Development hooks](#development-hooks)).
- **Services:** see [Services](#services) below.
- Storage goes to `%LOCALAPPDATA%\<title>`; the language comes from the
  user's locale, the text size from Settings › Accessibility › Text size
  (Prism scales the text itself: XAML's own text scaling is off), reduced
  motion from « Animation effects ».

## Services

| | |
|---|---|
| Files, folders, photos | the shell's common item dialogs (`IFileOpenDialog`, folders with `FOS_PICKFOLDERS`, pictures from the Pictures folder with an image filter), modal to the window; picked files are copied under the app's data directory, a folder's files are read in place. The filters are named in the app's language (« Fichiers (*.png;…) », « Images », « Tous les fichiers »: `prism.files`, `prism.images` and `prism.all_files` in the app's translations override them) |
| Saving a file | `IFileSaveDialog` (Windows asks before replacing); `reveal_path` selects the file in Explorer (`SHOpenFolderAndSelectItems`) |
| Notifications | Windows App SDK app notifications (`AppNotificationManager`), unpackaged and self-contained: title, subtitle, body, buttons (destructive ones in the critical style), reply fields, sound or silent, replace and withdraw by id (the tag), threads (the group); delays are timers in the app |
| Clicks on notifications | `on_notification_response` with the button and a reply's text, while the app runs or after Windows starts it for the click; the window comes forward unless the button is a background one |
| Badges | an overlay on the taskbar button: the number on an accent-colored disc (`ITaskbarList3::SetOverlayIcon`), from `cx.set_badge` or a notification's badge |
| Secure storage | Credential Manager (`CredWriteW`): generic credentials of the user named `Prism/<app>/<key>`, values longer than 2,560 bytes split over several; `biometric` values sealed with a key only Windows Hello gives back (`KeyCredentialManager`: Hello asked when stored and read, `Unavailable` without Hello set up); `confirm_owner` values need Windows Hello's consent when read |
| Biometrics | Windows Hello (`UserConsentVerifier`, its prompt owned by the window), which also takes the Hello PIN; `capabilities().biometry` is the face camera, fingerprint or iris reader the Windows Biometric Framework sees. Without Hello set up, `authenticate` answers `Unavailable` |
| Sign-in (`prism::oauth`) | the default browser and a loopback redirect, `http://127.0.0.1:<port>/callback` (RFC 8252), like on Linux; run end to end on screen against a local OpenID Connect provider (discovery, dynamic registration, consent in the browser, PKCE, a picture behind the access token) |
| Tray | `Shell_NotifyIcon`: the SF Symbol's Segoe Fluent Icons glyph in the taskbar's color (else the app's icon), the tooltip, and a Win32 popup menu on a click (dark when apps are); it comes back when Explorer restarts and goes when the app quits |
| Menu bar | `cx.set_app_menu` / `set_menu_bar` put a WinUI `MenuBar` across the top of the window, under the title bar (the app's view starts under it; see [The menu bar](#the-menu-bar)): access keys (Alt+F), `KeyboardAccelerator`s that work with the menus closed, shortcut hints shown only, checkable items, submenus; platform actions do Windows' own (Cut, Copy, Paste, Select All, Undo, Redo in the focused field; Minimize, Zoom, Close, Quit with Alt+F4) |
| Alerts | `ContentDialog`: the first action is the accent button unless it is destructive, Cancel is the close button (Escape), more than three actions are listed under the message |
| Clipboard | text, HTML (`HTML Format`), RTF, pictures (`PNG` and `CF_DIBV5`, read back from either or from a plain DIB), files (`CF_HDROP` as `text/uri-list`), other MIME types as formats of their name |
| Run-time fonts | written to `%LOCALAPPDATA%\<app>\prism-fonts` and named `ms-appx:///prism-fonts/<hash>.ttf#Family`. XAML loads font files by URI only, and an unpackaged app's `ms-appx:///` is its executable's folder (absolute paths, `file:` URIs and `AddFontResourceEx` are ignored): the app hands XAML its resource manager (`Application.ResourceManagerRequested`), the default one with a fallback (`ResourceManager.ResourceNotFound`) that resolves `prism-fonts/…` to the file in the data folder. So an app installed in a read-only folder (`C:\Program Files`) loads fonts too, nothing is installed, and the font is the app's alone |
| Background tasks | timers while the app runs, like on Linux (`PRISM_BACKGROUND_INTERVAL` shortens them) |
| Location | `Geolocator`, Windows' location service (a GNSS receiver where the PC has one, else Wi-Fi networks and the IP address). `capabilities().location` when the location service (`lfsvc`) is installed and not disabled (it starts on demand, so it need not be running), no policy turns location off and the service has a provider. The permission is Settings › Privacy & security › Location, read without asking (`AppCapability::CheckAccess`: the device's switch, the user's, the desktop apps' one); `LocationAlways` is the same permission (a desktop app has no background limit). `request_permission` asks with `RequestAccessAsync` (Windows 11 24H2 and later show a prompt, once per app). Positions with the web's settings (`Coarse`: a kilometer and ten minutes, `Balanced`: 100 m and 30 s, `Best`: the best and 30 s; 20 s at most); a watch is `PositionChanged` with a movement threshold. Refused is `Denied`, no provider `Unavailable`, no position in time "location unknown" |
| Motion | `Windows.Devices.Sensors`, which tablets and 2-in-1s have: `Accelerometer` (its fused gravity and linear readings where Windows has them, else gravity is the accelerometer low-pass filtered), `Gyrometer`, `Inclinometer`, read every `interval` on a thread of the watch's own, in Apple's units and axes. No permission. `capabilities().motion` when the PC has an accelerometer or a gyrometer |
| Taking a photo | the Windows Camera app's capture UI, launched for a result (`microsoft.windows.camera.picker:`, the protocol `CameraCaptureUI` drives), modal to the window: the app hands it an empty file under its data directory (`<data>\picked`) through a `SharedStorageAccessManager` token, and the Camera app writes the photo (JPEG) into it. Closing it is `Cancelled`. `capabilities().camera` when the PC has a camera and the Camera app; the permission is the Camera app's camera access. See [Device APIs without a package](#device-apis-without-a-package) |
| Share sheet | Windows' Share dialog for the window (`IDataTransferManagerInterop::GetForWindow`, `ShowShareUIForWindow`): the window's title, the text, a link (`SetWebLink`, and after the text, on its own line, for targets that read only text, as on Android) and files (read-only `StorageFile`s; a file that cannot be read is left out, and the text, the link and the other files still go). As on Apple platforms, it answers nothing |
| Passkeys | `webauthn.dll` (Windows 10 1903 and later), the API browsers use: `WebAuthNAuthenticatorMakeCredential` and `GetAssertion` with the relying party, the user, the challenge, the algorithms, `excludeCredentials` and `allowCredentials` with their transports, user verification, the attachment, resident keys, attestation and the timeout of the WebAuthn JSON. The client data is a browser's (`origin` `https://<rpId>`), so a server checks it as it checks a browser's. Windows' own dialog, owned by the window: Windows Hello, security keys, phones. A new ceremony cancels the open one (`WebAuthNCancelCurrentOperation`: `Cancelled`) and starts once that one has returned (Windows runs one at a time; 5 s at most), and only the newest waits; errors carry the web's names as on the web and Android (`WebAuthNGetErrorName`: `NotAllowedError` is `Cancelled`; `InvalidStateError` for an excluded credential, `ConstraintError` and `NotSupportedError`, a request Windows refuses, stay `Platform` errors with that name; none is `Unavailable`). `capabilities().passkeys` when the DLL loads; `webauthn_autofill` answers `Unavailable` (no conditional mediation for apps) |
| Haptics | the PC's vibration device (`VibrationDevice`, its `SimpleHapticsController`): click, press, success and error waveforms, with intensity and repeats. Most PCs have none: `capabilities().haptics` is false and `haptic` does nothing. A pen's haptics (only while it touches the screen, and `haptic` names no pointer) and haptic touchpads (Windows plays their feedback, apps have no API) are not used |
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

### Device APIs without a package

Prism apps on Windows are unpackaged (no MSIX, no package identity), and
every device API above works so. What needs a package, and what Prism does
instead:

- **The camera.** `Windows.Media.Capture.CameraCaptureUI` is for UWP apps
  (a `CoreWindow`), and the Windows App SDK's (1.7 and later) makes its
  file in `ApplicationData::Current()`, which fails without a package
  (`APPMODEL_ERROR_NO_PACKAGE`). Prism launches the capture UI those
  classes show, the Camera app's, itself, and hands it a file of its own.
  Capturing frames with `MediaCapture` would need a camera UI of Prism's
  own. A PC without the Camera app (Windows N editions, or the app
  removed) has no `take_photo`.
- **Push.** WNS gives no channel to an unpackaged app (see Services).
- **Prompts.** Windows asks no one for the camera, motion, sharing or
  haptics; the Camera app asks for the camera itself. Location is one
  switch for all desktop apps (« Let desktop apps access your location »),
  and Windows 11 24H2 and later also ask once per app, listing it by its
  executable's path.
- **Passkeys.** Windows lets a desktop app name any relying party, as it
  lets a browser, so no domain association is needed; the server checks
  the client data's origin, `https://<rpId>`. Conditional mediation
  (passkeys in a field's suggestions) has no API for apps, packaged or not.

`PRISM_DEVICE_REPORT` and `scripts/windows-device-apis.sh` (see
[Development hooks](#development-hooks)) show what a PC has, without a
prompt.

### The menu bar

The app's menus are a row under the title bar, as in Notepad and File
Explorer on Windows 11, not menus drawn in the title bar (Paint's way):

- WinUI's `MenuBar` is made for that row ("typically" at the top of the
  window, says the design guidance), 40 px tall, with its own hover,
  pressed and keyboard states;
- the title bar stays the system's: its caption buttons, drag, double
  click, the window menu and snap layouts need no code of Prism's
  (menus in the title bar mean `ExtendsContentIntoTitleBar`, drawing the
  icon and the title, and keeping the hit-test regions of every menu up to
  date, and its colors are ignored on Windows 10);
- a narrow window (the counter's is 460 px) keeps room for every menu
  beside the caption buttons.

The row is on the window's solid color (`SolidBackgroundFillColorBase`,
the color of Prism's tab bars and stack headers under it), so the title
bar alone shows Mica and the menus read as part of the window's chrome.

## The Windows App SDK

`prismsdk-windows`' build script (only when building for Windows):

1. fetches the component packages `Microsoft.WindowsAppSDK.Foundation`,
   `.InteractiveExperiences` and `.WinUI` (version 2.7.0) from nuget.org
   with `curl` and `tar` (both part of Windows 10 and 11), once, into
   `PRISM_WINAPPSDK_DIR`, else `<target dir>/winappsdk/2.7.0`. Each
   archive (and `Microsoft.Web.WebView2`'s) must match the SHA-256 pinned
   in `prism-build`, or the build stops before unpacking it; a cached
   package is reused only if it was unpacked from that archive. A
   `PRISM_WINAPPSDK_DIR` you fill yourself is used as it is;
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
| `PRISM_CLICK` | `+1\|Save`: presses by label (a control's title, an accessibility label, a tab, `Back`, a bar button, a clickable view showing it; buttons through their UI Automation peer); `fill:Placeholder=text` types into a field; `submit:Placeholder` presses Enter in it; `menu:Title` opens a context menu; `scroll:600` scrolls first (a popover then opens on its anchor in view); `wait:500` lets frames run before the next step (ms); `dpi:144` moves the window to a simulated monitor at that DPI (`WM_DPICHANGED`, the suggested rectangle); `key:Escape` (`Tab`, `Enter`) presses a key. Real mouse and keyboard input (`SendInput`, the window kept on top meanwhile; the view scrolled into sight first; a press goes only where the app's window is under the point and active, keys only while it has the keyboard, so other apps on a shared desktop get none): `hover:Label`, `click:Label@x,y`, `ctrlclick:Label`, `drag:Label=dx,dy` (`drag:Label@x,y=dx,dy`), `swipe:Label=dx`, `wheel:Label=dy` (`=dx,h` sideways), `zoom:Label=dy` (Ctrl + wheel), `type:Label=text`, `chord:ctrl+c`; touches from a synthetic touch device: `pinch:Label=k`, `tswipe:Label=dx`, `tdrag:Label=dx,dy`; the steps after them and `PRISM_AFTER` wait until that input has played (`click:Filters\|key:Escape`). The window is made the active one before the steps (a popover closes when another window takes the activation) |
| `PRISM_TRACE_INPUT` | logs the pointer, drag, swipe, wheel and scene tap events sent to the app |
| `PRISM_SCROLL` | scrolls the visible scroll views to that offset |
| `PRISM_SNAPSHOT` | saves the window's XAML (`RenderTargetBitmap`) as PNG, and what DWM shows of the window (`PrintWindow`: Mica, shadows, open flyouts) as `<name>-window.png`, then quits |
| `PRISM_DELAY`, `PRISM_AFTER` | seconds before the clicks (1) and between them and the snapshot (0.7) |
| `PRISM_WINDOW` | the window's size, `460x720` |
| `PRISM_APPEARANCE` | `light` or `dark` in place of the system's appearance |
| `PRISM_REDUCE_MOTION` | `1` (or `0`) in place of « Animation effects » |
| `PRISM_HIGH_CONTRAST` | in place of the contrast theme, for this app only: `0` (off), `1` (on, with the system colors as they are), or one of Windows 11's themes by name, with its colors: `aquatic`, `desert`, `dusk`, `night-sky`. `PRISM_CLICK=contrast:dusk` changes it while the app runs, through the system's own settings message |
| `PRISM_TRACE` | logs the startup phases (with the UI thread's CPU time), where the backend's time went (create, style, measure, place…), XAML's own layout, the first frame, the XAML elements and the memory then, each update (a click, a tab switch) until the frame showing it, 3D frame times, and each notification shown |
| `PRISM_PROFILE` | `stacks.txt`: samples the UI thread's stacks every millisecond until the first frame (`PRISM_PROFILE_MS` more), tagged by startup phase; `scripts/perf/profile.py` names them (x64) |
| `PRISM_SCALE` | `1.25`, `1.5`, `200%`: the app's scale in place of its monitor's (XAML's `OverrideScale` on the island, Prism's pixels per DIP) |
| `PRISM_DEFER` | `0`: every XAML element, off-screen ones too, is made and laid out before the first frame |
| `PRISM_BROWSER` | a browser's path or command line (`"C:\…\chrome.exe" --user-data-dir=…`): web pages, the sign-in's included, open with it instead of the default browser (for a test PC whose default must not change) |
| `PRISM_3D_READBACK` | `1`: 3D frames through the CPU, as without `WGL_NV_DX_interop2` |
| `PRISM_TRAY_MENU` | `<id>`: that tray item's menu opens by itself, as after a click |
| `PRISM_NOTIFICATIONS_CLEANUP` | `1`: on quit, the app removes its notification registration and its notifications |
| `PRISM_BACKGROUND_INTERVAL` | seconds between background task runs |
| `PRISM_DEVICE_REPORT` | `<file.json>`: at startup, writes what each device API sees, without a prompt or system UI: the capabilities and the checks behind them; location's policy, service, status and setting, and, when access is already allowed, a position's and a watch's accuracy and age (never where the PC is); the motion sensors and a first sample; the cameras, the Camera app's capture UI and a file token's round trip; the window's Share manager; `webauthn.dll`'s API version, its platform authenticator and its error names as Prism's errors; the vibration devices |

Every run logs when WinUI is ready, the first layout and the first frame
(from the process's creation), and the memory at the first frame and at
the snapshot; `PRISM_TRACE` says more (see [Performance](#performance)).

`scripts/windows-device-apis.sh` runs the counter with
`PRISM_DEVICE_REPORT` on a Windows machine and checks the report here
(`scripts/windows/device-report-check.py`): each capability says what its
checks saw, the parts that need no prompt work, the passkey errors map to
Prism's, nothing says where the PC is; with location refused, it also
presses « Locate » (`Denied` through the app). Its header lists what only a
person at the unlocked PC can prove (the dialogs).

`scripts/windows-smoke.sh` runs the counter on its six routes on a Windows
machine (`scripts/windows-remote.sh`, see Contributing › Windows), through
`run-in-session.ps1` when the build has the WinUI runtime next to it, and
checks the PNGs here. `scripts/windows/uia-dump.ps1 -Invoke "+1"`, run in
the session, presses the counter's button through UI Automation; with
`-Json` it writes the tree with what a screen reader reads of each element
(role, name, states, value, heading level, live region, dialog, landmark).
`scripts/windows-a11y.sh` (`PRISM_SMOKE_A11Y=1 scripts/windows-smoke.sh`
runs it too) opens the counter's six routes, a dialog, a sheet and a
popover in the session, presses drawn switches, check boxes and swatches
through UI Automation as a screen reader does, dumps each tree (once what
a case waits for shows: `waitFor`, the popover's `Popup`) and checks
the roles, names, states and reading order of
`scripts/windows/a11y-expect.json` (`scripts/windows/a11y-check.py`), and
that every control has a name. Narrator is never started: it would speak
aloud.

## Performance

The counter's components screen (668 nodes, a 460 × 720 window, release
build) shows its first frame in about 0.46 s on the test PC (Ryzen, 24
threads, AMD Radeon Pro W5700X, a 100 % display); macOS shows the same
screen in 0.37 s. Measured with `scripts/windows-perf.sh` (and
`scripts/macos-perf.sh`): the counter started several times in the desktop
session with `PRISM_TRACE=1`, its data kept after the first run, the
medians of each phase (`scripts/perf/report.py`). The PC was shared with
other agents' builds and tests, so the clock varied by up to 3× between
runs; the UI thread's CPU time, logged with each phase, varies much less.

| `/components`, release | before | now |
|---|---:|---:|
| first frame (median of 16) | 1682 ms | **460 ms** |
| UI thread CPU until the first frame | 1404 ms | 324 ms |
| the app's tree | 116 ms | 12 ms |
| text laid out by XAML to measure it | 2675 times, 218 ms | 367 times, 61 ms |
| XAML elements at the first frame | 2685 | 460 |
| XAML's own layout and the first render | 990 ms | 180 ms |
| memory at the first frame (working set, private) | 174 MB, 192 MB | 98 MB, 53 MB |
| a tab switch, to the screen | 32–85 ms | 49–75 ms |
| debug build: first frame | 2600 ms | 650 ms |

Making the off-screen elements after the first frame, measured in
alternating rounds against the build before it (16 runs each, the same
hour): XAML elements at the first frame 1562 → 460, styling them
30 → 3 ms, placing them 19 → 6 ms, the UI thread's CPU time until the
first frame 383 → 324 ms, the first frame's median 472 → 460 ms (the
clock gains less than the CPU time on the busy PC); everything is shown
by about 1.2 s.

What the backend does for it:

- **Fewer elements.** A node is a `Canvas` and its content; a box, shadows
  and a children `Canvas` exist only once needed (a leaf has no children).
- **Text measured once.** Each text keeps its sizes while its text and font
  stay the same: on one line (with its first baseline, read from the same
  XAML layout), at its narrowest, and within a few widths; a width the text
  fits in is not laid out again.
- **Off-screen content waits.** The layout is prism-native's, computed
  from the scene without XAML, so new nodes of a scroll view that lie
  outside the window get no XAML elements at all until they show: views,
  texts, controls, pictures and shapes are made once in view (containers
  that hold other kinds, such as scroll views, stacks, tabs, overlays, web
  views, videos, rich inputs and 3D scenes, are made at once). After the
  first frame the waiting nodes are made in batches of 48 per frame,
  nearest first (a subtree too big for one batch is split), and at once
  when scrolled to, focused, used as a popover's anchor, or reached by a
  `PRISM_CLICK` step. Each node is made with its state replayed from the
  scene (style, text, value, accessibility, pointer input), joins its
  parent in the tree's order (a `z_index` sibling keeps its paint order
  through `Canvas.ZIndex`), and comes with its
  UI Automation peer: containers and clickable views keep their custom
  peers, so the UI Automation tree is complete once the batches are done
  (about 1.2 s after the start on the counter's components screen).
  `PRISM_DEFER=0` turns it off.
- **3D after the first frame.** OpenGL (the GPU driver: 200 to 700 ms,
  90 MB) starts once the window has shown itself; built-in environments
  are prefiltered when a renderer first reads them, not while the app's
  tree is built (95 ms with MinGW's math functions).

Where the remaining time goes (a quiet run, release): WinUI's start
60 ms; the window, the services and the app's runtime 30 ms (the
counter's SQLite database opens there: hundreds of milliseconds on its
first run); the XAML island 30 ms; the app's tree 12 ms; the backend
creating and styling the visible XAML elements 20 ms; prism-native's layout with
text measurement 130 ms; XAML's own layout 130 ms; the first render 50 ms.
`PRISM_PROFILE` samples the UI thread's stacks by phase to dig further.

**Memory.** At the first frame 98 MB of working set, 53 MB private. The
GPU driver OpenGL loads, after the first frame, adds about 40 MB of
working set and 90 MB private: a screen with a 3D scene ends at about
195 MB / 225 MB, one without stays near its first frame's.

**3D frames.** On the GPU path the UI thread spends 0.5 to 0.7 ms per
frame of the counter's scene (388 × 280 pixels), against 2.0 to 2.4 ms
reading it back through the CPU; frames follow XAML's, at the display's
rate (a 16 ms timer gave about 30 a second).

**DPI.** `PRISM_SCALE=1.25` (`1.5`, `2`) runs the app at that scale on
any screen: XAML draws at it (the island's `OverrideScale`) and Prism
sizes the window and counts pixels with it; the `dpi:144` PRISM_CLICK
step moves the window to a simulated monitor through `WM_DPICHANGED`
(460 × 600 DIPs stay 460 × 600 DIPs: 690 × 900 pixels at 150 %). Text,
1-pixel borders, icons, pictures and 3D scenes are drawn at the scale.

## Not yet

- **Embedded content and input:** a precision touchpad's
  pinch is only handled as the Ctrl + wheel Windows sends apps (touchpads
  cannot be injected).
- **Services:** push answers « unavailable » (WNS needs package
  identity); notification banners and the Hello prompt are untested on
  screen (the test PC shows no banners from any app and has no Hello
  device: delivery, clicks and the « unavailable » answer are tested). The
  device APIs' dialogs (the Share dialog, the Camera app's capture UI,
  Windows' passkey dialog, the location prompt) are not run on screen:
  `scripts/windows-device-apis.sh` checks everything up to them, without a
  prompt; motion and haptics are untested on a PC with the sensors or a
  vibration device.
- **Packaging:** an MSIX with the Windows App Runtime as a framework
  dependency ([MSIX](#msix)); an installer (Start menu shortcut, file
  associations, uninstall entry): the zip is a portable app; code signing;
  MSVC, arm64 and x86 builds not run on a PC yet ([Architectures and
  MSVC](#architectures-and-msvc)).
- **Accessibility:** checked through UI Automation (`windows-a11y.sh`),
  not with Narrator listening (it speaks aloud); `PRISM_HIGH_CONTRAST`
  forces Prism's colors and WinUI's light or dark theme, but WinUI's own
  controls take their contrast colors from the system alone (a text box
  stays Fluent's under the override).
- **Performance:** a busy machine stretches the first frame (the
  measurements above come from a PC shared with other agents); a screen
  reader that starts reading in the first second or two of a long screen
  finds its off-screen part missing until the batches are done.
