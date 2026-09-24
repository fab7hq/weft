//! Which harness takes which of RingFrame's three acts, in this project.
//!
//! > Codex asks, Claude implements, Codex evaluates.
//!
//! All three acts are host-agnostic on disk — `eval open` briefs over *every*
//! open Ask, `seal` names no host, and a single Ask is always addressed by id —
//! so this needs nothing from RingFrame. It only says where Weft offers the
//! next act.
//!
//! Deliberately the simplest thing that works: one harness name per act, read
//! from [`crate::config`], no schema, no versioning, no permissions. It is a
//! preference about where to type, not a security boundary.

use std::collections::HashMap;

use serde_json::Value;

/// The three acts. Weft's own keys map onto these; `Send` does not, because a
/// Send is the delivery of an Ask already made and follows that record.
pub const ACTS: [&str; 3] = ["ask", "eval", "seal"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Routing {
    pub(crate) by_act: HashMap<String, String>,
    /// A name in the file that is not a harness Weft knows. Kept so it can be
    /// said once rather than silently ignored.
    pub unknown: Vec<String>,
    /// Any other name in the file Weft does not know, said once the same way.
    pub ignored: Vec<String>,
    /// A file `config.toml` replaced, found and not read.
    pub leftover: Vec<String>,
    /// `[eval.gather]` and `[eval.debate]`: a harness and role settings per stage.
    pub eval_stages: Option<Value>,
}

impl Routing {
    /// From what a daemon reports, rather than from the file.
    pub fn of(
        acts: serde_json::Value,
        unknown: serde_json::Value,
        ignored: serde_json::Value,
        leftover: serde_json::Value,
        eval_stages: serde_json::Value,
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
        let eval_stages = eval_stages.is_object().then_some(eval_stages);
        Routing {
            by_act,
            unknown: names(unknown),
            ignored: names(ignored),
            leftover: names(leftover),
            eval_stages,
        }
    }

    /// The harness this act goes to, if the project named one.
    pub fn get(&self, act: &str) -> Option<&str> {
        self.by_act.get(act).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.by_act.is_empty()
    }

    /// Every harness an act or an Eval stage is routed to, once each, sorted.
    pub fn harnesses(&self) -> Vec<String> {
        let stages = self.eval_stages.iter().flat_map(|s| s.as_object().into_iter().flatten());
        let mut out: Vec<String> = self
            .by_act
            .values()
            .cloned()
            .chain(stages.filter_map(|(_, st)| Some(st.get("harness")?.as_str()?.to_string())))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// What this project routes, in a fixed order, for showing.
    pub fn each(&self) -> Vec<(&'static str, &str)> {
        ACTS.iter().filter_map(|a| self.get(a).map(|h| (*a, h))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_harnesses_routing_names_include_each_eval_stages() {
        let r = crate::config::read(
            "[routing]\nask = \"claude-code\"\n\n[eval.gather]\nharness = \"codex\"\n\n\
             [eval.debate]\nharness = \"claude-code\"\n",
        )
        .routing(std::path::Path::new("/p"));
        assert_eq!(r.harnesses(), ["claude-code", "codex"]);
        assert!(Routing::default().harnesses().is_empty());
    }
}
