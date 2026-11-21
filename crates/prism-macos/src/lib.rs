//! # prism-macos
//!
//! AppKit backend: every Prism node is a real `NSView` (labels are
//! `NSTextField`s, buttons are `NSButton`s), laid out by `prism-native`.

#[cfg(target_os = "macos")]
mod a11y_dump;
#[cfg(target_os = "macos")]
mod across;
#[cfg(target_os = "macos")]
mod carets;
#[cfg(target_os = "macos")]
mod chrome;
#[cfg(target_os = "macos")]
mod dates;
#[cfg(target_os = "macos")]
mod decor;
#[cfg(target_os = "macos")]
mod device_info;
#[cfg(target_os = "macos")]
mod edit;
#[cfg(target_os = "macos")]
mod imp;
#[cfg(target_os = "macos")]
mod interact;
#[cfg(target_os = "macos")]
mod label;
#[cfg(target_os = "macos")]
mod menus;
#[cfg(target_os = "macos")]
mod navbar;
#[cfg(target_os = "macos")]
mod padding;
#[cfg(target_os = "macos")]
mod refresh;
#[cfg(target_os = "macos")]
mod rich;
#[cfg(target_os = "macos")]
mod scene;
#[cfg(target_os = "macos")]
mod services;
#[cfg(target_os = "macos")]
mod sticky;
#[cfg(target_os = "macos")]
mod table;
#[cfg(all(test, target_os = "macos"))]
mod temp_dir;
#[cfg(target_os = "macos")]
mod trace;
#[cfg(target_os = "macos")]
mod widget_key;

#[cfg(target_os = "macos")]
pub use imp::run;
