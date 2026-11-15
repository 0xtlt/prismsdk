//! # prism-ios
//!
//! UIKit backend: every Prism node is a real `UIView` (`UILabel`, `UIButton`,
//! `UITextField`), laid out by `prism-native`. Uses the UIScene lifecycle, so
//! the app bundle's Info.plist must declare `PrismSceneDelegate` (see
//! `platform/scripts/ios-run.sh` in `prism-cli`).

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
mod automation;
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
mod device_info;
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
mod forms;
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
mod imp;
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
mod layout;
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
mod menus;
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
mod services;
#[cfg(target_os = "tvos")]
mod tv;

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
pub use imp::run;
