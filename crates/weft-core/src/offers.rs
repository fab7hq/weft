//! Sentences and choices the board offers, derived from the record.
//!
//! These are rules — what to offer, and how to say it — so they live here
//! rather than in whatever is drawing. Nothing in this file does anything.

use crate::inject::Handoff;

/// One sentence about a handoff that is not a plain paste, for the panel that
/// asks before typing. Nothing to say when the prompt goes in whole.
pub fn say_handoff(how: &Handoff, harness: &str) -> Option<String> {
    use crate::inject::Handoff;
    match how {
        Handoff::Whole => None,
        Handoff::TypeCommand { command, .. } => Some(format!(
            "{harness} would fold a paste this long and stop reading it for commands, \
             so Weft types {command} and pastes the rest beside it."
        )),
        Handoff::EnterMode { command, active, .. } => Some(format!(
            "{harness} would fold a paste this long and stop reading it for commands, \
             so Weft sends {command} first and waits for \"{active}\" before the prompt."
        )),
    }
}

/// A refusal's first line: RingFrame says what is wrong, then what to do, and
/// a hint has room for the first half.
pub fn first_line(message: &str) -> String {
    let text = message.trim();
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or(text);
    match first.find(". ") {
        Some(at) => first[..=at].trim().to_string(),
        None => first.to_string(),
    }
}

pub fn mark_for(vote: &str) -> &'static str {
    match vote {
        "yes" => "✓",
        "no" => "✗",
        _ => "?",
    }
}

/// What Weft is about to type, and why — for the panel that asks first.
///
/// The words are a rule: two clients asking the same question must ask it the
/// same way. What the bytes *are* is the shell's to work out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asking {
    pub what: String,
    pub why: Vec<String>,
}

/// Sending a compiled prompt back to the harness it was compiled in.
pub fn sending(unit: &crate::ledger::Unit, how: &Handoff) -> Asking {
    let mut why = vec!["This is the exact wording. Weft will not change it.".to_string()];
    if unit.awaiting_yes() {
        why.push(format!(
            "{} asked you about this and never got an answer, so it is still waiting. \
             Sending it records your yes first.",
            unit.harness
        ));
    }
    // Say what the extra step is for, before it happens. It is still one
    // confirmation and the same bytes.
    if let Some(said) = say_handoff(how, &unit.harness) {
        why.push(said);
    }
    Asking { what: format!("Ready to send to {}", unit.harness), why }
}

/// Running Check or Decide, in whichever harness this project routes it to.
pub fn running(skill: &str, command: &str, into: &str, worked_in: &str) -> Asking {
    let mut why =
        vec![crate::words::fill("bar.will_type", &[("command", command), ("harness", into)])];
    if skill == "eval" {
        why.push("Weft has gathered the change. The agent will run RingFrame's".into());
        why.push("judging tasks on it; none of the judges did the work.".into());
    } else {
        why.push("The agent will ask what you decided, and write a receipt.".into());
    }
    // Both acts read the ledger rather than a conversation, so a harness that
    // did none of the work can still do them. When one is, say so: the person
    // should see that the judge is not the worker.
    if into != worked_in {
        why.push(format!("{into} did not do this work. Both acts read the record."));
    }
    Asking {
        what: if skill == "eval" { "Eval this work?".into() } else { "Seal this work?".into() },
        why,
    }
}

/// Asking for something new.
pub fn asking(command: &str, harness: &str) -> Asking {
    Asking {
        what: format!("Ready to ask {harness}"),
        why: vec![
            crate::words::fill("bar.will_type", &[("command", command), ("harness", harness)]),
            crate::words::word("ask.which_approach").into(),
        ],
    }
}

/// A refusal code in the words the person reading it would use.
fn said_plainly(code: &str) -> &str {
    match code {
        "seal.receipt_missing" => "the receipt is not on disk",
        "seal.receipt_tampered" => "the receipt on disk is not the one that was sealed",
        "seal.eval_changed" => "the check it rests on has changed since",
        other => other,
    }
}

