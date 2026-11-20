//! The Windows App SDK (WinUI 3) that Prism's Windows backend uses,
//! self-contained, without MSBuild, NuGet or MSVC:
//!
//! - [`ensure`] fetches its component packages from nuget.org into a cache
//!   once (`PRISM_WINAPPSDK_DIR`, else `<target dir>/winappsdk/<version>`),
//!   with `curl` and `tar` (both part of Windows 10 and 11; `unzip` too
//!   elsewhere);
//! - [`deploy`] copies its runtime (DLLs, `resources.pri`) next to the
//!   app's executable (about 55 MB, x64); [`runtime_files`] lists it;
//! - [`embed_resources`] gives the executable its manifest, which registers
//!   the runtime's WinRT classes (reg-free activation, what the SDK's
//!   MSBuild targets generate), DPI awareness and the Windows versions XAML
//!   islands need, its icon and its version information
//!   ([`winres`](crate::winres));
//! - [`gui_subsystem`] makes release builds Windows (GUI) programs, which
//!   open no console.
//!
//! [`link_app`] does all of this for the app's own build script
//! ([`crate::link_app`] calls it for Windows targets).
//!
//! `prismsdk-windows`' build script uses the same cache for its WinUI
//! bindings.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::winres::{self, AppInfo};

/// The Windows App SDK release the backend is built against.
pub const VERSION: &str = "2.7.0";

/// Its component packages the backend needs: metadata and runtime.
pub const PACKAGES: [&str; 3] = ["foundation", "interactiveexperiences", "winui"];

/// The build profile's directory (`<target>[/<triple>]/<profile>`), from a
/// build script's `OUT_DIR`: where the executables go.
pub fn profile_dir() -> PathBuf {
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR (run from a build script)"));
    // OUT_DIR is <profile>/build/<pkg>-<hash>/out.
    out.ancestors()
        .nth(3)
        .expect("target profile directory")
        .to_path_buf()
}

/// Where the packages are cached.
pub fn sdk_dir() -> PathBuf {
    println!("cargo:rerun-if-env-changed=PRISM_WINAPPSDK_DIR");
    sdk_dir_for(&profile_dir())
}

/// Where the packages are cached for a build profile's directory
/// (`<target>[/<triple>]/<profile>`): `PRISM_WINAPPSDK_DIR`, else
/// `<target>[/<triple>]/winappsdk/<version>`.
pub fn sdk_dir_for(profile_dir: &Path) -> PathBuf {
    match std::env::var_os("PRISM_WINAPPSDK_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => profile_dir
            .parent()
            .expect("target directory")
            .join("winappsdk")
            .join(VERSION),
    }
}

/// The packages in `sdk`, fetched if missing.
pub fn ensure(sdk: &Path) {
    for p in PACKAGES {
        fetch(sdk, p);
    }
}

/// `runtimes-framework/<arch>` for the target.
pub fn arch() -> &'static str {
    arch_of(&std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default())
}

/// `runtimes-framework/<arch>` for a Rust architecture (`x86_64`,
/// `aarch64`, `x86`) or target triple.
pub fn arch_of(rust_arch: &str) -> &'static str {
    match rust_arch.split('-').next().unwrap_or_default() {
        "aarch64" | "arm64ec" => "win-arm64",
        "x86" | "i586" | "i686" => "win-x86",
        _ => "win-x64",
    }
}

