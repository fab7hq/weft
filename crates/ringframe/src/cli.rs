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
use crate::{VERSION, ask, deltas, evaluate, ids, output, profiles, seal, sessions, store, workspace};

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
        ("sync", None) => s(&["--from"], NONE, NONE, false, NONE),
        ("profile", Some("show")) => s(&["--host", "--version"], NONE, &["--host"], true, NONE),
        ("ask", Some("compile")) => s(
            &["--staged", "--title", "--capability", "--classification", "--route", "--host",
              "--link", "--limitation"],
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
        ("ask", Some("copy")) => s(&["--ask"], NONE, &["--ask"], false, NONE),
        ("ask", Some("delivery")) => s(
            &["--ask", "--state", "--reason"],
            &["--from-hook", "--handoff"],
            NONE,
            false,
            NONE,
        ),
        ("ask", Some("list")) => s(NONE, NONE, NONE, true, NONE),
        ("ask", Some("show")) => s(&["--ask", "--session"], NONE, NONE, true, NONE),
        ("ask", Some("preflight")) => s(NONE, NONE, NONE, false, NONE),
        ("ask", Some("resolve")) => s(&["--session", "--kind"], NONE, NONE, true, NONE),
        ("eval", Some("open")) => {
            s(&["--anchor", "--subject-kind", "--subject-ref"], NONE, NONE, false, NONE)
        }
        ("eval", Some("close")) => s(
            &["--eval", "--intent", "--judgement"],
            NONE,
            &["--eval", "--intent", "--judgement"],
            false,
            &["--judgement"],
        ),
        ("eval", Some("list")) => s(NONE, NONE, NONE, true, NONE),
        ("seal", Some("create")) => {
            s(&["--disposition", "--eval", "--note"], NONE, &["--disposition"], false, NONE)
        }
        ("seal", Some("check")) => s(&["--seal"], NONE, &["--seal"], true, NONE),
        ("ledger", Some("verify")) => s(NONE, NONE, NONE, true, NONE),
        ("deltas", Some("list")) => {
            s(&["--host", "--capability", "--domain"], &["--effective"], NONE, true, NONE)
        }
        ("deltas", Some("domains")) => s(NONE, NONE, NONE, true, NONE),
        ("deltas", Some("render")) => s(
            &["--host", "--host-version", "--capability", "--classification", "--statuses"],
            NONE,
            &["--host", "--capability", "--classification"],
            true,
            NONE,
        ),
        ("sessions", Some("capture")) => {
            s(&["--host", "--host-version"], NONE, &["--host"], false, NONE)
        }
        ("sessions", Some("prune")) => s(&["--older-than"], NONE, &["--older-than"], false, NONE),
        ("export", None) => s(&["--ask", "--out"], NONE, &["--ask", "--out"], false, NONE),
        _ => None,
    }
}

/// Which commands take a subcommand at all.
fn takes_sub(cmd: &str) -> bool {
    matches!(cmd, "profile" | "ask" | "eval" | "seal" | "ledger" | "deltas" | "sessions")
}

const COMMANDS: [&str; 11] =
    ["init", "sync", "profile", "ask", "eval", "seal", "ledger", "deltas", "sessions", "export"
     , "--version"];

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

