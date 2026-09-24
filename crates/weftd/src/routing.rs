//! Where Weft's configuration is kept.
//!
//! What the file means lives in [`weft_core::config`]. Finding and reading it
//! is here; this reads and never writes. `weft --serve` writes the starting
//! file before the daemon starts.

use std::path::{Path, PathBuf};

pub use weft_core::config::{Config, STARTING};
pub use weft_core::routing::*;

/// `~/.fab7/weft/config.toml`. Absent, unreadable or empty sets nothing,
/// which is how Weft behaves without one.
pub fn file() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    home.join(".fab7").join("weft").join("config.toml")
}

/// The files `config.toml` replaced. One found beside it is named, not read.
const REPLACED: [&str; 2] = ["routing.json", "eval.json"];

/// The configuration at `path`, with any file it replaced reported.
pub fn read_at(path: &Path) -> Config {
    let mut config = weft_core::config::read(&std::fs::read_to_string(path).unwrap_or_default());
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
        assert_eq!(read_at(&path), Config::default(), "no file sets nothing");
        std::fs::write(dir.join("routing.json"), r#"{"/p": {"eval": "codex"}}"#).unwrap();
        std::fs::write(dir.join("eval.json"), "{}").unwrap();
        std::fs::write(&path, "[routing]\nask = \"codex\"\n").unwrap();
        let config = read_at(&path);
        assert_eq!(config.leftover, ["routing.json", "eval.json"]);
        let routing = config.routing(Path::new("/p"));
        assert_eq!(routing.each(), vec![("ask", "codex")], "routing.json is not read");
        assert_eq!(routing.leftover, ["routing.json", "eval.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
