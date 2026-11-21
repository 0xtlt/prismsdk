//! S24 (security review): the Android libraries an app needs (the car
//! library, Firebase) come from Maven at build time, through a cache in the
//! user's home. Every file they come from is pinned by SHA-256 in
//! `android-deps.lock`: a changed archive, downloaded or cached, stops the
//! build before anything is unpacked or packaged. Runs the resolver
//! (`android-deps.py`) with Python against a local repository.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Writes a small repository (`lib` 1.0, an AAR needing `dep` 2.0, a JAR)
/// into `argv[1]`; `argv[2] == "changed"` writes other (harmless) bytes for
/// the same versions. Prints the lock lines of what it wrote.
const REPO: &str = r#"
import hashlib, io, os, sys, zipfile
repo, changed = sys.argv[1], sys.argv[2] == "changed"
def pom(a, packaging, deps=""):
    return f'''<project xmlns="http://maven.apache.org/POM/4.0.0"><groupId>dev.prism.test</groupId>
<artifactId>{a}</artifactId><packaging>{packaging}</packaging><dependencies>{deps}</dependencies></project>'''.encode()
def zip_of(files):
    b = io.BytesIO()
    with zipfile.ZipFile(b, "w") as z:
        for name, data in files.items():
            z.writestr(zipfile.ZipInfo(name, (2020, 1, 1, 0, 0, 0)), data)
    return b.getvalue()
classes = zip_of({"dev/prism/Lib.class": b"changed" if changed else b"genuine"})
files = {
    "dev/prism/test/lib/1.0/lib-1.0.pom": pom("lib", "aar",
        "<dependency><groupId>dev.prism.test</groupId><artifactId>dep</artifactId><version>2.0</version></dependency>"),
    "dev/prism/test/lib/1.0/lib-1.0.aar": zip_of({
        "AndroidManifest.xml": b'<manifest package="dev.prism.test.lib"/>',
        "classes.jar": classes,
        "res/values/strings.xml": b"<resources/>",
    }),
    "dev/prism/test/dep/2.0/dep-2.0.pom": pom("dep", "jar"),
    "dev/prism/test/dep/2.0/dep-2.0.jar": zip_of({"dev/prism/Dep.class": b"changed" if changed else b"genuine"}),
}
for path, data in files.items():
    os.makedirs(os.path.dirname(os.path.join(repo, path)), exist_ok=True)
    open(os.path.join(repo, path), "wb").write(data)
    print(f"{hashlib.sha256(data).hexdigest()}  {path}")
"#;

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "prism-s24-maven-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    /// A repository at `name`; returns its lock.
    fn repo(&self, name: &str, variant: &str) -> String {
        let out = Command::new("python3")
            .args(["-I", "-c", REPO])
            .arg(self.0.join(name))
            .arg(variant)
            .output()
            .expect("python3");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Resolved {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// `android-deps.py <cache> roots…` with `lock` as the lock and `repo` as
/// the only repository (none: offline).
fn resolve(dir: &Dir, cache: &str, lock: &str, repo: Option<&str>, roots: &[&str]) -> Resolved {
    let lock_path = dir.0.join(format!("{cache}.lock"));
    std::fs::write(&lock_path, lock).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("platform/scripts/android-deps.py");
    let repos = match repo {
        Some(r) => format!("file://{}", dir.0.join(r).display()),
        None => format!("file://{}", dir.0.join("offline").display()),
    };
    let out = Command::new("python3")
        .arg("-I")
        .arg(script)
        .arg(dir.0.join(cache))
        .args(roots)
        .env("PRISM_MAVEN_LOCK", &lock_path)
        .env("PRISM_MAVEN_REPOS", repos)
        .output()
        .expect("python3");
    Resolved {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// Puts `repo`'s files into `cache` as an earlier run (or a shared cache)
/// would have left them.
fn seed_cache(dir: &Dir, repo: &str, cache: &str) {
    let ok = Command::new("cp")
        .arg("-R")
        .arg(dir.0.join(repo))
        .arg(dir.0.join(cache).join("files"))
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

const LIB: &str = "dev.prism.test:lib:1.0";

#[test]
fn s24_a_changed_cache_entry_stops_the_build() {
    let dir = Dir::new();
    let lock = dir.repo("genuine", "genuine");
    std::fs::create_dir_all(dir.0.join("cache")).unwrap();
    dir.repo("changed", "changed");
    seed_cache(&dir, "changed", "cache");
    let r = resolve(&dir, "cache", &lock, Some("genuine"), &[LIB]);
    assert!(!r.ok, "resolved from changed files: {}", r.stdout);
    assert!(r.stdout.is_empty(), "{}", r.stdout);
    assert!(r.stderr.contains("does not match"), "{}", r.stderr);
    assert!(
        !dir.0.join("cache/unpacked").exists(),
        "unpacked a changed archive"
    );
}

#[test]
fn s24_a_different_archive_under_the_same_version_stops_the_build() {
    let dir = Dir::new();
    let lock = dir.repo("genuine", "genuine");
    dir.repo("changed", "changed");
    let r = resolve(&dir, "cache", &lock, Some("changed"), &[LIB]);
    assert!(!r.ok, "resolved from changed files: {}", r.stdout);
    assert!(r.stderr.contains("SHA-256"), "{}", r.stderr);
    assert!(!dir.0.join("cache/unpacked").exists());
}

#[test]
fn s24_verified_files_resolve_the_same_offline() {
    let dir = Dir::new();
    let lock = dir.repo("genuine", "genuine");
    let online = resolve(&dir, "cache", &lock, Some("genuine"), &[LIB]);
    assert!(online.ok, "{}", online.stderr);
    let libs: serde_json::Value = serde_json::from_str(&online.stdout).unwrap();
    let ids: Vec<&str> = libs
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["dev.prism.test:dep:2.0", "dev.prism.test:lib:1.0"]);
    assert_eq!(libs[1]["package"], "dev.prism.test.lib");

    let offline = resolve(&dir, "cache", &lock, None, &[LIB]);
    assert!(offline.ok, "{}", offline.stderr);
    assert_eq!(offline.stdout, online.stdout);

    // An unpacked library changed in the cache is unpacked again from its
    // verified archive.
    let class = dir
        .0
        .join("cache/unpacked/dev.prism.test_lib_1.0/classes.jar");
    std::fs::write(&class, "changed").unwrap();
    let again = resolve(&dir, "cache", &lock, None, &[LIB]);
    assert!(again.ok, "{}", again.stderr);
    assert_ne!(std::fs::read(&class).unwrap(), b"changed");
}

#[test]
fn s24_unpinned_or_odd_coordinates_are_refused() {
    let dir = Dir::new();
    let lock = dir.repo("genuine", "genuine");
    for root in [
        "dev.prism.test:lib",
        "dev.prism.test:lib:1.0:extra",
        "../..:lib:1.0",
        "dev.prism.test:..:1.0",
        "dev.prism.test:lib:../../1.0",
    ] {
        let r = resolve(&dir, "cache", &lock, Some("genuine"), &[root]);
        assert!(!r.ok, "{root}: {}", r.stdout);
    }
    // A library the lock does not list.
    let r = resolve(&dir, "cache", "", Some("genuine"), &[LIB]);
    assert!(!r.ok, "{}", r.stdout);
    assert!(r.stderr.contains("--lock"), "{}", r.stderr);
}
