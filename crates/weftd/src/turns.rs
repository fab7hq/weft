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
    /// Look again. True when anything changed.
    pub fn refresh(&mut self, root: &Path) -> bool {
        let base = root.join(".fab7").join("rf").join("sessions");
        let mut found = HashMap::new();
        let mut changed = false;
        for harness in std::fs::read_dir(&base).into_iter().flatten().flatten() {
            let h = harness.file_name().to_string_lossy().into_owned();
            for session in std::fs::read_dir(harness.path()).into_iter().flatten().flatten() {
                let path = session.path().join("turns.jsonl");
                let Ok(meta) = std::fs::metadata(&path) else { continue };
                let stamp = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
                let entry = match self.seen.remove(&path) {
                    Some(old) if (old.0, old.1) == stamp => old,
                    _ => {
                        changed = true;
                        let id = session.file_name().to_string_lossy().into_owned();
                        let text = std::fs::read_to_string(&path).unwrap_or_default();
                        (stamp.0, stamp.1, read(&h, &id, &text))
                    }
                };
                found.insert(path, entry);
            }
        }
        changed |= !self.seen.is_empty();
        self.seen = found;
        changed
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
        assert!(turns.refresh(&root), "first sight");
        assert!(!turns.refresh(&root), "nothing moved");
        assert_eq!(turns.sessions()[0].latest, Turn::Ready);
        let more = [
            line("ready", "2026-09-27T10:00:00.000Z"),
            line("working", "2026-09-27T10:00:01.000Z"),
        ]
        .concat();
        std::fs::write(dir.join("turns.jsonl"), more).expect("write");
        assert!(turns.refresh(&root));
        assert_eq!(turns.sessions()[0].latest, Turn::Working);
        std::fs::remove_dir_all(&root).ok();
        assert!(turns.refresh(&root), "gone is a change");
        assert!(turns.sessions().is_empty());
    }
}
