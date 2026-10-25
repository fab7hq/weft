//! Which harness an Eval's judges run in, which model and effort each role
//! runs on, and how many judges run at once. The gather is Weft's own: it
//! opens the Eval through RingFrame's crate, which reads the change with Git
//! and records it as gathered. Each judge is a session of that harness Weft
//! starts; Weft calls no model and checks no model name — the harness owns
//! its catalogue. Weft ships no tiers: a role `config.toml` sets nothing for
//! is given RingFrame's own.

use serde_json::{Map, Value, json};

/// The debate's roles, in the order they run: RingFrame's judging tasks
/// (map the change, reduce per requirement, confirm).
pub const DEBATE_ROLES: [&str; 3] = ["map", "reduce", "confirm"];

/// What a role was once called, read as what it is now called. `intent` and
/// `context` no longer run, so they stay themselves and are reported.
pub fn role_now(name: &str) -> &str {
    match name {
        "drift" | "trace" => "map",
        "coverage" => "reduce",
        "adversary" => "confirm",
        _ => name,
    }
}

/// The debate, resolved: where it runs, and what each role that has a setting
/// was asked to run on. A role with neither is left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub harness: String,
    pub roles: Vec<(String, Option<String>, Option<String>)>,
    /// How many judges run at once.
    pub judges: usize,
}

/// How many judges run at once when the debate does not say.
pub const JUDGES: usize = 8;

/// The debate for this project: its harness is `[eval.debate] harness`, else
/// `routing.eval`, else `fallback`; each role is what `[eval.debate]` sets.
pub fn resolve(stages: Option<&Value>, eval: Option<&str>, fallback: &str) -> Stage {
    let route = stages.and_then(|e| e.get("debate"));
    let harness = route
        .and_then(|r| r.get("harness"))
        .and_then(Value::as_str)
        .or(eval)
        .unwrap_or(fallback)
        .to_string();
    let roles = DEBATE_ROLES
        .iter()
        .filter_map(|role| {
            let set = route.and_then(|r| r.get(*role));
            let pick = |key: &str| {
                set.and_then(|s| s.get(key))
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
            };
            let (model, effort) = (pick("model"), pick("effort"));
            (model.is_some() || effort.is_some()).then(|| (role.to_string(), model, effort))
        })
        .collect();
    let judges = route.and_then(|r| r.get("judges")).and_then(Value::as_u64);
    Stage { harness, roles, judges: judges.map_or(JUDGES, |n| n as usize) }
}

/// The roles as `ringframe eval open --agents` takes them, when any is set.
pub fn agents(stage: &Stage) -> Option<Value> {
    let mut out = Map::new();
    for (role, model, effort) in &stage.roles {
        let mut keys = Map::new();
        if let Some(m) = model {
            keys.insert("model".into(), json!(m));
        }
        if let Some(e) = effort {
            keys.insert("effort".into(), json!(e));
        }
        out.insert(role.clone(), Value::Object(keys));
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(stage: &Stage, name: &str) -> Option<(Option<String>, Option<String>)> {
        stage.roles.iter().find(|r| r.0 == name).map(|r| (r.1.clone(), r.2.clone()))
    }

    #[test]
    fn the_debate_harness_wins_over_routing_eval_which_wins_over_the_fallback() {
        let split = json!({"debate": {"harness": "codex"}});
        assert_eq!(resolve(Some(&split), Some("claude-code"), "antigravity").harness, "codex");
        assert_eq!(resolve(None, Some("claude-code"), "antigravity").harness, "claude-code");
        assert_eq!(resolve(None, None, "antigravity").harness, "antigravity");
    }

    #[test]
    fn eight_judges_run_at_once_unless_the_debate_says() {
        assert_eq!(resolve(None, None, "codex").judges, JUDGES);
        assert_eq!(JUDGES, 8);
        let three = json!({"debate": {"judges": 3}});
        assert_eq!(resolve(Some(&three), None, "codex").judges, 3);
    }

    #[test]
    fn a_role_runs_on_what_the_debate_sets_and_nothing_else() {
        let stages = json!({"debate": {"confirm": {"effort": "xhigh"}, "map": {"model": "m"}}});
        let d = resolve(Some(&stages), Some("codex"), "codex");
        assert_eq!(role(&d, "confirm"), Some((None, Some("xhigh".into()))));
        assert_eq!(role(&d, "map"), Some((Some("m".into()), None)));
        assert_eq!(role(&d, "reduce"), None);
        assert_eq!(
            agents(&d),
            Some(json!({"map": {"model": "m"}, "confirm": {"effort": "xhigh"}}))
        );
        assert_eq!(
            agents(&resolve(None, None, "codex")),
            None,
            "nothing set: the plugin's tiers apply"
        );
    }

    #[test]
    fn earlier_role_names_are_read_as_the_roles_now() {
        assert_eq!(role_now("drift"), "map");
        assert_eq!(role_now("trace"), "map");
        assert_eq!(role_now("coverage"), "reduce");
        assert_eq!(role_now("adversary"), "confirm");
        assert_eq!(role_now("intent"), "intent", "no longer runs");
    }
}
