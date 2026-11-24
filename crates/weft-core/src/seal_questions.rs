//! The Seal's questions (cards 17.4, 17.5): one question at a time, each
//! answered before the next, then a review of every answer, and only then the
//! Seal. A question carried from an earlier Seal says so in one quiet line.
//! What the person picks is theirs: nothing is sent until the Seal.

use serde::{Deserialize, Serialize};

use crate::ask::SealAnswer;
use crate::words::{fill, word};

/// Where a question comes from before this Eval: the decision it answers
/// again, and the Seal and day it was made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Before {
    pub decision: String,
    pub seal: String,
    /// `2026-11-10`, as the record writes it.
    pub date: String,
    /// What was answered then, for a decision asked again.
    #[serde(default)]
    pub answer: Option<String>,
    /// Asked again because a file it is about has changed since: those files.
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub changed: Vec<String>,
}

/// A rule of the project a question's behaviour goes against, as RingFrame
/// cites it: `./AGENTS.md@<commit>`, and its words.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionRule {
    pub source: String,
    #[serde(default)]
    pub quote: String,
}

impl QuestionRule {
    /// The rule's file as the person reads it: `AGENTS.md`.
    pub fn file(&self) -> String {
        let path = self.source.rsplit_once('@').map_or(self.source.as_str(), |(p, _)| p);
        path.strip_prefix("./").unwrap_or(path).to_string()
    }
}

/// A question an Eval left for its Seal, as RingFrame stored it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    /// `did_you_want`, `still_want` or `what_did_you_mean`.
    pub ask: String,
    pub behaviour: String,
    #[serde(default)]
    pub why: Option<String>,
    /// Left not sure at an earlier Seal.
    #[serde(default)]
    pub asked_before: Option<Before>,
    /// About a behaviour that goes against an earlier decision.
    #[serde(default)]
    pub against: Option<Before>,
    /// The rules of the project it goes against.
    #[serde(default)]
    pub rules: Vec<QuestionRule>,
}

impl Question {
    /// The answers it takes, in the order they are offered. `words` is the
    /// person's own.
    pub fn answers(&self) -> &'static [&'static str] {
        match self.ask.as_str() {
            "did_you_want" => &["wanted", "not_wanted", "not_sure"],
            "still_want" => &["now", "later", "drop", "not_sure"],
            _ => &["words", "not_sure"],
        }
    }

    /// What it asks: `Did you want this?`, `Still want it?` or `What did you
    /// mean?`.
    pub fn label(&self) -> String {
        let ask = match self.ask.as_str() {
            a @ ("did_you_want" | "still_want") => a,
            _ => "what_did_you_mean",
        };
        word(&format!("seal.q.{ask}")).to_string()
    }

    /// The quiet line above a question asked before: `Asked before · not
    /// sure on 10 Nov`, or `… not wanted on 3 Nov`. None for this Eval's own.
    /// `Asked before · wanted on 3 Nov` for a decision asked again because
    /// its file changed; `Against a rule of this project` for one against a
    /// rule.
    pub fn before_line(&self) -> Option<String> {
        let (key, b) = match (&self.asked_before, &self.against) {
            (Some(b), _) if b.stale => (
                match b.answer.as_deref() {
                    Some("wanted") => "seal.q.before_wanted",
                    Some("not_wanted") => "seal.q.before_not_wanted",
                    _ => "seal.q.before_answered",
                },
                b,
            ),
            (Some(b), _) => ("seal.q.before_not_sure", b),
            (None, Some(b)) => ("seal.q.before_not_wanted", b),
            (None, None) if !self.rules.is_empty() => {
                return Some(word("seal.q.against_rule").to_string());
            }
            (None, None) => return None,
        };
        Some(fill(key, &[("date", &day(&b.date))]))
    }

    /// Why a decision is asked again: `src/session.rs has changed since, so
    /// it’s asked again.` None for any other question.
    pub fn changed_line(&self) -> Option<String> {
        let b = self.asked_before.as_ref().filter(|b| b.stale && !b.changed.is_empty())?;
        let key =
            if b.changed.len() == 1 { "seal.q.changed_since" } else { "seal.q.changed_since_n" };
        Some(fill(key, &[("files", &b.changed.join(", "))]))
    }

    /// The rule it goes against: its file and its words.
    pub fn rule(&self) -> Option<(String, String)> {
        self.rules.first().map(|r| (r.file(), r.quote.clone()))
    }
}

/// `10 Nov` for `2026-11-10`.
pub fn day(date: &str) -> String {
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut parts = date.split('-').skip(1);
    let month = parts.next().and_then(|m| m.parse::<usize>().ok()).filter(|m| (1..=12).contains(m));
    let day = parts.next().and_then(|d| d.parse::<u32>().ok());
    match (month, day) {
        (Some(m), Some(d)) => format!("{d} {}", MONTHS[m - 1]),
        _ => date.to_string(),
    }
}

