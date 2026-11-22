//! The viewer's page, built into the binary: every file under `viewer/`,
//! listed as `(path, bytes)` for `src/viewer.rs`.

use std::path::{Path, PathBuf};

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("viewer");
    println!("cargo:rerun-if-changed=viewer");
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    let mut list = String::from("&[\n");
    for file in files {
        let rel = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        list.push_str(&format!("    ({rel:?}, include_bytes!({:?})),\n", file.to_string_lossy()));
    }
    list.push(']');
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("viewer.rs");
    std::fs::write(out, list).unwrap();
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk(&path, files);
        } else {
            files.push(path);
        }
    }
}
