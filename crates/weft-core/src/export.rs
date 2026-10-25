//! A Seal's record kept in the project's Git, when the person turned it on
//! for the project: a short note on the sealed commit under
//! `refs/notes/ringframe`. A note never changes a commit, and RingFrame's
//! ledger stays ignored by Git.

use serde_json::Value;

/// The notes ref a Seal's note is kept under.
pub const NOTE_REF: &str = "ringframe";

/// What the project records, as its settings say.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Export {
    pub git_note: bool,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
