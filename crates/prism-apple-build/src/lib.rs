//! Used from `build.rs` of the Apple backends: compiles the Swift sources in
//! `platform/apple/{Shared,Bridge}` into a static library for the cargo target
//! and links it, together with the Swift runtime search paths.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Compiles and links the bridge. No-op for non-Apple targets.
pub fn build_bridge() {
    build_swift(&["Shared", "Bridge"], "PrismBridge");
}

/// Compiles and links the watchOS SwiftUI host (`platform/apple/Watch`) with
/// the shared bridge (device APIs, notifications).
pub fn build_watch_host() {
    build_swift(&["Shared", "Bridge", "Watch"], "PrismWatch");
}

/// Compiles the Swift sources of `dirs` (under `platform/apple`) into a
/// static library named `module` and links it, for the cargo target.
fn build_swift(dirs: &[&str], module: &str) {
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

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let apple = manifest.join("../../platform/apple");
    let mut sources: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let dir = apple.join(dir);
        println!("cargo:rerun-if-changed={}", dir.display());
        for entry in std::fs::read_dir(&dir).expect("platform/apple sources") {
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
    // Swift's availability checks call compiler-rt's
    // `__isPlatformVersionAtLeast`; rustc links with -nodefaultlibs, and only
    // brings its own copy on some targets. watchOS devices need clang's.
    if target == "aarch64-apple-watchos" {
        let clang = xcrun(&["-f", "clang"]);
        let versions = Path::new(&clang).parent().unwrap().join("../lib/clang");
        let darwin = std::fs::read_dir(&versions)
            .into_iter()
            .flatten()
            .flatten()
            .map(|v| v.path().join("lib/darwin"))
            .find(|d| d.join("libclang_rt.watchos.a").exists());
        if let Some(darwin) = darwin {
            println!("cargo:rustc-link-search=native={}", darwin.display());
            // A fat archive: left for the final link (ld picks the slice).
            println!("cargo:rustc-link-lib=static:-bundle=clang_rt.watchos");
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
    for fw in ["WidgetKit", "UserNotifications", "SwiftUI", "ActivityKit"] {
        if frameworks.join(format!("{fw}.framework")).exists()
            && !(fw == "ActivityKit" && sdk == "macosx")
        {
            println!("cargo:rustc-link-lib=framework={fw}");
        }
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
mod tests {
    #[test]
    fn clamps_deployment_targets() {
        assert_eq!(super::at_least("10.0", "26.0"), "26.0");
        assert_eq!(super::at_least("26.1", "26.0"), "26.1");
        assert_eq!(super::at_least("27", "26.0"), "27");
    }
}
