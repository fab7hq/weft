//! Reading an Eval record off disk.
//!
//! What one means lives in [`weft_core::record`]; this opens the file and
//! hands the bytes over.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use weft_core::board::{Notice, notice};

pub use weft_core::record::*;

pub fn read(project_root: &Path, eval_id: &str) -> Option<Record> {
    let path = project_root.join(".fab7/rf/evals").join(eval_id).join("record.json");
    let bytes = std::fs::read(path).ok()?;
    Record::parse(&serde_json::from_slice::<Value>(&bytes).ok()?)
}

/// An id RingFrame made: letters, digits and `_`. Anything else is not
/// looked up, so a read cannot walk out of the Eval's folder.
fn plain(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn json_at(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// The Eval view of one Eval, from RingFrame's files as they are now: its
/// record once it has closed, else what its readers have handed in so far.
pub fn eval_view(project_root: &Path, eval_id: &str) -> Option<weft_core::eval_view::EvalView> {
    if !plain(eval_id) {
        return None;
    }
    let rf = project_root.join(".fab7/rf");
    let dir = rf.join("evals").join(eval_id);
    let record = json_at(&dir.join("record.json"));
    let changes = match json_at(&dir.join("changes.json")) {
        Some(c) => c,
        None if record.is_some() => Value::Null,
        None => return None,
    };
    let evidence = json_at(&dir.join("evidence.json")).unwrap_or(Value::Null);
    let brief = json_at(&dir.join("brief.json")).unwrap_or(Value::Null);
    // What is in, from the ledger: each accepted output, and every task that
    // is done with (accepted or failed).
    let mut outputs = Vec::new();
    let mut done = Vec::new();
    for line in std::fs::read_to_string(rf.join("ledger.jsonl")).unwrap_or_default().lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        let d = &e["data"];
        if e["type"] != "eval.task" || d["eval_id"] != eval_id {
            continue;
        }
        let task = d["task_id"].as_str().unwrap_or_default().to_string();
        match d["outcome"].as_str() {
            Some("accepted") => {
                if let Some(out) = d["artifact"]["path"].as_str().and_then(|p| json_at(&rf.join(p)))
                {
                    outputs.push(out);
                }
                done.push(task);
            }
            Some("failed") => done.push(task),
            _ => {}
        }
    }
    // What has been handed out: a task's file is written when it goes out.
    let planned: Vec<String> = std::fs::read_dir(rf.join(format!("tmp/eval-{eval_id}/tasks")))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|f| f.file_name().to_str().map(str::to_string))
        .filter_map(|n| n.strip_suffix(".json").map(str::to_string))
        .filter(|n| n.contains('~'))
        .collect();
    Some(weft_core::eval_view::EvalView::build(weft_core::eval_view::Sources {
        eval_id,
        changes: &changes,
        evidence: &evidence,
        brief: &brief,
        record: record.as_ref(),
        outputs: &outputs,
        planned: &planned,
        done: &done,
        checks_running: &crate::ringframe::checks_running(project_root, eval_id),
    }))
}

/// What each open Eval asks of the person, by its id: ready to seal or
/// needs you, from the Eval and its checks alone.
pub fn notices(project_root: &Path, units: &[weft_core::ledger::Unit]) -> BTreeMap<String, Notice> {
    let open = units.iter().filter(|u| u.sealed.is_none()).filter_map(|u| u.check.as_ref());
    open.filter_map(|c| Some((c.eval_id.clone(), notice(&eval_view(project_root, &c.eval_id)?)?)))
        .collect()
}

