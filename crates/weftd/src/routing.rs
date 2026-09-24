//! Where this project's routing policy is kept.
//!
//! The policy itself, and what it means, live in [`weft_core::routing`].
//! Finding and reading the file is here.

use std::path::{Path, PathBuf};

pub use weft_core::routing::*;

/// `~/.fab7/weft/routing.json`: every project Weft routes, keyed by its path.
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
    home.join(".fab7").join("weft").join("routing.json")
}

pub fn for_project(root: &Path) -> Routing {
    read(&std::fs::read_to_string(file()).unwrap_or_default(), root)
}

/// `~/.fab7/weft/eval.json`: the model and effort per Eval role, per harness.
pub fn eval_file() -> PathBuf {
    file().with_file_name("eval.json")
}

/// The tiers, after writing the shipped ones if there is no file yet. The file
/// is the person's from then on: Weft never writes it again.
pub fn tiers_at(path: &Path) -> weft_core::eval_stages::Tiers {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, weft_core::eval_stages::DEFAULTS);
    }
    weft_core::eval_stages::tiers(&std::fs::read_to_string(path).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_tiers_are_written_once_and_then_left_alone() {
        let dir = std::env::temp_dir().join(format!("weft-eval-json-{}", std::process::id()));
        let path = dir.join("weft").join("eval.json");
        let first = tiers_at(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), weft_core::eval_stages::DEFAULTS);
        assert!(first.unknown.is_empty());
        std::fs::write(&path, r#"{"codex": {"drift": {"effort": "max"}}}"#).unwrap();
        let mine = tiers_at(&path);
        assert_eq!(mine.by_harness["codex"]["drift"]["effort"], "max");
        assert!(
            mine.by_harness.get("claude-code").is_none(),
            "the person's file, not the defaults"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
