//! `prism dev`: run the app, rebuild on every save, restart it on the same
//! screen. The app saves its route to `PRISM_DEV_STATE`; the web client
//! reconnects by itself and re-renders its current page.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant, SystemTime};

use crate::config::Project;
use crate::{Options, Target};

pub fn dev(p: &Project, target: Target, o: &Options) -> Result<(), String> {
    let state = p.target_dir.join("prism").join("dev-route");
    std::fs::create_dir_all(state.parent().unwrap()).map_err(|e| e.to_string())?;
    let _ = std::fs::write(&state, o.path.as_deref().unwrap_or("/"));
    let mut child: Option<Child> = None;
    let mut seen = snapshot(&p.workspace_root);
    loop {
        let started = Instant::now();
        let route = std::fs::read_to_string(&state).unwrap_or_else(|_| "/".into());
        let env = [
            ("PRISM_DEV_STATE", state.display().to_string()),
            ("PRISM_PATH", route.clone()),
        ];
        match restart(p, target, o, &env, &mut child) {
            Ok(()) => eprintln!(
                "\x1b[32mprism dev\x1b[0m: running on {route} ({:.1}s) — edit and save to restart",
                started.elapsed().as_secs_f32()
            ),
            Err(e) => eprintln!("\x1b[31mprism dev\x1b[0m: {e} — fix it and save"),
        }
        // Wait for a change (debounced: editors write several files).
        loop {
            std::thread::sleep(Duration::from_millis(300));
            if let Some(c) = &mut child
                && let Ok(Some(status)) = c.try_wait()
            {
                eprintln!("prism dev: app exited ({status}); restarting on the next save");
                child = None;
            }
            let now = snapshot(&p.workspace_root);
            if now != seen {
                std::thread::sleep(Duration::from_millis(150));
                seen = snapshot(&p.workspace_root);
                break;
            }
        }
    }
}

fn restart(
    p: &Project,
    target: Target,
    o: &Options,
    env: &[(&str, String)],
    child: &mut Option<Child>,
) -> Result<(), String> {
    if target == Target::Ios {
        // Builds, reinstalls and relaunches (the simulator terminates the old process).
        let status = crate::apple_command(p, "ios-run.sh", o, false, env)?
            .status()
            .map_err(|e| e.to_string())?;
        return status.success().then_some(()).ok_or("build failed".into());
    }
    // Keep the old version running until the new one is built.
    crate::build(p, target, o)?;
    if let Some(mut old) = child.take() {
        let _ = old.kill();
        let _ = old.wait();
    }
    *child = Some(crate::spawn(p, target, o, env)?);
    Ok(())
}

/// Modification times of the sources under `root`.
fn snapshot(root: &Path) -> HashMap<PathBuf, SystemTime> {
    fn walk(dir: &Path, out: &mut HashMap<PathBuf, SystemTime>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || matches!(name.as_ref(), "target" | "node_modules") {
                continue;
            }
            match e.file_type() {
                Ok(t) if t.is_dir() => walk(&path, out),
                Ok(_) => {
                    if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                        out.insert(path, m);
                    }
                }
                Err(_) => {}
            }
        }
    }
    let mut out = HashMap::new();
    walk(root, &mut out);
    out
}
