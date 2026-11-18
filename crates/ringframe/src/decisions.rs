//! The person's decisions: what they answered to an Eval's questions when
//! they sealed the work. Each binds to the units and requirements its
//! question was about, never to the wording, and later Evals read it both
//! ways: a standing answer joins their briefs as the person's words, so it is
//! not asked again; a behaviour the person did not want is checked for, and
//! reported if it comes back. Nothing acts on a decision. A decision answered
//! again where it comes back is a new one that supersedes it; none expires.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::evaluate::{array_of, str_of};
use crate::workspace::Workspace;
use crate::{store, tasks};

/// The decisions a Seal records from its Eval's questions and the person's
/// answers: every question gets one, `not_sure` where it was left without
/// an answer. Refused: an answer to no question of the Eval, an answer its
/// question does not take, `words` without words.
pub fn for_seal(record: &Value, answers: &Value, seal_id: &str) -> Result<Vec<Value>, String> {
    let questions = array_of(record, "questions");
    let given = array_of(answers, "answers");
    for a in given {
        let q = str_of(a, "question");
        let Some(question) = questions.iter().find(|x| x["id"] == q) else {
            return Err(format!("{q:?} is not a question of this Eval"));
        };
        let ask = str_of(question, "ask");
        let takes = tasks::QUESTIONS.iter().find(|(k, _)| *k == ask).map_or(&[][..], |(_, t)| t);
        let answer = str_of(a, "answer");
        if !takes.contains(&answer.as_str()) {
            return Err(format!("{q} is {ask}: it takes {}, not {answer:?}", takes.join(", ")));
        }
        if answer == "words" && str_of(a, "words").trim().is_empty() {
            return Err(format!("{q}: an answer in words says them"));
        }
    }
    let against = array_of(&record["report"], "against_decisions");
    let mut out = Vec::new();
    for (n, q) in questions.iter().enumerate() {
        let a = given.iter().find(|a| a["question"] == q["id"]).cloned().unwrap_or(Value::Null);
        let items: Vec<String> =
            array_of(q, "items").iter().filter_map(Value::as_str).map(str::to_string).collect();
        let of = |p: &str| -> Vec<String> {
            items.iter().filter_map(|i| i.strip_prefix(p).map(str::to_string)).collect()
        };
        let windows = of("w|");
        // A question about a decision that came back, or one carried as not
        // sure, answers that decision again.
        let supersedes = of("c|").into_iter().next().or_else(|| {
            against
                .iter()
                .find(|d| array_of(d, "windows").iter().any(|w| windows.contains(&str_of_value(w))))
                .map(|d| str_of(d, "decision"))
        });
        let answer = a["answer"].as_str().unwrap_or("not_sure");
        out.push(json!({
            "id": format!("{seal_id}/d{}", n + 1), "question": q["id"], "ask": q["ask"],
            "behaviour": q["behaviour"], "windows": windows, "requirements": of("r|"),
            "answer": answer,
            "words": if answer == "words" { a["words"].clone() } else { Value::Null },
            "ms": a["ms"].as_u64(), "supersedes": supersedes,
        }));
    }
    Ok(out)
}

