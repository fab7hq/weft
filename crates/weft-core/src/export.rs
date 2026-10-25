//! A Seal's record kept in the project's Git, when the person turned it on
//! for the project: a short note on the sealed commit under
//! `refs/notes/ringframe`, and an Agent Trace record (agent-trace.dev) of the
//! lines the work changed under `refs/notes/agent-trace`. Notes never change
//! a commit, and RingFrame's ledger stays ignored by Git.

use serde_json::{Value, json};

/// The notes ref each record is kept under.
pub const NOTE_REF: &str = "ringframe";
pub const TRACE_REF: &str = "agent-trace";

/// What the project records, as its settings say.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Export {
    pub git_note: bool,
    pub agent_trace: bool,
}

/// The commit a Seal sealed, when the work was committed: a receipt's
/// subject is a commit only when nothing was left uncommitted.
pub fn sealed_commit(receipt: &Value) -> Option<&str> {
    let subject = &receipt["subject"];
    (subject["kind"] == "git_commit").then(|| subject["ref"].as_str()).flatten()
}

/// The note: the receipt's Ask titles, verdict, who judged, decision,
/// reason, approver and time, one line each.
pub fn git_note(receipt: &Value, judged_by: Option<&str>) -> String {
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let mut lines = vec![format!("RingFrame Seal {}", text(&receipt["seal_id"]))];
    for ask in receipt["asks"].as_array().into_iter().flatten() {
        lines.push(format!("Ask: {}", text(&ask["title"])));
    }
    let eval = &receipt["eval"];
    if !eval.is_null() {
        let mut verdict = format!("Verdict: {}", text(&eval["verdict"]));
        if let Some(c) = eval["confidence"].as_f64() {
            verdict.push_str(&format!(" {c:.2}"));
        }
        if let Some(by) = judged_by.filter(|b| !b.is_empty()) {
            verdict.push_str(&format!(", judged by {by}"));
        }
        lines.push(verdict);
    }
    lines.push(format!("Decision: {}", text(&receipt["disposition"])));
    if let Some(reason) = receipt["reason"].as_str() {
        lines.push(format!("Reason: {}", reason.replace('_', " ")));
    }
    lines.push(format!("By: {}", text(&receipt["authority"]["id"])));
    lines.push(format!("At: {}", text(&receipt["time"])));
    lines.join("\n") + "\n"
}

/// Each file a `git diff --unified=0` changed, with the line ranges it has
/// now; a file only taken lines from has none and is left out.
pub fn changed_lines(diff: &str) -> Vec<(String, Vec<(u32, u32)>)> {
    let mut files: Vec<(String, Vec<(u32, u32)>)> = Vec::new();
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            files.push((path.to_string(), Vec::new()));
        } else if let Some(hunk) = line.strip_prefix("@@ ") {
            // `@@ -a,b +c,d @@`: d lines from c, one when d is left out.
            let new = hunk.split_whitespace().find_map(|p| p.strip_prefix('+'));
            let (start, count) = match new.map(|n| n.split_once(',').unwrap_or((n, "1"))) {
                Some((s, c)) => (s.parse::<u32>().unwrap_or(0), c.parse::<u32>().unwrap_or(0)),
                None => continue,
            };
            if let (Some((_, ranges)), true) = (files.last_mut(), count > 0) {
                ranges.push((start, start + count - 1));
            }
        }
    }
    files.retain(|(_, ranges)| !ranges.is_empty());
    files
}

/// An Agent Trace record of the work a Seal sealed: each changed file's
/// lines, by an AI contributor, at the sealed commit.
pub fn agent_trace(id: &str, at: &str, commit: &str, files: &[(String, Vec<(u32, u32)>)]) -> Value {
    let files: Vec<Value> = files
        .iter()
        .map(|(path, ranges)| {
            let ranges: Vec<Value> =
                ranges.iter().map(|(s, e)| json!({"start_line": s, "end_line": e})).collect();
            json!({"path": path, "conversations": [{"contributor": {"type": "ai"}, "ranges": ranges}]})
        })
        .collect();
    json!({
        "version": "0.1.0",
        "id": id,
        "timestamp": at,
        "vcs": {"type": "git", "revision": commit},
        "tool": {"name": "weft"},
        "files": files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> Value {
        json!({
            "schema": "ringframe.seal/1", "seal_id": "sel_8k2",
            "asks": [{"ask_id": "ask_1", "title": "health endpoint"}],
            "eval": {"eval_id": "evl_2", "verdict": "aligned", "confidence": 0.92},
            "subject": {"kind": "git_commit", "ref": "4f2a9c1", "sha256": "x"},
            "disposition": "rejected", "reason": "broke_something",
            "authority": {"kind": "human", "id": "local-user", "authority": "interactive"},
            "time": "2026-10-25T11:32:04Z",
        })
    }

    #[test]
    fn the_note_holds_the_receipt_fields_one_line_each() {
        let note = git_note(&receipt(), Some("codex"));
        assert_eq!(
            note,
            "RingFrame Seal sel_8k2\nAsk: health endpoint\nVerdict: aligned 0.92, judged by codex\n\
             Decision: rejected\nReason: broke something\nBy: local-user\nAt: 2026-10-25T11:32:04Z\n"
        );
        let mut bare = receipt();
        bare["eval"] = Value::Null;
        bare.as_object_mut().unwrap().remove("reason");
        let note = git_note(&bare, None);
        assert!(!note.contains("Verdict") && !note.contains("Reason"), "{note}");
    }

    #[test]
    fn only_a_committed_subject_is_a_sealed_commit() {
        assert_eq!(sealed_commit(&receipt()), Some("4f2a9c1"));
        let mut dirty = receipt();
        dirty["subject"] = json!({"kind": "worktree", "ref": "/p"});
        assert_eq!(sealed_commit(&dirty), None);
    }

    #[test]
    fn changed_lines_are_read_from_a_zero_context_diff() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                    @@ -3,0 +4,2 @@\n+x\n+y\n@@ -9 +11 @@\n-a\n+b\n\
                    diff --git a/gone.rs b/gone.rs\n--- a/gone.rs\n+++ b/gone.rs\n@@ -1,2 +0,0 @@\n-p\n-q\n";
        assert_eq!(changed_lines(diff), [("src/a.rs".to_string(), vec![(4, 5), (11, 11)])]);
    }

    #[test]
    fn an_agent_trace_record_names_each_file_its_lines_and_the_commit() {
        let files = [("src/a.rs".to_string(), vec![(4, 5)])];
        let r = agent_trace(
            "3f6c1e2a-9b7d-4c5e-8a10-2d4f6b8e9c01",
            "2026-10-25T11:32:04Z",
            "4f2a9c1",
            &files,
        );
        assert_eq!(r["version"], "0.1.0");
        assert_eq!(r["vcs"], json!({"type": "git", "revision": "4f2a9c1"}));
        let file = &r["files"][0];
        assert_eq!(file["path"], "src/a.rs");
        assert_eq!(file["conversations"][0]["contributor"]["type"], "ai");
        assert_eq!(file["conversations"][0]["ranges"][0], json!({"start_line": 4, "end_line": 5}));
    }
}
