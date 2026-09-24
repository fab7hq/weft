//! Which harness takes each stage of an Eval, and which model and effort each
//! role runs on ([ADR-0011], [ADR-0012]). Weft types these into `/rf:eval`; it
//! calls no model and checks no model name — the harness owns its catalogue.
//! Weft ships no tiers: a role `config.toml` sets nothing for is not typed,
//! and the plugin's own tiers apply.
//!
//! [ADR-0011]: ../../../plans/weft/adr/0011-eval-stages-and-model-choice.md
//! [ADR-0012]: ../../../plans/weft/adr/0012-one-config-toml.md

use serde_json::Value;

/// The gather stage's one role.
pub const GATHER_ROLES: [&str; 1] = ["context"];
/// The debate stage's roles, in the order they run.
pub const DEBATE_ROLES: [&str; 4] = ["intent", "coverage", "drift", "adversary"];

/// One stage of an Eval, resolved: where it runs, and what each role that has
/// a setting was asked to run on. A role with neither is left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub harness: String,
    pub roles: Vec<(String, Option<String>, Option<String>)>,
}

/// Gather and debate for this project: each stage's harness is its own
/// `harness`, else `routing.eval`, else `fallback`; each role is what the
/// stage sets for it.
pub fn resolve(stages: Option<&Value>, eval: Option<&str>, fallback: &str) -> (Stage, Stage) {
    let stage = |name: &str, roles: &[&str]| {
        let route = stages.and_then(|e| e.get(name));
        let harness = route
            .and_then(|r| r.get("harness"))
            .and_then(Value::as_str)
            .or(eval)
            .unwrap_or(fallback)
            .to_string();
        let resolved = roles
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
        Stage { harness, roles: resolved }
    };
    (stage("gather", &GATHER_ROLES), stage("debate", &DEBATE_ROLES))
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

/// Where `[E]VAL` types next and what follows `/rf:eval `: the debate for an
/// Eval already gathered; else both stages at once when they share a harness;
/// else the gather.
pub fn next_send(gather: &Stage, debate: &Stage, gathered: Option<&str>) -> (String, String) {
    if let Some(eval_id) = gathered {
        let args = [vec!["debate".to_string(), eval_id.to_string()], words(debate)].concat();
        return (debate.harness.clone(), args.join(" "));
    }
    if gather.harness == debate.harness {
        return (gather.harness.clone(), [words(gather), words(debate)].concat().join(" "));
    }
    (gather.harness.clone(), [vec!["gather".to_string()], words(gather)].concat().join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The tiers Phase 3.14 shipped for Claude Code, now set in `config.toml`.
    fn claude_tiers(harness: &str) -> Value {
        json!({
            "gather": {"harness": harness, "context": {"model": "claude-sonnet-5", "effort": "low"}},
            "debate": {"harness": harness,
                "intent": {"model": "claude-sonnet-5", "effort": "medium"},
                "coverage": {"model": "claude-sonnet-5", "effort": "medium"},
                "drift": {"model": "claude-sonnet-5", "effort": "high"},
                "adversary": {"model": "claude-opus-5-5", "effort": "high"}},
        })
    }

    fn role(stage: &Stage, name: &str) -> Option<(Option<String>, Option<String>)> {
        stage.roles.iter().find(|r| r.0 == name).map(|r| (r.1.clone(), r.2.clone()))
    }

    fn some(model: &str, effort: &str) -> Option<(Option<String>, Option<String>)> {
        Some((Some(model.into()), Some(effort.into())))
    }

    #[test]
    fn a_stage_harness_wins_over_routing_eval_which_wins_over_the_fallback() {
        let split = json!({"gather": {"harness": "codex"}});
        let (gather, debate) = resolve(Some(&split), Some("claude-code"), "codex");
        assert_eq!((gather.harness.as_str(), debate.harness.as_str()), ("codex", "claude-code"));
        let (gather, debate) = resolve(None, None, "claude-code");
        assert_eq!(
            (gather.harness.as_str(), debate.harness.as_str()),
            ("claude-code", "claude-code")
        );
    }

    #[test]
    fn a_role_runs_on_what_its_stage_sets_and_nothing_else() {
        let stages = json!({"debate": {"adversary": {"effort": "xhigh"}, "drift": {"model": "m"}}});
        let (gather, debate) = resolve(Some(&stages), Some("codex"), "codex");
        assert!(gather.roles.is_empty(), "nothing set: the plugin's tiers apply");
        assert_eq!(role(&debate, "adversary"), Some((None, Some("xhigh".into()))));
        assert_eq!(role(&debate, "drift"), Some((Some("m".into()), None)));
        assert_eq!(role(&debate, "intent"), None);
    }

    #[test]
    fn what_eval_sends_is_unchanged_for_the_same_settings() {
        let (g, d) = resolve(Some(&claude_tiers("claude-code")), None, "claude-code");
        assert_eq!(role(&g, "context"), some("claude-sonnet-5", "low"));
        assert_eq!(
            next_send(&g, &d, None),
            (
                "claude-code".into(),
                "context=claude-sonnet-5/low intent=claude-sonnet-5/medium \
                 coverage=claude-sonnet-5/medium drift=claude-sonnet-5/high \
                 adversary=claude-opus-5-5/high"
                    .into()
            )
        );
        let split = json!({"gather": {"harness": "codex", "context": {"model": "gpt-6-luna", "effort": "low"}},
                           "debate": {"harness": "claude-code",
                                      "intent": {"model": "claude-sonnet-5", "effort": "medium"},
                                      "coverage": {"model": "claude-sonnet-5", "effort": "medium"},
                                      "drift": {"effort": "high"},
                                      "adversary": {"model": "claude-opus-5-5", "effort": "high"}}});
        let (g, d) = resolve(Some(&split), None, "claude-code");
        assert_eq!(
            next_send(&g, &d, None),
            ("codex".into(), "gather context=gpt-6-luna/low".into())
        );
        assert_eq!(
            next_send(&g, &d, Some("evl_1")),
            (
                "claude-code".into(),
                "debate evl_1 intent=claude-sonnet-5/medium coverage=claude-sonnet-5/medium \
                 drift=/high adversary=claude-opus-5-5/high"
                    .into()
            )
        );
        let (g, d) = resolve(None, Some("codex"), "claude-code");
        assert_eq!(
            next_send(&g, &d, None),
            ("codex".into(), String::new()),
            "no roles: a bare eval"
        );
    }
}
