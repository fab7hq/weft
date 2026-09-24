//! Which harness takes which of RingFrame's three acts, in this project.
//!
//! > Codex asks, Claude implements, Codex evaluates.
//!
//! All three acts are host-agnostic on disk — `eval open` briefs over *every*
//! open Ask, `seal` names no host, and a single Ask is always addressed by id —
//! so this needs nothing from RingFrame. It only says where Weft offers the
//! next act.
//!
//! Deliberately the simplest thing that works: one file, one harness name per
//! act, no schema, no versioning, no permissions. It is a preference about
//! where to type, not a security boundary.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

/// The three acts. Weft's own keys map onto these; `Send` does not, because a
/// Send is the delivery of an Ask already made and follows that record.
pub const ACTS: [&str; 3] = ["ask", "eval", "seal"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Routing {
    by_act: HashMap<String, String>,
    /// A name in the file that is not a harness Weft knows. Kept so it can be
    /// said once rather than silently ignored.
    pub unknown: Vec<String>,
    /// A name in `eval.json` Weft does not know, said once the same way.
    pub ignored: Vec<String>,
    /// `eval` given per stage rather than as one harness name.
    pub eval_stages: Option<Value>,
}

impl Routing {
    /// From what a daemon reports, rather than from the file.
    pub fn of(
        acts: serde_json::Value,
        unknown: serde_json::Value,
        ignored: serde_json::Value,
    ) -> Self {
        let by_act = acts
            .as_object()
            .map(|m| {
                m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()
            })
            .unwrap_or_default();
        let names = |v: serde_json::Value| -> Vec<String> {
            v.as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default()
        };
        Routing { by_act, unknown: names(unknown), ignored: names(ignored), eval_stages: None }
    }

    /// The harness this act goes to, if the project named one.
    pub fn get(&self, act: &str) -> Option<&str> {
        self.by_act.get(act).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.by_act.is_empty()
    }

    /// What this project routes, in a fixed order, for showing.
    pub fn each(&self) -> Vec<(&'static str, &str)> {
        ACTS.iter().filter_map(|a| self.get(a).map(|h| (*a, h))).collect()
    }
}

/// The rule itself, separated from the disk so it can be tested without one.
pub fn read(text: &str, root: &Path) -> Routing {
    let Ok(all) = serde_json::from_str::<Value>(text) else { return Routing::default() };
    let Some(mine) = all.get(root.to_string_lossy().as_ref()).and_then(Value::as_object) else {
        return Routing::default();
    };
    let mut routing = Routing::default();
    if let Some(stages) = mine.get("eval").filter(|v| v.is_object()) {
        routing.unknown.extend(unknown_in_stages(stages));
        routing.eval_stages = Some(stages.clone());
    }
    for act in ACTS {
        let Some(name) = mine.get(act).and_then(Value::as_str) else { continue };
        // Only a harness Weft supports. An unknown name is reported rather
        // than obeyed: obeying it would send an act nowhere.
        if crate::harness::find(name).is_some() {
            routing.by_act.insert(act.to_string(), name.to_string());
        } else {
            routing.unknown.push(format!("{act}: {name}"));
        }
    }
    routing
}

/// What an `eval` object names that Weft does not know: a stage, a harness, a
/// role outside its stage, or a key other than a role's `model` and `effort`.
fn unknown_in_stages(stages: &Value) -> Vec<String> {
    use crate::eval_stages::{DEBATE_ROLES, GATHER_ROLES};
    let mut out = Vec::new();
    for (stage, body) in stages.as_object().into_iter().flatten() {
        let roles: &[&str] = match stage.as_str() {
            "gather" => &GATHER_ROLES,
            "debate" => &DEBATE_ROLES,
            _ => {
                out.push(format!("eval.{stage}"));
                continue;
            }
        };
        for (key, value) in body.as_object().into_iter().flatten() {
            if key == "harness" {
                if value.as_str().is_none_or(|h| crate::harness::find(h).is_none()) {
                    out.push(format!("eval.{stage}: {}", value.as_str().unwrap_or("?")));
                }
            } else if !roles.contains(&key.as_str()) {
                out.push(format!("eval.{stage}.{key}"));
            } else {
                for k in value.as_object().into_iter().flatten().map(|(k, _)| k) {
                    if k != "model" && k != "effort" {
                        out.push(format!("eval.{stage}.{key}.{k}"));
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HERE: &str = "/work/thing";

    fn at(text: &str) -> Routing {
        read(text, Path::new(HERE))
    }

    #[test]
    fn a_project_routes_the_acts_it_names_and_no_others() {
        let r = at(r#"{"/work/thing": {"ask": "codex", "eval": "claude-code"}}"#);
        assert_eq!(r.get("ask"), Some("codex"));
        assert_eq!(r.get("eval"), Some("claude-code"));
        assert_eq!(r.get("seal"), None, "an act the project did not name");
        assert_eq!(r.each(), vec![("ask", "codex"), ("eval", "claude-code")]);
    }

    #[test]
    fn eval_per_stage_is_kept_and_what_it_names_wrongly_is_reported() {
        let r = at(r#"{"/work/thing": {"eval": {
            "gather": {"harness": "codex"},
            "debate": {"harness": "aider", "judge": {}, "drift": {"effort": "high", "temperature": 1}},
            "review": {}}}}"#);
        assert_eq!(r.get("eval"), None, "not one harness");
        assert_eq!(r.eval_stages.as_ref().unwrap()["gather"]["harness"], "codex");
        assert_eq!(
            r.unknown,
            [
                "eval.debate.drift.temperature",
                "eval.debate: aider",
                "eval.debate.judge",
                "eval.review"
            ]
        );
    }

    #[test]
    fn a_project_with_no_entry_routes_nothing() {
        // Which is how Weft behaved before routing existed, and must still.
        let r = at(r#"{"/somewhere/else": {"eval": "codex"}}"#);
        assert!(r.is_empty());
        for act in ACTS {
            assert_eq!(r.get(act), None);
        }
    }

    #[test]
    fn nothing_readable_routes_nothing_rather_than_failing() {
        // No file, an empty one, or something that is not a routing file at
        // all. None of these is worth stopping a person's work over.
        for text in ["", "not json", "[]", "null", r#"{"/work/thing": "codex"}"#] {
            assert!(at(text).is_empty(), "{text:?}");
        }
    }

    #[test]
    fn a_harness_weft_does_not_know_is_reported_rather_than_obeyed() {
        let r = at(r#"{"/work/thing": {"eval": "aider", "seal": "codex"}}"#);
        assert_eq!(r.get("eval"), None, "sending an act nowhere is worse than not routing it");
        assert_eq!(r.unknown, vec!["eval: aider"]);
        assert_eq!(r.get("seal"), Some("codex"), "and the rest still stands");
    }

    #[test]
    fn only_the_three_acts_are_read() {
        // Send is not here: it is the delivery of an Ask already made, and it
        // follows that Ask's record. Anything else in the file is ignored.
        let r = at(r#"{"/work/thing": {"send": "codex", "plan": "codex", "goal": "codex"}}"#);
        assert!(r.is_empty());
        assert!(r.unknown.is_empty(), "an unknown act is not an unknown harness");
    }
}
