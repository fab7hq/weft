//! `ringframe` command line.
//!
//! Fetch commands accept `--minimal` for conversation context or `--json` for
//! complete observation. Actions return a concise result by default and accept
//! neither flag. Output choices never change stored records.
//!
//! Exit codes: 0 ok, 1 usage, 2 refused by a rule, 3 needs input, 4 internal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ask::{AskError, NeedsInput};
use crate::seal::SealError;
use crate::workspace::Workspace;
use crate::{VERSION, ask, deltas, evaluate, output, seal, store, workspace};

/// What one invocation produced. The tests drive this directly, so nothing
/// here touches the real streams.
pub struct Run {
    pub code: i32,
    pub out: String,
    pub err: String,
}

/// A usage error: exit 1, a message on stderr, nothing on stdout.
struct Usage(String);

impl Usage {
    fn new(message: impl std::fmt::Display) -> Self {
        Usage(format!("ringframe: error: {message}"))
    }
}

/// The flags one command accepts.
struct Spec {
    /// Flags taking one value.
    values: &'static [&'static str],
    bools: &'static [&'static str],
    required: &'static [&'static str],
    /// Whether `--json` and `--minimal` are accepted.
    fetch: bool,
    /// Flags that may be given more than once.
    repeated: &'static [&'static str],
}

const NONE: &[&str] = &[];

fn spec(cmd: &str, sub: Option<&str>) -> Option<Spec> {
    let s = |values, bools, required, fetch, repeated| {
        Some(Spec { values, bools, required, fetch, repeated })
    };
    match (cmd, sub) {
        ("init", None) => s(&["--from"], &["--global"], NONE, false, NONE),
        ("sync", None) => s(&["--from"], &["--check"], NONE, false, NONE),
        ("ask", Some("list")) => s(NONE, NONE, NONE, true, NONE),
        ("eval", Some("open")) => s(
            &[
                "--ask",
                "--anchor",
                "--subject-kind",
                "--subject-ref",
                "--agents",
                "--override",
                "--repo",
                "--host",
            ],
            NONE,
            NONE,
            false,
            &["--repo"],
        ),
        ("eval", Some("list")) => s(NONE, NONE, NONE, true, NONE),
        ("eval", Some("show")) => s(&["--eval"], &["--summary"], &["--eval"], false, NONE),
        ("eval", Some("window")) => {
            s(&["--eval", "--window", "--task"], NONE, &["--eval", "--window"], false, NONE)
        }
        ("eval", Some("task")) => s(&["--eval", "--role"], NONE, &["--eval"], false, NONE),
        ("eval", Some("submit")) => s(&["--task", "--file"], NONE, &["--task"], false, NONE),
        ("eval", Some("commits")) => s(&["--eval"], NONE, &["--eval"], false, NONE),
        ("eval", Some("checks")) => s(&["--eval"], NONE, &["--eval"], false, NONE),
        ("eval", Some("grep")) => {
            s(&["--eval", "--text"], NONE, &["--eval", "--text"], false, NONE)
        }
        ("eval", Some("void")) => {
            s(&["--eval", "--reason"], NONE, &["--eval", "--reason"], false, NONE)
        }
        ("seal", Some("create")) => {
            s(&["--disposition", "--eval", "--note"], NONE, &["--disposition"], false, NONE)
        }
        ("seal", Some("check")) => s(&["--seal"], NONE, &["--seal"], true, NONE),
        ("ledger", Some("verify")) => s(NONE, NONE, NONE, true, NONE),
        ("deltas", Some("domains")) => s(&["--override"], NONE, NONE, true, NONE),
        ("deltas", Some("list")) => s(&["--override"], NONE, NONE, true, NONE),
        ("observe", Some("prompt")) => s(
            &["--by", "--host", "--session", "--host-version"],
            &["--layers"],
            &["--by", "--host", "--session"],
            false,
            NONE,
        ),
        ("observe", Some("tool")) => s(
            &["--by", "--host", "--session", "--tool", "--outcome", "--command"],
            NONE,
            &["--by", "--host", "--session", "--tool", "--outcome"],
            false,
            NONE,
        ),
        _ => None,
    }
}

/// Every (command, subcommand) pair, for the help and for the tests that hold
/// the surface. The order is the order the help prints them in.
const SURFACE: [(&str, Option<&str>); 22] = [
    ("init", None),
    ("sync", None),
    ("ask", Some("list")),
    ("eval", Some("open")),
    ("eval", Some("list")),
    ("eval", Some("task")),
    ("eval", Some("submit")),
    ("eval", Some("show")),
    ("eval", Some("window")),
    ("eval", Some("commits")),
    ("eval", Some("checks")),
    ("eval", Some("grep")),
    ("eval", Some("void")),
    ("seal", Some("create")),
    ("seal", Some("check")),
    ("ledger", Some("verify")),
    ("deltas", Some("domains")),
    ("deltas", Some("list")),
    ("observe", Some("prompt")),
    ("observe", Some("tool")),
    // Listed last because they are not commands.
    ("--version", None),
    ("--help", None),
];

/// What each command is for, in one line.
fn purpose(cmd: &str, sub: Option<&str>) -> &'static str {
    match (cmd, sub) {
        ("init", None) => "prepare this project, or with --global the configuration home",
        ("sync", None) => {
            "replace the synced configuration; overrides arrive with --override. \
             --check only says whether a newer release is out"
        }
        ("ask", Some("list")) => "every compiled Ask, oldest first",
        ("eval", Some("open")) => {
            "write the facts-only brief over every open Ask, or --ask one's unit of work"
        }
        ("eval", Some("list")) => "every Eval, opened, completed or voided",
        ("eval", Some("show")) => {
            "the report of a completed Eval, eval.md, verbatim; --summary: its head, what each \
             section holds, and where the rest is"
        }
        ("eval", Some("window")) => {
            "one window of an Eval's change, as a judge reads it; --task records the read"
        }
        ("eval", Some("task")) => {
            "one ready task for a judge, of --role when given: its brief and where its answer goes"
        }
        ("eval", Some("submit")) => {
            "check a task's output (--file, or its output path), every citation against its \
             source; accepted, refused or failed"
        }
        ("eval", Some("commits")) => "each repository's commits over the Eval's change",
        ("eval", Some("checks")) => "run the Eval's checks once (eval open starts it)",
        ("eval", Some("grep")) => {
            "at most 20 lines of an Eval's windows and named documents that contain --text"
        }
        ("eval", Some("void")) => {
            "withdraw an Eval so nothing builds on it; a person does this, with --reason"
        }
        ("seal", Some("create")) => "record the decision that closes the open Asks",
        ("seal", Some("check")) => "re-verify a receipt and say whether the subject still matches",
        ("ledger", _) => "re-check every event and every published artifact",
        ("deltas", Some("domains")) => {
            "installed practice domains and whether a layer switches each on"
        }
        ("deltas", Some("list")) => {
            "every catalog's rules as the layers leave them, each with the layer that last set it"
        }
        ("observe", Some("prompt")) => {
            "a prompt --by someone saw submitted in --session; reads its bytes on stdin and \
             keeps them only for a RingFrame invocation; with --layers, a JSON list on stdin: \
             the prompt received, then the host's other records of it"
        }
        ("observe", Some("tool")) => {
            "a tool --by someone saw run in --session, --outcome succeeded|failed; the \
             profile's [observe] says what it means: a delivery, a tool fact, or nothing"
        }
        ("--version", None) => "print the version",
        ("--help", None) => "print this",
        _ => "",
    }
}

fn help() -> String {
    let mut out = String::new();
    out.push_str(
        "ringframe \u{2014} record what was asked, judge what was done, seal the decision.\n",
    );
    out.push_str(
        "\nusage: ringframe [--workspace DIR] [--actor KIND:ID] [--authority A] <command> [...]\n",
    );
    let mut last = "";
    for (cmd, sub) in SURFACE {
        if cmd != last {
            out.push('\n');
            last = cmd;
        }
        let name = match sub {
            Some(s) => format!("{cmd} {s}"),
            None => cmd.to_string(),
        };
        out.push_str(&format!("  {name:<20} {}\n", purpose(cmd, sub)));
    }
    out.push_str("\nFetch commands take --json or --minimal; actions take neither.\n");
    out.push_str("`ringframe <command> --help` lists that command's options.\n");
    out.push_str("\nExit codes: 0 ok, 1 usage, 2 refused by a rule, 3 needs input, 4 internal.\n");
    out
}

