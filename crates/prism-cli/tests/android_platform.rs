//! `android-run.sh` builds against the newest stable Android platform of the
//! SDK, never a preview: a preview's APKs install only on that preview
//! (`INSTALL_FAILED_OLDER_SDK: Requires development platform ...`). The
//! target SDK is the platform's API level, not the end of its folder name
//! (`android-36-ext17`, `android-37.0-beta3`...).
//!
//! Runs the script's platform section on stand-in SDKs.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

#[path = "../src/temp_dir.rs"]
mod temp_dir;
use temp_dir::TempDir;

/// The platform section of the script, from `PLATFORM_DIR=""` to the
/// closing brace of its error.
fn section() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("platform/scripts/android-run.sh");
    let script = std::fs::read_to_string(path).unwrap();
    let lines: Vec<&str> = script.lines().collect();
    let start = lines
        .iter()
        .position(|l| *l == "PLATFORM_DIR=\"\"")
        .expect("PLATFORM_DIR");
    let end = start + lines[start..].iter().position(|l| *l == "}").expect("}");
    lines[start..=end].join("\n")
}

/// Platforms as `(folder, api level, codename)`.
fn sdk(dir: &Path, platforms: &[(&str, &str, &str)]) {
    for (name, api, codename) in platforms {
        let p = dir.join("platforms").join(name);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("android.jar"), "").unwrap();
        std::fs::write(
            p.join("source.properties"),
            format!(
                "Pkg.Revision=1\nPlatform.CodeName={codename}\nAndroidVersion.ApiLevel={api}\n"
            ),
        )
        .unwrap();
    }
}

/// `(status, stdout, stderr)` of the section with `ANDROID_HOME=dir`.
fn pick(dir: &Path) -> (bool, String, String) {
    let harness = format!(
        "set -euo pipefail\nANDROID_HOME=\"{}\"\n{}\necho \"$(basename \"$PLATFORM_DIR\") $TARGET_SDK\"\n",
        dir.display(),
        section()
    );
    let out = Command::new("bash")
        .arg("-c")
        .arg(harness)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn a_preview_platform_is_skipped() {
    let dir = TempDir::new("cli-android-platform");
    sdk(
        &dir,
        &[
            ("android-34", "34", ""),
            ("android-36-ext17", "36", ""),
            ("android-37.0-beta3", "37", "Baklava"),
        ],
    );
    let (ok, out, err) = pick(&dir);
    assert!(ok, "{err}");
    assert_eq!(out, "android-36-ext17 36");
}

#[test]
fn the_newest_stable_platform_wins() {
    let dir = TempDir::new("cli-android-platform");
    sdk(
        &dir,
        &[
            ("android-9", "28", ""),
            ("android-35", "35", ""),
            ("android-36", "36", ""),
        ],
    );
    let (ok, out, err) = pick(&dir);
    assert!(ok, "{err}");
    assert_eq!(out, "android-36 36");
}

#[test]
fn only_previews_is_an_error_that_names_the_fix() {
    let dir = TempDir::new("cli-android-platform");
    sdk(&dir, &[("android-CANARY", "37", "CANARY")]);
    let (ok, _, err) = pick(&dir);
    assert!(!ok);
    assert!(err.contains("no stable Android platform"), "{err}");
    assert!(err.contains("sdkmanager"), "{err}");
}
