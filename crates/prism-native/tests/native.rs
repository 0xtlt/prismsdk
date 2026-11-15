//! prism-native through a recording fake `Widgets`: layout, motion, patch
//! routing, and the JSON ops protocol of remote hosts (watchOS, Android).
//!
//! One test binary (submodules in `tests/native/`) keeps link time down.

#[path = "native/common.rs"]
mod common;
#[path = "native/layout.rs"]
mod layout;
#[path = "native/motion.rs"]
mod motion;
#[path = "native/patches.rs"]
mod patches;
#[path = "native/remote_ops.rs"]
mod remote_ops;
#[path = "native/states.rs"]
mod states;
