//! `~/.fab7/weft/config.toml`: where each RingFrame act goes, how an Eval is
//! split across harnesses, and RingFrame's overrides.
//!
//! The top-level tables hold for every project; `[projects."<path>"]` holds
//! the same tables for one, and wins key by key. Weft ships no model tiers:
//! a role Weft sets nothing for runs on its plugin's own.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

use crate::eval_stages::{DEBATE_ROLES, role_now};
use crate::routing::{ACTS, Routing};

/// What `weft --serve` writes when there is no file, and never again.
pub const STARTING: &str = r#"# Weft: where each RingFrame act goes, how an Eval is split, and RingFrame
# overrides. Weft's Settings change it at once; an edit by hand applies when a
# project opens.

# notify = false               # no notification when an agent needs you
# suggest = false              # no dim suggestions from your own words in the message field
# turbo = true                 # agents run with every permission granted and no
#                              # question asked: each harness's `turbo` flag, from
#                              # its Weft harness file, which says what else it drops.

# [harnesses.<harness>]        # this machine: set from Weft's Updates
# program = "/path/to/it"      # where its program is, when it is not on PATH

# [remote]                     # Weft from your phone at weft.getfab7.com: Remote
# enabled = true               # set when you turn it on or off there
# tunnel = "cloudflared"       # the tunnel you picked there, by its tool file
# program = "/path/to/it"      # where its program is, when it is not on PATH

# [service]                    # Weft's own service on this machine: Start at login
# start_at_login = true        # start Weft when you log in, and again if it stops

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

/// `[remote]`: Weft from another device (remote.md D9). This machine's only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Remote {
    /// The daemon keeps `weft remote` running.
    pub enabled: bool,
    /// The tunnel adapter chosen: a tool file's name.
    pub tunnel: Option<String>,
    /// Where its program is, when it is not on `PATH`.
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
    /// `suggest = false`: no suggestions from the person's own words.
    unsuggested: bool,
    remote: Remote,
    /// `[service] start_at_login`: Weft starts when the person logs in. Off
    /// unless turned on; Remote never turns it on.
    start_at_login: bool,
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

    pub fn remote(&self) -> &Remote {
        &self.remote
    }

    /// Whether Weft's own service starts it at login.
    pub fn start_at_login(&self) -> bool {
        self.start_at_login
    }
}

/// `config.toml` with `[service] start_at_login` set, the rest as it was.
pub fn set_start_at_login(text: &str, on: bool) -> Result<String, String> {
    set(text, &["service"], "start_at_login", &Value::Bool(on))
}

/// `config.toml` with `[remote] enabled` set, the rest as it was.
pub fn set_remote(text: &str, enabled: bool) -> Result<String, String> {
    set(text, &["remote"], "enabled", &Value::Bool(enabled))
}

/// `config.toml` with `[remote] tunnel` set to the adapter chosen,
/// the rest as it was.
pub fn set_tunnel(text: &str, tunnel: &str) -> Result<String, String> {
    set(text, &["remote"], "tunnel", &Value::String(tunnel.into()))
}

/// `config.toml` with `[remote] program` set to where Weft installed the
/// tunnel's program, the rest as it was.
pub fn set_program(text: &str, path: &str) -> Result<String, String> {
    set(text, &["remote"], "program", &Value::String(path.into()))
}

/// `config.toml` with one key of `[harnesses.<harness>]` set, the rest of the
/// file as the person wrote it, comments included (see [`edit`]).
pub fn set_harness(text: &str, harness: &str, key: &str, value: &Value) -> Result<String, String> {
    set(text, &["harnesses", harness], key, value)
}

/// One key of the table at `section` set, as [`edit`] says.
fn set(text: &str, section: &[&str], key: &str, value: &Value) -> Result<String, String> {
    let path: Vec<&str> = section.iter().copied().chain([key]).collect();
    edit(text, &path, Some(value))
}

