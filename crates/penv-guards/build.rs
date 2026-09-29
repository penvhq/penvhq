//! Embeds every folder under `guards/` as a built-in guard, so supporting a
//! harness is adding its folder.

use std::fmt::Write;
use std::fs;
use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("guards");
    println!("cargo:rerun-if-changed={}", root.display());
    let out = format!("[{}]", built_in(&root));
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("built_in.rs");
    fs::write(dest, out).unwrap();
}

/// One `BuiltIn` per folder and one entry per file in it, both in name order.
fn built_in(root: &Path) -> String {
    let mut out = String::new();
    for (name, dir) in sorted(root, |p| p.is_dir()) {
        write!(out, "BuiltIn {{ name: {name:?}, files: &[").unwrap();
        for (file, path) in sorted(&dir, |p| p.is_file()) {
            let path = path.display().to_string();
            write!(out, "({file:?}, include_str!({path:?})),").unwrap();
        }
        out.push_str("] },");
    }
    out
}

fn sorted(dir: &Path, keep: fn(&Path) -> bool) -> Vec<(String, std::path::PathBuf)> {
    let mut found: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| keep(path))
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            (!name.starts_with('.')).then_some((name, path))
        })
        .collect();
    found.sort();
    found
}