/// Downloads and unpacks one package into `<sdk>/<name>` unless it is there.
fn fetch(sdk: &Path, name: &str) {
    let dest = sdk.join(name);
    if dest.join("runtimes-framework").is_dir() {
        return;
    }
    fs::create_dir_all(sdk).expect("Windows App SDK cache directory");
    let id = format!("microsoft.windowsappsdk.{name}");
    let url = format!("https://api.nuget.org/v3-flatcontainer/{id}/{VERSION}/{id}.{VERSION}.nupkg");
    // Named after this process: the backend's build script and the app's
    // (`link_app`) may fetch at the same time on a first build.
    let tag = std::process::id();
    let pkg = sdk.join(format!("{id}.{VERSION}.{tag}.zip"));
    println!(
        "cargo:warning=fetching {url} (once, into {})",
        sdk.display()
    );
    let ok = Command::new("curl")
        .args(["-sSfL", "--retry", "3", "-o"])
        .arg(&pkg)
        .arg(&url)
        .status()
        .is_ok_and(|s| s.success());
    assert!(
        ok,
        "prism: could not download {url} with curl. Download the Windows App SDK packages \
         ({}) version {VERSION} from nuget.org, unpack each into <dir>/<name>, and set \
         PRISM_WINAPPSDK_DIR=<dir>.",
        PACKAGES.join(", ")
    );
    // Unpacked beside, then renamed: an interrupted build leaves no half
    // package behind.
    let tmp = sdk.join(format!("{name}.partial.{tag}"));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("unpack directory");
    let unpacked = Command::new("tar")
        .arg("-xf")
        .arg(&pkg)
        .arg("-C")
        .arg(&tmp)
        .status()
        .is_ok_and(|s| s.success())
        || Command::new("unzip")
            .args(["-q", "-o"])
            .arg(&pkg)
            .arg("-d")
            .arg(&tmp)
            .status()
            .is_ok_and(|s| s.success());
    assert!(unpacked, "prism: could not unpack {}", pkg.display());
    let _ = fs::remove_file(&pkg);
    let complete = |d: &Path| d.join("runtimes-framework").is_dir();
    if !complete(&dest) {
        let _ = fs::remove_dir_all(&dest);
        // Another build script may have renamed its copy in between.
        if fs::rename(&tmp, &dest).is_err() && !complete(&dest) {
            panic!(
                "prism: could not move the Windows App SDK package to {}",
                dest.display()
            );
        }
    }
    let _ = fs::remove_dir_all(&tmp);
}

/// The `<file>` entries registering the runtime's WinRT classes, from each
/// package's `package.appxfragment`.
pub fn classes(sdk: &Path) -> String {
    let mut files = String::new();
    for p in PACKAGES {
        let path = sdk.join(p).join("runtimes-framework/package.appxfragment");
        let fragment =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for server in fragment.split("<InProcessServer>").skip(1) {
            let server = server
                .split("</InProcessServer>")
                .next()
                .unwrap_or_default();
            let Some(dll) = between(server, "<Path>", "</Path>") else {
                continue;
            };
            let _ = writeln!(files, "  <asmv3:file name='{dll}'>");
            for class in server.split("ActivatableClassId=\"").skip(1) {
                let name = class.split('"').next().unwrap_or_default();
                let _ = writeln!(
                    files,
                    "    <winrtv1:activatableClass name='{name}' threadingModel='both'/>"
                );
            }
            files.push_str("  </asmv3:file>\n");
        }
    }
    files
}

fn between<'a>(s: &'a str, a: &str, b: &str) -> Option<&'a str> {
    let start = s.find(a)? + a.len();
    Some(&s[start..start + s[start..].find(b)?])
}

/// Text for XML.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The app's manifest: its identity (the app's id and version) and
/// description (its name), Windows 10/11 (XAML islands need
/// `maxversiontested` ≥ 10.0.18362), no elevation (`asInvoker`, what
/// MinGW's and MSVC's default manifests say), per-monitor DPI v2, UTF-8,
/// the common controls v6, and the runtime's classes.
pub fn app_manifest(sdk: &Path, app: &AppInfo) -> String {
    manifest_with(&classes(sdk), app)
}

fn manifest_with(files: &str, app: &AppInfo) -> String {
    let name = xml_escape(&app.assembly_name());
    let version = app.version_quad();
    let description = xml_escape(&app.name);
    format!(
        r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<assembly manifestVersion="1.0" xmlns="urn:schemas-microsoft-com:asm.v1"
  xmlns:asmv3="urn:schemas-microsoft-com:asm.v3" xmlns:winrtv1="urn:schemas-microsoft-com:winrt.v1">
  <assemblyIdentity type="win32" name="{name}" version="{version}"/>
  <description>{description}</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges>
      <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
    </requestedPrivileges></security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
      <maxversiontested Id="10.0.26100.0"/>
    </application>
  </compatibility>
  <asmv3:application>
    <asmv3:windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </asmv3:windowsSettings>
  </asmv3:application>
  <dependency><dependentAssembly>
    <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
      processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
  </dependentAssembly></dependency>
{files}</assembly>
"#
    )
}

