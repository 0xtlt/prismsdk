//! # prism-apple-build
//!
//! Build-script helper for Prism's Apple backends (`prism-macos`,
//! `prism-ios`, `prism-watchos`). It compiles the Swift sources shipped in
//! this crate's `apple/` directory into a static library for the cargo target
//! and links it, with the Swift runtime search paths:
//!
//! - `apple/Shared` and `apple/Bridge`: the bridge to WidgetKit, ActivityKit,
//!   UserNotifications, CarPlay and the device APIs ([`build_bridge`]);
//! - `apple/Watch`: the SwiftUI host that draws watchOS apps
//!   ([`build_watch_host`]).
//!
//! ```no_run
//! // In `fn main` of an Apple backend's build.rs:
//! prism_apple_build::build_bridge();
//! ```
//!
//! Both are no-ops for non-Apple targets and on docs.rs (`DOCS_RS`), where no
//! Xcode toolchain exists. Requires Xcode (`xcrun swiftc`) otherwise.
//!
//! The same sources, plus the widget extension (`apple/Extension`) and the
//! bundling helpers, are exposed as [`SOURCES`] for the `prism` CLI.

use std::path::{Path, PathBuf};
use std::process::Command;

/// clang's runtime library (`libclang_rt.<name>.a`) for a Rust Apple
/// target, `None` for macOS (rustc links what it needs there).
fn clang_runtime(target: &str) -> Option<&'static str> {
    let sim = target.ends_with("-sim") || target.starts_with("x86_64-apple-ios");
    let os = if target.contains("-apple-watchos") {
        "watchos"
    } else if target.contains("-apple-tvos") {
        "tvos"
    } else if target.contains("-apple-visionos") {
        "xros"
    } else if target.contains("-apple-ios") && !target.ends_with("-macabi") {
        "ios"
    } else {
        return None;
    };
    Some(match (os, sim) {
        ("ios", false) => "clang_rt.ios",
        ("ios", true) => "clang_rt.iossim",
        ("tvos", false) => "clang_rt.tvos",
        ("tvos", true) => "clang_rt.tvossim",
        ("xros", false) => "clang_rt.xros",
        ("xros", true) => "clang_rt.xrossim",
        ("watchos", false) => "clang_rt.watchos",
        _ => "clang_rt.watchossim",
    })
}

