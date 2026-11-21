//! The Windows App SDK (WinUI 3) that Prism's Windows backend uses,
//! self-contained, without MSBuild, NuGet or MSVC:
//!
//! - [`ensure`] fetches its component packages from nuget.org into a cache
//!   once (`PRISM_WINAPPSDK_DIR`, else `<target dir>/winappsdk/<version>`),
//!   with `curl` and `tar` (both part of Windows 10 and 11; `unzip` too
//!   elsewhere), and unpacks each only if its SHA-256 is the pinned one;
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
//!
//! Web views (WinUI's `WebView2`) also need the WinRT projection of
//! WebView2 (`Microsoft.Web.WebView2.Core.dll`, the loader linked in) from
//! the `Microsoft.Web.WebView2` package the WinUI package depends on: its
//! metadata for the bindings and its DLL next to the app (the WinUI
//! package's fragment already registers its classes). The browser engine
//! itself is the Evergreen WebView2 Runtime, part of Windows 11 (and
//! installed with Edge on Windows 10).

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::winres::{self, AppInfo};

/// The Windows App SDK release the backend is built against.
pub const VERSION: &str = "2.7.0";

/// Its component packages the backend needs: metadata and runtime.
pub const PACKAGES: [&str; 3] = ["foundation", "interactiveexperiences", "winui"];

/// The `Microsoft.Web.WebView2` release the WinUI package depends on
/// (its `.nuspec`), cached as `<sdk>/webview2`.
pub const WEBVIEW2_VERSION: &str = "1.0.3719.77";

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

/// A NuGet package the backend uses, unpacked as `<sdk>/<name>`.
struct Package {
    name: &'static str,
    id: &'static str,
    version: &'static str,
    /// The SHA-256 of its `.nupkg` (checked against nuget.org's catalog
    /// SHA-512 when pinned).
    sha256: &'static str,
    /// A path that exists once it is unpacked.
    marker: &'static str,
}

/// [`PACKAGES`] and `webview2`, pinned.
const PINNED: [Package; 4] = [
    Package {
        name: "foundation",
        id: "microsoft.windowsappsdk.foundation",
        version: VERSION,
        sha256: "4131660bda7476eef751ce134c9847b5a79e4051809bffbf9dd171f4246afdf8",
        marker: "runtimes-framework",
    },
    Package {
        name: "interactiveexperiences",
        id: "microsoft.windowsappsdk.interactiveexperiences",
        version: VERSION,
        sha256: "5df51fd9e30a6093f737698ae8ad4e8d62d87f7081b00aa6a4ea0b893f03e1c2",
        marker: "runtimes-framework",
    },
    Package {
        name: "winui",
        id: "microsoft.windowsappsdk.winui",
        version: VERSION,
        sha256: "027ecdc0c51a2d31fd25985616ef2e9438f02ee3e7b6e72fd0601ddb119688b4",
        marker: "runtimes-framework",
    },
    Package {
        name: "webview2",
        id: "microsoft.web.webview2",
        version: WEBVIEW2_VERSION,
        sha256: "2f6be3a10a1a8d6d1fde986af4131dab344f8181fffe75590824e0f4b037ed73",
        marker: "lib/Microsoft.Web.WebView2.Core.winmd",
    },
];

/// The file in an unpacked package naming the SHA-256 it was unpacked from.
const UNPACKED_FROM: &str = ".prism-sha256";

/// The packages in `sdk`, fetched if missing. Each download is checked
/// against its pinned SHA-256 before it is unpacked, and a package is only
/// reused if it was unpacked from that archive. A `PRISM_WINAPPSDK_DIR`
/// you filled yourself is used as it is.
pub fn ensure(sdk: &Path) {
    let vendored = std::env::var_os("PRISM_WINAPPSDK_DIR").is_some();
    for p in &PINNED {
        if let Err(e) = fetch(sdk, p, vendored, curl) {
            panic!("{e}");
        }
    }
}

fn curl(url: &str, to: &Path) -> bool {
    Command::new("curl")
        .args(["-sSfL", "--retry", "3", "-o"])
        .arg(to)
        .arg(url)
        .status()
        .is_ok_and(|s| s.success())
}

/// The SHA-256 of a file, in hex.
fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buf = vec![0; 1 << 16];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hash.finalize().iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    }))
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

