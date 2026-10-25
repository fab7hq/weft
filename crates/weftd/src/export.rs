//! A Seal's record written to the project's Git, as its settings ask: a note
//! on the sealed commit, under `refs/notes/ringframe`. A note never changes
//! the commit it is on.

use std::path::Path;

use serde_json::Value;
use weft_core::export::{self, Export};

/// Write the note `on` asks for; nothing when the work was not committed
/// when it was sealed.
pub fn seal(
    root: &Path,
    receipt: &Value,
    on: Export,
    judged_by: Option<&str>,
) -> Result<(), String> {
    let Some(commit) = export::sealed_commit(receipt) else { return Ok(()) };
    if on.git_note {
        note(root, export::NOTE_REF, &export::git_note(receipt, judged_by), commit)?;
    }
    Ok(())
}

/// One note on `commit` under `refs/notes/<reference>`, replacing any.
fn note(root: &Path, reference: &str, text: &str, commit: &str) -> Result<(), String> {
    git(root, &["notes", "--ref", reference, "add", "-f", "-m", text, commit]).map(|_| ())
}

pub(crate) fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("git did not run: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {}: {}", args[0], String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repo() -> (tempfile::TempDir, impl Fn(&[&str]) -> String) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let run = move |args: &[&str]| -> String {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        (tmp, run)
    }

    fn receipt(commit: &str) -> Value {
        json!({
            "seal_id": "sel_1", "asks": [{"ask_id": "ask_1", "title": "health endpoint"}],
            "eval": {"eval_id": "evl_1", "verdict": "aligned", "confidence": 1.0},
            "subject": {"kind": "git_commit", "ref": commit},
            "disposition": "accepted",
            "authority": {"kind": "human", "id": "local-user"},
            "time": "2026-10-25T11:32:04Z",
        })
    }

    #[test]
    fn a_seals_note_holds_the_receipt_and_leaves_the_commit_as_it_was() {
        let (tmp, git) = repo();
        git(&["init", "-q"]);
        std::fs::write(tmp.path().join("a.rs"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "base"]);
        std::fs::write(tmp.path().join("a.rs"), "one\ntwo\nthree\n").unwrap();
        git(&["commit", "-q", "-am", "work"]);
        let head = git(&["rev-parse", "HEAD"]);

        let on = Export { git_note: true };
        seal(tmp.path(), &receipt(&head), on, Some("codex")).expect("written");
        assert_eq!(git(&["rev-parse", "HEAD"]), head, "the commit is unchanged");
        assert_eq!(git(&["status", "--porcelain"]), "", "nothing in the work tree");
        let note = git(&["notes", "--ref", "ringframe", "show", &head]);
        for said in [
            "RingFrame Seal sel_1",
            "Ask: health endpoint",
            "Verdict: aligned 1.00, judged by codex",
            "Decision: accepted",
            "By: local-user",
            "At: 2026-10-25T11:32:04Z",
        ] {
            assert!(note.contains(said), "{said} in {note}");
        }
        // Off, or with nothing committed to note, nothing is written.
        let (other, git) = repo();
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "base"]);
        let head = git(&["rev-parse", "HEAD"]);
        seal(other.path(), &receipt(&head), Export::default(), None).unwrap();
        let mut dirty = receipt(&head);
        dirty["subject"] = json!({"kind": "worktree", "ref": "/p"});
        seal(other.path(), &dirty, on, None).unwrap();
        assert_eq!(git(&["notes", "--ref", "ringframe", "list"]), "");
    }
}
