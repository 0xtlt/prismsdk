//! S33 (security review): on Android, a deep link ends a browser sign-in
//! only if it answers it (`PrismSignIn.answers`, called by
//! `PrismSecurity.redirected`): the same scheme, host, port and path as the
//! redirect URI, and the sign-in's `state`, once. Checked here on the JVM
//! against `prism_core::security::is_sign_in_callback` (skipped without
//! `javac` and `java`). That the deep link reaches `redirected` is the
//! device's.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use prism_core::security::is_sign_in_callback;

const URL: &str = "https://idp.example/authorize?client_id=c&state=s%2B1&code_challenge=x";
const APP: &str = "prism-my-app:/oauth";
const HOST: &str = "com.example.app://auth/done";
const LINK: &str = "https://app.example/oauth";

/// (authorization URL, redirect URI, deep link).
const CASES: &[(&str, &str, &str)] = &[
    (URL, APP, "prism-my-app:/oauth?code=a&state=s%2B1"),
    (URL, APP, "PRISM-MY-APP:/oauth?state=s%2B1&code=a"),
    (URL, APP, "prism-my-app:/oauth#code=a&state=s%2B1"),
    (
        URL,
        APP,
        "prism-my-app:/oauth?error=access_denied&state=s%2B1",
    ),
    (URL, APP, "prism-my-app:/oauth?code=a&state=other"),
    (URL, APP, "prism-my-app:/oauth?code=a"),
    (URL, APP, "prism-my-app:/oauth?code=a&state=s+1"),
    (
        URL,
        APP,
        "prism-my-app:/oauth?state=other&code=a&state=s%2B1",
    ),
    (URL, APP, "prism-my-app:/oauth?state=s%2B1&state=s%2B1"),
    (URL, APP, "prism-my-app://evil/oauth?code=a&state=s%2B1"),
    (URL, APP, "prism-my-app:/oauth/x?code=a&state=s%2B1"),
    (URL, APP, "prism-my-app:/oauthx?code=a&state=s%2B1"),
    (URL, APP, "prism-other:/oauth?code=a&state=s%2B1"),
    (URL, APP, "prism-my-app:/other?code=a&state=s%2B1"),
    (URL, APP, ":/oauth?state=s%2B1"),
    (URL, APP, "no scheme"),
    (URL, HOST, "com.example.app://AUTH/done?state=s%2B1"),
    (URL, HOST, "com.example.app://other/done?state=s%2B1"),
    (URL, HOST, "com.example.app://auth/done/?state=s%2B1"),
    (URL, LINK, "https://app.example/oauth?code=a&state=s%2B1"),
    (
        URL,
        LINK,
        "https://app.example:8443/oauth?code=a&state=s%2B1",
    ),
    (
        URL,
        LINK,
        "https://user@app.example/oauth?code=a&state=s%2B1",
    ),
    (URL, LINK, "http://app.example/oauth?code=a&state=s%2B1"),
    (
        URL,
        "https://app.example",
        "https://app.example/?state=s%2B1",
    ),
    (
        "https://idp.example/authorize?state=%C3%A9t%C3%A9%zz",
        APP,
        "prism-my-app:/oauth?state=%c3%a9t%c3%a9%zz",
    ),
    (
        "https://idp.example/authorize?state=%C3%A9",
        APP,
        "prism-my-app:/oauth?state=%E9",
    ),
    (
        "https://idp.example/authorize?client_id=c",
        APP,
        "prism-my-app:/oauth?code=a",
    ),
    (
        "https://idp.example/authorize?client_id=c",
        APP,
        "prism-my-app:/x?code=a",
    ),
];

/// `PrismSignIn.java` and a small `main` reading cases (tab-separated) on
/// stdin, compiled; `None` without a JDK.
fn compiled() -> Option<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("prism-cli-s33-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("platform/android/java/dev/prism/PrismSignIn.java");
    let check = dir.join("SignInCheck.java");
    std::fs::write(
        &check,
        r#"package dev.prism;
import java.io.*;
import java.nio.charset.StandardCharsets;
public class SignInCheck {
    public static void main(String[] a) throws IOException {
        BufferedReader in = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
        StringBuilder out = new StringBuilder();
        for (String line; (line = in.readLine()) != null; ) {
            String[] f = line.split("\t", -1);
            out.append(PrismSignIn.answers(f[0], f[1], f[2])).append(' ')
               .append(PrismSignIn.sameTarget(f[1], f[2])).append('\n');
        }
        System.out.print(out);
    }
}
"#,
    )
    .unwrap();
    let javac = Command::new("javac")
        .arg("-d")
        .arg(&dir)
        .arg(&source)
        .arg(&check)
        .output();
    match javac {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("no javac: PrismSignIn is not run");
            None
        }
        Err(e) => panic!("javac: {e}"),
        Ok(out) => {
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            Some(dir)
        }
    }
}

#[test]
fn s33_android_takes_only_the_callback_of_the_sign_in() {
    let Some(dir) = compiled() else {
        return;
    };
    let mut java = Command::new("java")
        .arg("-cp")
        .arg(&dir)
        .arg("dev.prism.SignInCheck")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("java");
    let input: String = CASES
        .iter()
        .map(|(url, redirect, link)| format!("{url}\t{redirect}\t{link}\n"))
        .collect();
    java.stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = java.wait_with_output().unwrap();
    assert!(out.status.success());
    let lines: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(lines.len(), CASES.len());
    let no_state = "https://idp.example/authorize";
    for ((url, redirect, link), line) in CASES.iter().zip(&lines) {
        let rust = format!(
            "{} {}",
            is_sign_in_callback(url, redirect, link),
            is_sign_in_callback(no_state, redirect, link)
        );
        assert_eq!(*line, rust, "{url} {redirect} {link}");
    }
    // Not all alike.
    assert_eq!(lines[0], "true true");
    assert_eq!(lines[4], "false true");
    assert_eq!(lines[9], "false false");
    let _ = std::fs::remove_dir_all(dir);
}
