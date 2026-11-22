//! `prism build windows`: the app as a zip of one folder, its executable
//! (icon, version information and manifest inside, no console), the
//! Windows App SDK runtime, `resources.pri` and its assets. Built here on
//! Windows; elsewhere cross-compiled for `x86_64-pc-windows-gnu` with
//! MinGW-w64, on `PATH` or else in Docker (the `prism-windows-cross` image).

use std::path::{Path, PathBuf};
use std::process::Command;

use prism_build::winappsdk;

use crate::Options;
use crate::config::Project;
use crate::zip::ZipWriter;

const CROSS_IMAGE: &str = "prism-windows-cross";
/// Cargo's registry, shared with the Linux image's runs.
const REGISTRY_VOLUME: &str = "prism-cargo-registry";
/// The cross target when none is given off Windows.
const DEFAULT_CROSS: &str = "x86_64-pc-windows-gnu";

/// What `prism_build::link_app()` writes into the executable, from
/// Prism.toml: `PRISM_APP_NAME`, `PRISM_VERSION`, `PRISM_BUILD`,
/// `PRISM_BUNDLE_ID`, `PRISM_PUBLISHER`, `PRISM_DESCRIPTION`, `PRISM_ICON`.
pub fn app_env(p: &Project) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("PRISM_APP_NAME", p.name.clone()),
        ("PRISM_VERSION", p.version.clone()),
        ("PRISM_BUNDLE_ID", p.bundle_id.clone()),
    ];
    if let Some(b) = &p.build {
        env.push(("PRISM_BUILD", b.clone()));
    }
    if let Some(publisher) = &p.publisher {
        env.push(("PRISM_PUBLISHER", publisher.clone()));
    }
    if let Some(d) = &p.description {
        env.push(("PRISM_DESCRIPTION", d.clone()));
    }
    if let Some(icon) = &p.icon {
        env.push(("PRISM_ICON", icon.display().to_string()));
    }
    env
}

/// How the executable gets built.
#[derive(Debug, PartialEq, Eq)]
enum Toolchain {
    /// `cargo build` here, on Windows.
    Host,
    /// Cross-compiled here with MinGW-w64 from `PATH`.
    Cross,
    /// Cross-compiled in the `prism-windows-cross` image.
    Docker,
}

/// The short architecture name in file names: `x64`, `arm64`, `x86`.
fn arch_name(sdk_arch: &str) -> &str {
    sdk_arch.strip_prefix("win-").unwrap_or(sdk_arch)
}

