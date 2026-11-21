//! The app's lifecycle phase from its window (`cx.on_lifecycle`), as the
//! other desktops report it: active while the window is the active one,
//! inactive while another window is (the app switcher, a dialog of
//! another app), background while it is minimized and as it closes (the
//! last chance to save). No Win32 types: tested on every host.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::Lifecycle;

/// The phase of a window that is `minimized` (iconic), the `active` one,
/// or `closing`.
pub fn phase(minimized: bool, active: bool, closing: bool) -> Lifecycle {
    if closing || minimized {
        Lifecycle::Background
    } else if active {
        Lifecycle::Active
    } else {
        Lifecycle::Inactive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_gives_the_phase() {
        assert_eq!(phase(false, true, false), Lifecycle::Active);
        assert_eq!(phase(false, false, false), Lifecycle::Inactive);
        // Minimized: background, though Windows may still call it active.
        assert_eq!(phase(true, true, false), Lifecycle::Background);
        assert_eq!(phase(true, false, false), Lifecycle::Background);
        assert_eq!(phase(false, true, true), Lifecycle::Background);
    }
}
