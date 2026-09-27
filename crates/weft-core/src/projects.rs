//! The projects open in the daemon, as `~/.fab7/weft/projects.json` lists
//! them: Weft's own machine data, beside its `config.toml`. The daemon writes
//! it whenever a project opens; the first screen's picker reads it, before
//! any daemon is asked for anything.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opened {
    pub path: PathBuf,
    /// When the daemon opened it, in milliseconds since the epoch.
    pub opened: i64,
}

/// The file's text, as a whole list.
pub fn write(projects: &[Opened]) -> String {
    let mut text = serde_json::to_string_pretty(&serde_json::json!({ "projects": projects }))
        .unwrap_or_default();
    text.push('\n');
    text
}

/// What the file lists, read tolerantly: an entry that is not one is skipped,
/// and a file that is not JSON lists nothing.
pub fn read(text: &str) -> Vec<Opened> {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    v["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| serde_json::from_value(p.clone()).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_reads_back_and_a_broken_entry_is_skipped() {
        let one = Opened { path: "/work/one".into(), opened: 5 };
        assert_eq!(read(&write(std::slice::from_ref(&one))), std::slice::from_ref(&one));
        let text = r#"{"projects": [{"path": "/work/one", "opened": 5}, {"path": 3}, "x"]}"#;
        assert_eq!(read(text), [one]);
        assert!(read("{torn").is_empty());
    }
}