/// Whether the Ask's prompt went to its agent.
fn sent(unit: &crate::ledger::Unit) -> bool {
    use crate::ledger::Sent;
    !unit.requested_only && !matches!(unit.sent, Sent::NotSent | Sent::ReadyToSend)
}

// --- Details, one level each (ADR-0029 D3) ------------------------------
//
// A work shows only its Ask; an Eval, a Seal and a session each show once,
// as themselves. weftd's facts in, a payload out: the TUI draws these and a
// device is sent them (remote.md §6c), so the two cannot say different
// things.

use crate::rounds::Record as Rounds;

/// A Seal as a work's breadcrumb or an Eval names it: `{seal, n}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SealRef {
    pub seal: String,
    pub n: usize,
}
use crate::tree::{Ask, Why, Word};

/// A time from the ledger, in milliseconds since the epoch.
fn at(text: &str) -> Option<i64> {
    crate::turns::millis(text)
}

/// A date and time as the Mac reads it: `2026-10-08 11:29`.
pub fn stamp(at: i64, zone: i64) -> String {
    let s = crate::turns::stamp(at + zone);
    format!("{} {}", &s[..10], &s[11..16])
}

/// An Ask as a round lists it: title and harness.
fn listed(units: &[crate::ledger::Unit], ids: &[String]) -> Vec<Ask> {
    ids.iter()
        .filter_map(|id| units.iter().find(|u| &u.ask_id == id))
        .map(|u| Ask {
            unit: u.ask_id.clone(),
            title: u.title.clone(),
            harness: u.harness.clone(),
            word: None,
        })
        .collect()
}

/// Where a work stands, as its breadcrumb says: `› Eval 3 aligned › not
/// sealed`, `› Eval 2 aligned › Seal 4`, or `› not evaluated yet`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Crumb {
    pub eval: Option<CrumbEval>,
    pub seal: Option<SealRef>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CrumbEval {
    pub eval: String,
    pub n: usize,
    /// As recorded; none while it is being judged.
    pub verdict: Option<&'static str>,
}

/// The Ask's own part: `DONE` or `ASKING`, its line, and what was asked,
/// composed and sent.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AskPart {
    pub state: &'static str,
    pub aside: String,
    pub lines: Vec<String>,
}

/// How an Ask's prompt went (T11.2): typed into its harness's pane, word for
/// word or reworded, and when it arrived; else where it stands.
fn how_it_went(unit: &crate::ledger::Unit, zone: i64) -> String {
    let crate::ledger::Sent::Arrived { exact } = unit.sent else {
        return if unit.cancelled { "cancelled".into() } else { unit.sent_phrase().into() };
    };
    let mut line = format!(
        "typed into {}'s pane · {}",
        unit.harness,
        if exact { "word for word" } else { "reworded" }
    );
    if let Some(t) = unit.arrived_at.as_deref().and_then(at) {
        line.push_str(" · ");
        line.push_str(&stamp(t, zone)[11..]);
    }
    line
}

/// T11.2: a work's Details, its Ask only.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct WorkDetail {
    pub unit: String,
    pub title: String,
    pub harness: String,
    pub crumb: Crumb,
    pub ask: AskPart,
    /// The session it was asked in, as AGENTS reads it.
    pub session: Option<crate::tree::Agent>,
    /// A prompt waits to be sent: Proceed applies.
    pub proceed: bool,
    /// The id a follow-up on it carries.
    pub follows: String,
}