fn str_of_value(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// Every decision in the workspace, oldest first, each with the Seal that
/// recorded it, its date, and the Asks that Seal sealed.
pub fn all(ws: &Workspace) -> Vec<Value> {
    let events = store::events(ws).unwrap_or_default();
    let mut out = Vec::new();
    for e in events.iter().filter(|e| e["type"] == "seal.created") {
        for d in array_of(&e["data"], "decisions") {
            let mut d = d.clone();
            d["seal"] = e["id"].clone();
            d["date"] = json!(str_of(e, "time").chars().take(10).collect::<String>());
            d["asks"] = e["data"]["basis"]["asks"].clone();
            out.push(d);
        }
    }
    out
}

/// The newest decision of each chain: one no later decision supersedes.
fn newest(all: Vec<Value>) -> Vec<Value> {
    let superseded: BTreeSet<String> =
        all.iter().filter_map(|d| d["supersedes"].as_str().map(str::to_string)).collect();
    all.into_iter().filter(|d| !superseded.contains(&str_of(d, "id"))).collect()
}

/// The decisions that stand: the newest of each chain, and not `not_sure`.
pub fn standing(ws: &Workspace) -> Vec<Value> {
    newest(all(ws)).into_iter().filter(|d| d["answer"] != "not_sure").collect()
}

/// The `not_sure` decisions carried to the next Eval of their unit: the
/// newest of their chains, from a Seal of an Ask in `unit`.
pub fn carried(ws: &Workspace, unit: &[String]) -> Vec<Value> {
    newest(all(ws))
        .into_iter()
        .filter(|d| d["answer"] == "not_sure")
        .filter(|d| array_of(d, "asks").iter().any(|a| unit.contains(&str_of_value(a))))
        .collect()
}

/// A standing decision as the person's words in a later brief; `not_wanted`
/// has none: it goes in the decisions to check.
pub fn words(d: &Value) -> Option<String> {
    let said = match str_of(d, "answer").as_str() {
        "wanted" => format!("the person wanted: {}", str_of(d, "behaviour")),
        "now" | "later" => format!("the person still wants: {}", str_of(d, "behaviour")),
        "drop" => format!("the person no longer wants: {}", str_of(d, "behaviour")),
        "words" => format!(
            "the person said what they meant by \"{}\": {}",
            str_of(d, "behaviour"),
            str_of(d, "words")
        ),
        _ => return None,
    };
    Some(format!("On {} (Seal {}) {said}", str_of(d, "date"), str_of(d, "seal")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Value {
        json!({"questions": [
            {"id": "q1", "ask": "did_you_want", "behaviour": "Sessions end after an hour.", "items": ["w|w_a"]},
            {"id": "q2", "ask": "still_want", "behaviour": "The notes name it.", "items": ["r|S2"]},
            {"id": "q3", "ask": "what_did_you_mean", "behaviour": "short", "items": ["r|S3", "c|sel_1/d2"]}],
               "report": {"against_decisions": [{"decision": "sel_0/d1", "windows": ["w_a"]}]}})
    }

    #[test]
    fn answers_are_recorded_as_given_and_a_missing_one_is_not_sure() {
        let answers = json!({"answers": [{"question": "q1", "answer": "not_wanted", "ms": 4200},
                                         {"question": "q3", "answer": "words", "words": "only the form", "ms": 900}]});
        let d = for_seal(&record(), &answers, "sel_9").unwrap();
        assert_eq!(d.len(), 3);
        assert_eq!(d[0]["id"], "sel_9/d1");
        assert_eq!(
            (d[0]["answer"].clone(), d[0]["ms"].clone()),
            (json!("not_wanted"), json!(4200))
        );
        assert_eq!(d[0]["windows"], json!(["w_a"]));
        assert_eq!(d[0]["supersedes"], "sel_0/d1", "a decision that came back is answered again");
        assert_eq!(d[1]["answer"], "not_sure", "left without an answer");
        assert_eq!(d[1]["requirements"], json!(["S2"]));
        assert_eq!(d[2]["words"], "only the form");
        assert_eq!(d[2]["supersedes"], "sel_1/d2", "a carried question answers its decision");
        assert_eq!(for_seal(&json!({}), &json!({}), "sel_9").unwrap(), Vec::<Value>::new());
    }

    #[test]
    fn an_answer_its_question_does_not_take_is_refused() {
        let refused = |a: Value| for_seal(&record(), &json!({"answers": [a]}), "s").unwrap_err();
        assert!(refused(json!({"question": "q9", "answer": "wanted"})).contains("not a question"));
        assert!(refused(json!({"question": "q1", "answer": "now"})).contains("it takes"));
        assert!(
            refused(json!({"question": "q3", "answer": "words", "words": " "}))
                .contains("says them")
        );
    }

    #[test]
    fn a_standing_decision_reads_as_the_persons_words() {
        let d = |answer: &str| {
            json!({"answer": answer, "behaviour": "Sessions end after an hour.",
                                      "words": "only on the web", "date": "2026-11-17", "seal": "sel_1"})
        };
        assert_eq!(
            words(&d("wanted")).unwrap(),
            "On 2026-11-17 (Seal sel_1) the person wanted: Sessions end after an hour."
        );
        assert!(words(&d("later")).unwrap().contains("still wants"));
        assert!(words(&d("drop")).unwrap().contains("no longer wants"));
        assert!(
            words(&d("words"))
                .unwrap()
                .ends_with("\"Sessions end after an hour.\": only on the web")
        );
        assert_eq!(words(&d("not_wanted")), None);
        assert_eq!(words(&d("not_sure")), None);
    }

    #[test]
    fn only_the_newest_of_a_chain_stands() {
        let all = vec![
            json!({"id": "a/d1", "supersedes": null}),
            json!({"id": "b/d1", "supersedes": "a/d1"}),
            json!({"id": "c/d1", "supersedes": null}),
        ];
        let ids: Vec<String> = newest(all).iter().map(|d| str_of(d, "id")).collect();
        assert_eq!(ids, ["b/d1", "c/d1"]);
    }
}
