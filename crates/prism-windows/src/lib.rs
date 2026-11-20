//! # prism-windows
//!
//! WinUI 3 backend. One Win32 window per app window, whose client area is
//! a XAML island (`DesktopWindowXamlSource`) of the Windows App SDK,
//! self-contained: the SDK's runtime goes next to the app (see
//! `build.rs`), nothing is installed. Every Prism node becomes XAML
//! elements: real WinUI controls (`Button`, `TextBox`, `ToggleSwitch`,
//! `Slider`, `ComboBox`, `CalendarDatePicker`, `SelectorBar`…) and text
//! (`TextBlock`), laid out with prism-native's flexbox like the other
//! native backends and placed in `Canvas`es; boxes, gradients, borders,
//! shadows and transforms map to XAML brushes, `Border`s and Composition.
//! WinUI's own UI Automation peers expose the tree to screen readers.
//!
//! WinUI renders through DWM, so only in a desktop session: over SSH
//! (session 0) the tree is built and laid out but nothing shows.
//!
//! Modules:
//!
//! | | |
//! |---|---|
//! | `imp` | `run`, the window, the message loop, dispatch / commit |
//! | `host` | starting WinUI in a Win32 process; the XAML island |
//! | `outer` | the `Application`, aggregated so the control styles load |
//! | `winui` | the generated WinUI projection, conversions |
//! | `widgets` | `prism_native::Widgets` over XAML |
//! | `view` | one node's XAML elements: box, children, content, bars |
//! | `style` | backgrounds, gradients, borders, radii, shadows, transforms |
//! | `text` | `TextBlock`s from `StyledText`; measuring |
//! | `controls` | Prism's controls as WinUI controls, their events and sizes |
//! | `bars` | a stack's header, a tab bar |
//! | `events` | clicks, hover and press, focus, scrolling, menus |
//! | `keyboard` | key presses for Prism's key listeners |
//! | `smoke` | `PRISM_CLICK`, `PRISM_SNAPSHOT`… |
//! | `capture`, `wic` | `PrintWindow` and WIC: snapshots, pictures |
//! | `images` | decoding pictures |
//! | `services` | `PlatformServices`, routing to the modules below |
//! | `pickers` | file, folder and photo pickers, the save dialog |
//! | `notify`, `toast` | app notifications (Windows App SDK) |
//! | `taskbar` | the badge on the taskbar button |
//! | `security` | Credential Manager, Windows Hello, loopback sign-in |
//! | `tray`, `icons` | notification area icons and their menus |
//! | `menubar`, `accel` | the window's menu bar, keyboard accelerators |
//! | `alerts` | `ContentDialog` alerts |
//! | `clipboard` | the clipboard in all its types |
//! | `fonts` | run-time fonts |
//! | `shell` | the services' hidden window: modal UI, timers, threads |
//! | `system` | theme, accent, locale, DPI |
//! | `scene`, `keys`, `fluent`, `toast`, `accel` | pure helpers, tested on every host |

// Menu shortcuts as accelerators: no Win32, tested everywhere.
mod accel;
#[cfg(target_os = "windows")]
mod alerts;
#[cfg(target_os = "windows")]
mod bars;
#[cfg(target_os = "windows")]
mod capture;
#[cfg(target_os = "windows")]
mod clipboard;
#[cfg(target_os = "windows")]
mod controls;
#[cfg(target_os = "windows")]
mod events;
// Theme colors and control metrics: no Win32, tested everywhere.
mod fluent;
#[cfg(target_os = "windows")]
mod fonts;
#[cfg(target_os = "windows")]
mod host;
#[cfg(target_os = "windows")]
mod icons;
#[cfg(target_os = "windows")]
mod images;
#[cfg(target_os = "windows")]
mod imp;
#[cfg(target_os = "windows")]
mod keyboard;
// Key names and icon glyphs: no Win32, tested everywhere.
mod keys;
#[cfg(target_os = "windows")]
mod menubar;
#[cfg(target_os = "windows")]
mod notify;
#[cfg(target_os = "windows")]
mod outer;
#[cfg(target_os = "windows")]
mod pickers;
// The tree's model: no Win32, tested everywhere.
mod scene;
#[cfg(target_os = "windows")]
mod security;
#[cfg(target_os = "windows")]
mod services;
#[cfg(target_os = "windows")]
mod shell;
#[cfg(target_os = "windows")]
mod smoke;
#[cfg(target_os = "windows")]
mod style;
#[cfg(target_os = "windows")]
mod system;
#[cfg(target_os = "windows")]
mod taskbar;
#[cfg(target_os = "windows")]
mod text;
// Notifications as toast XML: no Win32, tested everywhere.
mod toast;
#[cfg(target_os = "windows")]
mod tray;
#[cfg(target_os = "windows")]
mod view;
#[cfg(target_os = "windows")]
mod wic;
#[cfg(target_os = "windows")]
mod widgets;
#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
mod winui;

#[cfg(target_os = "windows")]
pub use imp::run;
