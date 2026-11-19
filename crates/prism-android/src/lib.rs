//! # prism-android
//!
//! Android backend. The app runs in Rust (a `cdylib` loaded by the Java host
//! in `prism-cli`, `platform/android`); layout is prism-native's flexbox, like on iOS and
//! macOS. The Java host creates real Android views (`TextView`, `Button`,
//! `EditText`, `Switch`, `ProgressBar`, `ImageView`, `ScrollView`…), measures
//! text, and places each view at the frame Prism computed.
//!
//! Prism points are Android dp: the renderer zooms by the screen density, so
//! frames and text sizes reach the host in pixels.
//!
//! The JNI surface is a few calls (raw `jni-sys`, the C API of the JNI
//! specification):
//!
//! | direction | call |
//! |---|---|
//! | Java → Rust | `PrismHost.nativeMain()` (in the `prism` facade), `nativeReady(w, h, density, info)`, `nativeEvent(json)`, `nativeWake()`, `nativeConsumesKey(json)` and `nativeTakesText(id)` (a selection group's keys and typing, #231), `sceneBlob(id)` (3D scenes' blobs, any thread); without a screen too: `nativeBackground(task, info)`, `nativeBackgroundExpired(task)`, `nativeWidgetAction(…)`, `nativeCarEvent(kind, value, info)`, `nativePush(json, info)` |
//! | Rust → Java | `PrismHost.apply(json)`, `measure(kind, text, size, weight, maxWidth, spec)` (answers are cached by request), `post(delayMs)`, `jobFinished(task, ok)`, `cloudChanged()` |

#[cfg(target_os = "android")]
mod imp;
#[cfg(target_os = "android")]
mod jni;
#[cfg(target_os = "android")]
mod services;

#[cfg(target_os = "android")]
pub use imp::run;
