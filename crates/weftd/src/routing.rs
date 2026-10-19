//! Where Weft's configuration is kept.
//!
//! What the file means, and how a change is made to its text, live in
//! [`weft_core::config`]. Finding, reading and writing the file is here.
//! `weft --serve` writes the starting file before the daemon starts.

use std::path::{Path, PathBuf};

pub use weft_core::config::{Config, STARTING};
pub use weft_core::routing::*;

/// `~/.fab7/weft/config.toml`. Absent, unreadable or empty sets nothing,
/// which is how Weft behaves without one.
pub fn file() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    home.join(".fab7").join("weft").join("config.toml")
}

/// `config.toml` changed by `change`, which gets its text (empty when there is
/// none) and returns the new text; written whole or not at all.
pub fn change(
    path: &Path,
    change: impl FnOnce(&str) -> Result<String, String>,
) -> Result<(), String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let out = change(&text)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("config.toml was not written: {e}"))?;
    }
    let part = path.with_extension("toml.part");
    std::fs::write(&part, out)
        .and_then(|()| std::fs::rename(&part, path))
        .map_err(|e| format!("config.toml was not written: {e}"))
}

/// The files `config.toml` replaced. One found beside it is named, not read.
const REPLACED: [&str; 2] = ["routing.json", "eval.json"];

/// The configuration at `path`, with any file it replaced reported. A harness
/// name is known when a profile defines it.
pub fn read_at(path: &Path, known: &weft_core::harness::Harnesses) -> Config {
    let mut config =
        weft_core::config::read(&std::fs::read_to_string(path).unwrap_or_default(), known);
    config.leftover = REPLACED
        .iter()
        .filter(|f| path.with_file_name(f).exists())
        .map(|f| f.to_string())
        .collect();
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leftover_json_file_is_named_and_not_read() {
        let dir = std::env::temp_dir().join(format!("weft-config-toml-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        assert_eq!(
            read_at(&path, &weft_core::harness::fixture::harnesses()),
            Config::default(),
            "no file sets nothing"
        );
        std::fs::write(dir.join("routing.json"), r#"{"/p": {"eval": "codex"}}"#).unwrap();
        std::fs::write(dir.join("eval.json"), "{}").unwrap();
        std::fs::write(&path, "[routing]\nask = \"codex\"\n").unwrap();
        let config = read_at(&path, &weft_core::harness::fixture::harnesses());
        assert_eq!(config.leftover, ["routing.json", "eval.json"]);
        let routing = config.routing(Path::new("/p"));
        assert_eq!(routing.each(), vec![("ask", "codex")], "routing.json is not read");
        assert_eq!(routing.leftover, ["routing.json", "eval.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_is_written_whole_or_not_at_all() {
        let dir = std::env::temp_dir().join(format!("weft-config-change-{}", std::process::id()));
        let path = dir.join("weft").join("config.toml");
        change(&path, |t| weft_core::config::edit(t, &["turbo"], Some(&serde_json::json!(true))))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "turbo = true\n",
            "its folder made, the file written"
        );
        let refused = change(&path, |_| Err("no".into()));
        assert_eq!(refused, Err("no".into()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "turbo = true\n", "untouched");
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["config.toml"], "nothing left beside it");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
