//! `Prism.toml`: the app's name, bundle id, icon and Apple settings.
//!
//! ```toml
//! [app]
//! name = "Counter"              # display name
//! package = "counter"           # cargo package (default: the Cargo.toml's)
//! bundle_id = "com.example.counter"
//! version = "1.0.0"
//! icon = "icon.png"             # 1024×1024 PNG
//! build = 7                     # build number: CFBundleVersion, Android versionCode
//! category = "public.app-category.productivity"  # App Store category (macOS needs one)
//! uses_encryption = false       # export compliance (ITSAppUsesNonExemptEncryption)
//! description = "Counts things" # Linux packages
//!
//! [android]
//! version_code = 7              # instead of [app] build
//! track = "internal"            # `prism publish android` (internal, alpha, beta, production)
//!
//! [linux]
//! maintainer = "Jane <jane@example.com>"  # .deb Maintainer
//!
//! [permissions]               # why the app asks (shown by the system)
//! location = "Shows todos near you."
//! camera = "Takes a photo for a todo."
//!
//! [push]                      # remote push notifications
//! enabled = true                # aps-environment, background pushes, Android receivers
//! apns_environment = "development"  # "production" for TestFlight / App Store builds
//! google_services = "google-services.json"  # Android: FCM (else UnifiedPush only)
//! vapid_public_key = "BKv…"     # Web Push (or PRISM_VAPID_PUBLIC_KEY)
//!
//! [ios]
//! team = "ABCDE12345"           # Apple team for real devices
//! simulator = "iPhone 17"       # default simulator
//! device = "My iPhone"          # default device (instead of a simulator)
//! sign_in_with_apple = true     # the com.apple.developer.applesignin entitlement
//! associated_domains = ["webcredentials:example.com"]  # passkeys, universal links (on one line)
//! deployment_target = "17.0"    # oldest iOS/iPadOS (IPHONEOS_DEPLOYMENT_TARGET)
//!
//! [macos]                       # also [tvos], [watchos], [visionos]
//! deployment_target = "13.0"    # MACOSX_ (TVOS_, WATCHOS_, XROS_) DEPLOYMENT_TARGET
//!
//! [network]
//! allow_http = ["preview.localhost"]  # plain-HTTP hosts (and their subdomains), on one line
//! allow_http_dev = ["192.168.1.20"]   # the same, in development builds only (not --release)
//!
//! [web]
//! islands = ["islands/chart"]   # WASM island crates (on one line)
//! island_budget_kb = 40         # gzip, per island: `prism build web` fails above
//! ```
//!
//! Deployment targets default to iOS 17, tvOS 17, watchOS 10, visionOS 26 and
//! macOS 13; the environment variable wins when it is set.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Project {
    /// Directory holding Prism.toml (or Cargo.toml).
    pub root: PathBuf,
    pub package: String,
    pub name: String,
    pub bundle_id: String,
    pub version: String,
    pub icon: Option<PathBuf>,
    /// `[app] build`: CFBundleVersion and the default Android versionCode.
    pub build: Option<String>,
    /// `[android] version_code` (default: `build`).
    pub version_code: Option<String>,
    /// `[android] track`: the Play track `prism publish android` releases to.
    pub play_track: Option<String>,
    /// `[app] category`: LSApplicationCategoryType (`public.app-category.…`).
    pub category: Option<String>,
    /// `[app] uses_encryption`: false declares export compliance up front.
    pub uses_encryption: Option<bool>,
    /// `[app] description`: one line, for Linux packages.
    pub description: Option<String>,
    /// `[linux] maintainer`: the .deb Maintainer field.
    pub maintainer: Option<String>,
    /// `[permissions]`: why the app asks (Info.plist usage descriptions).
    pub permissions: Vec<(String, String)>,
    /// Languages with a catalog in `locales/` (`[app] locales` to move it).
    pub languages: Vec<String>,
    pub team: Option<String>,
    pub simulator: Option<String>,
    pub device: Option<String>,
    /// `[ios] icloud = true`: iCloud key-value storage.
    pub icloud: bool,
    /// `[ios] sign_in_with_apple = true`: the Sign in with Apple entitlement.
    pub apple_sign_in: bool,
    /// `[ios] associated_domains`: the `com.apple.developer.associated-domains`
    /// entitlement (`webcredentials:` for passkeys, `applinks:` for
    /// universal links), checked (#191).
    pub associated_domains: Vec<String>,
    /// `[push] enabled = true`: the APNs environment (`development` unless
    /// `apns_environment = "production"`).
    pub push: Option<String>,
    /// `[push] google_services`: Firebase's `google-services.json` (FCM on Android).
    pub google_services: Option<PathBuf>,
    /// `[push] vapid_public_key`: the web app's VAPID key.
    pub vapid_public_key: Option<String>,
    /// Cargo's target directory (the workspace's).
    pub target_dir: PathBuf,
    /// The Cargo workspace (watched by `prism dev`).
    pub workspace_root: PathBuf,
    /// Deployment targets for every `cargo` the CLI runs, as
    /// `(environment variable, version)`: `deployment_target` of `[ios]`,
    /// `[tvos]`, `[watchos]`, `[visionos]` and `[macos]`, else Prism's defaults.
    pub deployment_targets: Vec<(&'static str, String)>,

    // --- Islands: WASM Web Components for the web ---
    /// `[web] islands`: the island crates' directories.
    pub islands: Vec<PathBuf>,
    /// `[web] island_budget_kb`: the gzip budget of each island.
    pub island_budget_kb: f64,

    /// `[network] allow_http`: hosts (and their subdomains) the app may
    /// reach over plain HTTP: App Transport Security exceptions on Apple
    /// platforms, cleartext traffic on Android (#152).
    pub http_hosts: Vec<String>,
    /// `[network] allow_http_dev`: the same, for development builds only
    /// (`prism run` / `dev` without `--release`): release builds and
    /// store uploads leave them out (#187).
    pub http_hosts_dev: Vec<String>,
}

