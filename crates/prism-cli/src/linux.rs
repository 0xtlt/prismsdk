//! `prism build linux` (a .deb, built in the Linux dev image when not on
//! Linux) and `prism build web --docker` (the production web image).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Project;

const DEV_IMAGE: &str = "prism-linux-dev";
/// Cargo's registry, kept between container runs.
const REGISTRY_VOLUME: &str = "prism-cargo-registry";

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("{what}: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("{what} failed ({status})"))
}

/// The packaging script's settings, from Prism.toml.
fn package_env(p: &Project) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("PRISM_BUNDLE_ID", p.bundle_id.clone()),
        ("PRISM_APP_NAME", p.name.clone()),
        ("PRISM_VERSION", p.version.clone()),
        ("PRISM_BUILD", p.build.clone().unwrap_or_else(|| "1".into())),
    ];
    if let Some(d) = &p.description {
        env.push(("PRISM_DESCRIPTION", d.clone()));
    }
    if let Some(m) = &p.maintainer {
        env.push(("PRISM_MAINTAINER", m.clone()));
    }
    env
}

/// `path` inside the container, where `root` is mounted at `/src`.
fn in_container(root: &Path, path: &Path) -> Result<String, String> {
    let rel = path.strip_prefix(root).map_err(|_| {
        format!(
            "{} is outside the workspace {} (the container only sees the workspace)",
            path.display(),
            root.display()
        )
    })?;
    Ok(Path::new("/src").join(rel).display().to_string())
}

/// The `docker run` arguments that package the app in the dev image.
pub fn docker_args(p: &Project, platform: &Path, out: &Path) -> Result<Vec<String>, String> {
    let mut args: Vec<String> = vec!["run".into(), "--rm".into()];
    let mut mount = |host: &Path, guest: &str, ro: bool| {
        args.push("-v".into());
        args.push(format!(
            "{}:{guest}{}",
            host.display(),
            if ro { ":ro" } else { "" }
        ));
    };
    mount(&p.workspace_root, "/src", false);
    mount(out, "/out", false);
    mount(platform, "/prism", true);
    args.extend([
        "-v".into(),
        format!("{REGISTRY_VOLUME}:/usr/local/cargo/registry"),
    ]);
    let project = in_container(&p.workspace_root, &p.root)?;
    let mut env = package_env(p);
    // Its own target dir: the host's (maybe a symlink) holds macOS builds.
    env.push(("CARGO_TARGET_DIR", "/out/target".into()));
    env.push(("PRISM_LINUX_OUT", "/out".into()));
    env.push(("PRISM_PROJECT", project.clone()));
    if let Some(icon) = &p.icon {
        env.push(("PRISM_ICON", in_container(&p.workspace_root, icon)?));
    }
    for (k, v) in env {
        args.push("-e".into());
        args.push(format!("{k}={v}"));
    }
    args.extend(["-w".into(), project, DEV_IMAGE.into()]);
    args.extend([
        "bash".into(),
        "/prism/scripts/linux-package.sh".into(),
        p.package.clone(),
    ]);
    Ok(args)
}

/// Builds the .deb: here on Linux, else in the dev image (built if missing).
pub fn build(p: &Project) -> Result<(), String> {
    let platform = crate::embedded::extract(&p.target_dir)?;
    if cfg!(target_os = "linux") {
        let mut cmd = Command::new("bash");
        cmd.arg(platform.join("scripts/linux-package.sh"))
            .arg(&p.package)
            .current_dir(&p.root)
            .env("PRISM_PROJECT", &p.root);
        for (k, v) in package_env(p) {
            cmd.env(k, v);
        }
        if let Some(icon) = &p.icon {
            cmd.env("PRISM_ICON", icon);
        }
        return run(&mut cmd, "linux-package.sh");
    }
    let has_image = Command::new("docker")
        .args(["image", "inspect", DEV_IMAGE])
        .output()
        .map_err(|e| format!("docker: {e} (Linux packages are built in Docker on this OS)"))?
        .status
        .success();
    if !has_image {
        println!("Building the {DEV_IMAGE} image (Rust + GTK 4)…");
        let docker = platform.join("docker");
        run(
            Command::new("docker")
                .args(["build", "-t", DEV_IMAGE, "-f"])
                .arg(docker.join("linux-dev.Dockerfile"))
                .arg(&docker),
            "docker build",
        )?;
    }
    let out = p.target_dir.join("linux");
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    // Docker mounts real paths: target/ may be a symlink.
    let out = out.canonicalize().map_err(|e| e.to_string())?;
    run(
        Command::new("docker").args(docker_args(p, &platform, &out)?),
        "docker run",
    )
}

