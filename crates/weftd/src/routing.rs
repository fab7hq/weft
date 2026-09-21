//! Where this project's routing policy is kept.
//!
//! The policy itself, and what it means, live in [`weft_core::routing`]
//! (ADR-0007). Finding and reading the file is here.

use std::path::{Path, PathBuf};

pub use weft_core::routing::*;

/// `~/.weft/routing.json`: every project Weft routes, keyed by its path.
///
/// ```json
/// { "/home/me/work/thing": { "eval": "claude-code" } }
/// ```
///
/// One file rather than a tree, because a person edits this by hand and wants
/// to see the whole of it at once. Absent, unreadable or empty means no
/// routing, which is exactly how Weft behaved before this existed.
pub fn file() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    home.join(".weft").join("routing.json")
}

pub fn for_project(root: &Path) -> Routing {
    read(&std::fs::read_to_string(file()).unwrap_or_default(), root)
}