impl Project {
    /// The plain-HTTP hosts of a build: `allow_http`, plus
    /// `allow_http_dev` unless it is a release build (#187).
    pub fn http_hosts_for(&self, release: bool) -> Vec<String> {
        let mut hosts = self.http_hosts.clone();
        if !release {
            for h in &self.http_hosts_dev {
                if !hosts.contains(h) {
                    hosts.push(h.clone());
                }
            }
        }
        hosts
    }
}

/// The `Prism.toml` section of each deployment-target variable.
const DEPLOYMENT_SECTIONS: [(&str, &str); 5] = [
    ("ios", "IPHONEOS_DEPLOYMENT_TARGET"),
    ("tvos", "TVOS_DEPLOYMENT_TARGET"),
    ("watchos", "WATCHOS_DEPLOYMENT_TARGET"),
    ("visionos", "XROS_DEPLOYMENT_TARGET"),
    ("macos", "MACOSX_DEPLOYMENT_TARGET"),
];

/// `deployment_target` of each platform section, else Prism's default
/// (`prism_apple_build::DEPLOYMENT_TARGETS`).
fn deployment_targets(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Vec<(&'static str, String)>, String> {
    prism_apple_build::DEPLOYMENT_TARGETS
        .iter()
        .map(|&(var, default)| {
            let section = DEPLOYMENT_SECTIONS
                .iter()
                .find(|(_, v)| *v == var)
                .map_or("ios", |(s, _)| *s);
            let Some(version) = get(&format!("{section}.deployment_target")) else {
                return Ok((var, default.to_string()));
            };
            let valid = !version.is_empty()
                && version.split('.').count() <= 3
                && version
                    .split('.')
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
            if valid {
                Ok((var, version))
            } else {
                Err(format!(
                    "Prism.toml [{section}] deployment_target: a version like \"{default}\", not {version:?}"
                ))
            }
        })
        .collect()
}

/// A minimal TOML reader for `[section]` and `key = "value"` lines.
pub(crate) fn parse(source: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut section = String::new();
    for line in source.lines() {
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = s.trim().to_string();
        } else if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            // "basic" or 'literal' strings; anything else as written.
            let v = ['"', '\'']
                .iter()
                .find_map(|q| v.strip_prefix(*q)?.strip_suffix(*q))
                .unwrap_or(v);
            out.insert(format!("{section}.{}", k.trim()), v.to_string());
        }
    }
    out
}

