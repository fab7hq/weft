//! Reading an Eval record.
//!
//! The ledger event carries the verdict and the agreement; who judged, and how
//! each judge voted, live in `evals/<id>/record.json`. Weft reads it rather
//! than inferring, because "every verdict names the host that produced it".


use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Judge {
    pub angle: String,
    pub host: String,
    pub model: String,
}

/// One judge's vote on one item, as RingFrame recorded it.
///
/// `counted_as` is the whole point: RingFrame decides that a `yes` citing
/// nothing is not decisive, and writes down both the vote as cast and what it
/// counted as. Weft renders that decision and never repeats it — the rule has
/// one home, and it is not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vote {
    pub angle: String,
    pub cast: String,
    pub counted_as: String,
    pub uncited: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub text: String,
    /// `yes`, `no`, or `unknown` as the majority saw it.
    pub majority: String,
    pub agreement: f64,
    pub votes: Vec<Vote>,
    pub reasons: Vec<String>,
}

impl Item {
    /// How many judges the majority speaks for, counted from the votes
    /// themselves rather than worked back out of a rounded number.
    pub fn agreed(&self) -> Option<(usize, usize)> {
        if self.votes.is_empty() {
            return None;
        }
        let n = self.votes.iter().filter(|v| v.counted_as == self.majority).count();
        Some((n, self.votes.len()))
    }

    /// Votes RingFrame set aside because they cited nothing a reader could check.
    pub fn uncited(&self) -> usize {
        self.votes.iter().filter(|v| v.uncited).count()
    }
}

impl Item {
    /// What the detail screen shows in the vote column.
    pub fn plain_majority(&self) -> &'static str {
        match self.majority.as_str() {
            "yes" => "yes",
            "no" => "no",
            _ => "not sure",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub eval_id: String,
    pub verdict: String,
    pub confidence: f64,
    pub judges: Vec<Judge>,
    pub items: Vec<Item>,
    /// Changed paths no judge could tie to what was asked.
    pub unexplained: Vec<String>,
}

impl Record {

