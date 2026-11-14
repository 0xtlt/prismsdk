//! # prism-macos
//!
//! AppKit backend: every Prism node is a real `NSView` (labels are
//! `NSTextField`s, buttons are `NSButton`s), laid out by `prism-native`.

#[cfg(target_os = "macos")]
mod imp;

#[cfg(target_os = "macos")]
pub use imp::run;
