//! S24 (security review): `bundletool.sh` downloads bundletool once into a
//! cache in the user's home and checks it against a pinned SHA-256. The
//! cached jar is checked again before each run: a changed jar is never run.
//! `java` and `curl` are stubs here.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

#[path = "../src/temp_dir.rs"]
mod temp_dir;

use temp_dir::TempDir;

fn setup(name: &str) -> TempDir {
    let dir = TempDir::new(&format!("s24-bt-{name}"));
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for (tool, body) in [
        ("java", "#!/bin/sh\necho \"$*\" >> \"$STUB_LOG\"\n"),
        // A download that serves other bytes.
        (
            "curl",
            "#!/bin/sh\nwhile [ \"$1\" != -o ]; do shift; done\nprintf 'not bundletool' > \"$2\"\n",
        ),
    ] {
        let path = bin.join(tool);
        std::fs::write(&path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

fn run(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (bool, String) {
    let out = Command::new("bash")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("platform/scripts/bundletool.sh"))
        .args(args)
        .env_clear()
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", dir.join("bin").display()),
        )
        .env("HOME", dir)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("STUB_LOG", dir.join("java.log"))
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn java_ran(dir: &Path) -> bool {
    dir.join("java.log").exists()
}

#[test]
fn s24_a_changed_cached_bundletool_never_runs() {
    let dir = setup("cached");
    let jar = dir.join("cache/prism/bundletool/bundletool-all-1.18.3.jar");
    std::fs::create_dir_all(jar.parent().unwrap()).unwrap();
    for args in [&["build-bundle", "--output=app.aab"][..], &["--path"]] {
        std::fs::write(&jar, "changed").unwrap();
        let (ok, stderr) = run(&dir, args, &[]);
        assert!(!java_ran(&dir), "{args:?}: ran a changed bundletool");
        assert!(!ok, "{args:?}");
        assert!(stderr.contains("checksum mismatch"), "{stderr}");
    }
    // The changed jar is gone; the download (other bytes) was refused too.
    assert!(!jar.exists());
}

#[test]
fn s24_a_changed_download_never_runs() {
    let dir = setup("download");
    let (ok, stderr) = run(&dir, &["version"], &[]);
    assert!(!ok && !java_ran(&dir), "{stderr}");
    assert!(stderr.contains("checksum mismatch"), "{stderr}");
    let left: Vec<_> = std::fs::read_dir(dir.join("cache/prism/bundletool"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn an_explicit_bundletool_is_used_as_is() {
    let dir = setup("explicit");
    let mine = dir.join("mine.jar");
    std::fs::write(&mine, "vendored").unwrap();
    let (ok, stderr) = run(
        &dir,
        &["version"],
        &[("PRISM_BUNDLETOOL", mine.to_str().unwrap())],
    );
    assert!(ok, "{stderr}");
    let log = std::fs::read_to_string(dir.join("java.log")).unwrap();
    assert_eq!(log.trim(), format!("-jar {} version", mine.display()));
}