/// One change's hunk, as RingFrame cut it.
pub fn window_text(project_root: &Path, eval_id: &str, window: &str) -> Option<String> {
    if !plain(eval_id) || !plain(window) {
        return None;
    }
    let doc = json_at(&project_root.join(".fab7/rf/evals").join(eval_id).join("windows.json"))?;
    doc["windows"][window].as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_ringframe_still_runs_shows_until_its_results_are_in() {
        let dir = tempfile::tempdir().unwrap();
        let rf = dir.path().join(".fab7/rf");
        let eval = rf.join("evals/evl_1");
        let tmp = rf.join("tmp/eval-evl_1");
        std::fs::create_dir_all(&eval).unwrap();
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(eval.join("changes.json"), r#"{"requirements": []}"#).unwrap();
        std::fs::write(tmp.join("checks.plan.json"), r#"{"commands": ["cargo test"]}"#).unwrap();
        let view = eval_view(dir.path(), "evl_1").unwrap();
        assert_eq!(view.checks_running, ["cargo test"]);
        std::fs::write(tmp.join("checks.json"), r#"{"facts": [], "findings": []}"#).unwrap();
        assert!(eval_view(dir.path(), "evl_1").unwrap().checks_running.is_empty());
    }

    /// A closed Eval in `root`: whether it leaves a change to look at, and
    /// how its one check went.
    fn closed(root: &Path, id: &str, to_look_at: bool, check: &str) {
        let dir = root.join(".fab7/rf/evals").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("changes.json"), r#"{"requirements": []}"#).unwrap();
        let result = if to_look_at { "unexplained" } else { "explained" };
        let record = serde_json::json!({"schema": "ringframe.eval/2", "eval_id": id,
            "units": [{"id": "w1", "path": "a.rs", "class": "code", "result": result, "agreed": 1, "readings": []}],
            "checks": [{"command": "cargo test", "outcome": check, "seconds": 1}]});
        std::fs::write(dir.join("record.json"), record.to_string()).unwrap();
    }

    fn judged(ask: &str, eval: &str, sealed: Option<&str>) -> weft_core::ledger::Unit {
        weft_core::ledger::Unit {
            ask_id: ask.into(),
            check: Some(weft_core::ledger::Check {
                eval_id: eval.into(),
                report: weft_core::ledger::Report::default(),
                judged_by: None,
            }),
            sealed: sealed.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn each_open_eval_is_ready_to_seal_or_needs_you_and_a_sealed_one_is_neither() {
        use weft_core::board::Notice;
        let dir = tempfile::tempdir().unwrap();
        closed(dir.path(), "evl_ready", false, "succeeded");
        closed(dir.path(), "evl_failed", false, "failed");
        closed(dir.path(), "evl_drifted", true, "succeeded");
        closed(dir.path(), "evl_done", false, "succeeded");
        let units = [
            judged("ask_1", "evl_ready", None),
            judged("ask_2", "evl_failed", None),
            judged("ask_3", "evl_drifted", None),
            judged("ask_4", "evl_done", Some("accepted")),
        ];
        let notices = notices(dir.path(), &units);
        assert_eq!(
            notices.into_iter().collect::<Vec<_>>(),
            [
                ("evl_drifted".to_string(), Notice::NeedsYou),
                ("evl_failed".to_string(), Notice::NeedsYou),
                ("evl_ready".to_string(), Notice::ReadyToSeal),
            ]
        );
    }

    /// "Ready to seal" comes from the Eval and its checks alone: thirty
    /// Seals of Evals just like it, each turned down, change nothing.
    #[test]
    fn past_seals_never_demote_a_ready_eval() {
        use weft_core::board::Notice;
        let dir = tempfile::tempdir().unwrap();
        let mut units = Vec::new();
        for n in 0..30 {
            let id = format!("evl_{n}");
            closed(dir.path(), &id, false, "succeeded");
            units.push(judged(&format!("ask_{n}"), &id, Some("rejected")));
        }
        closed(dir.path(), "evl_new", false, "succeeded");
        units.push(judged("ask_new", "evl_new", None));
        assert_eq!(notices(dir.path(), &units).get("evl_new"), Some(&Notice::ReadyToSeal));
    }

    /// RingFrame's files while an Eval runs: the whole reading is in, and a
    /// re-read of what it was unsure of is handed out.
    #[test]
    fn a_running_eval_is_read_from_the_readings_handed_in_so_far() {
        let dir = tempfile::tempdir().unwrap();
        let rf = dir.path().join(".fab7/rf");
        let eval = rf.join("evals/evl_1");
        let tasks = rf.join("tmp/eval-evl_1/tasks");
        std::fs::create_dir_all(&eval).unwrap();
        std::fs::create_dir_all(&tasks).unwrap();
        let write = |p: std::path::PathBuf, v: serde_json::Value| {
            std::fs::write(p, v.to_string()).unwrap();
        };
        write(
            eval.join("changes.json"),
            serde_json::json!({"requirements": [{"id": "R1", "kind": "local", "text": "Show an error."}]}),
        );
        write(
            eval.join("evidence.json"),
            serde_json::json!({"windows": [{"id": "w1", "path": "src/a.rs", "class": "code"}]}),
        );
        write(
            eval.join("brief.json"),
            serde_json::json!({"asks": [{"ask_id": "a", "title": "fix login"}]}),
        );
        write(tasks.join("evl_1~rd1.json"), serde_json::json!({}));
        write(tasks.join("evl_1~rr1.json"), serde_json::json!({}));
        let out = "evals/evl_1/tasks/evl_1.rd1.1.json";
        std::fs::create_dir_all(rf.join("evals/evl_1/tasks")).unwrap();
        write(
            rf.join(out),
            serde_json::json!({"kind": "read", "task_id": "evl_1~rd1",
                "windows": [{"id": "w1", "answer": "unexplained", "quote": "x", "reason": "Posts outside."}],
                "requirements": [{"id": "R1", "vote": "met", "reason": "shown", "citations": []}]}),
        );
        let event = serde_json::json!({"type": "eval.task", "data": {"eval_id": "evl_1",
            "task_id": "evl_1~rd1", "outcome": "accepted", "artifact": {"path": out}}});
        std::fs::write(rf.join("ledger.jsonl"), format!("{event}\n")).unwrap();

        let v = eval_view(dir.path(), "evl_1").unwrap();
        assert!(!v.closed);
        let unit = v.item("w|w1").unwrap();
        assert_eq!((unit.text.as_str(), unit.again), ("Posts outside.", true), "it is read again");
        let p = v.running().unwrap();
        assert_eq!((p.label.as_str(), p.note.as_str()), ("2nd read", "reading"));
    }

    /// A closed Eval is its record: no other file is needed.
    #[test]
    fn a_closed_eval_is_read_from_its_record_alone() {
        let dir = tempfile::tempdir().unwrap();
        let eval = dir.path().join(".fab7/rf/evals/evl_v4");
        std::fs::create_dir_all(&eval).unwrap();
        std::fs::write(
            eval.join("record.json"),
            include_str!("../../weft-core/tests/fixtures/eval-v4/record.json"),
        )
        .unwrap();
        let v = eval_view(dir.path(), "evl_v4").unwrap();
        assert_eq!(v.headline(), "5 things to look at · 4 settled");
        assert_eq!(read(dir.path(), "evl_v4").unwrap().judged_by(), ["claude"]);
    }
}