/// Path dependencies the image's build context (the workspace) would miss.
fn outside_deps(p: &Project) -> Result<Vec<PathBuf>, String> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(&p.root)
        .output()
        .map_err(|e| format!("cargo: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    Ok(v["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|pkg| pkg["source"].is_null())
        .filter_map(|pkg| pkg["manifest_path"].as_str().map(PathBuf::from))
        .filter(|m| !m.starts_with(&p.workspace_root))
        .filter_map(|m| m.parent().map(Path::to_path_buf))
        .collect())
}

/// `docker build` of the production web image, tagged `<name>:<version>`.
pub fn web_docker(p: &Project) -> Result<String, String> {
    let outside = outside_deps(p)?;
    if !outside.is_empty() {
        return Err(format!(
            "the image is built from the workspace ({}), but these path dependencies are outside it: {}\n\
             (depend on Prism from git or crates.io, or put the project inside the Prism workspace)",
            p.workspace_root.display(),
            outside
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let platform = crate::embedded::extract(&p.target_dir)?;
    let dockerfile = platform.join("docker/web.Dockerfile");
    // BuildKit reads <Dockerfile>.dockerignore before the context's own:
    // keep target/ and local data out of the context.
    std::fs::write(
        platform.join("docker/web.Dockerfile.dockerignore"),
        "target\n**/target\n**/.prism-data\n.git\n",
    )
    .map_err(|e| e.to_string())?;
    let tag = format!("{}:{}", p.package.replace('_', "-"), p.version);
    // WASM islands: built here (target/ stays out of the context), passed
    // as the `islands` build context.
    let islands = crate::islands::build(p, true, true)?;
    let mut cmd = Command::new("docker");
    cmd.args(["build", "-f"]).arg(&dockerfile).args([
        "--build-arg",
        &format!("PACKAGE={}", p.package),
        "-t",
        &tag,
    ]);
    if let Some(dir) = islands {
        cmd.arg("--build-context")
            .arg(format!("islands={}", dir.display()));
    }
    run(cmd.arg(&p.workspace_root), "docker build")?;
    Ok(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        Project {
            root: "/w/examples/app".into(),
            package: "my_app".into(),
            name: "My App".into(),
            bundle_id: "com.example.app".into(),
            version: "1.2.0".into(),
            icon: Some("/w/examples/app/icon.png".into()),
            build: Some("7".into()),
            version_code: None,
            play_track: None,
            category: None,
            uses_encryption: None,
            description: Some("Counts".into()),
            maintainer: None,
            permissions: vec![],
            languages: vec![],
            team: None,
            simulator: None,
            device: None,
            icloud: false,
            apple_sign_in: false,
            push: None,
            google_services: None,
            vapid_public_key: None,
            target_dir: "/w/target".into(),
            workspace_root: "/w".into(),
            deployment_targets: vec![],
            islands: vec![],
            island_budget_kb: 40.,
            http_hosts: Vec::new(),
        }
    }

    #[test]
    fn packages_in_the_dev_image() {
        let args = docker_args(
            &project(),
            Path::new("/w/target/prism/platform"),
            Path::new("/real/linux"),
        )
        .unwrap();
        let line = args.join(" ");
        assert!(
            line.starts_with(
                "run --rm -v /w:/src -v /real/linux:/out -v /w/target/prism/platform:/prism:ro"
            ),
            "{line}"
        );
        for part in [
            "-e PRISM_BUNDLE_ID=com.example.app",
            "-e PRISM_BUILD=7",
            "-e PRISM_DESCRIPTION=Counts",
            "-e CARGO_TARGET_DIR=/out/target",
            "-e PRISM_ICON=/src/examples/app/icon.png",
            "-w /src/examples/app prism-linux-dev bash /prism/scripts/linux-package.sh my_app",
        ] {
            assert!(line.contains(part), "{part} in {line}");
        }
        let mut outside = project();
        outside.icon = Some("/elsewhere/icon.png".into());
        assert!(docker_args(&outside, Path::new("/p"), Path::new("/o")).is_err());
    }
}