/// Downloads package `p` with `download`, checks its SHA-256 and unpacks it
/// into `<sdk>/<name>`, unless it is there: unpacked from the pinned
/// archive, or anything with its marker in a `vendored` directory.
fn fetch(
    sdk: &Path,
    p: &Package,
    vendored: bool,
    download: impl Fn(&str, &Path) -> bool,
) -> Result<(), String> {
    let dest = sdk.join(p.name);
    let ready = |d: &Path| {
        d.join(p.marker).exists()
            && (vendored
                || fs::read_to_string(d.join(UNPACKED_FROM)).is_ok_and(|h| h.trim() == p.sha256))
    };
    if ready(&dest) {
        return Ok(());
    }
    fs::create_dir_all(sdk).map_err(|e| format!("{}: {e}", sdk.display()))?;
    // The backend's build script and the app's (`link_app`) may fetch at
    // the same time on a first build: one at a time, and the second reuses
    // what the first unpacked.
    let _lock = Lock::take(sdk.join(format!("{}.lock", p.name)));
    if ready(&dest) {
        return Ok(());
    }
    let (id, version) = (p.id, p.version);
    let url = format!("https://api.nuget.org/v3-flatcontainer/{id}/{version}/{id}.{version}.nupkg");
    let tag = std::process::id();
    let pkg = sdk.join(format!("{id}.{version}.{tag}.zip"));
    println!(
        "cargo:warning=fetching {url} (once, into {})",
        sdk.display()
    );
    if !download(&url, &pkg) {
        let _ = fs::remove_file(&pkg);
        return Err(format!(
            "prism: could not download {url} with curl. Download the Windows App SDK packages \
             ({}) version {VERSION} and Microsoft.Web.WebView2 {WEBVIEW2_VERSION} (as webview2) \
             from nuget.org, unpack each into <dir>/<name>, and set PRISM_WINAPPSDK_DIR=<dir>.",
            PACKAGES.join(", ")
        ));
    }
    let got = sha256_file(&pkg);
    if got.as_deref() != Ok(p.sha256) {
        let _ = fs::remove_file(&pkg);
        return Err(format!(
            "prism: {url} is not the pinned {id} {version} (SHA-256 {}, expected {}): not unpacked",
            got.unwrap_or_else(|e| e),
            p.sha256
        ));
    }
    // Unpacked beside, then renamed: an interrupted build leaves no half
    // package behind.
    let tmp = sdk.join(format!("{}.partial.{tag}", p.name));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
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
    let _ = fs::remove_file(&pkg);
    if !unpacked || fs::write(tmp.join(UNPACKED_FROM), p.sha256).is_err() {
        let _ = fs::remove_dir_all(&tmp);
        return Err(format!("prism: could not unpack {}", pkg.display()));
    }
    // An older copy (no SHA-256 stamp) goes first. On Windows a virus
    // scanner may still hold a file just unpacked, which fails the rename
    // for a moment: retried for up to 10 s.
    let mut moved = false;
    for attempt in 0..40 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        let _ = fs::remove_dir_all(&dest);
        if fs::rename(&tmp, &dest).is_ok() {
            moved = true;
            break;
        }
    }
    if !moved {
        let _ = fs::remove_dir_all(&tmp);
        return Err(format!(
            "prism: could not move the Windows App SDK package to {}",
            dest.display()
        ));
    }
    Ok(())
}

/// A lock file held while one package is fetched, removed when dropped.
/// A lock older than 15 minutes is a build that was killed: taken over.
/// After 15 minutes of waiting, the fetch goes ahead without it.
struct Lock(Option<PathBuf>);

