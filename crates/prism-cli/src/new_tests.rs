//! `prism new`: the generated files, name checks, and (slow, ignored) that
//! the project builds.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::create;
use crate::temp_dir::TempDir;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn read(dir: &Path, file: &str) -> String {
    std::fs::read_to_string(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"))
}

#[test]
fn creates_a_project_named_after_the_directory() {
    let tmp = TempDir::new("new");
    let dir = tmp.join("my-cool-app");
    create(dir.to_str().unwrap(), Some(repo())).unwrap();
    let cargo = read(&dir, "Cargo.toml");
    assert!(cargo.contains("name = \"my_cool_app\""), "{cargo}");
    assert!(cargo.contains(&format!(
        "path = \"{}/crates/prism\"",
        super::toml_path(&repo())
    )));
    assert!(cargo.contains("prism = { package = \"prismsdk\""));
    assert!(cargo.contains("prism-build = { package = \"prismsdk-build\""));
    let prism = read(&dir, "Prism.toml");
    assert!(prism.contains("name = \"My Cool App\""));
    assert!(prism.contains("bundle_id = \"dev.prism.my-cool-app\""));
    assert!(read(&dir, "src/main.rs").contains("my_cool_app::run();"));
    let lib = read(&dir, "src/lib.rs");
    assert!(lib.contains(".title(\"My Cool App\")") && !lib.contains("{title}"));
    assert!(lib.contains("prism::android_main!(run);"));
    // The app's own binary gets its link settings (Apple: Swift rpath;
    // Windows: the WinUI runtime, manifest, icon, version, no console).
    let build = read(&dir, "build.rs");
    assert!(build.contains("prism_build::link_app();"));
    assert!(build.contains("Windows"));
    assert!(read(&dir, "mise.toml").contains("windows = \"prism dev windows\""));
    assert!(prism.contains("# [windows]"));
    // Every deployment target is documented, with Prism's defaults.
    for (section, version) in [
        ("[ios]", "17.0"),
        ("[macos]", "13.0"),
        ("[tvos]", "17.0"),
        ("[watchos]", "10.0"),
        ("[visionos]", "26.0"),
    ] {
        assert!(prism.contains(section), "{section}");
        assert!(
            prism.contains(&format!("deployment_target = \"{version}\"")),
            "{section}"
        );
    }
    for f in [
        "build.rs",
        "locales/en.json",
        "locales/fr.json",
        "mise.toml",
        ".gitignore",
        "assets/.keep",
    ] {
        assert!(dir.join(f).exists(), "{f}");
    }
    // Catalogs are valid JSON with the keys the code uses.
    for f in ["locales/en.json", "locales/fr.json"] {
        let v: serde_json::Value = serde_json::from_str(&read(&dir, f)).unwrap();
        assert!(v["hello"].is_string() && v["taps"]["other"].is_string() && v["tap"].is_string());
    }
    // Prism.toml reads back with the generated values.
    let c = crate::config::parse(&prism);
    assert_eq!(c["app.version"], "0.1.0");
    // Never overwrites.
    let err = create(dir.to_str().unwrap(), Some(repo())).unwrap_err();
    assert!(err.contains("already exists"), "{err}");
}

#[test]
fn toml_paths_have_no_escapes() {
    assert_eq!(
        super::toml_path(Path::new(r"\\?\C:\Users\me\prism")),
        "C:/Users/me/prism"
    );
    assert_eq!(
        super::toml_path(Path::new("/home/me/prism")),
        "/home/me/prism"
    );
}

#[test]
fn rejects_names_that_are_not_crate_names() {
    let tmp = TempDir::new("names");
    for bad in ["1app", "my app", "café", "-x"] {
        let dir = tmp.join(bad);
        assert!(
            create(dir.to_str().unwrap(), Some(repo())).is_err(),
            "{bad}"
        );
        assert!(!dir.exists(), "{bad}: nothing written");
    }
    let missing = tmp.join("nowhere");
    let err = create(tmp.join("ok").to_str().unwrap(), Some(missing)).unwrap_err();
    assert!(err.contains("nowhere"), "{err}");
}

/// Builds the generated project against this checkout, in the workspace's
/// target directory (no extra build cache): `cargo test -p prismsdk-cli --
/// --ignored generated_project`.
#[test]
#[ignore = "slow: compiles the generated app"]
fn generated_project_builds() {
    let tmp = TempDir::new("build");
    let dir = tmp.join("hello_prism");
    create(dir.to_str().unwrap(), Some(repo())).unwrap();
    // Same dependency versions as the workspace, so artifacts are reused.
    std::fs::copy(repo().join("Cargo.lock"), dir.join("Cargo.lock")).unwrap();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo().join("target"));
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    for args in [
        vec!["build"],
        vec!["build", "--no-default-features", "--features", "web"],
    ] {
        let status = Command::new(&cargo)
            .args(&args)
            .current_dir(&dir)
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "cargo {args:?} failed in {}",
            dir.display()
        );
    }
}
