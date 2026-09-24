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
        Routing { by_act, unknown: names(unknown), ignored: names(ignored) }
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
