//! `prism`: create, run, build and live-restart Prism apps.
//!
//! ```text
//! prism new my-app                 create a project
//! prism run web|macos|ios          build and run (ios: simulator, or --device)
//! prism dev web|macos|ios          run, then rebuild and restart on every save
//! prism build web|macos|ios        release artifacts (.app bundles, server binary)
//! prism devices                    simulators and paired devices
//! ```
//!
//! Projects are configured by a `Prism.toml` next to their `Cargo.toml`. The
//! Apple bundling scripts and platform sources are embedded in this binary,
//! so the command works in any project.

mod config;
mod embedded;
mod new;
mod watch;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};

use config::Project;

const USAGE: &str = "\
prism — Rust UI for the web, macOS and iOS

USAGE:
    prism new <name> [--prism <path>]       Create a project
    prism run <web|macos|ios> [options]     Build and run
    prism dev <web|macos|ios> [options]     Run, rebuild and restart on save (keeps the screen)
    prism build <web|macos|ios> [options]   Build release artifacts
    prism devices                           List simulators and paired iOS devices

OPTIONS:
    --device <name|udid>    iOS: a real iPhone/iPad (signed with your team)
    --sim <name|udid>       iOS: a simulator
    --release               Optimized build
    --path </route>         Start on this screen
    --bundle                macOS: run as a full .app (widgets, notifications)
    --port <port>           Web: listen on this port
";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Target {
    Web,
    Macos,
    Ios,
}

#[derive(Default, Debug)]
struct Options {
    device: Option<String>,
    sim: Option<String>,
    release: bool,
    path: Option<String>,
    bundle: bool,
    port: Option<String>,
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
        "run" | "dev" | "build" => {
            let target = match args.get(1).map(String::as_str) {
                Some("web") => Target::Web,
                Some("macos" | "mac") => Target::Macos,
                Some("ios") => Target::Ios,
                _ => return Err(format!("usage: prism {cmd} <web|macos|ios>")),
            };
            let opts = Options {
                device: flag(args, "--device"),
                sim: flag(args, "--sim"),
                release: args.iter().any(|a| a == "--release") || cmd == "build",
                path: flag(args, "--path"),
                bundle: args.iter().any(|a| a == "--bundle"),
                port: flag(args, "--port"),
            };
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
        Target::Macos if o.bundle => apple_script(p, "macos-app.sh", o, false),
        _ => {
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
    }
}

/// `cargo build` for the web server or the macOS binary.
fn build(p: &Project, target: Target, o: &Options) -> Result<(), String> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&p.root).args(["build", "-p", &p.package]);
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
        (Some(d), _) => {
            cmd.env("PRISM_DEVICE", d);
        }
        (None, Some(s)) => {
            cmd.env("PRISM_SIM", s);
        }
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
    Ok(())
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
