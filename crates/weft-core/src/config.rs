//! `~/.fab7/weft/config.toml`: where each RingFrame act goes, how an Eval is
//! split across harnesses, and RingFrame's overrides ([ADR-0012]).
//!
//! The top-level tables hold for every project; `[projects."<path>"]` holds
//! the same tables for one, and wins key by key. Weft ships no model tiers:
//! a role Weft sets nothing for runs on its plugin's own.
//!
//! [ADR-0012]: ../../../plans/weft/adr/0012-one-config-toml.md

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

use crate::eval_stages::{DEBATE_ROLES, GATHER_ROLES};
use crate::routing::{ACTS, Routing};

/// What `weft --serve` writes when there is no file, and never again.
pub const STARTING: &str = r#"# Weft: where each RingFrame act goes, how an Eval is split, and RingFrame
# overrides. Read when a project opens; restart Weft after editing.

[routing]                      # every project; each act optional
# ask  = "codex"
# eval = "claude-code"
# seal = "claude-code"

[eval.gather]                  # an Eval split across harnesses
# harness = "codex"
# context = { model = "gpt-6-luna", effort = "low" }

[eval.debate]
# harness   = "claude-code"
# adversary = { model = "claude-opus-5-5", effort = "high" }

[ringframe]                    # RingFrame overrides, in RingFrame's keys
# [ringframe.deltas."practices/software-development"]
# ...

# [projects."/Users/me/work/thing"]      # one project: the same tables
# routing = { eval = "codex" }
"#;

/// The tables one scope holds, keeping only names Weft knows.
#[derive(Debug, Clone, Default, PartialEq)]
struct Tables {
    routing: Map<String, Value>,
    eval: Map<String, Value>,
    ringframe: Map<String, Value>,
}

/// The file as read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    machine: Tables,
    projects: BTreeMap<String, Tables>,
    /// Harness names Weft does not know. What names one is not routed.
    pub unknown: Vec<String>,
    /// Tables, acts, stages, roles and keys Weft does not know.
    pub ignored: Vec<String>,
    /// The files this one replaced, found beside it and not read.
    pub leftover: Vec<String>,
}

/// Parse the file. Nothing readable is nothing set, which is how Weft behaves
/// without one.
pub fn read(text: &str) -> Config {
    let mut c = Config::default();
    let parsed = text.parse::<toml::Table>().ok().and_then(|t| serde_json::to_value(t).ok());
    let Some(Value::Object(file)) = parsed else { return c };
    c.machine = c.tables(&file, "");
    for (path, t) in file.get("projects").and_then(Value::as_object).into_iter().flatten() {
        let at = format!("projects.\"{path}\".");
        match t.as_object() {
            Some(t) => {
                let tables = c.tables(t, &at);
                c.projects.insert(path.clone(), tables);
            }
            None => c.ignored.push(at.trim_end_matches('.').to_string()),
        }
    }
    c.unknown.sort();
    c.ignored.sort();
    c
}

/// A harness Weft supports, or the name reported as unknown.
fn harness<'a>(name: &'a Value, at: &str, c: &mut Config) -> Option<&'a str> {
    match name.as_str() {
        Some(h) if crate::harness::find(h).is_some() => Some(h),
        Some(h) => {
            c.unknown.push(format!("{at}: {h}"));
            None
        }
        None => {
            c.ignored.push(at.to_string());
            None
        }
    }
}

impl Config {
    /// One scope's tables, keeping what Weft knows and reporting the rest.
    fn tables(&mut self, t: &Map<String, Value>, at: &str) -> Tables {
        let mut out = Tables::default();
        for (key, value) in t {
            let here = format!("{at}{key}");
            match (key.as_str(), value) {
                ("routing", Value::Object(acts)) => {
                    for (act, name) in acts {
                        let path = format!("{here}.{act}");
                        if !ACTS.contains(&act.as_str()) {
                            self.ignored.push(path);
                        } else if let Some(h) = harness(name, &path, self) {
                            out.routing.insert(act.clone(), Value::from(h));
                        }
                    }
                }
                ("eval", Value::Object(stages)) => {
                    for (stage, body) in stages {
                        let path = format!("{here}.{stage}");
                        let roles: &[&str] = match stage.as_str() {
                            "gather" => &GATHER_ROLES,
                            "debate" => &DEBATE_ROLES,
                            _ => {
                                self.ignored.push(path);
                                continue;
                            }
                        };
                        let kept = self.stage(body, roles, &path);
                        if !kept.is_empty() {
                            out.eval.insert(stage.clone(), Value::Object(kept));
                        }
                    }
                }
                ("ringframe", Value::Object(r)) => out.ringframe = r.clone(),
                ("projects", _) if at.is_empty() => {}
                _ => self.ignored.push(here),
            }
        }
        out
    }

