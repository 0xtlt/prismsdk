//! `platform/scripts/profile-entitlements.py` (ios-run.sh): the entitlements
//! a provisioning profile grants, and the capability notes, over sample
//! profiles (the plists `security cms -D` prints).

use std::path::{Path, PathBuf};
use std::process::Command;

const TEAM: &str = "PRISMTEST1";
const BUNDLE: &str = "dev.prism.counter";

/// A decoded development profile for `app_id` with these extra entitlements.
fn profile(dir: &Path, name: &str, app_id: &str, extra: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Name</key><string>Prism Test {app_id}</string>
  <key>TeamIdentifier</key><array><string>{TEAM}</string></array>
  <key>Entitlements</key><dict>
    <key>application-identifier</key><string>{TEAM}.{app_id}</string>
    <key>com.apple.developer.team-identifier</key><string>{TEAM}</string>
    <key>keychain-access-groups</key><array><string>{TEAM}.*</string></array>
    <key>get-task-allow</key><true/>
    {extra}
  </dict>
</dict></plist>"#
        ),
    )
    .unwrap();
    path
}

const GROUP: &str = "<key>com.apple.security.application-groups</key><array><string>group.dev.prism.counter</string></array>";
const PUSH: &str = "<key>aps-environment</key><string>development</string>";

struct Run {
    notes: String,
    entitlements: String,
}

/// Runs the script for one bundle; `None` without python3.
fn run(dir: &Path, profile: &Path, bundle: &str, role: &str, env: &[(&str, &str)]) -> Option<Run> {
    let out = dir.join(format!("{bundle}.{role}.entitlements.plist"));
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("platform/scripts/profile-entitlements.py");
    let mut cmd = Command::new("python3");
    cmd.arg(&script)
        .arg(profile)
        .arg(&out)
        .arg(bundle)
        .arg(role);
    for key in ["PRISM_PUSH", "PRISM_ICLOUD", "PRISM_APPLE_SIGNIN"] {
        cmd.env_remove(key);
    }
    cmd.envs(env.iter().copied());
    let output = cmd.output().ok()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(Run {
        notes: String::from_utf8(output.stderr).unwrap(),
        entitlements: std::fs::read_to_string(&out).unwrap(),
    })
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("prism-signing-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Issue #45: the widget extension's profile has the App Group only; its
/// missing Push Notifications must not produce the app's push note.
#[test]
fn push_note_comes_from_the_app_profile_only() {
    let dir = temp("push-ok");
    let app = profile(&dir, "app.plist", BUNDLE, &format!("{GROUP}{PUSH}"));
    let widgets = profile(&dir, "widgets.plist", &format!("{BUNDLE}.widgets"), GROUP);
    let push = [("PRISM_PUSH", "development")];
    let Some(a) = run(&dir, &app, BUNDLE, "app", &push) else {
        return eprintln!("python3 not found: skipped");
    };
    assert_eq!(a.notes, "");
    assert!(a.entitlements.contains("<key>aps-environment</key>"));
    let w = run(
        &dir,
        &widgets,
        &format!("{BUNDLE}.widgets"),
        "extension",
        &push,
    )
    .unwrap();
    assert_eq!(w.notes, "", "no push note for the extension");
    assert!(!w.entitlements.contains("aps-environment"));
    std::fs::remove_dir_all(&dir).ok();
}

/// An app profile without the capability still gets the note, naming it.
#[test]
fn push_note_when_the_app_profile_lacks_it() {
    let dir = temp("push-missing");
    let app = profile(&dir, "app.plist", BUNDLE, GROUP);
    let Some(a) = run(&dir, &app, BUNDLE, "app", &[("PRISM_PUSH", "development")]) else {
        return eprintln!("python3 not found: skipped");
    };
    assert_eq!(
        a.notes.trim(),
        "note: the provisioning profile of dev.prism.counter has no Push Notifications capability: \
         register_for_push will fail (enable it for the App ID)"
    );
    // Without [push], no note at all.
    let quiet = run(&dir, &app, BUNDLE, "app", &[]).unwrap();
    assert_eq!(quiet.notes, "");
    std::fs::remove_dir_all(&dir).ok();
}

/// iCloud and Sign in with Apple: noted for the app only, like push.
#[test]
fn other_capability_notes_are_the_apps() {
    let dir = temp("caps");
    let widgets = profile(&dir, "widgets.plist", &format!("{BUNDLE}.widgets"), GROUP);
    let env = [
        ("PRISM_ICLOUD", "1"),
        ("PRISM_APPLE_SIGNIN", "1"),
        ("PRISM_PUSH", "development"),
    ];
    let Some(w) = run(
        &dir,
        &widgets,
        &format!("{BUNDLE}.widgets"),
        "extension",
        &env,
    ) else {
        return eprintln!("python3 not found: skipped");
    };
    assert_eq!(w.notes, "");
    let a = run(&dir, &widgets, BUNDLE, "app", &env).unwrap();
    assert_eq!(a.notes.lines().count(), 3, "{}", a.notes);
    assert!(a.notes.contains("has no iCloud"));
    assert!(a.notes.contains("has no Sign in with Apple"));
    std::fs::remove_dir_all(&dir).ok();
}

/// The signing entitlements: this bundle's identifier (a wildcard profile
/// serves the extension too), no wildcard keychain groups, the App Group.
#[test]
fn entitlements_for_the_bundle() {
    let dir = temp("ents");
    let wildcard = profile(&dir, "wildcard.plist", "*", GROUP);
    let Some(w) = run(
        &dir,
        &wildcard,
        &format!("{BUNDLE}.widgets"),
        "extension",
        &[],
    ) else {
        return eprintln!("python3 not found: skipped");
    };
    assert!(
        w.entitlements
            .contains("<string>PRISMTEST1.dev.prism.counter.widgets</string>")
    );
    assert!(!w.entitlements.contains("keychain-access-groups"));
    assert!(w.entitlements.contains("group.dev.prism.counter"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unknown_role_fails() {
    let dir = temp("role");
    let app = profile(&dir, "app.plist", BUNDLE, "");
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("platform/scripts/profile-entitlements.py");
    let Ok(out) = Command::new("python3")
        .arg(script)
        .arg(&app)
        .arg(dir.join("out.plist"))
        .arg(BUNDLE)
        .arg("widgets")
        .output()
    else {
        return eprintln!("python3 not found: skipped");
    };
    assert!(!out.status.success());
    std::fs::remove_dir_all(&dir).ok();
}