impl Lock {
    fn take(path: PathBuf) -> Lock {
        use std::time::{Duration, Instant};
        const STALE: Duration = Duration::from_secs(15 * 60);
        let started = Instant::now();
        loop {
            let created = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path);
            if created.is_ok() {
                return Lock(Some(path));
            }
            if started.elapsed() > STALE {
                return Lock(None);
            }
            let stale = fs::metadata(&path)
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > STALE));
            if stale {
                let _ = fs::remove_file(&path);
            } else {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
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
    // WebView2's WinRT projection (its loader linked in).
    let webview2 = sdk
        .join("webview2")
        .join("runtimes")
        .join(arch)
        .join("native_uap");
    list_tree(&webview2, Path::new(""), &mut files);
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

    /// A tar archive holding `lib/x.txt` (`tar -xf` reads it like a
    /// `.nupkg`).
    fn archive(dir: &Path, text: &str) -> Vec<u8> {
        let src = dir.join(format!("src-{text}"));
        fs::create_dir_all(src.join("lib")).unwrap();
        fs::write(src.join("lib/x.txt"), text).unwrap();
        let out = dir.join(format!("{text}.tar"));
        assert!(
            Command::new("tar")
                .arg("-cf")
                .arg(&out)
                .arg("-C")
                .arg(&src)
                .arg("lib")
                .status()
                .unwrap()
                .success()
        );
        fs::read(out).unwrap()
    }

    fn pinned_to(bytes: &[u8], dir: &Path) -> Package {
        let file = dir.join("pin");
        fs::write(&file, bytes).unwrap();
        Package {
            name: "pkg",
            id: "test.pkg",
            version: "1.0",
            sha256: Box::leak(sha256_file(&file).unwrap().into_boxed_str()),
            marker: "lib/x.txt",
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("prism-s24-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sha256_is_the_standard_one() {
        let dir = scratch("sha");
        fs::write(dir.join("abc"), "abc").unwrap();
        assert_eq!(
            sha256_file(&dir.join("abc")).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // The pinned packages, when they are at hand (PRISM_NUPKG_DIR).
        if let Some(nupkgs) = std::env::var_os("PRISM_NUPKG_DIR") {
            for p in &PINNED {
                let file = Path::new(&nupkgs).join(format!("{}.{}.nupkg", p.id, p.version));
                let started = std::time::Instant::now();
                assert_eq!(sha256_file(&file).unwrap(), p.sha256, "{}", p.id);
                eprintln!("{}: {:?}", p.id, started.elapsed());
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// S24: a different archive under the pinned version (a compromised
    /// mirror or cache) is never unpacked.
    #[test]
    fn s24_a_changed_archive_is_never_unpacked() {
        let dir = scratch("changed");
        let p = pinned_to(&archive(&dir, "genuine"), &dir);
        let changed = archive(&dir, "changed");
        let sdk = dir.join("sdk");
        let err = fetch(&sdk, &p, false, |_, to| fs::write(to, &changed).is_ok()).unwrap_err();
        assert!(err.contains("not the pinned test.pkg 1.0"), "{err}");
        assert!(!sdk.join("pkg").exists(), "unpacked");
        let left: Vec<_> = fs::read_dir(&sdk).unwrap().flatten().collect();
        assert!(left.is_empty(), "{left:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn s24_only_packages_unpacked_from_the_pinned_archive_are_reused() {
        let dir = scratch("reuse");
        let genuine = archive(&dir, "genuine");
        let p = pinned_to(&genuine, &dir);
        let sdk = dir.join("sdk");
        let downloads = std::cell::Cell::new(0);
        let serve = |_: &str, to: &Path| {
            downloads.set(downloads.get() + 1);
            fs::write(to, &genuine).is_ok()
        };
        fetch(&sdk, &p, false, serve).unwrap();
        let text = sdk.join("pkg/lib/x.txt");
        assert_eq!(fs::read_to_string(&text).unwrap(), "genuine");
        fetch(&sdk, &p, false, serve).unwrap();
        assert_eq!(downloads.get(), 1, "reused");

        // A cache entry not unpacked by this check (an older cache, or one
        // changed in place) is fetched again.
        fs::write(&text, "changed").unwrap();
        fs::remove_file(sdk.join("pkg").join(UNPACKED_FROM)).unwrap();
        fetch(&sdk, &p, false, serve).unwrap();
        assert_eq!(downloads.get(), 2);
        assert_eq!(fs::read_to_string(&text).unwrap(), "genuine");

        // A directory you provide (PRISM_WINAPPSDK_DIR) is used as it is.
        let vendor = dir.join("vendor");
        fs::create_dir_all(vendor.join("pkg/lib")).unwrap();
        fs::write(vendor.join("pkg/lib/x.txt"), "mine").unwrap();
        fetch(&vendor, &p, true, serve).unwrap();
        assert_eq!(downloads.get(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Two build scripts fetching at once (the backend's and the app's, on
    /// a first build): one downloads, the other reuses its copy, and an
    /// older copy without a stamp is replaced.
    #[test]
    fn s24_fetches_at_the_same_time_download_once() {
        let dir = scratch("together");
        let genuine = archive(&dir, "genuine");
        let p = pinned_to(&genuine, &dir);
        let sdk = dir.join("sdk");
        fs::create_dir_all(sdk.join("pkg/lib")).unwrap();
        fs::write(sdk.join("pkg/lib/x.txt"), "older").unwrap();
        let downloads = std::sync::atomic::AtomicUsize::new(0);
        let serve = |_: &str, to: &Path| {
            downloads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(300));
            fs::write(to, &genuine).is_ok()
        };
        std::thread::scope(|s| {
            let a = s.spawn(|| fetch(&sdk, &p, false, serve));
            let b = s.spawn(|| fetch(&sdk, &p, false, serve));
            assert_eq!(a.join().unwrap(), Ok(()));
            assert_eq!(b.join().unwrap(), Ok(()));
        });
        assert_eq!(downloads.load(std::sync::atomic::Ordering::SeqCst), 1);
        let text = sdk.join("pkg/lib/x.txt");
        assert_eq!(fs::read_to_string(text).unwrap(), "genuine");
        let left: Vec<_> = fs::read_dir(&sdk)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .filter(|n| n != "pkg")
            .collect();
        assert!(left.is_empty(), "{left:?}");
        let _ = fs::remove_dir_all(&dir);
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
