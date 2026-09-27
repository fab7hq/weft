//! Reading the agent-state receipts RingFrame's plugin writes, under
//! `<project>/.fab7/rf/sessions/<harness>/<id>/turns.jsonl`.
//!
//! What a receipt means lives in [`weft_core::turns`]; walking the directory
//! happens here. Read, never written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use weft_core::turns::*;

/// Every session's receipts in one project, re-read only when a file moved.
#[derive(Default)]
pub struct Turns {
    seen: HashMap<PathBuf, (SystemTime, u64, Option<Session>)>,
}

impl Turns {
    /// Look again, and answer every session whose latest receipt is new.
    pub fn refresh(&mut self, root: &Path) -> Vec<Session> {
        let base = root.join(".fab7").join("rf").join("sessions");
        let mut found = HashMap::new();
        let mut heard = Vec::new();
        for harness in std::fs::read_dir(&base).into_iter().flatten().flatten() {
            let h = harness.file_name().to_string_lossy().into_owned();
            for session in std::fs::read_dir(harness.path()).into_iter().flatten().flatten() {
                let path = session.path().join("turns.jsonl");
                let Ok(meta) = std::fs::metadata(&path) else { continue };
                let stamp = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
                let entry = match self.seen.remove(&path) {
                    Some(old) if (old.0, old.1) == stamp => old,
                    old => {
                        let id = session.file_name().to_string_lossy().into_owned();
                        let text = std::fs::read_to_string(&path).unwrap_or_default();
                        let now = read(&h, &id, &text);
                        let before = old.and_then(|o| o.2).map(|s| (s.latest, s.at));
                        if let Some(s) = now.as_ref().filter(|s| Some((s.latest, s.at)) != before) {
                            heard.push(s.clone());
                        }
                        (stamp.0, stamp.1, now)
                    }
                };
                found.insert(path, entry);
            }
        }
        self.seen = found;
        heard.sort_by_key(|s| s.at);
        heard
    }

    pub fn sessions(&self) -> Vec<Session> {
        self.seen.values().filter_map(|(_, _, s)| s.clone()).collect()
    }
}

/// Now, as receipts count time.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_receipt_is_read_once_and_again_only_when_it_moves() {
        let root = std::env::temp_dir().join(format!("weft-turns-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join(".fab7/rf/sessions/claude-code/s1");
        std::fs::create_dir_all(&dir).expect("dir");
        let line = |e: &str, t: &str| {
            format!("{}\n", serde_json::json!({"event": e, "session_id": "s1", "time": t}))
        };
        std::fs::write(dir.join("turns.jsonl"), line("ready", "2026-09-27T10:00:00.000Z"))
            .expect("write");
        let mut turns = Turns::default();
        assert_eq!(turns.refresh(&root).len(), 1, "first sight");
        assert!(turns.refresh(&root).is_empty(), "nothing moved");
        assert_eq!(turns.sessions()[0].latest, Turn::Ready);
        let more = [
            line("ready", "2026-09-27T10:00:00.000Z"),
            line("working", "2026-09-27T10:00:01.000Z"),
        ]
        .concat();
        std::fs::write(dir.join("turns.jsonl"), more).expect("write");
        assert_eq!(turns.refresh(&root)[0].latest, Turn::Working, "the new receipt");
        assert_eq!(turns.sessions()[0].latest, Turn::Working);
        std::fs::remove_dir_all(&root).ok();
        assert!(turns.refresh(&root).is_empty(), "nothing new was said");
        assert!(turns.sessions().is_empty());
    }
}
