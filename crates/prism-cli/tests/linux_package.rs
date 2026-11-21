//! S23 (security review): the Linux packager writes the app's launcher (a
//! shell script that runs at every start), its desktop entry and its
//! control file from Prism.toml values. The real script runs here with
//! bash, `cargo`, `dpkg`, `dpkg-shlibdeps` and `dpkg-deb` replaced by
//! stubs, and the test reads the tree it would package.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../src/temp_dir.rs"]
mod temp_dir;
use temp_dir::TempDir;

const STUBS: [(&str, &str); 4] = [
    (
        "cargo",
        r#"#!/bin/sh
case "$1" in
  metadata) printf '{"target_directory": "%s"}\n' "$STUB_TARGET" ;;
  build)
    echo "$*" >> "$STUB_LOG"
    mkdir -p "$STUB_TARGET/release"
    printf '#!/bin/sh\n' > "$STUB_TARGET/release/$4"
    chmod 755 "$STUB_TARGET/release/$4" ;;
esac
"#,
    ),
    ("dpkg", "#!/bin/sh\necho amd64\n"),
    (
        "dpkg-shlibdeps",
        "#!/bin/sh\necho 'shlibs:Depends=libc6 (>= 2.34), libgtk-4-1'\n",
    ),
    (
        "dpkg-deb",
        r#"#!/bin/sh
[ "$1" = --root-owner-group ] && cp -R "$3" "$STUB_STAGE"
exit 0
"#,
    ),
];

struct Run {
    dir: TempDir,
    ok: bool,
    stderr: String,
}

impl Run {
    fn stage(&self) -> PathBuf {
        self.dir.join("staged")
    }

    /// Whether the script got as far as building the app.
    fn built(&self) -> bool {
        self.dir.join("cargo.log").exists()
    }

