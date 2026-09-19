//! Reading the RingFrame ledger.
//!
//! Spec: `plans/weft/spec/board.md`. Weft reads `ledger.jsonl` directly and
//! never writes it. Direct reads are safe without a lock because the ledger is
//! append-only and each line is written whole: a reader sees a prefix of
//! complete lines, possibly followed by a partial tail, which is discarded.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// What the screen says, per `spec/interface.md` §Vocabulary. The recorded
/// term travels alongside so the plain wording is a gloss, never a substitute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Matches,
    DoesntMatch,
    NotEnoughProof,
}

impl Verdict {
    pub fn recorded(self) -> &'static str {
        match self {
            Verdict::Matches => "aligned",
            Verdict::DoesntMatch => "drifted",
            Verdict::NotEnoughProof => "incomplete",
        }
    }

    pub fn plain(self) -> &'static str {
        match self {
            Verdict::Matches => "matches what you asked",
            Verdict::DoesntMatch => "doesn't match what you asked",
            Verdict::NotEnoughProof => "not enough proof either way",
        }
    }

    fn from_recorded(s: &str) -> Option<Self> {
        match s {
            "aligned" => Some(Verdict::Matches),
            "drifted" => Some(Verdict::DoesntMatch),
            "incomplete" => Some(Verdict::NotEnoughProof),
            _ => None,
        }
    }
}

/// Where a unit of work stands on its way into an agent.
///
/// A route the harness takes for itself and a route the person has to type are
/// different situations that want opposite things: one needs nothing, the
/// other is waiting on you. They are not one state with a missing receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    /// Compiled on a handoff route, not yet confirmed.
    NotSent,
    /// Confirmed on a handoff route, waiting to be typed. Needs you.
    ReadyToSend,
    /// Typed by Weft, with no receipt yet. Weft's claim about Weft.
    Unconfirmed,
    /// A dispatch route: the harness took it and there is nothing to send.
    TakenByAgent,
    /// A hook saw it arrive. `exact` is a digest match against `prompt.txt`.
    Arrived { exact: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub eval_id: String,
    pub verdict: Verdict,
    /// Judge agreement, never correctness.
    pub agreement: f64,
    /// The host that produced the verdict. Always shown; no parity is implied.
    pub judged_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    pub ask_id: String,
    pub title: String,
    pub harness: String,
    pub route: String,
    pub asked_at: String,
    pub delivery_mode: String,
    pub cancelled: bool,
    pub confirmed: bool,
    pub sent: Sent,
    pub check: Option<Check>,
    pub sealed: Option<String>,
}

impl Unit {
    /// The one-line status the list shows. Plain words only.
    pub fn status(&self) -> String {
        if self.cancelled {
            return "cancelled".into();
        }
        if let Some(d) = &self.sealed {
            return match d.as_str() {
                "accepted" => "accepted".into(),
                "rejected" => "rejected".into(),
                "deferred" => "parked".into(),
                "abandoned" => "dropped".into(),
                other => other.into(),
            };
        }
        if let Some(c) = &self.check {
            return c.verdict.plain().into();
        }
        match self.sent {
            Sent::NotSent => "not sent yet".into(),
            Sent::ReadyToSend => "ready to send".into(),
            Sent::Unconfirmed => "sent, unconfirmed".into(),
            Sent::TakenByAgent => "the agent took it".into(),
            Sent::Arrived { exact: true } => "sent · word for word".into(),
            Sent::Arrived { exact: false } => "sent, reworded".into(),
        }
    }

    /// Waiting on a decision only the person can make: a prompt to type, or a
    /// verdict to decide on.
    ///
    /// A route the agent took for itself asks nothing, so it never counts —
    /// otherwise the count stops meaning "act now".
    pub fn needs_you(&self) -> bool {
        if self.cancelled || self.sealed.is_some() {
            return false;
        }
        matches!(self.sent, Sent::ReadyToSend) || self.check.is_some()
    }
}

