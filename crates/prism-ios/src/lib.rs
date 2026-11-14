//! # prism-ios
//!
//! UIKit backend: every Prism node is a real `UIView` (`UILabel`, `UIButton`,
//! `UITextField`), laid out by `prism-native`. Uses the UIScene lifecycle, so
//! the app bundle's Info.plist must declare `PrismSceneDelegate` (see
//! `scripts/ios-run.sh`).

#[cfg(target_os = "ios")]
mod imp;
#[cfg(target_os = "ios")]
mod menus;
#[cfg(target_os = "ios")]
mod services;

#[cfg(target_os = "ios")]
pub use imp::run;
