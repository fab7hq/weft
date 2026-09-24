//! Which harness takes each stage of an Eval, and which model and effort each
//! role runs on ([ADR-0011]). Weft types these into `/rf:eval`; it calls no
//! model and checks no model name — the harness owns its catalogue.
//!
//! [ADR-0011]: ../../../plans/weft/adr/0011-eval-stages-and-model-choice.md

use serde_json::{Map, Value};

/// The gather stage's one role.
pub const GATHER_ROLES: [&str; 1] = ["context"];
/// The debate stage's roles, in the order they run.
pub const DEBATE_ROLES: [&str; 4] = ["intent", "coverage", "drift", "adversary"];
const KEYS: [&str; 2] = ["model", "effort"];

/// What Weft writes to `~/.fab7/weft/eval.json` when there is none. The
/// owner's tiers: no Fable, no GPT-6 Astra, no Haiku.
pub const DEFAULTS: &str = r#"{
  "claude-code": {
    "context":   {"model": "claude-sonnet-5", "effort": "low"},
    "intent":    {"model": "claude-sonnet-5", "effort": "medium"},
    "coverage":  {"model": "claude-sonnet-5", "effort": "medium"},
    "drift":     {"model": "claude-sonnet-5", "effort": "high"},
    "adversary": {"model": "claude-opus-5-5", "effort": "high"}
  },
  "codex": {
    "context":   {"model": "gpt-6-luna", "effort": "low"},
    "intent":    {"model": "gpt-6-sol", "effort": "medium"},
    "coverage":  {"model": "gpt-6-sol", "effort": "medium"},
    "drift":     {"model": "gpt-6-sol", "effort": "high"},
    "adversary": {"model": "gpt-6-sol", "effort": "xhigh"}
  }
}
"#;

fn is_role(role: &str) -> bool {
    GATHER_ROLES.contains(&role) || DEBATE_ROLES.contains(&role)
}

/// The tiers per harness, as the person keeps them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tiers {
    /// harness → role → `{model?, effort?}`, only names Weft knows.
    pub by_harness: Map<String, Value>,
    /// Each name Weft does not know, to be said once rather than obeyed.
    pub unknown: Vec<String>,
}

/// Keep what names a harness, a role and `model` or `effort`; report the rest.
/// Nothing readable is no tiers: every role then runs on the harness default.
pub fn tiers(text: &str) -> Tiers {
    let mut out = Tiers::default();
    let Ok(Value::Object(all)) = serde_json::from_str::<Value>(text) else { return out };
    for (harness, roles) in all {
        if crate::harness::find(&harness).is_none() {
            out.unknown.push(harness);
            continue;
        }
        let mut kept = Map::new();
        for (role, keys) in roles.as_object().into_iter().flatten() {
            if !is_role(role) {
                out.unknown.push(format!("{harness}.{role}"));
                continue;
            }
            let mut settings = Map::new();
            for (key, value) in keys.as_object().into_iter().flatten() {
                if !KEYS.contains(&key.as_str()) {
                    out.unknown.push(format!("{harness}.{role}.{key}"));
                } else if value.as_str().is_some_and(|v| !v.is_empty()) {
                    settings.insert(key.clone(), value.clone());
                }
            }
            kept.insert(role.clone(), Value::Object(settings));
        }
        out.by_harness.insert(harness, Value::Object(kept));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_shipped_tiers_are_read_whole() {
        let t = tiers(DEFAULTS);
        assert!(t.unknown.is_empty(), "{:?}", t.unknown);
        assert_eq!(
            t.by_harness["claude-code"]["adversary"],
            json!({"model": "claude-opus-5-5", "effort": "high"})
        );
        assert_eq!(
            t.by_harness["codex"]["context"],
            json!({"model": "gpt-6-luna", "effort": "low"})
        );
        for harness in ["claude-code", "codex"] {
            for role in GATHER_ROLES.iter().chain(DEBATE_ROLES.iter()) {
                assert!(t.by_harness[harness].get(*role).is_some(), "{harness}.{role}");
            }
        }
    }

    #[test]
    fn names_weft_does_not_know_are_reported_and_left_out() {
        let t = tiers(
            r#"{"aider": {"drift": {"model": "x"}},
                "codex": {"judge": {"model": "x"}, "drift": {"model": "gpt-6-sol", "temperature": "1"}}}"#,
        );
        assert_eq!(t.unknown, ["aider", "codex.drift.temperature", "codex.judge"]);
        assert_eq!(t.by_harness["codex"], json!({"drift": {"model": "gpt-6-sol"}}));
    }

    #[test]
    fn nothing_readable_is_no_tiers() {
        for text in ["", "not json", "[]", "null"] {
            assert_eq!(tiers(text), Tiers::default(), "{text:?}");
        }
    }
}