/// Embeds the app's resources in the package's executables: its manifest
/// (resource 1, `RT_MANIFEST`), its icon (`PRISM_ICON`, a PNG or an
/// `.ico`) and its version information, written as one `.res` file
/// ([`winres`](crate::winres)) and linked by the target's toolchain:
///
/// - MSVC: `link.exe` takes the `.res` itself (it converts it with
///   `cvtres`), with `/MANIFEST:NO` so it adds no manifest of its own;
/// - GNU (MinGW): `windres` (MinGW's binutils; `<arch>-w64-mingw32-windres`
///   when it exists, or `WINDRES`) turns it into an object named like
///   MinGW's own default manifest, found first with `-B`, so the
///   executable gets one manifest, not two;
/// - GNU with LLVM (`*-gnullvm`): `lld` takes the `.res` itself (LLVM's
///   MinGW links no default manifest).
pub fn embed_resources(sdk: &Path, app: &AppInfo) {
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let manifest = app_manifest(sdk, app);
    // Written beside, for reading.
    let _ = fs::write(out.join("prism-app.manifest"), &manifest);
    let mut resources = Vec::new();
    if let Some(icon) = &app.icon {
        println!("cargo:rerun-if-changed={}", icon.display());
        let images = fs::read(icon)
            .map_err(|e| e.to_string())
            .and_then(|file| winres::icon(&file));
        match images {
            Ok(images) => resources.extend(images),
            Err(e) => panic!("prism: the app icon {}: {e}", icon.display()),
        }
    }
    resources.push(winres::Resource {
        kind: winres::RT_VERSION,
        id: 1,
        data: winres::version_info(app),
    });
    resources.push(winres::Resource::manifest(&manifest));
    let res = out.join("prism-app.res");
    fs::write(&res, winres::res_file(&resources)).expect("resource file");

    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_abi = std::env::var("CARGO_CFG_TARGET_ABI").unwrap_or_default();
    if target_env == "msvc" {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
        println!("cargo:rustc-link-arg-bins={}", res.display());
        return;
    }
    if target_abi == "llvm" {
        println!("cargo:rustc-link-arg-bins={}", res.display());
        return;
    }
    let obj = out.join("default-manifest.o");
    let windres = windres();
    let status = Command::new(&windres)
        .args(["--input-format=res", "--output-format=coff", "--input"])
        .arg(&res)
        .arg("--output")
        .arg(&obj)
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg-bins=-B{}", out.display());
        }
        _ => println!(
            "cargo:warning=prism: {windres} (MinGW binutils) is needed to embed the app's \
             manifest, icon and version: without the manifest WinUI cannot start. Install \
             MinGW-w64 or set WINDRES."
        ),
    }
}

/// The `windres` to run: `WINDRES`, else the target's
/// (`x86_64-w64-mingw32-windres`: cross-compilers have only this one,
/// Windows' MinGW distributions both), else `windres`.
fn windres() -> String {
    println!("cargo:rerun-if-env-changed=WINDRES");
    if let Ok(w) = std::env::var("WINDRES") {
        return w;
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => "i686".to_string(),
        Ok(a) => a.to_string(),
        Err(_) => "x86_64".to_string(),
    };
    let prefixed = format!("{arch}-w64-mingw32-windres");
    let runs = Command::new(&prefixed)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if runs { prefixed } else { "windres".into() }
}

/// Release builds (cargo's `PROFILE` is `release`) are Windows programs
/// (the GUI subsystem: no console window opens with the app); debug builds
/// stay console programs, so `cargo run` and `prism run windows` show the
/// app's logs. `PRISM_WINDOWS_SUBSYSTEM=windows` or `console` chooses. The
/// app needs no `#![windows_subsystem]`. GNU: `-Wl,--subsystem,windows`;
/// MSVC: `/SUBSYSTEM:WINDOWS` with `/ENTRY:mainCRTStartup` (Rust's `main`,
/// not `WinMain`), what rustc passes for `#![windows_subsystem = "windows"]`.
pub fn gui_subsystem() {
    println!("cargo:rerun-if-env-changed=PRISM_WINDOWS_SUBSYSTEM");
    let gui = match std::env::var("PRISM_WINDOWS_SUBSYSTEM").as_deref() {
        Ok("windows") => true,
        Ok("console") => false,
        _ => std::env::var("PROFILE").as_deref() == Ok("release"),
    };
    if !gui {
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/SUBSYSTEM:WINDOWS");
        println!("cargo:rustc-link-arg-bins=/ENTRY:mainCRTStartup");
    } else {
        println!("cargo:rustc-link-arg-bins=-Wl,--subsystem,windows");
    }
}