/// Parse whole lines, discarding a torn tail. Returns the bytes consumed.
pub fn parse_prefix(bytes: &[u8]) -> (Vec<Value>, usize) {
    let mut events = Vec::new();
    let mut consumed = 0usize;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        if !line.ends_with(b"\n") {
            break; // a partial final line; a writer is mid-append
        }
        consumed += line.len();
        if let Ok(v) = serde_json::from_slice::<Value>(line) {
            events.push(v);
        }
    }
    (events, consumed)
}

fn s<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str()
}

pub fn project(events: &[Value]) -> Vec<Unit> {
    let mut units: Vec<Unit> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for e in events {
        let kind = s(e, &["type"]).unwrap_or_default();
        let id = s(e, &["id"]).unwrap_or_default().to_string();
        let data = e.get("data").cloned().unwrap_or(Value::Null);

        match kind {
            "ask.compiled" => {
                let mode = s(&data, &["delivery_mode"]).unwrap_or_default().to_string();
                // A dispatch route has nothing for the person to send.
                let sent = if mode == "native_dispatch" { Sent::TakenByAgent } else { Sent::NotSent };
                index.insert(id.clone(), units.len());
                units.push(Unit {
                    ask_id: id,
                    title: s(&data, &["title"]).unwrap_or("untitled").to_string(),
                    harness: s(&data, &["host", "name"]).unwrap_or("unknown").to_string(),
                    route: s(&data, &["selected_capability"]).unwrap_or_default().to_string(),
                    asked_at: s(e, &["time"]).unwrap_or_default().to_string(),
                    delivery_mode: mode,
                    cancelled: false,
                    confirmed: false,
                    sent,
                    check: None,
                    sealed: None,
                });
            }
            "ask.confirmed" => {
                if let Some(u) = index.get(&id).and_then(|i| units.get_mut(*i)) {
                    u.confirmed = true;
                    if u.delivery_mode == "human_handoff" && u.sent == Sent::NotSent {
                        u.sent = Sent::ReadyToSend;
                    }
                }
            }
            "ask.cancelled" => {
                if let Some(u) = index.get(&id).and_then(|i| units.get_mut(*i)) {
                    u.cancelled = true;
                }
            }
            "ask.delivery" => {
                if let Some(u) = index.get(&id).and_then(|i| units.get_mut(*i)) {
                    match s(&data, &["state"]) {
                        Some("native_accepted") => u.sent = Sent::TakenByAgent,
                        Some("handoff_ready") if u.sent == Sent::NotSent => {
                            u.sent = Sent::ReadyToSend
                        }
                        _ => {}
                    }
                }
            }
            "ask.submission" => {
                if let Some(u) = index.get(&id).and_then(|i| units.get_mut(*i)) {
                    let exact = data.get("as_modified").and_then(Value::as_bool) != Some(true);
                    u.sent = Sent::Arrived { exact };
                }
            }
            "eval.completed" => {
                let Some(verdict) = s(&data, &["verdict"]).and_then(Verdict::from_recorded) else {
                    continue;
                };
                let check = Check {
                    eval_id: id,
                    verdict,
                    agreement: data.get("confidence").and_then(Value::as_f64).unwrap_or(0.0),
                    judged_by: s(&data, &["judged_by"]).map(str::to_string),
                };
                for ask in basis_asks(&data) {
                    if let Some(u) = index.get(&ask).and_then(|i| units.get_mut(*i)) {
                        u.check = Some(check.clone());
                    }
                }
            }
            "seal.created" => {
                let disposition = s(&data, &["disposition"]).unwrap_or_default().to_string();
                for ask in basis_asks(&data) {
                    if let Some(u) = index.get(&ask).and_then(|i| units.get_mut(*i)) {
                        u.sealed = Some(disposition.clone());
                    }
                }
            }
            _ => {}
        }
    }
    units
}