/// The line up to a `#` that is not inside a quoted string.
fn strip_comment(line: &str) -> &str {
    let mut quote = None;
    for (i, c) in line.char_indices() {
        match (c, quote) {
            ('"' | '\'', None) => quote = Some(c),
            (c, Some(q)) if c == q => quote = None,
            ('#', None) => return &line[..i],
            _ => {}
        }
    }
    line
}

impl Project {
    /// The project around the current directory.
    pub fn find() -> Result<Self, String> {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let root = cwd
            .ancestors()
            .find(|d| d.join("Prism.toml").exists())
            .or_else(|| cwd.ancestors().find(|d| d.join("Cargo.toml").exists()))
            .ok_or("no Prism.toml or Cargo.toml here (create a project with `prism new`)")?
            .to_path_buf();
        Self::load(&root)
    }

    pub fn load(root: &Path) -> Result<Self, String> {
        let config = std::fs::read_to_string(root.join("Prism.toml"))
            .map(|s| parse(&s))
            .unwrap_or_default();
        let cargo = std::fs::read_to_string(root.join("Cargo.toml"))
            .map(|s| parse(&s))
            .unwrap_or_default();
        let get = |k: &str| config.get(k).filter(|v| !v.is_empty()).cloned();
        let package = get("app.package")
            .or_else(|| cargo.get("package.name").cloned())
            .ok_or("set [app] package in Prism.toml (no [package] in Cargo.toml)")?;
        let name = get("app.name").unwrap_or_else(|| {
            let mut c = package.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        });
        let (target_dir, workspace_root) = cargo_dirs(root)?;
        let deployment_targets = deployment_targets(get)?;
        Ok(Project {
            bundle_id: get("app.bundle_id")
                .unwrap_or_else(|| format!("dev.prism.{}", package.replace('_', "-"))),
            version: get("app.version")
                .or_else(|| cargo.get("package.version").cloned())
                .filter(|v| !v.contains("workspace"))
                .unwrap_or_else(|| "0.1.0".into()),
            icon: get("app.icon").map(|i| crate::resolve(root, &i)),
            build: get("app.build"),
            version_code: get("android.version_code"),
            play_track: get("android.track"),
            category: get("app.category"),
            uses_encryption: get("app.uses_encryption").map(|v| v == "true"),
            description: get("app.description"),
            maintainer: get("linux.maintainer"),
            permissions: {
                let mut p: Vec<(String, String)> = config
                    .iter()
                    .filter_map(|(k, v)| {
                        Some((k.strip_prefix("permissions.")?.to_string(), v.clone()))
                    })
                    .collect();
                p.sort();
                p
            },
            languages: languages(&crate::resolve(
                root,
                &get("app.locales").unwrap_or_else(|| "locales".into()),
            )),
            team: get("ios.team"),
            simulator: get("ios.simulator"),
            device: get("ios.device"),
            icloud: get("ios.icloud").is_some_and(|v| v == "true"),
            apple_sign_in: get("ios.sign_in_with_apple").is_some_and(|v| v == "true"),
            associated_domains: get("ios.associated_domains")
                .map(|v| parse_list(&v))
                .unwrap_or_default()
                .into_iter()
                .map(|d| associated_domain(&d))
                .collect::<Result<_, _>>()?,
            push: get("push.enabled")
                .filter(|v| v == "true")
                .map(|_| get("push.apns_environment").unwrap_or_else(|| "development".into())),
            google_services: get("push.google_services").map(|g| crate::resolve(root, &g)),
            vapid_public_key: get("push.vapid_public_key"),
            root: root.to_path_buf(),
            package,
            name,
            target_dir,
            workspace_root,
            deployment_targets,
            islands: get("web.islands")
                .map(|v| parse_list(&v))
                .unwrap_or_default()
                .iter()
                .map(|d| crate::resolve(root, d))
                .collect(),
            island_budget_kb: match get("web.island_budget_kb") {
                Some(v) => v.parse::<f64>().ok().filter(|b| *b > 0.).ok_or_else(|| {
                    format!("Prism.toml [web] island_budget_kb: a number of KB, not {v:?}")
                })?,
                None => crate::islands::DEFAULT_BUDGET_KB,
            },
            http_hosts: get("network.allow_http")
                .map(|v| parse_list(&v))
                .unwrap_or_default()
                .into_iter()
                .map(|h| http_host(&h))
                .collect::<Result<_, _>>()?,
            http_hosts_dev: get("network.allow_http_dev")
                .map(|v| parse_list(&v))
                .unwrap_or_default()
                .into_iter()
                .map(|h| http_host(&h))
                .collect::<Result<_, _>>()?,
        })
    }
}