/// The runtime the app needs next to it, for `arch` (`win-x64`,
/// `win-arm64`, `win-x86`): each file's path there and its source in the
/// cache, `resources.pri` included. Unpackaged apps resolve `ms-appx:///`
/// through `resources.pri` next to the exe; MSBuild merges the
/// components' PRIs into it (makepri), but the controls' one alone is
/// enough for `XamlControlsResources`.
pub fn runtime_files(sdk: &Path, arch: &str) -> Vec<(PathBuf, PathBuf)> {
    let mut files = Vec::new();
    for p in PACKAGES {
        let root = sdk
            .join(p)
            .join("runtimes-framework")
            .join(arch)
            .join("native");
        list_tree(&root, Path::new(""), &mut files);
    }
    let controls = files
        .iter()
        .find(|(rel, _)| rel.as_os_str() == "Microsoft.UI.Xaml.Controls.pri")
        .map(|(_, src)| src.clone());
    if let Some(controls) = controls {
        files.push(("resources.pri".into(), controls));
    }
    files
}

fn list_tree(dir: &Path, rel: &Path, out: &mut Vec<(PathBuf, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let r = rel.join(e.file_name());
        if p.is_dir() {
            list_tree(&p, &r, out);
        } else {
            out.push((r, p));
        }
    }
}

/// The runtime next to the executables in `dir`.
pub fn deploy(sdk: &Path, dir: &Path) {
    let _ = fs::create_dir_all(dir);
    for (rel, src) in runtime_files(sdk, arch()) {
        let dest = dir.join(rel);
        if let Some(parent) = dest.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if !same_size(&src, &dest) {
            // A running app keeps its DLLs locked: copied on a later build.
            let _ = fs::copy(&src, &dest);
        }
    }
}

fn same_size(a: &Path, b: &Path) -> bool {
    let len = |p: &Path| fs::metadata(p).map(|m| m.len()).ok();
    len(a).is_some() && len(a) == len(b)
}

/// What [`link_app`](crate::link_app) does for a Windows app built with
/// the native backend: the runtime next to it; the manifest, the icon and
/// the version information in it ([`AppInfo::from_env`]); the GUI
/// subsystem for release builds.
pub fn link_app() {
    // A web-only build (`--no-default-features --features web`) runs no
    // WinUI.
    let web_only = std::env::var_os("CARGO_FEATURE_WEB").is_some()
        && std::env::var_os("CARGO_FEATURE_NATIVE").is_none();
    if web_only {
        return;
    }
    println!("cargo:rerun-if-env-changed=PRISM_WINAPPSDK_NO_COPY");
    let sdk = sdk_dir();
    ensure(&sdk);
    embed_resources(&sdk, &AppInfo::from_env());
    gui_subsystem();
    if std::env::var_os("PRISM_WINAPPSDK_NO_COPY").is_none() {
        let dir = profile_dir();
        deploy(&sdk, &dir);
        if dir.join("examples").is_dir() {
            deploy(&sdk, &dir.join("examples"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_name_the_app() {
        let app = AppInfo {
            name: "Tom & Jerry".into(),
            version: "1.4.0".into(),
            build: Some("12".into()),
            id: "com.example.tom-jerry".into(),
            package: "tom_jerry".into(),
            ..AppInfo::default()
        };
        let m = manifest_with("  <asmv3:file name='x.dll'/>\n", &app);
        assert!(m.contains(
            r#"<assemblyIdentity type="win32" name="com.example.tom-jerry" version="1.4.0.12"/>"#
        ));
        assert!(m.contains("<description>Tom &amp; Jerry</description>"));
        assert!(m.contains(r#"<requestedExecutionLevel level="asInvoker" uiAccess="false"/>"#));
        assert!(m.contains("<asmv3:file name='x.dll'/>\n</assembly>"));
    }

    #[test]
    fn architectures() {
        assert_eq!(arch_of("x86_64"), "win-x64");
        assert_eq!(arch_of("aarch64-pc-windows-gnullvm"), "win-arm64");
        assert_eq!(arch_of("i686-pc-windows-msvc"), "win-x86");
    }

    #[test]
    fn the_runtime_has_its_pri() {
        let dir = std::env::temp_dir().join(format!("prism-runtime-{}", std::process::id()));
        let native = dir.join("winui/runtimes-framework/win-x64/native");
        fs::create_dir_all(native.join("en-us")).unwrap();
        fs::write(native.join("Microsoft.UI.Xaml.Controls.pri"), "pri").unwrap();
        fs::write(native.join("en-us/Microsoft.UI.Xaml.dll.mui"), "mui").unwrap();
        let files: Vec<PathBuf> = runtime_files(&dir, "win-x64")
            .into_iter()
            .map(|(rel, _)| rel)
            .collect();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(
            files,
            [
                PathBuf::from("Microsoft.UI.Xaml.Controls.pri"),
                Path::new("en-us").join("Microsoft.UI.Xaml.dll.mui"),
                PathBuf::from("resources.pri"),
            ]
        );
    }
}