fn command_help(cmd: &str, sub: Option<&str>, spec: &Spec) -> String {
    let name = match sub {
        Some(s) => format!("{cmd} {s}"),
        None => cmd.to_string(),
    };
    let mut out = format!("usage: ringframe {name} [options]\n\n  {}\n\n", purpose(cmd, sub));
    for flag in spec.values {
        let mark = if spec.required.contains(flag) { " (required)" } else { "" };
        let more = if spec.repeated.contains(flag) { " (repeatable)" } else { "" };
        out.push_str(&format!("  {flag} VALUE{mark}{more}\n"));
    }
    for flag in spec.bools {
        out.push_str(&format!("  {flag}\n"));
    }
    if spec.fetch {
        out.push_str("  --json | --minimal\n");
    }
    out
}

/// Which commands take a subcommand at all.
fn takes_sub(cmd: &str) -> bool {
    matches!(cmd, "ask" | "eval" | "seal" | "ledger" | "deltas" | "observe")
}

const COMMANDS: [&str; 9] =
    ["init", "sync", "ask", "eval", "seal", "ledger", "deltas", "observe", "--version"];

struct Parsed {
    cmd: String,
    sub: Option<String>,
    values: BTreeMap<String, Vec<String>>,
    bools: Vec<String>,
    json: bool,
    minimal: bool,
    workspace: Option<PathBuf>,
    actor: Option<String>,
    authority: String,
    overrides: crate::config::Overrides,
}

impl Parsed {
    fn one(&self, flag: &str) -> Option<&str> {
        self.values.get(flag).and_then(|v| v.last()).map(String::as_str)
    }

    fn all(&self, flag: &str) -> Vec<String> {
        self.values.get(flag).cloned().unwrap_or_default()
    }

    fn has(&self, flag: &str) -> bool {
        self.bools.iter().any(|b| b == flag)
    }
}

/// Global options are accepted anywhere in the command line.
const GLOBALS: [&str; 3] = ["--workspace", "--actor", "--authority"];

