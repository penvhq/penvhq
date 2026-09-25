//! Declares every file in `src/credential/` but `mod.rs` as a credential kind,
//! so adding a kind is adding its file.

use std::fmt::Write;
use std::fs;
use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/credential");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut kinds: Vec<(String, String)> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|e| e == "rs"))
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?.to_string();
            (name != "mod").then(|| (name, path.display().to_string()))
        })
        .collect();
    kinds.sort();

    let mut out = String::new();
    for (name, path) in &kinds {
        writeln!(out, "#[path = {path:?}]\nmod {name};").unwrap();
    }
    out.push_str("/// Every kind's places, file by file.\nconst KINDS: &[&[Place]] = &[");
    for (name, _) in &kinds {
        write!(out, "{name}::PLACES,").unwrap();
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("kinds.rs");
    fs::write(dest, out).unwrap();
}
