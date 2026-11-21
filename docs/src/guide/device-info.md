# Device information

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

| Field | iOS / iPadOS | macOS | Web | Android | Linux | Windows |
|---|---|---|---|---|---|---|
| `kind` (phone, tablet, desktop, tv) | idiom | desktop | user agent + touch (iPads in desktop mode included) | `uiType`, else 600 dp and wider is a tablet | desktop | desktop |
| `os`, `model`, `model_name` | `iOS 18.2`, `iPhone17,1`, `iPhone 16 Pro` | `macOS 15.1`, `Mac15,6` | the visitor's OS, Android model, client hints | `Android 16`, `Build.MODEL`, maker and model | `/etc/os-release` version, no model | `RtlGetVersion`, no model |
| `screen` (pt, scale, inches, ppi, Hz, cutout) | screen pixels ÷ the model's ppi (table generated from Xcode's simulator profiles: `scripts/apple-models.py`) | `CGDisplayScreenSize` | estimated inches | real pixels ÷ `xdpi`, `ydpi` | the monitor (GDK: size in mm, scale, Hz) | not yet: the window's starting size and scale |
| `orientation`, `rotation` | interface orientation | screen shape | `screen.orientation` | screen shape | monitor shape | window shape |
| `posture`, `hinge_angle`, `fold` (Flat, Book, Tabletop, Closed) | `UIHingeInteraction` + division reserved regions (iOS 27.1+, iPhone Duo) | – | `devicePosture`, viewport segments | – | – | – |
| `window_mode` (fullscreen, split, slide over, windowed) | Split View / Stage Manager | fullscreen or windowed | installed app or browser tab | split in multi-window, else fullscreen | windowed | windowed |
| `browser` | `None` | `None` | always set | `None` | `None` | `None` |

On the web the server reads the `User-Agent` for the first HTML, then the page reports touch, screen, orientation, posture and client hints (and remembers them in a cookie for the next visit). Android reads them from its Java host, Linux from GDK.
