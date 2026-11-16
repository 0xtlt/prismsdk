//! WASM islands for the web: `[web] islands = ["islands/chart"]` in
//! `Prism.toml`.
//!
//! For each island crate, `prism run/dev/build web`:
//!
//! 1. builds it for `wasm32-unknown-unknown` with a size profile
//!    (`opt-level = "z"`, fat LTO, one codegen unit, `panic = "abort"`; a
//!    faster one for `prism dev` / `run` without `--release`);
//! 2. runs wasm-bindgen as a library (the version linked here, which
//!    `prism-island` pins: nothing to install), or a `wasm-bindgen` CLI of
//!    the island's exact version when they differ;
//! 3. runs `wasm-opt -Oz` when it is on `PATH`;
//! 4. appends the custom element (`<prism-island-chart>`) to the glue,
//!    content-hashes the `.js` and `.wasm`, and writes them plus
//!    `manifest.json` to `target/prism/islands/<profile>/`, which the server
//!    gets as `PRISM_ISLANDS_DIR` and serves under `/_prism/islands/`;
//! 5. prints each island's raw / gzip / brotli size, and checks the budget
//!    (`[web] island_budget_kb`, gzip, default 40): `prism build` fails
//!    above it, the others warn.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Project;

/// The wasm-bindgen this binary links (`wasm-bindgen-cli-support`), and
/// that `prism-island` pins.
pub const WASM_BINDGEN_VERSION: &str = "0.2.130";
const TARGET: &str = "wasm32-unknown-unknown";
/// Default `[web] island_budget_kb`.
pub const DEFAULT_BUDGET_KB: f64 = 40.;

/// One built island.
#[derive(Clone, Debug, PartialEq)]
pub struct Built {
    /// `chart`.
    pub name: String,
    /// `prism-island-chart.<hash>.js`.
    pub module: String,
    /// `prism-island-chart.<hash>.wasm`.
    pub wasm_file: String,
    /// Sizes of the JS (glue + element) and the WASM.
    pub js: Size,
    pub wasm: Size,
    /// Whether `wasm-opt -Oz` ran.
    pub optimized: bool,
}

/// Bytes: raw, gzip (level 9) and brotli (quality 11).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub raw: usize,
    pub gzip: usize,
    pub brotli: usize,
}

impl Size {
    pub fn of(bytes: &[u8]) -> Size {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        let _ = gz.write_all(bytes);
        let gzip = gz.finish().map(|v| v.len()).unwrap_or(0);
        let mut br = Vec::new();
        {
            let mut w = brotli::CompressorWriter::new(&mut br, 4096, 11, 22);
            let _ = w.write_all(bytes);
        }
        Size {
            raw: bytes.len(),
            gzip,
            brotli: br.len(),
        }
    }
}

impl std::ops::Add for Size {
    type Output = Size;
    fn add(self, o: Size) -> Size {
        Size {
            raw: self.raw + o.raw,
            gzip: self.gzip + o.gzip,
            brotli: self.brotli + o.brotli,
        }
    }
}

impl Built {
    /// What a page that shows the island downloads.
    pub fn total(&self) -> Size {
        self.js + self.wasm
    }
}

/// The island's name: its directory's (`islands/chart` → `chart`).
pub fn island_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// The custom element of island `name` (as `prism_core::island::island_tag`).
pub fn island_tag(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("prism-island-{}", name.trim_matches('-'))
}

/// Where the islands of a build go.
pub fn out_dir(p: &Project, release: bool) -> PathBuf {
    p.target_dir
        .join("prism")
        .join("islands")
        .join(if release { "release" } else { "dev" })
}

/// Builds every island of the project. Returns the directory to serve
/// (`PRISM_ISLANDS_DIR`), or `None` without islands. With `strict` (`prism
/// build`), an island above the budget fails the build.
pub fn build(p: &Project, release: bool, strict: bool) -> Result<Option<PathBuf>, String> {
    if p.islands.is_empty() {
        return Ok(None);
    }
    check_target(&p.root)?;
    let out = out_dir(p, release);
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let wasm_opt = which("wasm-opt");
    let mut built = Vec::new();
    for dir in &p.islands {
        built.push(build_one(p, dir, release, wasm_opt.as_deref(), &out)?);
    }
    write_manifest(&out, &built)?;
    println!("{}", report(&built, p.island_budget_kb, wasm_opt.is_some()));
    check_budget(&built, p.island_budget_kb, strict)?;
    Ok(Some(out))
}

