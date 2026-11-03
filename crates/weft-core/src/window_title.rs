//! A harness file's `[window_title]`: the item in the harness's own
//! configuration that makes it name its session in its window's title, and
//! how a title names one. A pane whose hooks name no pane (a harness's shared
//! server runs them) is told by its title which session it runs now.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the item goes: a list of strings at `key` (dotted) in `file`, under
/// the harness's configuration home. With no list there, the list is
/// `default` (what the harness shows when it is unset) and the item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowTitle {
    pub file: String,
    pub key: String,
    pub add: String,
    #[serde(default)]
    pub default: Vec<String>,
}

impl WindowTitle {
    pub fn of(v: &Value) -> Option<Self> {
        serde_json::from_value(v.clone()).ok()
    }

    /// The configuration's text with the item in its list, everything else
    /// as it was; none when it is there already, or when the text is not
    /// TOML or holds something other than a list of strings at the key.
    pub fn added(&self, text: &str) -> Option<String> {
        let mut doc: toml_edit::DocumentMut = text.parse().ok()?;
        let (path, last) = match self.key.rsplit_once('.') {
            Some((path, last)) => (path.split('.').collect::<Vec<_>>(), last),
            None => (Vec::new(), self.key.as_str()),
        };
        let mut table = doc.as_table_mut();
        for name in path {
            let entry = table.entry(name).or_insert_with(|| {
                let mut t = toml_edit::Table::new();
                t.set_implicit(true);
                toml_edit::Item::Table(t)
            });
            table = entry.as_table_mut()?;
        }
        match table.get_mut(last) {
            None => {
                let mut list = toml_edit::Array::new();
                for item in self.default.iter().chain([&self.add]) {
                    list.push(item.as_str());
                }
                table.insert(last, toml_edit::value(list));
            }
            Some(item) => {
                let list = item.as_array_mut()?;
                if list.iter().any(|v| v.as_str().is_none()) {
                    return None;
                }
                if list.iter().any(|v| v.as_str() == Some(self.add.as_str())) {
                    return None;
                }
                list.push(self.add.as_str());
            }
        }
        Some(doc.to_string())
    }
}

/// The shortest start of a session id a title may show for it: a harness
/// that shortens a long id still shows this much of it.
pub const SHOWN: usize = 24;

/// Whether a window title names `session`: it holds the id, or the id's
/// first [`SHOWN`] characters or more cut short with `...` or `…`, as a
/// harness shortens a long title item.
pub fn names(title: &str, session: &str) -> bool {
    if !crate::sessions::is_id(session) || session.len() < SHOWN {
        return false;
    }
    if title.contains(session) {
        return true;
    }
    let id_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    ["...", "…"].iter().any(|cut| {
        title.match_indices(cut).any(|(at, _)| {
            let before = &title[..at];
            let start = before.rfind(|c: char| !id_char(c)).map_or(0, |i| i + 1);
            let shown = &before[start..];
            shown.len() >= SHOWN && session.starts_with(shown)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex() -> WindowTitle {
        WindowTitle {
            file: "config.toml".into(),
            key: "tui.terminal_title".into(),
            add: "thread-id".into(),
            default: vec!["activity".into(), "project".into()],
        }
    }

    /// Unset, the list is the harness's own default and the item; a list
    /// the person set keeps their items and their comments.
    #[test]
    fn the_item_is_added_and_nothing_else_changes() {
        let text = "model = \"o\" # mine\n\n[tui]\nscreen = true\n";
        let out = codex().added(text).unwrap();
        assert!(out.starts_with("model = \"o\" # mine\n"), "{out}");
        assert!(
            out.contains("terminal_title = [\"activity\", \"project\", \"thread-id\"]"),
            "{out}"
        );
        let set = "[tui]\n# my title\nterminal_title = [\"project\"]\n";
        let out = codex().added(set).unwrap();
        assert!(out.contains("# my title\nterminal_title = [\"project\", \"thread-id\"]"), "{out}");
        assert_eq!(codex().added(&out), None, "already there");
        let fresh = codex().added("").unwrap();
        assert!(
            fresh.contains("[tui]\nterminal_title = [\"activity\", \"project\", \"thread-id\"]")
        );
    }

    #[test]
    fn a_key_that_holds_something_else_is_left_alone() {
        assert_eq!(codex().added("[tui]\nterminal_title = \"x\"\n"), None);
        assert_eq!(codex().added("[tui]\nterminal_title = [1]\n"), None);
        assert_eq!(codex().added("tui = 3\n"), None);
        assert_eq!(codex().added("not = = toml"), None);
    }

    /// A harness that shortens a long id shows its start and `...`.
    #[test]
    fn a_title_names_a_session_by_its_id_or_a_long_enough_start_of_it() {
        let id = "01a1a1a5-0257-70c0-adc8-937538f1d2c4";
        assert!(names("gl | 01a1a1a5-0257-70c0-adc8-93753...", id));
        assert!(names("⠋ gl | 01a1a1a5-0257-70c0-adc8-93753…", id));
        assert!(names(&format!("gl | {id}"), id));
        assert!(!names("gl | 01a1a1a5-0257-70c0-adc8-93754...", id), "another thread");
        assert!(!names("gl | 01a1a1a5-0257...", id), "too short to tell");
        assert!(!names("gl | 01a1a1a5-0257-70c0-adc8-93753", id), "not cut, not the whole id");
        assert!(!names("gl", id));
        assert!(!names("gl | short...", "short"), "an id too short to tell by");
    }
}
