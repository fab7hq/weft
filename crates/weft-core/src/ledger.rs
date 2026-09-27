//! Reading the RingFrame ledger.
//!
//! Weft reads `ledger.jsonl` directly and never writes it. Direct reads are
//! safe without a lock because the ledger is
//! append-only and each line is written whole: a reader sees a prefix of
//! complete lines, possibly followed by a partial tail, which is discarded.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What the screen says. The recorded
/// term travels alongside so the plain wording is a gloss, never a substitute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub eval_id: String,
    pub verdict: Verdict,
    /// Judge agreement, never correctness.
    pub agreement: f64,
    /// The host that produced the verdict. Always shown; no parity is implied.
    pub judged_by: Option<String>,
}

/// Where a hook saw a prompt arrive: the session it arrived in, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arrival {
    pub host: String,
    pub session: String,
    pub at: String,
}

/// An Eval's first stage, recorded: which Eval, and the harness that mapped it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gathered {
    pub eval_id: String,
    pub by: String,
}

/// The command a prompt carries, and what that command is.
///
/// Both hosts fold a paste past a size and stop reading a folded paste for
/// slash commands, so a prompt that carries `/plan` cannot always be sent in
/// one piece. RingFrame measures and records this; Weft renders the decision
/// and does not work it out again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    /// The command, without its trailing space: `/plan`, `/goal`, `/review`.
    pub mode: Option<String>,
    /// `mode` — it persists once entered, so it can be sent alone and seen.
    /// `inline` — it consumes its argument, so there is nothing to enter.
    pub kind: Option<String>,
    /// What the host shows once the mode is on, for Weft to look for rather
    /// than assume. Only a `mode` has one.
    pub active: Option<String>,
    /// Where the body starts in `prompt.txt`, in bytes.
    pub prefix_bytes: usize,
    /// Whether this host would fold this prompt, which is what makes the
    /// command unreadable in a single paste.
    pub folds: bool,
}

impl Delivery {
    /// Whether the command can be sent on its own first and then confirmed.
    pub fn is_mode(&self) -> bool {
        self.mode.is_some() && self.kind.as_deref() == Some("mode")
    }
}

