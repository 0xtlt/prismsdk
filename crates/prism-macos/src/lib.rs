//! # prism-macos
//!
//! AppKit backend: every Prism node is a real `NSView` (labels are
//! `NSTextField`s, buttons are `NSButton`s), laid out by `prism-native`.

#[cfg(target_os = "macos")]
mod a11y_dump;
#[cfg(target_os = "macos")]
mod decor;
#[cfg(target_os = "macos")]
mod device_info;
#[cfg(target_os = "macos")]
mod imp;
#[cfg(target_os = "macos")]
mod interact;
#[cfg(target_os = "macos")]
mod label;
#[cfg(target_os = "macos")]
mod menus;
#[cfg(target_os = "macos")]
mod services;
#[cfg(target_os = "macos")]
mod sticky;

#[cfg(target_os = "macos")]
pub use imp::run;
