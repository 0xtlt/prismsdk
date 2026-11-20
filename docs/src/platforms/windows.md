# Windows

`cargo run` on Windows opens a native window (`prism-windows`): one Win32
window whose content the backend draws itself with Direct2D and DirectWrite,
in the Windows 11 (Fluent) look. Text fields are RichEdit controls. Nothing
to install or ship: the app uses system DLLs only, and builds with the MSVC
and the GNU (MinGW) toolchains.

The first milestone: the `counter` example runs on every tab, and its smoke
test passes over SSH. Some kinds and services come in the next steps (see
[Not yet](#not-yet)).

## How it works

- **Layout** is prism-native's flexbox, as on macOS and Linux; the backend
  keeps a scene of the nodes (frames, styles, texts) and draws it. A change
  repaints the region it touched (where a node was and where it is now).
- **Styles** are drawn as specified: per-corner radii with anti-aliased
  clipping of children (`overflow: hidden`), solid, dashed, dotted and
  per-side borders, outer and inset shadows (Gaussian blur), linear, radial
  and conic gradients, opacity of a whole subtree, transforms, and backdrop
  blur where the target can be read back.
- **Text** is DirectWrite: fonts and weights per run, line heights (CSS
  line boxes), letter spacing, tabular figures, wrapping, line clamps with an
  ellipsis, alignment, underline and strikethrough, links, inline icons,
  color emoji. The UI font is Segoe UI Variable (Segoe UI on Windows 10).
- **Controls** are drawn in the Fluent look of WinUI 3, with hover, pressed
  and keyboard-focus states: buttons, text boxes, toggles, check boxes, radio
  groups, sliders, progress bars and rings, combo boxes (`picker`, a menu
  with the options), date pickers (their value), symbol images (Segoe Fluent
  Icons for common SF Symbol names), pictures (WIC: PNG, JPEG, GIF's first
  frame, BMP, TIFF, ICO, WebP and HEIF when Windows has the codecs) and
  shapes. A button or a field the app styles (a background, a border,
  `.border(0.)`) is drawn as the app says. Light and dark follow the system
  (`AppsUseLightTheme`) or the app's choice; controls use the system's accent
  color.
- **Navigation:** a tab bar at the top (a `NavigationView` in top mode,
  with icons) and a header per stack with Back, the title and the screen's
  bar buttons. Alt+Left goes back.
- **Text fields** (`text_input`, `text_area`) are RichEdit child windows
  over their node: typing, IME, selection, undo and the clipboard are
  Windows'. The backend draws their chrome and placeholder, reports edits,
  Enter (`on_submit`), the selection (`on_select`) and focus changes, and
  clips them to their scroll views and around floating nodes above them.
- **Input:** the mouse (hover, press, click counts, right-click menus),
  the wheel and precision touchpads (48 points per notch), keyboard focus
  with Tab and Shift+Tab, Enter and Space on the focused control, arrows on
  tab bars, sliders and radio groups, page keys in scroll views, Escape on an
  open overlay, and every key for `on_key_down` (with `consume_keys`).
  Overlay scroll bars appear on scroll views and can be dragged.
- **Accessibility:** a UI Automation provider exposes the scene to screen
  readers and test tools: names, control types, automation ids
  (`n<node id>`), bounds, focus, and the Invoke, Toggle, Value,
  SelectionItem and Scroll patterns.
- **Windows and DPI:** per-monitor DPI (v2), resizing, a minimum size, the
  title (`cx.set_title`), a dark title bar in dark mode, a confirmation on
  close with unsaved changes.
- **Rendering:** on screen, a DXGI flip-model swap chain on a D3D11 device
  (the GPU, else WARP); without GPU presentation (no desktop compositor),
  Direct2D's software rasterizer into a GDI bitmap. The same drawing goes
  offscreen into a WIC bitmap for `PRISM_SNAPSHOT`.
- **Services:** the clipboard (text), links opened with the default app
  (`cx.open_url`); every other device call answers « unavailable ».
- Storage goes to `%LOCALAPPDATA%\<title>`; the language comes from the
  user's locale, the text size from Settings › Accessibility › Text size,
  reduced motion from « Animation effects ».

## Development hooks

The same as the other backends, over SSH too (session 0, where no window
ever shows: snapshots are drawn offscreen):

| Variable | |
|---|---|
| `PRISM_PATH` | the first route |
| `PRISM_CLICK` | `+1\|Save`: presses by label (a control's title, an accessibility label, a tab, `Back`, a bar button, a clickable view showing it); `fill:Placeholder=text` types into a field; `submit:Placeholder` presses Enter in it; `menu:Title` opens a context menu |
| `PRISM_SCROLL` | scrolls the visible scroll views to that offset |
| `PRISM_SNAPSHOT` | saves the window (text fields included) as PNG, then quits |
| `PRISM_DELAY`, `PRISM_AFTER` | seconds before the clicks (1) and between them and the snapshot (0.7) |
| `PRISM_WINDOW` | the window's size, `460x720` |
| `PRISM_APPEARANCE` | `light` or `dark` in place of the system's appearance |
| `PRISM_WINDOWS_GDI` | draw through GDI (no swap chain) |

`scripts/windows-smoke.sh` runs the counter on its six routes on a Windows
machine over SSH (`scripts/windows-remote.sh`, see Contributing › Windows)
and checks the PNGs here. `scripts/windows/uia-dump.ps1 -Invoke "+1"` presses
the counter's button through UI Automation.

## Not yet

- **Controls:** date pickers have no calendar; sliders and progress bars
  have no RangeValue pattern; overlays (sheets, dialogs, popovers) are drawn
  in the window, not as popup windows, and context menus and pickers are
  classic `HMENU`s (light only); no animations yet (transitions apply at
  once); web views, videos and 3D scenes draw a placeholder box; dotted,
  dashed, thick and double underlines are single lines; rich inputs edit
  plain text.
- **Services:** notifications, alerts, the tray, the menu bar, badges,
  pickers of photos, files and folders, secure storage, biometrics,
  sign-in, background tasks, push and run-time fonts.
- **Packaging:** `prism build windows` (manifest, icon, version info, a zip
  or an MSIX); until then the DPI awareness is set at startup.
- **Accessibility:** the Text pattern for fields and labels, live regions,
  property-change events with values, Narrator testing, high contrast.
- **Not seen yet:** the on-screen path (swap chain, dark title bar,
  per-monitor DPI changes) and IME have only been tested without a desktop
  (over SSH, where Windows shows no window): a signed-in session is needed.
