//! The Windows App SDK (WinUI 3) that Prism's Windows backend uses,
//! self-contained, without MSBuild, NuGet or MSVC:
//!
//! - [`ensure`] fetches its component packages from nuget.org into a cache
//!   once (`PRISM_WINAPPSDK_DIR`, else `<target dir>/winappsdk/<version>`),
//!   with `curl` and `tar` (both part of Windows 10 and 11; `unzip` too
//!   elsewhere);
//! - [`deploy`] copies its runtime (DLLs, `resources.pri`) next to the
//!   app's executable (about 55 MB, x64);
//! - [`embed_manifest`] gives the executable the manifest that registers
//!   the runtime's WinRT classes (reg-free activation, what the SDK's
//!   MSBuild targets generate), DPI awareness and the Windows versions XAML
//!   islands need. Called by [`link_app`](crate::link_app) for Windows
//!   targets.
//!
//! `prismsdk-windows`' build script uses the same cache for its WinUI
//! bindings.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    match std::env::var_os("PRISM_WINAPPSDK_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => profile_dir()
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
    match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "win-arm64",
        Ok("x86") => "win-x86",
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
    let pkg = sdk.join(format!("{id}.{VERSION}.zip"));
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
    let tmp = sdk.join(format!("{name}.partial"));
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
    let _ = fs::remove_dir_all(&dest);
    fs::rename(&tmp, &dest).expect("Windows App SDK package");
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

/// The app's manifest: Windows 10/11 (XAML islands need
/// `maxversiontested` ≥ 10.0.18362), per-monitor DPI v2, UTF-8, the common
/// controls v6, and the runtime's classes.
pub fn app_manifest(sdk: &Path) -> String {
    let files = classes(sdk);
    format!(
        r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<assembly manifestVersion="1.0" xmlns="urn:schemas-microsoft-com:asm.v1"
  xmlns:asmv3="urn:schemas-microsoft-com:asm.v3" xmlns:winrtv1="urn:schemas-microsoft-com:winrt.v1">
  <assemblyIdentity type="win32" name="Prism.App" version="1.0.0.0"/>
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

/// Embeds the app manifest in the package's executables (resource 1,
/// `RT_MANIFEST`). GNU: compiled by `windres` (MinGW's binutils, or
/// `WINDRES`) into an object named like MinGW's own default manifest, found
/// first with `-B` so the executable gets one manifest, not two. MSVC: the
/// linker's `/MANIFESTINPUT`.
pub fn embed_manifest(sdk: &Path) {
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let manifest = out.join("prism-app.manifest");
    fs::write(&manifest, app_manifest(sdk)).expect("app manifest");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
        return;
    }
    println!("cargo:rerun-if-env-changed=WINDRES");
    let rc = out.join("prism-app.rc");
    let path = manifest.display().to_string().replace('\\', "/");
    fs::write(&rc, format!("1 24 \"{path}\"\n")).expect("resource script");
    let obj = out.join("default-manifest.o");
    let windres = std::env::var("WINDRES").unwrap_or_else(|_| "windres".into());
    let status = Command::new(&windres)
        .args(["--input-format=rc", "--output-format=coff", "--input"])
        .arg(&rc)
        .arg("--output")
        .arg(&obj)
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg-bins=-B{}", out.display());
        }
        _ => println!(
            "cargo:warning=prism: {windres} (MinGW binutils) is needed to embed the app's \
             manifest: without it WinUI cannot start. Install MinGW-w64 or set WINDRES."
        ),
    }
}

/// The runtime next to the executables in `dir`.
pub fn deploy(sdk: &Path, dir: &Path) {
    let _ = fs::create_dir_all(dir);
    for p in PACKAGES {
        copy_tree(
            &sdk.join(p)
                .join("runtimes-framework")
                .join(arch())
                .join("native"),
            dir,
        );
    }
    // Unpackaged apps resolve ms-appx:/// through `resources.pri` next to
    // the exe. MSBuild merges the components' PRIs into it (makepri); the
    // controls' one alone is enough for XamlControlsResources.
    let pri = dir.join("resources.pri");
    let controls = dir.join("Microsoft.UI.Xaml.Controls.pri");
    if !same_size(&controls, &pri) {
        let _ = fs::copy(&controls, &pri);
    }
}

fn same_size(a: &Path, b: &Path) -> bool {
    let len = |p: &Path| fs::metadata(p).map(|m| m.len()).ok();
    len(a).is_some() && len(a) == len(b)
}

fn copy_tree(from: &Path, to: &Path) {
    let Ok(entries) = fs::read_dir(from) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let dest = to.join(e.file_name());
        if p.is_dir() {
            let _ = fs::create_dir_all(&dest);
            copy_tree(&p, &dest);
        } else if !same_size(&p, &dest) {
            // A running app keeps its DLLs locked: copied on a later build.
            let _ = fs::copy(&p, &dest);
        }
    }
}

/// What [`link_app`](crate::link_app) does for a Windows app built with
/// the native backend: the runtime next to it, the manifest in it.
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
    embed_manifest(&sdk);
    if std::env::var_os("PRISM_WINAPPSDK_NO_COPY").is_none() {
        let dir = profile_dir();
        deploy(&sdk, &dir);
        if dir.join("examples").is_dir() {
            deploy(&sdk, &dir.join("examples"));
        }
    }
}