/// Global options are accepted anywhere, which is what argparse's hoisting
/// achieved for the Python CLI.
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
    // `--version` wins wherever it appears, as argparse's does.
    if argv.iter().any(|a| a == "--version") && !argv.iter().any(|a| a == "show") {
        return Run { code: 0, out: format!("ringframe {VERSION}\n"), err: String::new() };
    }
    let ns = match parse(argv) {
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
        Outcome::Needs(n) => (
            3,
            json!({
                "needs_input": n.reason,
                "candidates": if concise { output::candidates(&n.candidates) }
                              else { json!(n.candidates) },
            }),
        ),
        Outcome::Refused(codes) => {
            (2, json!({"error": "seal.refused", "refusal_codes": codes}))
        }
        Outcome::Error(code, detail) => (2, json!({"error": code, "detail": detail})),
    };
    Run { code, out: emit(&body, ns.json, concise), err: String::new() }
}

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
    macro_rules! ask_try {
        ($e:expr) => {
            match $e {
                Ok(v) => v,
                Err(e) => bail!(from_ask_error(e.into())),
            }
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
            match workspace::install_config(source.as_deref()) {
                Ok(v) => (ws, Outcome::Ok(0, v)),
                Err(e) => (ws, Outcome::Error(e.code, e.detail)),
            }
        }
        ("profile", Some("show")) => {
            let host = json!({"name": ns.one("--host"), "version": ns.one("--version").unwrap_or("")});
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
            (ws, Outcome::Ok(0, out))
        }
        ("ask", Some(what)) => ask_command(ns, ws, actor, what, read_stdin),
        ("deltas", Some(what)) => deltas_command(ns, ws, what),
        ("eval", Some("list")) => match evaluate::list_records(&ws) {
            Ok(evals) => (ws, Outcome::Ok(0, json!({"evals": evals}))),
            Err(e) => (ws, from_ask_error(e)),
        },
        ("eval", Some("open")) => {
            let out = evaluate::open_eval(&ws, evaluate::Open {
                anchor: ns.one("--anchor"),
                subject_kind: ns.one("--subject-kind"),
                subject_ref: ns.one("--subject-ref"),
                actor: Some(actor),
            });
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
            let out = evaluate::close_eval(
                &ws, ns.one("--eval").unwrap_or_default(), &intent, &judgements, Some(&actor),
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
                (ws, Outcome::Ok(if clean { 0 } else { 2 },
                                 json!({"findings": findings, "clean": clean})))
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
                Err(e) => return (workspace::resolve(None, None).expect("cwd"),
                                  Outcome::UsageDetail(e)),
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
        ("sessions", Some("prune")) => {
            match sessions::prune(&ws, ns.one("--older-than").unwrap_or_default()) {
                Ok(removed) => (ws, Outcome::Ok(0, json!({"removed": removed}))),
                Err(e) => (ws, Outcome::UsageDetail(e)),
            }
        }
        ("export", None) => {
            let out = export(&ws, ns.one("--ask").unwrap_or_default(), Path::new(ns.one("--out").unwrap_or_default()));
            match out {
                Ok(v) => (ws, Outcome::Ok(0, v)),
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
            done!(ask::compile(&ws, ask::Compile {
                staged: &staged,
                title: ns.one("--title").unwrap_or_default(),
                capability: ns.one("--capability").unwrap_or_default(),
                classification,
                route,
                host,
                links: links_of(&ns.all("--link")),
                limitations: ns.all("--limitation"),
                actor: Some(actor),
            }))
        }
        "confirm" => done!(ask::confirm(&ws, &id, Some(&actor))),
        "unanswered" => done!(ask::unanswered(&ws, &id, ns.one("--reason"), Some(&actor))),
        "cancel" => done!(ask::cancel(
            &ws, &id, ns.one("--reason"), Some(&actor), ns.has("--attributed")
        )),
        "submitted" => done!(ask::submitted(&ws, &id, ns.has("--as-modified"), Some(&actor))),
        "preflight" => done!(ask::preflight(&ws)),
        "copy" => match ask::prompt_text(&ws, &id) {
            Ok(text) => (ws, Outcome::Ok(0, Value::String(text))),
            Err(e) => (ws, from_ask_error(e)),
        },
        "delivery" => {
            if ns.has("--from-hook") {
                // A hook must never fail the host turn.
                let recorded = (|| -> Result<(Workspace, Option<Value>), String> {
                    let payload: Value =
                        serde_json::from_str(&read_stdin()).map_err(|e| e.to_string())?;
                    let ws = hook_workspace(ns.workspace.as_ref(), ws, &payload)?;
                    let rec =
                        ask::delivery_from_hook(&ws, &payload).map_err(|e| e.to_string())?;
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
        "show" => done!(ask::show(&ws, ns.one("--ask"), ns.one("--session"))),
        _ => done!(ask::resolve(&ws, ns.one("--session"), None)),
    }
}

fn deltas_command(ns: &Parsed, ws: Workspace, what: &str) -> (Workspace, Outcome) {
    let domain = ns.one("--domain").unwrap_or(deltas::DEFAULT_DOMAIN).to_string();
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
            let listed = config_try!(deltas::domains(Some(&ws)));
            (ws, Outcome::Ok(0, json!({"domains": listed})))
        }
        "list" => {
            if ns.has("--effective") {
                let listing = config_try!(deltas::effective(Some(&ws), &domain));
                let mut out = serde_json::Map::new();
                for (id, entry) in listing {
                    out.insert(id, entry);
                }
                return (ws, Outcome::Ok(0, Value::Object(out)));
            }
            let hosts = match ns.one("--host") {
                Some(h) => vec![h.to_string()],
                None => config_try!(deltas::host_catalog_names()),
            };
            let wanted = ns.one("--capability");
            let mut entries: Vec<Value> = Vec::new();
            for h in hosts {
                let cat = config_try!(deltas::load_host_catalog(&h, Some(&ws)));
                for e in cat["entries"].as_array().into_iter().flatten() {
                    if wanted.is_none_or(|c| e["capability"] == c) {
                        entries.push(e.clone());
                    }
                }
            }
            let catalog = config_try!(deltas::load_practice_catalog(&domain, Some(&ws)));
            (ws, Outcome::Ok(0, json!({
                "host": entries, "practice": catalog["entries"], "concerns": catalog["concerns"]
            })))
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
                Some(&ws),
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

/// One Ask's artifacts and its slice of the ledger, as a tar a person can send
/// somewhere else.
fn export(ws: &Workspace, ask_id: &str, out: &Path) -> Result<Value, AskError> {
    let lines: Vec<u8> = store::events(ws)?
        .iter()
        .filter(|e| e["id"] == ask_id)
        .flat_map(|e| {
            let mut line = store::canonical(e);
            line.push(b'\n');
            line
        })
        .collect();
    if lines.is_empty() {
        return Err(AskError::Ledger(store::LedgerError::new_public("ask.not_found", ask_id)));
    }
    // Staged into one directory and handed to `tar`, so the archive is whatever
    // this machine's tar writes rather than a format of our own.
    let staging = ws.rf_dir().join("tmp").join(format!("export-{}", ids::random_hex(4)));
    let inner = staging.join(ask_id);
    let outcome = (|| -> Result<Value, AskError> {
        std::fs::create_dir_all(&inner)?;
        let mut names: Vec<String> = Vec::new();
        let mut sources: Vec<PathBuf> = std::fs::read_dir(ws.rf_dir().join("asks").join(ask_id))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        sources.sort();
        for p in sources {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            std::fs::copy(&p, inner.join(&name))?;
            names.push(format!("{ask_id}/{name}"));
        }
        std::fs::write(inner.join("ledger.jsonl"), &lines)?;
        names.push(format!("{ask_id}/ledger.jsonl"));
        let status = std::process::Command::new("tar")
            .arg("-cf")
            .arg(out)
            .arg("-C")
            .arg(&staging)
            .args(&names)
            .status()?;
        if !status.success() {
            return Err(AskError::Ledger(store::LedgerError::new_public(
                "export.tar",
                format!("tar refused to write {}", out.display()),
            )));
        }
        Ok(json!({"ask_id": ask_id, "out": out.to_string_lossy(), "files": names.len()}))
    })();
    let _ = std::fs::remove_dir_all(&staging);
    outcome
}