/// The table printed after every build.
pub fn report(built: &[Built], budget_kb: f64, wasm_opt: bool) -> String {
    let kb = |b: usize| format!("{:.1} KB", b as f64 / 1024.);
    let mut out = format!("Islands (budget {budget_kb} KB gzip per island):\n");
    for b in built {
        let t = b.total();
        out += &format!(
            "  {:<12} {} raw, {} gzip, {} brotli  (JS {} + WASM {} gzip){}\n",
            b.name,
            kb(t.raw),
            kb(t.gzip),
            kb(t.brotli),
            kb(b.js.gzip),
            kb(b.wasm.gzip),
            if b.optimized { "" } else { "  [no wasm-opt]" },
        );
    }
    if !wasm_opt {
        out += "  wasm-opt is not on PATH: the WASM is not size-optimized \
                (install binaryen for ~10-15 % less).\n";
    }
    out.trim_end().to_string()
}

/// Fails (or warns, when not `strict`) for islands above `budget_kb` gzip.
pub fn check_budget(built: &[Built], budget_kb: f64, strict: bool) -> Result<(), String> {
    let over: Vec<String> = built
        .iter()
        .filter(|b| b.total().gzip as f64 > budget_kb * 1024.)
        .map(|b| format!("{} ({:.1} KB gzip)", b.name, b.total().gzip as f64 / 1024.))
        .collect();
    if over.is_empty() {
        return Ok(());
    }
    let msg = format!(
        "islands over the {budget_kb} KB budget: {}. Drop dependencies (serde, \
         format! of floats, large web-sys features) or raise [web] island_budget_kb",
        over.join(", ")
    );
    if strict {
        Err(msg)
    } else {
        eprintln!("prism: warning: {msg}");
        Ok(())
    }
}

fn write_manifest(out: &Path, built: &[Built]) -> Result<(), String> {
    let mut m = serde_json::Map::new();
    for b in built {
        let t = b.total();
        m.insert(
            b.name.clone(),
            serde_json::json!({
                "module": b.module,
                "files": [b.module, b.wasm_file],
                "size": { "raw": t.raw, "gzip": t.gzip, "brotli": t.brotli },
            }),
        );
    }
    let json = serde_json::to_string_pretty(&serde_json::Value::Object(m)).unwrap_or_default();
    std::fs::write(out.join("manifest.json"), json).map_err(|e| e.to_string())
}

/// The target is installed (a clear message instead of cargo's).
fn check_target(root: &Path) -> Result<(), String> {
    let out = Command::new("rustc")
        .args(["--print", "sysroot"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("rustc: {e}"))?;
    let sysroot = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    if sysroot
        .join("lib/rustlib")
        .join(TARGET)
        .join("lib")
        .is_dir()
    {
        Ok(())
    } else {
        Err(format!(
            "islands are compiled to WebAssembly, and the {TARGET} target is not installed.\n  \
             Install it with: rustup target add {TARGET}"
        ))
    }
}

/// `[package] name` of the crate in `dir`.
fn package_name(dir: &Path) -> Result<String, String> {
    let manifest = dir.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("island {}: {e}", manifest.display()))?;
    let c = crate::config::parse(&text);
    if !c
        .get("lib.crate-type")
        .is_some_and(|t| t.contains("cdylib"))
    {
        return Err(format!(
            "island {}: set `crate-type = [\"cdylib\"]` under [lib]",
            manifest.display()
        ));
    }
    c.get("package.name")
        .cloned()
        .ok_or_else(|| format!("island {}: no [package] name", manifest.display()))
}

