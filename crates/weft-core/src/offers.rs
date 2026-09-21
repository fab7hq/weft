//! Sentences and choices the board offers, derived from the record.
//!
//! ADR-0007: these are rules — what to offer, and how to say it — so they live
//! here rather than in whatever is drawing. Nothing in this file does anything.

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
        what: if skill == "eval" {
            "Check this work?".into()
        } else {
            "Decide on this work?".into()
        },
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
