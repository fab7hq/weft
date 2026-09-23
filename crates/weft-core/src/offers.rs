//! Sentences and choices the board offers, derived from the record.
//!
//! These are rules — what to offer, and how to say it — so they live here
//! rather than in whatever is drawing. Nothing in this file does anything.

use crate::inject::Handoff;
use crate::sessions::Recorded;

/// What the panel for an ended agent offers, in the order it offers it.
/// Resuming is first when there is a session on record, because picking up
/// where you were is what you almost always want.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Resume,
    Fresh,
    Close,
}

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

pub fn ended_choices(session: Option<&Recorded>) -> Vec<Ended> {
    match session {
        Some(_) => vec![Ended::Resume, Ended::Fresh, Ended::Close],
        // Nothing to resume is not the same as resuming nothing. Weft offers
        // only what it can name from the record.
        None => vec![Ended::Fresh, Ended::Close],
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
    let mut why = vec![format!("Weft will type  {command}  into {into}.")];
    if skill == "eval" {
        why.push("The agent will look at everything still open and have three".into());
        why.push("judges check it. None of them did the work.".into());
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
            format!("Weft will type  {command}  into {harness}."),
            "The agent will ask you which approach to take, in its own pane.".into(),
        ],
    }
}

/// The judges, their votes and their reasons, as the drawer reads them.
///
/// A rule, not a view: what a record says is the same whoever is showing it.
pub fn judges_read(record: &crate::record::Record, check: &crate::ledger::Check) -> Vec<String> {
    let mut lines = vec![
        format!(
            "judged by {} · recorded as {}, {:.2}",
            record.judged_by().join(" and "),
            check.verdict.recorded(),
            check.agreement
        ),
        format!("{} judges checked the work · none of them did it", record.judges.len()),
        String::new(),
    ];
    for j in &record.judges {
        lines.push(format!("JUDGE      {} · {} · {}", j.angle, j.host, j.model));
    }
    for item in &record.items {
        lines.push(String::new());
        lines.push(format!("{} {}", mark_for(item.plain_majority()), item.text));
        lines.push(format!("  {} · {}", item.plain_majority(), record.agreed_on(item)));
        for r in &item.reasons {
            lines.push(format!("  {r}"));
        }
    }
    if !record.unexplained.is_empty() {
        lines.push(String::new());
        lines.push("CHANGED WITH NO JUDGE ABLE TO TIE IT TO WHAT YOU ASKED".into());
        for p in &record.unexplained {
            lines.push(format!("  {p}"));
        }
    }
    lines.push(String::new());
    lines.push("A check is a judgement, not a guarantee.".into());
    lines
}

