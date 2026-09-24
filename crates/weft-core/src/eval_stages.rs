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

/// One stage of an Eval, resolved: where it runs, and what each role that has
/// a setting was asked to run on. A role with neither is left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub harness: String,
    pub roles: Vec<(String, Option<String>, Option<String>)>,
}

/// Gather and debate for this project: its `eval` route (a harness name, or an
/// object per stage) over the tiers, falling back to `fallback` for a stage
/// that names no harness.
pub fn resolve(tiers: &Tiers, eval: Option<&Value>, fallback: &str) -> (Stage, Stage) {
    let stage = |name: &str, roles: &[&str]| {
        let route = eval.and_then(|e| e.get(name));
        let harness = route
            .and_then(|r| r.get("harness"))
            .and_then(Value::as_str)
            .filter(|h| crate::harness::find(h).is_some())
            .or_else(|| eval.and_then(Value::as_str))
            .unwrap_or(fallback)
            .to_string();
        let resolved = roles
            .iter()
            .filter_map(|role| {
                let base = tiers.by_harness.get(&harness).and_then(|h| h.get(*role));
                let over = route.and_then(|r| r.get(*role));
                let pick = |key: &str| match over.and_then(|o| o.get(key)) {
                    Some(Value::Null) => None,
                    Some(Value::String(v)) if !v.is_empty() => Some(v.clone()),
                    _ => base.and_then(|b| b.get(key)).and_then(Value::as_str).map(str::to_string),
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

    fn role(stage: &Stage, name: &str) -> Option<(Option<String>, Option<String>)> {
        stage.roles.iter().find(|r| r.0 == name).map(|r| (r.1.clone(), r.2.clone()))
    }

    fn some(model: &str, effort: &str) -> Option<(Option<String>, Option<String>)> {
        Some((Some(model.into()), Some(effort.into())))
    }

    #[test]
    fn a_harness_name_sends_both_stages_there_on_its_tiers() {
        let (gather, debate) = resolve(&tiers(DEFAULTS), Some(&json!("codex")), "claude-code");
        assert_eq!((gather.harness.as_str(), debate.harness.as_str()), ("codex", "codex"));
        assert_eq!(role(&gather, "context"), some("gpt-6-luna", "low"));
        assert_eq!(role(&debate, "adversary"), some("gpt-6-sol", "xhigh"));
        let (gather, _) = resolve(&tiers(DEFAULTS), None, "claude-code");
        assert_eq!(gather.harness, "claude-code", "no route: the fallback");
    }

    #[test]
    fn a_stage_names_its_harness_and_a_project_key_overrides_one_role() {
        let route = json!({"gather": {"harness": "codex"},
                           "debate": {"harness": "claude-code",
                                      "adversary": {"effort": "xhigh"}, "drift": {"model": null}}});
        let (gather, debate) = resolve(&tiers(DEFAULTS), Some(&route), "claude-code");
        assert_eq!(gather.harness, "codex");
        assert_eq!(role(&gather, "context"), some("gpt-6-luna", "low"));
        assert_eq!(debate.harness, "claude-code");
        assert_eq!(role(&debate, "adversary"), some("claude-opus-5-5", "xhigh"));
        assert_eq!(
            role(&debate, "drift"),
            Some((None, Some("high".into()))),
            "null drops the model"
        );
        assert_eq!(role(&debate, "intent"), some("claude-sonnet-5", "medium"));
    }

    #[test]
    fn a_role_with_nothing_set_is_left_to_the_harness() {
        let (gather, debate) = resolve(&Tiers::default(), Some(&json!("codex")), "codex");
        assert!(gather.roles.is_empty() && debate.roles.is_empty());
    }

    #[test]
    fn what_eval_sends_depends_on_the_harnesses_and_the_record() {
        let t = tiers(DEFAULTS);
        let (g, d) = resolve(&t, None, "claude-code");
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
        let split = json!({"gather": {"harness": "codex"}, "debate": {"harness": "claude-code",
                                                                     "drift": {"model": null}}});
        let (g, d) = resolve(&t, Some(&split), "claude-code");
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
        let (g, d) = resolve(&Tiers::default(), Some(&json!("codex")), "codex");
        assert_eq!(
            next_send(&g, &d, None),
            ("codex".into(), String::new()),
            "no roles: a bare eval"
        );
    }

    #[test]
    fn nothing_readable_is_no_tiers() {
        for text in ["", "not json", "[]", "null"] {
            assert_eq!(tiers(text), Tiers::default(), "{text:?}");
        }
    }
}
