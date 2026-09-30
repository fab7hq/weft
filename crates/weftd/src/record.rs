//! Reading an Eval record off disk.
//!
//! What one means lives in [`weft_core::record`]; this opens the file and
//! hands the bytes over.

use std::path::Path;

use serde_json::Value;

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
        record: record.as_ref(),
        outputs: &outputs,
        planned: &planned,
        done: &done,
    }))
}

/// One change's hunk, as RingFrame cut it.
pub fn window_text(project_root: &Path, eval_id: &str, window: &str) -> Option<String> {
    if !plain(eval_id) || !plain(window) {
        return None;
    }
    let doc = json_at(&project_root.join(".fab7/rf/evals").join(eval_id).join("windows.json"))?;
    doc["windows"][window].as_str().map(str::to_string)
}