/// A host of `[network] allow_http`: a domain (`preview.localhost`,
/// `dev.example.com`) or an IP address, lower case, without a scheme, port
/// or path (they would not match).
fn http_host(raw: &str) -> Result<String, String> {
    let host = raw.trim().to_ascii_lowercase();
    let ok = !host.is_empty()
        && !host.starts_with(['.', '-'])
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if ok {
        Ok(host)
    } else {
        Err(format!(
            "Prism.toml [network] allow_http: {raw:?} is not a host name (no scheme, port or path: \"preview.localhost\")"
        ))
    }
}

/// An associated domain (#191): `service:host`, with an optional
/// `?mode=developer|managed|developer+managed`. The services Apple knows:
/// `webcredentials` (passkeys, passwords), `applinks` (universal links),
/// `activitycontinuation`, `appclips`. The host may start with `*.`.
fn associated_domain(raw: &str) -> Result<String, String> {
    const SERVICES: [&str; 4] = [
        "webcredentials",
        "applinks",
        "activitycontinuation",
        "appclips",
    ];
    let entry = raw.trim();
    let err = || {
        format!(
            "Prism.toml [ios] associated_domains: {raw:?} is not service:host (\"webcredentials:example.com\"; services: {})",
            SERVICES.join(", ")
        )
    };
    let (service, rest) = entry.split_once(':').ok_or_else(err)?;
    let (host, mode) = match rest.split_once("?mode=") {
        Some((h, m)) => (h, Some(m)),
        None => (rest, None),
    };
    let host = host.to_ascii_lowercase();
    let bare = host.strip_prefix("*.").unwrap_or(&host);
    let host_ok = !bare.is_empty()
        && bare.contains('.')
        && !bare.starts_with(['.', '-'])
        && bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    let mode_ok = mode.is_none_or(|m| matches!(m, "developer" | "managed" | "developer+managed"));
    if !SERVICES.contains(&service) || !host_ok || !mode_ok {
        return Err(err());
    }
    Ok(match mode {
        Some(m) => format!("{service}:{host}?mode={m}"),
        None => format!("{service}:{host}"),
    })
}

