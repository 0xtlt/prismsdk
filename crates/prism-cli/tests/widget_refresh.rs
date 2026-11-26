//! Widgets refreshed without the app (#331). On Android, an alarm runs the
//! widget's refresh hook at the time its reload policy gives
//! (`PrismRefreshPolicy.java`, run here on the JVM, skipped without a JDK);
//! the bundlers make the app's binary the WidgetKit extension's executable
//! when a widget has a hook (checked in their source).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[path = "../src/temp_dir.rs"]
mod temp_dir;
use temp_dir::TempDir;

fn platform() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("platform")
}

/// `PrismRefreshPolicy.java` and a `main` answering cases on stdin, compiled;
/// `None` without a JDK.
fn compiled() -> Option<TempDir> {
    if !Command::new("javac")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("no JDK: PrismRefreshPolicy is not run");
        return None;
    }
    let dir = TempDir::new("cli-331");
    std::fs::create_dir_all(&dir).unwrap();
    let check = dir.join("RefreshCheck.java");
    std::fs::write(
        &check,
        r#"package dev.prism;
import java.io.*;
import java.nio.charset.StandardCharsets;
public class RefreshCheck {
    public static void main(String[] a) throws IOException {
        BufferedReader in = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
        StringBuilder out = new StringBuilder();
        for (String line; (line = in.readLine()) != null; ) {
            // refresh, reload (n:<seconds>, s:<string>, null), last ahead (s, or nan), now (ms)
            String[] f = line.split("\t", -1);
            Object reload = f[1].startsWith("n:") ? (Object) Double.valueOf(f[1].substring(2))
                    : f[1].startsWith("s:") ? f[1].substring(2) : null;
            double last = f[2].equals("nan") ? Double.NaN : Double.parseDouble(f[2]);
            out.append(PrismRefreshPolicy.refreshAt(Boolean.parseBoolean(f[0]), reload, last, Long.parseLong(f[3])));
            out.append('\n');
        }
        System.out.print(out);
    }
}
"#,
    )
    .unwrap();
    let out = Command::new("javac")
        .args(["--release", "17", "-d"])
        .arg(&*dir)
        .arg(platform().join("android/java/dev/prism/PrismRefreshPolicy.java"))
        .arg(&check)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(dir)
}

const NOW: i64 = 1_800_000_000_000;

/// (refresh, reload, last drawing ahead, when the hook runs next).
const CASES: &[(bool, &str, &str, i64)] = &[
    // `Widget::reload_after`: the delay, from now.
    (true, "n:900", "nan", NOW + 900_000),
    (true, "n:90.5", "nan", NOW + 90_500),
    // At least a minute apart.
    (true, "n:1", "nan", NOW + 60_000),
    (true, "n:0", "nan", NOW + 60_000),
    // `WidgetReload::AtEnd`: when the drawings ahead run out.
    (true, "s:end", "1800003600", NOW + 3_600_000),
    (true, "s:end", "1800000010", NOW + 60_000),
    (true, "s:end", "1799999000", 0),
    (true, "s:end", "nan", 0),
    // `WidgetReload::Never`, or no policy.
    (true, "s:never", "nan", 0),
    (true, "null", "nan", 0),
    // No hook: the alarm only redraws.
    (false, "n:900", "nan", 0),
    // Past a year (`reload_after(Duration::MAX)`): never, not an overflow
    // into the past that fires at once and loops.
    (true, "n:18446744073709551615", "nan", 0),
    (true, "n:31536001", "nan", 0),
];

#[test]
fn android_runs_the_hook_at_the_reload_policys_time() {
    let Some(dir) = compiled() else {
        return;
    };
    let mut java = Command::new("java")
        .arg("-cp")
        .arg(&*dir)
        .arg("dev.prism.RefreshCheck")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("java");
    let mut input = String::new();
    for (refresh, reload, last, _) in CASES {
        input += &format!("{refresh}\t{reload}\t{last}\t{NOW}\n");
    }
    java.stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = java.wait_with_output().unwrap();
    assert!(out.status.success());
    let got = String::from_utf8(out.stdout).unwrap();
    let mut got = got.lines();
    for (refresh, reload, last, at) in CASES {
        assert_eq!(
            got.next(),
            Some(at.to_string().as_str()),
            "refresh {refresh}, reload {reload}, last {last}"
        );
    }
}

/// The alarm's broadcast runs the hook through `nativeWidgetRefresh`, keeps
/// the process alive until it ends, and sets the next try first (a failed
/// hook writes no snapshot to set it); a fresh drawing sets it anew.
#[test]
fn android_refresh_plumbing() {
    let java = std::fs::read_to_string(platform().join("android/java/dev/prism/PrismWidget.java"))
        .unwrap();
    let receiver = java
        .split("public static final class Refresh extends")
        .nth(1)
        .expect("PrismWidget.Refresh");
    let next = receiver.find("scheduleRefresh(c, kind, at);").unwrap();
    let run = receiver
        .find("PrismHost.nativeWidgetRefresh(task, kind,")
        .unwrap();
    assert!(next < run, "the next try is set before the hook runs");
    assert!(receiver.contains("running.put(task, goAsync());"));
    // A widget removed from every home screen stops refreshing, also when
    // its last hook failed (no new drawing to cancel the alarm).
    let gone = receiver.find("stopRefreshing(c, kind);").unwrap();
    assert!(gone < next, "a removed widget is not re-armed");
    assert!(java.contains("public void onDisabled(Context c)"));
    assert!(java.contains("prefs.putLong(name + REFRESH_AT, refreshAt(snap, now));"));
    let host =
        std::fs::read_to_string(platform().join("android/java/dev/prism/PrismHost.java")).unwrap();
    assert!(host.contains(
        "static native void nativeWidgetRefresh(String task, String kind, String info);"
    ));
    let job =
        std::fs::read_to_string(platform().join("android/java/dev/prism/PrismJob.java")).unwrap();
    assert!(job.contains("if (PrismWidget.Refresh.finished(task)) return;"));
    let script = std::fs::read_to_string(platform().join("scripts/android-run.sh")).unwrap();
    assert!(script.contains(
        r#"<receiver android:name="dev.prism.PrismWidget$Refresh" android:exported="false"/>"#
    ));
}

/// iOS, visionOS and macOS: with a hook, the extension's executable is the
/// app's binary, built with the extension's Swift linked in.
#[test]
fn apple_bundlers_ship_the_app_as_its_widget_extension() {
    for (script, copy) in [
        ("ios-run.sh", r#"cp "$APP/$PKG" "$APPEX/PrismWidgets""#),
        (
            "macos-app.sh",
            r#"cp "$BIN" "$APPEX/Contents/MacOS/PrismWidgets""#,
        ),
    ] {
        let text = std::fs::read_to_string(platform().join("scripts").join(script)).unwrap();
        assert!(
            text.contains(
                r#"print(int(any(w.get("refresh") for w in json.load(open(sys.argv[1])))))"#
            ),
            "{script}: a widget with a hook"
        );
        assert!(
            text.contains(r#"PRISM_WIDGET_EXTENSION="$GEN/Generated.swift""#),
            "{script}: the extension linked in"
        );
        assert!(
            text.contains(
                r#"PRISM_WIDGET_EXTENSION_CONSTVALUES="$GEN/PrismWidgets.swiftconstvalues""#
            ),
            "{script}: configurable widgets' metadata"
        );
        assert!(text.contains(copy), "{script}: the binary copied");
    }
}
