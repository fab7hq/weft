//! Which harness an Eval's readers run in, which model and effort each role
//! runs on, and how many readers run at once. The gather is Weft's own: it
//! opens the Eval through RingFrame's crate, which reads the change with Git
//! and records it as gathered. Each judge is a session of that harness Weft
//! starts; Weft calls no model and checks no model name — the harness owns
//! its catalogue. Weft ships no tiers: a role `config.toml` sets nothing for
//! is given RingFrame's own.

use serde_json::{Map, Value, json};

/// The debate's roles, in the order they run: the whole reading of the
/// change (with its re-reads and the requirements task), then the questions
/// for the person.
pub const DEBATE_ROLES: [&str; 2] = ["read", "question"];

/// The debate, resolved: where it runs, and what each role that has a setting
/// was asked to run on. A role with neither is left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub harness: String,
    pub roles: Vec<(String, Option<String>, Option<String>)>,
    /// The roles that run in a harness of their own, and that harness.
    pub hosts: Vec<(String, String)>,
    /// How many judges run at once.
    pub judges: usize,
}

impl Stage {
    /// The harness a task of `role` runs in: the role's own, else the
    /// debate's.
    pub fn harness_of(&self, role: &str) -> &str {
        self.hosts.iter().find(|(r, _)| r == role).map_or(&self.harness, |(_, h)| h)
    }

    /// Every harness the debate's judges run in, once each: the debate's,
    /// then each role's own in the order the roles run.
    pub fn harnesses(&self) -> Vec<&str> {
        let mut out = vec![self.harness.as_str()];
        for role in DEBATE_ROLES {
            let h = self.harness_of(role);
            if !out.contains(&h) {
                out.push(h);
            }
        }
        out
    }
}

/// How many judges run at once when the debate does not say.
pub const JUDGES: usize = 8;

/// The debate for this project: its harness is `[eval.debate] harness`, else
/// `routing.eval`, else `fallback`; each role is what `[eval.debate]` sets,
/// its own `harness` included.
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
    let hosts = DEBATE_ROLES
        .iter()
        .filter_map(|role| {
            let h = route?.get(*role)?.get("harness")?.as_str().filter(|h| !h.is_empty())?;
            Some((role.to_string(), h.to_string()))
        })
        .collect();
    let judges = route.and_then(|r| r.get("judges")).and_then(Value::as_u64);
    Stage { harness, roles, hosts, judges: judges.map_or(JUDGES, |n| n as usize) }
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
        let stages = json!({"debate": {"question": {"effort": "xhigh"}, "read": {"model": "m"}}});
        let d = resolve(Some(&stages), Some("codex"), "codex");
        assert_eq!(role(&d, "question"), Some((None, Some("xhigh".into()))));
        assert_eq!(role(&d, "read"), Some((Some("m".into()), None)));
        assert_eq!(
            agents(&d),
            Some(json!({"read": {"model": "m"}, "question": {"effort": "xhigh"}}))
        );
        assert_eq!(agents(&resolve(None, None, "codex")), None, "nothing set");
        let earlier = json!({"debate": {"map": {"model": "m"}, "confirm": {"effort": "high"}}});
        assert_eq!(agents(&resolve(Some(&earlier), None, "codex")), None, "earlier roles are gone");
    }

    /// A role may run in a harness of its own; one that names none runs in
    /// the debate's.
    #[test]
    fn a_role_runs_in_its_own_harness_else_the_debates() {
        let stages = json!({"debate": {"harness": "codex",
            "question": {"harness": "claude-code", "effort": "high"}}});
        let d = resolve(Some(&stages), None, "grok");
        assert_eq!(d.harness_of("read"), "codex");
        assert_eq!(d.harness_of("question"), "claude-code");
        assert_eq!(d.harnesses(), ["codex", "claude-code"]);
        assert_eq!(agents(&d), Some(json!({"question": {"effort": "high"}})), "no harness in it");
        assert_eq!(resolve(None, None, "codex").harnesses(), ["codex"]);
    }
}
