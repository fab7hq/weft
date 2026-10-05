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

/// The session picking work back up resumes: the one it was asked in, when
/// the record names one, else its harness's latest on record.
pub fn resume_session(
    unit: Option<&crate::ledger::Unit>,
    latest: Option<&crate::sessions::Recorded>,
) -> Option<crate::sessions::Recorded> {
    let own = unit.and_then(|u| {
        Some(crate::sessions::Recorded {
            id: u.session_ref.clone()?,
            at: crate::turns::millis(&u.asked_at).unwrap_or_default(),
            last: u.title.clone(),
        })
    });
    own.or_else(|| latest.cloned())
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

/// The judges, their votes and their reasons, as the drawer reads them.
///
/// A rule, not a view: what a record says is the same whoever is showing it.
pub fn judges_read(record: &crate::record::Record, check: &crate::ledger::Check) -> Vec<String> {
    let mut lines = vec![
        format!(
            "  judged by {} · recorded as {}, {:.2}",
            record.judged_by().join(" and "),
            check.verdict.recorded(),
            check.agreement
        ),
        format!("  {} judges checked the work · none of them did it", record.judges.len()),
    ];
    if let Some((judged, total)) = record.windows_judged {
        lines.push(format!("  {judged} of {total} changes judged"));
    }
    if record.fabrications > 0 {
        lines.push(format!(
            "  {} citations a judge gave were not in their source, and were not counted",
            record.fabrications
        ));
    }
    for j in &record.judges {
        let judge = crate::words::word("eval.judge");
        lines.push(format!("  {judge:<11}{} · {} · {}", j.angle, j.host, j.model));
    }
    lines.push(String::new());
    for item in &record.items {
        lines.push(format!("{} {}", mark_for(item.plain_majority()), item.text));
        lines.push(format!("    {} · {}", item.plain_majority(), record.agreed_on(item)));
        for r in &item.reasons {
            lines.push(format!("    {r}"));
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
                Some(c) => format!("  ON THE EVAL   recorded as {verdict}, {c:.2} agreed"),
                None => format!("  ON THE EVAL   recorded as {verdict}"),
            });
        }
        // A Seal may close work no judge looked at. That is allowed, and
        // saying so is the point of reading the receipt.
        _ => lines.push("  ON THE EVAL   none — this was sealed without an Eval".into()),
    }
    if let Some(id) = str_at("seal_id") {
        lines.push(format!("  RECEIPT       {id}"));
    }

    let codes: Vec<&str> = checked
        .get("codes")
        .and_then(serde_json::Value::as_array)
        .map(|a| a.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    let moved = checked.get("subject_matches").and_then(serde_json::Value::as_bool) == Some(false);
    if codes.is_empty() && !moved {
        lines.push("  The receipt still matches what RingFrame wrote.".into());
    } else {
        lines.push("  THE RECEIPT NO LONGER HOLDS".into());
        lines.extend(codes.into_iter().map(|c| format!("  {}", said_plainly(c))));
        if moved {
            lines.push("  the work has moved on since it was sealed".into());
        }
    }
    lines.push("  A Seal records what was decided. It does not make the work right.".into());
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
    format!(
        "ASK {}   EVAL {}   SEAL {}",
        ask_state(unit),
        eval_state(unit),
        act_state(unit.sealed.is_some())
    )
}

/// An Eval in two stages has a state between the two: its change is gathered
/// and its debate has not run.
fn eval_state(unit: &crate::ledger::Unit) -> &'static str {
    if unit.check.is_none() && unit.gathered.is_some() {
        "[GATHERED]"
    } else {
        act_state(unit.check.is_some())
    }
}

/// One pair of words for every act, in the headline and on its section, so a
/// person learns them once. What actually happened is in the section body.
pub fn act_state(done: bool) -> &'static str {
    if done { "[DONE]" } else { "[HAVEN'T RUN]" }
}

/// An Ask asked and not yet composed is under way, not done.
fn ask_state(unit: &crate::ledger::Unit) -> &'static str {
    if unit.requested_only { "[ASKING]" } else { act_state(true) }
}

