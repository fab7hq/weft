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

# notify = false               # no terminal notification when an agent needs you
# turbo = true                 # agents run with every permission granted and no
#                              # question asked: each harness's `turbo` flag, from
#                              # its RingFrame profile. Codex's also drops its sandbox.

[routing]                      # every project; each act optional
# ask  = "<harness>"          # a harness as its RingFrame profile names it
# eval = "<harness>"
# seal = "<harness>"

[eval.gather]                  # an Eval split across harnesses
# harness = "<harness>"
# context = { model = "<model>", effort = "low" }

[eval.debate]
# harness   = "<harness>"
# adversary = { model = "<model>", effort = "high" }

[ringframe]                    # RingFrame overrides, in RingFrame's keys
# [ringframe.deltas."practices/software-development"]
# ...

# [projects."/Users/me/work/thing"]      # one project: the same tables
# routing = { eval = "<harness>" }
"#;

/// The tables one scope holds, keeping only names Weft knows.
#[derive(Debug, Clone, Default, PartialEq)]
struct Tables {
    routing: Map<String, Value>,
    eval: Map<String, Value>,
    ringframe: Map<String, Value>,
    /// `turbo = true | false`, when this scope says.
    turbo: Option<bool>,
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
    /// `notify = false`: no terminal notification when an agent needs you.
    quiet: bool,
    /// The harnesses the profiles define, while the file is read.
    known: Vec<String>,
}

/// Parse the file. Nothing readable is nothing set, which is how Weft behaves
/// without one.
pub fn read(text: &str, known: &crate::harness::Harnesses) -> Config {
    let mut c =
        Config { known: known.iter().map(|h| h.name.clone()).collect(), ..Config::default() };
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
    c.known.clear();
    c
}

/// A harness Weft supports, or the name reported as unknown.
fn harness<'a>(name: &'a Value, at: &str, c: &mut Config) -> Option<&'a str> {
    match name.as_str() {
        Some(h) if c.known.iter().any(|k| k == h) => Some(h),
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
    /// Whether Weft tells the person, through the terminal, when an agent
    /// they are not looking at needs them. On unless one line turns it off.
    pub fn notify(&self) -> bool {
        !self.quiet
    }

    /// Whether agents Weft starts in the project at `root` run in turbo mode:
    /// the project's `turbo` line, else the machine's, else off.
    pub fn turbo(&self, root: &Path) -> bool {
        self.project(root).and_then(|p| p.turbo).or(self.machine.turbo).unwrap_or(false)
    }

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
                ("notify", Value::Bool(on)) if at.is_empty() => self.quiet = !on,
                ("turbo", Value::Bool(on)) => out.turbo = Some(*on),
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

    /// The `--override` value every `/rf:` command carries here, or `None`
    /// when neither layer overrides anything: the machine's `[ringframe]`,
    /// then the project's, each as it stands. A `'` is written as JSON's
    /// `\u0027`, so the value is always one single-quoted shell word.
    pub fn ringframe_override(&self, root: &Path) -> Option<String> {
        let project = self.project(root).map(|p| &p.ringframe);
        let layers: Vec<Value> =
            [("weft", Some(&self.machine.ringframe)), ("weft-project", project)]
                .into_iter()
                .filter_map(|(name, r)| {
                    r.filter(|r| !r.is_empty())
                        .map(|r| serde_json::json!({"layer": name, "ringframe": r}))
                })
                .collect();
        (!layers.is_empty()).then(|| Value::Array(layers).to_string().replace('\'', "\\u0027"))
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

    fn read_(text: &str) -> Config {
        read(text, &crate::harness::fixture::harnesses())
    }
    use serde_json::json;

    const HERE: &str = "/work/thing";

    fn at(text: &str) -> Routing {
        read_(text).routing(Path::new(HERE))
    }

    #[test]
    fn the_starting_file_sets_nothing() {
        assert_eq!(read_(STARTING), Config::default());
        assert_eq!(read_(STARTING).ringframe_override(Path::new(HERE)), None);
    }

    /// One line turns notifications off; absent, they are on.
    #[test]
    fn one_line_turns_notifications_off() {
        assert!(read_(STARTING).notify(), "on unless turned off");
        assert!(!read_("notify = false\n").notify());
        let uncommented = STARTING.replace("# notify = false", "notify = false");
        assert!(!read_(&uncommented).notify(), "the starting file's own line works uncommented");
        assert!(read_("notify = true\n").notify());
        let c = read_("notify = \"no\"\n");
        assert!(c.notify(), "a value that is not a yes or a no changes nothing");
        assert_eq!(c.ignored, ["notify"]);
    }

    /// Turbo mode: off unless turned on, for every project or for one, the
    /// project's line over the machine's.
    #[test]
    fn turbo_is_off_unless_turned_on_and_a_project_wins() {
        let here = Path::new(HERE);
        assert!(!read_(STARTING).turbo(here), "off in the starting file");
        let uncommented = STARTING.replace("# turbo = true", "turbo = true");
        assert!(read_(&uncommented).turbo(here), "its own line works uncommented");
        let text = format!("turbo = true\n\n[projects.\"{HERE}\"]\nturbo = false\n");
        assert!(!read_(&text).turbo(here), "the project's own line wins");
        assert!(read_(&text).turbo(Path::new("/elsewhere")), "the machine's holds elsewhere");
        let c = read_("turbo = \"yes\"\n");
        assert!(!c.turbo(here), "anything but a yes or a no changes nothing");
        assert_eq!(c.ignored, ["turbo"]);
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
        let other = read_(text).routing(Path::new("/work/other"));
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
        let c = read_(
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
            let c = read_(text);
            assert!(c.routing(Path::new(HERE)).is_empty(), "{text:?}");
            assert_eq!(c.ringframe_override(Path::new(HERE)), None, "{text:?}");
        }
    }

    const KISS: &str = r#"
[ringframe.deltas."practices/software-development"]
entries = [{ id = "practice.kiss", text = "Keep it plain." }]
"#;
    const MINE: &str = r#"
[projects."/work/thing".ringframe.deltas.codex]
entries = [{ id = "codex.native_plan.hand_back", status = "qualified" }]
"#;

    #[test]
    fn the_override_is_the_machine_layer_then_the_project_layer_byte_for_byte() {
        let machine = r#"[{"layer":"weft","ringframe":{"deltas":{"practices/software-development":{"entries":[{"id":"practice.kiss","text":"Keep it plain."}]}}}}]"#;
        let project = r#"[{"layer":"weft-project","ringframe":{"deltas":{"codex":{"entries":[{"id":"codex.native_plan.hand_back","status":"qualified"}]}}}}]"#;
        let here = Path::new(HERE);
        assert_eq!(read_(KISS).ringframe_override(here).as_deref(), Some(machine));
        assert_eq!(read_(MINE).ringframe_override(here).as_deref(), Some(project));
        let both = format!("{},{}", &machine[..machine.len() - 1], &project[1..]);
        assert_eq!(read_(&format!("{KISS}{MINE}")).ringframe_override(here), Some(both));
        assert_eq!(read_(MINE).ringframe_override(Path::new("/work/other")), None);
    }

    #[test]
    fn a_quote_in_an_override_cannot_end_the_shell_word() {
        let c = read_("[ringframe.deltas.codex]\nwhy = \"it's\"\n");
        let typed = c.ringframe_override(Path::new(HERE)).unwrap();
        assert!(!typed.contains('\''), "{typed}");
        let back: Value = serde_json::from_str(&typed).unwrap();
        assert_eq!(back[0]["ringframe"]["deltas"]["codex"]["why"], "it's");
    }
}
