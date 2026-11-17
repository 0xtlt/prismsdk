//! `prism`: create, run, build and live-restart Prism apps.
//!
//! ```text
//! prism new my-app                 create a project
//! prism run <target>               build and run (web, macos, ios, tvos, watchos, android)
//! prism dev <target>               run, then rebuild and restart on every save
//! prism build <target>             release artifacts (.app bundles, APK, server binary)
//! prism publish <target>           store builds, uploaded (Google Play, App Store
//!                                  Connect) or notarized (macOS Developer ID)
//! prism devices                    simulators, emulators and devices
//! ```
//!
//! Projects are configured by a `Prism.toml` next to their `Cargo.toml`. The
//! bundling scripts and platform sources (Apple, Android) are embedded in this binary,
//! so the command works in any project.

mod appstore;
mod config;
mod embedded;
mod islands;
mod jwt;
mod linux;
mod new;
mod play;
mod publish;
#[cfg(test)]
mod signing_tests;
mod watch;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};

use config::Project;

const USAGE: &str = "\
prism — Rust UI for the web, Apple platforms, Linux and Android

USAGE:
    prism new <name> [--prism <path>]       Create a project
    prism run <target> [options]            Build and run
    prism dev <target> [options]            Run, rebuild and restart on save (keeps the screen)
    prism build <target> [options]          Build release artifacts
    prism publish <target> [options]        Upload a store build (or notarize, macOS --direct)
    prism devices                           List simulators, emulators and devices

TARGETS:
    web, macos, ios, tvos, watchos, visionos, android, linux

OPTIONS:
    --device <name|udid>    iOS: a real iPhone/iPad (signed with your team);
                            Android: an adb serial (emulator-5554, a phone)
    --sim <name|udid>       iOS/tvOS/watchOS/visionOS: a simulator
    --release               Optimized build
    --path </route>         Start on this screen
    --bundle                macOS: run as a full .app (widgets, notifications)
    --port <port>           Web: listen on this port
    --aab                   Android: an App Bundle for Google Play (run: installed
                            through bundletool, like Play does)
    --store                 Apple: an App Store build (.ipa; macOS: a .pkg)
    --direct                macOS: Developer ID, hardened runtime, a .dmg
    --docker                Web: the production Docker image

PUBLISH OPTIONS:
    --dry-run               Build and check everything, send nothing
    --file <path>           Publish this .aab/.ipa/.pkg/.dmg instead of building
    --track <name>          Play: internal (default), alpha, beta, production
    --rollout <fraction>    Play: staged rollout (0.1 = 10 %)
    --draft                 Play: a draft release
    --notes <text>          Play: release notes (en-US)
    --service-account <p>   Play: the service account JSON (or PRISM_PLAY_SERVICE_ACCOUNT)
    --direct                macOS: notarize and staple the Developer ID .dmg
                            App Store Connect: PRISM_ASC_KEY_ID, PRISM_ASC_ISSUER_ID,
                            PRISM_ASC_KEY (.p8); notarization also PRISM_NOTARY_PROFILE
";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Target {
    Web,
    Macos,
    Ios,
    Tvos,
    Watchos,
    Visionos,
    Android,
    Linux,
}

#[derive(Default, Debug)]
struct Options {
    device: Option<String>,
    sim: Option<String>,
    release: bool,
    path: Option<String>,
    bundle: bool,
    port: Option<String>,
    /// Android App Bundle instead of an APK.
    aab: bool,
    /// Apple: App Store distribution build.
    store: bool,
    /// macOS: Developer ID distribution build.
    direct: bool,
    /// Web: the Docker image.
    docker: bool,
    /// `prism build`: islands above their size budget fail the build.
    strict_budget: bool,
}

/// The target's name in commands and output directories.
fn os_name(target: Target) -> &'static str {
    match target {
        Target::Web => "web",
        Target::Macos => "macos",
        Target::Ios => "ios",
        Target::Tvos => "tvos",
        Target::Watchos => "watchos",
        Target::Visionos => "visionos",
        Target::Android => "android",
        Target::Linux => "linux",
    }
}

