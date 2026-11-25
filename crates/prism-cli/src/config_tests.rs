//! `Prism.toml` parsing and project loading.

use std::path::Path;

use super::{Project, parse};
use crate::temp_dir::TempDir;

/// Writes `contents` at `path` under `dir`, making the folders it needs.
fn write(dir: &Path, path: &str, contents: &str) {
    let p = dir.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

#[test]
fn values_keep_hashes_inside_quotes() {
    let c = parse(
        "[app]\nname = \"Todo #1\"   # the title\n[permissions]\ncamera = \"Scan QR #codes\"\n",
    );
    assert_eq!(c["app.name"], "Todo #1");
    assert_eq!(c["permissions.camera"], "Scan QR #codes");
}

#[test]
fn literal_strings_bare_values_and_equals_signs() {
    let c = parse("[app]\nname = 'Single'\n[ios]\nicloud = true\n[x]\nurl = \"a=b\"\n");
    assert_eq!(c["app.name"], "Single");
    assert_eq!(c["ios.icloud"], "true");
    assert_eq!(c["x.url"], "a=b");
}

#[test]
fn sections_are_trimmed_and_keys_without_a_section_are_top_level() {
    let c = parse("top = \"1\"\n[ app ]\n  name=\"x\"\n# [ios]\nteam = \"T\"\n");
    assert_eq!(c[".top"], "1");
    assert_eq!(c["app.name"], "x");
    // The commented-out header does not start a section.
    assert_eq!(c["app.team"], "T");
}

fn project(dir: &TempDir, prism_toml: Option<&str>, cargo_toml: &str) -> Result<Project, String> {
    write(dir, "Cargo.toml", cargo_toml);
    write(dir, "src/lib.rs", "");
    if let Some(p) = prism_toml {
        write(dir, "Prism.toml", p);
    }
    Project::load(dir)
}

const CARGO: &str =
    "[package]\nname = \"my_app\"\nversion = \"2.3.4\"\nedition = \"2024\"\n\n[workspace]\n";

/// #187: hosts for development builds only, checked like `allow_http`.
#[test]
fn network_allow_http_dev_stays_out_of_release_builds() {
    let dir = TempDir::new("http-dev");
    let p = project(
        &dir,
        Some(
            "[network]\nallow_http = [\"shared.dev\"]\nallow_http_dev = [\"192.168.1.20\", \"Shared.dev\"]\n",
        ),
        CARGO,
    )
    .unwrap();
    assert_eq!(p.http_hosts_dev, ["192.168.1.20", "shared.dev"]);
    assert_eq!(p.http_hosts_for(false), ["shared.dev", "192.168.1.20"]);
    assert_eq!(p.http_hosts_for(true), ["shared.dev"]);
    let err = project(
        &dir,
        Some("[network]\nallow_http_dev = [\"http://x.dev\"]\n"),
        CARGO,
    )
    .err()
    .unwrap();
    assert!(err.contains("allow_http"), "{err}");
}

/// #191: associated domains, checked and normalized.
#[test]
fn ios_associated_domains_are_checked() {
    let dir = TempDir::new("assoc");
    let p = project(
        &dir,
        Some(
            "[ios]\nassociated_domains = [\"webcredentials:Example.com\", \"applinks:*.example.com?mode=developer\"]\n",
        ),
        CARGO,
    )
    .unwrap();
    assert_eq!(
        p.associated_domains,
        [
            "webcredentials:example.com",
            "applinks:*.example.com?mode=developer"
        ]
    );
    assert!(
        project(&dir, Some("[app]\nname = \"x\"\n"), CARGO)
            .unwrap()
            .associated_domains
            .is_empty()
    );
    for bad in [
        "example.com",
        "passkeys:example.com",
        "webcredentials:",
        "webcredentials:localhost",
        "webcredentials:https://example.com",
        "applinks:example.com?mode=test",
    ] {
        let toml = format!("[ios]\nassociated_domains = [\"{bad}\"]\n");
        let err = project(&dir, Some(&toml), CARGO)
            .err()
            .unwrap_or_else(|| panic!("{bad} accepted"));
        assert!(err.contains("associated_domains"), "{err}");
    }
}

/// #152: plain-HTTP hosts, checked.
#[test]
fn network_allow_http_lists_hosts() {
    let dir = TempDir::new("http");
    let p = project(
        &dir,
        Some("[network]\nallow_http = [\"Preview.Localhost\", \"192.168.1.20\"]\n"),
        CARGO,
    )
    .unwrap();
    assert_eq!(p.http_hosts, ["preview.localhost", "192.168.1.20"]);
    let none = project(&dir, Some("[app]\nname = \"x\"\n"), CARGO).unwrap();
    assert!(none.http_hosts.is_empty());
    for bad in [
        "http://preview.localhost",
        "preview.localhost:8080",
        "a/b",
        ".x",
        "",
    ] {
        let toml = format!("[network]\nallow_http = [\"{bad}\", \"ok.dev\"]\n");
        let r = project(&dir, Some(&toml), CARGO);
        if bad.is_empty() {
            // Empty entries are dropped by the list parser.
            assert_eq!(r.unwrap().http_hosts, ["ok.dev"]);
        } else {
            let err = r.err().unwrap_or_else(|| panic!("{bad} accepted"));
            assert!(err.contains("allow_http"), "{err}");
        }
    }
}

#[test]
fn every_section_of_prism_toml() {
    let dir = TempDir::new("all");
    write(&dir, "locales/en.json", "{}");
    write(&dir, "locales/fr.json", "{}");
    write(&dir, "locales/notes.txt", "");
    let p = project(
        &dir,
        Some(
            r#"[app]
name = "My App"
package = "my_app"
bundle_id = "com.example.app"
version = "1.0.0"
icon = "art/icon.png"

[permissions]
location = "Finds stores near you."
camera = "Scans receipts."

[ios]
team = "ABCDE12345"
simulator = "iPhone 17"
device = "My iPhone"
icloud = true
"#,
        ),
        CARGO,
    )
    .unwrap();
    assert_eq!(p.name, "My App");
    assert_eq!(p.package, "my_app");
    assert_eq!(p.bundle_id, "com.example.app");
    assert_eq!(p.version, "1.0.0");
    assert_eq!(p.icon, Some(dir.join("art/icon.png")));
    assert_eq!(
        p.permissions,
        [
            ("camera".to_string(), "Scans receipts.".to_string()),
            ("location".into(), "Finds stores near you.".into()),
        ]
    );
    assert_eq!(p.languages, ["en", "fr"]);
    assert_eq!(p.team.as_deref(), Some("ABCDE12345"));
    assert_eq!(p.simulator.as_deref(), Some("iPhone 17"));
    assert_eq!(p.device.as_deref(), Some("My iPhone"));
    assert!(p.icloud);
    assert_eq!(p.root, dir.to_path_buf());
    // `cargo metadata`'s target directory: `CARGO_TARGET_DIR` when the tests
    // run with one, else the project's `target`.
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .and_then(|t| std::path::Path::new(&t).file_name().map(ToOwned::to_owned))
        .unwrap_or_else(|| "target".into());
    assert!(p.target_dir.ends_with(&target), "{:?}", p.target_dir);
}

#[test]
fn defaults_come_from_cargo_toml() {
    let dir = TempDir::new("defaults");
    let p = project(&dir, None, CARGO).unwrap();
    assert_eq!(p.package, "my_app");
    assert_eq!(p.name, "My_app");
    assert_eq!(p.bundle_id, "dev.prism.my-app");
    assert_eq!(p.version, "2.3.4");
    assert_eq!(p.icon, None);
    assert!(p.permissions.is_empty() && p.languages.is_empty());
    assert!(!p.icloud && p.team.is_none());
}

#[test]
fn workspace_versions_and_custom_locale_dirs() {
    let dir = TempDir::new("ws");
    write(&dir, "strings/de.json", "{}");
    let p = project(
        &dir,
        Some("[app]\nlocales = \"strings\"\nicon = \"\"\n[ios]\nicloud = false\n"),
        "[package]\nname = \"w\"\nversion = { workspace = true }\nedition = \"2024\"\n[workspace]\n[workspace.package]\nversion = \"9.9.9\"\n",
    )
    .unwrap();
    // `version = { workspace = true }` is not a version.
    assert_eq!(p.version, "0.1.0");
    assert_eq!(p.languages, ["de"]);
    assert_eq!(p.icon, None, "an empty value is unset");
    assert!(!p.icloud);
}

#[test]
fn a_project_needs_a_package_name() {
    let dir = TempDir::new("nopkg");
    write(&dir, "Prism.toml", "[app]\nname = \"X\"\n");
    let err = Project::load(&dir).err().unwrap();
    assert!(err.contains("package"), "{err}");
}

/// S23: the package and bundle ID name files and become launcher source,
/// so they are checked before any command uses them.
#[test]
fn s23_identifiers_follow_narrow_grammars() {
    let dir = TempDir::new("ids");
    for id in ["com.example.app", "dev.prism.my-app", "org.Example_2.App-3"] {
        let toml = format!("[app]\nbundle_id = \"{id}\"\n");
        assert_eq!(project(&dir, Some(&toml), CARGO).unwrap().bundle_id, id);
    }
    let long = format!("com.{}", "a".repeat(252));
    for id in [
        r#"dev.prism.app\";touch pwned;\""#,
        "dev.prism.$(touch pwned)",
        "dev.prism.a pp",
        "dev..app",
        ".dev.app",
        "dev.app.",
        "app",
        "../../tmp/app",
        "com.example.café",
        long.as_str(),
    ] {
        let toml = format!("[app]\nbundle_id = \"{id}\"\n");
        let err = project(&dir, Some(&toml), CARGO).err();
        assert!(
            err.as_deref().is_some_and(|e| e.contains("bundle_id")),
            "{id}: {err:?}"
        );
    }
    assert_eq!(
        project(
            &dir,
            Some(&format!("[app]\nbundle_id = \"com.{}\"\n", "a".repeat(251))),
            CARGO
        )
        .unwrap()
        .bundle_id
        .len(),
        255
    );

    for pkg in ["my_app", "my-app", "app2", "_x"] {
        let toml = format!("[app]\npackage = \"{pkg}\"\n");
        assert_eq!(project(&dir, Some(&toml), CARGO).unwrap().package, pkg);
    }
    for pkg in [
        "app$(touch pwned)",
        "../app",
        "my app",
        "-p",
        "a;b",
        "app\\n",
    ] {
        let toml = format!("[app]\npackage = \"{pkg}\"\n");
        let err = project(&dir, Some(&toml), CARGO).err();
        assert!(
            err.as_deref().is_some_and(|e| e.contains("[app] package")),
            "{pkg}: {err:?}"
        );
    }
}

#[test]
fn web_islands_and_their_budget() {
    let c =
        parse("[web]\nislands = [\"islands/chart\", 'islands/map'] # two\nisland_budget_kb = 25\n");
    assert_eq!(
        super::parse_list(&c["web.islands"]),
        ["islands/chart", "islands/map"]
    );
    assert_eq!(c["web.island_budget_kb"], "25");
    assert!(super::parse_list("[]").is_empty());
    let dir = TempDir::new("islands");
    let toml = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n";
    let p = project(&dir, Some("[web]\nislands = [\"islands/chart\"]\n"), toml).unwrap();
    assert_eq!(p.islands, [dir.join("islands/chart")]);
    assert_eq!(p.island_budget_kb, 40.);
    let bad = project(&dir, Some("[web]\nisland_budget_kb = \"lots\"\n"), toml);
    assert!(bad.err().unwrap().contains("island_budget_kb"));
}

#[test]
fn a_failing_test_leaves_no_temp_folder() {
    let made = std::sync::Mutex::new(None);
    let failed = std::panic::catch_unwind(|| {
        let dir = crate::temp_dir::TempDir::new("guard");
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data/file"), "x").unwrap();
        *made.lock().unwrap() = Some(dir.to_path_buf());
        panic!("a test that fails (on purpose)");
    });
    assert!(failed.is_err());
    let dir = made.lock().unwrap().take().expect("the folder was made");
    assert!(!dir.exists(), "{} is left behind", dir.display());
}

/// #310: `[app] orientations` for iPhone's Info.plist and Android's activity.
#[test]
fn app_orientations() {
    let load = |toml: Option<&str>| {
        let dir = TempDir::new("orientations");
        project(&dir, toml, CARGO)
    };
    // Unset: iPhone's portrait and both landscapes, Android's default.
    let p = load(None).unwrap();
    assert_eq!(
        p.ios_orientations(),
        [
            "UIInterfaceOrientationPortrait",
            "UIInterfaceOrientationLandscapeLeft",
            "UIInterfaceOrientationLandscapeRight"
        ]
    );
    assert_eq!(p.android_screen_orientation(), None);

    let p = load(Some("[app]\norientations = [\"portrait\"]\n")).unwrap();
    assert_eq!(p.ios_orientations(), ["UIInterfaceOrientationPortrait"]);
    assert_eq!(p.android_screen_orientation(), Some("portrait"));

    for (list, android) in [
        ("\"landscape\"", Some("userLandscape")),
        ("\"landscape-right\"", Some("landscape")),
        ("\"landscape_left\"", Some("reverseLandscape")),
        (
            "\"portrait\", \"portrait_upside_down\"",
            Some("userPortrait"),
        ),
        ("\"upside_down\"", Some("reversePortrait")),
        (
            "\"portrait\", \"landscape\", \"upside_down\"",
            Some("fullUser"),
        ),
        // Mixed, not all four: the system decides on Android.
        ("\"portrait\", \"landscape\"", None),
    ] {
        let p = load(Some(&format!("[app]\norientations = [{list}]\n"))).unwrap();
        assert_eq!(p.android_screen_orientation(), android, "{list}");
    }
    // Duplicates go, the order stays.
    let p = load(Some(
        "[app]\norientations = [\"landscape\", \"landscape_left\", \"portrait\"]\n",
    ))
    .unwrap();
    assert_eq!(
        p.ios_orientations(),
        [
            "UIInterfaceOrientationLandscapeLeft",
            "UIInterfaceOrientationLandscapeRight",
            "UIInterfaceOrientationPortrait"
        ]
    );
    let err = load(Some("[app]\norientations = [\"sideways\"]\n"))
        .err()
        .unwrap();
    assert!(
        err.contains("[app] orientations") && err.contains("sideways"),
        "{err}"
    );
}

/// #318: each `[permissions]` name is one `prism` knows, and those whose
/// code is compiled in only when declared turn on prismsdk's feature,
/// through the key the app depends on prismsdk under.
#[test]
fn permissions_turn_on_prismsdk_features() {
    const TOML: &str = "[permissions]\ncamera = \"Scans receipts.\"\nface_id = \"Unlocks.\"\nlocation_always = \"Tracks runs.\"\ncontacts = \"Finds friends.\"\n";
    let with = |dep: &str| {
        let dir = TempDir::new("features");
        project(
            &dir,
            Some(TOML),
            &format!("{CARGO}\n[dependencies]\n{dep}\n"),
        )
    };
    let p = with("prism = { package = \"prismsdk\", version = \"0.3\" }").unwrap();
    assert_eq!(p.prism_dependency.as_deref(), Some("prism"));
    assert_eq!(
        p.permission_features(),
        ["prism/camera", "prism/face-id", "prism/location-always"]
    );
    let p = with("prismsdk = \"0.3\"").unwrap();
    assert_eq!(p.permission_features()[0], "prismsdk/camera");
    // A dev-dependency compiles no app code.
    let p = with("\n[dev-dependencies]\nprismsdk = \"0.3\"").unwrap();
    assert_eq!(p.prism_dependency, None);
    assert!(p.permission_features().is_empty());

    let dir = TempDir::new("unknown-permission");
    let Err(err) = project(&dir, Some("[permissions]\nlocaton = \"x\"\n"), CARGO) else {
        panic!("an unknown permission loads");
    };
    assert!(
        err.contains("locaton") && err.contains("location_always"),
        "{err}"
    );
}