pub fn work_detail(
    unit: &crate::ledger::Unit,
    record: &Rounds,
    rounds: &crate::rounds::Rounds,
    session: Option<crate::tree::Agent>,
    source: Option<&str>,
    prompt: Option<&str>,
    zone: i64,
) -> WorkDetail {
    let judging = rounds.judging.iter().find(|r| r.asks.contains(&unit.ask_id));
    let eval = match (judging, &unit.check) {
        (Some(j), _) => Some(CrumbEval { eval: j.eval.clone(), n: j.n, verdict: None }),
        (None, Some(c)) => record.evals.iter().find(|e| e.id == c.eval_id).map(|e| CrumbEval {
            eval: e.id.clone(),
            n: e.n,
            verdict: Some(c.verdict.recorded()),
        }),
        (None, None) => None,
    };
    let seal = unit.seal_id.as_ref().and_then(|id| {
        record.seals.iter().find(|s| &s.id == id).map(|s| SealRef { seal: s.id.clone(), n: s.n })
    });
    let went = sent(unit);
    let mut aside = Vec::new();
    if let Some(t) = at(&unit.asked_at) {
        aside.push(format!("asked {}", stamp(t, zone)));
    }
    aside.push(if went { "sent".to_string() } else { unit.sent_phrase().to_string() });
    if let Some(s) = session.as_ref().filter(|s| s.state == crate::sessions::State::Running)
        && let Some(t) = s.turn
    {
        aside.push(format!("{} is {}", s.harness, t.plain()));
    }
    let mut lines = Vec::new();
    match source {
        Some(text) => {
            lines.push("You asked:".to_string());
            lines.extend(text.lines().map(str::to_string));
        }
        None => lines.push("what you asked is not on disk".into()),
    }
    if unit.requested_only {
        lines.push(String::new());
        lines.push(format!("{} has not composed its prompt yet.", unit.harness));
    } else {
        match prompt {
            // How it went is its own line below (T11.2).
            Some(text) => {
                lines.push(String::new());
                lines.push(format!("The prompt {} composed:", unit.harness));
                lines.extend(text.lines().map(str::to_string));
            }
            None => lines.push("the prompt is not on disk".into()),
        }
        lines.push(String::new());
        lines.push("How it was sent:".into());
        lines.push(how_it_went(unit, zone));
    }
    WorkDetail {
        unit: unit.ask_id.clone(),
        title: unit.title.clone(),
        harness: unit.harness.clone(),
        crumb: Crumb { eval, seal },
        ask: AskPart {
            state: if unit.requested_only { "ASKING" } else { "DONE" },
            aside: aside.join(" · "),
            lines,
        },
        session,
        proceed: matches!(
            crate::board::next_step(unit),
            Some(crate::board::Next::Send | crate::board::Next::Confirm)
        ),
        follows: crate::board::follow_up_of(unit).to_string(),
    }
}

/// A finding or a check, marked ✓, ✗ or ?.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Marked {
    pub mark: &'static str,
    pub text: String,
    /// A check's result: `passed` or `failed`, else as recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

/// T11.3: an Eval, once.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct EvalDetail {
    pub eval: String,
    pub n: usize,
    pub judging: bool,
    pub verdict: Option<&'static str>,
    pub agreement: Option<String>,
    /// Each judging harness and its model, `<harness> · <model>`.
    pub judged_by: Option<String>,
    /// When it was judged, in milliseconds since the epoch.
    pub at: Option<i64>,
    pub findings: Vec<Marked>,
    pub checks: Vec<Marked>,
    pub asks: Vec<Ask>,
    pub sealed: Option<SealRef>,
    /// `None` when Seal can run on it, else why not: the act stays (D12.6).
    pub seal: Option<Why>,
}