/// An answer in the words a person reads.
pub fn answer_words(answer: &str) -> String {
    word(&format!("seal.a.{answer}")).to_string()
}

/// The colour an answer is said in, the same wherever it is shown, so the
/// person spots each decision: teal for what they want, amber for what they
/// do not (a decision, never an error), blue for later and for their own
/// words, grey for not sure.
pub fn answer_tone(answer: &str) -> &'static str {
    match answer {
        "wanted" | "now" => "teal",
        "not_wanted" | "drop" => "amber",
        "later" | "words" => "blue",
        _ => "grey",
    }
}

/// The colour a Seal's decision is said in, on the same scale as the
/// answers, cool to hot: teal accepted, blue parked, red rejected, grey
/// dropped. A reason picked takes its decision's colour.
pub fn decision_tone(disposition: &str) -> &'static str {
    match disposition {
        "accepted" => "teal",
        "deferred" => "blue",
        "rejected" => "red",
        _ => "grey",
    }
}

/// One answer given: what, the person's words for `words`, and how long the
/// question was shown before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Given {
    pub answer: String,
    pub words: Option<String>,
    pub ms: u64,
}

/// Where the person is in the Seal's questions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answering {
    pub questions: Vec<Question>,
    pub given: Vec<Option<Given>>,
    /// The question shown, or `questions.len()` for the review.
    pub at: usize,
}

impl Answering {
    pub fn new(questions: Vec<Question>) -> Self {
        let given = vec![None; questions.len()];
        Answering { questions, given, at: 0 }
    }

    /// The question shown, none on the review.
    pub fn shown(&self) -> Option<&Question> {
        self.questions.get(self.at)
    }

    /// `Question 2 of 5`.
    pub fn counter(&self) -> String {
        fill(
            "seal.q.counter",
            &[("n", &(self.at + 1).to_string()), ("of", &self.questions.len().to_string())],
        )
    }

    /// Answer the question shown. `words` needs some; another answer takes
    /// none. A question answered again keeps its new answer.
    pub fn answer(&mut self, answer: &str, words: Option<&str>, ms: u64) -> bool {
        let Some(q) = self.shown() else { return false };
        if !q.answers().contains(&answer) {
            return false;
        }
        let words = words.map(str::trim).filter(|w| !w.is_empty()).map(str::to_string);
        if answer == "words" && words.is_none() {
            return false;
        }
        let words = if answer == "words" { words } else { None };
        self.given[self.at] = Some(Given { answer: answer.into(), words, ms });
        true
    }

    /// On to the next question, or to the review after the last: only once
    /// the one shown has an answer.
    pub fn forward(&mut self) -> bool {
        if self.at >= self.questions.len() || self.given[self.at].is_none() {
            return false;
        }
        self.at += 1;
        true
    }

    pub fn back(&mut self) -> bool {
        if self.at == 0 {
            return false;
        }
        self.at -= 1;
        true
    }

    /// From the review, back to one question to change its answer.
    pub fn change(&mut self, i: usize) {
        if i < self.questions.len() {
            self.at = i;
        }
    }

    /// On the review, with every question answered: the Seal can go. Not
    /// sure counts as an answer.
    pub fn may_seal(&self) -> bool {
        self.at == self.questions.len() && self.given.iter().all(Option::is_some)
    }

