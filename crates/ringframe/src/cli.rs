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
use crate::{VERSION, ask, deltas, evaluate, output, profiles, seal, sessions, store, workspace};

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
        ("profile", Some("show")) => {
            s(&["--host", "--version", "--override"], NONE, &["--host"], true, NONE)
        }
        ("ask", Some("compile")) => s(
            &[
                "--staged",
                "--title",
                "--capability",
                "--classification",
                "--route",
                "--host",
                "--link",
                "--limitation",
                "--override",
            ],
            NONE,
            &["--staged", "--title", "--capability", "--classification", "--route", "--host"],
            false,
            &["--link", "--limitation"],
        ),
        ("ask", Some("confirm")) => s(&["--ask"], NONE, &["--ask"], false, NONE),
        ("ask", Some("unanswered")) => s(&["--ask", "--reason"], NONE, &["--ask"], false, NONE),
        ("ask", Some("cancel")) => {
            s(&["--ask", "--reason"], &["--attributed"], &["--ask"], false, NONE)
        }
        ("ask", Some("submitted")) => s(&["--ask"], &["--as-modified"], &["--ask"], false, NONE),
        ("ask", Some("copy")) => s(&["--ask"], &["--body"], &["--ask"], false, NONE),
        ("ask", Some("delivery")) => {
            s(&["--ask", "--state", "--reason"], &["--from-hook", "--handoff"], NONE, false, NONE)
        }
        ("ask", Some("list")) => s(NONE, NONE, NONE, true, NONE),
        ("ask", Some("preflight")) => s(NONE, NONE, NONE, false, NONE),
        ("eval", Some("open")) => s(
            &["--anchor", "--subject-kind", "--subject-ref", "--agents", "--override"],
            NONE,
            NONE,
            false,
            NONE,
        ),
        ("eval", Some("close")) => s(
            &["--eval", "--intent", "--judgement", "--agents"],
            NONE,
            &["--eval", "--intent", "--judgement"],
            false,
            &["--judgement"],
        ),
        ("eval", Some("context")) => {
            s(&["--eval", "--map", "--host"], NONE, &["--eval", "--map", "--host"], false, NONE)
        }
        ("eval", Some("list")) => s(NONE, NONE, NONE, true, NONE),
        ("seal", Some("create")) => {
            s(&["--disposition", "--eval", "--note"], NONE, &["--disposition"], false, NONE)
        }
        ("seal", Some("check")) => s(&["--seal"], NONE, &["--seal"], true, NONE),
        ("ledger", Some("verify")) => s(NONE, NONE, NONE, true, NONE),
        ("deltas", Some("domains")) => s(&["--override"], NONE, NONE, true, NONE),
        ("deltas", Some("render")) => s(
            &[
                "--host",
                "--host-version",
                "--capability",
                "--classification",
                "--statuses",
                "--override",
            ],
            NONE,
            &["--host", "--capability", "--classification"],
            true,
            NONE,
        ),
        ("sessions", Some("capture")) => {
            s(&["--host", "--host-version"], NONE, &["--host"], false, NONE)
        }
        ("fact", None) => s(&["--host"], &["--from-hook"], &["--host"], false, NONE),
        _ => None,
    }
}

/// Every (command, subcommand) pair, for the help and for the tests that hold
/// the surface. The order is the order the help prints them in.
const SURFACE: [(&str, Option<&str>); 25] = [
    ("init", None),
    ("sync", None),
    ("profile", Some("show")),
    ("ask", Some("compile")),
    ("ask", Some("confirm")),
    ("ask", Some("unanswered")),
    ("ask", Some("cancel")),
    ("ask", Some("submitted")),
    ("ask", Some("copy")),
    ("ask", Some("delivery")),
    ("ask", Some("list")),
    ("ask", Some("preflight")),
    ("eval", Some("open")),
    ("eval", Some("context")),
    ("eval", Some("close")),
    ("eval", Some("list")),
    ("seal", Some("create")),
    ("seal", Some("check")),
    ("ledger", Some("verify")),
    ("deltas", Some("domains")),
    ("deltas", Some("render")),
    ("sessions", Some("capture")),
    ("fact", None),
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
        ("profile", _) => "the host's capabilities and how each one has to be delivered",
        ("ask", Some("compile")) => {
            "persist a staged intent and its prompt; the only Ask that writes artifacts"
        }
        ("ask", Some("confirm")) => "record the chooser's answer",
        ("ask", Some("unanswered")) => {
            "the confirmation surface gave no answer; the Ask stays open"
        }
        ("ask", Some("cancel")) => "record that the person said no",
        ("ask", Some("submitted")) => "the person attests they submitted the prompt",
        ("ask", Some("copy")) => "the compiled prompt, verbatim; --body drops its command prefix",
        ("ask", Some("delivery")) => "record how the prompt reached the host, or emit the handoff",
        ("ask", Some("list")) => "every compiled Ask, oldest first",
        ("ask", Some("preflight")) => "refuse early what compile would refuse at the end",
        ("eval", Some("open")) => "write the facts-only brief over every open Ask",
        ("eval", Some("context")) => {
            "publish the change map (--map @file) for an open Eval, so a debate in any harness reads it"
        }
        ("eval", Some("close")) => "aggregate the intent and the judgements into a verdict",
        ("eval", Some("list")) => "every Eval, opened or completed",
        ("seal", Some("create")) => "record the decision that closes the open Asks",
        ("seal", Some("check")) => "re-verify a receipt and say whether the subject still matches",
        ("ledger", _) => "re-check every event and every published artifact",
        ("deltas", Some("domains")) => "installed practice domains and their concern vocabularies",
        ("deltas", Some("render")) => "the directives that apply to one classification",
        ("sessions", Some("capture")) => {
            "store a hook's prompt payload; reads the payload on stdin"
        }
        ("fact", None) => {
            "--from-hook: record whether the agent's shell command succeeded, against the \
             subject; reads a post-tool hook payload on stdin and never its output"
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
    if (cmd, sub) == ("ask", Some("compile")) {
        out.push_str(
            "\n  --link is revises:<ask_id>, follows:<ask_id|evl_id> or remediates:<evl_id>.\n  \
             A remedy's prompt starts by pointing at that Eval's eval.md. Any other link is\n  \
             refused (ask.link_rel_unknown), and so is one to nothing here (ask.link_unknown).\n",
        );
    }
    out
}

/// Which commands take a subcommand at all.
fn takes_sub(cmd: &str) -> bool {
    matches!(cmd, "profile" | "ask" | "eval" | "seal" | "ledger" | "deltas" | "sessions")
}

const COMMANDS: [&str; 11] = [
    "init",
    "sync",
    "profile",
    "ask",
    "eval",
    "seal",
    "ledger",
    "deltas",
    "sessions",
    "fact",
    "--version",
];

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
        ("--state", &["delivery_failed", "unavailable"][..]),
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

fn actor_of(text: Option<&str>, authority: &str) -> Value {
    let text = text.unwrap_or("human:local-user");
    let (kind, id) = text.split_once(':').unwrap_or((text, ""));
    json!({"kind": kind, "id": if id.is_empty() { "local-user" } else { id },
           "authority": authority})
}

fn links_of(items: &[String]) -> Vec<Value> {
    items
        .iter()
        .map(|item| {
            let (rel, id) = item.split_once(':').unwrap_or((item.as_str(), ""));
            json!({"rel": rel, "id": id})
        })
        .collect()
}

/// A hook payload names the project it fired in; without an explicit
/// `--workspace` that is the workspace, not wherever the hook process happens
/// to be.
fn hook_workspace(
    explicit: Option<&PathBuf>,
    ws: Workspace,
    payload: &Value,
) -> Result<Workspace, String> {
    let cwd = payload.get("cwd").and_then(Value::as_str).unwrap_or_default();
    if explicit.is_none() && !cwd.is_empty() {
        let path = Path::new(cwd);
        if !path.is_absolute() || !path.is_dir() {
            return Err("hook cwd must be an existing absolute directory".into());
        }
        return workspace::resolve(Some(path), None).map_err(|e| e.to_string());
    }
    Ok(ws)
}

/// Every outcome a command can end in, before it is rendered.
enum Outcome {
    Ok(i32, Value),
    /// A command whose result is text rather than a document.
    Text(String),
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
    // `profile show --version` names a host version, so it is not one of them.
    let naming_a_host = argv.iter().any(|a| a == "show");
    if argv.iter().any(|a| a == "--version") && !naming_a_host {
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
                "ringframe: limitation: override folders are not read; move them into Weft's \
                 config.toml [ringframe]: {}\n",
                left.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
            );
        }
    }
    let (ws, outcome) = dispatch(&ns, ws, read_stdin);
    let (code, body) = match outcome {
        Outcome::Ok(code, result) => {
            if (ns.cmd.as_str(), sub.as_deref()) == ("ask", Some("copy")) {
                return Run {
                    code,
                    out: result.as_str().unwrap_or_default().to_string(),
                    err: String::new(),
                };
            }
            (code, output::project(&ns.cmd, sub.as_deref(), ns.minimal, &result, &ws))
        }
        Outcome::Text(text) => (0, Value::String(text)),
        Outcome::UsageDetail(detail) => (1, json!({"error": "usage", "detail": detail})),
        Outcome::Needs(n) => (3, json!({"needs_input": n.reason})),
        Outcome::Refused(codes) => (2, json!({"error": "seal.refused", "refusal_codes": codes})),
        Outcome::Error(code, detail) => (2, json!({"error": code, "detail": detail})),
    };
    Run { code, out: emit(&body, ns.json, concise), err }
}