fn basis_asks(data: &Value) -> Vec<String> {
    data.get("basis")
        .and_then(|b| b.get("asks"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

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
    use serde_json::json;

    fn compiled(id: &str, title: &str, host: &str, mode: &str) -> Value {
        json!({
            "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.compiled",
            "time": "2026-09-19T14:02:00Z", "id": id,
            "actor": {"kind": "human", "id": "local-user"}, "links": [],
            "data": {
                "title": title,
                "selected_capability": "native_plan",
                "delivery_mode": mode,
                "host": {"name": host, "profile_id": host},
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
    fn a_compiled_ask_becomes_one_unit_of_work() {
        let units = project(&[compiled("ask_1", "health endpoint", "codex", "human_handoff")]);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].title, "health endpoint");
        assert_eq!(units[0].harness, "codex");
        assert_eq!(units[0].status(), "not sent yet");
    }

    #[test]
    fn a_confirmed_handoff_is_ready_to_send() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("ask.confirmed", "ask_1", json!({"confirmation": {}})),
        ]);
        assert_eq!(units[0].sent, Sent::ReadyToSend);
        assert_eq!(units[0].status(), "ready to send");
        assert!(units[0].needs_you());
    }

    #[test]
    fn a_route_the_agent_takes_says_so_and_asks_nothing() {
        // It used to read "not sent yet", which was the opposite of true:
        // there is nothing to send, and the work may already be done.
        let units = project(&[
            compiled("ask_1", "t", "claude-code", "native_dispatch"),
            ev("ask.confirmed", "ask_1", json!({"confirmation": {}})),
        ]);
        assert_eq!(units[0].sent, Sent::TakenByAgent);
        assert_eq!(units[0].status(), "the agent took it");
        assert!(!units[0].needs_you(), "a dispatch route never waits on you");
    }

    #[test]
    fn a_handoff_that_is_ready_is_the_thing_that_waits_on_you() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("ask.delivery", "ask_1", json!({
                "mode": "human_handoff", "mechanism": null, "state": "handoff_ready",
                "qualification": null, "receipt": null, "submission": "unobserved",
                "limitations": []
            })),
        ]);
        assert_eq!(units[0].sent, Sent::ReadyToSend);
        assert!(units[0].needs_you());
    }

    #[test]
    fn a_receipt_outranks_the_route_it_arrived_on() {
        let units = project(&[
            compiled("ask_1", "t", "claude-code", "native_dispatch"),
            ev("ask.submission", "ask_1", json!({
                "state": "observed", "observed_by": "hook:UserPromptSubmit",
                "attributed_by": null, "as_modified": false,
                "host": {"name": "claude-code"}, "prompt_sha256": "b"
            })),
        ]);
        assert_eq!(units[0].status(), "sent · word for word");
    }

    #[test]
    fn a_hook_receipt_is_what_makes_it_sent_word_for_word() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("ask.confirmed", "ask_1", json!({"confirmation": {}})),
            ev("ask.submission", "ask_1", json!({
                "state": "observed", "observed_by": "hook:UserPromptSubmit",
                "attributed_by": null, "as_modified": false,
                "host": {"name": "codex"}, "prompt_sha256": "b"
            })),
        ]);
        assert_eq!(units[0].sent, Sent::Arrived { exact: true });
        assert_eq!(units[0].status(), "sent · word for word");
    }

    #[test]
    fn a_reworded_submission_says_so() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("ask.submission", "ask_1", json!({
                "state": "observed", "observed_by": "hook:UserPromptSubmit",
                "attributed_by": null, "as_modified": true,
                "host": {"name": "codex"}, "prompt_sha256": "zz"
            })),
        ]);
        assert_eq!(units[0].status(), "sent, reworded");
    }

    #[test]
    fn a_verdict_is_shown_in_plain_words_and_keeps_its_recorded_term() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("eval.completed", "evl_1", json!({
                "basis": {"asks": ["ask_1"]}, "subject": {},
                "verdict": "drifted", "confidence": 0.67,
                "artifact": {}, "limitations": []
            })),
        ]);
        let check = units[0].check.clone().expect("a check");
        assert_eq!(check.verdict, Verdict::DoesntMatch);
        assert_eq!(check.verdict.plain(), "doesn't match what you asked");
        assert_eq!(check.verdict.recorded(), "drifted");
        assert_eq!(check.agreement, 0.67);
        assert_eq!(units[0].status(), "doesn't match what you asked");
    }

    #[test]
    fn an_eval_reaches_every_ask_in_its_basis() {
        let units = project(&[
            compiled("ask_1", "one", "codex", "human_handoff"),
            compiled("ask_2", "two", "codex", "human_handoff"),
            ev("eval.completed", "evl_1", json!({
                "basis": {"asks": ["ask_1", "ask_2"]}, "subject": {},
                "verdict": "aligned", "confidence": 1.0, "artifact": {}, "limitations": []
            })),
        ]);
        assert!(units.iter().all(|u| u.check.is_some()));
    }

    #[test]
    fn a_seal_closes_the_unit_and_nothing_needs_you_afterwards() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("eval.completed", "evl_1", json!({
                "basis": {"asks": ["ask_1"]}, "subject": {},
                "verdict": "aligned", "confidence": 1.0, "artifact": {}, "limitations": []
            })),
            ev("seal.created", "sel_1", json!({
                "basis": {"asks": ["ask_1"]}, "eval": null, "subject": {},
                "disposition": "deferred", "authority": {}, "artifact": {}
            })),
        ]);
        assert_eq!(units[0].sealed.as_deref(), Some("deferred"));
        assert_eq!(units[0].status(), "parked", "deferred reads as parked");
        assert!(!units[0].needs_you());
    }

    #[test]
    fn a_checked_but_unsealed_unit_still_needs_you_even_when_it_matched() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("eval.completed", "evl_1", json!({
                "basis": {"asks": ["ask_1"]}, "subject": {},
                "verdict": "aligned", "confidence": 1.0, "artifact": {}, "limitations": []
            })),
        ]);
        assert!(units[0].needs_you(), "an aligned verdict still wants a decision");
    }

    #[test]
    fn a_cancelled_ask_is_never_waiting_on_you() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("ask.confirmed", "ask_1", json!({"confirmation": {}})),
            ev("ask.cancelled", "ask_1", json!({"cancellation": {}})),
        ]);
        assert_eq!(units[0].status(), "cancelled");
        assert!(!units[0].needs_you());
    }

    #[test]
    fn a_torn_final_line_is_discarded_not_guessed_at() {
        let mut bytes = lines(&[compiled("ask_1", "whole", "codex", "human_handoff")]);
        bytes.extend_from_slice(br#"{"type":"ask.compiled","id":"ask_2","da"#);
        let (events, consumed) = parse_prefix(&bytes);
        assert_eq!(events.len(), 1);
        assert_eq!(project(&events).len(), 1);
        assert!(consumed < bytes.len(), "the partial tail is left for next time");
    }

    #[test]
    fn a_completed_tail_is_picked_up_on_the_next_read() {
        let whole = lines(&[
            compiled("ask_1", "one", "codex", "human_handoff"),
            compiled("ask_2", "two", "codex", "human_handoff"),
        ]);
        let torn = &whole[..whole.len() - 12];
        let (first, consumed) = parse_prefix(torn);
        assert_eq!(first.len(), 1);
        let (rest, _) = parse_prefix(&whole[consumed..]);
        assert_eq!(rest.len(), 1);
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

    #[test]
    fn an_unknown_event_type_contributes_nothing() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("eval.opened", "evl_1", json!({"brief": {}, "basis": {"asks": ["ask_1"]}})),
            ev("seal.refused", "sel_1", json!({"basis": {"asks": ["ask_1"]}, "disposition": "accepted"})),
        ]);
        assert!(units[0].check.is_none(), "an opened Eval is not a verdict");
        assert!(units[0].sealed.is_none(), "a refused Seal is not a decision");
    }
}