    /// What the Seal sends, once it may.
    pub fn answers(&self) -> Option<Vec<SealAnswer>> {
        self.may_seal().then(|| {
            self.questions
                .iter()
                .zip(&self.given)
                .filter_map(|(q, g)| {
                    let g = g.as_ref()?;
                    Some(SealAnswer {
                        question: q.id.clone(),
                        answer: g.answer.clone(),
                        words: g.words.clone(),
                        ms: Some(g.ms),
                    })
                })
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(id: &str, ask: &str) -> Question {
        Question {
            id: id.into(),
            ask: ask.into(),
            behaviour: format!("{id} changed."),
            ..Default::default()
        }
    }

    #[test]
    fn each_question_is_answered_before_the_next_and_the_seal_waits_for_every_answer() {
        let mut a = Answering::new(vec![q("q1", "did_you_want"), q("q2", "what_did_you_mean")]);
        assert_eq!(a.counter(), "Question 1 of 2");
        assert!(!a.forward(), "Next waits for an answer");
        assert!(!a.answer("now", None, 10), "not an answer this question takes");
        assert!(a.answer("not_wanted", None, 4200));
        assert!(a.forward());
        assert!(!a.answer("words", Some("  "), 5), "own words need words");
        assert!(a.answer("not_sure", None, 900), "not sure counts");
        assert!(!a.may_seal(), "not on the review yet");
        assert!(a.forward());
        assert!(a.may_seal());
        // Changed from the review: the new answer stands.
        a.change(1);
        assert!(a.answer("words", Some("only the login form"), 1800));
        assert!(a.forward());
        let sent = a.answers().unwrap();
        assert_eq!(
            sent[0],
            SealAnswer {
                question: "q1".into(),
                answer: "not_wanted".into(),
                words: None,
                ms: Some(4200)
            }
        );
        assert_eq!(
            (sent[1].answer.as_str(), sent[1].words.as_deref()),
            ("words", Some("only the login form"))
        );
        assert!(a.back() && a.back() && !a.back());
    }

    #[test]
    fn a_question_from_an_earlier_seal_says_so_in_one_quiet_line() {
        let before = Before {
            decision: "sel_1/d1".into(),
            seal: "sel_1".into(),
            date: "2026-11-10".into(),
            ..Before::default()
        };
        let carried = Question { asked_before: Some(before.clone()), ..q("q1", "did_you_want") };
        assert_eq!(carried.before_line().unwrap(), "Asked before · not sure on 10 Nov");
        let against = Question {
            against: Some(Before { date: "2026-11-03".into(), ..before }),
            ..q("q2", "did_you_want")
        };
        assert_eq!(against.before_line().unwrap(), "Asked before · not wanted on 3 Nov");
        assert_eq!(q("q3", "still_want").before_line(), None, "this Eval's own");
        assert_eq!(day("not a date"), "not a date");
        assert_eq!(carried.changed_line(), None, "not asked because a file changed");
    }

    #[test]
    fn a_decision_whose_file_changed_is_asked_again_saying_so() {
        let stale = Before {
            decision: "sel_1/d2".into(),
            seal: "sel_1".into(),
            date: "2026-11-03".into(),
            answer: Some("wanted".into()),
            stale: true,
            changed: vec!["src/session.rs".into()],
        };
        let again = Question { asked_before: Some(stale.clone()), ..q("q4", "did_you_want") };
        assert_eq!(again.before_line().unwrap(), "Asked before · wanted on 3 Nov");
        assert_eq!(
            again.changed_line().unwrap(),
            "src/session.rs has changed since, so it’s asked again."
        );
        let two = Before {
            answer: Some("later".into()),
            changed: vec!["a.rs".into(), "b.rs".into()],
            ..stale
        };
        let again = Question { asked_before: Some(two), ..q("q5", "still_want") };
        assert_eq!(again.before_line().unwrap(), "Asked before · answered on 3 Nov");
        assert_eq!(
            again.changed_line().unwrap(),
            "a.rs, b.rs have changed since, so it’s asked again."
        );
    }

    #[test]
    fn a_question_against_a_rule_names_the_rule() {
        let rule = QuestionRule {
            source: "./AGENTS.md@3f2a9c1".into(),
            quote: "Never write a person’s email or password to a log.".into(),
        };
        let asked = Question { rules: vec![rule], ..q("q3", "did_you_want") };
        assert_eq!(asked.before_line().unwrap(), "Against a rule of this project");
        assert_eq!(
            asked.rule().unwrap(),
            ("AGENTS.md".into(), "Never write a person’s email or password to a log.".into())
        );
        assert_eq!(asked.label(), "Did you want this?");
        let parsed: Question = serde_json::from_value(serde_json::json!({
            "id": "q3", "ask": "did_you_want", "behaviour": "b",
            "rules": [{"source": "api/docs/rules.md@abc", "quote": "q", "repo": "api"}]
        }))
        .unwrap();
        assert_eq!(parsed.rule().unwrap().0, "api/docs/rules.md");
    }

    #[test]
    fn every_answer_has_its_words_and_one_colour() {
        for (answer, tone) in [
            ("wanted", "teal"),
            ("now", "teal"),
            ("not_wanted", "amber"),
            ("drop", "amber"),
            ("later", "blue"),
            ("not_sure", "grey"),
            ("words", "blue"),
        ] {
            assert_eq!(answer_tone(answer), tone, "{answer}");
            assert_ne!(answer_tone(answer), "red", "no answer is an error");
            assert!(!answer_words(answer).starts_with("seal.a."), "{answer} has words");
        }
        assert_eq!(q("q", "still_want").answers(), ["now", "later", "drop", "not_sure"]);
    }

    #[test]
    fn every_decision_has_one_colour_on_the_answers_scale() {
        for (disposition, tone) in
            [("accepted", "teal"), ("deferred", "blue"), ("rejected", "red"), ("abandoned", "grey")]
        {
            assert_eq!(decision_tone(disposition), tone, "{disposition}");
        }
    }

    #[test]
    fn each_question_says_what_it_asks() {
        assert_eq!(q("q", "did_you_want").label(), "Did you want this?");
        assert_eq!(q("q", "still_want").label(), "Still want it?");
        assert_eq!(q("q", "what_did_you_mean").label(), "What did you mean?");
    }
}
