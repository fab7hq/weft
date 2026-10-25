//! Reading an Eval record off disk.
//!
//! What one means lives in [`weft_core::record`]; this opens the file and
//! hands the bytes over.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;
use weft_core::accept;
use weft_core::board::{Notice, notice};
use weft_core::eval_view::EvalView;

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
/// record once it has closed, else what its judges have handed in so far.
pub fn eval_view(project_root: &Path, eval_id: &str) -> Option<weft_core::eval_view::EvalView> {
    if !plain(eval_id) {
        return None;
    }
    let rf = project_root.join(".fab7/rf");
    let dir = rf.join("evals").join(eval_id);
    let changes = json_at(&dir.join("changes.json"))?;
    let evidence = json_at(&dir.join("evidence.json")).unwrap_or(Value::Null);
    let brief = json_at(&dir.join("brief.json")).unwrap_or(Value::Null);
    let windows = json_at(&dir.join("windows.json")).unwrap_or(Value::Null);
    let record = json_at(&dir.join("record.json"));
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
        windows: &windows,
        record: record.as_ref(),
        outputs: &outputs,
        planned: &planned,
        done: &done,
        checks_running: &crate::ringframe::checks_running(project_root, eval_id),
    }))
}

/// What each open Eval asks of the person, by its id: ready to seal or needs
/// you, a "Ready" demoted when your own past Seals say you turn such down.
pub fn notices(project_root: &Path, units: &[weft_core::ledger::Unit]) -> BTreeMap<String, Notice> {
    let mut sealed = BTreeMap::new();
    let mut open = BTreeSet::new();
    for u in units {
        let Some(c) = &u.check else { continue };
        if let Some(d) = &u.sealed {
            sealed.insert(c.eval_id.clone(), d == "accepted");
        } else {
            open.insert(c.eval_id.clone());
        }
    }
    let views: Vec<(String, EvalView)> = open
        .into_iter()
        .filter_map(|id| Some((id.clone(), eval_view(project_root, &id)?)))
        .collect();
    let any_ready = views.iter().any(|(_, v)| notice(v) == Some(Notice::ReadyToSeal));
    let demotion = any_ready.then(|| learned(project_root, &sealed)).flatten();
    views
        .iter()
        .filter_map(|(id, view)| {
            let n = notice(view)?;
            Some((id.clone(), demotion.as_ref().map_or(n, |d| d.apply(n, &accept::features(view)))))
        })
        .collect()
}

/// The demotion your sealed Evals teach, if there are enough of them.
fn learned(project_root: &Path, sealed: &BTreeMap<String, bool>) -> Option<accept::Demotion> {
    let seals: Vec<accept::Seal> = sealed
        .iter()
        .filter_map(|(id, accepted)| {
            let view = eval_view(project_root, id)?;
            Some(accept::Seal {
                features: accept::features(&view),
                ready: notice(&view) == Some(Notice::ReadyToSeal),
                accepted: *accepted,
            })
        })
        .collect();
    accept::Demotion::learn(&seals)
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

    /// A closed Eval in `root`: its verdict, and how its one check went.
    fn closed(root: &Path, id: &str, verdict: &str, check: &str) {
        let dir = root.join(".fab7/rf/evals").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("changes.json"), r#"{"requirements": []}"#).unwrap();
        let record = serde_json::json!({"verdict": verdict, "confidence": 1.0, "items": [],
            "windows": [], "checks": [{"command": "cargo test", "outcome": check, "seconds": 1}]});
        std::fs::write(dir.join("record.json"), record.to_string()).unwrap();
    }

    fn judged(ask: &str, eval: &str, sealed: Option<&str>) -> weft_core::ledger::Unit {
        weft_core::ledger::Unit {
            ask_id: ask.into(),
            check: Some(weft_core::ledger::Check {
                eval_id: eval.into(),
                verdict: weft_core::ledger::Verdict::Matches,
                agreement: 1.0,
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
        closed(dir.path(), "evl_ready", "aligned", "succeeded");
        closed(dir.path(), "evl_failed", "aligned", "failed");
        closed(dir.path(), "evl_drifted", "drifted", "succeeded");
        closed(dir.path(), "evl_done", "aligned", "succeeded");
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

    /// Thirty Seals of Evals just like it, each turned down: a "Ready" you
    /// keep turning down needs you.
    #[test]
    fn past_seals_turned_down_demote_a_ready_eval() {
        use weft_core::board::Notice;
        let dir = tempfile::tempdir().unwrap();
        let mut units = Vec::new();
        for n in 0..30 {
            let id = format!("evl_{n}");
            closed(dir.path(), &id, "aligned", "succeeded");
            units.push(judged(&format!("ask_{n}"), &id, Some("rejected")));
        }
        closed(dir.path(), "evl_new", "aligned", "succeeded");
        units.push(judged("ask_new", "evl_new", None));
        assert_eq!(notices(dir.path(), &units).get("evl_new"), Some(&Notice::NeedsYou));
        units.pop();
        units.remove(0);
        units.push(judged("ask_new", "evl_new", None));
        assert_eq!(
            notices(dir.path(), &units).get("evl_new"),
            Some(&Notice::ReadyToSeal),
            "29 Seals teach nothing"
        );
    }
}
