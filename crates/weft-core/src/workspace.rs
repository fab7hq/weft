//! Whether RingFrame is set up in a project, and what it still needs if not:
//! the checklist the tree shows in place of the work.
//!
//! Weft runs `ringframe init` when a project opens and again while it is not
//! set up. What RingFrame answers is all Weft knows: whether it ran, and the
//! code it refused with. Weft never looks at Git itself, so a line RingFrame
//! did not answer is unknown, never guessed.

use serde::{Deserialize, Serialize};

/// What a project still needs before RingFrame can be set up in it. A line
/// that is `None` is one nothing could answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Needs {
    /// `ringframe` runs on this machine.
    pub ringframe: bool,
    /// The project is a Git repository of its own.
    pub git: Option<bool>,
    /// It has a first commit.
    pub commit: Option<bool>,
}

/// One line of the checklist: its mark, its words, and how to make it hold
/// while it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    pub mark: &'static str,
    pub key: &'static str,
    pub how: Option<&'static str>,
}

impl Needs {
    /// What `ringframe init` refusing says: `None` when no `ringframe` ran,
    /// else the code RingFrame refused with.
    pub fn refused(code: Option<&str>) -> Self {
        let (git, commit) = match code {
            None => return Needs { ringframe: false, git: None, commit: None },
            // A folder inside another repository is not one of its own, and
            // `git init` is its fix too.
            Some("workspace.no_git" | "workspace.not_repo_root") => (Some(false), Some(false)),
            Some("workspace.no_commit") => (Some(true), Some(false)),
            Some(_) => (None, None),
        };
        Needs { ringframe: true, git, commit }
    }

    /// The checklist, top to bottom.
    pub fn lines(&self) -> [Line; 3] {
        let line = |held: Option<bool>, key, how| Line {
            mark: match held {
                Some(true) => "✓",
                Some(false) => "✗",
                None => "?",
            },
            key,
            how: (held == Some(false)).then_some(how),
        };
        [
            line(Some(self.ringframe), "tree.has_ringframe", "tree.ringframe_how"),
            line(self.git, "tree.has_git", "tree.git_how"),
            line(self.commit, "tree.has_commit", "tree.commit_how"),
        ]
    }
}

impl Line {
    pub fn say(&self) -> &'static str {
        crate::words::word(self.key)
    }

    pub fn how(&self) -> Option<&'static str> {
        self.how.map(crate::words::word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(needs: Needs) -> Vec<(&'static str, &'static str, Option<&'static str>)> {
        needs.lines().iter().map(|l| (l.mark, l.say(), l.how())).collect()
    }

    #[test]
    fn no_git_is_two_lines_to_fix_each_with_how() {
        assert_eq!(
            said(Needs::refused(Some("workspace.no_git"))),
            [
                ("✓", "ringframe is installed", None),
                ("✗", "a Git repository", Some("git init")),
                ("✗", "a first commit", Some("commit once")),
            ]
        );
        assert_eq!(
            Needs::refused(Some("workspace.not_repo_root")),
            Needs::refused(Some("workspace.no_git"))
        );
    }

    #[test]
    fn a_repository_with_no_commit_needs_only_the_commit() {
        assert_eq!(
            said(Needs::refused(Some("workspace.no_commit"))),
            [
                ("✓", "ringframe is installed", None),
                ("✓", "a Git repository", None),
                ("✗", "a first commit", Some("commit once")),
            ]
        );
    }

    #[test]
    fn with_no_ringframe_nothing_could_say_what_git_has() {
        assert_eq!(
            said(Needs::refused(None)),
            [
                ("✗", "ringframe is installed", Some("run Weft's installer again")),
                ("?", "a Git repository", None),
                ("?", "a first commit", None),
            ]
        );
        let other = Needs::refused(Some("workspace.io"));
        assert_eq!((other.ringframe, other.git, other.commit), (true, None, None));
    }

    #[test]
    fn the_wire_names_each_line() {
        assert_eq!(
            serde_json::to_value(Needs::refused(Some("workspace.no_commit"))).unwrap(),
            serde_json::json!({"ringframe": true, "git": true, "commit": false})
        );
    }
}
