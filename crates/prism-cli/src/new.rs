//! `prism new <name>`: a ready-to-run project (web, macOS, iOS, Android,
//! Linux, Windows).

use std::path::{Path, PathBuf};

/// The Prism checkout this CLI was built from, if it is still there.
fn default_prism() -> Option<PathBuf> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    repo.join("crates/prism/Cargo.toml")
        .exists()
        .then(|| repo.canonicalize().ok())
        .flatten()
}

/// A path for a TOML string: `/` separators (Windows takes them), without
/// the `\\?\` prefix `canonicalize` gives on Windows, whose `\` would be
/// escapes.
pub(crate) fn toml_path(path: &Path) -> String {
    let s = path.display().to_string();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    s.replace('\\', "/")
}

pub fn create(name: &str, prism: Option<PathBuf>) -> Result<(), String> {
    let dir = PathBuf::from(name);
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    let package = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("invalid name")?
        .replace('-', "_")
        .to_lowercase();
    if !package
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        || !package
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(format!("{name:?}: use letters, digits, - and _"));
    }
    // A Prism checkout (`--prism`, or the one this CLI was built from) gives
    // path dependencies; otherwise the crates.io release matching this CLI.
    let (prism_dep, build_dep) = match prism.or_else(default_prism) {
        Some(prism) => {
            let prism = prism
                .canonicalize()
                .map_err(|e| format!("{}: {e}", prism.display()))?;
            let prism = toml_path(&prism);
            (
                format!(
                    "{{ package = \"prismsdk\", path = \"{prism}/crates/prism\", default-features = false }}"
                ),
                format!(
                    "{{ package = \"prismsdk-build\", path = \"{prism}/crates/prism-build\" }}"
                ),
            )
        }
        None => {
            let version = env!("CARGO_PKG_VERSION");
            (
                format!(
                    "{{ package = \"prismsdk\", version = \"{version}\", default-features = false }}"
                ),
                format!("{{ package = \"prismsdk-build\", version = \"{version}\" }}"),
            )
        }
    };
    let title = {
        let mut c = package.split('_').map(|w| {
            let mut ch = w.chars();
            ch.next()
                .map(|f| f.to_uppercase().collect::<String>() + ch.as_str())
                .unwrap_or_default()
        });
        let first = c.next().unwrap_or_default();
        c.fold(first, |acc, w| acc + " " + &w)
    };
    let slug = package.replace('_', "-");

    let files: [(&str, String); 10] = [
        (
            "build.rs",
            "fn main() {\n    // The app binary's link settings (Apple: Swift's rpath, deployment target;\n    // Windows: the WinUI runtime next to it, its manifest, icon and version, no console).\n    prism_build::link_app();\n    // locales/*.json → typed `strings::…` functions, checked at build time.\n    prism_build::translations(\"locales\").fallback(\"en\").generate();\n}\n"
                .to_string(),
        ),
        (
            "locales/en.json",
            r#"{
  "hello": "Hello from Rust",
  "taps": { "one": "You tapped once.", "other": "You tapped {count} times." },
  "tap": "Tap me"
}
"#
            .to_string(),
        ),
        (
            "locales/fr.json",
            r#"{
  "hello": "Bonjour depuis Rust",
  "taps": { "one": "Tu as tapé {count} fois.", "other": "Tu as tapé {count} fois." },
  "tap": "Tape ici"
}
"#
            .to_string(),
        ),
        (
            "Cargo.toml",
            format!(
                r#"[package]
name = "{package}"
version = "0.1.0"
edition = "2024"
publish = false

[features]
default = ["native"]
native = ["prism/native"]
web = ["prism/web"]

[dependencies]
prism = {prism_dep}

[build-dependencies]
prism-build = {build_dep}
"#
            ),
        ),
        (
            "Prism.toml",
            format!(
                r#"[app]
name = "{title}"
bundle_id = "dev.prism.{slug}"
version = "0.1.0"
# icon = "icon.png"   # 1024×1024 PNG
# description = "What the app does"

# [windows]
# publisher = "Example Ltd"  # the executable's CompanyName

[ios]
# team = "ABCDE12345"        # Apple team for real devices
# device = "My iPhone"       # default to a device instead of a simulator
# deployment_target = "17.0" # oldest iOS/iPadOS (IPHONEOS_DEPLOYMENT_TARGET)

# Oldest system versions of the other platforms (these are the defaults; the
# environment variable, e.g. MACOSX_DEPLOYMENT_TARGET, wins when it is set).
# [macos]
# deployment_target = "13.0"   # MACOSX_DEPLOYMENT_TARGET
# [tvos]
# deployment_target = "17.0"   # TVOS_DEPLOYMENT_TARGET
# [watchos]
# deployment_target = "10.0"   # WATCHOS_DEPLOYMENT_TARGET
# [visionos]
# deployment_target = "26.0"   # XROS_DEPLOYMENT_TARGET
"#
            ),
        ),
        // The app lives in its library: Android loads it as a shared library.
        ("src/lib.rs", LIB_RS.replace("{title}", &title)),
        (
            "src/main.rs",
            format!(
                "fn main() {{\n    {}::run();\n}}\n",
                package.replace('-', "_")
            ),
        ),
        (
            "mise.toml",
            r#"[tools]
rust = { version = "1.99", targets = "aarch64-apple-darwin,aarch64-apple-ios,aarch64-apple-ios-sim,aarch64-linux-android" }
# Android: java = "temurin-21", android-sdk = "23.0", "cargo:cargo-ndk" = "4.1.2" (see Prism's README)

[tasks]
web = "prism dev web"
macos = "prism dev macos"
ios = "prism dev ios"
android = "prism dev android"
windows = "prism dev windows"
"#
            .to_string(),
        ),
        (".gitignore", "/target\n.prism-data/\n".to_string()),
        ("assets/.keep", String::new()),
    ];
    for (path, contents) in files {
        let dest = dir.join(path);
        std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&dest, contents).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    println!("Created {title} in {}/\n", dir.display());
    println!("  cd {name}");
    println!("  prism dev web      # http://localhost:3000, restarts on save");
    println!("  prism dev macos    # native window");
    println!("  prism dev ios      # iOS simulator (--device <name> for an iPhone)");
    println!("  prism dev android  # the running Android emulator or phone");
    println!("  prism dev windows  # native window (on Windows)");
    Ok(())
}

const LIB_RS: &str = r#"use prism::prelude::*;
use prism::ui::{self, Theme};

/// One function per key of locales/*.json (a typo is a compile error).
mod strings {
    prism::include_strings!();
}

/// The app (`main.rs` calls it; Android through `android_main!`).
pub fn run() {
    Application::new()
        .title("{title}")
        .assets(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"))
        .run(|cx| {
            cx.set_translations(strings::translations());
            Router::new()
                .route("/", |cx, _| cx.new_entity(|_| Home::default()))
                .build(cx)
        });
}

prism::android_main!(run);

#[derive(Default)]
struct Home {
    count: i64,
}

impl Render for Home {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        screen("{title}").bg(t.background).child(
            scroll().p(20.).gap(16.).child(
                ui::card()
                    .child(ui::heading(strings::hello(cx)))
                    .child(text(strings::taps(cx, self.count)).text_color(t.muted))
                    .child(ui::button(strings::tap(cx)).on_click(cx.listener(|this, _, _| this.count += 1))),
            ),
        )
    }
}
"#;

#[cfg(test)]
#[path = "new_tests.rs"]
mod tests;
