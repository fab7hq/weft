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

use crate::eval_stages::{DEBATE_ROLES, role_now};
use crate::routing::{ACTS, Routing};

/// What `weft --serve` writes when there is no file, and never again.
pub const STARTING: &str = r#"# Weft: where each RingFrame act goes, how an Eval is split, and RingFrame
# overrides. Read when a project opens; restart Weft after editing.

# notify = false               # no terminal notification when an agent needs you
# turbo = true                 # agents run with every permission granted and no
#                              # question asked: each harness's `turbo` flag, from
#                              # its Weft harness file, which says what else it drops.

# [harnesses.<harness>]        # this machine: set from Weft's [U] view
# program = "/path/to/it"      # where its program is, when it is not on PATH

[routing]                      # every project; each act optional
# ask  = "<harness>"          # a harness, as its harness file is named
# eval = "<harness>"
# seal = "<harness>"

[eval.debate]                  # Weft gathers the Eval itself; the debate goes here
# harness   = "<harness>"
# adversary = { model = "<model>", effort = "high" }
# trace     = { model = "<model>", effort = "low" }   # also intent, coverage, confirm

[ringframe]                    # RingFrame overrides, in RingFrame's keys
# [ringframe.deltas."practices/software-development"]
# ...
# [ringframe.eval]             # what an Eval's judges must not do; replaces
# deny = ["Push to any remote."]   # RingFrame's own list whole, [] denies nothing

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

/// What the person said about one harness on this machine: where its
/// program is, when it is not on `PATH`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Choice {
    pub program: Option<String>,
}

/// The file as read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    /// `[harnesses.<id>]`, for this machine only.
    harnesses: BTreeMap<String, Choice>,
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
}

/// Parse the file. Nothing readable is nothing set, which is how Weft behaves
/// without one.
pub fn read(text: &str, known: &crate::harness::Harnesses) -> Config {
    let known: Vec<&str> = known.iter().map(|h| h.name.as_str()).collect();
    let mut c = Config::default();
    let parsed = text.parse::<toml::Table>().ok().and_then(|t| serde_json::to_value(t).ok());
    let Some(Value::Object(file)) = parsed else { return c };
    c.machine = c.tables(&file, "", &known);
    for (path, t) in file.get("projects").and_then(Value::as_object).into_iter().flatten() {
        let at = format!("projects.\"{path}\".");
        match t.as_object() {
            Some(t) => {
                let tables = c.tables(t, &at, &known);
                c.projects.insert(path.clone(), tables);
            }
            None => c.ignored.push(at.trim_end_matches('.').to_string()),
        }
    }
    c.unknown.sort();
    c.ignored.sort();
    c
}

impl Config {
    /// Where the person said this harness's program is, when they did.
    pub fn program(&self, harness: &str) -> Option<&str> {
        self.harnesses.get(harness).and_then(|c| c.program.as_deref())
    }
}

/// `config.toml` with one key of `[harnesses.<harness>]` set, the rest of the
/// file as the person wrote it, comments included: the section's line is
/// replaced, or added to the section, or the section added at the end. The
/// result is parsed before it is handed back; a file this cannot edit safely
/// (a harness set some other way, say) is an error, and the person edits it.
pub fn set_harness(text: &str, harness: &str, key: &str, value: &Value) -> Result<String, String> {
    let literal = match value {
        Value::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        Value::Bool(b) => b.to_string(),
        other => return Err(format!("{other} is not a value config.toml keeps here")),
    };
    let header = format!("[harnesses.{harness}]");
    let line = format!("{key} = {literal}");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let start = lines.iter().position(|l| l.trim() == header);
    match start {
        Some(i) => {
            let end = lines[i + 1..]
                .iter()
                .position(|l| l.trim_start().starts_with('['))
                .map_or(lines.len(), |n| i + 1 + n);
            let at = lines[i + 1..end].iter().position(|l| {
                l.trim_start().strip_prefix(key).is_some_and(|r| r.trim_start().starts_with('='))
            });
            match at {
                Some(n) => lines[i + 1 + n] = line,
                None => lines.insert(i + 1, line),
            }
        }
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header);
            lines.push(line);
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    let parsed = out.parse::<toml::Table>().map_err(|e| {
        format!("config.toml could not be edited safely ({e}); add `{key}` under [harnesses.{harness}] by hand")
    })?;
    let got = parsed.get("harnesses").and_then(|h| h.get(harness)).and_then(|h| h.get(key));
    let want = match value {
        Value::String(s) => toml::Value::String(s.clone()),
        Value::Bool(b) => toml::Value::Boolean(*b),
        _ => unreachable!(),
    };
    if got != Some(&want) {
        return Err(format!(
            "config.toml sets [harnesses.{harness}] some other way; set `{key}` there by hand"
        ));
    }
    Ok(out)
}

