//! `prism new <name>`: a ready-to-run project (web, macOS, iOS).

use std::path::{Path, PathBuf};

/// The Prism checkout this CLI was built from, if it is still there.
fn default_prism() -> Option<PathBuf> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    repo.join("crates/prism/Cargo.toml")
        .exists()
        .then(|| repo.canonicalize().ok())
        .flatten()
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
    let prism = prism
        .or_else(default_prism)
        .ok_or("where is Prism? pass --prism <path to the Prism repository>")?;
    let prism = prism
        .canonicalize()
        .map_err(|e| format!("{}: {e}", prism.display()))?;
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
            "fn main() {\n    // locales/*.json → typed `strings::…` functions, checked at build time.\n    prism_build::translations(\"locales\").fallback(\"en\").generate();\n}\n"
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
prism = {{ path = "{prism}/crates/prism", default-features = false }}

[build-dependencies]
prism-build = {{ path = "{prism}/crates/prism-build" }}
"#,
                prism = prism.display()
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

[ios]
# team = "ABCDE12345"        # Apple team for real devices
# device = "My iPhone"       # default to a device instead of a simulator
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
