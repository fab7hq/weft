//! The label stream, kept on the person's machine:
//! `~/.fab7/weft/labels/<project key>.jsonl`, readable only by them.
//!
//! One row for each choice the person made in a dialog: an Ask they sent,
//! with the route, plan and rules they kept, removed or added; a mark on an
//! Eval's item; a Seal, with its disposition and reason. RingFrame's ledger
//! stays the record of the work; this is what Weft learns the person's
//! defaults from. It goes when its project is dismissed, and nothing in it is
//! sent anywhere.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const SCHEMA: &str = "weft.label/1";

/// Where the streams live, beside Weft's `config.toml`.
pub fn dir(config: &Path) -> PathBuf {
    config.with_file_name("labels")
}

/// One project's stream.
pub fn file(config: &Path, root: &Path) -> PathBuf {
    dir(config).join(format!("{}.jsonl", weft_core::sessions::project_key(root)))
}

/// Append one row, readable only by the person: the folder 0700, the file
/// 0600, the row in a single write.
pub fn append(config: &Path, root: &Path, row: &Value) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    let folder = dir(config);
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&folder)?;
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))?;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(file(config, root))?;
    f.write_all(format!("{row}\n").as_bytes())
}

/// Forget the project's stream.
pub fn forget(config: &Path, root: &Path) {
    let _ = std::fs::remove_file(file(config, root));
}

fn now_ms() -> u64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    since.map_or(0, |d| d.as_millis() as u64)
}

/// An Ask the person sent: their words, and the route, plan and practice
/// rules they sent it with, against the rules the sheet offered. No
/// suggestion was made, so `sense` is null.
pub fn asked(draft: &crate::ringframe::AskDraft, ask_id: &str) -> Value {
    let (chosen, offered) = (&draft.rules, &draft.offered);
    let kept: Vec<&String> = chosen.iter().filter(|r| offered.contains(r)).collect();
    let removed: Vec<&String> = offered.iter().filter(|r| !chosen.contains(r)).collect();
    let added: Vec<&String> = chosen.iter().filter(|r| !offered.contains(r)).collect();
    json!({
        "schema": SCHEMA, "kind": "ask", "at": now_ms(), "harness": draft.harness,
        "ask_id": ask_id, "intent": draft.words, "sense": null,
        "final": {"route": draft.route, "plan": draft.plan, "rules_kept": kept,
                  "rules_removed": removed, "rules_added": added},
    })
}

/// A mark on one item of a closed Eval: `asked_for` on a window,
/// `not_meant` on a requirement.
#[derive(Debug, Clone)]
pub struct Mark {
    pub harness: String,
    pub eval_id: String,
    pub mark: &'static str,
    pub item: String,
}

/// A mark the person put on a closed Eval.
pub fn marked(m: &Mark) -> Value {
    let item = Some(m.item.as_str());
    let (window, requirement) = if m.mark == "asked_for" { (item, None) } else { (None, item) };
    json!({
        "schema": SCHEMA, "kind": "mark", "at": now_ms(), "harness": m.harness,
        "eval_id": m.eval_id, "window": window, "requirement": requirement, "mark": m.mark,
    })
}

/// A Seal the person made, from its receipt.
pub fn sealed(harness: &str, receipt: &Value) -> Value {
    json!({
        "schema": SCHEMA, "kind": "seal", "at": now_ms(), "harness": harness,
        "seal_id": receipt["seal_id"], "eval_id": receipt["eval"]["eval_id"],
        "disposition": receipt["disposition"], "reason": receipt["reason"],
        "features": {}, "p_accept": null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn draft() -> crate::ringframe::AskDraft {
        crate::ringframe::AskDraft {
            words: "Fix the login bug.".into(),
            harness: "codex".into(),
            route: "native_plan".into(),
            task: "implement".into(),
            plan: Some("plans/x/plan.md".into()),
            rules: vec!["practice.kiss".into(), "mine.1".into()],
            offered: vec!["practice.kiss".into(), "practice.yagni".into()],
            link: None,
        }
    }

    #[test]
    fn an_ask_a_mark_and_a_seal_each_append_one_private_row_and_dismissing_deletes_them() {
        let tmp = tempfile::tempdir().unwrap();
        let (config, root) = (tmp.path().join("weft/config.toml"), Path::new("/work/project"));
        append(&config, root, &asked(&draft(), "ask_1")).unwrap();
        let receipt = json!({"seal_id": "sel_1", "eval": {"eval_id": "evl_1"},
                             "disposition": "rejected", "reason": "wrong_approach"});
        append(&config, root, &sealed("codex", &receipt)).unwrap();
        let mark = |mark, item: &str| Mark {
            harness: "codex".into(),
            eval_id: "evl_1".into(),
            mark,
            item: item.into(),
        };
        append(&config, root, &marked(&mark("asked_for", "w_1"))).unwrap();
        append(&config, root, &marked(&mark("not_meant", "ask_1/f0"))).unwrap();
        let path = file(&config, root);
        let text = std::fs::read_to_string(&path).unwrap();
        let rows: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(rows.len(), 4, "one row each");
        assert_eq!((&rows[0]["schema"], &rows[0]["kind"]), (&json!(SCHEMA), &json!("ask")));
        assert_eq!(rows[0]["intent"], "Fix the login bug.");
        assert_eq!(
            rows[0]["final"],
            json!({"route": "native_plan", "plan": "plans/x/plan.md",
                   "rules_kept": ["practice.kiss"], "rules_removed": ["practice.yagni"],
                   "rules_added": ["mine.1"]})
        );
        assert_eq!(rows[0]["sense"], Value::Null);
        assert_eq!(
            (&rows[1]["kind"], &rows[1]["eval_id"], &rows[1]["disposition"], &rows[1]["reason"]),
            (&json!("seal"), &json!("evl_1"), &json!("rejected"), &json!("wrong_approach"))
        );
        let mark = |r: &Value| {
            (r["kind"].clone(), r["window"].clone(), r["requirement"].clone(), r["mark"].clone())
        };
        assert_eq!(mark(&rows[2]), (json!("mark"), json!("w_1"), Value::Null, json!("asked_for")));
        assert_eq!(
            mark(&rows[3]),
            (json!("mark"), Value::Null, json!("ask_1/f0"), json!("not_meant"))
        );
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&path), mode(&dir(&config))), (0o600, 0o700));
        forget(&config, root);
        assert!(!path.exists(), "dismissing the project deletes its stream");
    }
}
