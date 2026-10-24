//! The crate as a library: one typed call for each thing a caller records or
//! reads.
//!
//! Every call takes the workspace, prints nothing, and returns its result or an
//! [`Error`] carrying RingFrame's refusal code. Each one records what the CLI
//! records for the same inputs.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ask::AskError;
use crate::config::Overrides;
use crate::seal::SealError;
pub use crate::workspace::Workspace;
use crate::{ask, evaluate, seal, store, tasks};

/// Why a call wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Refused, with its code: `ask.*`, `eval.*`, `seal.*`, `ledger.*` or
    /// `workspace.*`.
    Refused { code: String, detail: String },
    /// It needs something only the person can give.
    Needs(String),
    /// The Seal was not written, for every one of these codes. The refusal
    /// itself is recorded.
    SealRefused(Vec<String>),
}

impl Error {
    /// The code the CLI reports it with.
    pub fn code(&self) -> &str {
        match self {
            Error::Refused { code, .. } => code,
            Error::Needs(_) => "needs_input",
            Error::SealRefused(_) => "seal.refused",
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Refused { code, detail } => write!(f, "{code}: {detail}"),
            Error::Needs(reason) => write!(f, "needs_input: {reason}"),
            Error::SealRefused(codes) => write!(f, "seal.refused: {}", codes.join(", ")),
        }
    }
}

impl std::error::Error for Error {}

impl From<AskError> for Error {
    fn from(e: AskError) -> Self {
        match e {
            AskError::Ledger(l) => Error::Refused { code: l.code, detail: l.detail },
            AskError::Workspace(w) => Error::Refused { code: w.code, detail: w.detail },
            AskError::Needs(n) => Error::Needs(n.reason),
        }
    }
}

impl From<SealError> for Error {
    fn from(e: SealError) -> Self {
        match e {
            SealError::Refused(r) => Error::SealRefused(r.codes),
            SealError::Other(o) => o.into(),
        }
    }
}

/// The person acting, as the CLI records one who names no actor.
fn person() -> Value {
    json!({"kind": "human", "id": "local-user", "authority": "interactive"})
}

fn str_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

/// The workspace at a project's root, as `--workspace <root>` names it.
pub fn workspace(root: &Path) -> Result<Workspace, Error> {
    crate::workspace::resolve(None, Some(root)).map_err(|e| Error::Refused {
        code: "workspace.io".into(),
        detail: format!("{}: {e}", root.display()),
    })
}

fn workspace_error(e: crate::workspace::WorkspaceError) -> Error {
    Error::Refused { code: e.code, detail: e.detail }
}

/// Set the project up, as `ringframe init` does; refused outside Git. One set
/// up already stays as it is.
pub fn init(ws: &Workspace) -> Result<(), Error> {
    crate::workspace::require_git(ws).map_err(workspace_error)?;
    ws.ensure()
        .map_err(|e| Error::Refused { code: "workspace.io".into(), detail: e.to_string() })?;
    Ok(())
}

/// What `host`'s profile says, with its digest and the plans this project
/// holds, as `profile show` prints it.
pub fn profile(ws: &Workspace, host: &str) -> Result<Value, Error> {
    let mut out =
        crate::profiles::for_host(&json!({"name": host, "version": ""})).map_err(config_error)?;
    let name = out["host"].as_str().unwrap_or("unknown").to_string();
    out["sha256"] = json!(crate::profiles::sha256(&name).map_err(config_error)?);
    out["plans"] = json!(crate::workspace::plans(ws));
    Ok(out)
}

/// Every practice domain, every catalog with its entries, and what Eval judges
/// must not do, through these layers: `deltas domains` and `deltas list`.
pub fn catalogs(overrides: &Overrides) -> Result<Value, Error> {
    let domains = crate::deltas::domains(overrides).map_err(config_error)?;
    let listed = crate::deltas::list(overrides).map_err(config_error)?;
    let deny = overrides.eval_deny().map(|(from, rules)| (from, rules.to_vec()));
    Ok(json!({"domains": domains, "catalogs": listed,
              "judges": tasks::judges(deny.as_ref())}))
}

/// What `sync --check` says: the configuration installed against the latest.
pub fn config_check() -> Result<Value, Error> {
    crate::workspace::check_config().map_err(workspace_error)
}

/// Install the latest configuration, as `sync` does.
pub fn config_sync() -> Result<Value, Error> {
    crate::workspace::install_config(None).map_err(workspace_error)
}

// ---- What a harness was seen doing ------------------------------------------------

/// Who saw it, in which harness and session.
pub struct Seen<'a> {
    pub by: &'a str,
    pub host: &'a str,
    pub session: &'a str,
}

/// A prompt seen submitted: what the harness received, then its other records
/// of the same submission. Says whose submission it is, if any Ask's.
pub fn observe_prompt(
    ws: &Workspace,
    seen: &Seen<'_>,
    layers: &[String],
    host_version: Option<&str>,
) -> Result<Value, Error> {
    crate::workspace::require_initialized(ws).map_err(workspace_error)?;
    let (by, host, session) = (seen.by, seen.host, seen.session);
    Ok(crate::observe::layered(ws, by, host, session, layers, host_version)?)
}