fn parse(argv: &[String]) -> Result<Parsed, Usage> {
    let mut rest: Vec<String> = Vec::new();
    let mut out = Parsed {
        cmd: String::new(),
        sub: None,
        values: BTreeMap::new(),
        bools: Vec::new(),
        json: false,
        minimal: false,
        workspace: None,
        actor: None,
        authority: "interactive".into(),
        overrides: crate::config::Overrides::default(),
    };
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        if GLOBALS.contains(&a.as_str()) {
            let value = argv
                .get(i + 1)
                .ok_or_else(|| Usage::new(format!("argument {a}: expected one argument")))?;
            match a.as_str() {
                "--workspace" => out.workspace = Some(PathBuf::from(value)),
                "--actor" => out.actor = Some(value.clone()),
                _ => {
                    if !["interactive", "preauthorized"].contains(&value.as_str()) {
                        return Err(Usage::new(format!(
                            "argument --authority: invalid choice: '{value}' \
                             (choose from 'interactive', 'preauthorized')"
                        )));
                    }
                    out.authority = value.clone();
                }
            }
            i += 2;
            continue;
        }
        match a.as_str() {
            "--json" => out.json = true,
            "--minimal" => out.minimal = true,
            _ => rest.push(a.clone()),
        }
        i += 1;
    }
    if out.json && out.minimal {
        return Err(Usage::new("argument --minimal: not allowed with argument --json"));
    }
    let mut it = rest.into_iter();
    let Some(cmd) = it.next() else {
        return Err(Usage::new("the following arguments are required: cmd"));
    };
    if !COMMANDS.contains(&cmd.as_str()) {
        return Err(Usage::new(format!(
            "argument cmd: invalid choice: '{cmd}' (choose from {})",
            COMMANDS[..COMMANDS.len() - 1]
                .iter()
                .map(|c| format!("'{c}'"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    out.cmd = cmd;
    let mut words: Vec<String> = it.collect();
    if takes_sub(&out.cmd) {
        if words.is_empty() || words[0].starts_with("--") {
            return Err(Usage::new("the following arguments are required: sub"));
        }
        out.sub = Some(words.remove(0));
    }
    let Some(spec) = spec(&out.cmd, out.sub.as_deref()) else {
        return Err(Usage::new(format!(
            "argument sub: invalid choice: '{}'",
            out.sub.clone().unwrap_or_default()
        )));
    };
    if (out.json || out.minimal) && !spec.fetch {
        return Err(Usage::new(format!(
            "unrecognized arguments: {}",
            if out.json { "--json" } else { "--minimal" }
        )));
    }
    let mut i = 0;
    while i < words.len() {
        let flag = words[i].clone();
        if spec.bools.contains(&flag.as_str()) {
            out.bools.push(flag);
            i += 1;
            continue;
        }
        if !spec.values.contains(&flag.as_str()) {
            return Err(Usage::new(format!("unrecognized arguments: {flag}")));
        }
        let value = words
            .get(i + 1)
            .ok_or_else(|| Usage::new(format!("argument {flag}: expected one argument")))?;
        let slot = out.values.entry(flag.clone()).or_default();
        if !spec.repeated.contains(&flag.as_str()) {
            slot.clear();
        }
        slot.push(value.clone());
        i += 2;
    }
    let missing: Vec<&str> =
        spec.required.iter().copied().filter(|r| !out.values.contains_key(*r)).collect();
    if !missing.is_empty() {
        return Err(Usage::new(format!(
            "the following arguments are required: {}",
            missing.join(", ")
        )));
    }
    for (flag, allowed) in [
        ("--disposition", &seal::DISPOSITIONS[..]),
        ("--subject-kind", &evaluate::KINDS[..]),
        ("--outcome", &["succeeded", "failed"][..]),
    ] {
        if let Some(v) = out.one(flag)
            && !allowed.contains(&v)
        {
            return Err(Usage::new(format!(
                "argument {flag}: invalid choice: '{v}' (choose from {})",
                allowed.iter().map(|a| format!("'{a}'")).collect::<Vec<_>>().join(", ")
            )));
        }
    }
    Ok(out)
}

/// Inline JSON, or `@file`.
fn json_arg(text: &str) -> Result<Value, String> {
    let body = match text.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
        None => text.to_string(),
    };
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

/// `--agents`: each role's model and effort.
fn agents_arg(ns: &Parsed) -> Result<Option<Value>, String> {
    ns.one("--agents").map(json_arg).transpose()
}

fn actor_of(text: Option<&str>, authority: &str) -> Value {
    let text = text.unwrap_or("human:local-user");
    let (kind, id) = text.split_once(':').unwrap_or((text, ""));
    json!({"kind": kind, "id": if id.is_empty() { "local-user" } else { id },
           "authority": authority})
}

/// What `observe` answers in a repository with no workspace: nothing
/// recorded, and why. It never creates the workspace; `ringframe init` does.
fn not_initialized(ws: Workspace, key: &str) -> (Workspace, Outcome) {
    let out = json!({key: false, "reason": workspace::uninitialized(&ws)});
    (ws, Outcome::Ok(0, out))
}

/// Every outcome a command can end in, before it is rendered.
enum Outcome {
    Ok(i32, Value),
    UsageDetail(String),
    Needs(NeedsInput),
    Refused(Vec<String>),
    Error(String, String),
}

fn from_ask_error(e: AskError) -> Outcome {
    match e {
        AskError::Needs(n) => Outcome::Needs(n),
        AskError::Ledger(l) => Outcome::Error(l.code, l.detail),
        AskError::Workspace(w) => Outcome::Error(w.code, w.detail),
    }
}

fn from_seal_error(e: SealError) -> Outcome {
    match e {
        SealError::Refused(r) => Outcome::Refused(r.codes),
        SealError::Other(o) => from_ask_error(o),
    }
}

fn from_config_error(e: crate::config::ConfigError) -> Outcome {
    Outcome::Error("config".into(), e.0)
}

pub fn run(argv: &[String], read_stdin: &mut dyn FnMut() -> String) -> Run {
    // `--version` and `--help` win wherever they appear, as argparse's do.
    if argv.iter().any(|a| a == "--version") {
        return Run { code: 0, out: format!("ringframe {VERSION}\n"), err: String::new() };
    }
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        let words: Vec<&str> =
            argv.iter().map(String::as_str).filter(|a| !a.starts_with('-')).collect();
        let text = match words.first() {
            None => help(),
            Some(cmd) => {
                let sub = if takes_sub(cmd) { words.get(1).copied() } else { None };
                match spec(cmd, sub) {
                    Some(s) => command_help(cmd, sub, &s),
                    None => help(),
                }
            }
        };
        return Run { code: 0, out: text, err: String::new() };
    }
    let mut ns = match parse(argv) {
        Ok(ns) => ns,
        Err(Usage(message)) => {
            return Run { code: 1, out: String::new(), err: format!("{message}\n") };
        }
    };
    let ws = match workspace::resolve(None, ns.workspace.as_deref()) {
        Ok(ws) => ws,
        Err(e) => {
            return Run { code: 1, out: String::new(), err: format!("ringframe: error: {e}\n") };
        }
    };
    let sub = ns.sub.clone();
    let concise = ns.minimal || output::is_action(&ns.cmd, sub.as_deref());
    let mut err = String::new();
    if READS_CONFIG.contains(&(ns.cmd.as_str(), sub.as_deref())) {
        if let Some(text) = ns.one("--override") {
            match crate::config::Overrides::parse(text) {
                Ok(o) => ns.overrides = o,
                Err(e) => {
                    let body = json!({"error": "config.override", "detail": e.0});
                    return Run { code: 2, out: emit(&body, ns.json, concise), err };
                }
            }
        }
        let left = crate::config::leftover_folders(&ws);
        if !left.is_empty() {
            err = format!(
                "ringframe: limitation: override folders are not read; pass what they hold \
                 with --override: {}\n",
                left.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
            );
        }
    }
    let (ws, outcome) = dispatch(&ns, ws, read_stdin);
    let (code, body) = match outcome {
        Outcome::Ok(code, result) => {
            if result.is_string()
                && matches!(
                    (ns.cmd.as_str(), sub.as_deref()),
                    ("eval", Some("show")) | ("eval", Some("window")) | ("eval", Some("commits"))
                )
            {
                return Run {
                    code,
                    out: result.as_str().unwrap_or_default().to_string(),
                    err: String::new(),
                };
            }
            (code, output::project(&ns.cmd, sub.as_deref(), ns.minimal, &result, &ws))
        }
        Outcome::UsageDetail(detail) => (1, json!({"error": "usage", "detail": detail})),
        Outcome::Needs(n) => (3, json!({"needs_input": n.reason})),
        Outcome::Refused(codes) => (2, json!({"error": "seal.refused", "refusal_codes": codes})),
        Outcome::Error(code, detail) => (2, json!({"error": code, "detail": detail})),
    };
    Run { code, out: emit(&body, ns.json, concise), err }
}

/// The commands that read configuration, and so take `--override`.
const READS_CONFIG: [(&str, Option<&str>); 3] =
    [("deltas", Some("domains")), ("deltas", Some("list")), ("eval", Some("open"))];

fn emit(obj: &Value, as_json: bool, minimal: bool) -> String {
    let newline = |s: &str| if s.ends_with('\n') { s.to_string() } else { format!("{s}\n") };
    match obj.as_str() {
        // A command whose result is already text stays text.
        Some(text) if minimal || !as_json => newline(text),
        _ if minimal => format!("{}\n", serde_json::to_string(obj).expect("a value")),
        _ => format!("{}\n", serde_json::to_string_pretty(obj).expect("a value")),
    }
}

fn dispatch(
    ns: &Parsed,
    ws: Workspace,
    read_stdin: &mut dyn FnMut() -> String,
) -> (Workspace, Outcome) {
    let actor = actor_of(ns.actor.as_deref(), &ns.authority);
    let sub = ns.sub.as_deref();
    macro_rules! bail {
        ($e:expr) => {
            return (ws, $e)
        };
    }

    match (ns.cmd.as_str(), sub) {
        ("init", None) => {
            if ns.has("--global") {
                let source = ns.one("--from").map(PathBuf::from);
                return match workspace::install_config(source.as_deref()) {
                    Ok(v) => (ws, Outcome::Ok(0, v)),
                    Err(e) => (ws, Outcome::Error(e.code, e.detail)),
                };
            }
            if let Err(e) = workspace::require_git(&ws) {
                bail!(Outcome::Error(e.code, e.detail));
            }
            if let Err(e) = ws.ensure() {
                bail!(Outcome::Error("workspace.io".into(), e.to_string()));
            }
            let mut out = json!({
                "rf_dir": ws.rf_dir().to_string_lossy(), "rt_dir": ws.rf_dir().to_string_lossy()
            });
            for (k, v) in ws.describe().as_object().into_iter().flatten() {
                out[k] = v.clone();
            }
            (ws, Outcome::Ok(0, out))
        }
        ("sync", None) => {
            let source = ns.one("--from").map(PathBuf::from);
            let done = if ns.has("--check") {
                workspace::check_config()
            } else {
                workspace::install_config(source.as_deref())
            };
            match done {
                Ok(v) => (ws, Outcome::Ok(0, v)),
                Err(e) => (ws, Outcome::Error(e.code, e.detail)),
            }
        }
        ("ask", Some(_)) => match ask::list_asks(&ws) {
            Ok(asks) => (ws, Outcome::Ok(0, json!({"asks": asks}))),
            Err(e) => (ws, from_ask_error(e)),
        },
        ("deltas", Some(what)) => deltas_command(ns, ws, what),
        ("eval", Some("list")) => match evaluate::list_records(&ws) {
            Ok(evals) => (ws, Outcome::Ok(0, json!({"evals": evals}))),
            Err(e) => (ws, from_ask_error(e)),
        },
        ("eval", Some("open")) => {
            let agents = match agents_arg(ns) {
                Ok(v) => v,
                Err(e) => return (ws, Outcome::UsageDetail(e)),
            };
            let out = evaluate::open_eval(
                &ws,
                evaluate::Open {
                    anchor: ns.one("--anchor"),
                    subject_kind: ns.one("--subject-kind"),
                    subject_ref: ns.one("--subject-ref"),
                    actor: Some(actor),
                    agents,
                    repos: ns.all("--repo"),
                    host: ns.one("--host"),
                    ask: ns.one("--ask"),
                    deny: ns.overrides.eval_deny().map(|(from, rules)| (from, rules.to_vec())),
                },
            );
            match out {
                Ok(v) => (ws, Outcome::Ok(0, v)),
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        ("eval", Some("show")) => {
            let eval = ns.one("--eval").unwrap_or_default();
            match evaluate::show_eval(&ws, eval) {
                Ok(text) if ns.has("--summary") => {
                    (ws, Outcome::Ok(0, Value::String(evaluate::summary_of(&text, eval))))
                }
                Ok(text) => (ws, Outcome::Ok(0, Value::String(text))),
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        ("eval", Some("window")) => {
            let eval = ns.one("--eval").unwrap_or_default();
            let window = ns.one("--window").unwrap_or_default();
            let text = crate::evidence::window_text(&ws, eval, window).and_then(|t| {
                if let Some(task) = ns.one("--task") {
                    crate::tasks::log_read(&ws, task, window)?;
                }
                Ok(t)
            });
            match text {
                Ok(text) => (ws, Outcome::Ok(0, Value::String(text))),
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        ("eval", Some("task")) => {
            match crate::tasks::take(&ws, ns.one("--eval").unwrap_or_default(), ns.one("--role")) {
                Ok(v) => (ws, Outcome::Ok(0, v)),
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        ("eval", Some("submit")) => {
            let task = ns.one("--task").unwrap_or_default();
            let out = match ns.one("--file") {
                Some(file) => crate::tasks::submit_from(&ws, task, Path::new(file)),
                None => crate::tasks::submit(&ws, task),
            };
            match out {
                Ok((code, v)) => (ws, Outcome::Ok(code, v)),
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        ("eval", Some("commits")) => {
            match crate::tasks::commits(&ws, ns.one("--eval").unwrap_or_default()) {
                Ok(logs) => {
                    let text: String =
                        logs.iter().map(|(repo, log)| format!("## {repo}\n\n{log}\n")).collect();
                    (ws, Outcome::Ok(0, Value::String(text)))
                }
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        ("eval", Some("checks")) => {
            match crate::checks::run(&ws, ns.one("--eval").unwrap_or_default()) {
                Ok(()) => (ws, Outcome::Ok(0, json!({"state": "done"}))),
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        ("eval", Some("grep")) => {
            let eval = ns.one("--eval").unwrap_or_default();
            match crate::evidence::grep(&ws, eval, ns.one("--text").unwrap_or_default()) {
                Ok(lines) => (ws, Outcome::Ok(0, json!({"lines": lines}))),
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        ("eval", Some("void")) => {
            let out = evaluate::void_eval(
                &ws,
                ns.one("--eval").unwrap_or_default(),
                ns.one("--reason").unwrap_or_default(),
                Some(&actor),
            );
            match out {
                Ok(v) => (ws, Outcome::Ok(0, v)),
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        ("seal", Some("create")) => {
            let out = seal::create(
                &ws,
                ns.one("--disposition").unwrap_or_default(),
                ns.one("--eval"),
                None,
                ns.one("--note"),
                Some(&actor),
            );
            match out {
                Ok(v) => (ws, Outcome::Ok(0, v)),
                Err(e) => (ws, from_seal_error(e)),
            }
        }
        ("seal", Some("check")) => match seal::check(&ws, ns.one("--seal").unwrap_or_default()) {
            Ok(v) => {
                let code = if v["fresh"] == json!(true) { 0 } else { 2 };
                (ws, Outcome::Ok(code, v))
            }
            Err(e) => (ws, from_seal_error(e)),
        },
        ("ledger", Some("verify")) => match store::verify(&ws) {
            Ok(findings) => {
                let clean = findings.is_empty();
                (
                    ws,
                    Outcome::Ok(
                        if clean { 0 } else { 2 },
                        json!({"findings": findings, "clean": clean}),
                    ),
                )
            }
            Err(e) => (ws, Outcome::Error(e.code, e.detail)),
        },
        ("observe", Some(what)) => {
            if !ws.exists() {
                return not_initialized(ws, "recorded");
            }
            let one = |flag| ns.one(flag).unwrap_or_default();
            let (by, host, session) = (one("--by"), one("--host"), one("--session"));
            let out = if what == "prompt" && ns.has("--layers") {
                let Ok(layers) = serde_json::from_str::<Vec<String>>(&read_stdin()) else {
                    return (
                        ws,
                        Outcome::UsageDetail("--layers reads a JSON list of texts".into()),
                    );
                };
                crate::observe::layered(&ws, by, host, session, &layers, ns.one("--host-version"))
            } else if what == "prompt" {
                crate::observe::prompt(
                    &ws,
                    by,
                    host,
                    session,
                    &read_stdin(),
                    ns.one("--host-version"),
                )
            } else {
                crate::observe::tool(
                    &ws,
                    by,
                    host,
                    session,
                    one("--tool"),
                    one("--outcome"),
                    ns.one("--command"),
                )
            };
            match out {
                Ok(v) => (ws, Outcome::Ok(0, v)),
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        _ => (ws, Outcome::UsageDetail(format!("unknown command {}", ns.cmd))),
    }
}

fn deltas_command(ns: &Parsed, ws: Workspace, what: &str) -> (Workspace, Outcome) {
    macro_rules! config_try {
        ($e:expr) => {
            match $e {
                Ok(v) => v,
                Err(e) => return (ws, from_config_error(e)),
            }
        };
    }
    match what {
        "domains" => {
            let listed = config_try!(deltas::domains(&ns.overrides));
            (ws, Outcome::Ok(0, json!({"domains": listed})))
        }
        _ => {
            let listed = config_try!(deltas::list(&ns.overrides));
            let deny = ns.overrides.eval_deny().map(|(from, rules)| (from, rules.to_vec()));
            let judges = crate::tasks::judges(deny.as_ref());
            (ws, Outcome::Ok(0, json!({"catalogs": listed, "judges": judges})))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{commit, eval_bench, two_asks_and_work, with_config_home};

    struct Cli {
        root: PathBuf,
    }

    impl Cli {
        /// One invocation, with no stdin.
        fn go(&self, args: &[&str]) -> (i32, Value, String) {
            self.piped(args, "")
        }

        fn piped(&self, args: &[&str], stdin: &str) -> (i32, Value, String) {
            let mut argv: Vec<String> =
                vec!["--workspace".into(), self.root.to_string_lossy().to_string()];
            argv.extend(args.iter().map(|a| (*a).to_string()));
            let mut read = || stdin.to_string();
            let run = super::run(&argv, &mut read);
            let body =
                serde_json::from_str(&run.out).unwrap_or_else(|_| Value::String(run.out.clone()));
            (run.code, body, run.err)
        }
    }

    fn cli<T>(body: impl FnOnce(&Cli) -> T) -> T {
        eval_bench(|ws| body(&Cli { root: ws.root.clone() }))
    }

    #[test]
    fn init_makes_the_records_folder() {
        cli(|c| {
            let (code, out, _) = c.go(&["init"]);
            assert_eq!(code, 0);
            assert!(out["rf_dir"].as_str().unwrap().ends_with(".fab7/rf"));
            assert!(c.root.join(".fab7/rf/.gitignore").exists());
        });
    }

    /// RingFrame's CLI reads no hook: the commands that read a
    /// payload are gone, and `observe` names who saw what it records.
    #[test]
    fn no_command_reads_a_hook_payload_and_observe_names_its_observer() {
        cli(|c| {
            for gone in [
                &["sessions", "capture", "--host", "claude-code"][..],
                &["sessions", "turn", "--from-hook", "--host", "claude-code", "--event", "ready"],
                &["fact", "--from-hook", "--host", "claude-code", "--outcome", "succeeded"],
                &["ask", "delivery", "--from-hook", "--host", "claude-code"],
            ] {
                let (code, _, _) = c.piped(gone, r#"{"session_id":"s1","prompt":"x"}"#);
                assert_eq!(code, 1, "{gone:?} is gone");
            }
            for (cmd, sub) in SURFACE {
                if let Some(spec) = spec(cmd, sub) {
                    assert!(!spec.bools.contains(&"--from-hook"), "{cmd} {sub:?}");
                }
            }
            let (code, _, err) = c
                .piped(&["observe", "prompt", "--host", "claude-code", "--session", "s1"], "hello");
            assert_eq!(code, 1, "--by is required");
            assert!(err.contains("--by"), "{err}");
            let tool = ["observe", "tool", "--by", "weft", "--host", "claude-code"];
            let mut bad = tool.to_vec();
            bad.extend(["--session", "s1", "--tool", "Bash", "--outcome", "maybe"]);
            assert_eq!(c.go(&bad).0, 1, "an outcome is succeeded or failed");
        });
    }

    /// `observe` never creates a workspace: in a repository with none it says
    /// so, records nothing, and exits 0.
    #[test]
    fn observe_in_a_repository_with_no_record_records_nothing() {
        let repo = crate::testing::repo();
        let c = Cli { root: repo.path().to_path_buf() };
        let (code, out, _) = c.piped(
            &["observe", "prompt", "--by", "weft", "--host", "claude-code", "--session", "s1"],
            "/rf:ask fix login",
        );
        assert_eq!(code, 0);
        assert_eq!(out["recorded"], false);
        assert!(out["reason"].is_string(), "{out}");
        assert!(!repo.path().join(".fab7").exists());
    }

    /// `--layers`: stdin is a JSON list, the prompt received first; anything
    /// else is refused and records nothing.
    #[test]
    fn observe_prompt_takes_the_hosts_other_records_as_layers() {
        cli(|c| {
            let mut args = vec!["observe", "prompt", "--by", "weft", "--host", "claude-code"];
            args.extend(["--session", "s1", "--layers"]);
            let (code, out, _) = c.piped(&args, r#"["/rf:ask fix login", "fix login"]"#);
            assert_eq!((code, &out["recorded"]), (0, &json!(true)), "{out}");
            let kept = c.root.join(".fab7/rf/sessions/claude-code/s1/prompts.jsonl");
            let kept: Vec<Value> = std::fs::read_to_string(kept)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            assert_eq!(kept.len(), 1, "one prompt received");
            assert_eq!(
                kept[0]["sha256"],
                crate::digest::sha256_bytes(b"/rf:ask fix login"),
                "the first"
            );
            assert!(kept[0].get("prompt").is_none());
            let (code, _, _) = c.piped(&args, "/rf:ask fix login");
            assert_ne!(code, 0, "not a list");
            let (code, _, _) = c.piped(&args, r#"["a", 1]"#);
            assert_ne!(code, 0, "a list of texts only");
        });
    }

    #[test]
    fn an_observed_pasted_prompt_records_an_observed_submission() {
        cli(|c| {
            let ws = crate::workspace::resolve(Some(&c.root), None).unwrap();
            let id = crate::testing::confirm_ask(&ws, b"Fix login.\n")["ask_id"]
                .as_str()
                .unwrap()
                .to_string();
            let typed = crate::ask::prompt_text(&ws, &id).unwrap();
            let (code, cap, _) = c.piped(
                &["observe", "prompt", "--by", "weft", "--host", "claude-code", "--session", "c9"],
                &typed,
            );
            assert_eq!(code, 0);
            assert_eq!(cap["recorded"], true);
            assert_eq!(cap["submission"]["ask_id"], id);
            assert_eq!(cap["submission"]["state"], "observed");
            let (_, listed, _) = c.go(&["ask", "list", "--json"]);
            assert_eq!(listed["asks"][0]["submission"], "observed");
            assert_eq!(listed["asks"][0]["outcome"], "confirmed");
        });
    }

    #[test]
    fn an_observed_shell_command_is_a_fact() {
        eval_bench(|ws| {
            let c = Cli { root: ws.root.clone() };
            two_asks_and_work(ws);
            let fact = |command: &str| {
                c.go(&[
                    "observe",
                    "tool",
                    "--by",
                    "weft",
                    "--host",
                    "claude-code",
                    "--session",
                    "s9",
                    "--tool",
                    "Bash",
                    "--outcome",
                    "succeeded",
                    "--command",
                    command,
                ])
            };
            let (code, out, _) = fact("npm test");
            assert_eq!(code, 0, "{out}");
            assert_eq!(out["recorded"], true);
            assert_eq!(out["outcome"], "succeeded");
            let (code, out, _) = fact("ringframe ask list");
            assert_eq!(code, 0);
            assert_eq!(out["recorded"], false);
        });
    }

    #[test]
    fn a_person_voids_an_eval_over_the_cli() {
        eval_bench(|ws| {
            let c = Cli { root: ws.root.clone() };
            let o = crate::testing::opened(ws);
            let eval_id = o.out["eval_id"].as_str().unwrap().to_string();
            let (code, out, _) = c.go(&["eval", "show", "--eval", &eval_id]);
            assert_eq!((code, out["error"].clone()), (2, json!("eval.not_completed")));
            let (code, _, err) = c.go(&["eval", "void", "--eval", &eval_id]);
            assert_eq!(code, 1, "--reason is required: {err}");
            let (code, out, _) = c.go(&[
                "--actor",
                "agent:judge",
                "eval",
                "void",
                "--eval",
                &eval_id,
                "--reason",
                "x",
            ]);
            assert_eq!((code, out["error"].clone()), (2, json!("eval.void_needs_person")));
            let (code, out, _) =
                c.go(&["eval", "void", "--eval", &eval_id, "--reason", "opened by mistake"]);
            assert_eq!(code, 0);
            assert_eq!(
                out,
                json!({"eval_id": eval_id, "state": "voided", "reason": "opened by mistake"})
            );
            let (_, listed, _) = c.go(&["eval", "list", "--minimal"]);
            assert_eq!(listed["evals"][0]["state"], "voided");
            assert_eq!(listed["evals"][0]["voided"]["reason"], "opened by mistake");
        });
    }

    #[test]
    fn a_judge_takes_one_task_and_submits_its_answer_file_over_the_cli() {
        eval_bench(|ws| {
            let c = Cli { root: ws.root.clone() };
            two_asks_and_work(ws);
            let (_, opened, _) = c.go(&["eval", "open"]);
            let eval_id = opened["eval_id"].as_str().unwrap().to_string();
            let (code, got, _) = c.go(&["eval", "task", "--eval", &eval_id, "--role", "map"]);
            assert_eq!((code, got["state"].clone()), (0, json!("running")), "{got}");
            let task = got["task"]["task_id"].as_str().unwrap().to_string();
            assert!(Path::new(got["task"]["brief"].as_str().unwrap()).exists());
            assert!(got["task"]["answer"].as_str().unwrap().ends_with(&format!("out/{task}.json")));
            let file = ws.root.join("answer.json");
            let submit = |c: &Cli| {
                c.go(&["eval", "submit", "--task", &task, "--file", file.to_str().unwrap()])
            };
            let (code, out, _) = submit(&c);
            assert_eq!((code, out["error"].clone()), (2, json!("eval.no_output")), "{out}");
            std::fs::write(&file, "{}").unwrap();
            let (code, out, _) = submit(&c);
            assert_eq!((code, out["state"].clone()), (2, json!("refused")), "{out}");
            let (code, out, _) = c.go(&["eval", "task", "--eval", &eval_id, "--role", "judge"]);
            assert_eq!((code, out["error"].clone()), (2, json!("eval.role")), "{out}");
        });
    }

    #[test]
    fn an_eval_runs_by_tasks_over_the_cli() {
        eval_bench(|ws| {
            let c = Cli { root: ws.root.clone() };
            two_asks_and_work(ws);
            let (code, opened, _) = c.go(&["eval", "open"]);
            assert_eq!(code, 0, "{opened}");
            let eval_id = opened["eval_id"].as_str().unwrap().to_string();
            let (code, next, _) = c.go(&["eval", "task", "--eval", &eval_id]);
            assert_eq!(code, 0, "{next}");
            assert_eq!(next["state"], "running");
            let task = next["task"]["task_id"].as_str().unwrap().to_string();
            assert!(task.ends_with("~m1"), "{task}");
            // A judge's lookups.
            let ev: Value = serde_json::from_slice(
                &std::fs::read(ws.rf_dir().join(format!("evals/{eval_id}/evidence.json"))).unwrap(),
            )
            .unwrap();
            let w = ev["windows"][0]["id"].as_str().unwrap().to_string();
            let (code, text, _) =
                c.go(&["eval", "window", "--eval", &eval_id, "--window", &w, "--task", &task]);
            assert_eq!(code, 0);
            assert!(text.as_str().unwrap().starts_with("--- "));
            let reads = std::fs::read_to_string(
                ws.rf_dir().join(format!("tmp/eval-{eval_id}/reads.jsonl")),
            )
            .unwrap();
            assert!(reads.contains(&w));
            let (code, commits, _) = c.go(&["eval", "commits", "--eval", &eval_id]);
            assert_eq!(code, 0);
            assert!(commits.as_str().unwrap().starts_with("## ."), "{commits}");
            // No output yet, then a refused one.
            let (code, out, _) = c.go(&["eval", "submit", "--task", &task]);
            assert_eq!((code, out["error"].clone()), (2, json!("eval.no_output")));
            let path = ws.rf_dir().join(format!("tmp/eval-{eval_id}/out/{task}.json"));
            std::fs::write(&path, "{}").unwrap();
            let (code, out, _) = c.go(&["eval", "submit", "--task", &task]);
            assert_eq!(code, 2, "{out}");
            assert_eq!(out["state"], "refused");
            assert_eq!(out["left"], 2, "two more tries");
        });
    }

    #[test]
    fn eval_and_seal_over_the_cli() {
        eval_bench(|ws| {
            let c = Cli { root: ws.root.clone() };
            let (a, b, sha) = two_asks_and_work(ws);
            let (code, out, _) =
                c.go(&["eval", "open", "--agents", r#"{"drift":{"effort":"high"}}"#]);
            assert_eq!(code, 0);
            assert_eq!(out["subject"], "git_commit");
            assert!(out["brief_path"].as_str().unwrap().ends_with("brief.json"));
            assert!(out["changes_patch"].as_str().unwrap().ends_with("changes.patch"));
            assert_eq!(
                out["agents"],
                json!({"map": {"effort": "high"}}),
                "an earlier name is its role now"
            );
            let eval_id = out["eval_id"].as_str().unwrap().to_string();
            let brief: Value = serde_json::from_slice(
                &std::fs::read(ws.rf_dir().join(format!("evals/{eval_id}/brief.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(
                brief["asks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x["ask_id"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                [a.as_str(), b.as_str()]
            );
            assert_eq!(brief["subject"]["ref"], sha);
            // Judged task by task, as a judge Weft started would: nothing
            // done. The answers are written inside the repository, as a judge
            // may: they dirty the tree, so the Seal's subject is the worktree
            // rather than a commit, and a later change stops matching it.
            loop {
                let (code, got, _) = c.go(&["eval", "task", "--eval", &eval_id]);
                assert_eq!(code, 0, "{got}");
                if got["state"] == "done" {
                    break;
                }
                let task = got["task"]["task_id"].as_str().unwrap().to_string();
                crate::testing::answer_task_wrong(ws, &task);
                let answer = c.root.join(format!("{}.json", task.replace('~', "-")));
                std::fs::rename(got["task"]["answer"].as_str().unwrap(), &answer).unwrap();
                let answer = answer.to_string_lossy().to_string();
                let (code, out, _) = c.go(&["eval", "submit", "--task", &task, "--file", &answer]);
                assert_eq!(code, 0, "{out}");
            }
            let record: Value = serde_json::from_slice(
                &std::fs::read(ws.rf_dir().join(format!("evals/{eval_id}/record.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(record["verdict"], "drifted");

            let (_, listed, _) = c.go(&["eval", "list", "--json"]);
            assert_eq!(
                listed["evals"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e["verdict"].clone())
                    .collect::<Vec<_>>(),
                [json!("drifted")]
            );
            // The report is RingFrame's, printed as it wrote it.
            let (code, shown, _) = c.go(&["eval", "show", "--eval", &eval_id]);
            assert_eq!(code, 0);
            let md = std::fs::read_to_string(ws.rf_dir().join(format!("evals/{eval_id}/eval.md")))
                .unwrap();
            assert_eq!(shown, Value::String(md));

            let (code, receipt, _) = c.go(&[
                "seal",
                "create",
                "--disposition",
                "accepted",
                "--note",
                "shipping the drift knowingly",
            ]);
            assert_eq!(code, 0);
            assert_eq!(receipt["disposition"], "accepted");
            assert_eq!(receipt["eval"]["verdict"], "drifted");
            assert!(receipt["receipt_path"].as_str().unwrap().ends_with(".json"));
            let seal_id = receipt["seal_id"].as_str().unwrap().to_string();
            let stored: Value = serde_json::from_slice(
                &std::fs::read(ws.rf_dir().join(format!("seals/{seal_id}.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(stored["note"], "shipping the drift knowingly");

            let (code, out, _) = c.go(&["seal", "create", "--disposition", "accepted"]);
            assert_eq!(code, 2);
            assert_eq!(out["error"], "seal.refused");
            assert_eq!(out["refusal_codes"], json!(["seal.no_open_ask"]));
            let (code, out, _) = c.go(&["eval", "open"]);
            assert_eq!(code, 2);
            assert_eq!(out["error"], "eval.no_open_ask");

            let (code, out, _) = c.go(&["seal", "check", "--seal", &seal_id, "--json"]);
            assert_eq!(code, 0);
            assert_eq!(out["fresh"], true);
            assert_eq!(out["subject_matches"], true);
            commit(&ws.root, &[("README.md", Some("changed\n"))], "change");
            let (code, out, _) = c.go(&["seal", "check", "--seal", &seal_id, "--json"]);
            // A fact, not a refusal.
            assert_eq!(code, 0);
            assert_eq!(out["fresh"], true);
            assert_eq!(out["subject_matches"], false);
            let (code, out, _) = c.go(&["seal", "check", "--seal", "sel_nope", "--json"]);
            assert_eq!(code, 2);
            assert_eq!(out["fresh"], false);

            let (code, _, err) = c.go(&["seal", "create", "--disposition", "shipped"]);
            assert_eq!(code, 1);
            assert!(err.contains("invalid choice"), "{err}");
        });
    }

    #[test]
    fn every_command_is_listed_documented_and_reachable() {
        // The help is generated from one table and the parser reads another.
        // If they drift, a command exists that nothing can find, or the help
        // names one that does not run.
        for (cmd, sub) in SURFACE {
            if cmd.starts_with("--") {
                continue;
            }
            assert!(spec(cmd, sub).is_some(), "{cmd} {sub:?} is listed but has no options");
            assert!(!purpose(cmd, sub).is_empty(), "{cmd} {sub:?} has no one-line purpose");
            assert!(COMMANDS.contains(&cmd), "{cmd} is listed but is not a command");
            assert_eq!(
                takes_sub(cmd),
                sub.is_some(),
                "{cmd}: the help and the parser disagree about a subcommand"
            );
        }
        // And every command the help prints answers --help itself.
        let mut none = || String::new();
        for (cmd, sub) in SURFACE {
            let mut argv = vec![cmd.to_string()];
            if let Some(s) = sub {
                argv.push(s.to_string());
            }
            argv.push("--help".into());
            let run = super::run(&argv, &mut none);
            assert_eq!(run.code, 0, "{cmd} {sub:?} --help");
            assert!(!run.out.is_empty(), "{cmd} {sub:?} --help printed nothing");
        }
    }

    #[test]
    fn help_is_printed_for_the_whole_cli_and_for_one_command() {
        let mut none = || String::new();
        let whole = super::run(&["--help".to_string()], &mut none);
        assert_eq!(whole.code, 0);
        assert!(whole.out.contains("usage: ringframe"), "{}", whole.out);
        assert!(whole.out.contains("eval submit"));
        assert!(whole.out.contains("Exit codes: 0 ok, 1 usage"));
        // Eval runs only by tasks Weft hands its judges, and Weft asks and
        // confirms: nothing of eval/1, of a coordinator or of a harness's
        // skill is left.
        for gone in [
            "eval close",
            "eval context",
            "eval next",
            "--parallel",
            "--returned",
            "--agents-from",
            "profile show",
            "ask request",
            "ask compile",
            "ask confirm",
            "ask unanswered",
            "ask cancel",
            "ask submitted",
            "ask copy",
            "ask delivery",
            "ask preflight",
            "deltas render",
        ] {
            assert!(!whole.out.contains(gone), "{gone}: {}", whole.out);
        }
        let one = super::run(&["eval".to_string(), "task".into(), "--help".into()], &mut none);
        assert_eq!(one.code, 0);
        assert!(one.out.contains("usage: ringframe eval task"), "{}", one.out);
        assert!(one.out.contains("--eval VALUE (required)"), "{}", one.out);
    }

    #[test]
    fn the_version_is_the_rust_era() {
        let mut none = || String::new();
        let run = super::run(&["--version".to_string()], &mut none);
        assert_eq!(run.code, 0);
        assert_eq!(run.out, format!("ringframe {VERSION}\n"));
        assert!(!VERSION.starts_with("0.0."), "the Rust era starts at 0.1.0: {VERSION}");
    }

    #[test]
    fn empty_listings() {
        cli(|c| {
            assert_eq!(c.go(&["eval", "list", "--json"]).1, json!({"evals": []}));
            assert_eq!(c.go(&["ask", "list", "--json"]).1, json!({"asks": []}));
        });
    }

    #[test]
    fn init_makes_the_private_workspace_and_an_ask_needs_it() {
        with_config_home(|_| {
            let repo = crate::testing::repo();
            let c = Cli { root: repo.path().to_path_buf() };
            let ws = crate::workspace::resolve(Some(repo.path()), None).unwrap();
            let e = crate::ask::preflight(&ws).unwrap_err();
            assert!(e.to_string().contains("workspace.uninitialized"), "{e}");
            assert!(!repo.path().join(".fab7").exists());

            let (code, _, _) = c.go(&["init"]);
            assert_eq!(code, 0);
            let rf = repo.path().join(".fab7/rf");
            assert_eq!(std::fs::read_to_string(rf.join(".gitignore")).unwrap(), "*\n");
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(rf.metadata().unwrap().permissions().mode() & 0o777, 0o700);
            assert!(crate::ask::preflight(&ws).is_ok());
        });
    }

    #[test]
    fn global_init_mirrors_the_bundle_and_creates_no_overrides() {
        with_config_home(|home| {
            let repo = crate::testing::repo();
            let c = Cli { root: repo.path().to_path_buf() };
            let fixture = crate::testing::fixture_config();
            let (code, out, _) = c.go(&["init", "--global", "--from", &fixture.to_string_lossy()]);
            assert_eq!(code, 0);
            assert_eq!(out["revision"], "local");
            let root = home.join(".fab7/rf");
            let mut held: Vec<String> = std::fs::read_dir(&root)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            held.sort();
            assert_eq!(held, ["config"]);
            for rel in [
                "deltas/codex.toml",
                "deltas/claude-code.toml",
                "deltas/practices/software-development.toml",
            ] {
                assert_eq!(
                    std::fs::read(root.join("config").join(rel)).unwrap(),
                    std::fs::read(fixture.join(rel)).unwrap(),
                    "{rel}"
                );
            }
            // An edit to the mirror does not survive a sync.
            let edited = root.join("config/deltas/codex.toml");
            let before = std::fs::read_to_string(&edited).unwrap();
            std::fs::write(&edited, format!("{before}\n# scribbled on the mirror\n")).unwrap();
            let (code, _, err) = c.go(&["sync", "--from", &fixture.to_string_lossy()]);
            assert_eq!(code, 0, "{err}");
            assert_eq!(
                std::fs::read(&edited).unwrap(),
                std::fs::read(fixture.join("deltas/codex.toml")).unwrap()
            );
        });
    }

    #[test]
    fn sync_refuses_a_yaml_bundle_or_file_by_name_and_mirrors_toml() {
        with_config_home(|home| {
            let repo = crate::testing::repo();
            let c = Cli { root: repo.path().to_path_buf() };
            let product = crate::testing::tmp_dir();
            let source = product.path().join("config");
            crate::workspace::copy_tree(&crate::testing::fixture_config(), &source).unwrap();
            let from = source.to_string_lossy().to_string();
            let manifest = product.path().join("bundle.yaml");
            std::fs::write(&manifest, "schema: ringframe.bundle/2\n").unwrap();
            let (code, out, _) = c.go(&["sync", "--from", &from]);
            assert_eq!((code, out["error"].as_str()), (2, Some("config.yaml_found")), "{out}");
            let beside = product.path().canonicalize().unwrap();
            let want = format!(
                "{} is YAML; this release reads {}",
                beside.join("bundle.yaml").display(),
                beside.join("bundle.toml").display()
            );
            assert_eq!(out["detail"], want);

            std::fs::remove_file(&manifest).unwrap();
            std::fs::write(product.path().join("bundle.toml"), "schema = \"ringframe.bundle/2\"\n")
                .unwrap();
            let host = source.join("deltas/codex.toml");
            std::fs::rename(&host, host.with_extension("yaml")).unwrap();
            let (code, out, _) = c.go(&["sync", "--from", &from]);
            assert_eq!((code, out["error"].as_str()), (2, Some("config.yaml_found")), "{out}");
            assert!(out["detail"].as_str().unwrap().contains("deltas/codex.yaml is YAML"), "{out}");

            std::fs::rename(host.with_extension("yaml"), &host).unwrap();
            std::fs::write(&host, format!("{}# synced\n", std::fs::read_to_string(&host).unwrap()))
                .unwrap();
            let (code, _, err) = c.go(&["sync", "--from", &from]);
            assert_eq!(code, 0, "{err}");
            assert_eq!(
                std::fs::read(home.join(".fab7/rf/config/deltas/codex.toml")).unwrap(),
                std::fs::read(&host).unwrap()
            );
        });
    }

    #[test]
    fn git_is_required_at_the_earliest_command() {
        with_config_home(|_| {
            let dir = crate::testing::tmp_dir();
            let c = Cli { root: dir.path().to_path_buf() };
            let (code, out, _) = c.go(&["init"]);
            assert_eq!(code, 2);
            assert_eq!(out["error"], "workspace.no_git");
            assert!(out["detail"].as_str().unwrap().contains("git init"));
            assert!(!dir.path().join(".fab7").exists());

            crate::testing::run(&["git", "init", "-q", &dir.path().to_string_lossy()]);
            let (code, out, _) = c.go(&["init"]);
            assert_eq!(code, 2);
            assert_eq!(out["error"], "workspace.no_commit");
        });
    }

    #[test]
    fn deltas_domains_lists_the_base() {
        cli(|c| {
            let (code, out, _) = c.go(&["deltas", "domains", "--json"]);
            assert_eq!(code, 0);
            let base = out["domains"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["base"] == json!(true))
                .unwrap();
            assert_eq!(base["domain"], "software-development");
        });
    }

    // ---- output modes ------------------------------------------------------

    #[test]
    fn minimal_and_json_are_mutually_exclusive() {
        cli(|c| {
            let (code, _, err) = c.go(&["eval", "list", "--json", "--minimal"]);
            assert_eq!(code, 1);
            assert!(err.contains("not allowed with"), "{err}");
        });
    }

    #[test]
    fn minimal_emits_compact_json() {
        cli(|c| {
            let (_, out, _) = c.go(&["eval", "list", "--minimal"]);
            assert!(out.is_object());
            let (_, raw, _) = c.go(&["ask", "list", "--minimal"]);
            assert!(raw.is_object());
        });
    }

    #[test]
    fn a_directory_inside_someone_elses_repository_is_not_a_workspace() {
        with_config_home(|_| {
            // Git discovery walks up, so this used to work: the records landed
            // here and the anchor came from the repository above, which the
            // person never opened. Every Eval and Seal would have been against
            // a history this directory does not contain.
            let repo = crate::testing::repo();
            let project = repo.path().join("test");
            std::fs::create_dir_all(&project).unwrap();
            let c = Cli { root: project.clone() };

            let (code, out, _) = c.go(&["init"]);
            assert_eq!(code, 2);
            assert_eq!(out["error"], "workspace.not_repo_root");
            let said = out["detail"].as_str().unwrap();
            assert!(said.contains("git init"), "the way forward is named: {said}");
            assert!(
                said.contains(&repo.path().to_string_lossy().to_string()),
                "and so is the repository it found: {said}"
            );
            assert!(!project.join(".fab7").exists(), "and nothing was written");

            // Its own repository makes it its own workspace, and the records
            // stay in it.
            crate::testing::run(&["git", "init", "-q", &project.to_string_lossy()]);
            std::fs::write(project.join("README.md"), "fixture\n").unwrap();
            let p = project.to_string_lossy().to_string();
            crate::testing::run(&["git", "-C", &p, "add", "-A"]);
            crate::testing::run(&[
                "git",
                "-C",
                &p,
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "init",
            ]);
            assert_eq!(c.go(&["init"]).0, 0);
            let ws = crate::workspace::resolve(Some(&project), None).unwrap();
            let a = crate::testing::confirm_ask(&ws, b"Fix login.\n");
            assert_eq!(crate::ask::list_asks(&ws).unwrap()[0]["ask_id"], a["ask_id"]);
            assert!(!repo.path().join(".fab7").exists(), "the parent keeps nothing");
        });
    }

    /// A route rule made a candidate in a layer, as Weft's Harnesses writes
    /// one: listed as such, from that layer, and no longer in the prompt.
    #[test]
    fn a_route_rule_a_layer_makes_a_candidate_is_listed_so_and_left_out() {
        cli(|c| {
            let (_, out, _) = c.go(&["deltas", "list", "--json"]);
            let rule = out["catalogs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["catalog"] == "claude-code")
                .and_then(|c| c["entries"].as_array())
                .and_then(|e| {
                    e.iter()
                        .find(|e| e["status"] == "qualified" && e["capability"] == "native_plan")
                })
                .cloned()
                .expect("a qualified plan rule");
            let (id, label) = (rule["id"].as_str().unwrap(), rule["label"].as_str().unwrap());
            let over = json!([{"layer": "weft", "ringframe": {"deltas": {"claude-code":
                {"entries": [{"id": id, "status": "candidate"}]}}}}])
            .to_string();
            let (_, listed, _) = c.go(&["deltas", "list", "--json", "--override", &over]);
            let now = listed["catalogs"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["catalog"] == "claude-code")
                .flat_map(|c| c["entries"].as_array().unwrap().clone())
                .find(|e| e["id"] == id)
                .unwrap();
            assert_eq!((&now["status"], &now["source"]), (&json!("candidate"), &json!("weft")));
            let class = json!({"task": ["plan"], "result": "plan", "effects": ["read", "write"]});
            let route = |over: &crate::config::Overrides| -> Vec<String> {
                crate::rules::route_rules(over, "claude-code", "native_plan", &class)
                    .unwrap()
                    .iter()
                    .map(|r| r["label"].as_str().unwrap_or_default().to_string())
                    .collect()
            };
            let layered = crate::config::Overrides::parse(&over).unwrap();
            assert!(
                route(&Default::default()).contains(&label.to_string()),
                "shipped, it is in the prompt"
            );
            assert!(!route(&layered).contains(&label.to_string()), "a candidate is not");
        });
    }

    #[test]
    fn every_rule_is_listed_with_the_layer_that_last_set_it() {
        cli(|c| {
            let over = r#"[{"layer":"weft","ringframe":{"deltas":{"practices/software-development":{"render":{"core_cap":3},"entries":[{"id":"practice.kiss","enabled":false},{"id":"mine.plain","text":"Say it plainly.","applies_to":["implement"]}]}}}},{"layer":"weft-project","ringframe":{"deltas":{"practices/software-development":{"entries":[{"id":"practice.yagni","tier":"situational"}]}}}}]"#;
            let (code, out, err) = c.go(&["deltas", "list", "--json", "--override", over]);
            assert_eq!(code, 0, "{err}");
            let catalogs = out["catalogs"].as_array().unwrap();
            let sd = catalogs
                .iter()
                .find(|c| c["catalog"] == "practices/software-development")
                .expect("the base practice");
            assert_eq!(sd["render"]["core_cap"], 3);
            let entry = |id: &str| {
                sd["entries"].as_array().unwrap().iter().find(|e| e["id"] == id).unwrap().clone()
            };
            assert_eq!(entry("practice.kiss")["source"], "weft");
            assert_eq!(entry("practice.kiss")["enabled"], false);
            assert_eq!(entry("practice.yagni")["source"], "weft-project");
            assert_eq!(entry("practice.yagni")["tier"], "situational");
            assert_eq!(entry("practice.dry")["source"], "shipped");
            assert_eq!(entry("mine.plain")["source"], "weft");
            assert!(
                catalogs.iter().any(|c| c["catalog"] == "claude-code"
                    && c["entries"].as_array().is_some_and(|e| !e.is_empty())),
                "the hosts' route rules too"
            );
            assert_eq!(out["judges"]["from"], "default", "RingFrame's own list, said");
            assert!(out["judges"]["deny"].as_array().is_some_and(|d| !d.is_empty()));
            let over = r#"[{"layer":"weft","ringframe":{"eval":{"deny":["Push."]}}}]"#;
            let (_, out, _) = c.go(&["deltas", "list", "--json", "--override", over]);
            assert_eq!(out["judges"], json!({"deny": ["Push."], "from": "weft"}));
        });
    }

    #[test]
    fn a_rule_the_catalogs_cannot_hold_is_refused_by_the_listing() {
        cli(|c| {
            let over = r#"[{"layer":"weft","ringframe":{"deltas":{"practices/software-development":{"entries":[{"id":"mine.plain","enabled":true}]}}}}]"#;
            let (code, out, _) = c.go(&["deltas", "list", "--json", "--override", over]);
            assert_ne!(code, 0, "{out}");
            assert!(out.to_string().contains("mine.plain"), "{out}");
        });
    }

    #[test]
    fn a_malformed_override_is_refused_by_every_command_that_reads_configuration() {
        cli(|c| {
            for cmd in [&["deltas", "domains"][..], &["deltas", "list"], &["eval", "open"]] {
                let mut args = cmd.to_vec();
                args.extend(["--override", r#"[{"layer":"weft"}]"#]);
                let (code, out, _) = c.go(&args);
                assert_eq!(code, 2, "{cmd:?}: {out}");
                assert_eq!(out["error"], "config.override", "{cmd:?}");
                assert!(out["detail"].as_str().unwrap().contains("needs a \"ringframe\""), "{out}");
            }
        });
    }

    #[test]
    fn a_leftover_override_folder_is_reported_once_and_not_read() {
        cli(|c| {
            let ws = crate::workspace::resolve(Some(&c.root), None).unwrap();
            ws.ensure().unwrap();
            let project = ws.rf_dir().join("deltas");
            std::fs::create_dir_all(project.join("practices")).unwrap();
            std::fs::write(
                project.join("practices/software-development.toml"),
                "description = \"team_boundary\"\n",
            )
            .unwrap();
            let personal = crate::config::home().join("overrides");
            std::fs::create_dir_all(&personal).unwrap();
            let (code, out, err) = c.go(&["deltas", "domains", "--json"]);
            assert_eq!(code, 0);
            assert_eq!(
                err,
                format!(
                    "ringframe: limitation: override folders are not read; pass what they \
                     hold with --override: {}, {}\n",
                    personal.display(),
                    project.display()
                )
            );
            let base = out["domains"].as_array().unwrap().iter().find(|d| d["base"] == true);
            assert_ne!(base.unwrap()["description"], "team_boundary");
            // Only the commands that read configuration say so.
            let (_, _, err) = c.go(&["ask", "list"]);
            assert_eq!(err, "");
        });
    }
}

/// The marketplace's side of the contract, checked against this binary.
///
/// `fab7` is synced, not built, and its skills call `ringframe` by name. If a
/// command they use stops existing, a wording fix in the marketplace becomes a
/// broken install for everyone.
#[cfg(test)]
mod the_marketplace {
    /// RingFrame raises nothing in a harness: its plugins hold their hooks
    /// and nothing else, no skill and no agent.
    #[test]
    fn the_plugins_hold_only_their_hooks() {
        let plugins = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../fab7/products/ringframe/plugins");
        let Ok(plugins) = plugins.canonicalize() else {
            return; // not checked out here
        };
        let mut stack = vec![plugins];
        let mut hooks = 0;
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                assert!(
                    matches!(name, "hooks.json" | "weft-signal" | "plugin.json"),
                    "{} is not a hook",
                    path.display()
                );
                hooks += usize::from(name == "hooks.json");
            }
        }
        assert_eq!(hooks, 4, "one hooks file per harness");
    }
}
