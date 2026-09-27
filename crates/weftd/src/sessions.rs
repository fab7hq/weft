//! Finding the harness sessions RingFrame has a receipt for.
//!
//! What a recorded session *is* lives in [`weft_core::sessions`]; walking the
//! receipt directory happens here.

use std::path::Path;

pub use weft_core::sessions::*;

/// The most recently used session this harness has a receipt for.
pub fn latest(root: &Path, harness: &str) -> Option<Recorded> {
    let dir = root.join(".fab7").join("rf").join("sessions").join(harness);
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| read(harness, &e.path()))
        .max_by_key(|r| r.at)
}

/// One session directory: when it was last used and its last prompt, from
/// its receipts. A harness with no prompt hook (Antigravity) records only its
/// turns, and a session in which a turn ran is still one to pick up. One that
/// only said it was `ready` never held a conversation, so there is nothing
/// to resume.
fn read(harness: &str, dir: &Path) -> Option<Recorded> {
    // The id is the directory's, not the receipt's: the directory is what the
    // hook keyed on, and a receipt that disagreed with it would be the bug.
    let id = dir.file_name()?.to_str()?.to_string();
    let turns = std::fs::read_to_string(dir.join("turns.jsonl")).unwrap_or_default();
    let turns = weft_core::turns::read(harness, &id, &turns);
    let prompt = receipts(&dir.join("prompts.jsonl")).pop();
    let prompted = prompt.as_ref().and_then(|r| weft_core::turns::millis(r.get("time")?.as_str()?));
    let at = last_used(turns.as_ref(), prompted)?;
    let said = prompt.as_ref().and_then(|r| r.get("prompt")?.as_str()).unwrap_or("");
    Some(Recorded { id, at, last: first_line(said) })
}

/// Every receipt a file holds, skipping a line that is not one.
fn receipts(path: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
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
        assert_eq!(clock(found.at), "09:30");
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

    /// Antigravity has no prompt hook, so its sessions hold only turn
    /// receipts: it is still offered, as of its latest turn, with no prompt
    /// to show.
    #[test]
    fn a_session_with_only_turn_receipts_is_offered_too() {
        let tmp = workspace("case");
        let dir = tmp.join(".fab7/rf/sessions/agy/c1");
        std::fs::create_dir_all(&dir).expect("dir");
        let turns = [
            r#"{"event":"working","session_id":"c1","time":"2026-09-27T09:33:00.000Z"}"#,
            r#"{"event":"turn_ended","session_id":"c1","time":"2026-09-27T09:34:06.392Z"}"#,
        ];
        std::fs::write(dir.join("turns.jsonl"), turns.join("\n") + "\n").expect("turns");
        let found = latest(&tmp, "agy").expect("a session");
        assert_eq!((found.id.as_str(), found.last.as_str()), ("c1", ""));
        let at = |t| weft_core::turns::millis(t).expect("a time");
        assert_eq!(found.at, at("2026-09-27T09:34:06.392Z"));
        // Where both are kept, the later of the two says when it was used.
        receipt(&tmp, "agy", "c1", "2026-09-27T09:32:00.000Z", "/plan it");
        let found = latest(&tmp, "agy").expect("a session");
        assert_eq!((found.at, found.last.as_str()), (at("2026-09-27T09:34:06.392Z"), "/plan it"));
    }

    #[test]
    fn a_session_that_only_started_has_nothing_to_resume() {
        let tmp = workspace("case");
        let dir = tmp.join(".fab7/rf/sessions/claude-code/s1");
        std::fs::create_dir_all(&dir).expect("dir");
        let ready = r#"{"event":"ready","session_id":"s1","time":"2026-09-27T09:33:00.000Z"}"#;
        std::fs::write(dir.join("turns.jsonl"), format!("{ready}\n")).expect("turns");
        assert_eq!(latest(&tmp, "claude-code"), None);
    }

    /// RingFrame writes a time with or without its milliseconds. As text,
    /// `…06Z` sorts after `…06.392Z`; as times, it is before.
    #[test]
    fn receipt_times_are_compared_as_times_whatever_their_shape() {
        let tmp = workspace("shapes");
        receipt(&tmp, "codex", "whole", "2026-09-27T09:34:06Z", "$rf:ask one");
        receipt(&tmp, "codex", "later", "2026-09-27T09:34:06.392Z", "$rf:ask two");
        assert_eq!(latest(&tmp, "codex").map(|s| s.id), Some("later".into()));
        // Within one session too: a turn after its last prompt.
        let dir = tmp.join(".fab7/rf/sessions/codex/whole");
        let ended = r#"{"event":"turn_ended","session_id":"whole","time":"2026-09-27T09:34:07Z"}"#;
        std::fs::write(dir.join("turns.jsonl"), format!("{ended}\n")).expect("turns");
        let found = latest(&tmp, "codex").expect("a session");
        let at = weft_core::turns::millis("2026-09-27T09:34:07Z");
        assert_eq!((found.id.as_str(), Some(found.at)), ("whole", at));
    }

    #[test]
    fn the_last_prompt_is_shown_by_its_first_line() {
        let tmp = workspace("case");
        receipt(&tmp, "codex", "c1", "2026-09-20T07:00:00.000Z", "first line\nsecond line");
        assert_eq!(latest(&tmp, "codex").expect("one").last, "first line");
    }
}