/// A tool call seen, and how it ended; the harness's profile says which
/// tools RingFrame makes something of.
pub fn observe_tool(
    ws: &Workspace,
    seen: &Seen<'_>,
    tool: &str,
    outcome: &str,
    command: Option<&str>,
) -> Result<Value, Error> {
    crate::workspace::require_initialized(ws).map_err(workspace_error)?;
    let (by, host, session) = (seen.by, seen.host, seen.session);
    Ok(crate::observe::tool(ws, by, host, session, tool, outcome, command)?)
}

// ---- Ask --------------------------------------------------------------------------

/// Refuses a workspace no Ask could finish: no Git, or RingFrame never set up.
pub fn preflight(ws: &Workspace) -> Result<(), Error> {
    ask::preflight(ws)?;
    Ok(())
}

/// Another record this Ask points at: `follows`, `remediates` or `revises`.
pub struct Link<'a> {
    pub rel: &'a str,
    pub id: &'a str,
}

/// The harness the Ask is for.
pub struct Harness<'a> {
    pub name: &'a str,
    pub version: Option<&'a str>,
    pub session_ref: Option<&'a str>,
}

/// What the work is: its tasks, what it delivers, and what it may do.
pub struct Classification {
    pub task: Vec<String>,
    pub result: String,
    pub effects: Vec<String>,
}

/// Who chose: the person, or Sense with how sure it was of each choice.
pub enum Chooser {
    Person,
    Sense { version: String, probabilities: std::collections::BTreeMap<String, f64> },
}

impl Chooser {
    fn recorded(&self) -> Value {
        match self {
            Chooser::Person => json!({"by": "person"}),
            Chooser::Sense { version, probabilities } => {
                json!({"by": format!("sense@{version}"), "probabilities": probabilities})
            }
        }
    }
}

/// What the person sent, from the one dialog of the tool they use. Sending is
/// the confirmation.
pub struct Compile<'a> {
    /// The person's words, kept as they are.
    pub source: &'a str,
    pub harness: Harness<'a>,
    /// The route: a capability of the harness's profile.
    pub capability: &'a str,
    pub classification: Classification,
    /// The plan the work follows, by its path (`plans/<slug>/plan.md`).
    pub plan: Option<&'a str>,
    /// The practice rules chosen, by id.
    pub practices: Vec<String>,
    pub classified_by: Chooser,
    /// Sense's probabilities are the scores trimming drops by; a rule the
    /// person added scores above all of them.
    pub selected_by: Chooser,
    pub links: Vec<Link<'a>>,
    pub overrides: &'a Overrides,
    /// The tool, by name; recorded as where the person confirmed it.
    pub via: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Compiled {
    pub ask_id: String,
    /// `prompt.txt`: the words, then the practices, then the route rules.
    pub prompt: String,
    pub prompt_path: PathBuf,
    /// Practice rules dropped to fit the harness's limit, lowest score first.
    pub trimmed: Vec<String>,
}

fn compile_request<'a>(req: &'a Compile<'a>) -> crate::compile::Request<'a> {
    let c = &req.classification;
    crate::compile::Request {
        source: req.source.as_bytes(),
        host: json!({"name": req.harness.name, "version": req.harness.version,
                     "session_ref": req.harness.session_ref}),
        capability: req.capability,
        classification: json!({"task": c.task, "result": c.result, "effects": c.effects}),
        plan: req.plan,
        practices: req.practices.clone(),
        classified_by: req.classified_by.recorded(),
        selected_by: req.selected_by.recorded(),
        links: req.links.iter().map(|l| json!({"rel": l.rel, "id": l.id})).collect(),
        overrides: req.overrides,
        via: req.via,
    }
}

/// What a send would type, before it is sent: nothing is recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct Composed {
    /// Exactly what is typed: the route's command, then the prompt.
    pub text: String,
    /// Practice rules dropped to fit the harness's limit, lowest score first.
    pub trimmed: Vec<String>,
    /// How the harness takes it, as the Ask will record it.
    pub delivery: Value,
}

pub fn compose(ws: &Workspace, req: &Compile<'_>) -> Result<Composed, Error> {
    let out = crate::compile::compose(ws, &compile_request(req))?;
    Ok(Composed { text: out.typed, trimmed: out.trimmed, delivery: out.delivery })
}

pub fn compile(ws: &Workspace, req: Compile<'_>) -> Result<Compiled, Error> {
    let out = crate::compile::compile(ws, compile_request(&req))?;
    Ok(Compiled {
        ask_id: str_of(&out, "ask_id"),
        prompt: str_of(&out, "prompt"),
        prompt_path: PathBuf::from(str_of(&out, "prompt_path")),
        trimmed: out["trimmed"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
    })
}

/// The kinds of work an Ask can name, in the order their rules are shown.
pub fn tasks() -> Vec<&'static str> {
    crate::deltas::PHASES.iter().map(|(task, _)| *task).collect()
}

/// A rule an Ask can carry.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub id: String,
    pub label: String,
    pub text: String,
    /// When it matters; a person's own rule may not say.
    pub when: Option<String>,
    pub family: Option<String>,
    /// The layer that last set it: `shipped`, `repo`, or a layer of the
    /// person's.
    pub layer: String,
}

/// The rules for one route and kind of work.
#[derive(Debug, Clone, PartialEq)]
pub struct Rules {
    /// Every practice rule for the tasks, in shipped order: the ones to
    /// choose from.
    pub practices: Vec<Rule>,
    /// Every rule of the route; each prompt carries them all.
    pub route: Vec<Rule>,
}

