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
    // Set when the widget extension is linked in (see `widget_extension`).
    println!("cargo::rustc-check-cfg=cfg(prism_widget_extension)");
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
    let features = BridgeFeatures::from_env();
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
        .args(features.swift_flags(sdk))
        .arg("-o")
        .arg(&lib)
        .args(&sources)
        .status()
        .expect("prism: swiftc");
    assert!(status.success(), "prism: compiling the Swift bridge failed");

    if module == "PrismBridge" {
        widget_extension(sdk, &triple, &apple, &out, features);
    }

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
        // Secure storage (PrismSecurity.swift).
        "Security",
        // Videos (PrismVideo.swift, #207).
        "AVFoundation",
        "AVKit",
    ] {
        if frameworks.join(format!("{fw}.framework")).exists()
            && !(fw == "ActivityKit" && sdk == "macosx")
        {
            println!("cargo:rustc-link-lib=framework={fw}");
        }
    }
    // Owner authentication (PrismSecurity.swift): on macOS and watchOS, and
    // with the `face-id` feature on iOS and visionOS.
    if features.local_auth(sdk) && frameworks.join("LocalAuthentication.framework").exists() {
        println!("cargo:rustc-link-lib=framework=LocalAuthentication");
    }
    // Sign-in (PrismSecurity.swift), with the `sign-in` feature only (#314).
    if features.sign_in {
        println!("cargo:rustc-link-lib=framework=AuthenticationServices");
    }
    // In-app purchases (PrismPurchases.swift, #306), with the `purchases`
    // feature only (#312).
    if features.purchases && frameworks.join("StoreKit.framework").exists() {
        println!("cargo:rustc-link-lib=framework=StoreKit");
    }
}

/// The widget extension's Swift (`apple/Shared`, `apple/Extension` and the
/// app's generated widget declarations), linked into the app's own binary
/// for an app whose widgets refresh without it (`Widget::refresh`, #331).
///
/// The bundler sets `PRISM_WIDGET_EXTENSION` to the generated file
/// (`generate_widgets.py`) and copies the binary into the `.appex`: there
/// the app's `main` hands over to WidgetKit (`prism_widget_extension_main`),
/// and the extension's timeline providers call back into the app's Rust
/// code to run the refresh hooks. `PRISM_WIDGET_EXTENSION_CONSTVALUES`
/// names where the App Intents constant values go (configurable widgets
/// and widget buttons: the bundler extracts their metadata from them).
///
/// The backend's crate gets the `prism_widget_extension` cfg when the
/// extension is linked.
fn widget_extension(sdk: &str, triple: &str, apple: &Path, out: &Path, features: BridgeFeatures) {
    println!("cargo:rerun-if-env-changed=PRISM_WIDGET_EXTENSION");
    println!("cargo:rerun-if-env-changed=PRISM_WIDGET_EXTENSION_CONSTVALUES");
    let Some(generated) = std::env::var_os("PRISM_WIDGET_EXTENSION").filter(|g| !g.is_empty())
    else {
        return;
    };
    // WidgetKit: iOS, iPadOS, macOS and visionOS (not tvOS or watchOS).
    if !matches!(
        sdk,
        "iphoneos" | "iphonesimulator" | "macosx" | "xros" | "xrsimulator"
    ) {
        return;
    }
    let generated = PathBuf::from(generated);
    println!("cargo:rerun-if-changed={}", generated.display());
    let mut sources = Vec::new();
    for dir in ["Shared", "Extension"] {
        for entry in std::fs::read_dir(apple.join(dir)).expect("prism-apple-build: apple/ sources")
        {
            let p = entry.unwrap().path();
            if p.extension().is_some_and(|e| e == "swift") {
                sources.push(p);
            }
        }
    }
    sources.sort();
    sources.push(generated);
    let lib = out.join("libPrismWidgets.a");
    let mut cmd = Command::new("xcrun");
    cmd.args(["--sdk", sdk, "swiftc", "-target", triple])
        .args(["-swift-version", "5", "-O", "-wmo", "-parse-as-library"])
        .args(["-emit-library", "-static", "-module-name", "PrismWidgets"])
        .args(["-application-extension", "-D", "PRISM_RUST_EXTENSION"])
        // The bundle's `@main` without a `main` symbol (the app has its own):
        // NSExtensionMain still finds it through the entry Swift records in
        // the binary's `__swift5_entry` section.
        .args(["-Xfrontend", "-entry-point-function-name", "-Xfrontend"])
        .arg("prism_widget_bundle_main")
        .args(features.swift_flags(sdk));
    if let Some(values) =
        std::env::var_os("PRISM_WIDGET_EXTENSION_CONSTVALUES").filter(|v| !v.is_empty())
    {
        // What the App Intents metadata processor reads (Xcode's « Extract
        // App Intents Metadata »), as ios-run.sh does for a Swift-only
        // extension.
        // Missing (a cleaned bundle directory): made again.
        println!("cargo:rerun-if-changed={}", Path::new(&values).display());
        let protocols = out.join("intents-protocols.json");
        std::fs::write(&protocols, INTENTS_PROTOCOLS).expect("prism: intents protocols");
        cmd.arg("-emit-const-values-path")
            .arg(&values)
            .args(["-Xfrontend", "-const-gather-protocols-file", "-Xfrontend"])
            .arg(&protocols);
    }
    let status = cmd
        .arg("-o")
        .arg(&lib)
        .args(&sources)
        .status()
        .expect("prism: swiftc");
    assert!(
        status.success(),
        "prism: compiling the widget extension failed"
    );
    println!("cargo:rustc-link-lib=static=PrismWidgets");
    println!("cargo:rustc-cfg=prism_widget_extension");
}