    fn file(&self, rel: &str) -> String {
        std::fs::read_to_string(self.stage().join(rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}\n{}", self.stderr))
    }

    /// The launcher in `/usr/bin`, if the script staged one.
    fn launcher(&self) -> Option<PathBuf> {
        std::fs::read_dir(self.stage().join("usr/bin"))
            .ok()?
            .flatten()
            .map(|e| e.path())
            .next()
    }
}

/// Runs `linux-package.sh <package>` in a fresh directory with `env`.
fn package(pkg: &str, env: &[(&str, &str)]) -> Run {
    let dir = TempDir::new("s23");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for (name, body) in STUBS {
        let path = bin.join(name);
        std::fs::write(&path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("platform/scripts/linux-package.sh");
    let out = Command::new("bash")
        .arg(script)
        .arg(pkg)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", &*dir)
        .env("PRISM_PROJECT", &*dir)
        .env("PRISM_LINUX_OUT", dir.join("out"))
        .env("STUB_TARGET", dir.join("target"))
        .env("STUB_LOG", dir.join("cargo.log"))
        .env("STUB_STAGE", dir.join("staged"))
        .envs(env.iter().copied())
        .output()
        .expect("bash");
    Run {
        ok: out.status.success(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        dir,
    }
}

/// Runs the staged launcher with `sh`, as the desktop does: whatever it
/// starts, it must not run anything else. Returns whether it did.
fn launcher_runs_more(run: &Run) -> bool {
    let Some(launcher) = run.launcher() else {
        return false;
    };
    let cwd = run.dir.join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let _ = Command::new("sh")
        .arg(&launcher)
        .current_dir(&cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output();
    cwd.join("pwned").exists()
}

#[test]
fn s23_hostile_metadata_stops_before_staging() {
    let cases: [(&str, &[(&str, &str)]); 12] = [
        // Shell source in the identifiers that end up in the launcher.
        (
            "app",
            &[("PRISM_BUNDLE_ID", r#"dev.prism.app";touch pwned;""#)],
        ),
        ("app", &[("PRISM_BUNDLE_ID", "dev.prism.$(touch pwned)")]),
        ("app$(touch pwned)", &[]),
        // A line break in a display string: a new launcher line, a new
        // desktop key or a new control field.
        ("app", &[("PRISM_APP_NAME", "App\ntouch pwned")]),
        (
            "app",
            &[("PRISM_DESCRIPTION", "Counts\nExec=sh -c 'touch pwned'")],
        ),
        (
            "app",
            &[("PRISM_MAINTAINER", "Me <me@example.com>\nPre-Depends: evil")],
        ),
        ("app", &[("PRISM_APP_NAME", "App\rX")]),
        // Separators and odd punctuation in identifiers and the revision.
        ("app", &[("PRISM_BUNDLE_ID", "dev.prism.a pp")]),
        ("app", &[("PRISM_BUNDLE_ID", "../../tmp/app")]),
        ("app", &[("PRISM_BUNDLE_ID", "dev..app")]),
        ("../app", &[]),
        ("app", &[("PRISM_BUILD", "1/../../../x")]),
    ];
    for (pkg, env) in cases {
        let run = package(pkg, env);
        assert!(
            !launcher_runs_more(&run),
            "{pkg} {env:?}: the launcher ran a command"
        );
        assert!(!run.ok, "{pkg} {env:?}: packaged\n{}", run.stderr);
        assert!(!run.built(), "{pkg} {env:?}: built before checking");
        assert!(!run.stage().exists(), "{pkg} {env:?}: staged");
    }
}

#[test]
fn s23_display_strings_stay_literal_in_each_format() {
    let name = r#"Tom's "Best" Café \ $(no) `no` <&>"#;
    let description = r#"Counts things; really: $HOME \n 100% <ok> & "fine""#;
    let maintainer = "Tom O'Brien <tom@example.com>";
    let run = package(
        "my_app",
        &[
            ("PRISM_BUNDLE_ID", "com.example.my-app_2"),
            ("PRISM_APP_NAME", name),
            ("PRISM_DESCRIPTION", description),
            ("PRISM_MAINTAINER", maintainer),
            ("PRISM_VERSION", "1.2.0"),
            ("PRISM_BUILD", "3~beta+1"),
        ],
    );
    assert!(run.ok, "{}", run.stderr);

    // The launcher only holds identifiers, each quoted as a literal.
    assert_eq!(
        run.file("usr/bin/my-app"),
        r#"#!/bin/sh
# Starts the app: it finds its pictures in its assets folder, and desktops
# match its window to its desktop entry.
if [ -z "${PRISM_ASSETS_DIR:-}" ]; then PRISM_ASSETS_DIR='/usr/lib/my-app/assets'; fi
export PRISM_ASSETS_DIR
export PRISM_APP_ID='com.example.my-app_2'
exec '/usr/lib/my-app/my_app' "$@"
"#
    );
    assert!(!launcher_runs_more(&run));

    // Desktop entry: `\` is the escape character.
    let desktop = run.file("usr/share/applications/com.example.my-app_2.desktop");
    assert!(
        desktop.contains(r#"Name=Tom's "Best" Café \\ $(no) `no` <&>"#),
        "{desktop}"
    );
    assert!(
        desktop.contains(r#"Comment=Counts things; really: $HOME \\n 100% <ok> & "fine""#),
        "{desktop}"
    );
    assert!(desktop.contains("\nExec=my-app\n"), "{desktop}");
    assert!(desktop.contains("\nStartupWMClass=com.example.my-app_2\n"));

    // AppStream: XML text.
    let xml = run.file("usr/share/metainfo/com.example.my-app_2.metainfo.xml");
    assert!(
        xml.contains("<name>Tom's \"Best\" Café \\ $(no) `no` &lt;&amp;&gt;</name>"),
        "{xml}"
    );

    // Control file: one line per field.
    let control = run.file("DEBIAN/control");
    let fields: Vec<&str> = control
        .lines()
        .filter(|l| !l.starts_with(' '))
        .map(|l| l.split_once(':').unwrap().0)
        .collect();
    assert_eq!(
        fields,
        [
            "Package",
            "Version",
            "Architecture",
            "Maintainer",
            "Installed-Size",
            "Depends",
            "Section",
            "Priority",
            "Description"
        ],
        "{control}"
    );
    assert!(control.contains("\nVersion: 1.2.0-3~beta+1\n"), "{control}");
    assert!(control.contains(&format!("\nMaintainer: {maintainer}\n")));
    assert!(control.contains(&format!("\nDescription: {description}\n")));
}