/// The commands that read configuration, and so take `--override`.
const READS_CONFIG: [(&str, Option<&str>); 5] = [
    ("profile", Some("show")),
    ("deltas", Some("domains")),
    ("deltas", Some("render")),
    ("ask", Some("compile")),
    ("eval", Some("open")),
];

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
    macro_rules! json_try {
        ($flag:expr) => {
            match json_arg(ns.one($flag).unwrap_or_default()) {
                Ok(v) => v,
                Err(e) => bail!(Outcome::UsageDetail(e)),
            }
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
        ("profile", Some("show")) => {
            let host =
                json!({"name": ns.one("--host"), "version": ns.one("--version").unwrap_or("")});
            let profile = match profiles::for_host(&host) {
                Ok(p) => p,
                Err(e) => bail!(from_config_error(e)),
            };
            let name = profile.get("host").and_then(Value::as_str).unwrap_or("unknown").to_string();
            let sha = match profiles::sha256(&name) {
                Ok(s) => s,
                Err(e) => bail!(from_config_error(e)),
            };
            let mut out = profile;
            out["sha256"] = json!(sha);
            // The routing authority answers "which plan already exists", so
            // an Ask that names one can be routed to building it.
            out["plans"] = json!(workspace::plans(&ws));
            (ws, Outcome::Ok(0, out))
        }
        ("ask", Some(what)) => ask_command(ns, ws, actor, what, read_stdin),
        ("deltas", Some(what)) => deltas_command(ns, ws, what),
        ("eval", Some("list")) => match evaluate::list_records(&ws) {
            Ok(evals) => (ws, Outcome::Ok(0, json!({"evals": evals}))),
            Err(e) => (ws, from_ask_error(e)),
        },
        ("eval", Some("open")) => {
            let agents = match ns.one("--agents").map(json_arg).transpose() {
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
                },
            );
            match out {
                Ok(v) => (ws, Outcome::Ok(0, v)),
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        ("eval", Some("context")) => {
            let raw = ns.one("--map").unwrap_or_default();
            let map = match raw.strip_prefix('@') {
                Some(path) => match std::fs::read(path) {
                    Ok(b) => b,
                    Err(e) => bail!(Outcome::UsageDetail(format!("{path}: {e}"))),
                },
                None => raw.as_bytes().to_vec(),
            };
            let out = evaluate::gather(
                &ws,
                ns.one("--eval").unwrap_or_default(),
                &map,
                ns.one("--host").unwrap_or_default(),
                Some(&actor),
            );
            match out {
                Ok(v) => (ws, Outcome::Ok(0, v)),
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        ("eval", Some("close")) => {
            let intent = json_try!("--intent");
            let mut judgements = Vec::new();
            for raw in ns.all("--judgement") {
                match json_arg(&raw) {
                    Ok(v) => judgements.push(v),
                    Err(e) => bail!(Outcome::UsageDetail(e)),
                }
            }
            let agents = match ns.one("--agents").map(json_arg).transpose() {
                Ok(v) => v,
                Err(e) => bail!(Outcome::UsageDetail(e)),
            };
            let out = evaluate::close_eval(
                &ws,
                ns.one("--eval").unwrap_or_default(),
                &intent,
                &judgements,
                agents.as_ref(),
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
        ("sessions", Some("capture")) => {
            let payload: Value = match serde_json::from_str(&read_stdin()) {
                Ok(v) => v,
                Err(e) => bail!(Outcome::UsageDetail(e.to_string())),
            };
            let ws = match hook_workspace(ns.workspace.as_ref(), ws, &payload) {
                Ok(w) => w,
                Err(e) => {
                    return (workspace::resolve(None, None).expect("cwd"), Outcome::UsageDetail(e));
                }
            };
            let host = ns.one("--host").unwrap_or_default();
            let rec = match sessions::capture(&ws, host, &payload, ns.one("--host-version")) {
                Ok(r) => r,
                Err(e) => return (ws, Outcome::Error("sessions.io".into(), e.to_string())),
            };
            let mut submission = Value::Null;
            if let Some(rec) = &rec
                && rec.get("prompt").is_none()
            {
                // A hook must never fail the host turn.
                let session = rec["session_id"].as_str().unwrap_or_default();
                let sha = rec["sha256"].as_str().unwrap_or_default();
                if let Ok(Some(s)) = ask::submission_from_capture(&ws, host, session, sha) {
                    submission = s;
                }
            }
            let mut out = json!({"captured": rec.is_some()});
            for (k, v) in rec.iter().flat_map(|r| r.as_object().into_iter().flatten()) {
                out[k] = v.clone();
            }
            out["submission"] = submission;
            (ws, Outcome::Ok(0, out))
        }
        ("fact", None) => {
            if !ns.has("--from-hook") {
                bail!(Outcome::UsageDetail(
                    "a fact is only recorded from a hook: --from-hook".into()
                ));
            }
            let payload: Value = match serde_json::from_str(&read_stdin()) {
                Ok(v) => v,
                Err(e) => bail!(Outcome::UsageDetail(e.to_string())),
            };
            let ws = match hook_workspace(ns.workspace.as_ref(), ws, &payload) {
                Ok(w) => w,
                Err(e) => {
                    return (workspace::resolve(None, None).expect("cwd"), Outcome::UsageDetail(e));
                }
            };
            let host = ns.one("--host").unwrap_or_default();
            match evaluate::record_fact(&ws, host, &payload) {
                Ok(Some(ev)) => {
                    let out = json!({"recorded": true, "fact_id": ev["id"],
                                     "outcome": ev["data"]["outcome"]});
                    (ws, Outcome::Ok(0, out))
                }
                Ok(None) => (ws, Outcome::Ok(0, json!({"recorded": false}))),
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        _ => (ws, Outcome::UsageDetail(format!("unknown command {}", ns.cmd))),
    }
}

fn ask_command(
    ns: &Parsed,
    ws: Workspace,
    actor: Value,
    what: &str,
    read_stdin: &mut dyn FnMut() -> String,
) -> (Workspace, Outcome) {
    macro_rules! done {
        ($e:expr) => {
            match $e {
                Ok(v) => return (ws, Outcome::Ok(0, v)),
                Err(e) => return (ws, from_ask_error(e.into())),
            }
        };
    }
    let id = ns.one("--ask").unwrap_or_default().to_string();
    match what {
        "compile" => {
            let classification = match json_arg(ns.one("--classification").unwrap_or_default()) {
                Ok(v) => v,
                Err(e) => return (ws, Outcome::UsageDetail(e)),
            };
            let route = match json_arg(ns.one("--route").unwrap_or_default()) {
                Ok(v) => v,
                Err(e) => return (ws, Outcome::UsageDetail(e)),
            };
            let host = match json_arg(ns.one("--host").unwrap_or_default()) {
                Ok(v) => v,
                Err(e) => return (ws, Outcome::UsageDetail(e)),
            };
            let staged = PathBuf::from(ns.one("--staged").unwrap_or_default());
            done!(ask::compile(
                &ws,
                ask::Compile {
                    staged: &staged,
                    title: ns.one("--title").unwrap_or_default(),
                    capability: ns.one("--capability").unwrap_or_default(),
                    classification,
                    route,
                    host,
                    links: links_of(&ns.all("--link")),
                    limitations: ns.all("--limitation"),
                    actor: Some(actor),
                    overrides: &ns.overrides,
                }
            ))
        }
        "confirm" => done!(ask::confirm(&ws, &id, Some(&actor))),
        "unanswered" => done!(ask::unanswered(&ws, &id, ns.one("--reason"), Some(&actor))),
        "cancel" => {
            done!(ask::cancel(&ws, &id, ns.one("--reason"), Some(&actor), ns.has("--attributed")))
        }
        "submitted" => done!(ask::submitted(&ws, &id, ns.has("--as-modified"), Some(&actor))),
        "preflight" => done!(ask::preflight(&ws)),
        "copy" => {
            // `--body` is the prompt without the command its prefix names, so
            // a person typing the command has something to paste that needs no
            // slicing.
            let got = if ns.has("--body") {
                ask::prompt_body(&ws, &id)
            } else {
                ask::prompt_text(&ws, &id)
            };
            match got {
                Ok(text) => (ws, Outcome::Ok(0, Value::String(text))),
                Err(e) => (ws, from_ask_error(e)),
            }
        }
        "delivery" => {
            if ns.has("--from-hook") {
                // A hook must never fail the host turn.
                let recorded = (|| -> Result<(Workspace, Option<Value>), String> {
                    let payload: Value =
                        serde_json::from_str(&read_stdin()).map_err(|e| e.to_string())?;
                    let ws = hook_workspace(ns.workspace.as_ref(), ws, &payload)?;
                    let rec = ask::delivery_from_hook(&ws, &payload).map_err(|e| e.to_string())?;
                    Ok((ws, rec))
                })();
                return match recorded {
                    Ok((ws, rec)) => {
                        let mut out = json!({"recorded": rec.is_some()});
                        for (k, v) in rec.iter().flat_map(|r| r.as_object().into_iter().flatten()) {
                            out[k] = v.clone();
                        }
                        (ws, Outcome::Ok(0, out))
                    }
                    Err(error) => (
                        workspace::resolve(None, ns.workspace.as_deref()).expect("a workspace"),
                        Outcome::Ok(0, json!({"recorded": false, "error": error})),
                    ),
                };
            }
            if id.is_empty() {
                return (ws, Outcome::UsageDetail("--ask is required unless --from-hook".into()));
            }
            if ns.has("--handoff") {
                return match ask::delivery_handoff(&ws, &id) {
                    Ok((text, _)) => (ws, Outcome::Text(text)),
                    Err(e) => (ws, from_ask_error(e)),
                };
            }
            let Some(state) = ns.one("--state") else {
                return (ws, Outcome::UsageDetail("--handoff or --state is required".into()));
            };
            done!(ask::delivery_state(&ws, &id, state, ns.one("--reason").unwrap_or("")))
        }
        "list" => match ask::list_asks(&ws) {
            Ok(asks) => (ws, Outcome::Ok(0, json!({"asks": asks}))),
            Err(e) => (ws, from_ask_error(e)),
        },
        other => (ws, Outcome::UsageDetail(format!("unknown ask command {other}"))),
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
            let host = json!({"name": ns.one("--host"),
                              "version": ns.one("--host-version").unwrap_or("")});
            let profile = config_try!(profiles::for_host(&host));
            let classification = match json_arg(ns.one("--classification").unwrap_or_default()) {
                Ok(v) => v,
                Err(e) => return (ws, Outcome::UsageDetail(e)),
            };
            let statuses: Vec<String> = ns
                .one("--statuses")
                .unwrap_or("qualified")
                .split(',')
                .map(str::to_string)
                .collect();
            let rendered = config_try!(deltas::render(
                &ns.overrides,
                &profile,
                ns.one("--capability").unwrap_or_default(),
                &classification,
                &statuses,
                deltas::DEFAULT_DOMAIN,
            ));
            if ns.json || ns.minimal {
                (ws, Outcome::Ok(0, rendered))
            } else {
                (ws, Outcome::Text(rendered["text"].as_str().unwrap_or_default().to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{commit, eval_bench, intent_doc, two_asks_and_work, with_config_home};

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

        fn staged(&self, name: &str) -> String {
            let d = self.root.join(".fab7/rf/tmp").join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("source.txt"), b"fix login\n").unwrap();
            std::fs::write(d.join("prompt.txt"), b"Fix login.\n").unwrap();
            d.to_string_lossy().to_string()
        }

        /// Compile an Ask and confirm it in one step.
        fn confirm(&self, stage: &str, title: &str, session: &str, capability: &str) -> Value {
            let staged = self.staged(stage);
            let host = host_json(session);
            let (code, out, _) = self.go(&[
                "ask",
                "compile",
                "--staged",
                &staged,
                "--title",
                title,
                "--capability",
                capability,
                "--classification",
                CLS,
                "--route",
                ROUTE,
                "--host",
                &host,
            ]);
            if code == 0 {
                let id = out["ask_id"].as_str().unwrap().to_string();
                let (c2, o2, _) = self.go(&["ask", "confirm", "--ask", &id]);
                assert_eq!(c2, 0);
                assert_eq!(o2["confirmation"]["observed_by"], "skill");
            }
            out
        }
    }

    const CLS: &str = r#"{"task":["plan"],"result":"plan","interaction":"approval_gated","horizon":"session","effects":["read"]}"#;
    const ROUTE: &str =
        r#"{"fits":"f","alternatives":[],"continuation":"c","effects":"e","gaps":[]}"#;

    fn host_json(session: &str) -> String {
        format!(
            r#"{{"name":"claude-code","version":"2.1.260","surface":"native-tui","session_ref":"{session}"}}"#
        )
    }

    fn cli<T>(body: impl FnOnce(&Cli) -> T) -> T {
        eval_bench(|ws| body(&Cli { root: ws.root.clone() }))
    }

    #[test]
    fn init_and_profile_show() {
        cli(|c| {
            let (code, out, _) = c.go(&["init"]);
            assert_eq!(code, 0);
            assert!(out["rf_dir"].as_str().unwrap().ends_with(".fab7/rf"));
            assert!(c.root.join(".fab7/rf/.gitignore").exists());
            let (code, out, _) = c.go(&[
                "profile",
                "show",
                "--host",
                "claude-code",
                "--version",
                "2.1.260",
                "--json",
            ]);
            assert_eq!(code, 0);
            assert_eq!(out["profile_id"], "claude-code");
            assert_eq!(out["sha256"].as_str().unwrap().len(), 64);
            let (_, out, _) = c.go(&["profile", "show", "--host", "cursor", "--json"]);
            assert_eq!(out["profile_id"], "unknown");
        });
    }

    #[test]
    fn the_routing_authority_says_which_plans_the_project_already_holds() {
        // Which wording named a plan — a path, a slug, or words — is not
        // something the record can settle. Routing reads what is on disk.
        cli(|c| {
            c.go(&["init"]);
            let (_, out, text) = c.go(&["profile", "show", "--host", "claude-code", "--minimal"]);
            assert!(out["plans"].is_null(), "a project with none says nothing about plans");
            assert!(!text.contains("plans"), "{text}");

            std::fs::create_dir_all(c.root.join("plans/crypto-trading-agent/adr")).unwrap();
            let (code, out, _) = c.go(&["profile", "show", "--host", "claude-code", "--minimal"]);
            assert_eq!(code, 0);
            assert_eq!(out["plans"], serde_json::json!(["crypto-trading-agent"]));
        });
    }

    #[test]
    fn ask_confirm_show_delivery_flow() {
        cli(|c| {
            let payload = format!(
                r#"{{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"/rf:ask fix login","cwd":"{}"}}"#,
                c.root.display()
            );
            let (code, out, _) =
                c.piped(&["sessions", "capture", "--host", "claude-code"], &payload);
            assert_eq!(code, 0);
            assert_eq!(out["captured"], true);

            let out = c.confirm("stage-1", "Login", "s1", "native_plan");
            let keys: BTreeMap<String, Value> =
                out.as_object().unwrap().clone().into_iter().collect();
            assert_eq!(
                keys.keys().cloned().collect::<Vec<_>>(),
                ["ask_id", "delivery_mode", "prompt_path", "source_verified"]
            );
            assert_eq!(out["source_verified"], "exact");
            let ask_id = out["ask_id"].as_str().unwrap().to_string();

            let hook = r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"EnterPlanMode","tool_use_id":"t1","tool_response":{"ok":1}}"#;
            let (code, out, _) = c.piped(&["ask", "delivery", "--from-hook"], hook);
            assert_eq!(code, 0);
            assert_eq!(out["recorded"], true);
            assert_eq!(out["state"], "native_accepted");
            // Never fails the host turn.
            let (code, out, _) = c.piped(&["ask", "delivery", "--from-hook"], hook);
            assert_eq!(code, 0);
            assert_eq!(out["recorded"], false);

            let (code, out, _) = c.go(&["ask", "list", "--json"]);
            assert_eq!(code, 0);
            assert_eq!(out["asks"][0]["id"], ask_id);
            assert_eq!(out["asks"][0]["delivery"], "native_accepted");
            let (code, out, _) = c.go(&["ledger", "verify", "--json"]);
            assert_eq!(code, 0);
            assert_eq!(out, json!({"findings": [], "clean": true}));
        });
    }

    #[test]
    fn ask_handoff_state_and_resolution_exit_codes() {
        cli(|c| {
            let a = c.confirm("stage-1", "Login", "s1", "native_plan");
            let a_id = a["ask_id"].as_str().unwrap().to_string();
            let (code, out, _) = c.go(&["ask", "delivery", "--ask", &a_id, "--handoff"]);
            assert_eq!(code, 0);
            let text = out.as_str().unwrap();
            assert!(text.contains("prompt.txt"), "{text}");
            assert!(text.contains("Claude Code TUI"));
            let (code, out, _) = c.go(&[
                "ask",
                "delivery",
                "--ask",
                &a_id,
                "--state",
                "unavailable",
                "--reason",
                "x",
            ]);
            assert_eq!(code, 2);
            assert_eq!(out["error"], "delivery.duplicate");

            // Two Asks in one workspace are both listed, oldest first. There
            // is no resolution step: a client picks from the list it holds.
            let b = c.confirm("stage-2", "Logout", "s2", "native_direct");
            let b_id = b["ask_id"].as_str().unwrap().to_string();
            let (code, out, _) = c.go(&["ask", "list", "--json"]);
            assert_eq!(code, 0);
            let ids: Vec<&str> =
                out["asks"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
            assert_eq!(ids, [a_id.as_str(), b_id.as_str()]);
        });
    }

    #[test]
    fn ask_compile_cancel_submitted_copy_and_usage_errors() {
        cli(|c| {
            let staged = c.staged("stage-1");
            let host = host_json("s1");
            let (code, out, _) = c.go(&[
                "ask",
                "compile",
                "--staged",
                &staged,
                "--title",
                "t",
                "--capability",
                "native_plan",
                "--classification",
                CLS,
                "--route",
                ROUTE,
                "--host",
                &host,
            ]);
            assert_eq!(code, 0);
            assert_eq!(out["delivery_mode"], "native_dispatch");
            let id = out["ask_id"].as_str().unwrap().to_string();

            let (code, out, _) = c.go(&["ask", "cancel", "--ask", &id, "--reason", "nah"]);
            assert_eq!(code, 0);
            assert_eq!(out["state"], "cancelled");
            let (code, out, _) = c.go(&["ask", "submitted", "--ask", &id, "--as-modified"]);
            assert_eq!(code, 0);
            assert_eq!(out["state"], "attributed");
            assert_eq!(out["as_modified"], true);
            let (code, out, _) = c.go(&["ask", "copy", "--ask", &id]);
            assert_eq!(code, 0);
            assert_eq!(out, "Fix login.\n");

            let (code, out, _) = c.go(&[
                "ask",
                "compile",
                "--staged",
                "/nope",
                "--title",
                "t",
                "--capability",
                "native_plan",
                "--classification",
                CLS,
                "--route",
                ROUTE,
                "--host",
                &host,
            ]);
            assert_eq!(code, 2);
            // Outside the opened directory is refused before its contents are
            // ever looked at, because compile deletes what it reads.
            assert_eq!(out["error"], "workspace.outside");

            // Inside it, a directory that is not a staging directory is still
            // refused for what it holds.
            let bad = c.root.join(".fab7/rf/tmp/not-a-stage");
            std::fs::create_dir_all(&bad).unwrap();
            let (code, out, _) = c.go(&[
                "ask",
                "compile",
                "--staged",
                &bad.to_string_lossy(),
                "--title",
                "t",
                "--capability",
                "native_plan",
                "--classification",
                CLS,
                "--route",
                ROUTE,
                "--host",
                &host,
            ]);
            assert_eq!(code, 2);
            assert_eq!(out["error"], "ask.staged_dir");

            // Missing required arguments are a usage error, on stderr.
            let (code, _, err) = c.go(&["ask", "compile"]);
            assert_eq!(code, 1);
            assert!(err.contains("the following arguments are required"), "{err}");

            let staged = c.staged("s3");
            let (code, out, _) = c.go(&[
                "ask",
                "compile",
                "--staged",
                &staged,
                "--title",
                "t",
                "--capability",
                "native_plan",
                "--classification",
                "{not json",
                "--route",
                ROUTE,
                "--host",
                &host,
            ]);
            assert_eq!(code, 1);
            assert_eq!(out["error"], "usage");
        });
    }

    #[test]
    fn capture_of_a_pasted_prompt_records_an_observed_submission() {
        cli(|c| {
            let staged = c.staged("stage-1");
            let (code, out, _) = c.go(&[
                "ask",
                "compile",
                "--staged",
                &staged,
                "--title",
                "t",
                "--capability",
                "native_direct",
                "--classification",
                CLS,
                "--route",
                ROUTE,
                "--host",
                r#"{"name":"codex","surface":"native-tui"}"#,
            ]);
            assert_eq!(code, 0);
            let id = out["ask_id"].as_str().unwrap().to_string();
            let payload = format!(
                r#"{{"hook_event_name":"UserPromptSubmit","session_id":"c9","prompt":"Fix login.\n","cwd":"{}"}}"#,
                c.root.display()
            );
            let (code, cap, _) = c.piped(&["sessions", "capture", "--host", "codex"], &payload);
            assert_eq!(code, 0);
            assert_eq!(cap["captured"], true);
            assert_eq!(cap["submission"]["ask_id"], id);
            assert_eq!(cap["submission"]["state"], "observed");
            let (_, listed, _) = c.go(&["ask", "list", "--json"]);
            assert_eq!(listed["asks"][0]["submission"], "observed");
            assert_eq!(listed["asks"][0]["outcome"], "compiled");
        });
    }

    #[test]
    fn a_hook_records_a_shell_command_as_a_fact() {
        eval_bench(|ws| {
            let c = Cli { root: ws.root.clone() };
            two_asks_and_work(ws);
            let payload = format!(
                r#"{{"hook_event_name":"PostToolUse","session_id":"s9","tool_name":"Bash","tool_input":{{"command":"npm test"}},"tool_response":{{"stdout":"ok"}},"tool_use_id":"t1","cwd":"{}"}}"#,
                c.root.display()
            );
            let (code, out, _) =
                c.piped(&["fact", "--from-hook", "--host", "claude-code"], &payload);
            assert_eq!(code, 0, "{out}");
            assert_eq!(out["recorded"], true);
            assert_eq!(out["outcome"], "succeeded");
            let skipped = payload.replace("npm test", "ringframe ask list");
            let (code, out, _) =
                c.piped(&["fact", "--from-hook", "--host", "claude-code"], &skipped);
            assert_eq!(code, 0);
            assert_eq!(out["recorded"], false);
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
            assert_eq!(out["agents"], json!({"drift": {"effort": "high"}}));
            let eval_id = out["eval_id"].as_str().unwrap().to_string();
            let map = ws.root.join("map.md");
            std::fs::write(&map, "## src/uptime.js\nAdds uptime().\n").unwrap();
            let at = format!("@{}", map.to_string_lossy());
            let context = ["eval", "context", "--eval", &eval_id, "--map", &at, "--host", "codex"];
            let (code, gathered, _) = c.go(&context);
            assert_eq!(code, 0);
            assert!(gathered["context_map"].as_str().unwrap().ends_with("context.md"));
            let (code, again, _) = c.go(&context);
            assert_ne!(code, 0);
            assert_eq!(again["error"], "eval.already_gathered");
            std::fs::remove_file(&map).unwrap();
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
            let brief_sha = out["brief"]["sha256"].as_str().unwrap().to_string();

            // The intent and judgements are written inside the repository,
            // which is what pytest's shared tmp_path did. It matters: they
            // dirty the tree, so the Seal's subject is the worktree rather
            // than a commit, and a later change stops matching it.
            let dir = c.root.clone();
            let items = json!([{"id": "i1", "text": "Expose an uptime endpoint", "ask_id": a, "status": "active"}]);
            let intent_path = dir.join("intent.json");
            std::fs::write(&intent_path, intent_doc(&brief_sha, items).to_string()).unwrap();
            let intent_arg = format!("@{}", intent_path.display());
            let mut judgement_args: Vec<String> = Vec::new();
            for (n, ang) in ["coverage", "drift", "adversary"].iter().enumerate() {
                let f = dir.join(format!("j{n}.json"));
                let j = crate::testing::judgement_over(
                    &brief_sha,
                    ang,
                    &[("i1", "yes")],
                    &[("docs/notes.md", "unexplained")],
                    &crate::testing::CHANGED,
                );
                std::fs::write(&f, j.to_string()).unwrap();
                judgement_args.push("--judgement".into());
                judgement_args.push(format!("@{}", f.display()));
            }
            let base: Vec<&str> =
                vec!["eval", "close", "--eval", &eval_id, "--intent", &intent_arg];
            let two: Vec<&str> = base
                .iter()
                .copied()
                .chain(judgement_args[..4].iter().map(String::as_str))
                .collect();
            let (code, out, _) = c.go(&two);
            assert_eq!(code, 2);
            assert_eq!(out["error"], "eval.too_few_judges");

            let all: Vec<&str> = base
                .iter()
                .copied()
                .chain(judgement_args.iter().map(String::as_str))
                .chain(["--agents", r#"{"drift":{"effort":"high"}}"#])
                .collect();
            let (code, rec, _) = c.go(&all);
            assert_eq!(code, 0);
            let record: Value = serde_json::from_slice(
                &std::fs::read(ws.rf_dir().join(format!("evals/{eval_id}/record.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(record["agents"], json!({"drift": {"effort": "high"}}));
            assert_eq!(rec["verdict"], "drifted");
            assert_eq!(rec["confidence"], 1.0);
            assert_eq!(rec["drift"]["commission"][0]["path"], "docs/notes.md");
            let (code, out, _) = c.go(&all);
            assert_eq!(code, 2);
            assert_eq!(out["error"], "ledger.immutable");

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
        assert!(whole.out.contains("ask compile"));
        assert!(whole.out.contains("Exit codes: 0 ok, 1 usage"));
        let one = super::run(&["eval".to_string(), "close".into(), "--help".into()], &mut none);
        assert_eq!(one.code, 0);
        assert!(one.out.contains("usage: ringframe eval close"), "{}", one.out);
        assert!(one.out.contains("--judgement VALUE (required) (repeatable)"), "{}", one.out);
        let compile =
            super::run(&["ask".to_string(), "compile".into(), "--help".into()], &mut none);
        for said in [
            "remediates:<evl_id>",
            "follows:<ask_id|evl_id>",
            "ask.link_unknown",
            "ask.link_rel_unknown",
        ] {
            assert!(compile.out.contains(said), "{said} missing from:\n{}", compile.out);
        }
        // `profile show --version` names a host version, not this binary's.
        let host = super::run(
            &[
                "profile".to_string(),
                "show".into(),
                "--host".into(),
                "codex".into(),
                "--version".into(),
                "0.1".into(),
                "--minimal".into(),
            ],
            &mut none,
        );
        assert!(!host.out.starts_with("ringframe 0."), "{}", host.out);
    }

    #[test]
    fn the_version_is_the_rust_era() {
        let mut none = || String::new();
        let run = super::run(&["--version".to_string()], &mut none);
        assert_eq!(run.code, 0);
        assert_eq!(run.out, format!("ringframe {VERSION}\n"));
        assert!(VERSION.starts_with("0.1."), "the Rust era starts at 0.1.0: {VERSION}");
    }

    #[test]
    fn deltas_commands() {
        cli(|c| {
            let cls = r#"{"task":["implement"],"result":"workspace_change","interaction":"approval_gated","horizon":"session","effects":["write"],"concerns":["api_surface"]}"#;
            let (code, out, _) = c.go(&[
                "deltas",
                "render",
                "--host",
                "codex",
                "--host-version",
                "codex-cli 0.153.4",
                "--capability",
                "native_plan",
                "--classification",
                cls,
                "--json",
            ]);
            assert_eq!(code, 0);
            assert!(
                out["practice"]["selected"].as_array().unwrap().contains(&json!("practice.hyrum"))
            );
            assert!(!out["text"].as_str().unwrap().is_empty());
            let (code, text, _) = c.go(&[
                "deltas",
                "render",
                "--host",
                "codex",
                "--host-version",
                "codex-cli 0.153.4",
                "--capability",
                "native_plan",
                "--classification",
                cls,
            ]);
            assert_eq!(code, 0);
            assert!(text.as_str().unwrap().contains("observable behaviour"));
        });
    }

    #[test]
    fn deltas_render_json_exposes_each_directive_for_composition() {
        cli(|c| {
            let cls = r#"{"task":["implement"],"result":"workspace_change","interaction":"approval_gated","horizon":"session","effects":["write"],"concerns":["api_surface"]}"#;
            let (code, out, _) = c.go(&[
                "deltas",
                "render",
                "--host",
                "codex",
                "--host-version",
                "codex-cli 0.153.4",
                "--capability",
                "native_plan",
                "--classification",
                cls,
                "--json",
            ]);
            assert_eq!(code, 0);
            let ids: Vec<&str> = out["practice"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["id"].as_str().unwrap())
                .collect();
            let selected: Vec<&str> = out["practice"]["selected"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e.as_str().unwrap())
                .collect();
            assert_eq!(ids, selected);
            assert!(
                out["practice"]["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|e| !e["text"].as_str().unwrap_or_default().is_empty())
            );
            // Nothing qualified yet.
            assert_eq!(out["host"]["entries"], json!([]));
        });
    }

    #[test]
    fn empty_listings() {
        cli(|c| {
            assert_eq!(c.go(&["eval", "list", "--json"]).1, json!({"evals": []}));
            assert_eq!(c.go(&["ask", "list", "--json"]).1, json!({"asks": []}));
        });
    }

    #[test]
    fn a_hook_uses_the_payload_project_and_initializes_private_storage() {
        for host_name in ["codex", "claude-code"] {
            eval_bench(|ws| {
                let project = ws.root.join("test");
                std::fs::create_dir_all(&project).unwrap();
                let payload = format!(
                    r#"{{"cwd":"{}","session_id":"nested","prompt":"/rf:ask fix login"}}"#,
                    project.display()
                );
                let mut read = || payload.clone();
                let run = super::run(
                    &["sessions".into(), "capture".into(), "--host".into(), host_name.into()],
                    &mut read,
                );
                assert_eq!(run.code, 0, "{}", run.err);
                let rf = project.join(".fab7/rf");
                assert!(rf.join(format!("sessions/{host_name}/nested/prompts.jsonl")).exists());
                assert_eq!(std::fs::read_to_string(rf.join(".gitignore")).unwrap(), "*\n");
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(rf.metadata().unwrap().permissions().mode() & 0o777, 0o700);
            });
        }
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

            let staged = c.staged("stage-1");
            let host = host_json("s1");
            let compile: Vec<&str> = vec![
                "ask",
                "compile",
                "--staged",
                &staged,
                "--title",
                "Login",
                "--capability",
                "native_plan",
                "--classification",
                CLS,
                "--route",
                ROUTE,
                "--host",
                &host,
            ];
            let (code, out, _) = c.go(&compile);
            assert_eq!(code, 2);
            assert_eq!(out["error"], "workspace.no_git");

            crate::testing::run(&["git", "init", "-q", &dir.path().to_string_lossy()]);
            let (code, out, _) = c.go(&compile);
            assert_eq!(code, 2);
            assert_eq!(out["error"], "workspace.no_commit");
        });
    }

    #[test]
    fn deltas_domains_and_an_unknown_domain_is_refused() {
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
            assert!(!base["concerns"].as_array().unwrap().is_empty());
            let cls = r#"{"task":["plan"],"result":"plan","interaction":"approval_gated","horizon":"session","effects":["read"],"domains":["teleportation"]}"#;
            let staged = c.staged("stage-1");
            let host = host_json("s1");
            let (code, out, _) = c.go(&[
                "ask",
                "compile",
                "--staged",
                &staged,
                "--title",
                "T",
                "--capability",
                "native_plan",
                "--classification",
                cls,
                "--route",
                ROUTE,
                "--host",
                &host,
            ]);
            assert_eq!(code, 2);
            assert_eq!(out["error"], "ask.classification");
            assert!(out["detail"].as_str().unwrap().contains("installed"));
        });
    }

    // ---- output modes ------------------------------------------------------

    #[test]
    fn minimal_and_json_are_mutually_exclusive() {
        cli(|c| {
            let (code, _, err) =
                c.go(&["profile", "show", "--host", "claude-code", "--json", "--minimal"]);
            assert_eq!(code, 1);
            assert!(err.contains("not allowed with"), "{err}");
        });
    }

    #[test]
    fn minimal_emits_compact_json() {
        cli(|c| {
            let (_, out, _) = c.go(&["profile", "show", "--host", "claude-code", "--minimal"]);
            // Still JSON the skill can address by name.
            assert!(out.is_object());
            let (_, raw, _) = c.go(&["ask", "list", "--minimal"]);
            assert!(raw.is_object());
        });
    }

    #[test]
    fn minimal_profile_keeps_only_what_routing_reads() {
        cli(|c| {
            let (_, out, _) = c.go(&["profile", "show", "--host", "claude-code", "--minimal"]);
            let mut keys: Vec<&String> = out.as_object().unwrap().keys().collect();
            keys.sort();
            assert_eq!(keys, ["capabilities", "host", "paste_fold_chars", "routing"]);
            let allowed: BTreeMap<&str, ()> = [
                "id",
                "selection",
                "effects",
                "confirmation",
                "activation",
                "delivery_mode",
                "continuation",
                "limitations",
                "requires_explicit_request_for_effects",
                "receipt",
                "prompt_prefix",
                "prompt_prefix_kind",
                "prompt_prefix_active",
            ]
            .into_iter()
            .map(|k| (k, ()))
            .collect();
            let first = &out["capabilities"][0];
            for k in first.as_object().unwrap().keys() {
                assert!(allowed.contains_key(k.as_str()), "{k} is not part of the routing view");
            }
            assert!(!first["selection"].as_str().unwrap().is_empty());
            assert!(!out["routing"]["precedence"].as_array().unwrap().is_empty());
        });
    }

    #[test]
    fn minimal_profile_says_how_a_prefixed_prompt_has_to_be_submitted() {
        // A client that knows the command but not whether it is a mode cannot
        // decide how to send it, and both hosts stop reading a long paste for
        // commands. The prefix, its kind and the fold size travel together.
        cli(|c| {
            let (_, out, _) = c.go(&["profile", "show", "--host", "codex", "--minimal"]);
            assert_eq!(out["paste_fold_chars"], 1001);
            let by_id: BTreeMap<String, Value> = out["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| (c["id"].as_str().unwrap().to_string(), c.clone()))
                .collect();
            assert_eq!(by_id["native_plan"]["prompt_prefix_kind"], "mode");
            assert_eq!(by_id["native_plan"]["prompt_prefix_active"], "Plan mode");
            // `/goal` takes its objective as the argument, so there is no mode
            // to enter.
            assert_eq!(by_id["native_goal"]["prompt_prefix_kind"], "inline");
            assert!(by_id["native_goal"].get("prompt_prefix_active").is_none());
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
            let a = c.confirm("stage-1", "Login", "s1", "native_plan");
            let ws = crate::workspace::resolve(Some(&project), None).unwrap();
            assert_eq!(crate::ask::list_asks(&ws).unwrap()[0]["ask_id"], a["ask_id"]);
            assert!(!repo.path().join(".fab7").exists(), "the parent keeps nothing");
        });
    }

    #[test]
    fn compile_reads_the_merged_ledger_delta_files() {
        cli(|c| {
            let over = json!([
                {"layer": "weft", "ringframe": {"deltas": {"practices/software-development":
                    {"concerns": ["project_special"], "render": {"core_cap": 1},
                     "entries": [{"id": "practice.kiss", "text": "Use the machine setting."}]}}}},
                {"layer": "weft-project", "ringframe": {"deltas": {"practices/software-development":
                    {"entries": [{"id": "practice.kiss", "text": "Use the project setting."}]}}}},
            ])
            .to_string();
            let cls = r#"{"task":["implement"],"result":"workspace_change","interaction":"approval_gated","horizon":"session","effects":["write"],"concerns":["project_special"]}"#;
            let stage = c.root.join("stage");
            std::fs::create_dir_all(&stage).unwrap();
            std::fs::write(stage.join("source.txt"), "fix login").unwrap();
            std::fs::write(stage.join("body.txt"), "Fix login.").unwrap();
            let host = host_json("s1");
            let (code, out, err) = c.go(&[
                "ask",
                "compile",
                "--staged",
                &stage.to_string_lossy(),
                "--title",
                "Login",
                "--capability",
                "native_plan",
                "--classification",
                cls,
                "--route",
                ROUTE,
                "--host",
                &host,
                "--override",
                &over,
            ]);
            assert_eq!(code, 0, "{err}");
            let text = std::fs::read_to_string(out["prompt_path"].as_str().unwrap()).unwrap();
            // The compiled prompt is the merge: the later layer won.
            assert!(text.contains("Use the project setting."), "{text}");
        });
    }

    #[test]
    fn profile_exposes_routing_and_capability_sources() {
        cli(|c| {
            for name in ["codex", "claude-code"] {
                let (code, out, _) = c.go(&["profile", "show", "--host", name, "--json"]);
                assert_eq!(code, 0);
                assert_eq!(out["routing"], crate::profiles::load(name).unwrap()["routing"]);
                let precedence: BTreeMap<String, ()> = out["routing"]["precedence"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| (p.as_str().unwrap().to_string(), ()))
                    .collect();
                let ids: BTreeMap<String, ()> = out["capabilities"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| (c["id"].as_str().unwrap().to_string(), ()))
                    .collect();
                assert_eq!(precedence.keys().collect::<Vec<_>>(), ids.keys().collect::<Vec<_>>());
                for cap in out["capabilities"].as_array().unwrap() {
                    assert!(!cap["selection"].as_str().unwrap_or_default().is_empty());
                    let sources = cap["sources"].as_array().unwrap();
                    assert!(!sources.is_empty());
                    assert!(sources.iter().all(|u| u.as_str().unwrap().starts_with("https://")));
                    assert!(
                        ["native_dispatch", "human_handoff"]
                            .contains(&cap["delivery_mode"].as_str().unwrap())
                    );
                }
            }
        });
    }

    #[test]
    fn a_delta_listing_exposes_the_merged_concern_vocabulary() {
        cli(|c| {
            let over = r#"[{"layer":"weft","ringframe":{"deltas":{"practices/software-development":{"concerns":["team_boundary"]}}}}]"#;
            let (code, out, err) = c.go(&["deltas", "domains", "--json", "--override", over]);
            assert_eq!(code, 0);
            assert_eq!(err, "", "no folder, nothing to report");
            let base = out["domains"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["base"] == true)
                .expect("the base domain");
            assert_eq!(base["concerns"], json!(["team_boundary"]));
        });
    }

    #[test]
    fn a_malformed_override_is_refused_by_every_command_that_reads_configuration() {
        cli(|c| {
            for cmd in [
                &["profile", "show", "--host", "codex"][..],
                &["deltas", "domains"],
                &[
                    "deltas",
                    "render",
                    "--host",
                    "codex",
                    "--capability",
                    "native_plan",
                    "--classification",
                    "{}",
                ],
                &[
                    "ask",
                    "compile",
                    "--staged",
                    "x",
                    "--title",
                    "t",
                    "--capability",
                    "native_plan",
                    "--classification",
                    "{}",
                    "--route",
                    "{}",
                    "--host",
                    "{}",
                ],
                &["eval", "open"],
            ] {
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
                "concerns = [\"team_boundary\"]\n",
            )
            .unwrap();
            let personal = crate::config::home().join("overrides");
            std::fs::create_dir_all(&personal).unwrap();
            let (code, out, err) = c.go(&["deltas", "domains", "--json"]);
            assert_eq!(code, 0);
            assert_eq!(
                err,
                format!(
                    "ringframe: limitation: override folders are not read; move them into \
                     Weft's config.toml [ringframe]: {}, {}\n",
                    personal.display(),
                    project.display()
                )
            );
            let base = out["domains"].as_array().unwrap().iter().find(|d| d["base"] == true);
            assert!(
                !base.unwrap()["concerns"].as_array().unwrap().contains(&json!("team_boundary"))
            );
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
mod the_marketplace_contract {
    use super::*;

    /// Every `ringframe …` the shipped skills, hooks and plugins invoke.
    /// Taken from `fab7/products/ringframe/`, and checked against it below
    /// when that tree happens to be beside this one.
    const CALLED: [(&str, Option<&str>); 19] = [
        ("init", None),
        ("profile", Some("show")),
        ("deltas", Some("domains")),
        ("deltas", Some("render")),
        ("ask", Some("preflight")),
        ("ask", Some("compile")),
        ("ask", Some("copy")),
        ("ask", Some("confirm")),
        ("ask", Some("cancel")),
        ("ask", Some("unanswered")),
        ("ask", Some("submitted")),
        ("ask", Some("delivery")),
        ("ask", Some("list")),
        ("eval", Some("open")),
        ("eval", Some("close")),
        ("eval", Some("list")),
        ("seal", Some("create")),
        ("seal", Some("check")),
        ("sessions", Some("capture")),
    ];

    #[test]
    fn every_command_the_skills_call_still_exists() {
        for (cmd, sub) in CALLED {
            assert!(
                spec(cmd, sub).is_some(),
                "the skills call `ringframe {cmd} {}` and this release has no such command",
                sub.unwrap_or("")
            );
            assert!(
                SURFACE.contains(&(cmd, sub)),
                "`ringframe {cmd} {}` is not in the help",
                sub.unwrap_or("")
            );
        }
    }

    #[test]
    fn the_marketplace_calls_nothing_this_release_lacks() {
        // Read from the marketplace itself when it is checked out beside this
        // repository, so the list above cannot quietly fall behind.
        let market = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../fab7/products/ringframe");
        let Ok(market) = market.canonicalize() else {
            return; // not checked out here; the list above still holds
        };
        let mut found: std::collections::BTreeSet<(String, Option<String>)> = Default::default();
        let mut stack = vec![market];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                for call in text.match_indices("ringframe ").map(|(i, _)| &text[i..]) {
                    // Prose and code fences wrap these in backticks, quotes
                    // and commas; the command is what is left.
                    let word = |w: &str| {
                        w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-').to_string()
                    };
                    let mut words = call.split_whitespace().skip(1).map(word);
                    let Some(cmd) = words.next() else { continue };
                    // `--version` and `--help` are answered before parsing, so
                    // they have no options table; SURFACE covers them.
                    if cmd.starts_with('-') || !COMMANDS.contains(&cmd.as_str()) {
                        continue;
                    }
                    let sub = words.next().filter(|_| takes_sub(&cmd));
                    found.insert((cmd, sub));
                }
            }
        }
        assert!(found.len() > 10, "only {} calls found; the scan broke", found.len());
        for (cmd, sub) in &found {
            assert!(
                spec(cmd, sub.as_deref()).is_some(),
                "the marketplace calls `ringframe {cmd} {}`, which this release does not have",
                sub.clone().unwrap_or_default()
            );
        }
    }
}