pub fn eval_detail(
    eval: &crate::rounds::Eval,
    judges: Option<&crate::record::Record>,
    units: &[crate::ledger::Unit],
    record: &Rounds,
    seal: Option<Why>,
) -> EvalDetail {
    let judged_by = judges.map(|r| {
        let mut by: Vec<String> = Vec::new();
        for j in &r.judges {
            let one = if j.model.is_empty() {
                j.host.clone()
            } else {
                format!("{} · {}", j.host, j.model)
            };
            if !by.contains(&one) {
                by.push(one);
            }
        }
        by.join(" and ")
    });
    let findings = judges
        .map(|r| {
            r.items
                .iter()
                .map(|i| Marked {
                    mark: mark_for(i.plain_majority()),
                    text: i.text.clone(),
                    result: None,
                })
                .collect()
        })
        .unwrap_or_default();
    let checks = judges
        .map(|r| {
            r.checks
                .iter()
                .map(|c| {
                    let (mark, result) = match c.outcome.as_str() {
                        "succeeded" | "passed" => ("✓", "passed".to_string()),
                        "failed" => ("✗", "failed".to_string()),
                        other => ("?", other.to_string()),
                    };
                    Marked { mark, text: c.command.clone(), result: Some(result) }
                })
                .collect()
        })
        .unwrap_or_default();
    EvalDetail {
        eval: eval.id.clone(),
        n: eval.n,
        judging: !eval.completed(),
        verdict: eval.verdict.map(|v| v.recorded()),
        agreement: eval.agreement.map(crate::ledger::score),
        judged_by: judged_by.filter(|b| !b.is_empty()).or_else(|| eval.judged_by.clone()),
        at: eval.completed_at.as_deref().and_then(at),
        findings,
        checks,
        asks: listed(units, &eval.asks),
        sealed: record
            .seals
            .iter()
            .rev()
            .find(|s| s.eval.as_deref() == Some(eval.id.as_str()))
            .map(|s| SealRef { seal: s.id.clone(), n: s.n }),
        seal,
    }
}

/// The Eval a Seal seals, as its Details name it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SealedEval {
    pub eval: String,
    pub n: usize,
    pub verdict: Option<&'static str>,
    pub agreement: Option<String>,
}

/// T11.4: a Seal, once. No act: a Seal is final.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SealDetail {
    pub seal: String,
    pub n: usize,
    /// As a person reads it: accepted, rejected, parked, dropped.
    pub decision: String,
    pub at: Option<i64>,
    pub by: String,
    pub eval: Option<SealedEval>,
    /// The receipt RingFrame checked.
    pub receipt: String,
    /// Whether the receipt still holds, once RingFrame said.
    pub holds: Option<bool>,
    /// What no longer holds, said plainly.
    pub broken: Vec<String>,
    pub asks: Vec<Ask>,
}

pub fn seal_detail(
    seal: &crate::rounds::Seal,
    units: &[crate::ledger::Unit],
    record: &Rounds,
    checked: Option<&serde_json::Value>,
) -> SealDetail {
    let eval = seal.eval.as_deref().and_then(|id| record.evals.iter().find(|e| e.id == id));
    let codes: Vec<String> = checked
        .and_then(|c| c.get("codes"))
        .and_then(serde_json::Value::as_array)
        .map(|a| a.iter().filter_map(|c| c.as_str().map(|c| said_plainly(c).to_string())).collect())
        .unwrap_or_default();
    let moved = checked.and_then(|c| c.get("subject_matches")).and_then(serde_json::Value::as_bool)
        == Some(false);
    let mut broken = codes;
    if moved {
        broken.push("the work has moved on since it was sealed".into());
    }
    let ids: Vec<String> = units
        .iter()
        .filter(|u| u.seal_id.as_deref() == Some(seal.id.as_str()))
        .map(|u| u.ask_id.clone())
        .collect();
    SealDetail {
        seal: seal.id.clone(),
        n: seal.n,
        decision: crate::tree::decision(&seal.disposition),
        at: at(&seal.at),
        by: seal.by.clone(),
        eval: eval.map(|e| SealedEval {
            eval: e.id.clone(),
            n: e.n,
            verdict: e.verdict.map(|v| v.recorded()),
            agreement: e.agreement.map(crate::ledger::score),
        }),
        receipt: checked
            .and_then(|c| c.get("seal_id"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&seal.id)
            .to_string(),
        holds: checked.map(|_| broken.is_empty()),
        broken,
        asks: listed(units, &ids),
    }
}

/// An Ask made in a session, with its state word and where it stands.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SessionAsk {
    pub unit: String,
    pub title: String,
    pub harness: String,
    pub word: Word,
    pub eval: Option<usize>,
    pub seal: Option<usize>,
}

