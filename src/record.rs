//! Reading an Eval record.
//!
//! The ledger event carries the verdict and the agreement; who judged, and how
//! each judge voted, live in `evals/<id>/record.json`. Weft reads it rather
//! than inferring, because "every verdict names the host that produced it".

use std::path::Path;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judge {
    pub angle: String,
    pub host: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub text: String,
    /// `yes`, `no`, or `unknown` as the majority saw it.
    pub majority: String,
    pub agreement: f64,
    pub reasons: Vec<String>,
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

#[derive(Debug, Clone, PartialEq)]
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
    pub fn read(project_root: &Path, eval_id: &str) -> Option<Self> {
        let path = project_root
            .join(".fab7/rf/evals")
            .join(eval_id)
            .join("record.json");
        let bytes = std::fs::read(path).ok()?;
        Self::parse(&serde_json::from_slice::<Value>(&bytes).ok()?)
    }

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
                    .map(|i| Item {
                        text: i.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                        majority: i.get("majority").and_then(Value::as_str).unwrap_or("unknown").to_string(),
                        agreement: i.get("agreement").and_then(Value::as_f64).unwrap_or(0.0),
                        reasons: i
                            .get("votes")
                            .and_then(Value::as_array)
                            .map(|vs| {
                                vs.iter()
                                    .filter_map(|x| x.get("reason").and_then(Value::as_str))
                                    .filter(|r| !r.is_empty())
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let unexplained = v
            .get("drift")
            .and_then(|d| d.get("commission"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| path_of(x)).collect())
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