fn rule(e: &Value) -> Rule {
    let some = |key: &str| e[key].as_str().filter(|s| !s.trim().is_empty()).map(str::to_string);
    Rule {
        id: str_of(e, "id"),
        label: crate::deltas::label(e),
        text: crate::deltas::squeeze(&str_of(e, "text")),
        when: some("when"),
        family: some("family"),
        layer: some("layer").unwrap_or_else(|| "shipped".into()),
    }
}

fn config_error(e: crate::config::ConfigError) -> Error {
    Error::Refused { code: "config.error".into(), detail: e.0 }
}

pub fn rules(
    ws: &Workspace,
    host: &str,
    capability: &str,
    classification: &Classification,
    overrides: &Overrides,
) -> Result<Rules, Error> {
    let layers = crate::rules::with_repo(ws, overrides).map_err(config_error)?;
    let c = json!({"task": classification.task, "result": classification.result,
                   "effects": classification.effects});
    let route = crate::rules::route_rules(&layers, host, capability, &c).map_err(config_error)?;
    let practices =
        crate::rules::practice_rules(&layers, &classification.task).map_err(config_error)?;
    Ok(Rules {
        practices: practices.iter().map(rule).collect(),
        route: route.iter().map(rule).collect(),
    })
}

/// Move one of the person's rules into the project's `.fab7/rules.toml`,
/// shared through its Git. Returns the rule as moved, to take out of the
/// person's own layer.
pub fn rules_share(ws: &Workspace, id: &str, overrides: &Overrides) -> Result<Value, Error> {
    Ok(crate::rules::share(ws, id, overrides)?)
}

/// What is typed into a harness for an Ask, and how it has to be sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub text: String,
    pub delivery: Value,
}

/// The Ask's prompt as `host` takes it: as compiled for the harness it was
/// composed in, or with that host's command for the same capability. No host
/// is the harness it was composed in.
pub fn prompt(ws: &Workspace, ask_id: &str, host: Option<&str>) -> Result<Prompt, Error> {
    let compiled_for = match host {
        Some(h) => h.to_string(),
        None => compiled_host(ws, ask_id)?,
    };
    let out = ask::prompt_for_host(ws, ask_id, &compiled_for)?;
    Ok(Prompt { text: str_of(&out, "prompt"), delivery: out["delivery"].clone() })
}

fn compiled_host(ws: &Workspace, ask_id: &str) -> Result<String, Error> {
    events(ws)?
        .iter()
        .find(|e| e["type"] == "ask.compiled" && e["id"] == ask_id)
        .map(|e| str_of(&e["data"]["host"], "name"))
        .ok_or_else(|| Error::Refused { code: "ask.not_compiled".into(), detail: ask_id.into() })
}

// ---- Eval -------------------------------------------------------------------------

pub struct EvalOpen<'a> {
    /// An Ask of the unit of work to judge; None judges every open Ask.
    pub ask: Option<&'a str>,
    /// Who opens it. RingFrame gathers the change as it opens.
    pub host: &'a str,
    /// Model and effort per role, recorded as given.
    pub agents: Option<Value>,
    /// Their `eval.deny` is what the judges must not do.
    pub overrides: &'a Overrides,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvalOpened {
    pub eval_id: String,
    pub brief_path: PathBuf,
}

pub fn eval_open(ws: &Workspace, req: EvalOpen<'_>) -> Result<EvalOpened, Error> {
    let out = evaluate::open_eval(
        ws,
        evaluate::Open {
            actor: Some(person()),
            agents: req.agents,
            host: Some(req.host),
            ask: req.ask,
            deny: req.overrides.eval_deny().map(|(from, rules)| (from, rules.to_vec())),
            ..evaluate::Open::default()
        },
    )?;
    Ok(EvalOpened {
        eval_id: str_of(&out, "eval_id"),
        brief_path: PathBuf::from(str_of(&out, "brief_path")),
    })
}

/// A task handed out to a judge.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub task_id: String,
    /// `map` or `reduce`.
    pub kind: String,
    /// `map`, `reduce` or `confirm`.
    pub role: String,
    /// What to tell the judge.
    pub prompt: String,
    /// The brief the judge reads.
    pub brief: PathBuf,
    /// Where the judge writes its answer.
    pub answer: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EvalStep {
    /// A task handed out now.
    Task(Task),
    /// Every ready task of the role is out, or none is ready yet; `checks`
    /// still running, which the verdict waits for.
    Waiting { remaining: usize, checks: Vec<String> },
    /// Every task is in, and the record is settled.
    Settled { verdict: String, confidence: Option<f64> },
}