fn build_one(
    p: &Project,
    dir: &Path,
    release: bool,
    wasm_opt: Option<&Path>,
    out: &Path,
) -> Result<Built, String> {
    let name = island_name(dir);
    let tag = island_tag(&name);
    let package = package_name(dir)?;
    let profile = if release {
        "prism-island"
    } else {
        "prism-island-dev"
    };
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&p.root)
        .args(["build", "--lib", "--target", TARGET, "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .args(["--profile", profile]);
    for (k, v) in profile_settings(release) {
        cmd.arg("--config")
            .arg(format!("profile.{profile}.{k}={v}"));
    }
    let status = cmd.status().map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err(format!("island {name}: build failed"));
    }
    let input = p
        .target_dir
        .join(TARGET)
        .join(profile)
        .join(format!("{}.wasm", package.replace('-', "_")));
    let used = bindgen_versions(&p.root, &package)?;
    let tmp = out.join(format!(".{name}"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let (glue, mut wasm) = bindgen(&input, &tag, &used, &tmp)?;
    let mut optimized = false;
    if let Some(opt) = wasm_opt {
        match optimize(opt, &wasm, &tmp) {
            Ok(smaller) => {
                wasm = smaller;
                optimized = true;
            }
            Err(e) => eprintln!("prism: wasm-opt failed on island {name} ({e}); kept as is"),
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    let wasm_file = format!("{tag}.{}.wasm", hash(&wasm));
    let js = element_module(&glue, &tag, &wasm_file)?;
    let module = format!("{tag}.{}.js", hash(js.as_bytes()));
    std::fs::write(out.join(&wasm_file), &wasm).map_err(|e| e.to_string())?;
    std::fs::write(out.join(&module), &js).map_err(|e| e.to_string())?;
    Ok(Built {
        name,
        module,
        wasm_file,
        js: Size::of(js.as_bytes()),
        wasm: Size::of(&wasm),
        optimized,
    })
}

/// The size profile (`--config profile.<name>.<key>=<value>`). Not
/// `strip = true`: wasm-bindgen needs its custom sections; it drops the
/// names and producers itself.
pub fn profile_settings(release: bool) -> Vec<(&'static str, &'static str)> {
    if release {
        vec![
            ("inherits", "\"release\""),
            ("opt-level", "\"z\""),
            ("lto", "true"),
            ("codegen-units", "1"),
            ("panic", "\"abort\""),
            ("debug", "false"),
            ("strip", "false"),
            ("incremental", "false"),
        ]
    } else {
        // Quick rebuilds on save; still small enough to load fast.
        vec![
            ("inherits", "\"dev\""),
            ("opt-level", "\"s\""),
            ("debug", "false"),
            ("debug-assertions", "false"),
            ("overflow-checks", "false"),
            ("panic", "\"abort\""),
        ]
    }
}

/// The wasm-bindgen versions the island links (`cargo tree`, no build).
fn bindgen_versions(root: &Path, package: &str) -> Result<Vec<String>, String> {
    let out = Command::new("cargo")
        .current_dir(root)
        .args([
            "tree", "-q", "-e", "normal", "--prefix", "none", "--target", TARGET, "-p",
        ])
        .arg(package)
        .output()
        .map_err(|e| format!("cargo tree: {e}"))?;
    Ok(tree_versions(
        &String::from_utf8_lossy(&out.stdout),
        "wasm-bindgen",
    ))
}

/// `name vX.Y.Z` lines of a `cargo tree --prefix none` output.
pub fn tree_versions(tree: &str, name: &str) -> Vec<String> {
    let mut v: Vec<String> = tree
        .lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            if parts.next()? != name {
                return None;
            }
            Some(parts.next()?.trim_start_matches('v').to_string())
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The JS glue and the WASM, from wasm-bindgen of the island's version.
fn bindgen(
    input: &Path,
    tag: &str,
    used: &[String],
    tmp: &Path,
) -> Result<(String, Vec<u8>), String> {
    let version = match used {
        [v] => v.clone(),
        [] => {
            return Err(format!(
                "{}: the island does not use wasm-bindgen (depend on prism-island)",
                input.display()
            ));
        }
        many => {
            return Err(format!(
                "the island links several wasm-bindgen versions ({}): keep one ({WASM_BINDGEN_VERSION})",
                many.join(", ")
            ));
        }
    };
    if version == WASM_BINDGEN_VERSION {
        let mut out = wasm_bindgen_cli_support::Bindgen::new()
            .input_path(input)
            .out_name(tag)
            .web(true)
            .map_err(|e| e.to_string())?
            .typescript(false)
            .remove_name_section(true)
            .remove_producers_section(true)
            .omit_default_module_path(true)
            .generate_output()
            .map_err(|e| format!("wasm-bindgen: {e:#}"))?;
        if !out.snippets().values().all(Vec::is_empty) || !out.local_modules().is_empty() {
            return Err("islands cannot use `inline_js` or `module = …` JS snippets yet".into());
        }
        let js = out.js().to_string();
        return Ok((js, out.wasm_mut().emit_wasm()));
    }
    // Another version: its own CLI, if installed.
    let cli = which("wasm-bindgen").filter(|cli| {
        Command::new(cli).arg("--version").output().is_ok_and(|o| {
            String::from_utf8_lossy(&o.stdout).trim() == format!("wasm-bindgen {version}")
        })
    });
    let Some(cli) = cli else {
        return Err(format!(
            "the island uses wasm-bindgen {version}; this prism runs {WASM_BINDGEN_VERSION} \
             (the version prism-island pins).\n  Use it: cargo update -p wasm-bindgen --precise {WASM_BINDGEN_VERSION}\n  \
             or install a matching CLI: cargo install wasm-bindgen-cli --version {version}"
        ));
    };
    let status = Command::new(cli)
        .args([
            "--target",
            "web",
            "--no-typescript",
            "--remove-name-section",
            "--remove-producers-section",
            "--omit-default-module-path",
            "--out-name",
            tag,
            "--out-dir",
        ])
        .arg(tmp)
        .arg(input)
        .status()
        .map_err(|e| format!("wasm-bindgen: {e}"))?;
    if !status.success() {
        return Err("wasm-bindgen failed".into());
    }
    let js = std::fs::read_to_string(tmp.join(format!("{tag}.js"))).map_err(|e| e.to_string())?;
    let wasm = std::fs::read(tmp.join(format!("{tag}_bg.wasm"))).map_err(|e| e.to_string())?;
    Ok((js, wasm))
}

/// `wasm-opt -Oz`, with the features rustc's wasm32 target emits.
fn optimize(wasm_opt: &Path, wasm: &[u8], tmp: &Path) -> Result<Vec<u8>, String> {
    let (input, output) = (tmp.join("in.wasm"), tmp.join("out.wasm"));
    std::fs::write(&input, wasm).map_err(|e| e.to_string())?;
    let out = Command::new(wasm_opt)
        .arg("-Oz")
        .args([
            "--enable-bulk-memory",
            "--enable-mutable-globals",
            "--enable-nontrapping-float-to-int",
            "--enable-sign-ext",
            "--enable-reference-types",
            "--enable-multivalue",
        ])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    std::fs::read(&output).map_err(|e| e.to_string())
}

/// The glue plus the custom element: mounts the island when the element
/// is connected (once the WASM is loaded), updates it, unmounts it when
/// removed (not when moved).
pub fn element_module(glue: &str, tag: &str, wasm_file: &str) -> Result<String, String> {
    if !glue.contains("__wbg_init as default") && !glue.contains("export default __wbg_init") {
        return Err("unexpected wasm-bindgen output (no __wbg_init)".into());
    }
    if !glue.contains("function __prism_mount(") {
        return Err(format!(
            "<{tag}>: the island crate does not export an island: add `prism_island::register!(YourIsland);`"
        ));
    }
    let element = ELEMENT_JS
        .replace("__TAG__", tag)
        .replace("__WASM__", wasm_file);
    Ok(format!("{glue}\n{element}"))
}

const ELEMENT_JS: &str = r#"// --- <__TAG__>: the Prism island around the exports above -----------------
// Props: `this.prismProps` (all) and `prismUpdate(changed)`; renders into
// the shadow root, so the light DOM (the server's fallback) shows until then.
const prismWasm = new URL("./__WASM__", import.meta.url);
let prismReady;
class PrismIsland extends HTMLElement {
  connectedCallback() {
    if (this.prismHandle !== undefined || this.prismMounting) return;
    this.prismMounting = true;
    (prismReady ||= __wbg_init({ module_or_path: prismWasm })).then(() => {
      this.prismMounting = false;
      if (!this.isConnected || this.prismHandle !== undefined) return;
      const root = this.shadowRoot || this.attachShadow({ mode: "open" });
      this.prismHandle = __prism_mount(this, root, this.prismProps || {});
    }, (e) => { this.prismMounting = false; console.error("<__TAG__>: cannot load its WASM", e); });
  }
  disconnectedCallback() {
    // Moved (removed and inserted again at once) is not removed.
    queueMicrotask(() => {
      if (this.isConnected || this.prismHandle === undefined) return;
      const handle = this.prismHandle;
      this.prismHandle = undefined;
      __prism_unmount(handle);
    });
  }
  prismUpdate(changed) {
    if (this.prismHandle !== undefined) __prism_update(this.prismHandle, changed);
  }
}
if (!customElements.get("__TAG__")) customElements.define("__TAG__", PrismIsland);
"#;

/// The first 12 hex digits of the SHA-256 (like prism-web's asset names).
fn hash(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    digest.as_ref()[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(name: &str, gzip: usize) -> Built {
        Built {
            name: name.into(),
            module: format!("{}.abc.js", island_tag(name)),
            wasm_file: format!("{}.def.wasm", island_tag(name)),
            js: Size {
                raw: 20_000,
                gzip: 5_000,
                brotli: 4_000,
            },
            wasm: Size {
                raw: 40_000,
                gzip,
                brotli: gzip * 9 / 10,
            },
            optimized: true,
        }
    }

    #[test]
    fn names_tags_and_profiles() {
        assert_eq!(island_name(Path::new("islands/chart")), "chart");
        assert_eq!(island_tag("chart"), "prism-island-chart");
        assert_eq!(island_tag("Big_Chart"), "prism-island-big-chart");
        let release = profile_settings(true);
        assert!(release.contains(&("opt-level", "\"z\"")));
        assert!(release.contains(&("lto", "true")));
        assert!(release.contains(&("codegen-units", "1")));
        assert!(release.contains(&("panic", "\"abort\"")));
        // wasm-bindgen warns about stripped modules.
        assert!(release.contains(&("strip", "false")));
    }

    #[test]
    fn the_budget_fails_prism_build_only() {
        let ok = [built("chart", 15_000)];
        assert!(check_budget(&ok, 40., true).is_ok());
        let big = [built("chart", 15_000), built("editor", 50_000)];
        let err = check_budget(&big, 40., true).unwrap_err();
        assert!(
            err.contains("editor (53.7 KB gzip)") && !err.contains("chart ("),
            "{err}"
        );
        assert!(check_budget(&big, 40., false).is_ok());
        assert!(check_budget(&big, 60., true).is_ok());
    }

    #[test]
    fn the_report_shows_every_size() {
        let r = report(&[built("chart", 15_000)], 40., false);
        assert!(r.contains("budget 40 KB gzip"), "{r}");
        assert!(r.contains("chart") && r.contains("58.6 KB raw"), "{r}");
        for part in ["19.5 KB gzip", "brotli", "wasm-opt is not on PATH"] {
            assert!(r.contains(part), "{part}: {r}");
        }
    }

    #[test]
    fn sizes_compress() {
        let s = Size::of(&[b'a'; 10_000]);
        assert_eq!(s.raw, 10_000);
        assert!(s.gzip < 200 && s.brotli < 200, "{s:?}");
    }

    #[test]
    fn versions_come_from_cargo_tree() {
        let tree = "counter-chart-island v0.1.0 (/x)\nprism-island v0.1.0 (/y)\njs-sys v0.3.107\n\
                    wasm-bindgen v0.2.130\nwasm-bindgen v0.2.130 (*)\nwasm-bindgen-macro v0.2.130\n";
        assert_eq!(tree_versions(tree, "wasm-bindgen"), ["0.2.130"]);
        assert!(tree_versions(tree, "serde").is_empty());
    }

    #[test]
    fn the_element_wraps_the_glue() {
        let glue = "export function __prism_mount(element, root, props) {}\nexport { initSync, __wbg_init as default };";
        let js = element_module(glue, "prism-island-chart", "prism-island-chart.abc.wasm").unwrap();
        assert!(js.starts_with(glue));
        assert!(js.contains(r#"customElements.define("prism-island-chart", PrismIsland)"#));
        assert!(js.contains(r#"new URL("./prism-island-chart.abc.wasm", import.meta.url)"#));
        assert!(!js.contains("__TAG__") && !js.contains("__WASM__"));
        let err = element_module(
            "export { initSync, __wbg_init as default };",
            "prism-island-x",
            "x.wasm",
        )
        .unwrap_err();
        assert!(err.contains("register!"), "{err}");
    }

    #[test]
    fn the_pinned_version_is_the_linked_one() {
        // prism-island pins the same wasm-bindgen the CLI links.
        let island = include_str!("../../prism-island/Cargo.toml");
        assert!(island.contains(&format!("version = \"={WASM_BINDGEN_VERSION}\"")));
        let cli = include_str!("../Cargo.toml");
        assert!(cli.contains(&format!(
            "wasm-bindgen-cli-support = \"={WASM_BINDGEN_VERSION}\""
        )));
    }
}
