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
//! | `services` | `PlatformServices` |
//! | `system` | theme, accent, locale, DPI |
//! | `scene`, `keys`, `fluent` | pure helpers, tested on every host |

// `cx.show_alert` as a ContentDialog.
#[cfg(target_os = "windows")]
mod alert;
#[cfg(target_os = "windows")]
mod bars;
#[cfg(target_os = "windows")]
mod capture;
#[cfg(target_os = "windows")]
mod controls;
#[cfg(target_os = "windows")]
mod events;
// Theme colors and control metrics: no Win32, tested everywhere.
mod fluent;
#[cfg(target_os = "windows")]
mod host;
#[cfg(target_os = "windows")]
mod images;
#[cfg(target_os = "windows")]
mod imp;
#[cfg(target_os = "windows")]
mod keyboard;
// Animations: Composition on the elements, frames per rendered frame.
#[cfg(target_os = "windows")]
mod motion;
// Key names and icon glyphs: no Win32, tested everywhere.
mod keys;
// Sheets and dialogs as ContentDialog-like panels, popovers as flyouts.
#[cfg(target_os = "windows")]
mod outer;
#[cfg(target_os = "windows")]
mod overlay;
// Inset shadows, borders of several colors, conic gradients, rounded clips.
#[cfg(target_os = "windows")]
mod decor;
// Conic gradient pixels, per-side border outlines: no Win32, tested everywhere.
mod paint;
// The tree's model: no Win32, tested everywhere.
mod scene;
#[cfg(target_os = "windows")]
mod services;
#[cfg(target_os = "windows")]
mod smoke;
#[cfg(target_os = "windows")]
mod style;
#[cfg(target_os = "windows")]
mod system;
#[cfg(target_os = "windows")]
mod text;
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
