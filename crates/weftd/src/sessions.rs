//! Finding the harness sessions RingFrame has a receipt for.
//!
//! What a recorded session *is* lives in [`weft_core::sessions`] (ADR-0007);
//! walking the receipt directory happens here.

use std::path::Path;

pub use weft_core::sessions::*;

/// The most recently used session this harness has a receipt for.
pub fn latest(root: &Path, harness: &str) -> Option<Recorded> {
    let dir = root.join(".fab7").join("rf").join("sessions").join(harness);
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| read(&e.path()))
        .max_by(|a, b| a.at.cmp(&b.at))
}

/// One session directory: the last receipt in it, if it has any.
fn read(dir: &Path) -> Option<Recorded> {
    let id = dir.file_name()?.to_str()?.to_string();
    let text = std::fs::read_to_string(dir.join("prompts.jsonl")).ok()?;
    let last = text.lines().rev().find(|l| !l.trim().is_empty())?;
    let receipt: serde_json::Value = serde_json::from_str(last).ok()?;
    // The id is the directory's, not the receipt's: the directory is what the
    // hook keyed on, and a receipt that disagreed with it would be the bug.
    let at = receipt.get("time")?.as_str()?.to_string();
    let prompt = receipt.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
    Some(Recorded { id, at, last: first_line(prompt) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(root: &Path, harness: &str, id: &str, time: &str, prompt: &str) {
        let dir = root.join(".fab7/rf/sessions").join(harness).join(id);
        std::fs::create_dir_all(&dir).expect("session dir");
        let line = serde_json::json!({
            "session_id": id, "time": time, "prompt": prompt,
            "cwd": root.to_string_lossy(), "bytes": prompt.len(),
        });
        std::fs::write(dir.join("prompts.jsonl"), format!("{line}\n")).expect("receipt");
    }

    /// A throwaway workspace, named the way the other fixtures here are.
    fn workspace(name: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "weft-sessions-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        root
    }

    #[test]
    fn the_session_offered_is_the_one_most_recently_used() {
        let tmp = workspace("case");
        receipt(&tmp, "codex", "older", "2026-09-20T07:00:00.000Z", "$rf:ask one");
        receipt(&tmp, "codex", "newer", "2026-09-20T09:30:00.000Z", "$rf:ask two");
        let found = latest(&tmp, "codex").expect("a session");
        assert_eq!(found.id, "newer");
        assert_eq!(found.last, "$rf:ask two");
        assert_eq!(clock(&found.at), "09:30");
    }

    #[test]
    fn each_harness_is_asked_about_its_own_sessions_only() {
        let tmp = workspace("case");
        receipt(&tmp, "codex", "c1", "2026-09-20T07:00:00.000Z", "$rf:ask one");
        assert_eq!(latest(&tmp, "codex").map(|s| s.id), Some("c1".into()));
        assert_eq!(latest(&tmp, "claude-code"), None, "no receipt, nothing to offer");
    }

    #[test]
    fn a_workspace_with_no_record_offers_nothing_rather_than_a_guess() {
        let tmp = workspace("case");
        assert_eq!(latest(&tmp, "codex"), None);
        // And a session directory with an empty receipt file is the same:
        // there is no event to trace the id to.
        let dir = tmp.join(".fab7/rf/sessions/codex/empty");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("prompts.jsonl"), "").expect("write");
        assert_eq!(latest(&tmp, "codex"), None);
    }

    #[test]
    fn the_last_prompt_is_shown_by_its_first_line() {
        let tmp = workspace("case");
        receipt(&tmp, "codex", "c1", "2026-09-20T07:00:00.000Z", "first line\nsecond line");
        assert_eq!(latest(&tmp, "codex").expect("one").last, "first line");
    }
}
