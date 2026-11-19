//! # prism-linux
//!
//! GTK 4 backend: every Prism node is a real GTK widget (`GtkLabel`,
//! `GtkButton`, `GtkEntry`, `GtkSwitch`…), placed by prism-native's flexbox
//! layout like on macOS and iOS. A small layout manager puts each widget at
//! the frame Prism computed, so layouts match the other platforms.
//!
//! Tested in Docker (`docker/linux-dev.Dockerfile`, Xvfb).

#[cfg(target_os = "linux")]
mod across;
#[cfg(target_os = "linux")]
mod carets;
#[cfg(target_os = "linux")]
mod controls;
#[cfg(target_os = "linux")]
mod decor;
#[cfg(target_os = "linux")]
mod edit;
#[cfg(target_os = "linux")]
mod focal;
#[cfg(target_os = "linux")]
mod imp;
// Key and icon names: no GTK, tested everywhere.
mod input;
#[cfg(target_os = "linux")]
mod layout;
#[cfg(target_os = "linux")]
mod navbar;
#[cfg(target_os = "linux")]
mod padding;
#[cfg(target_os = "linux")]
mod push;
#[cfg(target_os = "linux")]
mod scene3d;
#[cfg(target_os = "linux")]
mod security;
#[cfg(target_os = "linux")]
mod services;
// The nodes' shared CSS rules: no GTK, tested everywhere.
mod sheet;
#[cfg(target_os = "linux")]
mod text;
#[cfg(target_os = "linux")]
mod tray;

#[cfg(target_os = "linux")]
pub use imp::run;
