//! What a Seal offers after it, and what each offer would type.
//!
//! A Seal records a decision and does nothing more. What follows is offered,
//! never done: each offer has its own dialog. Weft runs no Git itself.

use crate::words::{fill, word};

/// An offer on sealed work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Offer {
    FollowUp,
    Undo,
}

/// What sealed work offers, by what was decided: rejected work may be
/// followed up, or undone when its first Ask recorded the commit it started
/// from. Accepted, parked and dropped work offer nothing.
pub fn offers(disposition: &str, has_base_commit: bool) -> &'static [Offer] {
    match disposition {
        "rejected" if has_base_commit => &[Offer::FollowUp, Offer::Undo],
        "rejected" => &[Offer::FollowUp],
        _ => &[],
    }
}

/// A commit Undo can put the code back to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Commit {
    pub id: String,
    pub short: String,
    pub subject: String,
    /// When it was made: the time today, else the date and time.
    pub when: String,
    /// The commit the work's first Ask was sent on: where it started.
    pub start: bool,
}

/// Whether `id` is a commit id, as Git writes one: 7 to 64 hex digits. Only
/// such an id reaches a Git command line, never one that reads as an option.
pub fn is_commit_id(id: &str) -> bool {
    (7..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The `git log` format `commits` reads.
pub const LOG_FORMAT: &str = "%H%x1f%h%x1f%ct%x1f%s";

/// The commits Undo offers, as `git log` printed them in `LOG_FORMAT`:
/// newest first, down to `base`, where the work started. `now` is in
/// milliseconds; `zone` is the Mac's offset from UTC.
pub fn commits(log: &str, base: &str, now: i64, zone: i64) -> Vec<Commit> {
    let today = &crate::offers::stamp(now, zone)[..10];
    log.lines()
        .filter_map(|line| {
            let [id, short, at, subject] = line.splitn(4, '\u{1f}').collect::<Vec<_>>()[..] else {
                return None;
            };
            let stamp = crate::offers::stamp(at.parse::<i64>().ok()? * 1000, zone);
            let when = if stamp.starts_with(today) { stamp[11..].to_string() } else { stamp };
            Some(Commit {
                id: id.into(),
                short: short.into(),
                subject: subject.into(),
                when,
                start: id == base,
            })
        })
        .collect()
}

/// The words Undo types: the code back as it was at the commit picked.
pub fn undo_request(title: &str, commit: &str) -> String {
    fill("dialog.undo.request", &[("title", title), ("commit", commit)])
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

/// What a follow-up fixes, one line each, from the Eval's report: each
/// requirement not met, then each change nothing asked for (its file, and why
/// as the reading said).
pub fn remedy_lines(view: &crate::eval_view::EvalView) -> Vec<String> {
    let (reqs, units): (Vec<_>, Vec<_>) = view.faults.iter().partition(|f| f.requirement);
    reqs.iter()
        .map(|f| fill("follow_up.not_met", &[("what", f.what.trim())]))
        .chain(units.iter().map(|f| {
            fill("follow_up.unexplained", &[("path", &f.what), ("why", f.why.trim())])
                .trim()
                .to_string()
        }))
        .collect()
}

/// A follow-up's words, written for the person to change: what the Eval
/// found, one line each; then what the person decided at the Seal, word for
/// word.
pub fn remedy_words(findings: &[String], decided: &[String]) -> String {
    let block = |head: &str, lines: &[String]| -> Option<String> {
        (!lines.is_empty()).then(|| {
            let lines: Vec<String> = lines.iter().map(|f| format!("- {}", f.trim())).collect();
            format!("{}\n{}", word(head), lines.join("\n"))
        })
    };
    [block("follow_up.from_findings", findings), block("follow_up.from_decisions", decided)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The person's decisions at a Seal (its receipt's `decisions`), one line
/// each, in their own terms; a question left not sure decides nothing.
pub fn decision_lines(decisions: &[serde_json::Value]) -> Vec<String> {
    decisions
        .iter()
        .filter_map(|d| {
            let text = |k: &str| d[k].as_str().unwrap_or_default().trim().to_string();
            let key = match text("answer").as_str() {
                "wanted" => "follow_up.d.wanted",
                "not_wanted" => "follow_up.d.not_wanted",
                "now" => "follow_up.d.now",
                "later" => "follow_up.d.later",
                "drop" => "follow_up.d.drop",
                "words" => "follow_up.d.words",
                _ => return None,
            };
            Some(fill(key, &[("behaviour", &text("behaviour")), ("words", &text("words"))]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::{Status, UnitOfWork};

    #[test]
    fn only_rejected_work_offers_anything_and_undo_needs_its_commit() {
        assert!(offers("accepted", true).is_empty());
        assert_eq!(offers("rejected", true), [Offer::FollowUp, Offer::Undo]);
        assert_eq!(offers("rejected", false), [Offer::FollowUp], "no commit, no Undo");
        assert!(offers("deferred", true).is_empty());
        assert!(offers("abandoned", true).is_empty());
    }

    #[test]
    fn undo_names_the_commit_keeps_history_and_asks_before_discarding_other_changes() {
        let undo = undo_request("health endpoint", "7ab3d10");
        assert!(undo.contains("\"health endpoint\"") && undo.contains("commit 7ab3d10."), "{undo}");
        assert!(undo.contains("Don't rewrite Git history"), "{undo}");
        assert!(undo.contains("ask me before you discard any change"), "{undo}");
    }

    #[test]
    fn only_a_commit_id_reaches_git() {
        assert!(is_commit_id("4f2a9c1") && is_commit_id(&"a".repeat(40)));
        for not in ["", "4f2a9c", "--output=/tmp/x", "HEAD", "4f2a9c1 ", &"a".repeat(65)] {
            assert!(!is_commit_id(not), "{not:?}");
        }
    }

    #[test]
    fn undo_lists_the_commits_newest_first_down_to_where_the_work_started() {
        let log = "e81b7c2aa\u{1f}e81b7c2\u{1f}1792929480\u{1f}Return 503 while the database is down\n\
                   7ab3d10bb\u{1f}7ab3d10\u{1f}1792928520\u{1f}Add a health endpoint\n\
                   4f2a9c1cc\u{1f}4f2a9c1\u{1f}1792845000\u{1f}Tidy the router\n\
                   not a commit line\n";
        let now = 1_792_929_600_000;
        let c = commits(log, "4f2a9c1cc", now, 0);
        let ids: Vec<&str> = c.iter().map(|c| c.short.as_str()).collect();
        assert_eq!(ids, ["e81b7c2", "7ab3d10", "4f2a9c1"]);
        assert_eq!((c[0].when.as_str(), c[0].start), ("11:58", false));
        assert!(c[2].start, "the base commit is where it started");
        assert_eq!(c[2].when, "2026-10-24 12:30", "another day shows its date");
        assert_eq!(c[2].subject, "Tidy the router");
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
    fn a_follow_up_starts_from_what_the_eval_found_and_what_the_person_decided() {
        let found = ["not met: Return 503".to_string(), "unexplained change: a.rs".to_string()];
        let words = remedy_words(&found, &[]);
        assert_eq!(
            words,
            "Fix what the last Eval found:\n- not met: Return 503\n- unexplained change: a.rs"
        );
        assert_eq!(remedy_words(&[], &[]), "");
        let decided = decision_lines(&[
            serde_json::json!({"answer": "not_wanted", "behaviour": "Usage is sent to a server."}),
            serde_json::json!({"answer": "not_sure", "behaviour": "The theme follows the hour."}),
            serde_json::json!({"answer": "words", "behaviour": "show an error", "words": "a line under the field"}),
            serde_json::json!({"answer": "later", "behaviour": "Sessions last a day."}),
        ]);
        assert_eq!(
            decided,
            [
                "I did not want: Usage is sent to a server.",
                "By “show an error” I meant: a line under the field",
                "Still wanted, later: Sessions last a day.",
            ],
            "word for word, and not sure decides nothing"
        );
        let both = remedy_words(&found[..1], &decided[..1]);
        assert_eq!(
            both,
            "Fix what the last Eval found:\n- not met: Return 503\n\nWhat I decided:\n- I did not want: Usage is sent to a server."
        );
    }

    /// What a follow-up fixes, from the Eval's report: each requirement not
    /// met, then each change nothing asked for (its file and why).
    #[test]
    fn a_follow_up_names_what_was_not_met_and_what_nothing_explains() {
        use crate::eval_view::{EvalView, Fault};
        let fault = |requirement, what: &str, why: &str| Fault {
            requirement,
            what: what.into(),
            why: why.into(),
        };
        let view = EvalView {
            faults: vec![
                fault(false, "src/db/pool.rs", "rewrites the retry loop"),
                fault(true, "Return 503 while the database is down", "no 503 anywhere"),
            ],
            ..EvalView::default()
        };
        assert_eq!(
            remedy_lines(&view),
            [
                "not met: Return 503 while the database is down",
                "unexplained change: src/db/pool.rs rewrites the retry loop",
            ]
        );
        assert!(remedy_lines(&EvalView::default()).is_empty());
    }

    /// The faults a follow-up names are the report's, read from a record.
    #[test]
    fn a_follow_up_reads_the_report_s_faults() {
        let record: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/eval-v4/record.json")).unwrap();
        let view = crate::eval_view::EvalView::of_record("evl_v4", &record);
        assert_eq!(
            remedy_lines(&view),
            [
                "not met: Keep me signed in on this device.",
                "unexplained change: src/auth/session.ts Sessions are cut to an hour.",
            ]
        );
    }
}
