//! `prism build windows`: the app as a zip of one folder, its executable
//! (icon, version information and manifest inside, no console), the
//! Windows App SDK runtime, `resources.pri` and its assets. Built here on
//! Windows; elsewhere cross-compiled for `x86_64-pc-windows-gnu` with
//! MinGW-w64, on `PATH` or else in Docker (the `prism-windows-cross` image).
//!
//! `--msix`: an MSIX package instead (`msix.rs`), its executable without the
//! runtime's class registrations, which the Windows App Runtime framework
//! package it depends on provides.
//!
//! With `PRISM_WINDOWS_CERTIFICATE` (`winsign.rs`), the executable is
//! signed (Authenticode), and so is the MSIX.

use std::path::{Path, PathBuf};
use std::process::Command;

use prism_build::winappsdk;

use crate::Options;
use crate::config::Project;
use crate::msix;
use crate::winsign::{self, Certificate};
use crate::zip::ZipWriter;

const CROSS_IMAGE: &str = "prism-windows-cross";
/// Cargo's registry, shared with the Linux image's runs.
const REGISTRY_VOLUME: &str = "prism-cargo-registry";
/// The cross target when none is given off Windows.
const DEFAULT_CROSS: &str = "x86_64-pc-windows-gnu";

/// What `prism_build::link_app()` writes into the executable, from
/// Prism.toml: `PRISM_APP_NAME`, `PRISM_VERSION`, `PRISM_BUILD`,
/// `PRISM_BUNDLE_ID`, `PRISM_PUBLISHER`, `PRISM_DESCRIPTION`, `PRISM_ICON`;
/// and `PRISM_WNS_REMOTE_ID` (`[push] wns_remote_id`), which the Windows
/// backend reads.
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
    // Push (WNS) in an MSIX: the channel's remote id (`prism-windows`).
    if let Some(id) = &p.wns_remote_id {
        env.push(("PRISM_WNS_REMOTE_ID", id.clone()));
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

/// Builds and packages the app; returns the zip's (or the MSIX's) path.
pub fn build(p: &Project, o: &Options) -> Result<PathBuf, String> {
    let (triple, toolchain) = plan(o.target.as_deref(), cfg!(windows), runs)?;
    let certificate = Certificate::from_env()?;
    // Before the build: what Prism.toml says must fit an MSIX.
    if o.msix {
        msix::package_name(&p.bundle_id)?;
        if let Some(publisher) = &p.package_publisher {
            msix::check_publisher(publisher)
                .map_err(|e| format!("Prism.toml [windows] package_publisher: {e}"))?;
        }
    }
    let env = build_env(p, o.msix);
    let profile = match &toolchain {
        Toolchain::Host | Toolchain::Cross => {
            cargo_build(p, triple.as_deref(), &env)?;
            let base = match &triple {
                Some(t) => p.target_dir.join(t),
                None => p.target_dir.clone(),
            };
            base.join("release")
        }
        Toolchain::Docker => {
            let triple = triple.as_deref().unwrap_or(DEFAULT_CROSS);
            docker_build(p, triple, &env)?.join(triple).join("release")
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
    let exe_name = format!("{}.exe", p.package);
    let mut files: Vec<(PathBuf, PathBuf)> = vec![(exe_name.clone().into(), exe)];
    if !o.msix {
        let sdk = winappsdk::sdk_dir_for(&profile);
        let runtime = winappsdk::runtime_files(&sdk, arch);
        if runtime.is_empty() {
            return Err(format!(
                "no Windows App SDK runtime for {arch} in {}",
                sdk.display()
            ));
        }
        files.extend(runtime);
    }
    let mut asset_files = Vec::new();
    if let Some(dir) = &assets {
        list(dir, Path::new("assets"), &mut asset_files);
    }
    let has_assets = !asset_files.is_empty();
    files.extend(asset_files);

    let out = p.target_dir.join("windows");
    let base = format!(
        "{}-{}-windows-{}",
        p.package.replace('_', "-"),
        p.version,
        arch_name(arch)
    );
    // The files as they ship (the executable signed), in a folder of their
    // own: `<app name>` for the zip, `<base>-msix` for the package.
    let folder = if o.msix {
        format!("{base}-msix")
    } else {
        folder_name(&p.name, &p.package)
    };
    let stage = out.join(&folder);
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| format!("{}: {e}", stage.display()))?;
    for (rel, src) in &files {
        let dest = stage.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(src, &dest).map_err(|e| format!("{}: {e}", src.display()))?;
    }
    let staged_exe = stage.join(&exe_name);
    if let Some(cert) = &certificate {
        cert.sign(&staged_exe)?;
    }
    let signed = if certificate.is_some() {
        ", signed"
    } else {
        ""
    };
    if o.msix {
        let path = out.join(format!("{base}.msix"));
        let count = build_msix(p, arch, &stage, &files, certificate.as_ref(), &path)?;
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        println!(
            "Built {} ({:.1} MB, {count} files: {exe_name}, logos{}{signed}; needs the \
             {} {} framework package)",
            path.display(),
            size as f64 / 1e6,
            if has_assets { ", assets" } else { "" },
            winappsdk::FRAMEWORK_PACKAGE,
            winappsdk::FRAMEWORK_MIN_VERSION,
        );
        if certificate.is_none() {
            println!(
                "Unsigned: Windows installs signed packages only. Set {} and {} to sign it.",
                winsign::CERTIFICATE_VAR,
                winsign::PASSWORD_VAR
            );
        }
        return Ok(path);
    }
    let zip_path = out.join(format!("{base}.zip"));
    let file =
        std::fs::File::create(&zip_path).map_err(|e| format!("{}: {e}", zip_path.display()))?;
    let mut zip = ZipWriter::new(std::io::BufWriter::new(file));
    for (rel, src) in &files {
        let staged = stage.join(rel);
        let data = std::fs::read(&staged).map_err(|e| format!("{}: {e}", staged.display()))?;
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
        "Built {} ({:.1} MB, {} files: {exe_name}{signed}, the Windows App SDK runtime{})",
        zip_path.display(),
        size as f64 / 1e6,
        files.len(),
        if has_assets { ", assets" } else { "" }
    );
    println!("Unzipped: {}", stage.display());
    Ok(zip_path)
}

/// The cargo build's environment: [`app_env`], and
/// `PRISM_WINDOWS_PACKAGED=1` for an MSIX (the executable's manifest then
/// registers no runtime class, and no runtime is copied next to it).
fn build_env(p: &Project, msix: bool) -> Vec<(&'static str, String)> {
    let mut env = app_env(p);
    env.push((
        "PRISM_WINDOWS_PACKAGED",
        if msix { "1" } else { "0" }.into(),
    ));
    env
}

/// The MSIX's `Publisher`: the signing certificate's subject (read back
/// from the signed executable), which `[windows] package_publisher` must
/// then equal; else `package_publisher`, else `CN=<publisher or name>`.
fn package_publisher(p: &Project, signer: Option<&str>) -> Result<String, String> {
    match (signer, &p.package_publisher) {
        (Some(subject), Some(set)) if subject != set => Err(format!(
            "Prism.toml [windows] package_publisher is {set:?}, but the certificate's subject \
             is {subject:?}: an MSIX's publisher must be its signer (remove the setting or \
             make it equal)"
        )),
        (Some(subject), _) => Ok(subject.to_string()),
        (None, Some(set)) => Ok(set.clone()),
        (None, None) => Ok(msix::default_publisher(
            p.publisher.as_deref().unwrap_or(&p.name),
        )),
    }
}

/// `[permissions]` keys as the device capabilities an MSIX declares (the
/// privacy settings Windows asks for).
fn device_capabilities(p: &Project) -> Vec<String> {
    let mut caps = Vec::new();
    for (key, _) in &p.permissions {
        let cap = match key.as_str() {
            "location" | "location_always" => "location",
            "camera" => "webcam",
            "microphone" => "microphone",
            _ => continue,
        };
        if !caps.iter().any(|c| c == cap) {
            caps.push(cap.to_string());
        }
    }
    caps
}

/// The MSIX's version: `[app] version`'s three numbers and 0
/// (`1.2.0.0`). The Store wants the fourth part (the revision) to be 0,
/// so `[app] build` stays out of it (the executable's file version keeps
/// it), and each MSIX release needs a higher `[app] version`.
fn package_version(p: &Project) -> String {
    let info = prism_build::winres::AppInfo {
        version: p.version.clone(),
        ..Default::default()
    };
    let [a, b, c, _] = info.version_parts();
    format!("{a}.{b}.{c}.0")
}

/// The MSIX's manifest.
fn manifest(p: &Project, arch: &str, publisher: String) -> Result<msix::Manifest, String> {
    let name = msix::package_name(&p.bundle_id)?;
    Ok(msix::Manifest {
        toast_clsid: msix::class_id(&format!("{name}|{publisher}")),
        name,
        version: package_version(p),
        arch: arch_name(arch).to_string(),
        display_name: p.name.clone(),
        publisher_display_name: p.publisher.clone().unwrap_or_else(|| p.name.clone()),
        description: p.description.clone().unwrap_or_else(|| p.name.clone()),
        executable: format!("{}.exe", p.package),
        languages: p.languages.clone(),
        push_app_id: p.wns_app_id.clone(),
        device_capabilities: device_capabilities(p),
        publisher,
    })
}

/// Packs the staged files, the logos and the manifest into `path`, and
/// signs it; returns how many files it holds.
fn build_msix(
    p: &Project,
    arch: &str,
    stage: &Path,
    files: &[(PathBuf, PathBuf)],
    certificate: Option<&Certificate>,
    path: &Path,
) -> Result<usize, String> {
    let exe = stage.join(&files[0].0);
    let signer = match certificate {
        Some(_) => {
            let data = std::fs::read(&exe).map_err(|e| format!("{}: {e}", exe.display()))?;
            Some(winsign::signer_subject(&data)?)
        }
        None => None,
    };
    let publisher = package_publisher(p, signer.as_deref())?;
    msix::check_publisher(&publisher)?;
    let manifest = manifest(p, arch, publisher)?.xml();
    let icon = match &p.icon {
        Some(i) => Some(std::fs::read(i).map_err(|e| format!("{}: {e}", i.display()))?),
        None => None,
    };
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut pack = msix::Packer::new(std::io::BufWriter::new(file), std::time::SystemTime::now());
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    for (rel, _) in files {
        let data = std::fs::read(stage.join(rel)).map_err(|e| e.to_string())?;
        pack.add(&zip_name(rel), &data).map_err(err)?;
    }
    for (logo, size) in msix::LOGOS {
        let png = prism_build::winres::logo_png(icon.as_deref(), size)
            .map_err(|e| format!("the app icon: {e}"))?;
        pack.add(logo, &png).map_err(err)?;
    }
    pack.finish(&manifest)
        .and_then(|mut w| std::io::Write::flush(&mut w))
        .map_err(err)?;
    if let Some(cert) = certificate {
        cert.sign(path)?;
    }
    Ok(files.len() + msix::LOGOS.len() + 1)
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

/// `cargo build --release` here (on Windows, or with MinGW-w64).
fn cargo_build(
    p: &Project,
    triple: Option<&str>,
    env: &[(&'static str, String)],
) -> Result<(), String> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&p.root)
        .args(["build", "--release", "-p", &p.package]);
    if let Some(t) = triple {
        cmd.args(["--target", t]);
    }
    for (k, v) in env {
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
fn docker_build(
    p: &Project,
    triple: &str,
    env: &[(&'static str, String)],
) -> Result<PathBuf, String> {
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
        .args(docker_args(p, &out, triple, env)?)
        .status()
        .map_err(|e| format!("docker run: {e}"))?;
    if !status.success() {
        return Err(format!("the build in {CROSS_IMAGE} failed ({status})"));
    }
    Ok(out)
}

/// The `docker run` arguments of the cross build: the workspace at
/// `/src`, the target directory at `/out/target`.
fn docker_args(
    p: &Project,
    out: &Path,
    triple: &str,
    env: &[(&'static str, String)],
) -> Result<Vec<String>, String> {
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
    let mut env = env.to_vec();
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
    fn msix_publishers_and_capabilities() {
        let mut p = crate::linux::tests::project();
        assert_eq!(package_publisher(&p, None).unwrap(), "CN=My App");
        p.publisher = Some("Example, Inc.".into());
        assert_eq!(package_publisher(&p, None).unwrap(), "CN=\"Example, Inc.\"");
        p.package_publisher = Some("CN=Example, O=Example, C=FR".into());
        assert_eq!(
            package_publisher(&p, None).unwrap(),
            "CN=Example, O=Example, C=FR"
        );
        assert_eq!(
            package_publisher(&p, Some("CN=Example, O=Example, C=FR")).unwrap(),
            "CN=Example, O=Example, C=FR"
        );
        let err = package_publisher(&p, Some("CN=Other")).unwrap_err();
        assert!(err.contains("must be its signer"), "{err}");
        p.package_publisher = None;
        assert_eq!(package_publisher(&p, Some("CN=Other")).unwrap(), "CN=Other");

        p.permissions = [
            "camera",
            "face_id",
            "location",
            "location_always",
            "microphone",
        ]
        .map(|k| (k.to_string(), "why".to_string()))
        .to_vec();
        assert_eq!(
            device_capabilities(&p),
            ["webcam", "location", "microphone"]
        );

        p.wns_app_id = Some("7edfab6c-25ae-4678-b406-d1848f97919a".into());
        let m = manifest(&p, "win-arm64", "CN=Example".into()).unwrap();
        assert_eq!(m.name, "com.example.app");
        // The revision is 0 (the Store's rule), whatever `[app] build` is.
        assert_eq!(p.build.as_deref(), Some("7"));
        assert_eq!(m.version, "1.2.0.0");
        p.version = "2.10.3-beta.1".into();
        assert_eq!(package_version(&p), "2.10.3.0");
        assert_eq!(m.arch, "arm64");
        assert_eq!(m.executable, "my_app.exe");
        assert_eq!(m.publisher_display_name, "Example, Inc.");
        assert_eq!(m.toast_clsid, msix::class_id("com.example.app|CN=Example"));
        assert!(m.xml().contains("WindowsAppRuntimePushServer"));
    }

    #[test]
    fn cross_builds_in_the_image() {
        let mut p = crate::linux::tests::project();
        p.publisher = Some("Example".into());
        let args = docker_args(
            &p,
            Path::new("/real/out"),
            DEFAULT_CROSS,
            &build_env(&p, true),
        )
        .unwrap();
        let line = args.join(" ");
        for part in [
            "run --rm -v /w:/src -v /real/out:/out/target",
            "-e PRISM_APP_NAME=My App",
            "-e PRISM_PUBLISHER=Example",
            "-e PRISM_ICON=/src/examples/app/icon.png",
            "-e CARGO_TARGET_DIR=/out/target",
            "-e PRISM_WINDOWS_PACKAGED=1",
            "-w /src/examples/app prism-windows-cross cargo build --release -p my_app --target x86_64-pc-windows-gnu",
        ] {
            assert!(line.contains(part), "{part} in {line}");
        }
        assert_eq!(line.matches("PRISM_ICON").count(), 1);
    }
}
