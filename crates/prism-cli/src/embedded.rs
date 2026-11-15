//! Bundling scripts and platform sources (Apple, Android), embedded so `prism` works in
//! any project. Extracted under `<target>/prism/platform` when they change.

use std::path::{Path, PathBuf};

macro_rules! files {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!("../../../", $path)))),*]
    };
}

const FILES: &[(&str, &str)] = files![
    "scripts/ios-run.sh",
    "scripts/macos-app.sh",
    "scripts/android-run.sh",
    "scripts/android-deps.py",
    "platform/apple/app-icon.sh",
    "platform/apple/generate_widgets.py",
    "platform/apple/Bridge/PrismBridge.swift",
    "platform/apple/Extension/PrismRenderer.swift",
    "platform/apple/Extension/PrismWidgetKit.swift",
    "platform/apple/Shared/PrismActivityAttributes.swift",
    "platform/android/java/dev/prism/PrismActivity.java",
    "platform/android/java/dev/prism/PrismDevice.java",
    "platform/android/java/dev/prism/PrismFiles.java",
    "platform/android/java/dev/prism/PrismHost.java",
    "platform/android/java/dev/prism/PrismJob.java",
    "platform/android/java/dev/prism/PrismLayout.java",
    "platform/android/java/dev/prism/PrismMotion.java",
    "platform/android/java/dev/prism/PrismShape.java",
    "platform/android/java/dev/prism/PrismSurface.java",
    "platform/android/java/dev/prism/PrismWidget.java",
    "platform/android/java/dev/prism/PrismSecurity.java",
    "platform/android/java/dev/prism/PrismLinkActivity.java",
    "platform/android/java/dev/prism/PrismBackup.java",
    "platform/android/car/dev/prism/PrismCarService.java",
];

/// Writes the files (if needed) and returns the platform directory, laid out
/// as `scripts/…`, `apple/…` and `android/…`.
pub fn extract(target_dir: &Path) -> Result<PathBuf, String> {
    let dir = target_dir.join("prism").join("platform");
    for (path, contents) in FILES {
        let rel = path.strip_prefix("platform/").unwrap_or(path);
        let dest = dir.join(rel);
        if std::fs::read_to_string(&dest).ok().as_deref() == Some(*contents) {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&dest, contents).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    Ok(dir)
}
