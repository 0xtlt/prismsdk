//! S36 (security review): installing an App Bundle on a device
//! (`android-run.sh`, step 8: `bundletool build-apks --local-testing`) never
//! hands the store key's passwords to bundletool, which reads them only from
//! its command line (shown by `ps`) or from files (left on disk if the build
//! is killed): the device's split APKs are signed with the SDK's debug key.
//! The bundle itself is still signed with the store key, by jarsigner, its
//! passwords in the environment.
//!
//! Runs the script's step 8 with a stand-in bundletool that records its
//! arguments, the password files it is given and the files holding a
//! password while it runs.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

const SECRET: &str = "S36-SECRET";

fn script() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("platform/scripts/android-run.sh");
    std::fs::read_to_string(path).unwrap()
}

/// From the line with `from` to the next one with `to`, included.
fn section(script: &str, from: &str, to: &str) -> String {
    let lines: Vec<&str> = script.lines().collect();
    let start = lines.iter().position(|l| l.contains(from)).expect(from);
    let end = start
        + lines[start..]
            .iter()
            .position(|l| l.contains(to))
            .expect(to);
    lines[start..=end].join("\n")
}

#[test]
fn s36_installing_a_bundle_never_writes_or_shows_the_store_passwords() {
    let dir = std::env::temp_dir().join(format!("prism-cli-s36-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for d in ["tmp", "out"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    let log = dir.join("bundletool.log");
    let stub = dir.join("bundletool.sh");
    std::fs::write(
        &stub,
        r#"{ printf 'ARGS:'; printf ' %s' "$@"; printf '\n'; } >> "$S36_LOG"
for a in "$@"; do
  case "$a" in
    --ks-pass=file:*|--key-pass=file:*)
      f="${a#*=file:}"
      printf 'READ: %s\nLEFT: %s\n' "$(cat "$f")" "$f" >> "$S36_LOG" ;;
  esac
done
# What the build being killed now would leave on disk.
grep -rl -e S36-SECRET "$TMPDIR" "$OUT" 2>/dev/null | sed 's/^/LEFT: /' >> "$S36_LOG" || true
"#,
    )
    .unwrap();
    let step = section(&script(), "# 8. Install like Play would", "install-apks");
    let d = dir.display();
    let harness = format!(
        r#"set -euo pipefail
export TMPDIR="{d}/tmp" S36_LOG="{log}"
export OUT="{d}/out"
DEBUG_KEYSTORE="{d}/debug.keystore"
debug_keystore() {{ :; }}
KEYSTORE="{d}/release.jks" PASS="{SECRET}-STORE" KEY_PASS="{SECRET}-KEY" ALIAS=upload
BUNDLE="{d}/app.aab" SERIAL=emulator-5554 ADB=/usr/bin/true BUILD_TOOLS="{d}"
BUNDLETOOL=(bash "{stub}")
{step}
"#,
        log = log.display(),
        stub = stub.display(),
    );
    let out = Command::new("bash")
        .arg("-c")
        .arg(&harness)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = std::fs::read_to_string(&log).unwrap();
    assert!(!seen.contains(SECRET), "{seen}");
    assert!(!seen.contains("LEFT:"), "{seen}");
    // The splits: the debug key; then installed.
    let build = seen
        .lines()
        .find(|l| l.starts_with("ARGS: build-apks"))
        .expect("build-apks");
    assert!(
        build.contains(&format!("--ks={d}/debug.keystore")),
        "{build}"
    );
    assert!(!build.contains("release.jks"), "{build}");
    assert!(
        seen.lines().any(|l| l.starts_with("ARGS: install-apks")),
        "{seen}"
    );
    // The bundle Play gets: the store key, passwords from the environment.
    let script = script();
    assert!(
        script.contains(
            r#"PRISM_KS_PASS="$PASS" PRISM_KEY_PASS="$KEY_PASS" jarsigner -keystore "$KEYSTORE""#
        ) && script.contains("-storepass:env PRISM_KS_PASS -keypass:env PRISM_KEY_PASS"),
        "the bundle is no longer signed with the store key"
    );
    let _ = std::fs::remove_dir_all(dir);
}