/// One ready task for a judge, of `role` (`map`, `reduce` or `confirm`)
/// when given. A task out past its time goes out once more, then fails.
/// Once every task is in, the Eval settles.
pub fn eval_task(ws: &Workspace, eval_id: &str, role: Option<&str>) -> Result<EvalStep, Error> {
    let out = tasks::take(ws, eval_id, role)?;
    if out["state"] == "done" {
        let record = evaluate::load_record(ws, eval_id)?.unwrap_or_default();
        return Ok(EvalStep::Settled {
            verdict: str_of(&record, "verdict"),
            confidence: record["confidence"].as_f64(),
        });
    }
    if out["state"] == "waiting" {
        let checks = out["checks"].as_array().into_iter().flatten();
        return Ok(EvalStep::Waiting {
            remaining: out["remaining"].as_u64().unwrap_or(0) as usize,
            checks: checks.filter_map(Value::as_str).map(str::to_string).collect(),
        });
    }
    let t = &out["task"];
    let text = |key: &str| t[key].as_str().map(str::to_string);
    Ok(EvalStep::Task(Task {
        task_id: str_of(t, "task_id"),
        kind: str_of(t, "kind"),
        role: str_of(t, "role"),
        prompt: str_of(t, "prompt"),
        brief: PathBuf::from(str_of(t, "brief")),
        answer: PathBuf::from(str_of(t, "answer")),
        model: text("model"),
        effort: text("effort"),
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    /// The judge is told what to fix and may try again.
    Refused,
    /// Refused on its last attempt: the task is recorded failed.
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Submitted {
    pub task_id: String,
    pub outcome: Outcome,
    pub attempt: usize,
    /// Attempts left.
    pub left: usize,
    /// What the checks found, each naming where.
    pub errors: Vec<Value>,
    /// On a last attempt kept in part: the windows or requirements left
    /// unjudged.
    pub unjudged: Vec<String>,
}

/// Check the answer a judge wrote, and record it.
pub fn eval_submit(ws: &Workspace, task_id: &str, answer: &Path) -> Result<Submitted, Error> {
    let (code, out) = tasks::submit_from(ws, task_id, answer)?;
    let outcome = match code {
        0 => Outcome::Accepted,
        2 => Outcome::Refused,
        _ => Outcome::Failed,
    };
    let strings = |key: &str| -> Vec<String> {
        out[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    };
    let count = |key: &str| out[key].as_u64().unwrap_or(0) as usize;
    Ok(Submitted {
        task_id: str_of(&out, "task_id"),
        outcome,
        attempt: count("attempt"),
        left: count("left"),
        errors: out["errors"].as_array().cloned().unwrap_or_default(),
        unjudged: strings("unjudged"),
    })
}

/// Only a person voids an Eval, and says why.
pub fn eval_void(ws: &Workspace, eval_id: &str, reason: &str) -> Result<(), Error> {
    evaluate::void_eval(ws, eval_id, reason, Some(&person()))?;
    Ok(())
}

/// The settled record (`ringframe.eval/1`), or None while the Eval is open.
pub fn eval_record(ws: &Workspace, eval_id: &str) -> Result<Option<Value>, Error> {
    Ok(evaluate::load_record(ws, eval_id)?)
}

// ---- Seal -------------------------------------------------------------------------

pub struct SealCreate<'a> {
    /// `accepted`, `rejected`, `deferred` or `abandoned`.
    pub disposition: &'a str,
    /// The Eval it records; None takes the latest over the open Asks.
    pub eval_id: Option<&'a str>,
    /// Why, from the disposition's list (`seal::reasons`); accepted takes none.
    pub reason: Option<&'a str>,
    pub note: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sealed {
    pub seal_id: String,
    /// `ringframe.seal/1`, as published.
    pub receipt: Value,
}

pub fn seal_create(ws: &Workspace, req: SealCreate<'_>) -> Result<Sealed, Error> {
    let receipt =
        seal::create(ws, req.disposition, req.eval_id, req.reason, req.note, Some(&person()))?;
    Ok(Sealed { seal_id: str_of(&receipt, "seal_id"), receipt })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealCheck {
    /// The receipt and its Eval record are as written.
    pub fresh: bool,
    /// The work is still what was sealed.
    pub subject_matches: bool,
    /// Why it is not fresh.
    pub codes: Vec<String>,
    /// Everything the check says, as `seal check` prints it: the
    /// disposition, its basis and the Eval it records.
    pub report: Value,
}

pub fn seal_check(ws: &Workspace, seal_id: &str) -> Result<SealCheck, Error> {
    let report = seal::check(ws, seal_id)?;
    Ok(SealCheck {
        fresh: report["fresh"] == json!(true),
        subject_matches: report["subject_matches"] == json!(true),
        codes: report["codes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        report,
    })
}

// ---- The ledger -------------------------------------------------------------------

/// Every Ask, as `ask list` lists them.
pub fn asks(ws: &Workspace) -> Result<Vec<Value>, Error> {
    Ok(ask::list_asks(ws)?)
}

/// Every Eval, as `eval list` lists them.
pub fn evals(ws: &Workspace) -> Result<Vec<Value>, Error> {
    Ok(evaluate::list_records(ws)?)
}

/// The ledger's events, in order.
pub fn events(ws: &Workspace) -> Result<Vec<Value>, Error> {
    store::events(ws).map_err(|l| Error::Refused { code: l.code, detail: l.detail })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Overrides;
    use crate::testing::{answer_task, commit, eval_bench, repo, ws_for};

    const PLAN: &str = "# Uptime\n\n## Phase 1\n\n| # | Step | Done when |\n| --- | --- | --- |\n\
                        | 1 | Add `uptime_seconds` to the server. | A test calls `uptime_seconds`. |\n";
    const WORDS: &str = "Build Phase 1 of plans/uptime/plan.md, step 1.";

    /// The same steps, through the CLI as weftd ran them, or through this API.
    #[derive(Clone, Copy, PartialEq)]
    enum Via {
        Cli,
        Api,
    }

    fn cli(ws: &Workspace, args: &[&str]) -> Value {
        let root = ws.root.to_string_lossy().to_string();
        let argv: Vec<String> =
            ["--workspace", root.as_str()].iter().chain(args).map(|s| s.to_string()).collect();
        let run = crate::cli::run(&argv, &mut String::new);
        assert!(!run.out.is_empty(), "{args:?}: {}", run.err);
        serde_json::from_str(&run.out).unwrap_or(Value::String(run.out))
    }

    fn request<'a>(source: &'a str, over: &'a Overrides) -> Compile<'a> {
        Compile {
            source,
            harness: Harness { name: "claude-code", version: Some("2.1.260"), session_ref: None },
            capability: "native_goal",
            classification: Classification {
                task: vec!["implement".into()],
                result: "workspace_change".into(),
                effects: vec!["write".into()],
            },
            plan: Some("plans/uptime/plan.md"),
            practices: Vec::new(),
            classified_by: Chooser::Person,
            selected_by: Chooser::Person,
            links: Vec::new(),
            overrides: over,
            via: "weft",
        }
    }

    /// Compiled through this API on either side: weftd never compiled
    /// through the CLI, and the Eval and Seal steps are what is compared.
    fn compiled(ws: &Workspace, _: Via) -> String {
        commit(&ws.root, &[("plans/uptime/plan.md", Some(PLAN))], "plan");
        let over = Overrides::default();
        let out = compile(ws, request(WORDS, &over)).unwrap();
        assert!(out.prompt_path.ends_with("prompt.txt"));
        out.ask_id
    }

    fn worked(ws: &Workspace) {
        let work = "export const uptime_seconds = () => process.uptime();\n";
        commit(&ws.root, &[("src/uptime.js", Some(work))], "work");
    }

    fn opened(ws: &Workspace, via: Via, ask_id: &str) -> String {
        match via {
            Via::Cli => {
                let out = cli(ws, &["eval", "open", "--host", "weft", "--ask", ask_id]);
                out["eval_id"].as_str().expect("opened").to_string()
            }
            Via::Api => {
                let req = EvalOpen {
                    ask: Some(ask_id),
                    host: "weft",
                    agents: None,
                    overrides: &Overrides::default(),
                };
                eval_open(ws, req).unwrap().eval_id
            }
        }
    }

    /// Hand out, answer and submit every task until the Eval settles.
    fn judged(ws: &Workspace, via: Via, eval_id: &str) {
        for _ in 0..40 {
            let tasks: Vec<String> = match via {
                Via::Cli => {
                    let out = cli(ws, &["eval", "next", "--eval", eval_id, "--parallel", "8"]);
                    if out["state"] == "done" {
                        return;
                    }
                    let tasks = out["tasks"].as_array().cloned().unwrap_or_default();
                    tasks.iter().map(|t| t["task_id"].as_str().unwrap().to_string()).collect()
                }
                Via::Api => match eval_task(ws, eval_id, None).unwrap() {
                    EvalStep::Settled { verdict, .. } => {
                        assert!(!verdict.is_empty());
                        return;
                    }
                    EvalStep::Task(t) => vec![t.task_id],
                    EvalStep::Waiting { .. } => panic!("nothing is out, so a task is ready"),
                },
            };
            for t in tasks {
                answer_task(ws, &t);
                match via {
                    Via::Cli => {
                        assert_eq!(cli(ws, &["eval", "submit", "--task", &t])["state"], "accepted")
                    }
                    Via::Api => {
                        let answer = ws.rf_dir().join(format!("tmp/eval-{eval_id}/out/{t}.json"));
                        assert_eq!(eval_submit(ws, &t, &answer).unwrap().outcome, Outcome::Accepted)
                    }
                }
            }
        }
        panic!("the Eval did not settle");
    }

    fn voided(ws: &Workspace, via: Via, eval_id: &str) {
        match via {
            Via::Cli => {
                let out = cli(ws, &["eval", "void", "--eval", eval_id, "--reason", "wrong change"]);
                assert_eq!(out["state"], "voided", "{out}");
            }
            Via::Api => eval_void(ws, eval_id, "wrong change").unwrap(),
        }
    }

    fn sealed(ws: &Workspace, via: Via, eval_id: &str) -> String {
        match via {
            Via::Cli => {
                let args = ["seal", "create", "--disposition", "accepted", "--eval", eval_id];
                let out = cli(ws, &[&args[..], &["--note", "ship it"]].concat());
                out["seal_id"].as_str().expect("sealed").to_string()
            }
            Via::Api => {
                let req = SealCreate {
                    disposition: "accepted",
                    eval_id: Some(eval_id),
                    reason: None,
                    note: Some("ship it"),
                };
                seal_create(ws, req).unwrap().seal_id
            }
        }
    }

    /// Everything one run recorded, compiled, voided, judged and sealed.
    fn everything(ws: &Workspace, via: Via) -> (String, String, String) {
        let ask_id = compiled(ws, via);
        worked(ws);
        let first = opened(ws, via, &ask_id);
        voided(ws, via, &first);
        let eval_id = opened(ws, via, &ask_id);
        judged(ws, via, &eval_id);
        let seal_id = sealed(ws, via, &eval_id);
        (ask_id, eval_id, seal_id)
    }

    /// Run the same steps in two repositories, one through the CLI and one
    /// through this API.
    fn both<T>(steps: impl Fn(&Workspace, Via) -> T) -> ((Workspace, T), (Workspace, T)) {
        let (a, b) = (repo(), repo());
        let (wa, wb) = (ws_for(a.path()), ws_for(b.path()));
        let (wa, wb) = (workspace(&wa.root).unwrap(), workspace(&wb.root).unwrap());
        let (ra, rb) = (steps(&wa, Via::Cli), steps(&wb, Via::Api));
        // The repositories go with this scope; what was read from them stays.
        let keep = |d: crate::testing::TempDir| std::mem::forget(d);
        keep(a);
        keep(b);
        ((wa, ra), (wb, rb))
    }

    /// A value with what differs between two runs of the same steps named
    /// instead: ids by the order they first appear, times, digests and
    /// commits, and the repository's own path.
    struct Mask {
        root: Vec<String>,
        ids: Vec<String>,
    }

    impl Mask {
        fn new(ws: &Workspace) -> Self {
            let root = ws.root.to_string_lossy().to_string();
            let real = std::fs::canonicalize(&ws.root).unwrap().to_string_lossy().to_string();
            Mask { root: vec![real, root], ids: Vec::new() }
        }

        fn value(&mut self, v: &Value) -> Value {
            match v {
                Value::String(s) => Value::String(self.text(s)),
                Value::Array(a) => Value::Array(a.iter().map(|x| self.value(x)).collect()),
                Value::Object(m) => {
                    Value::Object(m.iter().map(|(k, x)| (k.clone(), self.value(x))).collect())
                }
                other => other.clone(),
            }
        }

        fn text(&mut self, s: &str) -> String {
            let mut s = s.to_string();
            for r in self.root.clone() {
                s = s.replace(&r, "<root>");
            }
            let mut out = String::new();
            let mut word = String::new();
            for c in s.chars().chain(std::iter::once('\0')) {
                if c.is_ascii_alphanumeric() || c == '_' {
                    word.push(c);
                    continue;
                }
                out.push_str(&self.word(&std::mem::take(&mut word)));
                if c != '\0' {
                    out.push(c);
                }
            }
            if is_time(&out) { "<time>".into() } else { out }
        }

        fn word(&mut self, w: &str) -> String {
            if let Some((prefix, body)) = w.split_once('_')
                && body.len() == 26
                && body.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
            {
                let n = match self.ids.iter().position(|i| i == w) {
                    Some(n) => n,
                    None => {
                        self.ids.push(w.to_string());
                        self.ids.len() - 1
                    }
                };
                return format!("<{prefix}{n}>");
            }
            let hex = w.len() >= 7 && w.bytes().all(|b| b.is_ascii_hexdigit());
            if hex && w.bytes().any(|b| b.is_ascii_digit()) { "<hex>".into() } else { w.into() }
        }
    }

    fn is_time(s: &str) -> bool {
        s.len() == 24 && s.as_bytes()[10] == b'T' && s.ends_with('Z')
    }

    fn masked_events(ws: &Workspace) -> Vec<Value> {
        let mut mask = Mask::new(ws);
        crate::store::events(ws).unwrap().iter().map(|e| mask.value(e)).collect()
    }

    fn assert_same_events(cli: &Workspace, api: &Workspace) {
        let (c, a) = (masked_events(cli), masked_events(api));
        for (i, (c, a)) in c.iter().zip(&a).enumerate() {
            assert_eq!(c, a, "event {i}: the CLI recorded the first, the API the second");
        }
        assert_eq!(c.len(), a.len(), "as many events");
    }

    fn masked_file(ws: &Workspace, rel: &str) -> Value {
        let bytes = std::fs::read(ws.rf_dir().join(rel)).unwrap();
        Mask::new(ws).value(&serde_json::from_slice(&bytes).unwrap())
    }

    #[test]
    fn an_eval_opened_voided_judged_and_settled_records_what_the_cli_recorded() {
        eval_bench(|_| {
            let ((cw, c), (aw, a)) = both(|ws, via| {
                let ask_id = compiled(ws, via);
                worked(ws);
                let first = opened(ws, via, &ask_id);
                voided(ws, via, &first);
                let eval_id = opened(ws, via, &ask_id);
                judged(ws, via, &eval_id);
                eval_id
            });
            assert_same_events(&cw, &aw);
            let rel = |e: &str| format!("evals/{e}/record.json");
            assert_eq!(masked_file(&cw, &rel(&c)), masked_file(&aw, &rel(&a)));
            let record = eval_record(&aw, &a).unwrap().expect("settled");
            assert_eq!(record["eval_id"], json!(a));
        });
    }

    #[test]
    fn a_seal_created_records_what_the_cli_recorded() {
        eval_bench(|_| {
            let ((cw, (_, _, cs)), (aw, (_, _, a_s))) = both(everything);
            assert_same_events(&cw, &aw);
            let rel = |s: &str| format!("seals/{s}.json");
            assert_eq!(masked_file(&cw, &rel(&cs)), masked_file(&aw, &rel(&a_s)));
        });
    }

    #[test]
    fn the_reads_say_what_the_cli_says() {
        eval_bench(|_| {
            let (_, (aw, (aa, _, a_s))) = both(everything);
            let said = |args: &[&str]| cli(&aw, args);

            preflight(&aw).unwrap();
            let got = prompt(&aw, &aa, Some("codex")).unwrap();
            let want = said(&["ask", "copy", "--ask", &aa, "--host", "codex"]);
            assert_eq!(got.text, want["prompt"].as_str().unwrap());
            assert_eq!(got.delivery, want["delivery"]);
            let typed = prompt(&aw, &aa, Some("claude-code")).unwrap().text;
            assert!(typed.starts_with(&format!("/goal {WORDS}\n")), "{typed}");
            assert_eq!(json!(typed), said(&["ask", "copy", "--ask", &aa]));
            assert_eq!(
                prompt(&aw, &aa, None).unwrap(),
                prompt(&aw, &aa, Some("claude-code")).unwrap()
            );
            assert_eq!(prompt(&aw, "ask_none", None).unwrap_err().code(), "ask.not_compiled");

            let check = seal_check(&aw, &a_s).unwrap();
            let want = said(&["seal", "check", "--seal", &a_s]);
            assert_eq!((check.fresh, check.subject_matches), (true, true));
            assert_eq!(json!(check.codes), want["codes"]);
            assert_eq!(check.report, want);

            assert_eq!(json!(asks(&aw).unwrap()), said(&["ask", "list"])["asks"]);
            assert_eq!(json!(evals(&aw).unwrap()), said(&["eval", "list"])["evals"]);
            assert_eq!(events(&aw).unwrap(), crate::store::events(&aw).unwrap());
        });
    }

    #[test]
    fn a_refusal_surfaces_with_its_code() {
        eval_bench(|_| {
            let dir = repo();
            let bare = crate::workspace::resolve(Some(dir.path()), None).unwrap();
            assert_eq!(preflight(&bare).unwrap_err().code(), "workspace.uninitialized");
            let ws = ws_for(dir.path());
            let code = |e: Error| e.code().to_string();
            assert_eq!(code(eval_void(&ws, "evl_none", "").unwrap_err()), "eval.void_reason");
            assert_eq!(code(eval_void(&ws, "evl_none", "why").unwrap_err()), "eval.missing");
            let nowhere = ws.root.join("nothing.json");
            assert_eq!(
                code(eval_submit(&ws, "nothing", &nowhere).unwrap_err()),
                "eval.task_unknown"
            );
            let bad = SealCreate { disposition: "merged", eval_id: None, reason: None, note: None };
            assert_eq!(code(seal_create(&ws, bad).unwrap_err()), "seal.disposition");
            let none =
                SealCreate { disposition: "accepted", eval_id: None, reason: None, note: None };
            assert_eq!(
                seal_create(&ws, none).unwrap_err(),
                Error::SealRefused(vec!["seal.no_open_ask".into()])
            );
        });
    }

    #[test]
    fn rules_lists_the_practices_with_their_layer_when_and_family_and_the_route_rules() {
        eval_bench(|ws| {
            let layers = Overrides::parse(
                &json!([{"layer": "weft", "ringframe": {"deltas": {
                "practices/software-development": {"entries": [
                    {"id": "own.1", "text": "One commit per item.",
                     "applies_to": {"task": ["implement"]}}]}}}}])
                .to_string(),
            )
            .unwrap();
            let c = request(WORDS, &layers).classification;
            let got = rules(ws, "claude-code", "native_goal", &c, &layers).unwrap();
            let kiss = got.practices.iter().find(|r| r.id == "practice.kiss").expect("kiss");
            assert_eq!((kiss.label.as_str(), kiss.layer.as_str()), ("KISS", "shipped"));
            assert!(kiss.when.is_some() && kiss.family.as_deref() == Some("simplicity"));
            let own = got.practices.iter().find(|r| r.id == "own.1").expect("the person's own");
            assert_eq!((own.layer.as_str(), own.when.as_deref()), ("weft", None));
            let route: Vec<&str> = got.route.iter().map(|r| r.label.as_str()).collect();
            assert_eq!(route, ["Step Loop", "Done Check"]);
            assert!(got.route.iter().all(|r| r.layer == "shipped"));
            let on = Overrides::parse(
                &json!([{"layer": "eval", "ringframe": {"deltas": {
                "claude-code": {"entries": [{"id": "claude-code.native_goal.done_check",
                                             "text": "Show each check."}]}}}}])
                .to_string(),
            )
            .unwrap();
            let got = rules(ws, "claude-code", "native_goal", &c, &on).unwrap();
            let done = got.route.iter().find(|r| r.label == "Done Check").unwrap();
            assert_eq!((done.layer.as_str(), done.text.as_str()), ("eval", "Show each check."));

            let shared = rules_share(ws, "own.1", &layers).unwrap_err();
            assert_eq!(shared.code(), "rules.incomplete");
        });
    }

    #[test]
    fn compose_shows_the_exact_prompt_compile_then_records() {
        eval_bench(|ws| {
            commit(&ws.root, &[("plans/uptime/plan.md", Some(PLAN))], "plan");
            let over = Overrides::default();
            let mut req = request(WORDS, &over);
            req.practices = vec!["practice.kiss".into()];
            let before = events(ws).unwrap().len();
            let shown = compose(ws, &req).unwrap();
            assert_eq!(events(ws).unwrap().len(), before, "composing records nothing");
            assert!(shown.text.starts_with(&format!("/goal {WORDS}\n")), "{}", shown.text);
            assert!(shown.text.contains("- KISS:"), "{}", shown.text);
            let compiled = compile(ws, req).unwrap();
            let sent = prompt(ws, &compiled.ask_id, None).unwrap();
            assert_eq!((sent.text, sent.delivery), (shown.text.clone(), shown.delivery));
            assert!(shown.trimmed.is_empty());
            let mut bad = request(WORDS, &over);
            bad.practices = vec!["practice.none".into()];
            assert_eq!(compose(ws, &bad).unwrap_err().code(), "ask.practice_unknown");
        });
    }

    #[test]
    fn init_sets_a_repository_up_and_refuses_a_folder_with_no_git() {
        crate::testing::with_config_home(|_| {
            let dir = repo();
            let ws = workspace(dir.path()).unwrap();
            assert!(!ws.exists());
            init(&ws).unwrap();
            assert!(ws.exists());
            init(&ws).unwrap();
            let bare = crate::testing::tmp_dir();
            let refused = init(&workspace(bare.path()).unwrap()).unwrap_err();
            assert_eq!(refused.code(), "workspace.no_git");
        });
    }

    #[test]
    fn the_profile_and_the_catalogs_say_what_the_cli_says() {
        eval_bench(|ws| {
            let said = |args: &[&str]| cli(ws, args);
            assert_eq!(
                profile(ws, "claude-code").unwrap(),
                said(&["profile", "show", "--host", "claude-code", "--json"])
            );
            assert_eq!(
                profile(ws, "loom").unwrap(),
                said(&["profile", "show", "--host", "loom", "--json"])
            );
            let got = catalogs(&Overrides::default()).unwrap();
            let listed = said(&["deltas", "list", "--json"]);
            assert_eq!(got["domains"], said(&["deltas", "domains", "--json"])["domains"]);
            assert_eq!(
                (&got["catalogs"], &got["judges"]),
                (&listed["catalogs"], &listed["judges"])
            );
            assert_eq!(catalogs(&Overrides::default()).unwrap(), got);
        });
    }

    /// A prompt and a tool seen, through the CLI with the prompt on stdin, or
    /// through this API.
    fn observed(ws: &Workspace, via: Via) -> String {
        let ask_id = compiled(ws, via);
        let typed = prompt(ws, &ask_id, None).unwrap().text;
        let seen = Seen { by: "weft", host: "claude-code", session: "s1" };
        let head =
            ["observe", "prompt", "--by", "weft", "--host", "claude-code", "--session", "s1"];
        let tool = ["observe", "tool", "--by", "weft", "--host", "claude-code", "--session", "s1"];
        let tool = [&tool[..], &["--tool", "Bash", "--outcome", "succeeded", "--command", "ls"]];
        match via {
            Via::Cli => {
                let root = ws.root.to_string_lossy().to_string();
                let argv: Vec<String> = ["--workspace", root.as_str()]
                    .iter()
                    .chain(&head)
                    .map(|s| s.to_string())
                    .collect();
                let run = crate::cli::run(&argv, &mut || typed.clone());
                assert_eq!(run.code, 0, "{}", run.err);
                cli(ws, &tool.concat());
            }
            Via::Api => {
                let said = observe_prompt(ws, &seen, &[typed], None).unwrap();
                assert_eq!(said["submission"]["ask_id"], json!(ask_id), "{said}");
                observe_tool(ws, &seen, "Bash", "succeeded", Some("ls")).unwrap();
            }
        }
        ask_id
    }

    #[test]
    fn a_prompt_and_a_tool_observed_record_what_the_cli_recorded() {
        eval_bench(|_| {
            let ((cw, _), (aw, _)) = both(observed);
            assert_same_events(&cw, &aw);
            let bare = workspace(repo().path()).unwrap();
            let seen = Seen { by: "weft", host: "claude-code", session: "s1" };
            let refused = observe_prompt(&bare, &seen, &["hi".into()], None).unwrap_err();
            assert_eq!(refused.code(), "workspace.uninitialized");
        });
    }

    #[test]
    fn a_seal_through_the_library_holds_its_disposition_and_reason() {
        eval_bench(|ws| {
            let ask_id = compiled(ws, Via::Api);
            worked(ws);
            let eval_id = opened(ws, Via::Api, &ask_id);
            judged(ws, Via::Api, &eval_id);
            let req = SealCreate {
                disposition: "rejected",
                eval_id: Some(&eval_id),
                reason: Some("not_what_i_asked"),
                note: None,
            };
            let sealed = seal_create(ws, req).unwrap();
            assert_eq!(sealed.receipt["disposition"], "rejected");
            assert_eq!(sealed.receipt["reason"], "not_what_i_asked");
            let on_disk = std::fs::read(ws.rf_dir().join(format!("seals/{}.json", sealed.seal_id)));
            let on_disk: Value = serde_json::from_slice(&on_disk.unwrap()).unwrap();
            assert_eq!(
                (&on_disk["disposition"], &on_disk["reason"]),
                (&json!("rejected"), &json!("not_what_i_asked"))
            );
            assert!(seal_check(ws, &sealed.seal_id).unwrap().fresh);
        });
    }
}