/// A one-line TOML array of strings: `["a", 'b']` → `a`, `b`.
pub(crate) fn parse_list(raw: &str) -> Vec<String> {
    let inner = raw.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

impl Project {
    /// Sets the deployment targets on a command that runs `cargo` (directly
    /// or through a bundling script), except those the environment sets.
    pub fn deployment_env(&self, cmd: &mut Command) {
        for (var, version) in &self.deployment_targets {
            if std::env::var_os(var).is_none() {
                cmd.env(var, version);
            }
        }
    }
}

impl Project {
    /// The Android versionCode: `[android] version_code`, else `[app] build`, else 1.
    pub fn version_code(&self) -> Result<u32, String> {
        let raw = self
            .version_code
            .as_deref()
            .or(self.build.as_deref())
            .unwrap_or("1");
        parse_version_code(raw)
    }
}

/// Play accepts versionCodes from 1 to 2,100,000,000.
pub fn parse_version_code(raw: &str) -> Result<u32, String> {
    match raw.trim().parse::<u32>() {
        Ok(v) if (1..=2_100_000_000).contains(&v) => Ok(v),
        _ => Err(format!(
            "versionCode must be a whole number from 1 to 2100000000 (Prism.toml [app] build or [android] version_code), not {raw:?}"
        )),
    }
}

fn languages(dir: &Path) -> Vec<String> {
    let mut langs: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension()? == "json").then(|| p.file_stem()?.to_str().map(str::to_string))?
        })
        .collect();
    langs.sort();
    langs
}

fn cargo_dirs(root: &Path) -> Result<(PathBuf, PathBuf), String> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let dir = |k: &str| {
        v[k].as_str()
            .map(PathBuf::from)
            .ok_or_else(|| format!("cargo metadata: no {k}"))
    };
    Ok((dir("target_directory")?, dir("workspace_root")?))
}

#[cfg(test)]
#[path = "config_tests.rs"]
pub(crate) mod more_tests;

#[cfg(test)]
mod tests {
    #[test]
    fn parses_sections_and_comments() {
        let c = super::parse(
            "# app\n[app]\nname = \"My App\" # shown on the home screen\nicon = \"icon.png\"\n\n[ios]\nteam = \"ABC\"\n",
        );
        assert_eq!(c["app.name"], "My App");
        assert_eq!(c["app.icon"], "icon.png");
        assert_eq!(c["ios.team"], "ABC");
    }

    #[test]
    fn parses_store_settings() {
        let c = super::parse(
            "[app]\nbuild = 42\ncategory = \"public.app-category.games\"\n[android]\ntrack = \"beta\" # testers\n",
        );
        assert_eq!(c["app.build"], "42");
        assert_eq!(c["app.category"], "public.app-category.games");
        assert_eq!(c["android.track"], "beta");
    }

    #[test]
    fn deployment_targets_default_to_prisms_and_read_each_section() {
        let c = super::parse(
            "[ios]\ndeployment_target = \"18.0\"\n[macos]\ndeployment_target = \"14\"\n",
        );
        let get = |k: &str| c.get(k).cloned();
        let t = super::deployment_targets(get).unwrap();
        let find = |var: &str| t.iter().find(|(v, _)| *v == var).unwrap().1.clone();
        assert_eq!(find("IPHONEOS_DEPLOYMENT_TARGET"), "18.0");
        assert_eq!(find("MACOSX_DEPLOYMENT_TARGET"), "14");
        assert_eq!(find("TVOS_DEPLOYMENT_TARGET"), "17.0");
        assert_eq!(find("WATCHOS_DEPLOYMENT_TARGET"), "10.0");
        assert_eq!(find("XROS_DEPLOYMENT_TARGET"), "26.0");
        assert_eq!(t.len(), 5);
        let bad = super::parse("[watchos]\ndeployment_target = \"ten\"\n");
        let err = super::deployment_targets(|k| bad.get(k).cloned()).unwrap_err();
        assert!(err.contains("[watchos]"), "{err}");
    }

    #[test]
    fn checks_version_codes() {
        assert_eq!(super::parse_version_code("7"), Ok(7));
        assert_eq!(super::parse_version_code("2100000000"), Ok(2_100_000_000));
        assert!(super::parse_version_code("0").is_err());
        assert!(super::parse_version_code("2100000001").is_err());
        assert!(super::parse_version_code("1.2").is_err());
    }
}
