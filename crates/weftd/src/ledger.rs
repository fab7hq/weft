//! Following a ledger file on disk.
//!
//! The meaning of what is in it lives in [`weft_core::ledger`], which this
//! re-exports: the rules are testable without a filesystem, and reading one is
//! all that happens here (ADR-0007).

use std::path::{Path, PathBuf};

use serde_json::Value;

pub use weft_core::ledger::*;

/// A ledger being followed. Refresh is a stat, then a read of only new bytes.
pub struct Ledger {
    pub path: PathBuf,
    consumed: usize,
    head: Option<[u8; 32]>,
    events: Vec<Value>,
}

impl Ledger {
    pub fn at(project_root: &Path) -> Self {
        Self {
            path: project_root.join(".fab7/rf/ledger.jsonl"),
            consumed: 0,
            head: None,
            events: Vec::new(),
        }
    }

    pub fn units(&self) -> Vec<Unit> {
        project(&self.events)
    }

    /// Returns true when anything new was folded in.
    pub fn refresh(&mut self) -> bool {
        let Ok(meta) = std::fs::metadata(&self.path) else {
            return false;
        };
        let size = meta.len() as usize;
        if size == self.consumed && self.head.is_some() {
            return false;
        }
        let Ok(bytes) = std::fs::read(&self.path) else {
            return false;
        };
        let head = head_digest(&bytes);
        // Shrunk, or rewritten from the start: the cache cannot be trusted.
        if size < self.consumed || (self.head.is_some() && self.head != head) {
            self.consumed = 0;
            self.events.clear();
        }
        self.head = head;
        let (mut new, used) = parse_prefix(&bytes[self.consumed..]);
        if new.is_empty() && used == 0 {
            return false;
        }
        self.consumed += used;
        self.events.append(&mut new);
        true
    }
}

fn head_digest(bytes: &[u8]) -> Option<[u8; 32]> {
    if bytes.is_empty() {
        return None;
    }
    // Cheap change detector over the first block: a rewritten ledger is a
    // different ledger, and must be re-read from zero rather than appended to.
    let n = bytes.len().min(4096);
    let mut out = [0u8; 32];
    for (i, b) in bytes[..n].iter().enumerate() {
        out[i % 32] ^= b.rotate_left((i % 8) as u32);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn compiled(id: &str, title: &str, host: &str, mode: &str) -> Value {
        json!({
            "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.compiled",
            "time": "2026-09-19T14:02:00Z", "id": id,
            "actor": {"kind": "human", "id": "local-user"}, "links": [],
            "data": {
                "title": title, "selected_capability": "native_plan", "delivery_mode": mode,
                "host": {"name": host, "profile_id": host,
                         "session_ref": "01a0bdb6-1d1f", "session_ref_source": "capture"},
                "source": {"role": "source_intent", "path": "x", "bytes": 2, "sha256": "a"},
                "prompt": {"role": "generated_prompt", "path": "y", "bytes": 9, "sha256": "b"},
                "source_verified": "exact", "limitations": [],
                "classification": {}, "route_explanation": {}
            }
        })
    }

    fn ev(kind: &str, id: &str, data: Value) -> Value {
        json!({
            "schema": "ringframe.ledger/1", "event_id": "evt_x", "type": kind,
            "time": "2026-09-19T14:03:00Z", "id": id,
            "actor": {"kind": "human", "id": "local-user"}, "links": [], "data": data
        })
    }

    fn lines(events: &[Value]) -> Vec<u8> {
        let mut out = Vec::new();
        for e in events {
            out.extend_from_slice(serde_json::to_string(e).unwrap().as_bytes());
            out.push(b'\n');
        }
        out
    }


    #[test]
    fn folding_new_lines_matches_reading_the_whole_file() {
        let dir = std::env::temp_dir().join(format!("weft-ledger-{}", std::process::id()));
        let rf = dir.join(".fab7/rf");
        std::fs::create_dir_all(&rf).unwrap();
        let path = rf.join("ledger.jsonl");

        std::fs::write(&path, lines(&[compiled("ask_1", "one", "codex", "human_handoff")])).unwrap();
        let mut led = Ledger::at(&dir);
        assert!(led.refresh());
        assert_eq!(led.units().len(), 1);
        assert!(!led.refresh(), "an untouched ledger reports no change");

        let mut grown = lines(&[compiled("ask_1", "one", "codex", "human_handoff")]);
        grown.extend(lines(&[ev("ask.confirmed", "ask_1", json!({"confirmation": {}}))]));
        std::fs::write(&path, &grown).unwrap();
        assert!(led.refresh());

        let folded = led.units();
        let (all, _) = parse_prefix(&grown);
        assert_eq!(folded, project(&all), "folding must equal a full re-read");
        assert_eq!(folded[0].sent, Sent::ReadyToSend);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_rewritten_ledger_is_read_from_zero_rather_than_appended_to() {
        let dir = std::env::temp_dir().join(format!("weft-rewrite-{}", std::process::id()));
        let rf = dir.join(".fab7/rf");
        std::fs::create_dir_all(&rf).unwrap();
        let path = rf.join("ledger.jsonl");

        std::fs::write(&path, lines(&[compiled("ask_1", "first", "codex", "human_handoff")])).unwrap();
        let mut led = Ledger::at(&dir);
        led.refresh();
        assert_eq!(led.units()[0].title, "first");

        std::fs::write(
            &path,
            lines(&[
                compiled("ask_9", "replaced", "claude-code", "human_handoff"),
                compiled("ask_8", "and another", "claude-code", "human_handoff"),
            ]),
        )
        .unwrap();
        led.refresh();
        let units = led.units();
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].title, "replaced");

        std::fs::remove_dir_all(&dir).ok();
    }
}
