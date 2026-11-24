//! The person's decisions, as a project's settings list them (card 9.15):
//! each answer given at a Seal, newest first, in its answer's colour, with
//! its day and the files it is about. A decision whose file has changed since
//! says so: the next Eval that touches that file asks it again. Nothing here
//! acts; the list only shows.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::seal_questions::{answer_tone, day};
use crate::words::{fill, word};

/// One decision, as its row says it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub behaviour: String,
    /// `wanted`, `not_wanted`, `now`, `later`, `drop`, `words` or `not_sure`.
    pub answer: String,
    /// `Wanted`, `Not wanted`, …
    pub answer_words: String,
    /// `teal`, `amber`, `blue` or `grey`, as the Seal's answers.
    pub tone: String,
    /// `3 Nov`.
    pub day: String,
    /// The files it is about: `src/session.rs`, `api/AGENTS.md`.
    pub files: Vec<String>,
    /// A file it is about has changed since.
    pub stale: bool,
    /// `src/session.rs has changed since; the next Eval that touches it asks
    /// again`, when stale.
    #[serde(default)]
    pub stale_line: Option<String>,
}

/// The section: its title, how many, and the decisions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decisions {
    /// `YOUR DECISIONS`.
    pub title: String,
    /// `4 · 1 to ask again`, or `4`.
    pub count: String,
    /// `changed since`: the mark a stale one carries.
    pub changed: String,
    pub list: Vec<Decision>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// A pin's file as the person reads it: its path, under its repository's
/// name when that is not the project's own.
fn file_of(pin: &Value) -> String {
    let (repo, path) = (s(pin, "repo"), s(pin, "path"));
    if repo.is_empty() || repo == "." { path } else { format!("{repo}/{path}") }
}

/// The decisions of RingFrame's knowledge view (`knowledge show`).
pub fn of_view(view: &Value) -> Decisions {
    let mut list: Vec<Decision> = view["decisions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| {
            let answer = s(d, "answer");
            let mut files: Vec<String> = Vec::new();
            for f in d["pins"].as_array().into_iter().flatten().map(file_of) {
                if !f.is_empty() && !files.contains(&f) {
                    files.push(f);
                }
            }
            let stale = d["stale"] == Value::Bool(true);
            let changed: Vec<String> = d["changed"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            let shown = if changed.is_empty() { files.clone() } else { changed };
            let stale_line = stale.then(|| {
                let key = if shown.len() == 1 { "decisions.stale" } else { "decisions.stale_n" };
                fill(key, &[("files", &shown.join(", "))])
            });
            Decision {
                id: s(d, "id"),
                behaviour: s(d, "behaviour"),
                answer_words: word(&format!("decisions.a.{answer}")).to_string(),
                tone: answer_tone(&answer).to_string(),
                answer,
                day: day(s(d, "date").get(..10).unwrap_or_default()),
                files,
                stale,
                stale_line,
            }
        })
        .collect();
    // Newest first; the day is `YYYY-MM-DD…`, so its text orders it.
    let date = |d: &Value| s(d, "date");
    let order: Vec<(String, String)> =
        view["decisions"].as_array().into_iter().flatten().map(|d| (s(d, "id"), date(d))).collect();
    let when = |id: &str| order.iter().find(|(i, _)| i == id).map(|(_, d)| d.clone());
    list.sort_by_key(|d| std::cmp::Reverse(when(&d.id)));
    let n = list.len().to_string();
    let stale = list.iter().filter(|d| d.stale).count();
    let count = if stale == 0 {
        fill("decisions.count", &[("n", &n)])
    } else {
        fill("decisions.count_stale", &[("n", &n), ("stale", &stale.to_string())])
    };
    Decisions {
        title: word("decisions.title").to_string(),
        count,
        changed: word("decisions.changed").to_string(),
        list,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decisions_are_listed_newest_first_and_a_stale_one_says_which_file_changed() {
        let view = json!({"decisions": [
            {"id": "sel_1/d2", "behaviour": "A session ends after 1 hour without activity.",
             "answer": "wanted", "date": "2026-11-03T10:00:00Z", "stale": true,
             "changed": ["src/session.rs"],
             "pins": [{"repo": ".", "path": "src/session.rs", "sha256": "a"}]},
            {"id": "sel_2/d1", "behaviour": "The sign-in button is off while it sends.",
             "answer": "wanted", "date": "2026-11-10T10:00:00Z", "stale": false,
             "pins": [{"repo": ".", "path": "src/login.rs", "sha256": "b"}]},
            {"id": "sel_1/d1", "behaviour": "Emails are written to the server log.",
             "answer": "not_wanted", "date": "2026-11-03T09:00:00Z", "stale": false,
             "pins": [{"repo": ".", "path": "src/log.rs"}, {"repo": "api", "path": "AGENTS.md"}]},
        ]});
        let d = of_view(&view);
        assert_eq!((d.title.as_str(), d.count.as_str()), ("YOUR DECISIONS", "3 · 1 to ask again"));
        let ids: Vec<&str> = d.list.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["sel_2/d1", "sel_1/d2", "sel_1/d1"]);
        let stale = &d.list[1];
        assert_eq!((stale.answer_words.as_str(), stale.tone.as_str()), ("Wanted", "teal"));
        assert_eq!(stale.day, "3 Nov");
        assert_eq!(
            stale.stale_line.as_deref(),
            Some("src/session.rs has changed since; the next Eval that touches it asks again")
        );
        let not = &d.list[2];
        assert_eq!((not.answer_words.as_str(), not.tone.as_str()), ("Not wanted", "amber"));
        assert_eq!(not.files, ["src/log.rs", "api/AGENTS.md"]);
        assert_eq!(not.stale_line, None);
        assert_eq!(of_view(&json!({"decisions": []})).count, "0");
    }

    #[test]
    fn every_answer_a_decision_can_hold_has_its_words() {
        for a in ["wanted", "not_wanted", "now", "later", "drop", "words", "not_sure"] {
            let d =
                of_view(&json!({"decisions": [{"id": "d", "answer": a, "date": "2026-11-01"}]}));
            assert!(!d.list[0].answer_words.starts_with("decisions."), "{a}");
        }
    }
}
