//! #318: the privacy manifest (`PrivacyInfo.xcprivacy`) the Apple bundlers
//! write into each bundle, and the Info.plist texts of `[permissions]`.
//! Runs `apple-privacy.py` with Python on binaries made of the names it
//! looks for.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

#[path = "../src/temp_dir.rs"]
mod temp_dir;
use temp_dir::TempDir;

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/platform/scripts/apple-privacy.py"
);

/// A "binary" holding `names` as a Mach-O string table does: each one
/// between NUL bytes.
fn binary(dir: &Path, file: &str, names: &[&str]) -> String {
    let mut bytes = b"\xcf\xfa\xed\xfe\0".to_vec();
    for n in names {
        bytes.extend_from_slice(n.as_bytes());
        bytes.push(0);
    }
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(file);
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().into_owned()
}

fn run(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new("python3");
    cmd.arg(SCRIPT)
        .args(args)
        .env_remove("PRISM_PERMISSIONS")
        .env_remove("PRISM_PRIVACY");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("python3")
}

/// A manifest as JSON (plistlib reads what plistlib wrote).
fn manifest(dir: &Path) -> serde_json::Value {
    let out = Command::new("python3")
        .args([
            "-c",
            "import json, plistlib, sys; print(json.dumps(plistlib.load(open(sys.argv[1], 'rb'))))",
        ])
        .arg(dir.join("PrivacyInfo.xcprivacy"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn reasons(m: &serde_json::Value) -> Vec<(String, Vec<String>)> {
    m["NSPrivacyAccessedAPITypes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["NSPrivacyAccessedAPIType"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("NSPrivacyAccessedAPICategory")
                    .to_string(),
                a["NSPrivacyAccessedAPITypeReasons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Prism's own uses get Prism's reasons, in the app and the widget
/// extension: what a Prism app references (#318).
#[test]
fn declares_prisms_own_required_reason_apis() {
    let dir = TempDir::new("privacy-prism");
    let (app, appex) = (
        dir.join("App.app"),
        dir.join("App.app/PlugIns/PrismWidgets.appex"),
    );
    let bin = binary(
        &dir,
        "app",
        &[
            "_fstat",
            "_stat$INODE64",
            "systemUptime",
            "_fstatfs",
            "_lstatx",
        ],
    );
    let ext = binary(&dir, "ext", &["_mach_absolute_time"]);
    let out = run(
        &[
            "manifest",
            "ios",
            "1",
            &app.to_string_lossy(),
            &bin,
            &appex.to_string_lossy(),
            &ext,
        ],
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let m = manifest(&app);
    assert_eq!(
        reasons(&m),
        [
            ("DiskSpace".to_string(), vec!["E174.1".to_string()]),
            ("FileTimestamp".into(), vec!["C617.1".into()]),
            ("SystemBootTime".into(), vec!["35F9.1".into()]),
        ]
    );
    assert_eq!(m["NSPrivacyTracking"], false);
    assert_eq!(m["NSPrivacyCollectedDataTypes"], serde_json::json!([]));
    assert_eq!(
        reasons(&manifest(&appex)),
        [("SystemBootTime".to_string(), vec!["35F9.1".to_string()])]
    );
    // Nothing referenced: an empty manifest, still there.
    let none = binary(&dir, "none", &["_open"]);
    let out = run(
        &[
            "manifest",
            "macos",
            "0",
            &dir.join("Mac").to_string_lossy(),
            &none,
        ],
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(reasons(&manifest(&dir.join("Mac"))).is_empty());
}

/// The app's own categories need its reasons: App Store builds for iOS stop
/// without them, other builds warn.
#[test]
fn the_app_gives_reasons_for_its_own_apis() {
    let dir = TempDir::new("privacy-app");
    let app = dir.join("App.app").to_string_lossy().into_owned();
    let bin = binary(
        &dir,
        "app",
        &["_OBJC_CLASS_$_NSUserDefaults", "activeInputModes"],
    );
    let out = run(&["manifest", "ios", "1", &app, &bin], &[]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(
        err.contains("UserDefaults") && err.contains("user_defaults = ["),
        "{err}"
    );
    assert!(err.contains("ActiveKeyboards"), "{err}");
    let out = run(&["manifest", "ios", "0", &app, &bin], &[]);
    assert!(
        out.status.success() && stderr(&out).contains("warning: "),
        "{}",
        stderr(&out)
    );

    // [privacy] (PRISM_PRIVACY) and the project's manifest, merged.
    let own = dir.join("PrivacyInfo.xcprivacy");
    std::fs::write(
        &own,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>NSPrivacyAccessedAPITypes</key><array><dict>
    <key>NSPrivacyAccessedAPIType</key><string>NSPrivacyAccessedAPICategoryActiveKeyboards</string>
    <key>NSPrivacyAccessedAPITypeReasons</key><array><string>3EC4.1</string></array>
  </dict></array>
  <key>NSPrivacyCollectedDataTypes</key><array><dict>
    <key>NSPrivacyCollectedDataType</key><string>NSPrivacyCollectedDataTypeEmailAddress</string>
    <key>NSPrivacyCollectedDataTypeLinked</key><false/>
    <key>NSPrivacyCollectedDataTypeTracking</key><false/>
    <key>NSPrivacyCollectedDataTypePurposes</key><array><string>NSPrivacyCollectedDataTypePurposeAnalytics</string></array>
  </dict></array>
  <key>MyOwnKey</key><string>kept</string>
</dict></plist>"#,
    )
    .unwrap();
    let privacy = serde_json::json!({
        "tracking": false,
        "tracking_domains": [],
        "apis": [{"category": "NSPrivacyAccessedAPICategoryUserDefaults", "reasons": ["CA92.1"]}],
        "collected": [{
            "type": "NSPrivacyCollectedDataTypeEmailAddress",
            "linked": true,
            "tracking": false,
            "purposes": ["NSPrivacyCollectedDataTypePurposeAppFunctionality"],
        }],
        "manifest": own.to_string_lossy(),
    })
    .to_string();
    let out = run(
        &["manifest", "ios", "1", &app, &bin],
        &[("PRISM_PRIVACY", &privacy)],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let m = manifest(&dir.join("App.app"));
    assert_eq!(
        reasons(&m),
        [
            ("ActiveKeyboards".to_string(), vec!["3EC4.1".to_string()]),
            ("UserDefaults".into(), vec!["CA92.1".into()]),
        ]
    );
    assert_eq!(m["MyOwnKey"], "kept");
    assert_eq!(
        m["NSPrivacyCollectedDataTypes"],
        serde_json::json!([{
            "NSPrivacyCollectedDataType": "NSPrivacyCollectedDataTypeEmailAddress",
            "NSPrivacyCollectedDataTypeLinked": true,
            "NSPrivacyCollectedDataTypeTracking": false,
            "NSPrivacyCollectedDataTypePurposes": [
                "NSPrivacyCollectedDataTypePurposeAnalytics",
                "NSPrivacyCollectedDataTypePurposeAppFunctionality"
            ],
        }])
    );
}

/// Tracking needs its domains and the App Tracking Transparency text.
#[test]
fn tracking_needs_domains_and_its_prompt() {
    let dir = TempDir::new("privacy-tracking");
    let app = dir.join("App.app").to_string_lossy().into_owned();
    let bin = binary(&dir, "app", &[]);
    let privacy = r#"{"tracking": true, "tracking_domains": [], "apis": [], "collected": [], "manifest": null}"#;
    let out = run(
        &["manifest", "ios", "1", &app, &bin],
        &[("PRISM_PRIVACY", privacy)],
    );
    let err = stderr(&out);
    assert!(!out.status.success());
    assert!(
        err.contains("NSPrivacyTrackingDomains") && err.contains("NSUserTrackingUsageDescription"),
        "{err}"
    );
    let privacy = r#"{"tracking": true, "tracking_domains": ["ads.example.com"], "apis": [], "collected": [], "manifest": null}"#;
    let out = run(
        &["manifest", "ios", "1", &app, &bin],
        &[
            ("PRISM_PRIVACY", privacy),
            ("PRISM_PERMISSIONS", "tracking\tPersonalized ads."),
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let m = manifest(&dir.join("App.app"));
    assert_eq!(m["NSPrivacyTracking"], true);
    assert_eq!(
        m["NSPrivacyTrackingDomains"],
        serde_json::json!(["ads.example.com"])
    );
}

/// Protected APIs in the binary need their purpose string (ITMS-90683):
/// code a crate brings, which the app's [permissions] doesn't declare.
#[test]
fn protected_apis_need_their_purpose_string() {
    let dir = TempDir::new("privacy-protected");
    let app = dir.join("App.app").to_string_lossy().into_owned();
    let bin = binary(
        &dir,
        "app",
        &[
            "_OBJC_CLASS_$_CLLocationManager",
            "_OBJC_CLASS_$_EKEventStore",
            "_OBJC_CLASS_$_LAContext",
        ],
    );
    let out = run(&["manifest", "ios", "1", &app, &bin], &[]);
    let err = stderr(&out);
    assert!(!out.status.success());
    assert!(
        err.contains("`location = ")
            && err.contains("`calendars = ")
            && err.contains("`face_id = "),
        "{err}"
    );
    // Declared (reminders covers EKEventStore too): it passes.
    let declared = "location\tMaps.\nreminders\tLists.\nface_id\tUnlocks.";
    let out = run(
        &["manifest", "ios", "1", &app, &bin],
        &[("PRISM_PERMISSIONS", declared)],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    // Touch ID asks no text: on macOS, LAContext is fine.
    let out = run(
        &["manifest", "macos", "0", &app, &bin],
        &[("PRISM_PERMISSIONS", "location\tMaps.\nreminders\tLists.")],
    );
    assert!(
        out.status.success() && stderr(&out).is_empty(),
        "{}",
        stderr(&out)
    );
}

#[test]
fn writes_the_usage_descriptions() {
    let out = run(
        &["usage"],
        &[(
            "PRISM_PERMISSIONS",
            "camera\tScans <receipts>.\nbluetooth\tFinds the band.",
        )],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "  <key>NSCameraUsageDescription</key><string>Scans &lt;receipts&gt;.</string>\n\
         \x20 <key>NSBluetoothAlwaysUsageDescription</key><string>Finds the band.</string>\n\
         \x20 <key>NSBluetoothPeripheralUsageDescription</key><string>Finds the band.</string>\n"
    );
}