/// `config.toml` with the value at `path` (its tables, then the key) set, or
/// taken out when `value` is `None` along with any table that leaves empty and
/// that carries no comment of its own. Every other line stays as the person
/// wrote it, comments included; a table that is not there is added. The result
/// is read back before it is handed over: a value that did not land, or a file
/// this cannot edit safely, is an error, and the person edits it.
pub fn edit(text: &str, path: &[&str], value: Option<&Value>) -> Result<String, String> {
    let shown = path.join(".");
    let (key, tables) = path.split_last().ok_or("nothing to set")?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("config.toml could not be read ({e}); set `{shown}` by hand"))?;
    match value {
        Some(value) => {
            let mut table: &mut dyn toml_edit::TableLike = doc.as_table_mut();
            for part in tables {
                // A table that is not there goes at the end of the file.
                let mut implicit = toml_edit::Table::new();
                implicit.set_implicit(true);
                implicit.set_position(Some(isize::MAX));
                table = table
                    .entry(part)
                    .or_insert(toml_edit::Item::Table(implicit))
                    .as_table_like_mut()
                    .ok_or_else(|| {
                        format!(
                            "config.toml sets `{part}` some other way; set `{shown}` there by hand"
                        )
                    })?;
            }
            table.insert(key, toml_edit::Item::Value(toml_value(value)?));
        }
        None => take_out(doc.as_table_mut(), path),
    }
    let out = doc.to_string();
    let parsed: toml::Table = out.parse().map_err(|e| {
        format!("config.toml could not be edited safely ({e}); set `{shown}` by hand")
    })?;
    let parsed = serde_json::to_value(parsed).map_err(|e| e.to_string())?;
    let got = path.iter().try_fold(&parsed, |v, part| v.get(*part));
    if got != value {
        return Err(format!("config.toml sets `{shown}` some other way; set it there by hand"));
    }
    Ok(out)
}

/// A JSON value as TOML writes it: objects inline, so a key stays one line.
fn toml_value(value: &Value) -> Result<toml_edit::Value, String> {
    Ok(match value {
        Value::String(s) => s.as_str().into(),
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.into(),
            None => {
                n.as_f64().ok_or_else(|| format!("{n} is not a number config.toml keeps"))?.into()
            }
        },
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(toml_value(item)?);
            }
            toml_edit::Value::Array(array)
        }
        Value::Object(map) => {
            let mut table = toml_edit::InlineTable::new();
            for (k, v) in map {
                table.insert(k, toml_value(v)?);
            }
            toml_edit::Value::InlineTable(table)
        }
        Value::Null => return Err("config.toml has no null: leave the key out instead".into()),
    })
}

/// The key at `path` taken out, and each table on the way that it leaves empty,
/// unless the table's header carries a comment (the starting file's do).
fn take_out(table: &mut dyn toml_edit::TableLike, path: &[&str]) {
    let Some((first, rest)) = path.split_first() else { return };
    if rest.is_empty() {
        table.remove(first);
        return;
    }
    let Some(inner) = table.get_mut(first) else { return };
    let keep = match inner {
        toml_edit::Item::Table(t) => {
            take_out(t, rest);
            !t.is_empty() || commented(t)
        }
        toml_edit::Item::Value(toml_edit::Value::InlineTable(t)) => {
            take_out(t, rest);
            !t.is_empty()
        }
        _ => true,
    };
    if !keep {
        table.remove(first);
    }
}

