//! What a Seal offers after it, and what each offer would type.
//!
//! A Seal records a decision and does nothing more. What follows is offered,
//! never done: each offer has its own dialog, which says in a sentence what
//! the work's agent will do. Weft runs no Git itself.

use crate::offers::Asking;
use crate::words::{fill, word};

/// An offer on sealed work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Offer {
    KeepRule,
    FollowUp,
    Undo,
}

/// What sealed work offers, by what was decided: accepted work may keep a
/// rule; rejected work may be followed up, or undone when its first Ask
/// recorded the commit it started from. Parked and dropped work offer
/// nothing.
pub fn offers(disposition: &str, has_base_commit: bool) -> &'static [Offer] {
    match disposition {
        "accepted" => &[Offer::KeepRule],
        "rejected" if has_base_commit => &[Offer::FollowUp, Offer::Undo],
        "rejected" => &[Offer::FollowUp],
        _ => &[],
    }
}

/// The words typed into the work's agent so it drafts one rule from what was
/// asked and the follow-ups, into the file at `path` that Weft reads back.
pub fn rule_request(title: &str, asked: &str, follow_ups: &[String], path: &str) -> String {
    let follow_ups = match follow_ups {
        [] => "- none".to_string(),
        some => some.iter().map(|f| format!("- {}", f.trim())).collect::<Vec<_>>().join("\n"),
    };
    fill(
        "dialog.rule_draft.request",
        &[("title", title), ("path", path), ("asked", asked.trim()), ("follow_ups", &follow_ups)],
    )
}

/// A rule as the agent drafted it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Drafted {
    pub name: String,
    pub rule: String,
    pub when: String,
    pub family: String,
}

/// The draft read from the agent's file: `key: value` lines, any order, any
/// case. A rule is needed; its name, when left out, is its first words.
pub fn drafted(text: &str) -> Option<Drafted> {
    let mut d = Drafted::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim().trim_matches(|c| c == '`' || c == '"').trim().to_string();
        match key.trim().trim_start_matches(['-', '*', ' ']).to_lowercase().as_str() {
            "name" => d.name = value,
            "rule" => d.rule = value,
            "when" => d.when = value,
            "family" => d.family = value.to_lowercase(),
            _ => {}
        }
    }
    if d.rule.is_empty() {
        return None;
    }
    if d.name.is_empty() {
        d.name = d.rule.split_whitespace().take(3).collect::<Vec<_>>().join(" ");
    }
    Some(d)
}

/// The draft once the agent's turn is over: its four lines when it wrote
/// them, else its first line as the rule, so what it wrote is never lost.
pub fn settled(text: &str) -> Option<Drafted> {
    drafted(text).or_else(|| {
        let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
        drafted(&format!("rule: {line}"))
    })
}

/// The words Undo types: the work back as it was at the commit its first
/// Ask was sent on.
pub fn undo_request(title: &str, base_commit: &str) -> String {
    fill("dialog.undo.request", &[("title", title), ("commit", base_commit)])
}

/// The dialog before Undo types: what happens, in a sentence.
pub fn undoing(harness: &str, title: &str) -> Asking {
    Asking {
        what: word("dialog.undo.title").into(),
        why: vec![fill("dialog.undo.why", &[("harness", harness), ("title", title)])],
    }
}

/// What a follow-up on this work links to: rejected work's newest Eval, as
/// the one it remediates; any other work's newest Eval or Ask, as the one it
/// follows.
pub fn follow_up_link(unit: &crate::work::UnitOfWork) -> (&'static str, &str) {
    let rejected =
        matches!(&unit.status, crate::work::Status::Sealed { decision } if decision == "rejected");
    match unit.evals.last() {
        Some(eval) if rejected => ("remediates", eval),
        _ => ("follows", &unit.follows),
    }
}