/// The directory of clang's Darwin runtimes holding `file`:
/// `$(clang -print-resource-dir)/lib/darwin`, else any version under the
/// toolchain's `lib/clang`.
fn clang_darwin_dir(file: &str) -> Option<PathBuf> {
    let resource = Command::new("xcrun")
        .args(["clang", "-print-resource-dir"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    if let Some(dir) = resource.map(|r| Path::new(&r).join("lib/darwin"))
        && dir.join(file).exists()
    {
        return Some(dir);
    }
    let clang = xcrun(&["-f", "clang"]);
    let versions = Path::new(&clang).parent()?.join("../lib/clang");
    std::fs::read_dir(&versions)
        .into_iter()
        .flatten()
        .flatten()
        .map(|v| v.path().join("lib/darwin"))
        .find(|d| d.join(file).exists())
}

fn xcrun(args: &[&str]) -> String {
    let out = Command::new("xcrun")
        .args(args)
        .output()
        .expect("prism: xcrun not found (install Xcode)");
    assert!(
        out.status.success(),
        "prism: xcrun {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Every file of this crate's `apple/` directory, as `(path, contents)` with
/// paths relative to it (`"Bridge/PrismBridge.swift"`, `"generate_widgets.py"`).
///
/// The `prism` CLI writes them under its platform directory to bundle apps
/// (widget extension, icons), so projects don't need a Prism checkout.
pub const SOURCES: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/sources.rs"));

/// Compiles and links the bridge (`apple/Shared` + `apple/Bridge`). No-op for
/// non-Apple targets.
pub fn build_bridge() {
    build_swift(&["Shared", "Bridge"], "PrismBridge");
}

/// Compiles and links the watchOS SwiftUI host (`apple/Watch`) with
/// the shared bridge (device APIs, notifications).
pub fn build_watch_host() {
    build_swift(&["Shared", "Bridge", "Watch"], "PrismWatch");
}

/// Compiles the Swift sources of `dirs` (under `apple/`) into a
/// static library named `module` and links it, for the cargo target.
fn build_swift(dirs: &[&str], module: &str) {
    // docs.rs builds documentation on Linux, without Xcode.
    println!("cargo:rerun-if-env-changed=DOCS_RS");
    if std::env::var_os("DOCS_RS").is_some() {
        return;
    }
    let target = std::env::var("TARGET").unwrap_or_default();
    let (sdk, platform, triple) = match target.as_str() {
        "aarch64-apple-ios-sim" => (
            "iphonesimulator",
            "iphonesimulator",
            format!(
                "arm64-apple-ios{}-simulator",
                env_or("IPHONEOS_DEPLOYMENT_TARGET", "17.0")
            ),
        ),
        "aarch64-apple-ios" => (
            "iphoneos",
            "iphoneos",
            format!(
                "arm64-apple-ios{}",
                env_or("IPHONEOS_DEPLOYMENT_TARGET", "17.0")
            ),
        ),
        "aarch64-apple-tvos-sim" => (
            "appletvsimulator",
            "appletvsimulator",
            format!(
                "arm64-apple-tvos{}-simulator",
                env_or("TVOS_DEPLOYMENT_TARGET", "17.0")
            ),
        ),
        "aarch64-apple-tvos" => (
            "appletvos",
            "appletvos",
            format!(
                "arm64-apple-tvos{}",
                env_or("TVOS_DEPLOYMENT_TARGET", "17.0")
            ),
        ),
        "aarch64-apple-visionos-sim" => (
            "xrsimulator",
            "xrsimulator",
            format!(
                "arm64-apple-xros{}-simulator",
                env_or("XROS_DEPLOYMENT_TARGET", "26.0")
            ),
        ),
        "aarch64-apple-visionos" => (
            "xros",
            "xros",
            format!(
                "arm64-apple-xros{}",
                env_or("XROS_DEPLOYMENT_TARGET", "26.0")
            ),
        ),
        "aarch64-apple-watchos-sim" => (
            "watchsimulator",
            "watchsimulator",
            format!(
                "arm64-apple-watchos{}-simulator",
                env_or("WATCHOS_DEPLOYMENT_TARGET", "10.0")
            ),
        ),
        // arm64 (not arm64_32) watches run watchOS 26+, rustc's minimum for
        // this target: below it the Swift code needs compiler-rt's
        // availability checks, which Rust does not link.
        "aarch64-apple-watchos" => (
            "watchos",
            "watchos",
            format!(
                "arm64-apple-watchos{}",
                at_least(&env_or("WATCHOS_DEPLOYMENT_TARGET", "10.0"), "26.0")
            ),
        ),
        "aarch64-apple-darwin" | "x86_64-apple-darwin" => (
            "macosx",
            "macosx",
            format!(
                "{}-apple-macos{}",
                if target.starts_with("aarch64") {
                    "arm64"
                } else {
                    "x86_64"
                },
                // WidgetKit needs macOS 14.
                env_or("PRISM_MACOS_BRIDGE_TARGET", "14.0")
            ),
        ),
        _ => return,
    };

    // This crate's own sources (a checkout, or the registry's copy), not the
    // manifest of the backend whose build script calls us.
    let apple = Path::new(env!("CARGO_MANIFEST_DIR")).join("apple");
    let mut sources: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let dir = apple.join(dir);
        println!("cargo:rerun-if-changed={}", dir.display());
        for entry in std::fs::read_dir(&dir).expect("prism-apple-build: apple/ sources") {
            let p = entry.unwrap().path();
            if p.extension().is_some_and(|e| e == "swift") {
                sources.push(p);
            }
        }
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let lib = out.join(format!("lib{module}.a"));
    let status = Command::new("xcrun")
        .args([
            "--sdk",
            sdk,
            "swiftc",
            "-target",
            &triple,
            "-swift-version",
            "5",
            "-O",
            // Optimized as one module, like Xcode's release builds: about a
            // quarter faster to compile than file by file.
            "-wmo",
        ])
        .args([
            "-parse-as-library",
            "-emit-library",
            "-static",
            "-module-name",
            module,
        ])
        .arg("-o")
        .arg(&lib)
        .args(&sources)
        .status()
        .expect("prism: swiftc");
    assert!(status.success(), "prism: compiling the Swift bridge failed");

    let sdk_path = xcrun(&["--sdk", sdk, "--show-sdk-path"]);
    let swiftc = xcrun(&["-f", "swiftc"]);
    let toolchain_lib = Path::new(&swiftc)
        .parent()
        .unwrap()
        .join("../lib/swift")
        .join(platform);
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static={module}");
    println!("cargo:rustc-link-search=native={}", toolchain_lib.display());
    println!("cargo:rustc-link-search=native={sdk_path}/usr/lib/swift");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    // Swift's availability checks (`if #available`, SwiftUI's own) call
    // compiler-rt's `__isPlatformVersionAtLeast`; rustc links with
    // -nodefaultlibs and its builtins do not always have it (iOS and watchOS
    // simulators and devices with the 26 SDKs, release builds), so clang's
    // runtime for the target is linked. Only the members needed are pulled
    // in: nothing clashes where rustc's builtins have it too.
    if let Some(lib) = clang_runtime(&target) {
        if let Some(darwin) = clang_darwin_dir(&format!("lib{lib}.a")) {
            println!("cargo:rustc-link-search=native={}", darwin.display());
            // A fat archive: left for the final link (ld picks the slice).
            println!("cargo:rustc-link-lib=static:-bundle={lib}");
        } else {
            println!("cargo:warning=prism: lib{lib}.a not found in clang's resource directory");
        }
    }
    // Simulator builds read entitlements (App Groups…) from sections of the
    // binary itself, like Xcode's `-sectcreate __TEXT __entitlements`. A
    // build script's link args do not reach the final executable, so the
    // sections come from a tiny whole-archive object instead.
    println!("cargo:rerun-if-env-changed=PRISM_ENTITLEMENTS");
    if let Ok(ent) = std::env::var("PRISM_ENTITLEMENTS")
        && target.ends_with("-sim")
    {
        println!("cargo:rerun-if-changed={ent}");
        let der = format!("{ent}.der");
        let mut asm =
            format!(".section __TEXT,__entitlements,regular,no_dead_strip\n.incbin \"{ent}\"\n");
        if Path::new(&der).exists() {
            println!("cargo:rerun-if-changed={der}");
            asm +=
                &format!(".section __TEXT,__ents_der,regular,no_dead_strip\n.incbin \"{der}\"\n");
        }
        let src = out.join("entitlements.s");
        std::fs::write(&src, asm).unwrap();
        let obj = out.join("entitlements.o");
        let ok = Command::new("xcrun")
            .args(["--sdk", sdk, "clang", "-c", "-target", &triple])
            .arg(&src)
            .arg("-o")
            .arg(&obj)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "prism: assembling entitlements failed");
        let ok = Command::new("ar")
            .arg("rcs")
            .arg(out.join("libPrismEntitlements.a"))
            .arg(&obj)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "prism: archiving entitlements failed");
        println!("cargo:rustc-link-lib=static:+whole-archive=PrismEntitlements");
    }
    // Only what this SDK has (tvOS: no WidgetKit or ActivityKit).
    let frameworks = Path::new(&sdk_path).join("System/Library/Frameworks");
    for fw in [
        "WidgetKit",
        "UserNotifications",
        "SwiftUI",
        "ActivityKit",
        // Secure storage, owner authentication, sign-in (PrismSecurity.swift).
        "Security",
        "LocalAuthentication",
        "AuthenticationServices",
        // Videos (PrismVideo.swift, #207).
        "AVFoundation",
        "AVKit",
        // In-app purchases (PrismPurchases.swift, #306).
        "StoreKit",
    ] {
        if frameworks.join(format!("{fw}.framework")).exists()
            && !(fw == "ActivityKit" && sdk == "macosx")
        {
            println!("cargo:rustc-link-lib=framework={fw}");
        }
    }
}

/// The deployment targets Prism builds for when the environment sets none,
/// as `(environment variable, version)`: iOS and iPadOS 17, tvOS 17, watchOS
/// 10, visionOS 26 (WidgetKit there), macOS 13.
///
/// The `prism` CLI sets them for every `cargo` it runs (overridden by
/// `deployment_target` in the platform's section of `Prism.toml`, and by the
/// variable itself when it is already set); [`link_app`] uses them for builds
/// without the CLI. Below iOS 15 Swift Concurrency would be a back-deployed
/// `@rpath` library the app does not ship.
pub const DEPLOYMENT_TARGETS: [(&str, &str); 5] = [
    ("IPHONEOS_DEPLOYMENT_TARGET", "17.0"),
    ("TVOS_DEPLOYMENT_TARGET", "17.0"),
    ("WATCHOS_DEPLOYMENT_TARGET", "10.0"),
    ("XROS_DEPLOYMENT_TARGET", "26.0"),
    ("MACOSX_DEPLOYMENT_TARGET", "13.0"),
];

/// The deployment-target variable of a Rust target, `None` off Apple
/// platforms.
fn deployment_variable(target: &str) -> Option<&'static str> {
    let var = if target.contains("-apple-watchos") {
        "WATCHOS_DEPLOYMENT_TARGET"
    } else if target.contains("-apple-tvos") {
        "TVOS_DEPLOYMENT_TARGET"
    } else if target.contains("-apple-visionos") {
        "XROS_DEPLOYMENT_TARGET"
    } else if target.contains("-apple-ios") {
        "IPHONEOS_DEPLOYMENT_TARGET"
    } else if target.contains("-apple-darwin") {
        "MACOSX_DEPLOYMENT_TARGET"
    } else {
        return None;
    };
    Some(var)
}

/// The deployment target a Rust target is linked for: its variable from
/// the environment, else Prism's default ([`DEPLOYMENT_TARGETS`]); arm64
/// watches (`aarch64-apple-watchos`) at least watchOS 26.
fn deployment_target(target: &str, env: Option<String>) -> Option<(&'static str, String)> {
    let var = deployment_variable(target)?;
    let default = DEPLOYMENT_TARGETS.iter().find(|(v, _)| *v == var)?.1;
    let mut version = env.unwrap_or_else(|| default.to_string());
    if target == "aarch64-apple-watchos" {
        version = at_least(&version, "26.0");
    }
    Some((var, version))
}

/// Link settings for the **app's own** binary, from the app's `build.rs`
/// (`prism new` generates the call, through `prism_build::link_app`).
///
/// Cargo applies a build script's `rustc-link-arg` only to the package that
/// printed it, so the backends' build scripts cannot reach the final
/// executable. For Apple targets this adds the Swift runtime's search path
/// (`-rpath /usr/lib/swift`, as Xcode does) and, when the environment sets no
/// deployment target, Prism's ([`DEPLOYMENT_TARGETS`]) for the final link:
/// plain `cargo build` then links like the `prism` CLI. No-op elsewhere.
pub fn link_app() {
    let target = std::env::var("TARGET").unwrap_or_default();
    let Some(var) = deployment_variable(&target) else {
        return;
    };
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    println!("cargo:rerun-if-env-changed={var}");
    if std::env::var_os(var).is_none()
        && let Some((var, version)) = deployment_target(&target, None)
    {
        // rustc reads it when linking the binary.
        println!("cargo:rustc-env={var}={version}");
    }
}

/// The higher of two `major.minor` versions.
fn at_least(version: &str, min: &str) -> String {
    let parse = |v: &str| -> Vec<u32> { v.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    if parse(version) < parse(min) {
        min.to_string()
    } else {
        version.to_string()
    }
}

fn env_or(key: &str, default: &str) -> String {
    println!("cargo:rerun-if-env-changed={key}");
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[cfg(test)]
mod temp_dir;
#[cfg(test)]
mod tests {
    #[test]
    fn clamps_deployment_targets() {
        assert_eq!(super::at_least("10.0", "26.0"), "26.0");
        assert_eq!(super::at_least("26.1", "26.0"), "26.1");
        assert_eq!(super::at_least("27", "26.0"), "27");
    }

    #[test]
    fn deployment_targets_follow_the_rust_target() {
        let t = |target: &str, env: Option<&str>| {
            super::deployment_target(target, env.map(str::to_string))
        };
        for (target, var, version) in [
            (
                "aarch64-apple-ios-sim",
                "IPHONEOS_DEPLOYMENT_TARGET",
                "17.0",
            ),
            ("aarch64-apple-ios", "IPHONEOS_DEPLOYMENT_TARGET", "17.0"),
            ("aarch64-apple-tvos-sim", "TVOS_DEPLOYMENT_TARGET", "17.0"),
            ("aarch64-apple-visionos", "XROS_DEPLOYMENT_TARGET", "26.0"),
            (
                "aarch64-apple-watchos-sim",
                "WATCHOS_DEPLOYMENT_TARGET",
                "10.0",
            ),
            ("aarch64-apple-watchos", "WATCHOS_DEPLOYMENT_TARGET", "26.0"),
            ("aarch64-apple-darwin", "MACOSX_DEPLOYMENT_TARGET", "13.0"),
        ] {
            assert_eq!(
                t(target, None),
                Some((var, version.to_string())),
                "{target}"
            );
        }
        assert_eq!(t("aarch64-apple-ios-sim", Some("18.2")).unwrap().1, "18.2");
        assert_eq!(t("aarch64-apple-watchos", Some("11.0")).unwrap().1, "26.0");
        assert_eq!(t("x86_64-unknown-linux-gnu", None), None);
        assert_eq!(t("aarch64-linux-android", None), None);
    }

    #[test]
    fn every_apple_target_but_macos_links_clangs_runtime() {
        for (target, lib) in [
            ("aarch64-apple-ios", Some("clang_rt.ios")),
            ("aarch64-apple-ios-sim", Some("clang_rt.iossim")),
            ("x86_64-apple-ios", Some("clang_rt.iossim")),
            ("aarch64-apple-tvos", Some("clang_rt.tvos")),
            ("aarch64-apple-tvos-sim", Some("clang_rt.tvossim")),
            ("aarch64-apple-visionos", Some("clang_rt.xros")),
            ("aarch64-apple-visionos-sim", Some("clang_rt.xrossim")),
            ("aarch64-apple-watchos", Some("clang_rt.watchos")),
            ("aarch64-apple-watchos-sim", Some("clang_rt.watchossim")),
            ("aarch64-apple-darwin", None),
            ("aarch64-apple-ios-macabi", None),
        ] {
            assert_eq!(super::clang_runtime(target), lib, "{target}");
        }
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
}