/// One unit's whole story: the Ask that started it, the Eval that judged it,
/// the Seal that closed it. Three sections in the order they happen, so there
/// is one place to read instead of three keys to find.
///
/// Until the prompt goes, the Ask is the one section. After, each part is
/// `None` when it has not happened, and the section says so rather than being
/// left out — a missing section reads as a gap in the record, and an act that
/// has not happened is not a gap.
pub fn detail_read(
    unit: &crate::ledger::Unit,
    source: Option<&str>,
    prompt: Option<&str>,
    record: Option<&crate::record::Record>,
    seal: Option<&serde_json::Value>,
    debate_in: Option<&str>,
) -> Vec<String> {
    let mut out = vec![detail_headline(unit), format!("  {}", provenance(unit)), String::new()];

    // What the person wrote, then what the harness made of it. A part that is
    // not on disk is said in its place.
    let went = sent(unit);
    out.push(format!("ASK {}", ask_state(unit)));
    match source {
        Some(text) => {
            out.push("  You asked:".into());
            out.extend(text.lines().map(|l| format!("  {l}")));
        }
        None => out.push("  what you asked is not on disk".into()),
    }
    if unit.requested_only {
        out.push(format!("  {} has not composed its prompt yet.", unit.harness));
    } else {
        match prompt {
            Some(text) => {
                let how = if went { "sent" } else { "composed" };
                out.push(format!("  The prompt {} {how}:", unit.harness));
                out.extend(text.lines().map(|l| format!("  {l}")));
            }
            None => out.push("  the prompt is not on disk".into()),
        }
    }
    // Until its prompt goes, the Ask is all there is to read.
    if !went {
        return out;
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
                Some(r) => out.extend(judges_read(r, check)),
                None => out.push("  that check's record is not on disk".into()),
            }
        }
        None => match &unit.gathered {
            Some(g) => {
                out.push(format!("EVAL {}", eval_state(unit)));
                // Who read the change for it: Weft itself, or a harness whose
                // skill ran the gather.
                let by = if g.by == "weft" { "Weft" } else { g.by.as_str() };
                out.push(format!("  gathered by {by}"));
                out.push(format!(
                    "  [E]VAL  debate in {}",
                    debate_in.unwrap_or("the routed harness")
                ));
            }
            None => out.push(format!("EVAL {}", act_state(false))),
        },
    }

    out.push(String::new());
    match seal {
        Some(checked) => {
            out.push(format!(
                "SEAL {}   {}",
                act_state(true),
                unit.seal_state().unwrap_or_else(|| "sealed".into())
            ));
            out.extend(seal_read(unit, checked).into_iter().skip(2));
        }
        None => out.push(format!("SEAL {}", act_state(false))),
    }
    out
}

/// The Details a device draws: the TUI's, in its sections.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Detail {
    pub title: String,
    pub harness: String,
    /// Where its fields came from: asked when, sent how, judged by whom.
    pub provenance: String,
    pub sections: Vec<Section>,
    /// A prompt waits to be sent: Proceed applies.
    pub proceed: bool,
    /// It has an Eval to view.
    pub eval_view: bool,
    /// The Eval as recorded, `drifted, 0.67`, once it is judged.
    pub recorded: Option<String>,
    /// Whether the Seal's receipt still holds, once it is sealed.
    pub holds: Option<bool>,
    /// The harness a gathered change's debate would run in.
    pub debate_in: Option<String>,
    /// The id a follow-up on it carries.
    pub follows: String,
}

/// One act's section: `ask`, `eval` or `seal`, where it stands (`DONE`,
/// `HAVEN'T RUN`, `ASKING`, `GATHERED`), what is said beside that, and its lines.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Section {
    pub act: String,
    pub state: String,
    pub aside: String,
    pub lines: Vec<String>,
    /// Open at first: the Ask until it is sent, else the latest act that
    /// ran, and any section whose record is not on disk.
    pub open: bool,
}

/// [`detail_read`], in sections: the same lines the TUI shows, so the two
/// cannot say different things.
pub fn detail(
    unit: &crate::ledger::Unit,
    source: Option<&str>,
    prompt: Option<&str>,
    record: Option<&crate::record::Record>,
    seal: Option<&serde_json::Value>,
    debate_in: Option<&str>,
) -> Detail {
    let mut sections: Vec<Section> = Vec::new();
    for line in detail_read(unit, source, prompt, record, seal, debate_in).into_iter().skip(3) {
        let heading = ["ASK", "EVAL", "SEAL"].iter().find_map(|act| {
            let (state, aside) = line.strip_prefix(act)?.strip_prefix(" [")?.split_once(']')?;
            Some(Section {
                act: act.to_lowercase(),
                state: state.into(),
                aside: aside.trim().into(),
                lines: Vec::new(),
                open: false,
            })
        });
        match (heading, sections.last_mut()) {
            (Some(s), _) => sections.push(s),
            (None, Some(s)) => s.lines.push(line.strip_prefix("  ").unwrap_or(&line).to_string()),
            (None, None) => {}
        }
    }
    let latest = if unit.sealed.is_some() {
        "seal"
    } else if unit.check.is_some() || unit.gathered.is_some() {
        "eval"
    } else {
        ""
    };
    let missing =
        [(source.is_none() || prompt.is_none() && sent(unit), "ask"), (record.is_none(), "eval")];
    for s in &mut sections {
        while s.lines.last().is_some_and(String::is_empty) {
            s.lines.pop();
        }
        s.open = s.act == latest
            || s.act == "ask" && !sent(unit)
            || s.state == "DONE" && missing.iter().any(|&(gone, act)| gone && act == s.act);
    }
    Detail {
        title: unit.title.clone(),
        harness: unit.harness.clone(),
        provenance: provenance(unit),
        sections,
        proceed: matches!(
            crate::board::next_step(unit),
            Some(crate::board::Next::Send | crate::board::Next::Confirm)
        ),
        eval_view: unit.check.is_some() || unit.gathered.is_some(),
        recorded: unit
            .check
            .as_ref()
            .map(|c| format!("{}, {:.2}", c.verdict.recorded(), c.agreement)),
        holds: seal.map(|c| {
            let codes = c.get("codes").and_then(serde_json::Value::as_array);
            codes.is_none_or(|c| c.is_empty())
                && c.get("subject_matches").and_then(serde_json::Value::as_bool) != Some(false)
        }),
        debate_in: unit.gathered.as_ref().and(debate_in).map(str::to_string),
        follows: crate::board::follow_up_of(unit).to_string(),
    }
}