fn runs(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The MinGW-w64 C compiler cargo links `triple` with.
fn mingw_compiler(triple: &str) -> String {
    let arch = match triple.split('-').next().unwrap_or_default() {
        "i586" | "i686" => "i686",
        a => a,
    };
    let llvm = triple.ends_with("gnullvm");
    format!("{arch}-w64-mingw32-{}", if llvm { "clang" } else { "gcc" })
}

/// The Rust target, and how to build for it.
fn plan(
    target: Option<&str>,
    on_windows: bool,
    has: impl Fn(&str) -> bool,
) -> Result<(Option<String>, Toolchain), String> {
    if on_windows {
        return Ok((target.map(str::to_string), Toolchain::Host));
    }
    let triple = target.unwrap_or(DEFAULT_CROSS).to_string();
    if !triple.contains("-windows-") {
        return Err(format!(
            "{triple} is not a Windows target (x86_64-pc-windows-gnu, aarch64-pc-windows-gnullvm…)"
        ));
    }
    if triple.ends_with("-msvc") {
        return Err(format!(
            "{triple}: MSVC builds need Windows (link.exe and the Windows SDK); from here, use a -gnu or -gnullvm target"
        ));
    }
    let compiler = mingw_compiler(&triple);
    if has(&compiler) {
        return Ok((Some(triple), Toolchain::Cross));
    }
    if triple == DEFAULT_CROSS {
        return Ok((Some(triple), Toolchain::Docker));
    }
    Err(format!(
        "{triple} needs {compiler} on PATH (llvm-mingw for -gnullvm targets); the Docker image only builds {DEFAULT_CROSS}"
    ))
}

/// Builds and packages the app; returns the zip's path.
pub fn build(p: &Project, o: &Options) -> Result<PathBuf, String> {
    let (triple, toolchain) = plan(o.target.as_deref(), cfg!(windows), runs)?;
    let profile = match &toolchain {
        Toolchain::Host | Toolchain::Cross => {
            cargo_build(p, triple.as_deref())?;
            let base = match &triple {
                Some(t) => p.target_dir.join(t),
                None => p.target_dir.clone(),
            };
            base.join("release")
        }
        Toolchain::Docker => {
            let triple = triple.as_deref().unwrap_or(DEFAULT_CROSS);
            docker_build(p, triple)?.join(triple).join("release")
        }
    };
    let rust_arch = triple
        .as_deref()
        .unwrap_or(std::env::consts::ARCH)
        .to_string();
    let arch = winappsdk::arch_of(&rust_arch);
    let exe = profile.join(format!("{}.exe", p.package));
    if !exe.is_file() {
        return Err(format!("{} was not built", exe.display()));
    }
    // The app says where its assets are (`PRISM_DESCRIBE`) when it runs
    // here; a cross-compiled one, or one for another architecture, cannot:
    // the project's `assets` folder.
    let assets = (toolchain == Toolchain::Host)
        .then(|| describe_assets(&exe))
        .flatten()
        .or_else(|| Some(p.root.join("assets")).filter(|d| d.is_dir()));
    let sdk = winappsdk::sdk_dir_for(&profile);
    let runtime = winappsdk::runtime_files(&sdk, arch);
    if runtime.is_empty() {
        return Err(format!(
            "no Windows App SDK runtime for {arch} in {}",
            sdk.display()
        ));
    }

    let out = p.target_dir.join("windows");
    let folder = folder_name(&p.name, &p.package);
    let stage = out.join(&folder);
    let _ = std::fs::remove_dir_all(&stage);
    let mut files: Vec<(PathBuf, PathBuf)> = vec![(format!("{}.exe", p.package).into(), exe)];
    files.extend(runtime);
    let mut asset_files = Vec::new();
    if let Some(dir) = &assets {
        list(dir, Path::new("assets"), &mut asset_files);
    }
    let has_assets = !asset_files.is_empty();
    files.extend(asset_files);
    let zip_path = out.join(format!(
        "{}-{}-windows-{}.zip",
        p.package.replace('_', "-"),
        p.version,
        arch_name(arch)
    ));
    std::fs::create_dir_all(&stage).map_err(|e| format!("{}: {e}", stage.display()))?;
    let file =
        std::fs::File::create(&zip_path).map_err(|e| format!("{}: {e}", zip_path.display()))?;
    let mut zip = ZipWriter::new(std::io::BufWriter::new(file));
    for (rel, src) in &files {
        let dest = stage.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let data = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
        std::fs::write(&dest, &data).map_err(|e| format!("{}: {e}", dest.display()))?;
        let modified = std::fs::metadata(src)
            .and_then(|m| m.modified())
            .unwrap_or_else(|_| std::time::SystemTime::now());
        let name = format!("{folder}/{}", zip_name(rel));
        zip.add(&name, &data, modified)
            .map_err(|e| format!("{}: {e}", zip_path.display()))?;
    }
    zip.finish()
        .and_then(|mut w| std::io::Write::flush(&mut w))
        .map_err(|e| format!("{}: {e}", zip_path.display()))?;
    let size = std::fs::metadata(&zip_path).map(|m| m.len()).unwrap_or(0);
    println!(
        "Built {} ({:.1} MB, {} files: {}.exe, the Windows App SDK runtime{})",
        zip_path.display(),
        size as f64 / 1e6,
        files.len(),
        p.package,
        if has_assets { ", assets" } else { "" }
    );
    println!("Unzipped: {}", stage.display());
    Ok(zip_path)
}

/// The folder inside the zip: the app's name without the characters
/// Windows forbids in file names.
fn folder_name(name: &str, package: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| !c.is_control() && !r#"<>:"/\|?*"#.contains(*c))
        .collect();
    let clean = clean.trim().trim_end_matches('.').to_string();
    if clean.is_empty() {
        package.to_string()
    } else {
        clean
    }
}

/// `a\b` → `a/b`.
fn zip_name(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn list(dir: &Path, rel: &Path, out: &mut Vec<(PathBuf, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = e.path();
        if path.is_dir() {
            list(&path, &rel.join(&name), out);
        } else {
            out.push((rel.join(&name), path));
        }
    }
}

/// `PRISM_DESCRIBE=assets`: the app's assets folder, if it has one.
fn describe_assets(exe: &Path) -> Option<PathBuf> {
    let out = Command::new(exe)
        .env("PRISM_DESCRIBE", "assets")
        .output()
        .ok()?;
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(PathBuf::from(dir)).filter(|d| out.status.success() && d.is_dir())
}

/// The rustflags that link LLVM's unwinder into a `*-gnullvm` app
/// (`+crt-static`), unless the caller set its own (`has(var)`): linked
/// dynamically, the app needs llvm-mingw's `libunwind.dll`, which no PC
/// has, and does not start (0xC0000135). The C runtime stays the system's
/// UCRT.
fn static_unwind(triple: &str, has: impl Fn(&str) -> bool) -> Option<(String, String)> {
    if !triple.ends_with("-gnullvm") {
        return None;
    }
    let var = format!(
        "CARGO_TARGET_{}_RUSTFLAGS",
        triple.to_uppercase().replace('-', "_")
    );
    if has("RUSTFLAGS") || has("CARGO_ENCODED_RUSTFLAGS") || has(&var) {
        return None;
    }
    Some((var, "-C target-feature=+crt-static".into()))
}

/// `cargo build --release` here (on Windows, or with MinGW-w64).
fn cargo_build(p: &Project, triple: Option<&str>) -> Result<(), String> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&p.root)
        .args(["build", "--release", "-p", &p.package]);
    if let Some(t) = triple {
        cmd.args(["--target", t]);
        if let Some((k, v)) = static_unwind(t, |k| std::env::var_os(k).is_some()) {
            cmd.env(k, v);
        }
    }
    for (k, v) in app_env(p) {
        cmd.env(k, v);
    }
    let status = cmd.status().map_err(|e| format!("cargo: {e}"))?;
    if status.success() {
        return Ok(());
    }
    Err(match triple {
        Some(t) if !cfg!(windows) => {
            format!("build failed (is the target installed? rustup target add {t})")
        }
        _ => "build failed".into(),
    })
}