/// The Seal that closed the work. RingFrame re-verifies the receipt on every
/// read; Weft says what it found rather than deciding anything itself.
///
/// `checked` is `ringframe seal check --json`. A field it does not carry is
/// left out rather than guessed at.
pub fn seal_read(unit: &crate::ledger::Unit, checked: &serde_json::Value) -> Vec<String> {
    let str_at = |k: &str| checked.get(k).and_then(serde_json::Value::as_str);
    let mut lines = vec![
        format!(
            "{} · sealed {}",
            unit.seal_state().unwrap_or_else(|| "sealed".into()),
            unit.sealed_at.as_deref().unwrap_or("at an hour the record does not give")
        ),
        String::new(),
    ];

    match checked.get("eval") {
        Some(e) if !e.is_null() => {
            let verdict = e.get("verdict").and_then(serde_json::Value::as_str).unwrap_or("?");
            let confidence = e.get("confidence").and_then(serde_json::Value::as_f64);
            lines.push(match confidence {
                Some(c) => format!("ON THE EVAL   recorded as {verdict}, {c:.2} agreed"),
                None => format!("ON THE EVAL   recorded as {verdict}"),
            });
        }
        // A Seal may close work no judge looked at. That is allowed, and
        // saying so is the point of reading the receipt.
        _ => lines.push("ON THE EVAL   none — this was sealed without an Eval".into()),
    }
    if let Some(id) = str_at("seal_id") {
        lines.push(format!("RECEIPT       {id}"));
    }

    lines.push(String::new());
    let codes: Vec<&str> = checked
        .get("codes")
        .and_then(serde_json::Value::as_array)
        .map(|a| a.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    if codes.is_empty() {
        lines.push("The receipt still matches what RingFrame wrote.".into());
    } else {
        lines.push("THE RECEIPT NO LONGER HOLDS".into());
        for c in codes {
            lines.push(format!("  {}", said_plainly(c)));
        }
    }
    if checked.get("subject_matches").and_then(serde_json::Value::as_bool) == Some(false) {
        lines.push("  the work has moved on since it was sealed".into());
    }

    lines.push(String::new());
    lines.push("A Seal records what was decided. It does not make the work right.".into());
    lines
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

/// The headline of a unit's detail view: where it stands, in the vocabulary
/// the sidebar uses.
pub fn detail_headline(unit: &crate::ledger::Unit) -> String {
    let dot = if unit.ask_needs_you() || unit.seal_needs_you() { "● " } else { "" };
    format!(
        "{dot}ASK {}   EVAL {}   SEAL {}",
        act_state(true),
        act_state(unit.check.is_some()),
        act_state(unit.sealed.is_some())
    )
}

/// One pair of words for every act, in the headline and on its section, so a
/// person learns them once. What actually happened is in the section body.
pub fn act_state(done: bool) -> &'static str {
    if done { "[DONE]" } else { "[HAVEN'T RUN]" }
}

/// One unit's whole story: the Ask that started it, the Eval that judged it,
/// the Seal that closed it. Three sections in the order they happen, so there
/// is one place to read instead of three keys to find.
///
/// Each part is `None` when it has not happened, and the section says so
/// rather than being left out — a missing section reads as a gap in the
/// record, and an act that has not happened is not a gap.
pub fn detail_read(
    unit: &crate::ledger::Unit,
    prompt: Option<&str>,
    record: Option<&crate::record::Record>,
    seal: Option<&serde_json::Value>,
) -> Vec<String> {
    let mut out = vec![detail_headline(unit), format!("  {}", provenance(unit)), String::new()];

    out.push(format!("ASK {}", act_state(true)));
    match prompt {
        Some(text) => out.extend(text.lines().map(|l| format!("  {l}"))),
        None => out.push("  the prompt is not on disk".into()),
    }

    out.push(String::new());
    // Whether the Eval ran is the ledger's to say. Whether its record can be
    // read is a separate thing, and a record that will not open is not an act
    // that never happened.
    match &unit.check {
        Some(check) => {
            out.push(format!(
                "EVAL {}   {}",
                act_state(true),
                unit.eval_state().unwrap_or_default()
            ));
            match record {
                Some(r) => out.extend(judges_read(r, check).into_iter().map(indent)),
                None => out.push("  that check's record is not on disk".into()),
            }
        }
        None => out.push(format!("EVAL {}", act_state(false))),
    }

    out.push(String::new());
    match seal {
        Some(checked) => {
            out.push(format!(
                "SEAL {}   {}",
                act_state(true),
                unit.seal_state().unwrap_or_else(|| "sealed".into())
            ));
            out.extend(seal_read(unit, checked).into_iter().skip(2).map(indent));
        }
        None => out.push(format!("SEAL {}", act_state(false))),
    }
    out
}

fn indent(line: String) -> String {
    if line.is_empty() { line } else { format!("  {line}") }
}

/// Where a unit's fields came from, in the terms the record uses.
fn provenance(unit: &crate::ledger::Unit) -> String {
    let mut parts = vec![format!("asked {}", unit.asked_at), unit.sent_phrase().to_string()];
    if let Some(c) = &unit.check {
        parts.push(format!("judged by {}", c.judged_by.clone().unwrap_or_else(|| "?".into())));
    }
    if let Some(at) = &unit.sealed_at {
        parts.push(format!("sealed {at}"));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::ledger::{Sent, Unit};

    fn sealed(disposition: &str) -> Unit {
        Unit {
            ask_id: "ask_1".into(),
            title: "health endpoint".into(),
            harness: "codex".into(),
            session_ref: None,
            delivery: Default::default(),
            route: "native_plan".into(),
            asked_at: "2026-09-19T14:02:00Z".into(),
            delivery_mode: "human_handoff".into(),
            cancelled: false,
            unanswered: false,
            confirmed: true,
            sent: Sent::Arrived { exact: true },
            check: None,
            sealed: Some(disposition.into()),
            seal_id: Some("sel_1".into()),
            sealed_at: Some("2026-09-19T16:40:00Z".into()),
        }
    }

    #[test]
    fn a_receipt_that_still_holds_says_so_and_names_the_check_it_rests_on() {
        let lines = super::seal_read(
            &sealed("accepted"),
            &json!({"seal_id": "sel_1", "fresh": true, "codes": [],
                    "eval": {"eval_id": "evl_1", "verdict": "aligned", "confidence": 0.92},
                    "subject_matches": true}),
        );
        let read = lines.join("\n");
        assert!(read.contains("accepted"), "{read}");
        assert!(read.contains("aligned") && read.contains("0.92"), "{read}");
        assert!(read.contains("still matches"), "{read}");
    }

    #[test]
    fn a_receipt_that_no_longer_holds_says_why_in_words_not_codes() {
        let lines = super::seal_read(
            &sealed("accepted"),
            &json!({"seal_id": "sel_1", "fresh": false,
                    "codes": ["seal.receipt_tampered", "seal.eval_changed"],
                    "eval": null, "subject_matches": false}),
        );
        let read = lines.join("\n");
        assert!(!read.contains("seal.receipt_tampered"), "a code is not a sentence:\n{read}");
        assert!(read.contains("is not the one that was sealed"), "{read}");
        assert!(read.contains("the check it rests on has changed"), "{read}");
        assert!(read.contains("moved on since it was sealed"), "{read}");
        // A Seal may close work no judge looked at, and the reading says so
        // rather than leaving the line out.
        assert!(read.contains("sealed without an Eval"), "{read}");
    }
}
