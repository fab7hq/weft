//! Which harness takes an Eval's debate, and which model and effort each role
//! runs on ([ADR-0011], [ADR-0012]). The gather is Weft's own: it runs
//! `ringframe eval open`, which reads the change with Git and records the Eval
//! as gathered, so no harness is asked to map it. Weft types the
//! debate into `/rf:eval`; it calls no model and checks no model name — the
//! harness owns its catalogue. Weft ships no tiers: a role `config.toml` sets
//! nothing for is not typed, and the plugin's own tiers apply.
//!
//! [ADR-0011]: ../../../plans/weft/adr/0011-eval-stages-and-model-choice.md
//! [ADR-0012]: ../../../plans/weft/adr/0012-one-config-toml.md

use serde_json::{Map, Value, json};

/// The debate's roles, in the order they run: RingFrame's judging tasks
/// (ADR-0020: map the change, reduce per requirement, confirm).
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
}

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
    Stage { harness, roles }
}

/// `role=model/effort` for each role that has a setting, a side left empty
/// when only the other is set.
fn words(stage: &Stage) -> Vec<String> {
    stage
        .roles
        .iter()
        .map(|(role, model, effort)| {
            format!(
                "{role}={}/{}",
                model.as_deref().unwrap_or_default(),
                effort.as_deref().unwrap_or_default()
            )
        })
        .collect()
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

/// Where `[E]VAL` types, and what follows `/rf:eval `, for a gathered Eval.
pub fn debate_send(stage: &Stage, eval_id: &str) -> (String, String) {
    let args = [vec!["debate".to_string(), eval_id.to_string()], words(stage)].concat();
    (stage.harness.clone(), args.join(" "))
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
    fn the_debate_is_typed_with_its_eval_and_its_roles() {
        let stages = json!({"debate": {"harness": "claude-code",
            "reduce": {"model": "claude-sonnet-5", "effort": "medium"},
            "map": {"effort": "low"},
            "confirm": {"model": "claude-opus-5-5", "effort": "high"}}});
        let d = resolve(Some(&stages), None, "codex");
        assert_eq!(
            debate_send(&d, "evl_1"),
            (
                "claude-code".into(),
                "debate evl_1 map=/low reduce=claude-sonnet-5/medium confirm=claude-opus-5-5/high"
                    .into()
            )
        );
        assert_eq!(
            debate_send(&resolve(None, None, "codex"), "evl_2"),
            ("codex".into(), "debate evl_2".into())
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