/// Builds in the `prism-windows-cross` image (Rust and MinGW-w64), built
/// if missing; returns the target directory (on this machine).
fn docker_build(p: &Project, triple: &str) -> Result<PathBuf, String> {
    let platform = crate::embedded::extract(&p.target_dir)?;
    let has_image = Command::new("docker")
        .args(["image", "inspect", CROSS_IMAGE])
        .output()
        .map_err(|e| {
            format!(
                "docker: {e} (Windows apps are cross-compiled with MinGW-w64: install it, \
                 e.g. `brew install mingw-w64` or `apt install gcc-mingw-w64-x86-64`, \
                 with `rustup target add {triple}`, or Docker)"
            )
        })?
        .status
        .success();
    let docker = platform.join("docker");
    if !has_image {
        println!("Building the {CROSS_IMAGE} image (Rust + MinGW-w64)…");
        let status = Command::new("docker")
            .args(["build", "-t", CROSS_IMAGE, "-f"])
            .arg(docker.join("windows-cross.Dockerfile"))
            .arg(&docker)
            .status()
            .map_err(|e| format!("docker build: {e}"))?;
        if !status.success() {
            return Err(format!("docker build failed ({status})"));
        }
    }
    // Beside target/windows, whose folders are named after apps.
    let out = p.target_dir.join("windows-cross");
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    // Docker mounts real paths: target/ may be a symlink.
    let out = out.canonicalize().map_err(|e| e.to_string())?;
    let status = Command::new("docker")
        .args(docker_args(p, &out, triple)?)
        .status()
        .map_err(|e| format!("docker run: {e}"))?;
    if !status.success() {
        return Err(format!("the build in {CROSS_IMAGE} failed ({status})"));
    }
    Ok(out)
}

