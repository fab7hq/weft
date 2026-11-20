//! Files an Eval should skip: what RingFrame's read check found that git
//! could be told to leave out, with the one line that does it, and what an
//! opened Eval counted and did not read.

use serde::{Deserialize, Serialize};

/// A line for `.gitignore` or `.gitattributes`, at the project's root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fix {
    /// `.gitignore` or `.gitattributes`.
    pub file: String,
    /// The line, without its newline.
    pub line: String,
}

/// Something the readers would read that git could be told to skip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// `untracked_bulk`, `data_bulk`, `committed_media`, `committed_build`, `large_text` or
    /// `sub_repo`.
    pub kind: String,
    /// The folder (with a trailing `/`) or file, from the project's root.
    pub path: String,
    /// How many files it holds; for a nested repository, how many have
    /// uncommitted changes.
    pub files: usize,
    /// The text the readers would read of it, at least.
    #[serde(default)]
    pub bytes: u64,
    /// For a nested repository, its commits since the work began.
    #[serde(default)]
    pub commits: usize,
    /// Its files by extension, the most first: `[["json", 13708]]`.
    #[serde(default)]
    pub kinds_of_files: Vec<(String, usize)>,
    /// None when no line keeps it out without hiding something a person
    /// wrote, or when nothing written would help (a nested repository).
    #[serde(default)]
    pub fix: Option<Fix>,
    /// Its line was written while the dialog it is shown in was open: it is
    /// no longer read.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub added: bool,
}

impl Finding {
    /// Shown in the project's settings: a nested repository is said only
    /// in an Eval whose work it holds changes for.
    pub fn in_settings(&self) -> bool {
        self.kind != "sub_repo"
    }
}

/// Files an Eval counted and did not read, of one kind, in one folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotRead {
    /// `generated`, `vendored`, `binary` or `lock`.
    pub kind: String,
    /// The deepest folder that holds them all, with a trailing `/`; the file
    /// itself when it is one; empty for several at the top.
    pub folder: String,
    pub files: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finding_reads_as_the_daemon_writes_it() {
        let wire = serde_json::json!({
            "kind": "data_bulk", "path": "research/evidence/", "files": 13708, "bytes": 52000000,
            "commits": 0, "kinds_of_files": [["json", 13708]],
            "fix": {"file": ".gitattributes", "line": "research/evidence/** linguist-generated=true"},
        });
        let f: Finding = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(f.kinds_of_files, [("json".to_string(), 13708)]);
        assert!(!f.added && f.in_settings());
        assert_eq!(serde_json::to_value(&f).unwrap(), wire, "added is said only when true");
        let sub = Finding { kind: "sub_repo".into(), fix: None, ..f };
        assert!(!sub.in_settings());
    }
}
