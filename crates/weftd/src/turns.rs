//! Reading Weft's own session receipts for one project,
//! `~/.fab7/weft/sessions/<project key>/<harness>/<id>.jsonl` ([`crate::receipts`]).
//!
//! What a receipt means lives in [`weft_core::turns`]; walking the directory
//! happens here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use weft_core::sessions::{Heard, Process};
pub use weft_core::turns::*;

/// Every session's receipts in one project, re-read only when a file moved.
#[derive(Default)]
pub struct Turns {
    seen: HashMap<PathBuf, (SystemTime, u64, Option<Session>, Option<Process>)>,
}

impl Turns {
    /// Look again at one project's receipts (`dir`), and answer every
    /// session whose latest receipt is new.
    pub fn refresh(&mut self, dir: &Path) -> Vec<Session> {
        let mut found = HashMap::new();
        let mut heard = Vec::new();
        for harness in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let h = harness.file_name().to_string_lossy().into_owned();
            for session in std::fs::read_dir(harness.path()).into_iter().flatten().flatten() {
                let path = session.path();
                if path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                let Ok(meta) = std::fs::metadata(&path) else { continue };
                let stamp = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
                let entry = match self.seen.remove(&path) {
                    Some(old) if (old.0, old.1) == stamp => old,
                    old => {
                        let id =
                            path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                        let text = std::fs::read_to_string(&path).unwrap_or_default();
                        let now = read(&h, &id, &text);
                        let before = old.and_then(|o| o.2).map(|s| (s.latest, s.at));
                        if let Some(s) = now.as_ref().filter(|s| Some((s.latest, s.at)) != before) {
                            heard.push(s.clone());
                        }
                        (stamp.0, stamp.1, now, weft_core::sessions::process(&text))
                    }
                };
                found.insert(path, entry);
            }
        }
        self.seen = found;
        heard.sort_by_key(|s| s.at);
        heard
    }

    /// Every session's latest event, in a fixed order, so the same receipts
    /// always make the same answer and nothing is told twice.
    pub fn sessions(&self) -> Vec<Session> {
        let mut all: Vec<Session> =
            self.seen.values().filter_map(|(_, _, s, _)| s.clone()).collect();
        all.sort_by(|a, b| (&a.harness, &a.id).cmp(&(&b.harness, &b.id)));
        all
    }

    /// Every session's receipts, with the harness process they name last.
    pub fn heard(&self) -> Vec<Heard> {
        self.seen
            .values()
            .filter_map(|(_, _, s, p)| Some(Heard { session: s.clone()?, process: p.clone() }))
            .collect()
    }
}

/// The Mac's distance from UTC now, in milliseconds: how its clock and its
/// calendar read a time (the earlier sessions' date groups).
pub fn zone() -> i64 {
    let now = now_millis() / 1000;
    // SAFETY: `localtime_r` writes only the `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = now as libc::time_t;
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i64 * 1000
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
        let base = tempfile::tempdir().unwrap();
        let (base, root) = (base.path(), Path::new("/work/thing"));
        let dir = crate::receipts::project_dir(base, root);
        let signal = |state: &str, at: i64| {
            let sent = weft_core::signal::Sent {
                harness: "claude-code".into(),
                session: "s1".into(),
                state: Some(state.into()),
                ..Default::default()
            };
            crate::receipts::record(base, root, &sent, at).unwrap();
        };
        signal("ready", 1_000);
        let mut turns = Turns::default();
        assert_eq!(turns.refresh(&dir).len(), 1, "first sight");
        assert!(turns.refresh(&dir).is_empty(), "nothing moved");
        assert_eq!(turns.sessions()[0].latest, Turn::Ready);
        signal("working", 2_000);
        assert_eq!(turns.refresh(&dir)[0].latest, Turn::Working, "the new receipt");
        // A daemon started again reads the same state from the same receipts.
        let mut again = Turns::default();
        again.refresh(&dir);
        assert_eq!(again.sessions(), turns.sessions());
        std::fs::remove_dir_all(&dir).ok();
        assert!(turns.refresh(&dir).is_empty(), "nothing new was said");
        assert!(turns.sessions().is_empty());
    }
}