    pub fn parse(v: &Value) -> Option<Self> {
        let judges = v
            .get("judgements")?
            .as_array()?
            .iter()
            .filter_map(|j| {
                let j = j.get("judge")?;
                Some(Judge {
                    angle: j.get("angle")?.as_str()?.to_string(),
                    host: j.get("host")?.as_str()?.to_string(),
                    model: j.get("model").and_then(Value::as_str).unwrap_or("").to_string(),
                })
            })
            .collect::<Vec<_>>();

        let items = v
            .get("items")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|i| i.get("status").and_then(Value::as_str) != Some("withdrawn"))
                    .map(|i| {
                        let votes: Vec<Vote> = i
                            .get("votes")
                            .and_then(Value::as_array)
                            .map(|vs| vs.iter().map(vote_of).collect())
                            .unwrap_or_default();
                        Item {
                            text: i.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                            majority: i.get("majority").and_then(Value::as_str).unwrap_or("unknown").to_string(),
                            agreement: i.get("agreement").and_then(Value::as_f64).unwrap_or(0.0),
                            reasons: votes
                                .iter()
                                .filter(|v| !v.reason.is_empty())
                                .map(|v| v.reason.clone())
                                .collect(),
                            votes,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let unexplained = v
            .get("drift")
            .and_then(|d| d.get("commission"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(path_of).collect())
            .unwrap_or_default();

        Some(Record {
            eval_id: v.get("eval_id")?.as_str()?.to_string(),
            verdict: v.get("verdict")?.as_str()?.to_string(),
            confidence: v.get("confidence").and_then(Value::as_f64).unwrap_or(0.0),
            judges,
            items,
            unexplained,
        })
    }

    /// The hosts that produced this verdict, in order, without repeats.
    pub fn judged_by(&self) -> Vec<String> {
        let mut hosts: Vec<String> = Vec::new();
        for j in &self.judges {
            if !hosts.contains(&j.host) {
                hosts.push(j.host.clone());
            }
        }
        hosts
    }

    /// An item's own agreement, counted from the votes RingFrame recorded.
    /// Where this is available it is exact, and it is what a row should show.
    pub fn agreed_on(&self, item: &Item) -> String {
        match item.agreed() {
            None => self.agreed(item.agreement),
            Some((n, total)) if n == total => format!("all {total} judges agreed"),
            Some((n, total)) => format!("{n} of {total} judges agreed"),
        }
    }

    pub fn agreed_short_on(&self, item: &Item) -> String {
        match item.agreed() {
            None => self.agreed_short(item.agreement),
            Some((n, total)) if n == total => format!("all {total} agreed"),
            Some((n, total)) => format!("{n} of {total} agreed"),
        }
    }

    /// "all 3 agreed" / "2 of 3 agreed". The same count, in a row's width.
    pub fn agreed_short(&self, agreement: f64) -> String {
        match self.agreed_counts(agreement) {
            None => "agreement not recorded".into(),
            Some((n, total)) if n == total => format!("all {total} agreed"),
            Some((n, total)) => format!("{n} of {total} agreed"),
        }
    }

    fn agreed_counts(&self, agreement: f64) -> Option<(usize, usize)> {
        let total = self.judges.len();
        (total > 0).then(|| ((agreement * total as f64).round() as usize, total))
    }

    /// "2 of 3 judges agreed". Agreement, never correctness.
    pub fn agreed(&self, agreement: f64) -> String {
        let total = self.judges.len();
        if total == 0 {
            return "judge count not recorded".into();
        }
        let n = (agreement * total as f64).round() as usize;
        if n == total {
            format!("all {total} judges agreed")
        } else {
            format!("{n} of {total} judges agreed")
        }
    }
}

fn vote_of(v: &Value) -> Vote {
    let cast = v.get("vote").and_then(Value::as_str).unwrap_or("unknown").to_string();
    Vote {
        angle: v.get("angle").and_then(Value::as_str).unwrap_or("").to_string(),
        // An older record has no `counted_as`: then the vote as cast is what
        // counted, which is exactly what it meant before the rule existed.
        counted_as: v
            .get("counted_as")
            .and_then(Value::as_str)
            .unwrap_or(&cast)
            .to_string(),
        uncited: v.get("uncited").and_then(Value::as_bool).unwrap_or(false),
        reason: v.get("reason").and_then(Value::as_str).unwrap_or("").to_string(),
        cast,
    }
}

fn path_of(v: &Value) -> Option<String> {
    v.get("path")
        .and_then(Value::as_str)
        .or_else(|| v.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record() -> Value {
        json!({
            "schema": "ringframe.eval/1",
            "eval_id": "evl_1",
            "verdict": "drifted",
            "confidence": 0.67,
            "judgements": [
                {"judge": {"angle": "coverage", "host": "codex", "model": "gpt-5"}},
                {"judge": {"angle": "drift", "host": "codex", "model": "gpt-5"}},
                {"judge": {"angle": "adversary", "host": "codex", "model": "gpt-5"}}
            ],
            "items": [
                {"id": "i1", "status": "active", "text": "returns the real build number",
                 "majority": "no", "agreement": 1.0,
                 "votes": [{"angle": "drift", "vote": "no", "reason": "it still reads a literal \"dev\""}]},
                {"id": "i2", "status": "withdrawn", "text": "gone", "majority": "yes", "agreement": 1.0, "votes": []},
                {"id": "i3", "status": "active", "text": "reverts the README change",
                 "majority": "unknown", "agreement": 0.67, "votes": []}
            ],
            "drift": {"commission": [{"path": "README.md", "finding": "unexplained"}]}
        })
    }

    #[test]
    fn the_judging_hosts_come_from_the_record_not_from_the_asking_harness() {
        let r = Record::parse(&record()).expect("parse");
        assert_eq!(r.judged_by(), vec!["codex".to_string()]);
    }

    #[test]
    fn judges_from_different_hosts_are_all_named() {
        let mut v = record();
        v["judgements"][0]["judge"]["host"] = json!("claude-code");
        let r = Record::parse(&v).expect("parse");
        assert_eq!(r.judged_by(), vec!["claude-code".to_string(), "codex".to_string()]);
    }

    #[test]
    fn agreement_reads_as_a_count_of_judges() {
        let r = Record::parse(&record()).expect("parse");
        assert_eq!(r.agreed(0.67), "2 of 3 judges agreed");
        assert_eq!(r.agreed(1.0), "all 3 judges agreed");
    }

    #[test]
    fn weft_counts_what_ringframe_said_a_vote_counted_as() {
        // RingFrame decides that a `yes` citing nothing is not decisive and
        // records both the vote and what it counted as. Weft renders that
        // decision; the rule has one home and it is not here.
        let mut v = record();
        v["items"][0]["votes"] = json!([
            {"angle": "coverage", "vote": "yes", "counted_as": "unknown", "uncited": true, "reason": "looks right"},
            {"angle": "drift", "vote": "no", "counted_as": "no", "reason": "still reads \"dev\""},
            {"angle": "adversary", "vote": "no", "counted_as": "no", "reason": "src/server.js line 41"}
        ]);
        v["items"][0]["majority"] = json!("no");
        let r = Record::parse(&v).expect("parse");
        let item = &r.items[0];
        assert_eq!(item.agreed(), Some((2, 3)), "two of three, the uncited one still counted in");
        assert_eq!(r.agreed_on(item), "2 of 3 judges agreed");
        assert_eq!(item.uncited(), 1);
    }

    #[test]
    fn a_record_written_before_the_rule_reads_as_it_always_did() {
        // No `counted_as` means the vote as cast is what counted, which is
        // exactly what it meant before RingFrame set uncited votes aside.
        let r = Record::parse(&record()).expect("parse");
        let item = &r.items[0];
        assert!(item.votes.iter().all(|v| v.counted_as == v.cast));
        assert_eq!(item.uncited(), 0);
    }

    #[test]
    fn an_item_with_no_recorded_votes_falls_back_to_the_agreement_number() {
        let mut v = record();
        v["items"][0]["votes"] = json!([]);
        let r = Record::parse(&v).expect("parse");
        assert_eq!(r.items[0].agreed(), None);
        assert_eq!(r.agreed_on(&r.items[0]), "all 3 judges agreed", "from agreement 1.0");
    }

    #[test]
    fn the_short_form_says_the_same_thing_in_a_rows_width() {
        let r = Record::parse(&record()).expect("parse");
        assert_eq!(r.agreed_short(0.67), "2 of 3 agreed");
        assert_eq!(r.agreed_short(1.0), "all 3 agreed");
    }

    #[test]
    fn a_record_without_judges_says_so_rather_than_inventing_a_count() {
        let mut v = record();
        v["judgements"] = json!([]);
        let r = Record::parse(&v).expect("parse");
        assert_eq!(r.agreed(0.67), "judge count not recorded");
    }

    #[test]
    fn withdrawn_items_are_not_shown() {
        let r = Record::parse(&record()).expect("parse");
        assert_eq!(r.items.len(), 2);
        assert!(r.items.iter().all(|i| i.text != "gone"));
    }

    #[test]
    fn an_undecided_item_reads_as_not_sure() {
        let r = Record::parse(&record()).expect("parse");
        assert_eq!(r.items[1].plain_majority(), "not sure");
        assert_eq!(r.items[0].plain_majority(), "no");
    }

    #[test]
    fn a_judges_reason_is_carried_through_verbatim() {
        let r = Record::parse(&record()).expect("parse");
        assert_eq!(r.items[0].reasons, vec!["it still reads a literal \"dev\"".to_string()]);
    }

    #[test]
    fn unexplained_changes_are_listed() {
        let r = Record::parse(&record()).expect("parse");
        assert_eq!(r.unexplained, vec!["README.md".to_string()]);
    }
}