/// T11.5: a session.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SessionDetail {
    pub session: String,
    pub harness: String,
    pub state: crate::sessions::State,
    pub turn: Option<crate::turns::Turn>,
    pub started: i64,
    pub last: i64,
    /// The Weft pane that holds it, while one runs.
    #[serde(skip)]
    pub pane: Option<u32>,
    /// It can be resumed: it held a conversation, and nothing runs it.
    pub resume: bool,
    pub asks: Vec<SessionAsk>,
}

pub fn session_detail(
    row: &crate::sessions::Row,
    units: &[crate::ledger::Unit],
    record: &Rounds,
) -> SessionDetail {
    let mut mine: Vec<&crate::ledger::Unit> =
        units.iter().filter(|u| u.session_ref.as_deref() == Some(row.id.as_str())).collect();
    mine.reverse();
    mine.sort_by(|a, b| b.asked_at.cmp(&a.asked_at));
    let n_of = |id: &str| record.evals.iter().find(|e| e.id == id).map(|e| e.n);
    SessionDetail {
        session: row.id.clone(),
        harness: row.harness.clone(),
        state: row.state,
        turn: row.turn,
        started: row.started,
        last: row.last,
        pane: row.pane,
        resume: row.resumable && row.state != crate::sessions::State::Running,
        asks: mine
            .into_iter()
            .map(|u| SessionAsk {
                unit: u.ask_id.clone(),
                title: u.title.clone(),
                harness: u.harness.clone(),
                word: Word::of(u),
                eval: u.check.as_ref().and_then(|c| n_of(&c.eval_id)),
                seal: u
                    .seal_id
                    .as_ref()
                    .and_then(|id| record.seals.iter().find(|s| &s.id == id).map(|s| s.n)),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    // --- Details, one level each -----------------------------------------

    use super::{
        Crumb, CrumbEval, Marked, SealRef, eval_detail, seal_detail, session_detail, work_detail,
    };
    use crate::rounds::{record as record_of, rounds as rounds_of};
    use crate::sessions::{Row, State};

    fn ev(kind: &str, id: &str, minute: u32, data: serde_json::Value) -> serde_json::Value {
        json!({"type": kind, "id": id, "time": format!("2026-10-08T11:{minute:02}:00Z"),
               "actor": {"kind": "human", "id": "local-user"}, "links": [], "data": data})
    }

    fn compiled(id: &str, title: &str, minute: u32) -> serde_json::Value {
        ev(
            "ask.compiled",
            id,
            minute,
            json!({"title": title, "delivery_mode": "native_dispatch",
            "host": {"name": "claude", "session_ref": "883b9d12", "session_ref_source": "capture"}}),
        )
    }

    /// The cards' sample: fix login open, health endpoint under Eval 3,
    /// readme fix sealed by Seal 4 on Eval 2, an old idea cancelled.
    fn sample() -> Vec<serde_json::Value> {
        let mut seal = ev(
            "seal.created",
            "sel_4",
            40,
            json!({"basis": {"asks": ["ask_r"]}, "disposition": "accepted"}),
        );
        seal["links"] = json!([{"rel": "seals", "id": "ask_r"}, {"rel": "seals", "id": "evl_2"}]);
        vec![
            compiled("ask_r", "readme fix", 1),
            ev("eval.opened", "evl_1", 2, json!({"basis": {"asks": []}})),
            ev("eval.opened", "evl_2", 3, json!({"basis": {"asks": ["ask_r"]}})),
            ev(
                "eval.completed",
                "evl_2",
                4,
                json!({"basis": {"asks": ["ask_r"]}, "verdict": "aligned", "confidence": 1.0}),
            ),
            seal,
            compiled("ask_h", "health endpoint", 5),
            ev("eval.opened", "evl_3", 6, json!({"basis": {"asks": ["ask_h"]}})),
            ev(
                "eval.completed",
                "evl_3",
                7,
                json!({"basis": {"asks": ["ask_h"]}, "verdict": "aligned", "confidence": 0.67}),
            ),
            compiled("ask_f", "fix login", 29),
            compiled("ask_x", "old idea", 30),
            ev("ask.cancelled", "ask_x", 31, json!({})),
        ]
    }

    #[test]
    fn a_works_details_hold_its_ask_under_a_breadcrumb_and_nothing_of_its_round() {
        let events = sample();
        let (units, record) = (crate::ledger::project(&events), record_of(&events));
        let rounds = rounds_of(&units, &record);
        let unit = |id: &str| units.iter().find(|u| u.ask_id == id).unwrap();
        let fix = work_detail(
            unit("ask_f"),
            &record,
            &rounds,
            None,
            Some("Refresh the token"),
            Some("In src/login.rs"),
            0,
        );
        assert_eq!(fix.crumb, Crumb::default(), "not evaluated yet");
        assert_eq!(fix.ask.state, "DONE");
        assert_eq!(fix.ask.aside, "asked 2026-10-08 11:29 · sent");
        assert_eq!(
            fix.ask.lines,
            [
                "You asked:",
                "Refresh the token",
                "",
                "The prompt claude composed:",
                "In src/login.rs",
                "",
                "How it was sent:",
                "the agent took it"
            ],
            "a blank line between its parts (T11.2)"
        );
        let health = work_detail(unit("ask_h"), &record, &rounds, None, None, None, 0);
        assert_eq!(
            health.crumb.eval,
            Some(CrumbEval { eval: "evl_3".into(), n: 3, verdict: Some("aligned") })
        );
        assert_eq!(health.crumb.seal, None, "not sealed");
        let readme = work_detail(unit("ask_r"), &record, &rounds, None, None, None, 7 * 3_600_000);
        assert_eq!((readme.crumb.eval.unwrap().n, readme.crumb.seal.unwrap().n), (2, 1));
        assert_eq!(readme.ask.aside, "asked 2026-10-08 18:01 · sent", "in the Mac's time");
        let v = serde_json::to_value(&fix).unwrap();
        assert!(v.get("eval").is_none() && v.get("sections").is_none(), "no Eval or Seal section");
    }

    #[test]
    fn a_prompt_that_arrived_says_how_it_was_sent_and_when_as_t11_2_does() {
        let typed = |modified: bool| {
            let events = vec![
                ev(
                    "ask.compiled",
                    "ask_f",
                    28,
                    json!({"title": "fix login", "delivery_mode": "human_handoff",
                    "host": {"name": "claude"}}),
                ),
                ev(
                    "ask.submission",
                    "ask_f",
                    29,
                    json!({"state": "observed", "as_modified": modified,
                    "host": {"name": "claude", "session_ref": "883b9d12"}}),
                ),
            ];
            let (units, record) = (crate::ledger::project(&events), record_of(&events));
            let rounds = rounds_of(&units, &record);
            work_detail(&units[0], &record, &rounds, None, None, None, 0).ask.lines.last().cloned()
        };
        assert_eq!(
            typed(false).as_deref(),
            Some("typed into claude's pane · word for word · 11:29")
        );
        assert_eq!(typed(true).as_deref(), Some("typed into claude's pane · reworded · 11:29"));
    }

    #[test]
    fn an_evals_details_say_it_once_with_its_findings_checks_and_asks() {
        let events = sample();
        let (units, record) = (crate::ledger::project(&events), record_of(&events));
        let judges = crate::record::Record::parse(&json!({
            "eval_id": "evl_3", "verdict": "aligned", "confidence": 0.67,
            "judgements": [{"judge": {"angle": "coverage", "host": "codex", "model": "gpt-5.6-terra"}},
                           {"judge": {"angle": "drift", "host": "codex", "model": "gpt-5.6-terra"}}],
            "items": [{"text": "/health returns the build number", "majority": "yes", "agreement": 1.0}],
            "checks": [{"command": "cargo test", "outcome": "succeeded", "seconds": 12}]
        }))
        .unwrap();
        let e = &record.evals[2];
        let d = eval_detail(e, Some(&judges), &units, &record, None);
        assert_eq!((d.n, d.verdict, d.agreement.as_deref()), (3, Some("aligned"), Some("0.67")));
        assert_eq!(d.judged_by.as_deref(), Some("codex · gpt-5.6-terra"));
        assert_eq!(
            d.findings,
            [Marked { mark: "✓", text: "/health returns the build number".into(), result: None }]
        );
        assert_eq!(
            d.checks,
            [Marked { mark: "✓", text: "cargo test".into(), result: Some("passed".into()) }]
        );
        assert_eq!(
            d.asks.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
            ["health endpoint"]
        );
        assert_eq!(d.sealed, None);
        let two = eval_detail(
            &record.evals[1],
            None,
            &units,
            &record,
            crate::rounds::sealed_already(&record, "evl_2"),
        );
        assert_eq!(two.sealed, Some(SealRef { seal: "sel_4".into(), n: 1 }));
        assert_eq!(two.seal.map(|w| w.say()).as_deref(), Some("Eval 2 is sealed already."));
    }

    #[test]
    fn a_seals_details_say_its_decision_its_eval_its_receipt_and_its_asks() {
        let events = sample();
        let (units, record) = (crate::ledger::project(&events), record_of(&events));
        let held = json!({"seal_id": "sel_4", "codes": [], "subject_matches": true});
        let d = seal_detail(&record.seals[0], &units, &record, Some(&held));
        assert_eq!((d.decision.as_str(), d.by.as_str(), d.holds), ("accepted", "you", Some(true)));
        assert_eq!(
            d.eval.as_ref().map(|e| (e.n, e.verdict, e.agreement.as_deref())),
            Some((2, Some("aligned"), Some("1.0")))
        );
        assert_eq!(d.asks.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), ["readme fix"]);
        let moved = json!({"seal_id": "sel_4", "codes": ["seal.receipt_tampered"], "subject_matches": false});
        let d = seal_detail(&record.seals[0], &units, &record, Some(&moved));
        assert_eq!(d.holds, Some(false));
        assert_eq!(
            d.broken,
            [
                "the receipt on disk is not the one that was sealed",
                "the work has moved on since it was sealed"
            ]
        );
        assert_eq!(seal_detail(&record.seals[0], &units, &record, None).holds, None, "not checked");
    }

    #[test]
    fn a_sessions_details_list_its_asks_with_their_words_cancelled_included() {
        let events = sample();
        let (units, record) = (crate::ledger::project(&events), record_of(&events));
        let mut row = Row::of("claude", "883b9d12", State::Ended, None, 5);
        row.started = 1;
        let d = session_detail(&row, &units, &record);
        let asks: Vec<_> =
            d.asks.iter().map(|a| (a.title.as_str(), a.word.say(), a.eval, a.seal)).collect();
        assert_eq!(
            asks,
            [
                ("old idea", "CANCELLED", None, None),
                ("fix login", "ASKED", None, None),
                ("health endpoint", "EVALED", Some(3), None),
                ("readme fix", "SEALED", Some(2), Some(1)),
            ]
        );
        assert!(d.resume, "ended, with a conversation");
        row.state = State::Running;
        assert!(!session_detail(&row, &units, &record).resume, "running: go to it instead");
        row.state = State::Quiet;
        assert!(session_detail(&row, &units, &record).resume, "quiet: resumed after asking");
        row.resumable = false;
        assert!(!session_detail(&row, &units, &record).resume, "only ever ready");
    }
}
