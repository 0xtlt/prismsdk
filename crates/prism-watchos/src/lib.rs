//! # prism-watchos
//!
//! watchOS apps have no UIKit views (`UIView` is `API_UNAVAILABLE(watchos)`
//! in the SDK), so this backend keeps the app in Rust and lets a SwiftUI
//! host (`platform/apple/Watch/PrismWatch.swift`) draw the tree. Layout is
//! prism-native's flexbox (taffy), like on iOS and macOS: Swift measures
//! text and draws each node at its frame; events come back as JSON.
//!
//! Prism's styles are phone-sized: the host zooms them by the watch's body
//! text size over 17 pt, and the app sees its viewport in Prism points.

#[cfg(target_os = "watchos")]
mod imp;
#[cfg(target_os = "watchos")]
mod services;

#[cfg(target_os = "watchos")]
pub use imp::run;
