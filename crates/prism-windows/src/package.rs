//! What an app shipped by `prism build windows` brings with it: the icon
//! in its executable (`prism_build::link_app()` embeds it as icon group 1)
//! and its `assets` folder next to it.

use std::path::PathBuf;

use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, HICON, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON,
    SYSTEM_METRICS_INDEX,
};
use windows::core::PCWSTR;

/// The executable's icon (resource 1) at a system size, if it has one.
fn load(instance: HINSTANCE, metric: SYSTEM_METRICS_INDEX) -> HICON {
    unsafe {
        let size = GetSystemMetrics(metric);
        // MAKEINTRESOURCE(1): the id as a pointer.
        LoadImageW(
            Some(instance),
            PCWSTR(std::ptr::without_provenance(1)),
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        )
        .map(|h| HICON(h.0))
        .unwrap_or_default()
    }
}

/// The window class's large and small icons (Alt+Tab, the taskbar, the
/// title bar): the app's, or Windows' default when it has none.
pub fn class_icons(instance: HINSTANCE) -> (HICON, HICON) {
    (load(instance, SM_CXICON), load(instance, SM_CXSMICON))
}

/// The app's assets: `PRISM_ASSETS_DIR`, else an `assets` folder next to
/// the executable (a shipped app), else the directory the app configures
/// (the project's own, for `cargo run`).
pub fn assets_dir(configured: Option<&str>) -> Option<PathBuf> {
    if let Some(dir) = crate::win::env("PRISM_ASSETS_DIR") {
        return Some(dir.into());
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("assets")))
        .filter(|dir| dir.is_dir());
    beside.or_else(|| configured.map(PathBuf::from))
}
