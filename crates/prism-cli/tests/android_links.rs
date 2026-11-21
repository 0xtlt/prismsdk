//! S41 (security review): on Android, a web view opens something outside
//! the app (an `Intent`) only when the user asked for it, and only for web
//! links, mail and phone numbers, as on Apple platforms and Windows: a page
//! could open a new window on its own, to another app's own scheme
//! (`intent:`, `market:`, a custom one). The policy (`PrismLinks.java`)
//! runs here on the JVM (skipped without a JDK); that `PrismWeb` applies it
//! is checked in its source. The `Intent` itself is the device's.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn java_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("platform/android/java/dev/prism")
}

/// (URL, opens outside the app).
const URLS: &[(&str, bool)] = &[
    ("https://example.com/terms", true),
    ("HTTP://example.com", true),
    ("mailto:support@example.com", true),
    ("tel:+33123456789", true),
    (
        "intent://scan/#Intent;scheme=zxing;package=com.example.scanner;end",
        false,
    ),
    ("market://details?id=com.example", false),
    ("otherapp://transfer?to=me", false),
    ("file:///sdcard/Download/x.pdf", false),
    ("content://com.example.provider/secret", false),
    ("javascript:alert(1)", false),
    ("data:text/html,<p>x</p>", false),
    ("sms:+33123456789?body=x", false),
    (" https://example.com", false),
    ("https", false),
    (":https://example.com", false),
    ("", false),
];

/// (user gesture, sandbox: `None` for none, else its allowances), opens.
const POPUPS: &[(bool, Option<&str>, bool)] = &[
    (true, None, true),
    (false, None, false),
    (true, Some("scripts,popups"), true),
    (false, Some("scripts,popups"), false),
    (true, Some("scripts"), false),
    (true, Some(""), false),
];

/// `PrismLinks.java` and a `main` answering cases (tab-separated) on stdin,
/// compiled; `None` without a JDK (macOS has a `javac` that fails without
/// one).
fn compiled() -> Option<PathBuf> {
    if !Command::new("javac")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("no JDK: PrismLinks is not run");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("prism-cli-s41-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let check = dir.join("LinksCheck.java");
    std::fs::write(
        &check,
        r#"package dev.prism;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;
public class LinksCheck {
    public static void main(String[] a) throws IOException {
        BufferedReader in = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
        StringBuilder out = new StringBuilder();
        for (String line; (line = in.readLine()) != null; ) {
            String[] f = line.split("\t", -1);
            if (f[0].equals("url")) {
                out.append(PrismLinks.external(f[1]));
            } else {
                Set<String> sandbox = null;
                if (!f[2].equals("none")) {
                    sandbox = new HashSet<>();
                    for (String s : f[2].split(",")) if (!s.isEmpty()) sandbox.add(s);
                }
                out.append(PrismLinks.popup(Boolean.parseBoolean(f[1]), sandbox));
            }
            out.append('\n');
        }
        System.out.print(out);
    }
}
"#,
    )
    .unwrap();
    let out = Command::new("javac")
        .arg("--release")
        .arg("17")
        .arg("-d")
        .arg(&dir)
        .arg(java_dir().join("PrismLinks.java"))
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

#[test]
fn s41_only_a_tap_opens_and_only_web_mail_and_phone_links() {
    let Some(dir) = compiled() else {
        return;
    };
    let mut java = Command::new("java")
        .arg("-cp")
        .arg(&dir)
        .arg("dev.prism.LinksCheck")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("java");
    let mut input = String::new();
    for (url, _) in URLS {
        input += &format!("url\t{url}\n");
    }
    for (gesture, sandbox, _) in POPUPS {
        input += &format!("popup\t{gesture}\t{}\n", sandbox.unwrap_or("none"));
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
    for (url, opens) in URLS {
        assert_eq!(got.next(), Some(opens.to_string().as_str()), "{url:?}");
    }
    for (gesture, sandbox, opens) in POPUPS {
        assert_eq!(
            got.next(),
            Some(opens.to_string().as_str()),
            "gesture {gesture}, sandbox {sandbox:?}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Every way out of the app goes through the policy: a new window asks
/// `popup` before it is made, and `openExternally` (the one place that
/// starts an `Intent`) asks `external`.
#[test]
fn s41_the_web_view_applies_the_policy() {
    let java = std::fs::read_to_string(java_dir().join("PrismWeb.java")).unwrap();
    let window = java
        .find("public boolean onCreateWindow(")
        .expect("onCreateWindow");
    let catcher = window + java[window..].find("new WebView(").unwrap();
    assert!(
        java[window..catcher].contains("PrismLinks.popup(gesture, s.sandbox)"),
        "a new window without a tap"
    );
    assert_eq!(java.matches("new Intent(").count(), 1);
    let open = java
        .find("static void openExternally(")
        .expect("openExternally");
    let intent = open + java[open..].find("new Intent(").unwrap();
    assert!(
        java[open..intent].contains("PrismLinks.external("),
        "an Intent for any scheme"
    );
}
