//! A Seal's record written to the project's Git, as its settings ask: notes
//! on the sealed commit, under refs Weft names. A note never changes the
//! commit it is on.

use std::path::Path;

use serde_json::Value;
use weft_core::export::{self, Export};

/// Write what `export` turns on for this receipt; nothing when the work was
/// not committed when it was sealed. `base` is the commit its first Ask was
/// sent on, where the Agent Trace's changed lines start.
pub fn seal(
    root: &Path,
    receipt: &Value,
    on: Export,
    judged_by: Option<&str>,
    base: Option<&str>,
) -> Result<(), String> {
    let Some(commit) = export::sealed_commit(receipt) else { return Ok(()) };
    if on.git_note {
        note(root, export::NOTE_REF, &export::git_note(receipt, judged_by), commit)?;
    }
    if on.agent_trace {
        let diff = match base {
            Some(base) => git(root, &["diff", "--unified=0", &format!("{base}..{commit}")])?,
            None => git(root, &["show", "--format=", "--unified=0", commit])?,
        };
        let at = receipt["time"].as_str().unwrap_or_default();
        let record = export::agent_trace(&uuid()?, at, commit, &export::changed_lines(&diff));
        note(root, export::TRACE_REF, &record.to_string(), commit)?;
    }
    Ok(())
}

/// One note on `commit` under `refs/notes/<reference>`, replacing any.
fn note(root: &Path, reference: &str, text: &str, commit: &str) -> Result<(), String> {
    git(root, &["notes", "--ref", reference, "add", "-f", "-m", text, commit]).map(|_| ())
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
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

/// A random UUID, version 4, as an Agent Trace record's id must be.
fn uuid() -> Result<String, String> {
    use std::io::Read;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| format!("no random id: {e}"))?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
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
        let base = git(&["rev-parse", "HEAD"]);
        std::fs::write(tmp.path().join("a.rs"), "one\ntwo\nthree\n").unwrap();
        git(&["commit", "-q", "-am", "work"]);
        let head = git(&["rev-parse", "HEAD"]);

        let both = Export { git_note: true, agent_trace: true };
        seal(tmp.path(), &receipt(&head), both, Some("codex"), Some(&base)).expect("written");
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
        let trace: Value =
            serde_json::from_str(&git(&["notes", "--ref", "agent-trace", "show", &head])).unwrap();
        assert_eq!(trace["vcs"]["revision"], head.as_str());
        assert_eq!(trace["files"][0]["path"], "a.rs");
        assert_eq!(trace["files"][0]["conversations"][0]["ranges"][0]["start_line"], 2);
        assert_eq!(trace["id"].as_str().map(str::len), Some(36));

        // Off, or with nothing committed to note, nothing is written.
        let (other, git) = repo();
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "base"]);
        let head = git(&["rev-parse", "HEAD"]);
        seal(other.path(), &receipt(&head), Export::default(), None, None).unwrap();
        let mut dirty = receipt(&head);
        dirty["subject"] = json!({"kind": "worktree", "ref": "/p"});
        seal(other.path(), &dirty, both, None, None).unwrap();
        assert_eq!(git(&["notes", "--ref", "ringframe", "list"]), "");
        assert_eq!(git(&["notes", "--ref", "agent-trace", "list"]), "");
    }
}