/// Whether the Ask's prompt went to its agent.
fn sent(unit: &crate::ledger::Unit) -> bool {
    use crate::ledger::Sent;
    !unit.requested_only && !matches!(unit.sent, Sent::NotSent | Sent::ReadyToSend)
}

/// Where a unit's fields came from, in the terms the record uses.
fn provenance(unit: &crate::ledger::Unit) -> String {
    let how = if unit.cancelled && !sent(unit) { "cancelled" } else { unit.sent_phrase() };
    let mut parts = vec![format!("asked {}", unit.asked_at), how.to_string()];
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

    /// The Ask section, as T5.1–T5.4 and T5.10 draw it: what the person
    /// asked, then the prompt the harness composed or sent; each part that is
    /// not on disk is said in its place.
    #[test]
    fn the_ask_says_what_you_asked_then_the_prompt_composed_or_sent() {
        let ask = |u: &Unit, source, prompt| {
            let lines = super::detail_read(u, source, prompt, None, None, None);
            let at = lines.iter().position(|l| l.starts_with("ASK [") && l.len() < 14).unwrap();
            let end = lines[at..].iter().position(String::is_empty).map_or(lines.len(), |n| at + n);
            lines[at..end].to_vec()
        };
        let mut u = sealed("accepted");
        (u.sealed, u.seal_id, u.sealed_at, u.check) = (None, None, None, None);

        u.requested_only = true;
        u.harness = "claude".into();
        assert_eq!(
            ask(&u, Some("Count how often /health is called, per hour"), None),
            [
                "ASK [ASKING]",
                "  You asked:",
                "  Count how often /health is called, per hour",
                "  claude has not composed its prompt yet.",
            ]
        );

        u.requested_only = false;
        u.harness = "codex".into();
        u.sent = Sent::ReadyToSend;
        let source = Some("Read the build number");
        assert_eq!(
            ask(&u, source, Some("Implement /health\nAdd a test")),
            [
                "ASK [DONE]",
                "  You asked:",
                "  Read the build number",
                "  The prompt codex composed:",
                "  Implement /health",
                "  Add a test",
            ]
        );
        u.sent = Sent::Arrived { exact: true };
        assert_eq!(ask(&u, source, Some("Implement /health"))[3], "  The prompt codex sent:");
        assert_eq!(
            ask(&u, None, None),
            ["ASK [DONE]", "  what you asked is not on disk", "  the prompt is not on disk"]
        );
    }

    /// The Seal section, as T5.7 and T5.8 draw it: one block, no gaps.
    #[test]
    fn the_seal_reads_as_one_block() {
        let held = json!({"seal_id": "rcp_7f3a2c", "codes": [], "subject_matches": true,
            "eval": {"verdict": "aligned", "confidence": 1.0}});
        assert_eq!(
            super::seal_read(&sealed("accepted"), &held)[2..],
            [
                "  ON THE EVAL   recorded as aligned, 1.00 agreed",
                "  RECEIPT       rcp_7f3a2c",
                "  The receipt still matches what RingFrame wrote.",
                "  A Seal records what was decided. It does not make the work right.",
            ]
        );
        let moved = json!({"seal_id": "rcp_7f3a2c", "codes": [], "subject_matches": false});
        let lines = super::seal_read(&sealed("accepted"), &moved);
        assert!(
            lines.windows(2).any(|w| w
                == [
                    "  THE RECEIPT NO LONGER HOLDS",
                    "  the work has moved on since it was sealed"
                ]),
            "{lines:?}"
        );
    }

    /// A device's Details are the TUI's lines, in their three sections.
    #[test]
    fn details_are_the_tuis_lines_in_sections() {
        // Waiting for the person's yes: Proceed applies, nothing to view.
        let mut waiting = sealed("accepted");
        (waiting.sealed, waiting.seal_id, waiting.sealed_at) = (None, None, None);
        waiting.sent = Sent::ReadyToSend;
        let d = super::detail(
            &waiting,
            None,
            Some("Refresh the token\nbefore it expires"),
            None,
            None,
            None,
        );
        let heads: Vec<(&str, &str)> =
            d.sections.iter().map(|s| (s.act.as_str(), s.state.as_str())).collect();
        // Until it is sent, the Ask is the one section, and open.
        assert_eq!(heads, [("ask", "DONE")]);
        assert!(d.sections[0].open);
        assert_eq!(
            d.sections[0].lines,
            [
                "what you asked is not on disk",
                "The prompt codex composed:",
                "Refresh the token",
                "before it expires"
            ]
        );
        assert!(d.proceed && !d.eval_view);
        assert_eq!(d.provenance, super::provenance(&waiting));

        // Judged: its section says the verdict beside DONE; nothing to send.
        let mut judged = waiting.clone();
        judged.sent = Sent::Arrived { exact: true };
        judged.check = Some(crate::ledger::Check {
            eval_id: "evl_2".into(),
            verdict: crate::ledger::Verdict::DoesntMatch,
            agreement: 0.67,
            judged_by: Some("codex".into()),
        });
        let d = super::detail(&judged, Some("s"), Some("p"), None, None, None);
        let open: Vec<bool> = d.sections.iter().map(|s| s.open).collect();
        // Sent, the Ask folds; the Eval ran last, and its record is missing.
        assert_eq!(open, [false, true, false]);
        assert_eq!(d.recorded.as_deref(), Some("drifted, 0.67"));
        assert_eq!(d.follows, "evl_2");
        assert_eq!(d.sections[1].state, "DONE");
        assert_eq!(d.sections[1].aside, judged.eval_state().unwrap());
        assert_eq!(d.sections[1].lines, ["that check's record is not on disk"]);
        assert!(!d.proceed && d.eval_view);

        // Sealed: the receipt, as the TUI reads it.
        let checked = json!({"seal_id": "rcp_7f3a2c", "codes": [], "subject_matches": true,
            "eval": {"verdict": "aligned", "confidence": 1.0}});
        let d =
            super::detail(&sealed("accepted"), Some("s"), Some("p"), None, Some(&checked), None);
        assert_eq!(d.holds, Some(true));
        assert!(d.sections[2].open);
        assert_eq!(
            (d.sections[2].state.as_str(), d.sections[2].aside.as_str()),
            ("DONE", "accepted")
        );
        assert!(
            d.sections[2].lines.contains(&"RECEIPT       rcp_7f3a2c".to_string()),
            "{:?}",
            d.sections[2].lines
        );
        assert_eq!(
            d.sections[2].lines.last().unwrap(),
            "A Seal records what was decided. It does not make the work right."
        );
    }

    fn sealed(disposition: &str) -> Unit {
        Unit {
            ask_id: "ask_1".into(),
            title: "health endpoint".into(),
            harness: "codex".into(),
            route: "native_plan".into(),
            asked_at: "2026-09-19T14:02:00Z".into(),
            delivery_mode: "human_handoff".into(),
            confirmed: true,
            sent: Sent::Arrived { exact: true },
            sealed: Some(disposition.into()),
            seal_id: Some("sel_1".into()),
            sealed_at: Some("2026-09-19T16:40:00Z".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_gathered_eval_says_who_gathered_it_and_where_the_debate_goes() {
        let mut u = sealed("accepted");
        u.sealed = None;
        u.seal_id = None;
        u.gathered = Some(crate::ledger::Gathered { eval_id: "evl_1".into(), by: "codex".into() });
        let lines = super::detail_read(&u, None, None, None, None, Some("claude-code"));
        assert!(lines[0].contains("EVAL [GATHERED]"), "{lines:?}");
        let at = lines.iter().position(|l| l == "EVAL [GATHERED]").expect("the section");
        assert_eq!(lines[at + 1], "  gathered by codex");
        assert_eq!(lines[at + 2], "  [E]VAL  debate in claude-code");
        u.gathered = Some(crate::ledger::Gathered { eval_id: "evl_1".into(), by: "weft".into() });
        let lines = super::detail_read(&u, None, None, None, None, Some("claude-code"));
        assert!(lines.iter().any(|l| l == "  gathered by Weft"), "{lines:?}");
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
