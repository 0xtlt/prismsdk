//! `Prism.toml`: the app's name, bundle id, icon and Apple settings.
//!
//! ```toml
//! [app]
//! name = "Counter"              # display name
//! package = "counter"           # cargo package (default: the Cargo.toml's)
//! bundle_id = "com.example.counter"
//! version = "1.0.0"
//! icon = "icon.png"             # 1024×1024 PNG
//!
//! [permissions]               # why the app asks (shown by the system)
//! location = "Shows todos near you."
//! camera = "Takes a photo for a todo."
//!
//! [ios]
//! team = "ABCDE12345"           # Apple team for real devices
//! simulator = "iPhone 17"       # default simulator
//! device = "My iPhone"          # default device (instead of a simulator)
//! ```

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
    /// `[permissions]`: why the app asks (Info.plist usage descriptions).
    pub permissions: Vec<(String, String)>,
    /// Languages with a catalog in `locales/` (`[app] locales` to move it).
    pub languages: Vec<String>,
    pub team: Option<String>,
    pub simulator: Option<String>,
    pub device: Option<String>,
    /// Cargo's target directory (the workspace's).
    pub target_dir: PathBuf,
    /// The Cargo workspace (watched by `prism dev`).
    pub workspace_root: PathBuf,
}

/// A minimal TOML reader for `[section]` and `key = "value"` lines.
fn parse(source: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut section = String::new();
    for line in source.lines() {
        let line = line.split(" #").next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = s.trim().to_string();
        } else if let Some((k, v)) = line.split_once('=') {
            let v = v.trim().trim_matches('"').to_string();
            out.insert(format!("{section}.{}", k.trim()), v);
        }
    }
    out
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
        Ok(Project {
            bundle_id: get("app.bundle_id")
                .unwrap_or_else(|| format!("dev.prism.{}", package.replace('_', "-"))),
            version: get("app.version")
                .or_else(|| cargo.get("package.version").cloned())
                .filter(|v| !v.contains("workspace"))
                .unwrap_or_else(|| "0.1.0".into()),
            icon: get("app.icon").map(|i| crate::resolve(root, &i)),
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
            root: root.to_path_buf(),
            package,
            name,
            target_dir,
            workspace_root,
        })
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
}