    /// A stage's `harness` and each of its roles' `model` and `effort`.
    fn stage(&mut self, body: &Value, roles: &[&str], at: &str) -> Map<String, Value> {
        let mut kept = Map::new();
        let Some(body) = body.as_object() else {
            self.ignored.push(at.to_string());
            return kept;
        };
        for (key, value) in body {
            let path = format!("{at}.{key}");
            if key == "harness" {
                if let Some(h) = harness(value, &path, self) {
                    kept.insert(key.clone(), Value::from(h));
                }
            } else if !roles.contains(&key.as_str()) || !value.is_object() {
                self.ignored.push(path);
            } else {
                let mut settings = Map::new();
                for (k, v) in value.as_object().into_iter().flatten() {
                    if ["model", "effort"].contains(&k.as_str())
                        && v.as_str().is_some_and(|v| !v.is_empty())
                    {
                        settings.insert(k.clone(), v.clone());
                    } else {
                        self.ignored.push(format!("{path}.{k}"));
                    }
                }
                kept.insert(key.clone(), Value::Object(settings));
            }
        }
        kept
    }

    fn project(&self, root: &Path) -> Option<&Tables> {
        self.projects.get(root.to_string_lossy().as_ref())
    }

    /// Routing and Eval delegation for the project at `root`.
    pub fn routing(&self, root: &Path) -> Routing {
        let mut acts = self.machine.routing.clone();
        let mut eval = self.machine.eval.clone();
        if let Some(p) = self.project(root) {
            merge(&mut acts, &p.routing);
            merge(&mut eval, &p.eval);
        }
        Routing {
            by_act: acts
                .into_iter()
                .filter_map(|(act, h)| Some((act, h.as_str()?.to_string())))
                .collect(),
            unknown: self.unknown.clone(),
            ignored: self.ignored.clone(),
            leftover: self.leftover.clone(),
            eval_stages: (!eval.is_empty()).then_some(Value::Object(eval)),
        }
    }
}

/// `over`'s keys over `base`'s, table by table.
fn merge(base: &mut Map<String, Value>, over: &Map<String, Value>) {
    for (key, value) in over {
        match (base.get_mut(key), value) {
            (Some(Value::Object(b)), Value::Object(o)) => merge(b, o),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HERE: &str = "/work/thing";

    fn at(text: &str) -> Routing {
        read(text).routing(Path::new(HERE))
    }

    #[test]
    fn the_starting_file_sets_nothing() {
        assert_eq!(read(STARTING), Config::default());
    }

    #[test]
    fn the_machine_default_applies_to_every_project() {
        let r = at("[routing]\nask = \"codex\"\neval = \"claude-code\"\n");
        assert_eq!(r.each(), vec![("ask", "codex"), ("eval", "claude-code")]);
        assert_eq!(r.get("seal"), None);
    }

    #[test]
    fn a_project_overrides_one_act_and_keeps_the_rest() {
        let text = r#"
[routing]
ask = "codex"
eval = "claude-code"

[projects."/work/thing"]
routing = { eval = "codex" }

[projects."/work/other"]
routing = { ask = "claude-code" }
"#;
        assert_eq!(at(text).each(), vec![("ask", "codex"), ("eval", "codex")]);
        let other = read(text).routing(Path::new("/work/other"));
        assert_eq!(other.each(), vec![("ask", "claude-code"), ("eval", "claude-code")]);
    }

    #[test]
    fn a_stage_harness_is_kept_beside_routing_eval() {
        let r = at("[routing]\neval = \"claude-code\"\n\n[eval.gather]\nharness = \"codex\"\n");
        assert_eq!(r.get("eval"), Some("claude-code"));
        assert_eq!(r.eval_stages, Some(json!({"gather": {"harness": "codex"}})));
    }

    #[test]
    fn a_project_role_key_overrides_the_machines_key_by_key() {
        let text = r#"
[eval.debate]
harness = "claude-code"
adversary = { model = "claude-opus-5-5", effort = "high" }
drift = { effort = "high" }

[projects."/work/thing".eval.debate]
adversary = { effort = "xhigh" }
"#;
        assert_eq!(
            at(text).eval_stages,
            Some(json!({"debate": {
                "harness": "claude-code",
                "adversary": {"model": "claude-opus-5-5", "effort": "xhigh"},
                "drift": {"effort": "high"}}}))
        );
    }

    #[test]
    fn unknown_names_are_reported_once_and_ignored() {
        let c = read(
            r#"
colour = "blue"

[routing]
ask = "aider"
send = "codex"
seal = "codex"

[eval.review]
harness = "codex"

[eval.debate]
harness = "aider"
judge = { model = "x" }
drift = { effort = "high", temperature = "1" }

[projects."/work/thing".routing]
eval = "cursor"
"#,
        );
        assert_eq!(
            c.unknown,
            [
                "eval.debate.harness: aider",
                "projects.\"/work/thing\".routing.eval: cursor",
                "routing.ask: aider"
            ]
        );
        assert_eq!(
            c.ignored,
            [
                "colour",
                "eval.debate.drift.temperature",
                "eval.debate.judge",
                "eval.review",
                "routing.send"
            ]
        );
        let r = c.routing(Path::new(HERE));
        assert_eq!(r.each(), vec![("seal", "codex")], "and the rest still stands");
        assert_eq!(r.eval_stages, Some(json!({"debate": {"drift": {"effort": "high"}}})));
        assert_eq!(r.unknown, c.unknown);
        assert_eq!(r.ignored, c.ignored);
    }

    #[test]
    fn nothing_readable_sets_nothing_rather_than_failing() {
        for text in ["", "not toml", "routing = \"codex\"", "[[routing]]\nask = \"codex\"\n"] {
            let c = read(text);
            assert!(c.routing(Path::new(HERE)).is_empty(), "{text:?}");
        }
    }
}
