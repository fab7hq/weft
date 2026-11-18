//! Reading an Eval record.
//!
//! The ledger event carries the report's counts; who read the change, what
//! each item came to and the checks RingFrame ran live in
//! `evals/<id>/record.json`. Weft reads it rather than inferring, because
//! every report names the readers that produced it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::eval_view::EvalView;

/// A reader of the Eval, as the record names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Judge {
    /// `read` or `question`.
    pub angle: String,
    pub host: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub eval_id: String,
    pub judges: Vec<Judge>,
    /// The checks RingFrame ran on the work (`cargo test`), and how each went.
    #[serde(default)]
    pub checks: Vec<Ran>,
    /// What it leaves for the person, item by item.
    #[serde(default)]
    pub view: EvalView,
}

/// One check RingFrame ran: its command, and `succeeded` or `failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ran {
    pub command: String,
    pub outcome: String,
}

fn str_at(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

impl Record {
    /// A record as RingFrame wrote it, or one Weft has read before.
    pub fn parse(v: &Value) -> Option<Self> {
        if v.get("schema").is_none() {
            return serde_json::from_value(v.clone()).ok();
        }
        let eval_id = v.get("eval_id")?.as_str()?.to_string();
        let judges = v
            .get("readers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|j| Judge {
                angle: str_at(j, "role"),
                host: str_at(j, "host"),
                model: str_at(j, "model"),
            })
            .collect();
        let checks = v
            .get("checks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| {
                Some(Ran {
                    command: c.get("command")?.as_str()?.to_string(),
                    outcome: str_at(c, "outcome"),
                })
            })
            .collect();
        Some(Record { view: EvalView::of_record(&eval_id, v), eval_id, judges, checks })
    }

    /// The hosts that read it, in order, without repeats.
    pub fn judged_by(&self) -> Vec<String> {
        let mut hosts: Vec<String> = Vec::new();
        for j in &self.judges {
            if !hosts.contains(&j.host) {
                hosts.push(j.host.clone());
            }
        }
        hosts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/eval-v4/record.json")).unwrap()
    }

    #[test]
    fn a_record_names_its_readers_its_checks_and_what_it_leaves_to_look_at() {
        let r = Record::parse(&fixture()).unwrap();
        assert_eq!(r.eval_id, "evl_v4");
        assert_eq!(r.judged_by(), ["claude"]);
        assert_eq!(r.judges.len(), 2, "the reader and the question reader");
        assert_eq!(r.checks, [Ran { command: "npm test".into(), outcome: "succeeded".into() }]);
        assert_eq!(r.view.headline(), "5 things to look at · 4 settled");
    }

    #[test]
    fn readers_from_different_hosts_are_all_named() {
        let mut v = fixture();
        v["readers"][1]["host"] = json!("codex");
        assert_eq!(Record::parse(&v).unwrap().judged_by(), ["claude", "codex"]);
    }

    #[test]
    fn a_record_read_before_reads_the_same_again() {
        let r = Record::parse(&fixture()).unwrap();
        let again = Record::parse(&serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(again, r);
    }

    #[test]
    fn a_record_without_the_new_fields_reads_without_error() {
        let r = Record::parse(&json!({"schema": "ringframe.eval/2", "eval_id": "evl_0"})).unwrap();
        assert!(r.judges.is_empty() && r.checks.is_empty() && r.view.items.is_empty());
    }
}