/// The `docker run` arguments of the cross build: the workspace at
/// `/src`, the target directory at `/out/target`.
fn docker_args(p: &Project, out: &Path, triple: &str) -> Result<Vec<String>, String> {
    let mut args: Vec<String> = vec!["run".into(), "--rm".into()];
    for (host, guest) in [(p.workspace_root.as_path(), "/src"), (out, "/out/target")] {
        args.push("-v".into());
        args.push(format!("{}:{guest}", host.display()));
    }
    args.extend([
        "-v".into(),
        format!("{REGISTRY_VOLUME}:/usr/local/cargo/registry"),
    ]);
    let project = crate::linux::in_container(&p.workspace_root, &p.root)?;
    let mut env = app_env(p);
    if let Some(icon) = &p.icon {
        let icon = crate::linux::in_container(&p.workspace_root, icon)?;
        env.retain(|(k, _)| *k != "PRISM_ICON");
        env.push(("PRISM_ICON", icon));
    }
    env.push(("CARGO_TARGET_DIR", "/out/target".into()));
    for (k, v) in env {
        args.push("-e".into());
        args.push(format!("{k}={v}"));
    }
    args.extend(["-w".into(), project, CROSS_IMAGE.into()]);
    args.extend(
        [
            "cargo",
            "build",
            "--release",
            "-p",
            &p.package,
            "--target",
            triple,
        ]
        .map(String::from),
    );
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_the_build() {
        let none = |_: &str| false;
        let gcc = |t: &str| t == "x86_64-w64-mingw32-gcc";
        assert_eq!(plan(None, true, none), Ok((None, Toolchain::Host)));
        assert_eq!(
            plan(Some("aarch64-pc-windows-msvc"), true, none),
            Ok((Some("aarch64-pc-windows-msvc".into()), Toolchain::Host))
        );
        assert_eq!(
            plan(None, false, gcc),
            Ok((Some(DEFAULT_CROSS.into()), Toolchain::Cross))
        );
        assert_eq!(
            plan(None, false, none),
            Ok((Some(DEFAULT_CROSS.into()), Toolchain::Docker))
        );
        assert!(plan(Some("x86_64-pc-windows-msvc"), false, gcc).is_err());
        assert!(plan(Some("aarch64-apple-darwin"), false, gcc).is_err());
        let clang = |t: &str| t == "aarch64-w64-mingw32-clang";
        assert_eq!(
            plan(Some("aarch64-pc-windows-gnullvm"), false, clang),
            Ok((Some("aarch64-pc-windows-gnullvm".into()), Toolchain::Cross))
        );
        assert!(plan(Some("aarch64-pc-windows-gnullvm"), false, none).is_err());
    }

    #[test]
    fn llvm_builds_carry_their_unwinder() {
        let none = |_: &str| false;
        assert_eq!(
            static_unwind("i686-pc-windows-gnullvm", none),
            Some((
                "CARGO_TARGET_I686_PC_WINDOWS_GNULLVM_RUSTFLAGS".into(),
                "-C target-feature=+crt-static".into()
            ))
        );
        assert!(static_unwind("aarch64-pc-windows-gnullvm", none).is_some());
        assert_eq!(static_unwind("x86_64-pc-windows-gnu", none), None);
        // The caller's flags win.
        assert_eq!(
            static_unwind("i686-pc-windows-gnullvm", |k| k == "RUSTFLAGS"),
            None
        );
    }

    #[test]
    fn names() {
        assert_eq!(folder_name("My App: Pro?", "my_app"), "My App Pro");
        assert_eq!(folder_name(" .. ", "my_app"), "my_app");
        assert_eq!(
            zip_name(&Path::new("assets").join("fonts").join("a.ttf")),
            "assets/fonts/a.ttf"
        );
        assert_eq!(arch_name("win-arm64"), "arm64");
        assert_eq!(
            mingw_compiler("i686-pc-windows-gnu"),
            "i686-w64-mingw32-gcc"
        );
    }

    #[test]
    fn cross_builds_in_the_image() {
        let mut p = crate::linux::tests::project();
        p.publisher = Some("Example".into());
        let args = docker_args(&p, Path::new("/real/out"), DEFAULT_CROSS).unwrap();
        let line = args.join(" ");
        for part in [
            "run --rm -v /w:/src -v /real/out:/out/target",
            "-e PRISM_APP_NAME=My App",
            "-e PRISM_PUBLISHER=Example",
            "-e PRISM_ICON=/src/examples/app/icon.png",
            "-e CARGO_TARGET_DIR=/out/target",
            "-w /src/examples/app prism-windows-cross cargo build --release -p my_app --target x86_64-pc-windows-gnu",
        ] {
            assert!(line.contains(part), "{part} in {line}");
        }
        assert_eq!(line.matches("PRISM_ICON").count(), 1);
    }
}
