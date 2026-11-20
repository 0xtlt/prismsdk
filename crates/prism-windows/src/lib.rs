//! # prism-windows
//!
//! Win32 + Direct2D/DirectWrite backend. One window (`HWND`) per app
//! window; every Prism node is a node of a retained scene the backend draws
//! itself with Direct2D, in the Windows 11 (Fluent) look, and lays out with
//! prism-native's flexbox like the other native backends. Text fields are
//! RichEdit child windows placed over their node (IME, selection, undo,
//! clipboard). UI Automation exposes the scene to screen readers.
//!
//! The same drawing code renders on screen (a DXGI swap chain, or GDI when
//! there is no GPU presentation) and offscreen into a WIC bitmap, which
//! serves `PRISM_SNAPSHOT` over SSH (session 0, no desktop compositor).
//!
//! Modules:
//!
//! | | |
//! |---|---|
//! | `imp` | `run`, the window, the message loop, dispatch / commit, smoke hooks |
//! | `scene` | the retained tree: frames, hit testing, focus order, label search |
//! | `widgets` | `prism_native::Widgets` over the scene |
//! | `gfx` | Direct2D, DirectWrite and WIC factories, the window's surface, PNG output |
//! | `paint` | draws the scene: backgrounds, gradients, borders, shadows, clips, transforms |
//! | `text` | DirectWrite layouts of `StyledText`: measure, baseline, draw |
//! | `controls` | Fluent drawing and metrics of buttons, toggles, sliders, bars… |
//! | `fields` | RichEdit text fields |
//! | `images` | WIC decoding of pictures |
//! | `input` | pointer, wheel, keys, focus |
//! | `uia` | UI Automation provider |
//! | `services` | `PlatformServices` |
//! | `system` | theme, accent, locale, DPI |
//! | `scene`, `keys`, `fluent` | pure helpers, tested on every host |

#[cfg(target_os = "windows")]
mod controls;
#[cfg(target_os = "windows")]
mod fields;
// Theme colors and control metrics: no Win32, tested everywhere.
mod fluent;
#[cfg(target_os = "windows")]
mod gfx;
#[cfg(target_os = "windows")]
mod images;
#[cfg(target_os = "windows")]
mod imp;
#[cfg(target_os = "windows")]
mod input;
// Key names and icon glyphs: no Win32, tested everywhere.
mod keys;
#[cfg(target_os = "windows")]
mod paint;
// The retained tree: no Win32, tested everywhere.
mod scene;
#[cfg(target_os = "windows")]
mod services;
#[cfg(target_os = "windows")]
mod smoke;
#[cfg(target_os = "windows")]
mod system;
#[cfg(target_os = "windows")]
mod text;
#[cfg(target_os = "windows")]
mod uia;
#[cfg(target_os = "windows")]
mod widgets;
#[cfg(target_os = "windows")]
mod win;

#[cfg(target_os = "windows")]
pub use imp::run;