/// A harness Weft supports, or the name reported as unknown.
fn harness<'a>(name: &'a Value, at: &str, known: &[&str], c: &mut Config) -> Option<&'a str> {
    match name.as_str() {
        Some(h) if known.contains(&h) => Some(h),
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
    fn tables(&mut self, t: &Map<String, Value>, at: &str, known: &[&str]) -> Tables {
        let mut out = Tables::default();
        for (key, value) in t {
            let here = format!("{at}{key}");
            match (key.as_str(), value) {
                ("routing", Value::Object(acts)) => {
                    for (act, name) in acts {
                        let path = format!("{here}.{act}");
                        if !ACTS.contains(&act.as_str()) {
                            self.ignored.push(path);
                        } else if let Some(h) = harness(name, &path, known, self) {
                            out.routing.insert(act.clone(), Value::from(h));
                        }
                    }
                }
                ("eval", Value::Object(stages)) => {
                    for (stage, body) in stages {
                        let path = format!("{here}.{stage}");
                        // The gather is Weft's own now; only the debate is routed.
                        let roles: &[&str] = match stage.as_str() {
                            "debate" => &DEBATE_ROLES,
                            _ => {
                                self.ignored.push(path);
                                continue;
                            }
                        };
                        let kept = self.stage(body, roles, &path, known);
                        if !kept.is_empty() {
                            out.eval.insert(stage.clone(), Value::Object(kept));
                        }
                    }
                }
                ("ringframe", Value::Object(r)) => out.ringframe = r.clone(),
                ("notify", Value::Bool(on)) if at.is_empty() => self.quiet = !on,
                ("turbo", Value::Bool(on)) => out.turbo = Some(*on),
                ("projects", _) if at.is_empty() => {}
                ("harnesses", Value::Object(hs)) if at.is_empty() => {
                    for (name, body) in hs {
                        let path = format!("{here}.{name}");
                        if !known.contains(&name.as_str()) {
                            self.unknown.push(path);
                            continue;
                        }
                        let mut choice = Choice::default();
                        for (k, v) in body.as_object().into_iter().flatten() {
                            match (k.as_str(), v) {
                                ("program", Value::String(p)) if !p.is_empty() => {
                                    choice.program = Some(p.clone())
                                }
                                _ => self.ignored.push(format!("{path}.{k}")),
                            }
                        }
                        self.harnesses.insert(name.clone(), choice);
                    }
                }
                _ => self.ignored.push(here),
            }
        }
        out
    }

    /// A stage's `harness` and each of its roles' `model` and `effort`.
    fn stage(
        &mut self,
        body: &Value,
        roles: &[&str],
        at: &str,
        known: &[&str],
    ) -> Map<String, Value> {
        let mut kept = Map::new();
        let Some(body) = body.as_object() else {
            self.ignored.push(at.to_string());
            return kept;
        };
        for (key, value) in body {
            let path = format!("{at}.{key}");
            let key = &role_now(key).to_string();
            if key == "harness" {
                if let Some(h) = harness(value, &path, known, self) {
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
    fn a_harness_s_program_and_skip_are_this_machine_s_and_edited_in_place() {
        let text = "# mine\nturbo = true\n\n[harnesses.codex]\n# where it lives\n\n[routing]\neval = \"codex\"\n";
        assert_eq!(read_(text).program("codex"), None);
        // Added inside its section; every other line is as it was.
        let out = set_harness(text, "codex", "program", &json!("/opt/codex/bin/codex")).unwrap();
        assert_eq!(
            out,
            "# mine\nturbo = true\n\n[harnesses.codex]\nprogram = \"/opt/codex/bin/codex\"\n# where it lives\n\n[routing]\neval = \"codex\"\n"
        );
        assert_eq!(read_(&out).program("codex"), Some("/opt/codex/bin/codex"));
        // Replaced where it is; a section that is not there is added at the end.
        let again = set_harness(&out, "codex", "program", &json!("/usr/bin/codex")).unwrap();
        assert_eq!(read_(&again).program("codex"), Some("/usr/bin/codex"));
        assert_eq!(again.matches("program =").count(), 1);
        let added = set_harness(text, "claude-code", "program", &json!("/x/claude")).unwrap();
        assert!(added.ends_with("\n[harnesses.claude-code]\nprogram = \"/x/claude\"\n"), "{added}");
        assert_eq!(read_(&added).routing(Path::new(HERE)), read_(text).routing(Path::new(HERE)));
        // A harness set some other way is the person's to edit.
        let inline = "harnesses = { codex = { program = \"/x\" } }\n";
        assert!(set_harness(inline, "codex", "program", &json!("/y")).is_err());
        assert_eq!(read_("[harnesses.nowhere]\nprogram = \"/x\"\n").unknown, ["harnesses.nowhere"]);
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
        let r = at("[routing]\neval = \"claude-code\"\n\n[eval.debate]\nharness = \"codex\"\n");
        assert_eq!(r.get("eval"), Some("claude-code"));
        assert_eq!(r.eval_stages, Some(json!({"debate": {"harness": "codex"}})));
    }

    /// Weft gathers an Eval itself: a gather stage is no longer
    /// read, and says so, and the old `drift` is read as `map`.
    #[test]
    fn a_gather_stage_is_reported_and_drift_is_map() {
        let c = read_(
            "[eval.gather]\nharness = \"codex\"\n\n[eval.debate]\ndrift = { effort = \"high\" }\ncontext = { effort = \"low\" }\n",
        );
        assert_eq!(c.ignored, ["eval.debate.context", "eval.gather"]);
        let r = c.routing(Path::new(HERE));
        assert_eq!(r.eval_stages, Some(json!({"debate": {"map": {"effort": "high"}}})));
    }

    #[test]
    fn a_project_role_key_overrides_the_machines_key_by_key() {
        let text = r#"
[eval.debate]
harness = "claude-code"
confirm = { model = "claude-opus-5-5", effort = "high" }
drift = { effort = "high" }

[projects."/work/thing".eval.debate]
adversary = { effort = "xhigh" }
"#;
        assert_eq!(
            at(text).eval_stages,
            Some(json!({"debate": {
                "harness": "claude-code",
                "confirm": {"model": "claude-opus-5-5", "effort": "xhigh"},
                "map": {"effort": "high"}}}))
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
        assert_eq!(r.eval_stages, Some(json!({"debate": {"map": {"effort": "high"}}})));
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