/// The protocols whose conformances the App Intents metadata processor
/// reads constant values of (the list Xcode passes).
const INTENTS_PROTOCOLS: &str = r#"["AppIntent","EntityQuery","AppEntity","TransientEntity","AppEnum","AppShortcutProviding","AppShortcutsProvider","AnyResolverProviding","AppIntentsPackage","DynamicOptionsProvider","_IntentValueRepresentable","_AssistantIntentsProvider","_GenerativeFunctionExtractable","IntentValueQuery","Resolver"]"#;

/// The bridge's optional parts, from the cargo features of the backend whose
/// build script calls [`build_bridge`].
///
/// - `prismsdk`'s `sign-in`, `photo-picker` and `purchases` (#312, #314):
///   each one's framework drags ~1000 more images into every launch in the
///   simulator (~1 s of dyld work), so an app links them only when it asks
///   for them.
/// - `location`, `location-always`, `camera`, `microphone`, `photo-library`
///   and `face-id`: App Store Connect asks for a purpose string for each of
///   these protected resources whose API the binary references, used or not
///   (ITMS-90683). `prism` turns them on from `[permissions]` in Prism.toml,
///   which also writes those strings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BridgeFeatures {
    /// AuthenticationServices: Sign in with Apple, web sign-in, passkeys.
    sign_in: bool,
    /// PhotosUI: the photo library picker (`pick_images`).
    photo_picker: bool,
    /// StoreKit: in-app purchases (`purchase`, `owns`…).
    purchases: bool,
    /// CoreLocation: `current_location`, `watch_location`, its permission.
    location: bool,
    /// CoreLocation's « Always » authorization (`location_always`, iOS).
    location_always: bool,
    /// The camera: its permission and `take_photo`.
    camera: bool,
    /// The microphone's permission.
    microphone: bool,
    /// The photo library's permission (PHPhotoLibrary).
    photo_library: bool,
    /// LocalAuthentication on iOS and visionOS (Face ID): biometric secrets
    /// and owner checks.
    face_id: bool,
}