/// A follow-up's words, written for the person to change: what the Eval
/// found, one line each.
pub fn remedy_words(findings: &[String]) -> String {
    if findings.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = findings.iter().map(|f| format!("- {}", f.trim())).collect();
    format!("{}\n{}", word("follow_up.from_findings"), lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::{Status, UnitOfWork};

    #[test]
    fn accepted_work_keeps_a_rule_and_rejected_work_follows_up_or_undoes_to_its_commit() {
        assert_eq!(offers("accepted", true), [Offer::KeepRule]);
        assert_eq!(offers("rejected", true), [Offer::FollowUp, Offer::Undo]);
        assert_eq!(offers("rejected", false), [Offer::FollowUp], "no commit, no Undo");
        assert!(offers("deferred", true).is_empty());
        assert!(offers("abandoned", true).is_empty());
    }

    #[test]
    fn the_draft_request_names_the_file_and_quotes_what_was_asked_and_each_follow_up() {
        let said = rule_request(
            "health endpoint",
            "Add a health endpoint\n",
            &["Return 503 while the database is down".into()],
            "/w/drafts/sel_1.txt",
        );
        assert!(said.contains("\"health endpoint\""), "{said}");
        assert!(said.contains("Write it to /w/drafts/sel_1.txt"), "{said}");
        assert!(said.contains("What I asked: Add a health endpoint\n"), "{said}");
        assert!(
            said.ends_with("My follow-ups:\n- Return 503 while the database is down"),
            "{said}"
        );
        for key in ["name:", "rule:", "when:", "family:"] {
            assert!(said.contains(key), "{key} in {said}");
        }
        let none = rule_request("t", "a", &[], "/p");
        assert!(none.ends_with("My follow-ups:\n- none"), "{none}");
    }

    #[test]
    fn a_draft_is_read_from_its_lines_and_needs_a_rule() {
        let d = drafted(
            "Here it is:\nName: Degraded health\nrule: `Health checks answer 503 when a dependency is down.`\n\
             when: The work changes a health endpoint.\nfamily: Domain\n",
        )
        .expect("a draft");
        assert_eq!(d.name, "Degraded health");
        assert_eq!(d.rule, "Health checks answer 503 when a dependency is down.");
        assert_eq!(d.when, "The work changes a health endpoint.");
        assert_eq!(d.family, "domain");
        let bare = drafted("rule: Keep diffs small for fixes.").expect("a rule alone");
        assert_eq!(bare.name, "Keep diffs small");
        assert_eq!(drafted("name: x\nwhen: y\n"), None, "no rule, no draft");
    }

    #[test]
    fn once_the_agents_turn_is_over_any_line_it_wrote_is_its_rule() {
        let lines =
            settled("name: Small diffs\nrule: Keep fixes small.\n").expect("the four lines");
        assert_eq!(
            (lines.name.as_str(), lines.rule.as_str()),
            ("Small diffs", "Keep fixes small.")
        );
        let plain = settled("\n  Keep fixes to the lines they need.  \nmore\n").expect("a line");
        assert_eq!(plain.rule, "Keep fixes to the lines they need.");
        assert_eq!(plain.name, "Keep fixes to");
        assert_eq!(settled("  \n\n"), None, "nothing written");
    }

    #[test]
    fn undo_names_the_commit_keeps_history_and_asks_before_discarding_other_changes() {
        let undo = undo_request("health endpoint", "4f2a9c1");
        assert!(undo.contains("\"health endpoint\"") && undo.contains("commit 4f2a9c1"), "{undo}");
        assert!(undo.contains("Don't rewrite Git history"), "{undo}");
        assert!(undo.contains("ask me before you discard any change"), "{undo}");
        let undo = undoing("codex", "health endpoint");
        assert_eq!(undo.what, "Undo this work?");
        assert!(!undo.why[0].contains("commit "), "no commit id is shown: {:?}", undo.why);
        assert!(
            undo.why[0].ends_with("and asks before discarding anything else."),
            "{:?}",
            undo.why
        );
    }

    #[test]
    fn a_follow_up_on_rejected_work_remediates_its_eval_and_any_other_follows() {
        let unit = |status: Status| UnitOfWork {
            root: "ask_1".into(),
            asks: vec!["ask_1".into()],
            open: Vec::new(),
            evals: vec!["evl_1".into(), "evl_2".into()],
            seals: vec!["sel_1".into()],
            status,
            at: String::new(),
            route: None,
            follows: "evl_2".into(),
        };
        let sealed = |d: &str| Status::Sealed { decision: d.into() };
        assert_eq!(follow_up_link(&unit(sealed("rejected"))), ("remediates", "evl_2"));
        assert_eq!(follow_up_link(&unit(sealed("accepted"))), ("follows", "evl_2"));
        assert_eq!(follow_up_link(&unit(Status::Asked)), ("follows", "evl_2"));
        let unjudged =
            UnitOfWork { evals: Vec::new(), follows: "ask_1".into(), ..unit(sealed("rejected")) };
        assert_eq!(follow_up_link(&unjudged), ("follows", "ask_1"), "nothing to remedy");
    }

    #[test]
    fn a_follow_up_starts_from_what_the_eval_found() {
        let words =
            remedy_words(&["not met: Return 503".into(), "unexplained change: a.rs".into()]);
        assert_eq!(
            words,
            "Fix what the last Eval found:\n- not met: Return 503\n- unexplained change: a.rs"
        );
        assert_eq!(remedy_words(&[]), "");
    }
}