/// What `ask.compiled` says about how the prompt has to be submitted. An
/// event from before RingFrame recorded this yields the default: no mode.
fn delivery_of(data: &Value) -> Delivery {
    let d = data.get("delivery").cloned().unwrap_or(Value::Null);
    let text = |key: &str| d.get(key).and_then(Value::as_str).map(str::to_string);
    Delivery {
        mode: text("mode"),
        kind: text("mode_kind"),
        active: text("mode_active"),
        prefix_bytes: d.get("prefix_bytes").and_then(Value::as_u64).unwrap_or(0) as usize,
        folds: d.get("folds").and_then(Value::as_bool).unwrap_or(false),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unit {
    pub ask_id: String,
    pub title: String,
    pub harness: String,
    /// The harness's own session, as RingFrame captured it when the Ask was
    /// compiled. This is what `claude --resume` and `codex resume` take, so a
    /// row of work can lead back to the agent that has it. Read, never
    /// invented: a unit whose event carries none offers nothing.
    pub session_ref: Option<String>,
    /// How this prompt has to reach the host, as RingFrame recorded it when
    /// the Ask was compiled. Absent on a record written before RingFrame wrote
    /// it down, which then reads as it always did: no mode, send it whole.
    pub delivery: Delivery,
    pub route: String,
    pub asked_at: String,
    pub delivery_mode: String,
    pub cancelled: bool,
    pub confirmed: bool,
    /// The confirmation surface was shown and gave no answer. Not a refusal:
    /// the Ask is still open, and still the person's to say yes to.
    pub unanswered: bool,
    pub sent: Sent,
    /// Where the prompt was observed arriving, when a hook saw it. The work
    /// goes on in that session, which may not be the one it was asked in.
    #[serde(default)]
    pub arrived: Option<Arrival>,
    /// When the person attested they submitted it — Weft typing it on their
    /// yes — where no hook observed it arrive (`ask.submission` attributed).
    #[serde(default)]
    pub attributed_at: Option<String>,
    pub check: Option<Check>,
    /// An open Eval over this work whose change map is published and whose
    /// debate has not run: the next `[E]VAL` sends the debate.
    #[serde(default)]
    pub gathered: Option<Gathered>,
    pub sealed: Option<String>,
    /// The Seal that closed this, when one has. `sealed` is its disposition;
    /// these are the record it came from, so a row can lead to the receipt.
    pub seal_id: Option<String>,
    pub sealed_at: Option<String>,
    /// Asked, and nothing composed yet: RingFrame recorded the person's
    /// words (`ask.requested`) and no compile has followed. ASKING, not
    /// ASKED.
    #[serde(default)]
    pub requested_only: bool,
}

impl Unit {
    /// The one-line status the list shows. Plain words only.
    pub fn status(&self) -> String {
        if self.cancelled {
            return "cancelled".into();
        }
        if let Some(word) = self.seal_state() {
            return word;
        }
        if let Some(c) = &self.check {
            return c.verdict.plain().into();
        }
        self.sent_phrase().into()
    }

    /// Where each of RingFrame's three acts stands. The list shows one line
    /// per act, because one collapsed status cannot say both that the Ask
    /// arrived word for word and that no Eval has run on it yet.
    ///
    /// `None` means the act has not happened. That is a fact about the work,
    /// not a gap in the record, so the row says so rather than leaving a hole.
    pub fn ask_state(&self) -> &'static str {
        if self.cancelled { "cancelled" } else { self.sent_phrase() }
    }

    pub fn eval_state(&self) -> Option<String> {
        self.check.as_ref().map(|c| format!("{}, {:.2} agreed", c.verdict.plain(), c.agreement))
    }

    pub fn seal_state(&self) -> Option<String> {
        self.sealed.as_deref().map(|d| match d {
            "deferred" => "parked".to_string(),
            "abandoned" => "dropped".to_string(),
            // `accepted` and `rejected` are already the words a person reads,
            // and anything else is shown as the record wrote it.
            other => other.to_string(),
        })
    }

    /// Where the Ask itself stands, whatever has happened to it since. The
    /// status collapses into this once there is no verdict and no seal.
    pub fn sent_phrase(&self) -> &'static str {
        if self.requested_only {
            return "asked, still being composed";
        }
        match self.sent {
            // An Ask nobody answered is not an Ask nobody wants. It says what
            // is missing — a yes — rather than that nothing has happened.
            Sent::NotSent if self.unanswered => "waiting for your yes",
            Sent::NotSent => "not sent yet",
            Sent::ReadyToSend => "ready to send",
            Sent::Unconfirmed => "sent, unconfirmed",
            Sent::TakenByAgent => "the agent took it",
            Sent::Arrived { exact: true } => "sent · word for word",
            Sent::Arrived { exact: false } => "sent, reworded",
        }
    }

    /// Compiled, asked about, and never answered. The candidate is on disk and
    /// the only thing missing is the person saying yes.
    pub fn awaiting_yes(&self) -> bool {
        self.unanswered && !self.confirmed && !self.cancelled && self.sent == Sent::NotSent
    }

    /// Not sealed and not cancelled: what `OPEN` counts.
    pub fn is_open(&self) -> bool {
        self.sealed.is_none() && !self.cancelled
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
    // `eval.gathered` names only its Eval, so each Eval's Asks are kept from
    // `eval.opened`.
    let mut opened: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    for e in events {
        let kind = s(e, &["type"]).unwrap_or_default();
        let id = s(e, &["id"]).unwrap_or_default().to_string();
        let data = e.get("data").cloned().unwrap_or(Value::Null);

        match kind {
            // Asked, before anything is composed: on the list at once, so an
            // Ask whose composing never finished is not lost.
            "ask.requested" => {
                if index.contains_key(&id) {
                    continue;
                }
                index.insert(id.clone(), units.len());
                units.push(Unit {
                    ask_id: id,
                    title: s(&data, &["title"]).unwrap_or("untitled").to_string(),
                    harness: s(&data, &["host", "name"]).unwrap_or("unknown").to_string(),
                    delivery: Delivery::default(),
                    // Captured by the prompt hook, from the host itself.
                    session_ref: s(&data, &["host", "session_ref"]).map(str::to_string),
                    route: String::new(),
                    asked_at: s(e, &["time"]).unwrap_or_default().to_string(),
                    delivery_mode: String::new(),
                    cancelled: false,
                    confirmed: false,
                    unanswered: false,
                    sent: Sent::NotSent,
                    arrived: None,
                    attributed_at: None,
                    check: None,
                    gathered: None,
                    sealed: None,
                    seal_id: None,
                    sealed_at: None,
                    requested_only: true,
                });
            }
            "ask.compiled" => {
                let mode = s(&data, &["delivery_mode"]).unwrap_or_default().to_string();
                // A dispatch route has nothing for the person to send.
                let sent =
                    if mode == "native_dispatch" { Sent::TakenByAgent } else { Sent::NotSent };
                // The compile of an Ask already on the list as asked takes its
                // place there, and keeps when it was asked.
                let asked = index.get(&id).map(|i| units[*i].asked_at.clone());
                let at = *index.entry(id.clone()).or_insert(units.len());
                let unit = Unit {
                    ask_id: id,
                    title: s(&data, &["title"]).unwrap_or("untitled").to_string(),
                    harness: s(&data, &["host", "name"]).unwrap_or("unknown").to_string(),
                    delivery: delivery_of(&data),
                    // Only a captured reference. `session_ref_source` says how
                    // RingFrame came by it, and anything it did not capture
                    // from the host itself is not a session Weft may reopen.
                    session_ref: (s(&data, &["host", "session_ref_source"]) == Some("capture"))
                        .then(|| s(&data, &["host", "session_ref"]).map(str::to_string))
                        .flatten(),
                    route: s(&data, &["selected_capability"]).unwrap_or_default().to_string(),
                    asked_at: asked
                        .unwrap_or_else(|| s(e, &["time"]).unwrap_or_default().to_string()),
                    delivery_mode: mode,
                    cancelled: false,
                    confirmed: false,
                    unanswered: false,
                    sent,
                    arrived: None,
                    attributed_at: None,
                    check: None,
                    gathered: None,
                    sealed: None,
                    seal_id: None,
                    sealed_at: None,
                    requested_only: false,
                };
                if at == units.len() {
                    units.push(unit);
                } else {
                    units[at] = unit;
                }
            }
            "ask.unanswered" => {
                if let Some(u) = index.get(&id).and_then(|i| units.get_mut(*i)) {
                    u.unanswered = true;
                }
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
                    if s(&data, &["state"]) == Some("attributed") {
                        // The person's word that it was sent: it went, but no
                        // hook saw it arrive, so a receipt, in either order,
                        // outranks it.
                        u.attributed_at = s(e, &["time"]).map(str::to_string);
                        if !matches!(u.sent, Sent::Arrived { .. }) {
                            u.sent = Sent::Unconfirmed;
                        }
                        continue;
                    }
                    u.sent = Sent::Arrived { exact };
                    if s(&data, &["state"]) == Some("observed")
                        && let (Some(host), Some(session)) =
                            (s(&data, &["host", "name"]), s(&data, &["host", "session_ref"]))
                    {
                        u.arrived = Some(Arrival {
                            host: host.into(),
                            session: session.into(),
                            at: s(e, &["time"]).unwrap_or_default().into(),
                        });
                    }
                }
            }
            "eval.opened" => {
                opened.insert(id, basis_asks(&data));
            }
            "eval.gathered" => {
                let gathered =
                    Gathered { eval_id: id.clone(), by: s(&data, &["host"]).unwrap_or("?").into() };
                for ask in opened.get(&id).into_iter().flatten() {
                    if let Some(u) = index.get(ask).and_then(|i| units.get_mut(*i)) {
                        u.gathered = Some(gathered.clone());
                    }
                }
            }
            "eval.completed" => {
                for ask in basis_asks(&data) {
                    if let Some(u) = index.get(&ask).and_then(|i| units.get_mut(*i))
                        && u.gathered.as_ref().is_some_and(|g| g.eval_id == id)
                    {
                        u.gathered = None;
                    }
                }
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
                let at = s(e, &["time"]).unwrap_or_default().to_string();
                for ask in basis_asks(&data) {
                    if let Some(u) = index.get(&ask).and_then(|i| units.get_mut(*i)) {
                        u.sealed = Some(disposition.clone());
                        u.seal_id = Some(id.clone());
                        u.sealed_at = Some(at.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Next;
    use serde_json::json;

    fn requested(id: &str, title: &str) -> Value {
        json!({
            "schema": "ringframe.ledger/1", "event_id": "evt_0", "type": "ask.requested",
            "time": "2026-09-19T14:01:00Z", "id": id,
            "actor": {"kind": "human", "id": "local-user"}, "links": [],
            "data": {"title": title, "captured_by": "prompt_hook",
                     "source": {"role": "source_intent", "path": "x", "bytes": 2, "sha256": "a"},
                     "host": {"name": "codex", "session_ref": "01a0"}}
        })
    }

    /// An Ask is on the list the moment it is asked: ASKING, with the
    /// person's words as its title, until its compile takes its place and
    /// keeps when it was asked.
    #[test]
    fn an_ask_is_on_the_list_as_soon_as_it_is_asked_and_its_compile_takes_its_place() {
        let asked = project(&[requested("ask_1", "fix the login bug")]);
        assert_eq!(asked.len(), 1);
        let u = &asked[0];
        assert!(u.requested_only && u.is_open());
        assert_eq!((u.title.as_str(), u.harness.as_str()), ("fix the login bug", "codex"));
        assert_eq!(u.status(), "asked, still being composed");
        assert_eq!(crate::board::next_step(u), None, "nothing to send yet");

        let both = project(&[
            requested("ask_0", "another"),
            requested("ask_1", "fix the login bug"),
            compiled("ask_1", "Login fix", "codex", "human_handoff"),
        ]);
        assert_eq!(both.len(), 2, "one unit per Ask");
        let u = &both[1];
        assert!(!u.requested_only);
        assert_eq!(u.title, "Login fix");
        assert_eq!(u.asked_at, "2026-09-19T14:01:00Z", "when it was asked");
        assert_eq!(u.status(), "not sent yet");
    }

    fn compiled(id: &str, title: &str, host: &str, mode: &str) -> Value {
        json!({
            "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.compiled",
            "time": "2026-09-19T14:02:00Z", "id": id,
            "actor": {"kind": "human", "id": "local-user"}, "links": [],
            "data": {
                "title": title,
                "selected_capability": "native_plan",
                "delivery_mode": mode,
                "host": {"name": host, "profile_id": host,
                         "session_ref": "01a0bdb6-1d1f", "session_ref_source": "capture"},
                "source": {"role": "source_intent", "path": "x", "bytes": 2, "sha256": "a"},
                "prompt": {"role": "generated_prompt", "path": "y", "bytes": 9, "sha256": "b"},
                "source_verified": "exact", "limitations": [],
                "classification": {}, "route_explanation": {}
            }
        })
    }

    #[test]
    fn a_gathered_eval_marks_its_work_until_the_debate_closes_it() {
        let opened = ev("eval.opened", "evl_1", json!({"brief": {}, "basis": {"asks": ["ask_1"]}}));
        let gathered = ev(
            "eval.gathered",
            "evl_1",
            json!({"eval_id": "evl_1", "host": "codex", "context_map": {}}),
        );
        let mut events =
            vec![compiled("ask_1", "t", "claude-code", "native_dispatch"), opened, gathered];
        let u = &project(&events)[0];
        assert_eq!(u.gathered, Some(Gathered { eval_id: "evl_1".into(), by: "codex".into() }));
        events.push(ev(
            "eval.completed",
            "evl_1",
            json!({"verdict": "aligned", "confidence": 1.0, "basis": {"asks": ["ask_1"]}}),
        ));
        let u = &project(&events)[0];
        assert_eq!(u.gathered, None);
        assert!(u.check.is_some());
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
        assert_eq!(crate::board::next_step(&units[0]), Some(Next::Send));
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
        assert_ne!(
            crate::board::next_step(&units[0]),
            Some(Next::Send),
            "a dispatch route has nothing to send"
        );
    }

    #[test]
    fn a_handoff_that_is_ready_is_the_thing_that_waits_on_you() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev(
                "ask.delivery",
                "ask_1",
                json!({
                    "mode": "human_handoff", "mechanism": null, "state": "handoff_ready",
                    "qualification": null, "receipt": null, "submission": "unobserved",
                    "limitations": []
                }),
            ),
        ]);
        assert_eq!(units[0].sent, Sent::ReadyToSend);
        assert_eq!(crate::board::next_step(&units[0]), Some(Next::Send));
    }

    #[test]
    fn a_receipt_outranks_the_route_it_arrived_on() {
        let units = project(&[
            compiled("ask_1", "t", "claude-code", "native_dispatch"),
            ev(
                "ask.submission",
                "ask_1",
                json!({
                    "state": "observed", "observed_by": "hook:UserPromptSubmit",
                    "attributed_by": null, "as_modified": false,
                    "host": {"name": "claude-code"}, "prompt_sha256": "b"
                }),
            ),
        ]);
        assert_eq!(units[0].status(), "sent · word for word");
    }

    #[test]
    fn a_hook_receipt_is_what_makes_it_sent_word_for_word() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("ask.confirmed", "ask_1", json!({"confirmation": {}})),
            ev(
                "ask.submission",
                "ask_1",
                json!({
                    "state": "observed", "observed_by": "hook:UserPromptSubmit",
                    "attributed_by": null, "as_modified": false,
                    "host": {"name": "codex"}, "prompt_sha256": "b"
                }),
            ),
        ]);
        assert_eq!(units[0].sent, Sent::Arrived { exact: true });
        assert_eq!(units[0].status(), "sent · word for word");
    }

    #[test]
    fn a_reworded_submission_says_so() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev(
                "ask.submission",
                "ask_1",
                json!({
                    "state": "observed", "observed_by": "hook:UserPromptSubmit",
                    "attributed_by": null, "as_modified": true,
                    "host": {"name": "codex"}, "prompt_sha256": "zz"
                }),
            ),
        ]);
        assert_eq!(units[0].status(), "sent, reworded");
    }

    #[test]
    fn a_verdict_is_shown_in_plain_words_and_keeps_its_recorded_term() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev(
                "eval.completed",
                "evl_1",
                json!({
                    "basis": {"asks": ["ask_1"]}, "subject": {},
                    "verdict": "drifted", "confidence": 0.67,
                    "artifact": {}, "limitations": []
                }),
            ),
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
            ev(
                "eval.completed",
                "evl_1",
                json!({
                    "basis": {"asks": ["ask_1", "ask_2"]}, "subject": {},
                    "verdict": "aligned", "confidence": 1.0, "artifact": {}, "limitations": []
                }),
            ),
        ]);
        assert!(units.iter().all(|u| u.check.is_some()));
    }

    #[test]
    fn a_seal_closes_the_unit_and_nothing_needs_you_afterwards() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev(
                "eval.completed",
                "evl_1",
                json!({
                    "basis": {"asks": ["ask_1"]}, "subject": {},
                    "verdict": "aligned", "confidence": 1.0, "artifact": {}, "limitations": []
                }),
            ),
            ev(
                "seal.created",
                "sel_1",
                json!({
                    "basis": {"asks": ["ask_1"]}, "eval": null, "subject": {},
                    "disposition": "deferred", "authority": {}, "artifact": {}
                }),
            ),
        ]);
        assert_eq!(units[0].sealed.as_deref(), Some("deferred"));
        assert_eq!(units[0].status(), "parked", "deferred reads as parked");
        assert_eq!(crate::board::next_step(&units[0]), None);
    }

    #[test]
    fn the_three_acts_each_say_where_they_stand() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("ask.confirmed", "ask_1", json!({"confirmation": {}})),
        ]);
        let u = &units[0];
        // An act that has not happened says so. A blank would read as a gap
        // in the record rather than as work still to do.
        assert_eq!(u.ask_state(), "ready to send");
        assert_eq!(u.eval_state(), None);
        assert_eq!(u.seal_state(), None);
    }

    #[test]
    fn a_seal_says_which_receipt_closed_the_work() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev(
                "seal.created",
                "sel_1",
                json!({
                    "basis": {"asks": ["ask_1"]}, "eval": null, "subject": {},
                    "disposition": "accepted", "authority": {}, "artifact": {}
                }),
            ),
        ]);
        // Without the id the row can say a Seal happened but never lead to it.
        assert_eq!(units[0].seal_id.as_deref(), Some("sel_1"));
        assert!(units[0].sealed_at.is_some(), "and when, so the reading is dated");
        assert_eq!(units[0].seal_state().as_deref(), Some("accepted"));
    }

    #[test]
    fn a_checked_but_unsealed_unit_still_needs_you_even_when_it_matched() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev(
                "eval.completed",
                "evl_1",
                json!({
                    "basis": {"asks": ["ask_1"]}, "subject": {},
                    "verdict": "aligned", "confidence": 1.0, "artifact": {}, "limitations": []
                }),
            ),
        ]);
        assert_eq!(
            crate::board::next_step(&units[0]),
            Some(Next::Seal),
            "an aligned verdict still wants a decision"
        );
    }

    #[test]
    fn a_cancelled_ask_is_never_waiting_on_you() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("ask.confirmed", "ask_1", json!({"confirmation": {}})),
            ev("ask.cancelled", "ask_1", json!({"cancellation": {}})),
        ]);
        assert_eq!(units[0].status(), "cancelled");
        assert_eq!(crate::board::next_step(&units[0]), None);
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
    fn an_unknown_event_type_contributes_nothing() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("eval.opened", "evl_1", json!({"brief": {}, "basis": {"asks": ["ask_1"]}})),
            ev(
                "seal.refused",
                "sel_1",
                json!({"basis": {"asks": ["ask_1"]}, "disposition": "accepted"}),
            ),
        ]);
        assert!(units[0].check.is_none(), "an opened Eval is not a verdict");
        assert!(units[0].sealed.is_none(), "a refused Seal is not a decision");
    }

    #[test]
    fn a_unit_carries_the_session_the_ask_was_made_in() {
        // RingFrame captures the host's own session id on every Ask, so the
        // row of work leads back to the agent that has it. Weft reads it and
        // never invents one.
        let units = project(&[compiled("ask_1", "health endpoint", "codex", "native_plan")]);
        assert_eq!(units[0].session_ref.as_deref(), Some("01a0bdb6-1d1f"));
    }

    #[test]
    fn only_a_captured_session_reference_counts() {
        // `session_ref_source` says how RingFrame came by the id. Anything it
        // did not capture from the host is not a session Weft may reopen.
        let mut event = compiled("ask_1", "health endpoint", "codex", "native_plan");
        event["data"]["host"]["session_ref_source"] = json!("declared");
        assert_eq!(project(&[event]).swap_remove(0).session_ref, None);

        let mut missing = compiled("ask_2", "health endpoint", "codex", "native_plan");
        missing["data"]["host"] = json!({"name": "codex"});
        assert_eq!(project(&[missing]).swap_remove(0).session_ref, None);
    }

    /// A submission the person attests to — Weft typing it on their yes,
    /// where no hook could observe it — reads as sent, unconfirmed; one a hook
    /// observed wins, in either order.
    #[test]
    fn an_attributed_submission_is_sent_and_an_observed_one_wins() {
        let at = |kind: &str, state: &str, t: &str| {
            json!({"type": kind, "id": "ask_1", "time": t,
                   "data": {"state": state, "as_modified": false,
                            "host": {"name": "codex", "session_ref": "c1"}}})
        };
        let base = || {
            vec![
                compiled("ask_1", "t", "codex", "human_handoff"),
                ev("ask.confirmed", "ask_1", json!({})),
            ]
        };
        let mut attributed = base();
        attributed.push(at("ask.submission", "attributed", "2026-09-19T14:04:00Z"));
        let u = &project(&attributed)[0];
        assert_eq!(u.sent, Sent::Unconfirmed);
        assert_eq!(u.attributed_at.as_deref(), Some("2026-09-19T14:04:00Z"));
        assert_eq!(u.sent_phrase(), "sent, unconfirmed");
        let mut then_observed = attributed.clone();
        then_observed.push(at("ask.submission", "observed", "2026-09-19T14:04:01Z"));
        assert_eq!(project(&then_observed)[0].sent, Sent::Arrived { exact: true });
        let mut observed_first = base();
        observed_first.push(at("ask.submission", "observed", "2026-09-19T14:04:00Z"));
        observed_first.push(at("ask.submission", "attributed", "2026-09-19T14:04:01Z"));
        assert_eq!(project(&observed_first)[0].sent, Sent::Arrived { exact: true });
    }

    #[test]
    fn an_ask_nobody_answered_still_needs_you() {
        // Codex's chooser returns the same empty result whether it was
        // dismissed or timed out, so RingFrame records that no answer came
        // rather than a refusal. The candidate is on disk and the only thing
        // missing is the person's yes.
        let units = project(&[
            compiled("ask_1", "health endpoint", "codex", "human_handoff"),
            ev("ask.unanswered", "ask_1", json!({"unanswered": {"observed_by": "skill"}})),
        ]);
        let u = &units[0];
        assert!(u.unanswered && !u.cancelled && !u.confirmed);
        assert!(u.awaiting_yes(), "still the person's to say yes to");
        assert_eq!(crate::board::next_step(u), Some(Next::Confirm), "and it is waiting on them");
        assert_eq!(u.status(), "waiting for your yes");
    }

    #[test]
    fn saying_yes_afterwards_makes_it_ready_to_send() {
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("ask.unanswered", "ask_1", json!({"unanswered": {"observed_by": "skill"}})),
            ev("ask.confirmed", "ask_1", json!({"confirmation": {"observed_by": "skill"}})),
        ]);
        assert!(!units[0].awaiting_yes(), "answered now");
        assert_eq!(units[0].sent, Sent::ReadyToSend);
        assert_eq!(units[0].status(), "ready to send");
    }

    #[test]
    fn a_cancelled_ask_is_not_waiting_for_anything() {
        // Unanswered is not a softer cancel: an actual no still ends it, and
        // an Ask cancelled after going unanswered stays ended.
        let units = project(&[
            compiled("ask_1", "t", "codex", "human_handoff"),
            ev("ask.unanswered", "ask_1", json!({"unanswered": {"observed_by": "skill"}})),
            ev("ask.cancelled", "ask_1", json!({"cancellation": {"observed_by": "skill"}})),
        ]);
        assert!(!units[0].awaiting_yes());
        assert_eq!(crate::board::next_step(&units[0]), None);
        assert_eq!(units[0].status(), "cancelled");
    }

    #[test]
    fn a_record_with_no_unanswered_event_reads_as_it_always_did() {
        let units = project(&[compiled("ask_1", "t", "codex", "human_handoff")]);
        assert!(!units[0].unanswered && !units[0].awaiting_yes());
        assert_eq!(units[0].status(), "not sent yet");
    }
}