fn commented(table: &toml_edit::Table) -> bool {
    let decor = table.decor();
    [decor.prefix(), decor.suffix()]
        .into_iter()
        .flatten()
        .any(|raw| raw.as_str().is_some_and(|s| s.contains('#')))
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

    /// Whether the message field shows suggestions from the person's own
    /// words. On unless one line turns it off.
    pub fn suggest(&self) -> bool {
        !self.unsuggested
    }

    /// Whether agents Weft starts run in turbo mode: the machine's `turbo`
    /// line, for every project, else off.
    pub fn turbo(&self, _root: &Path) -> bool {
        self.machine.turbo.unwrap_or(false)
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
                ("suggest", Value::Bool(on)) if at.is_empty() => self.unsuggested = !on,
                // Every project's only (weft-after-desktop ADR-0003 D4): a
                // project's own line is reported and not read.
                ("turbo", Value::Bool(on)) if at.is_empty() => out.turbo = Some(*on),
                ("projects", _) if at.is_empty() => {}
                ("remote", Value::Object(r)) if at.is_empty() => {
                    for (k, v) in r {
                        match (k.as_str(), v) {
                            ("enabled", Value::Bool(on)) => self.remote.enabled = *on,
                            ("tunnel", Value::String(t)) if !t.is_empty() => {
                                self.remote.tunnel = Some(t.clone())
                            }
                            // Emptied when another tunnel is chosen: none.
                            ("program", Value::String(p)) => {
                                self.remote.program = Some(p.clone()).filter(|p| !p.is_empty())
                            }
                            _ => self.ignored.push(format!("{here}.{k}")),
                        }
                    }
                }
                ("service", Value::Object(t)) if at.is_empty() => {
                    for (k, v) in t {
                        match (k.as_str(), v) {
                            ("start_at_login", Value::Bool(on)) => self.start_at_login = *on,
                            _ => self.ignored.push(format!("{here}.{k}")),
                        }
                    }
                }
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
        self.override_of(self.project(root).map(|p| &p.ringframe))
    }

    /// The machine's layer alone: what every project gets, before any
    /// project's own.
    pub fn machine_ringframe_override(&self) -> Option<String> {
        self.override_of(None)
    }

    fn override_of(&self, project: Option<&Map<String, Value>>) -> Option<String> {
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
    fn an_empty_program_is_none_and_not_a_mistake() {
        let c = read("[remote]\ntunnel = \"other\"\nprogram = \"\"\n", &Default::default());
        assert_eq!(c.remote().program, None);
        assert!(c.ignored.is_empty(), "{:?}", c.ignored);
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
        // A harness written inline is edited where it is; one that is not a
        // table at all is the person's to edit.
        let inline = "harnesses = { codex = { program = \"/x\" } }\n";
        let edited = set_harness(inline, "codex", "program", &json!("/y")).unwrap();
        assert_eq!(edited, "harnesses = { codex = { program = \"/y\" } }\n");
        assert!(set_harness("harnesses = \"codex\"\n", "codex", "program", &json!("/y")).is_err());
        assert_eq!(read_("[harnesses.nowhere]\nprogram = \"/x\"\n").unknown, ["harnesses.nowhere"]);
    }

    /// `[service] start_at_login`: Weft's own option, off
    /// unless turned on, and not a key of a project's; Remote never sets it.
    #[test]
    fn start_at_login_is_off_unless_turned_on() {
        assert!(!read_(STARTING).start_at_login(), "off in the starting file");
        let uncommented = STARTING
            .replace("# [service]", "[service]")
            .replace("# start_at_login = true", "start_at_login = true");
        assert!(read_(&uncommented).start_at_login(), "its own lines work uncommented");
        let on = set_start_at_login(STARTING, true).unwrap();
        assert!(read_(&on).start_at_login());
        assert!(!read_(&set_start_at_login(&on, false).unwrap()).start_at_login());
        assert!(!read_(&set_remote(STARTING, true).unwrap()).start_at_login());
        assert_eq!(read_("[service]\nat_boot = true\n").ignored, ["service.at_boot"]);
        assert_eq!(
            read_("[projects.\"/p\".service]\nstart_at_login = true\n").ignored,
            ["projects.\"/p\".service"]
        );
    }

    /// `[remote]`: off unless turned on, the tunnel chosen and where its
    /// program is when not on PATH, any other key reported and ignored (remote.md D9); turned on
    /// and off by editing the one line.
    #[test]
    fn remote_access_is_off_unless_turned_on() {
        assert_eq!(read_(STARTING).remote(), &Remote::default(), "off in the starting file");
        let uncommented = STARTING
            .replace("# [remote]", "[remote]")
            .replace("# enabled = true", "enabled = true");
        assert!(read_(&uncommented).remote().enabled, "its own lines work uncommented");
        let c = read_(
            "[remote]\nenabled = true\ntunnel = \"cloudflared\"\nprogram = \"/opt/cf\"\nrelay = \"x\"\n",
        );
        assert!(c.remote().enabled);
        assert_eq!(c.remote().tunnel.as_deref(), Some("cloudflared"));
        assert_eq!(c.remote().program.as_deref(), Some("/opt/cf"));
        assert_eq!(c.ignored, ["remote.relay"]);
        assert_eq!(
            read_("[projects.\"/p\".remote]\nenabled = true\n").ignored,
            ["projects.\"/p\".remote"]
        );

        let on = set_remote(STARTING, true).unwrap();
        assert!(read_(&on).remote().enabled);
        assert_eq!(
            on.lines().count(),
            STARTING.lines().count() + 3,
            "a blank line and the table, at the end"
        );
        let off = set_remote(&on, false).unwrap();
        assert!(!read_(&off).remote().enabled);
        assert_eq!(
            off.lines().filter(|l| l.starts_with("enabled =")).count(),
            1,
            "replaced in place"
        );

        // The tunnel chosen, and where its program is, beside `enabled`.
        let chosen = set_tunnel(&on, "cloudflared").unwrap();
        assert_eq!(read_(&chosen).remote().tunnel.as_deref(), Some("cloudflared"));
        let at = set_program(&chosen, "/home/me/.fab7/weft/bin/cloudflared").unwrap();
        assert_eq!(read_(&at).remote().tunnel.as_deref(), Some("cloudflared"), "the rest kept");
        assert_eq!(
            read_(&at).remote().program.as_deref(),
            Some("/home/me/.fab7/weft/bin/cloudflared")
        );
        assert!(read_(&at).remote().enabled, "the rest kept");
    }

    #[test]
    fn an_edit_keeps_every_comment_and_line_it_does_not_touch() {
        let text = "# mine\nturbo = true   # all of them\n\n[routing]   # who does what\nask = \"codex\"\n# eval = \"codex\"\n\n[eval.debate]\nharness = \"codex\"\n";
        let out = edit(text, &["routing", "seal"], Some(&json!("claude-code"))).unwrap();
        assert_eq!(
            out,
            "# mine\nturbo = true   # all of them\n\n[routing]   # who does what\nask = \"codex\"\nseal = \"claude-code\"\n# eval = \"codex\"\n\n[eval.debate]\nharness = \"codex\"\n"
        );
        let back = edit(&out, &["routing", "seal"], None).unwrap();
        assert_eq!(back, text, "taking it out again gives the file back");
        let numbers =
            edit(text, &["eval", "debate", "map"], Some(&json!({"effort": "high"}))).unwrap();
        assert!(
            numbers.ends_with("harness = \"codex\"\nmap = { effort = \"high\" }\n"),
            "{numbers}"
        );
    }

    #[test]
    fn a_project_s_ringframe_table_is_written_nested_and_read_back() {
        let entries = json!([{"id": "practice.kiss", "enabled": false, "tier": "situational"}]);
        let path = [
            "projects",
            "/work/thing",
            "ringframe",
            "deltas",
            "practices/software-development",
            "entries",
        ];
        let out = edit(STARTING, &path, Some(&entries)).unwrap();
        // The entry's keys keep serde_json's order, which a build may preserve.
        assert!(
            out.contains("\n[projects.\"/work/thing\".ringframe.deltas.\"practices/software-development\"]\nentries = [{ "),
            "{out}"
        );
        let added: Vec<&str> = out.lines().filter(|l| !STARTING.lines().any(|s| s == *l)).collect();
        assert_eq!(added.len(), 2, "only the table's two lines are new: {added:?}");
        let kept: Vec<&str> = out.lines().filter(|l| !added.contains(l) && !l.is_empty()).collect();
        let starting: Vec<&str> = STARTING.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(kept, starting, "every line of the starting file, in its order");
        let layers: Value =
            serde_json::from_str(&read_(&out).ringframe_override(Path::new(HERE)).unwrap())
                .unwrap();
        assert_eq!(layers[0]["layer"], "weft-project");
        assert_eq!(
            layers[0]["ringframe"]["deltas"]["practices/software-development"]["entries"],
            entries
        );
        let cap = edit(
            &out,
            &["ringframe", "deltas", "practices/software-development", "render", "core_cap"],
            Some(&json!(3)),
        )
        .unwrap();
        assert_eq!(
            read_(&cap).ringframe_override(Path::new("/elsewhere")).unwrap(),
            r#"[{"layer":"weft","ringframe":{"deltas":{"practices/software-development":{"render":{"core_cap":3}}}}}]"#
        );
    }

    #[test]
    fn taking_out_the_last_key_takes_out_the_tables_it_empties_but_not_a_commented_one() {
        let text = "[routing]   # who does what\n\n[projects.\"/p\".routing]\neval = \"codex\"\n";
        let out = edit(text, &["projects", "/p", "routing", "eval"], None).unwrap();
        assert_eq!(out, "[routing]   # who does what\n");
        assert_eq!(edit(text, &["routing", "nothing"], None).unwrap(), text, "nothing to take out");
        let inline = "[projects.\"/p\"]\nrouting = { eval = \"codex\" }\n";
        assert_eq!(edit(inline, &["projects", "/p", "routing", "eval"], None).unwrap(), "");
    }

    #[test]
    fn a_value_toml_cannot_hold_or_a_file_it_cannot_read_is_refused() {
        assert!(edit("", &["turbo"], Some(&Value::Null)).is_err());
        assert!(edit("not = toml = here", &["turbo"], Some(&json!(true))).is_err());
        assert!(edit("[routing]\n", &[], Some(&json!(true))).is_err());
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

    /// Suggestions in the message field: on unless one line turns them off.
    #[test]
    fn one_line_turns_suggestions_off() {
        assert!(read_(STARTING).suggest(), "on unless turned off");
        assert!(!read_("suggest = false\n").suggest());
        let uncommented = STARTING.replace("# suggest = false", "suggest = false");
        assert!(!read_(&uncommented).suggest());
        assert!(read_("suggest = true\n").suggest());
        let c = read_("suggest = \"no\"\n");
        assert!(c.suggest());
        assert_eq!(c.ignored, ["suggest"]);
    }

    /// Turbo mode: off unless turned on, for every project; a project's own
    /// line is reported and not read.
    #[test]
    fn turbo_is_off_unless_turned_on_and_is_the_machine_s_only() {
        let here = Path::new(HERE);
        assert!(!read_(STARTING).turbo(here), "off in the starting file");
        let uncommented = STARTING.replace("# turbo = true", "turbo = true");
        assert!(read_(&uncommented).turbo(here), "its own line works uncommented");
        let text = format!("turbo = true\n\n[projects.\"{HERE}\"]\nturbo = false\n");
        assert!(read_(&text).turbo(here), "a project's own line is not read");
        assert!(read_(&text).turbo(Path::new("/elsewhere")));
        assert_eq!(read_(&text).ignored, [format!("projects.\"{HERE}\".turbo")]);
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
eval = "loom"
"#,
        );
        assert_eq!(
            c.unknown,
            [
                "eval.debate.harness: aider",
                "projects.\"/work/thing\".routing.eval: loom",
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