impl BridgeFeatures {
    fn from_env() -> Self {
        let on = |var: &str| {
            println!("cargo:rerun-if-env-changed={var}");
            std::env::var_os(var).is_some()
        };
        BridgeFeatures {
            sign_in: on("CARGO_FEATURE_SIGN_IN"),
            photo_picker: on("CARGO_FEATURE_PHOTO_PICKER"),
            purchases: on("CARGO_FEATURE_PURCHASES"),
            location: on("CARGO_FEATURE_LOCATION"),
            location_always: on("CARGO_FEATURE_LOCATION_ALWAYS"),
            camera: on("CARGO_FEATURE_CAMERA"),
            microphone: on("CARGO_FEATURE_MICROPHONE"),
            photo_library: on("CARGO_FEATURE_PHOTO_LIBRARY"),
            face_id: on("CARGO_FEATURE_FACE_ID"),
        }
    }

    /// LocalAuthentication is compiled in: macOS (Touch ID needs no purpose
    /// string) and watchOS (no biometrics), else with `face-id`.
    fn local_auth(self, sdk: &str) -> bool {
        self.face_id || sdk == "macosx" || sdk.starts_with("watch")
    }

    /// The `swiftc -D` conditions the Swift sources test, for an SDK.
    fn swift_flags(self, sdk: &str) -> Vec<&'static str> {
        let mut flags = Vec::new();
        for (on, condition) in [
            (self.sign_in, "PRISM_AUTH_SERVICES"),
            (self.photo_picker, "PRISM_PHOTOS_PICKER"),
            (self.purchases, "PRISM_STOREKIT"),
            (self.location || self.location_always, "PRISM_LOCATION"),
            (self.location_always, "PRISM_LOCATION_ALWAYS"),
            (self.camera, "PRISM_CAMERA"),
            (self.microphone, "PRISM_MICROPHONE"),
            (self.photo_library, "PRISM_PHOTO_LIBRARY"),
            (self.local_auth(sdk), "PRISM_LOCAL_AUTH"),
        ] {
            if on {
                flags.extend(["-D", condition]);
            }
        }
        flags
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

    #[test]
    fn bridge_features_become_swift_conditions() {
        use super::BridgeFeatures;
        assert!(BridgeFeatures::default().swift_flags("iphoneos").is_empty());
        let all = BridgeFeatures {
            sign_in: true,
            photo_picker: true,
            purchases: true,
            location: true,
            location_always: true,
            camera: true,
            microphone: true,
            photo_library: true,
            face_id: true,
        };
        let conditions = |f: BridgeFeatures, sdk| -> Vec<&str> {
            f.swift_flags(sdk)
                .into_iter()
                .filter(|f| *f != "-D")
                .collect()
        };
        assert_eq!(
            conditions(all, "iphoneos"),
            [
                "PRISM_AUTH_SERVICES",
                "PRISM_PHOTOS_PICKER",
                "PRISM_STOREKIT",
                "PRISM_LOCATION",
                "PRISM_LOCATION_ALWAYS",
                "PRISM_CAMERA",
                "PRISM_MICROPHONE",
                "PRISM_PHOTO_LIBRARY",
                "PRISM_LOCAL_AUTH",
            ]
        );
        // `location-always` brings the location code with it.
        let always = BridgeFeatures {
            location_always: true,
            ..BridgeFeatures::default()
        };
        assert_eq!(
            conditions(always, "iphoneos"),
            ["PRISM_LOCATION", "PRISM_LOCATION_ALWAYS"]
        );
        // LocalAuthentication: always on macOS and watchOS (no purpose
        // string there), with `face-id` on iOS and visionOS.
        for sdk in ["macosx", "watchos", "watchsimulator"] {
            assert_eq!(
                conditions(BridgeFeatures::default(), sdk),
                ["PRISM_LOCAL_AUTH"]
            );
        }
        for sdk in ["iphoneos", "iphonesimulator", "xros", "appletvos"] {
            assert!(conditions(BridgeFeatures::default(), sdk).is_empty());
        }
    }
}
