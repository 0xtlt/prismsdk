//! Embeds every file of `platform/` (bundling scripts, the Android Java host)
//! so `prism` can write them out in any project. The list is generated, so a
//! new file only has to be added to the directory.

use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let hidden = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("platform");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, &mut files);
    let mut code = String::from("&[\n");
    for path in files {
        let rel = path.strip_prefix(&root).unwrap().to_str().unwrap();
        let rel = rel.replace('\\', "/");
        code += &format!(
            "    ({rel:?}, include_str!({:?})),\n",
            path.to_str().unwrap()
        );
    }
    code += "]\n";
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(out.join("embedded.rs"), code).unwrap();
}
