//! Finding the harness sessions Weft has a receipt for.
//!
//! What a recorded session *is* lives in [`weft_core::sessions`]; walking one
//! project's receipts (`~/.fab7/weft/sessions/<project key>/`) happens here.

use std::path::Path;

pub use weft_core::sessions::*;

/// The most recently used session this harness has a receipt for, in one
/// project's receipts `dir`.
pub fn latest(dir: &Path, harness: &str) -> Option<Recorded> {
    std::fs::read_dir(dir.join(harness))
        .ok()?
        .flatten()
        .filter_map(|e| read(harness, &e.path()))
        .max_by_key(|r| r.at)
}

/// One session's receipts: when it was last used. One that only said it was
/// `ready` never held a conversation, so there is nothing to resume. Nothing
/// from the conversation is kept, so there is no last prompt to show.
fn read(harness: &str, path: &Path) -> Option<Recorded> {
    // The id is the file's, not a line's: the file is what weftd keyed on.
    let id = path.file_stem()?.to_str()?.to_string();
    if path.extension()? != "jsonl" || !is_id(&id) {
        return None;
    }
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let turns = weft_core::turns::read(harness, &id, &text);
    let at = last_used(turns.as_ref(), None)?;
    Some(Recorded { id, at, last: String::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One receipt line, as weftd writes it.
    fn receipt(dir: &Path, harness: &str, id: &str, event: &str, time: &str) {
        let d = dir.join(harness);
        std::fs::create_dir_all(&d).expect("harness dir");
        let line = serde_json::json!({"event": event, "session": id, "time": time,
                                      "harness": harness});
        let path = d.join(format!("{id}.jsonl"));
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        text.push_str(&format!("{line}\n"));
        std::fs::write(path, text).expect("receipt");
    }

    #[test]
    fn the_session_offered_is_the_one_most_recently_used() {
        let tmp = tempfile::tempdir().unwrap();
        receipt(tmp.path(), "codex", "older", "turn_ended", "2026-09-20T07:00:00.000Z");
        receipt(tmp.path(), "codex", "newer", "turn_ended", "2026-09-20T09:30:00.000Z");
        let found = latest(tmp.path(), "codex").expect("a session");
        assert_eq!((found.id.as_str(), found.last.as_str()), ("newer", ""));
        assert_eq!(clock(found.at), "09:30");
    }

    #[test]
    fn a_file_named_like_flags_is_not_a_session() {
        let tmp = tempfile::tempdir().unwrap();
        receipt(tmp.path(), "codex", "c1", "turn_ended", "2026-09-20T07:00:00.000Z");
        receipt(tmp.path(), "codex", "--last --yolo", "turn_ended", "2099-01-01T00:00:00.000Z");
        assert_eq!(latest(tmp.path(), "codex").map(|s| s.id), Some("c1".into()));
    }

    #[test]
    fn each_harness_is_asked_about_its_own_sessions_only() {
        let tmp = tempfile::tempdir().unwrap();
        receipt(tmp.path(), "codex", "c1", "working", "2026-09-20T07:00:00.000Z");
        assert_eq!(latest(tmp.path(), "codex").map(|s| s.id), Some("c1".into()));
        assert_eq!(latest(tmp.path(), "claude-code"), None, "no receipt, nothing to offer");
    }

    #[test]
    fn no_receipt_or_one_that_only_started_offers_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(latest(tmp.path(), "codex"), None);
        receipt(tmp.path(), "codex", "s", "ready", "2026-09-20T07:00:00.000Z");
        assert_eq!(latest(tmp.path(), "codex"), None, "it never held a conversation");
    }

    #[test]
    fn receipt_times_are_compared_as_times_whatever_their_shape() {
        let tmp = tempfile::tempdir().unwrap();
        receipt(tmp.path(), "codex", "whole", "turn_ended", "2026-09-27T09:34:06Z");
        receipt(tmp.path(), "codex", "later", "turn_ended", "2026-09-27T09:34:06.392Z");
        assert_eq!(latest(tmp.path(), "codex").map(|s| s.id), Some("later".into()));
    }
}
