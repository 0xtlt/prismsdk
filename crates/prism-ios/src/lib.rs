//! # prism-ios
//!
//! UIKit backend: every Prism node is a real `UIView` (`UILabel`, `UIButton`,
//! `UITextField`), laid out by `prism-native`. Uses the UIScene lifecycle, so
//! the app bundle's Info.plist must declare `PrismSceneDelegate` (see
//! `scripts/ios-run.sh`).

#[cfg(any(target_os = "ios", target_os = "tvos"))]
mod automation;
#[cfg(any(target_os = "ios", target_os = "tvos"))]
mod device_info;
#[cfg(any(target_os = "ios", target_os = "tvos"))]
mod imp;
#[cfg(any(target_os = "ios", target_os = "tvos"))]
mod menus;
#[cfg(any(target_os = "ios", target_os = "tvos"))]
mod services;
#[cfg(target_os = "tvos")]
mod tv;

#[cfg(any(target_os = "ios", target_os = "tvos"))]
pub use imp::run;