fn parse_target(name: Option<&str>) -> Option<Target> {
    Some(match name? {
        "web" => Target::Web,
        "macos" | "mac" => Target::Macos,
        "ios" => Target::Ios,
        "tvos" | "tv" => Target::Tvos,
        "watchos" | "watch" => Target::Watchos,
        "visionos" | "vision" => Target::Visionos,
        "android" => Target::Android,
        "linux" => Target::Linux,
        _ => return None,
    })
}

const APPLE_MOBILE: [Target; 4] = [Target::Ios, Target::Tvos, Target::Watchos, Target::Visionos];

/// Flags that only make sense for some targets.
fn check_flags(target: Target, o: &Options) -> Result<(), String> {
    let wrong = |flag: &str, for_what: &str| Err(format!("{flag} is for {for_what}"));
    if o.aab && target != Target::Android {
        return wrong("--aab", "android");
    }
    if o.store && !(target == Target::Macos || APPLE_MOBILE.contains(&target)) {
        return wrong("--store", "Apple targets");
    }
    if o.direct && target != Target::Macos {
        return wrong("--direct", "macos");
    }
    if o.store && o.direct {
        return Err("--store (Mac App Store) or --direct (Developer ID), not both".into());
    }
    if o.docker && target != Target::Web {
        return wrong(
            "--docker",
            "web (prism build linux uses Docker by itself off Linux)",
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("prism: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(cmd) = args.first() else {
        print!("{USAGE}");
        return Ok(());
    };
    match cmd.as_str() {
        "new" => {
            let name = args.get(1).ok_or("usage: prism new <name>")?;
            let prism = flag(args, "--prism").map(PathBuf::from);
            new::create(name, prism)
        }
        "publish" => {
            let target = parse_target(args.get(1).map(String::as_str)).ok_or(
                "usage: prism publish <android|ios|tvos|watchos|visionos|macos> [--dry-run]",
            )?;
            let opts = publish::PublishOptions::parse(args)?;
            let project = Project::find()?;
            publish::publish(&project, target, &opts)
        }
        "run" | "dev" | "build" => {
            let target = parse_target(args.get(1).map(String::as_str)).ok_or_else(|| {
                format!("usage: prism {cmd} <web|macos|ios|tvos|watchos|visionos|android|linux>")
            })?;
            let has = |f: &str| args.iter().any(|a| a == f);
            let opts = Options {
                device: flag(args, "--device"),
                sim: flag(args, "--sim"),
                release: has("--release") || cmd == "build",
                path: flag(args, "--path"),
                bundle: has("--bundle"),
                port: flag(args, "--port"),
                aab: has("--aab"),
                store: has("--store"),
                direct: has("--direct"),
                docker: has("--docker"),
                strict_budget: cmd == "build",
            };
            check_flags(target, &opts)?;
            if cmd != "build" && (opts.store || opts.direct || opts.docker) {
                return Err("--store, --direct and --docker are for prism build / publish".into());
            }
            if target == Target::Linux && cmd != "build" && !cfg!(target_os = "linux") {
                return Err(
                    "run Linux apps on Linux (or in the dev image: see the README); prism build linux packages them from here"
                        .into(),
                );
            }
            let project = Project::find()?;
            match cmd.as_str() {
                "run" => run_target(&project, target, &opts),
                "dev" => watch::dev(&project, target, &opts),
                _ => build_target(&project, target, &opts),
            }
        }
        "devices" => devices(),
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n\n{USAGE}")),
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

// --- Running -------------------------------------------------------------------------

fn run_target(p: &Project, target: Target, o: &Options) -> Result<(), String> {
    match target {
        Target::Ios => apple_script(p, "ios-run.sh", o, false),
        Target::Tvos => apple_script_os(p, o, false, "tvos"),
        Target::Watchos => apple_script_os(p, o, false, "watchos"),
        Target::Visionos => apple_script_os(p, o, false, "visionos"),
        Target::Android => apple_script(p, "android-run.sh", o, false),
        Target::Macos if o.bundle => apple_script(p, "macos-app.sh", o, false),
        _ => {
            // Linux (on Linux) runs like macOS: the native binary.
            build(p, target, o)?;
            let status = spawn(p, target, o, &[])?
                .wait()
                .map_err(|e| e.to_string())?;
            status
                .success()
                .then_some(())
                .ok_or_else(|| format!("app exited with {status}"))
        }
    }
}

fn build_target(p: &Project, target: Target, o: &Options) -> Result<(), String> {
    match target {
        Target::Web if o.docker => {
            let tag = linux::web_docker(p)?;
            println!("Built the image {tag}");
            println!(
                "Run it: docker run -p 3000:3000 -v {}-data:/data {tag}",
                p.package.replace('_', "-")
            );
            Ok(())
        }
        Target::Linux => linux::build(p),
        Target::Web => {
            build(p, target, o)?;
            let bin = binary(p, target, o);
            // Hashed static files (client, CSS, assets) for a CDN.
            let public = bin.with_file_name("public");
            let _ = std::fs::remove_dir_all(&public);
            let status = spawn(
                p,
                target,
                o,
                &[("PRISM_EXPORT_STATIC", public.display().to_string())],
            )?
            .wait()
            .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("exporting static files failed ({status})"));
            }
            println!("Built {}", bin.display());
            println!(
                "Static files: {}/ (content-hashed, cache them forever)",
                public.display()
            );
            println!(
                "Run the server anywhere (PRISM_HOST, PRISM_PORT, PRISM_DATA_DIR). Behind a CDN:\n  \
                 upload public/ and set PRISM_ASSET_URL=https://cdn.example.com,\n  \
                 or let the CDN cache /_prism/* and /assets/* from the server (immutable).\n  \
                 Pages are `private, no-store`; proxy /ws as a WebSocket."
            );
            Ok(())
        }
        Target::Macos => apple_script(p, "macos-app.sh", o, true),
        Target::Ios => apple_script(p, "ios-run.sh", o, true),
        Target::Tvos => apple_script_os(p, o, true, "tvos"),
        Target::Watchos => apple_script_os(p, o, true, "watchos"),
        Target::Visionos => apple_script_os(p, o, true, "visionos"),
        Target::Android => apple_script(p, "android-run.sh", o, true),
    }
}

/// `cargo build` for the web server or the macOS binary.
fn build(p: &Project, target: Target, o: &Options) -> Result<(), String> {
    if target == Target::Web {
        // The islands first: their sizes print with every build.
        islands::build(p, o.release, o.strict_budget)?;
    }
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&p.root).args(["build", "-p", &p.package]);
    p.deployment_env(&mut cmd);
    if o.release {
        cmd.arg("--release");
    }
    if target == Target::Web {
        // A separate target dir: switching web ↔ native doesn't rebuild everything.
        cmd.args(["--no-default-features", "--features", "web", "--target-dir"])
            .arg(p.target_dir.join("web"));
    }
    let status = cmd.status().map_err(|e| format!("cargo: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "build failed".to_string())
}

fn binary(p: &Project, target: Target, o: &Options) -> PathBuf {
    let base = if target == Target::Web {
        p.target_dir.join("web")
    } else {
        p.target_dir.clone()
    };
    base.join(if o.release { "release" } else { "debug" })
        .join(&p.package)
}

/// Starts the built web server or macOS app.
fn spawn(
    p: &Project,
    target: Target,
    o: &Options,
    env: &[(&str, String)],
) -> Result<Child, String> {
    let mut cmd = Command::new(binary(p, target, o));
    cmd.current_dir(&p.root);
    if let Some(path) = &o.path {
        cmd.env("PRISM_PATH", path);
    }
    if let Some(port) = &o.port {
        cmd.env("PRISM_PORT", port);
    }
    if let Some(key) = &p.vapid_public_key {
        cmd.env("PRISM_VAPID_PUBLIC_KEY", key);
    }
    if target == Target::Web && !p.islands.is_empty() {
        cmd.env("PRISM_ISLANDS_DIR", islands::out_dir(p, o.release));
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.spawn()
        .map_err(|e| format!("could not start the app: {e}"))
}

/// Runs an embedded bundling script (`ios-run.sh`, `macos-app.sh`).
fn apple_script(p: &Project, script: &str, o: &Options, build_only: bool) -> Result<(), String> {
    let status = apple_command(p, script, o, build_only, &[])?
        .status()
        .map_err(|e| e.to_string())?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("{script} failed"))
}

/// The iOS script for another UIKit platform (`PRISM_OS=tvos`).
fn apple_script_os(p: &Project, o: &Options, build_only: bool, os: &str) -> Result<(), String> {
    let status = apple_command(
        p,
        "ios-run.sh",
        o,
        build_only,
        &[("PRISM_OS", os.to_string())],
    )?
    .status()
    .map_err(|e| e.to_string())?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("ios-run.sh ({os}) failed"))
}

fn apple_command(
    p: &Project,
    script: &str,
    o: &Options,
    build_only: bool,
    env: &[(&str, String)],
) -> Result<Command, String> {
    let platform = embedded::extract(&p.target_dir)?;
    let mut cmd = Command::new("bash");
    cmd.arg(platform.join("scripts").join(script))
        .arg(&p.package)
        .current_dir(&p.root)
        .env("PRISM_PROJECT", &p.root)
        .env("PRISM_PLATFORM", &platform)
        .env("PRISM_BUNDLE_ID", &p.bundle_id)
        .env("PRISM_APP_NAME", &p.name)
        .env("PRISM_VERSION", &p.version);
    // The scripts' cargo builds, Swift compiles and MinimumOSVersion.
    p.deployment_env(&mut cmd);
    if script == "android-run.sh" {
        cmd.env("PRISM_BUILD", p.version_code()?.to_string());
        if o.aab {
            cmd.env("PRISM_ANDROID_AAB", "1");
        }
    } else if let Some(build) = &p.build {
        cmd.env("PRISM_BUILD", build);
    }
    if let Some(category) = &p.category {
        cmd.env("PRISM_CATEGORY", category);
    }
    if p.uses_encryption == Some(false) {
        cmd.env("PRISM_ENCRYPTION", "0");
    }
    if o.store {
        cmd.env("PRISM_DIST", "appstore");
    } else if o.direct {
        cmd.env("PRISM_DIST", "developer-id");
    }
    if build_only {
        cmd.arg(if script == "macos-app.sh" {
            "--no-open"
        } else {
            "--no-launch"
        })
        .env("PRISM_NO_INSTALL", "1");
    }
    if let Some(icon) = &p.icon {
        cmd.env("PRISM_ICON", icon);
    }
    if p.icloud {
        cmd.env("PRISM_ICLOUD", "1");
    }
    if p.apple_sign_in {
        cmd.env("PRISM_APPLE_SIGNIN", "1");
    }
    // Remote push: entitlements, background mode, Android receivers.
    if let Some(env) = &p.push {
        cmd.env("PRISM_PUSH", env);
    }
    if let Some(g) = &p.google_services {
        cmd.env("PRISM_GOOGLE_SERVICES", g);
    }
    if let Some(key) = &p.vapid_public_key {
        cmd.env("PRISM_VAPID_PUBLIC_KEY", key);
    }
    if !p.permissions.is_empty() {
        let lines: Vec<String> = p
            .permissions
            .iter()
            .map(|(k, v)| format!("{k}\t{v}"))
            .collect();
        cmd.env("PRISM_PERMISSIONS", lines.join("\n"));
    }
    if !p.languages.is_empty() {
        cmd.env("PRISM_LOCALES", p.languages.join(","));
    }
    // `[network] allow_http` (#152): ATS exceptions, Android cleartext;
    // `allow_http_dev` too in development builds (#187).
    let http_hosts = p.http_hosts_for(o.release);
    if !http_hosts.is_empty() {
        cmd.env("PRISM_HTTP_HOSTS", http_hosts.join(","));
    }
    if o.release {
        cmd.env("PRISM_RELEASE", "1");
    }
    if let Some(path) = &o.path {
        cmd.env("PRISM_PATH", path);
    }
    if let Some(team) = &p.team {
        cmd.env("PRISM_TEAM", team);
    }
    match (&o.device, &o.sim) {
        (Some(d), _) if script == "android-run.sh" => {
            cmd.env("PRISM_SERIAL", d);
        }
        (Some(d), _) => {
            cmd.env("PRISM_DEVICE", d);
        }
        (None, Some(s)) => {
            cmd.env("PRISM_SIM", s);
        }
        // Prism.toml's defaults are the [ios] section's.
        (None, None) if script == "android-run.sh" => {}
        (None, None) => {
            if let Some(d) = &p.device {
                cmd.env("PRISM_DEVICE", d);
            } else if let Some(s) = &p.simulator {
                cmd.env("PRISM_SIM", s);
            }
        }
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    Ok(cmd)
}

fn devices() -> Result<(), String> {
    println!("Simulators (booted first):");
    let out = Command::new("xcrun")
        .args(["simctl", "list", "devices", "available", "-j"])
        .output()
        .map_err(|e| format!("xcrun: {e}"))?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let mut sims: Vec<(bool, String, String)> = Vec::new();
    if let Some(runtimes) = v["devices"].as_object() {
        for (runtime, list) in runtimes {
            if !runtime.contains("iOS") {
                continue;
            }
            for d in list.as_array().into_iter().flatten() {
                sims.push((
                    d["state"] != "Booted",
                    d["name"].as_str().unwrap_or("?").to_string(),
                    d["udid"].as_str().unwrap_or("?").to_string(),
                ));
            }
        }
    }
    sims.sort();
    for (shutdown, name, udid) in sims {
        println!("  {}{name}  {udid}", if shutdown { "  " } else { "● " });
    }
    println!("\nDevices (prism run ios --device <name>):");
    let tmp = std::env::temp_dir().join(format!("prism-devices-{}.json", std::process::id()));
    let ok = Command::new("xcrun")
        .args(["devicectl", "list", "devices", "--json-output"])
        .arg(&tmp)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    let json: serde_json::Value = std::fs::read(&tmp)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    let mut any = false;
    for d in json["result"]["devices"].as_array().into_iter().flatten() {
        let hw = &d["hardwareProperties"];
        if hw["platform"] != "iOS" || hw["reality"] != "physical" {
            continue;
        }
        any = true;
        println!(
            "  {}  {}  ({})",
            d["deviceProperties"]["name"].as_str().unwrap_or("?"),
            hw["udid"].as_str().unwrap_or("?"),
            d["connectionProperties"]["pairingState"]
                .as_str()
                .unwrap_or("?"),
        );
    }
    if !ok || !any {
        println!("  none (pair an iPhone with Xcode › Devices)");
    }
    android_devices();
    Ok(())
}

/// Running emulators and connected phones (`adb devices`), then the emulators
/// that can be started.
fn android_devices() {
    let sdk = std::env::var_os("ANDROID_HOME").map(PathBuf::from);
    let tool = |rel: &str, name: &str| {
        sdk.as_ref()
            .map(|s| s.join(rel))
            .filter(|p| p.exists())
            .unwrap_or_else(|| PathBuf::from(name))
    };
    println!("\nAndroid (prism run android --device <serial>):");
    let out = Command::new(tool("platform-tools/adb", "adb"))
        .args(["devices", "-l"])
        .output();
    let Ok(out) = out else {
        println!("  adb not found (set ANDROID_HOME; see the README)");
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut any = false;
    for line in text.lines().skip(1).filter(|l| !l.trim().is_empty()) {
        let mut parts = line.split_whitespace();
        let (Some(serial), Some(state)) = (parts.next(), parts.next()) else {
            continue;
        };
        let model = parts
            .find_map(|p| p.strip_prefix("model:"))
            .unwrap_or("")
            .replace('_', " ");
        any = true;
        println!("  ● {serial}  {model}  ({state})");
    }
    if !any {
        println!("  none running");
    }
    if let Ok(out) = Command::new(tool("emulator/emulator", "emulator"))
        .arg("-list-avds")
        .output()
    {
        let avds = String::from_utf8_lossy(&out.stdout);
        let avds: Vec<&str> = avds.lines().filter(|l| !l.trim().is_empty()).collect();
        if !avds.is_empty() {
            println!(
                "  emulators: {} (start one: emulator -avd <name>)",
                avds.join(", ")
            );
        }
    }
}

/// `path` relative to `base` unless absolute.
fn resolve(base: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}
