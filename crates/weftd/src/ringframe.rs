//! The bridge to RingFrame: its crate, through `ringframe::api`.
//!
//! Weft never writes `.fab7/rf/` itself; every record goes through RingFrame,
//! which validates, digests and refuses.

use std::path::Path;

use ringframe::api;
use ringframe::config::Overrides;

use serde_json::Value;
use weft_core::workspace::Needs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Refused { code: i32, message: String },
}

/// The layers Weft passes RingFrame, as RingFrame reads them.
fn overrides(layers: Option<&str>) -> Result<Overrides, Error> {
    match layers {
        Some(o) => Overrides::parse(o)
            .map_err(|e| Error::Refused { code: REFUSED_BY_A_RULE, message: e.0 }),
        None => Ok(Overrides::default()),
    }
}

/// Every practice and every rule, with the layer that last set each, and what
/// Eval judges must not do, through these override layers:
/// `{"domains": […], "catalogs": […], "judges": {"deny": […], "from": …}}`.
pub fn rules(layers: Option<&str>) -> Result<Value, Error> {
    api::catalogs(&overrides(layers)?).map_err(refused)
}

/// What RingFrame says of `host`: its routes, how each is delivered, its limits.
pub fn profile(project: &Path, host: &str) -> Result<Value, Error> {
    api::profile(&workspace(project)?, host).map_err(refused)
}

/// The Rules block an Ask on `host`'s route `capability` for a task of `task`
/// would carry, through these layers: `{"text", "kept", "left_out"}`, the
/// practices it offers and none left out.
pub fn preview(
    project: &Path,
    layers: Option<&str>,
    (host, capability, task): (&str, &str, &str),
) -> Result<Value, Error> {
    let result = if task == "plan" { "plan" } else { "workspace_change" };
    let classification = api::Classification {
        task: vec![task.into()],
        result: result.into(),
        effects: vec!["read".into(), "write".into()],
    };
    let ws = workspace(project)?;
    let rules =
        api::rules(&ws, host, capability, &classification, &overrides(layers)?).map_err(refused)?;
    let line = |r: &api::Rule| format!("- {}: {}", r.label, r.text);
    let mut text = vec!["Rules:".to_string()];
    text.extend(rules.practices.iter().map(line));
    if !rules.route.is_empty() {
        text.push("For this route:".into());
        text.extend(rules.route.iter().map(line));
    }
    Ok(serde_json::json!({"text": text.join("\n"), "kept": rules.practices.len(),
                          "left_out": 0}))
}

/// Whether RingFrame reads these override layers: every catalog listed
/// through them, which refuses what any call reading them would.
pub fn reads_overrides(layers: &str) -> Result<(), Error> {
    rules(Some(layers)).map(|_| ())
}

/// RingFrame's own exit code for "refused by a rule", as its help documents.
const REFUSED_BY_A_RULE: i32 = 2;

/// Set the project up for RingFrame, as a person would with `ringframe init`,
/// and say what it still needs if RingFrame refused. A hook never makes the
/// workspace; Weft does, when the project opens. One that is there already is
/// not asked about again.
pub fn set_up(project: &Path) -> Option<Needs> {
    if project.join(".fab7/rf").is_dir() {
        return None;
    }
    let done = api::workspace(project).and_then(|ws| api::init(&ws));
    done.err().map(|e| Needs::refused(Some(e.code())))
}

/// What RingFrame makes of one forwarded signal of `harness`'s `session`,
/// which for a prompt says whose submission it is. Weft writes no record
/// itself: RingFrame decides what the fact means.
pub fn observe(
    project: &Path,
    (harness, session): (&str, &str),
    call: &weft_core::signal::Forward,
) -> Result<Value, Error> {
    use weft_core::signal::Forward;
    let ws = workspace(project)?;
    let seen = api::Seen { by: "weft", host: harness, session };
    match call {
        Forward::Prompt { layers } => api::observe_prompt(&ws, &seen, layers, None),
        Forward::Tool { tool, outcome, command } => {
            api::observe_tool(&ws, &seen, tool, outcome, command.as_deref())
        }
    }
    // Only the refusal: nothing a prompt said is logged.
    .map_err(refused)
}

/// The tools RingFrame makes something of for this harness, its profile's
/// `[observe]`, or none when RingFrame cannot say.
pub fn observe_tools(project: &Path, host: &str) -> Option<Value> {
    let v = profile(project, host).ok()?;
    Some(v.get("observe").cloned().unwrap_or(Value::Null))
}

/// An Ask as the person sent it from Weft's sheet: their words as written,
/// and what they chose.
#[derive(Debug, Clone, PartialEq)]
pub struct AskDraft {
    pub words: String,
    pub harness: String,
    pub route: String,
    pub task: String,
    /// What doing it affects: Sense's, or reads and writes.
    pub effects: Vec<String>,
    pub plan: Option<String>,
    /// Practice rules, by id.
    pub rules: Vec<String>,
    /// The work it follows: `follows` or `remediates`, and that record's id.
    pub link: Option<(String, String)>,
    /// What Sense set in the sheet, when it answered.
    pub sense: Option<SenseShown>,
    /// The harness's session it is typed into, when Weft knows it.
    pub session: Option<String>,
}

/// Sense's answer and the defaults the sheet showed from it.
#[derive(Debug, Clone, PartialEq)]
pub struct SenseShown {
    pub answer: weft_sense::AskAnswer,
    pub route: Option<String>,
    pub task: Option<String>,
    pub rules: Vec<String>,
}

impl SenseShown {
    /// Who chose the route and task: Sense, when the person kept both.
    fn classified_by(&self, d: &AskDraft) -> api::Chooser {
        let kept = self.route.as_deref() == Some(&d.route) && self.task.as_deref() == Some(&d.task);
        let p = &self.answer.route.p;
        self.chooser(kept, p.iter().map(|(c, p)| (c.clone(), f64::from(*p))).collect())
    }

    /// Who chose the practice rules: Sense, when the person kept its own.
    fn selected_by(&self, d: &AskDraft) -> api::Chooser {
        let (mut kept, mut shown) = (d.rules.clone(), self.rules.clone());
        kept.sort();
        shown.sort();
        // A rule the convention decided was not scored, so it has no probability.
        let scored = self.answer.rules.iter().filter_map(|r| Some((r.id.clone(), f64::from(r.p?))));
        let p = scored.collect();
        self.chooser(kept == shown, p)
    }

    fn chooser(
        &self,
        kept: bool,
        probabilities: std::collections::BTreeMap<String, f64>,
    ) -> api::Chooser {
        match kept {
            true => api::Chooser::Sense { version: self.answer.sense.clone(), probabilities },
            false => api::Chooser::Person,
        }
    }
}

impl AskDraft {
    fn request<'a>(&'a self, overrides: &'a Overrides, offered: Vec<String>) -> api::Compile<'a> {
        let result = if self.task == "plan" { "plan" } else { "workspace_change" };
        let by = |s: &SenseShown, f: fn(&SenseShown, &AskDraft) -> api::Chooser| f(s, self);
        let classified_by =
            self.sense.as_ref().map_or(api::Chooser::Person, |s| by(s, SenseShown::classified_by));
        let selected_by =
            self.sense.as_ref().map_or(api::Chooser::Person, |s| by(s, SenseShown::selected_by));
        api::Compile {
            source: &self.words,
            harness: api::Harness {
                name: &self.harness,
                version: None,
                session_ref: self.session.as_deref(),
            },
            capability: &self.route,
            classification: api::Classification {
                task: vec![self.task.clone()],
                result: result.into(),
                effects: self.effects.clone(),
            },
            plan: self.plan.as_deref(),
            practices: self.rules.clone(),
            offered,
            classified_by,
            selected_by,
            links: self.link.iter().map(|(rel, id)| api::Link { rel, id }).collect(),
            overrides,
            via: "weft",
        }
    }
}

/// What Send would type for this Ask, recording nothing.
pub fn compose(project: &Path, d: &AskDraft, layers: Option<&str>) -> Result<api::Composed, Error> {
    let overrides = overrides(layers)?;
    api::compose(&workspace(project)?, &d.request(&overrides, Vec::new())).map_err(refused)
}

/// Compile the Ask, confirmed by the person's Send in Weft, and say its id.
pub fn compile(project: &Path, d: &AskDraft, layers: Option<&str>) -> Result<String, Error> {
    let overrides = overrides(layers)?;
    let ws = workspace(project)?;
    let offered = every_practice(&ws, &d.harness, Some(&d.route), &overrides)?;
    let offered = offered.into_iter().map(|r| r.id).collect();
    let compiled = api::compile(&ws, d.request(&overrides, offered)).map_err(refused)?;
    Ok(compiled.ask_id)
}

/// What Sense made of the words: nothing, as without a bundle that answers
/// an Ask or before any words; a failure, as when it took too long; or its
/// answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Sensed {
    Off,
    Failed,
    Answered(Box<weft_sense::AskAnswer>),
}

/// What Sense is asked to read: the words, and the route and plan the sheet
/// holds when the person picked a route or there is a plan (the plan as its
/// candidate text); the routes it may pick and the practice rules it may
/// propose.
pub struct Reading<'a> {
    pub words: &'a str,
    pub route: Option<&'a str>,
    pub plan: Option<&'a str>,
    pub routes: &'a [&'a str],
    pub rules: &'a [weft_sense::Rule],
}

/// Sense on a reading of the words.
pub type Sensing<'a> = &'a dyn Fn(&Reading) -> Sensed;

/// The plan Sense's plan head defaults to among `(name, candidate)` pairs,
/// or none: unsure, no plan head, or past its deadline.
pub type PlanPicking<'a> = &'a dyn Fn(&str, &[(String, String)]) -> Option<String>;

/// How many plans BM25 shortlists for the plan head; the last-used plan
/// may make one more.
pub const PLAN_SHORTLIST: usize = 5;

/// The chosen route and plan, the person's: a plan path, or `Some(None)` for
/// no plan.
pub type Picked<'a> = (Option<&'a str>, Option<&'a str>, Option<Option<&'a str>>);

/// What the Ask sheet offers for `harness` and these words: every route of
/// its profile; the kinds of work; the plans held here and the one Sense's
/// plan head defaults to; and, for the route and task given or their
/// defaults, the practice rules to choose from and the route's rules. With
/// Sense, its answer sets the defaults and proposes practices; a route the
/// person picked, or a plan the sheet holds, is read again with the words. A
/// route Sense does not pick is direct, or the first where the harness has
/// none: plan, goal and review are the person's to ask for. When Sense fails
/// no practice is chosen; without Sense every practice is offered.
pub fn sheet(
    project: &Path,
    layers: Option<&str>,
    words: (&str, &str),
    picked: Picked,
    sense: Sensing,
    pick_plan: PlanPicking,
) -> Result<Value, Error> {
    sheet_scoped(project, layers, words, picked, sense, pick_plan, None)
}

/// The [`sheet`] for an agent working in a project's folder (`scope`, see
/// [`plan_scope`]): Sense's plan head reads that project's plans and the
/// flat ones. A plan the words name is found among every plan.
pub fn sheet_scoped(
    project: &Path,
    layers: Option<&str>,
    words: (&str, &str),
    picked: Picked,
    sense: Sensing,
    pick_plan: PlanPicking,
    scope: Option<&str>,
) -> Result<Value, Error> {
    sheet_at((Stage::Full, scope), project, layers, words, picked, sense, pick_plan)
}

/// The project whose plans an agent working in `folder` reads: the first
/// folder of its path under `root`, when `plans/<that>/` is there. At the
/// root, or anywhere else, none: every plan.
pub fn plan_scope(root: &Path, folder: &Path) -> Option<String> {
    let (root, folder) = (root.canonicalize().ok()?, folder.canonicalize().ok()?);
    let first = folder.strip_prefix(&root).ok()?.components().next()?;
    let name = first.as_os_str().to_str()?;
    root.join("plans").join(name).is_dir().then(|| name.to_string())
}

/// The sheet as Sense's heads alone fill it, `"stage": "heads"`: its route,
/// task and effects, the plan only when the person picked one, and no rule
/// ranked or proposed. The full [`sheet`] follows with the rest.
pub fn heads_sheet(
    project: &Path,
    layers: Option<&str>,
    words: (&str, &str),
    picked: Picked,
    sense: Sensing,
) -> Result<Value, Error> {
    let no_plan = |_: &str, _: &[(String, String)]| None;
    sheet_at((Stage::Heads, None), project, layers, words, picked, sense, &no_plan)
}

/// How much of the sheet Sense fills: its heads alone, or everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Heads,
    Full,
}

fn sheet_at(
    (stage, scope): (Stage, Option<&str>),
    project: &Path,
    layers: Option<&str>,
    (harness, words): (&str, &str),
    (route, task, plan): Picked,
    sense: Sensing,
    pick_plan: PlanPicking,
) -> Result<Value, Error> {
    let ws = workspace(project)?;
    let profile = api::profile(&ws, harness).map_err(refused)?;
    let routes: Vec<&str> = profile["capabilities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["id"].as_str())
        .collect();
    let overrides = overrides(layers)?;
    let candidates = candidate_rules(&ws, harness, routes.first().copied(), &overrides)?;
    let classes: Vec<&str> = routes.iter().filter_map(|r| r.strip_prefix("native_")).collect();
    // The heads on the words alone first: Sense's own route.
    let reading = Reading { words, route: None, plan: None, routes: &classes, rules: &[] };
    let first = sense(&reading);
    let failed = first == Sensed::Failed;
    let first = match first {
        Sensed::Answered(a) => Some(*a),
        Sensed::Off | Sensed::Failed => None,
    };
    let fallback = routes.iter().find(|r| **r == "native_direct").or(routes.first());
    let picked_route = route;
    let route = route
        .map(str::to_string)
        .or_else(|| first.as_ref().and_then(|a| SenseDefaults::of(a).route))
        .or_else(|| fallback.map(|r| r.to_string()))
        .unwrap_or_default();
    let route = route.as_str();
    // A plan folder with no plan.md has no path, and is no plan to work.
    let plans: Vec<weft_core::ask::Plan> = profile["plans"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let slug = p["slug"].as_str()?.to_string();
            let title = p["title"].as_str().unwrap_or(&slug).to_string();
            let paragraph = p["paragraph"].as_str().unwrap_or_default().to_string();
            let project = p["project"].as_str().map(str::to_string);
            let path = p["path"].as_str()?.into();
            Some(weft_core::ask::Plan { project, path, title, slug, paragraph })
        })
        .collect();
    let plan = match plan {
        Some(path) => plans.iter().find(|p| Some(p.path.as_str()) == path).cloned(),
        None if stage == Stage::Heads => None,
        // The plan the words name, with no model; else Sense's pick among
        // BM25's shortlist and the plan this harness's last Ask used.
        None => weft_core::ask::named(words, &plans).cloned().or_else(|| {
            let last = api::last_plan(&ws, harness).ok().flatten();
            let ours =
                |p: &&weft_core::ask::Plan| p.project.is_none() || p.project.as_deref() == scope;
            let scoped: Vec<weft_core::ask::Plan> =
                plans.iter().filter(|p| scope.is_none() || ours(p)).cloned().collect();
            let shortlist =
                weft_core::ask::shortlist(words, &scoped, PLAN_SHORTLIST, last.as_deref());
            let candidates: Vec<(String, String)> =
                shortlist.iter().map(|p| (p.name(), p.candidate())).collect();
            let name = pick_plan(words, &candidates);
            plans.iter().find(|p| Some(p.name()) == name).cloned()
        }),
    };
    // Then the rules, within what the sheet holds: the person's route, the plan.
    let class = route.strip_prefix("native_").filter(|r| classes.contains(r));
    let answer = match first {
        Some(first) if stage == Stage::Full => {
            let within = picked_route.is_some() || plan.is_some();
            let candidate = plan.as_ref().map(weft_core::ask::Plan::candidate);
            let only: Vec<&str> = class.into_iter().filter(|_| within).collect();
            let routes = if only.is_empty() { &classes } else { &only };
            let reading = Reading {
                words,
                route: class.filter(|_| within),
                plan: candidate.as_deref(),
                routes,
                rules: &candidates,
            };
            match sense(&reading) {
                // Sense's own route stays the one it read from the words.
                Sensed::Answered(rules) => {
                    Some(weft_sense::AskAnswer { route: first.route, ..*rules })
                }
                Sensed::Off | Sensed::Failed => Some(first),
            }
        }
        first => first,
    };
    let defaults = answer.as_ref().map(SenseDefaults::of);
    let sensed_task = defaults.as_ref().and_then(|d| d.task.as_deref());
    let picked_task = task;
    let task = task.or(sensed_task);
    let task = task.unwrap_or(if route.contains("plan") { "plan" } else { "implement" });
    let effects =
        defaults.as_ref().map_or(vec!["read".into(), "write".into()], |d| d.effects.clone());
    // Sense's tasks, unless the person picked one.
    let tasks = match (&answer, picked_task) {
        (Some(a), None) if !a.task.on.is_empty() => a.task.on.clone(),
        _ => vec![task.to_string()],
    };
    let result =
        weft_sense::ask::derive_result(class.unwrap_or(route), &tasks, &effects, plan.is_some());
    let classification =
        api::Classification { task: vec![task.into()], result: result.into(), effects };
    let rules = api::rules(&ws, harness, route, &classification, &overrides).map_err(refused)?;
    let rule = |r: &api::Rule| {
        serde_json::json!({"id": r.id, "label": r.label, "text": r.text, "family": r.family,
                           "layer": r.layer})
    };
    let offered = |id: &&String| rules.practices.iter().any(|r| &r.id == *id);
    let proposed: Option<Vec<&String>> = match &defaults {
        _ if stage == Stage::Heads => None,
        Some(d) => Some(d.rules.iter().filter(offered).collect()),
        None => failed.then(Vec::new),
    };
    // Sense's ticked practices first, then the rest by its score, best first.
    let mut practices: Vec<&api::Rule> = rules.practices.iter().collect();
    if let (Some(a), Some(ticked)) = (&answer, &proposed) {
        let score = |r: &api::Rule| a.unticked.iter().find(|s| s.id == r.id).and_then(|s| s.p);
        let rank = |r: &api::Rule| match (ticked.contains(&&r.id), score(r)) {
            (true, _) => (0, 0.0),
            (false, Some(p)) => (1, -p),
            (false, None) => (2, 0.0),
        };
        practices.sort_by(|a, b| {
            let (a, b) = (rank(a), rank(b));
            a.0.cmp(&b.0).then(a.1.total_cmp(&b.1))
        });
    }
    Ok(serde_json::json!({
        "routes": routes, "route": (!route.is_empty()).then_some(route), "tasks": api::tasks(), "task": task,
        "effects": classification.effects, "plan": plan, "plans": plans, "scope": scope,
        "practices": practices.into_iter().map(rule).collect::<Vec<_>>(),
        "proposed": proposed,
        "route_rules": rules.route.iter().map(rule).collect::<Vec<_>>(),
        "sense": answer,
        "stage": match stage { Stage::Heads => "heads", Stage::Full => "full" },
    }))
}

/// What Sense sets in the sheet from its answer: the route it picked; its
/// likeliest task; its effects; its proposed rules.
#[derive(Debug, Clone, PartialEq)]
pub struct SenseDefaults {
    pub route: Option<String>,
    pub task: Option<String>,
    pub effects: Vec<String>,
    pub rules: Vec<String>,
}

impl SenseDefaults {
    pub fn of(answer: &weft_sense::AskAnswer) -> Self {
        let route = answer.route.pick.as_ref().map(|pick| format!("native_{pick}"));
        let likeliest = answer.task.on.iter().max_by(|a, b| {
            let p = |c: &String| answer.task.p.get(c).copied().unwrap_or(0.0);
            p(a).total_cmp(&p(b))
        });
        Self {
            route,
            task: likeliest.cloned(),
            effects: answer.effects.on.clone(),
            rules: answer.rules.iter().map(|r| r.id.clone()).collect(),
        }
    }
}

/// Every practice rule Sense may propose for an Ask on `harness`: those of
/// every kind of work, with their `when` lines and the example intents
/// people kept them for.
fn candidate_rules(
    ws: &ringframe::workspace::Workspace,
    harness: &str,
    route: Option<&str>,
    overrides: &Overrides,
) -> Result<Vec<weft_sense::Rule>, Error> {
    let practices = every_practice(ws, harness, route, overrides)?;
    Ok(practices
        .into_iter()
        .filter_map(|r| {
            Some(weft_sense::Rule { id: r.id, label: r.label, when: r.when?, counts: None })
        })
        .collect())
}

/// Every practice rule the layers hold, for any task: the ones an Ask is
/// offered, the person's own among them.
fn every_practice(
    ws: &ringframe::workspace::Workspace,
    harness: &str,
    route: Option<&str>,
    overrides: &Overrides,
) -> Result<Vec<api::Rule>, Error> {
    let all = api::Classification {
        task: api::tasks().iter().map(|t| t.to_string()).collect(),
        result: "workspace_change".into(),
        effects: vec!["read".into(), "write".into()],
    };
    let rules = api::rules(ws, harness, route.unwrap_or_default(), &all, overrides);
    Ok(rules.map_err(refused)?.practices)
}

/// Whether this workspace could finish an Ask at all, asked of RingFrame.
///
/// `ask compile` has always refused a workspace with no Git repository or no
/// commit, but it runs at the end. Weft asks first, so Ask is dim with a
/// reason rather than typing an invocation that costs a turn and then fails.
pub fn ask_preflight(project: &Path) -> Result<(), Error> {
    api::preflight(&workspace(project)?).map_err(refused)
}

/// The checks RingFrame still runs for an Eval; none when the project cannot
/// be opened.
pub fn checks_running(project: &Path, eval_id: &str) -> Vec<String> {
    workspace(project).map(|ws| api::eval_checks_running(&ws, eval_id)).unwrap_or_default()
}

/// The workspace at `project`, as RingFrame opens one.
fn workspace(project: &Path) -> Result<api::Workspace, Error> {
    api::workspace(project).map_err(refused)
}

/// A refusal from the crate, said in RingFrame's own words.
fn refused(e: api::Error) -> Error {
    let message = match &e {
        api::Error::Refused { detail, .. } => detail.clone(),
        other => other.to_string(),
    };
    Error::Refused { code: REFUSED_BY_A_RULE, message }
}

/// Open and gather the Eval of the open Asks of the unit of work `ask` is in,
/// and say which it is: RingFrame
/// reads the change with Git and records it as gathered, so no harness has to
/// map it. An Eval already open over the same Asks and the same
/// work is the one to continue, and RingFrame names it.
pub fn eval_open(
    project: &Path,
    ask: &str,
    agents: Option<&Value>,
    over: Option<&str>,
) -> Result<String, Error> {
    let ws = workspace(project)?;
    let overrides = overrides(over)?;
    let open = api::EvalOpen {
        ask: Some(ask),
        host: "weft",
        agents: agents.cloned(),
        overrides: &overrides,
        statements: Vec::new(),
    };
    match api::eval_open(&ws, open) {
        Ok(opened) => Ok(opened.eval_id),
        Err(api::Error::Refused { code, detail }) if code == "eval.already_open" => detail
            .split_whitespace()
            .next()
            .filter(|w| w.starts_with("evl_"))
            .map(str::to_string)
            .ok_or(Error::Refused { code: REFUSED_BY_A_RULE, message: detail }),
        Err(e) => Err(refused(e)),
    }
}

/// One ready task of an open Eval for a judge in `harness`, of `role` when
/// given, that reads nothing the judge's session has `read`; or that none is
/// ready, or that the Eval has settled.
pub fn eval_task(
    project: &Path,
    eval_id: &str,
    role: Option<&str>,
    harness: &str,
    read: &[String],
) -> Result<api::EvalStep, Error> {
    api::eval_task_for(&workspace(project)?, eval_id, role, harness, read).map_err(refused)
}

/// Whether a task's answer is in: accepted, or recorded failed.
pub fn eval_task_in(project: &Path, task_id: &str) -> Result<bool, Error> {
    api::eval_task_in(&workspace(project)?, task_id).map_err(refused)
}

/// A judge's session ended without its task's answer: the task goes out again.
pub fn eval_return(project: &Path, task_id: &str) -> Result<(), Error> {
    api::eval_return(&workspace(project)?, task_id).map_err(refused)
}

/// Weft started again without the judges it ran: every task of the Eval
/// still out goes out again.
pub fn eval_return_out(project: &Path, eval_id: &str) -> Result<Vec<String>, Error> {
    api::eval_return_out(&workspace(project)?, eval_id).map_err(refused)
}

/// Stop an Eval: RingFrame hands out nothing more and refuses what its judges
/// still submit.
pub fn eval_void(project: &Path, eval_id: &str, reason: &str) -> Result<(), Error> {
    api::eval_void(&workspace(project)?, eval_id, reason).map_err(refused)
}

pub fn ask_copy(project: &Path, ask_id: &str) -> Result<Vec<u8>, Error> {
    let prompt = api::prompt(&workspace(project)?, ask_id, None).map_err(refused)?;
    Ok(prompt.text.into_bytes())
}

/// An Ask's prompt as another host takes it: that host's command for the
/// same capability in front of the same wording, and how it has to be sent.
pub fn ask_for_host(project: &Path, ask_id: &str, host: &str) -> Result<Value, Error> {
    let prompt = api::prompt(&workspace(project)?, ask_id, Some(host)).map_err(refused)?;
    Ok(serde_json::json!({"ask_id": ask_id, "host": host, "prompt": prompt.text,
                          "delivery": prompt.delivery}))
}

/// Seal the open work as the person decided, on the Eval named, and say
/// the receipt. Nothing is typed into a harness.
pub fn seal(
    project: &Path,
    disposition: &str,
    eval_id: Option<&str>,
    reason: Option<&str>,
    answers: Option<&[weft_core::ask::SealAnswer]>,
) -> Result<Value, Error> {
    let answers = answers.map(|a| serde_json::json!({ "answers": a }));
    let req = api::SealCreate { disposition, eval_id, reason, note: None, answers };
    let sealed = api::seal_create(&workspace(project)?, req).map_err(|e| match e {
        api::Error::SealRefused(codes) => {
            Error::Refused { code: REFUSED_BY_A_RULE, message: codes.join(", ") }
        }
        other => refused(other),
    })?;
    Ok(sealed.receipt)
}

/// Everything RingFrame says about a receipt when it re-verifies it.
pub fn seal_checked(project: &Path, seal_id: &str) -> Result<Value, Error> {
    Ok(api::seal_check(&workspace(project)?, seal_id).map_err(refused)?.report)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A scratch folder, with Git and one commit when `git` says so.
    pub(crate) fn folder(name: &str, git: bool) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("weft-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .status()
                .unwrap();
            assert!(ok.success(), "git {args:?}");
        };
        if git {
            run(&["init", "-q"]);
            run(&["commit", "-q", "--allow-empty", "-m", "init"]);
        }
        dir
    }

    #[test]
    fn setting_a_project_up_says_what_ringframe_refused_with() {
        let bare = folder("set-up-bare", false);
        assert_eq!(set_up(&bare), Some(Needs::refused(Some("workspace.no_git"))));
        assert!(!bare.join(".fab7/rf").exists(), "nothing made where it refused");
        let repo = folder("set-up-repo", true);
        assert_eq!(set_up(&repo), None, "set up");
        assert!(repo.join(".fab7/rf").is_dir());
        assert_eq!(set_up(&repo), None, "a workspace that is there stays");
        for dir in [bare, repo] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// RingFrame's test configuration, installed in a home of this test's own:
    /// nextest runs each test in a process of its own.
    pub(crate) fn fixture_home() {
        let home = std::env::temp_dir().join(format!("weft-rf-home-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        // SAFETY: this test's process reads HOME on this thread only.
        unsafe { std::env::set_var("HOME", &home) };
        let config =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../ringframe/tests/fixtures/config");
        ringframe::workspace::install_config(Some(&config)).unwrap();
    }

    #[test]
    fn the_profile_and_the_rules_are_read_through_the_crate() {
        fixture_home();
        let repo = folder("profile", true);
        assert_eq!(profile(&repo, "codex").unwrap()["title"], "Codex");
        let observe = observe_tools(&repo, "claude-code").expect("claude-code's [observe]");
        assert!(observe.get("shell_tool").is_some(), "{observe}");
        let listed = rules(None).unwrap();
        assert!(listed["catalogs"].as_array().is_some_and(|c| !c.is_empty()), "{listed}");
        assert!(matches!(reads_overrides("not json"), Err(Error::Refused { code: 2, .. })));
        let _ = std::fs::remove_dir_all(repo);
    }

    /// The Ask sheet: the profile's routes, direct the default; the plan
    /// the words name; and the rules for the route and task.
    #[test]
    fn the_ask_sheet_offers_routes_a_plan_line_and_rules() {
        fixture_home();
        let repo = folder("sheet", true);
        std::fs::create_dir_all(repo.join("plans/uptime")).unwrap();
        std::fs::write(repo.join("plans/uptime/plan.md"), "# Uptime endpoint\n").unwrap();
        let said = sheet(
            &repo,
            None,
            ("claude-code", "add the uptime endpoint"),
            (None, None, None),
            &no_sense,
            &no_plan,
        );
        let said = said.unwrap();
        assert_eq!(said["route"], "native_direct", "direct is the default");
        assert_eq!(said["plan"], Value::Null, "words alone pick no plan: the plan head does");
        assert!(said["practices"].as_array().is_some_and(|p| !p.is_empty()), "{said}");
        let goal = sheet(
            &repo,
            None,
            ("claude-code", "fix it"),
            (Some("native_goal"), None, None),
            &no_sense,
            &no_plan,
        );
        let goal = goal.unwrap();
        assert_eq!((&goal["route"], &goal["task"]), (&"native_goal".into(), &"implement".into()));
        assert_eq!(goal["plan"], Value::Null, "no plan named");
        let labels: Vec<&str> = goal["route_rules"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["label"].as_str())
            .collect();
        assert_eq!(labels, ["Done Check"], "a goal with no plan has no steps to loop");
        let _ = std::fs::remove_dir_all(repo);
    }

    fn no_plan(_: &str, _: &[(String, String)]) -> Option<String> {
        None
    }

    fn no_sense(_: &Reading) -> Sensed {
        Sensed::Off
    }

    /// Sense's answer as a fixture: its route pick, top route, tasks on,
    /// effects on and proposed rules.
    fn sensed(
        pick: Option<&str>,
        tasks: &[&str],
        effects: &[&str],
        rules: &[&str],
    ) -> weft_sense::AskAnswer {
        let p = |on: &[&str]| {
            on.iter()
                .map(|c| (c.to_string(), serde_json::json!(0.9)))
                .collect::<serde_json::Map<_, _>>()
        };
        serde_json::from_value(serde_json::json!({
            "sense": "0.3.0",
            "route": {"pick": pick, "p": {"direct": 0.1, "plan": 0.7, "goal": 0.1, "review": 0.1}},
            "task": {"on": tasks, "p": p(tasks)},
            "effects": {"on": effects, "p": p(effects)},
            "result": "plan",
            "rules": rules.iter().map(|id| serde_json::json!({"id": id, "p": 0.8})).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    fn answered(pick: Option<&str>, tasks: &[&str], effects: &[&str], rules: &[&str]) -> Sensed {
        Sensed::Answered(Box::new(sensed(pick, tasks, effects, rules)))
    }

    /// Sense sets the route, the task and effects and the practices it
    /// proposes; the sheet still lists every practice to add.
    #[test]
    fn sense_fills_the_sheets_defaults() {
        fixture_home();
        let repo = folder("sense-defaults", true);
        let words = ("claude-code", "plan the export");
        let plain =
            sheet(&repo, None, words, (Some("native_plan"), None, None), &no_sense, &no_plan)
                .unwrap();
        let offered: Vec<String> = plain["practices"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["id"].as_str().map(str::to_string))
            .collect();
        assert!(offered.len() >= 2, "{plain}");
        let asked = std::cell::RefCell::new(Vec::new());
        let sense = |r: &Reading| {
            asked
                .borrow_mut()
                .push((r.routes.iter().map(|r| r.to_string()).collect::<Vec<_>>(), r.rules.len()));
            answered(
                Some("plan"),
                &["plan"],
                &["read", "write"],
                &[&offered[1], "practice.unoffered"],
            )
        };
        let said = sheet(
            &repo,
            None,
            ("claude-code", "plan the export"),
            (None, None, None),
            &sense,
            &no_plan,
        )
        .unwrap();
        assert_eq!((&said["route"], &said["task"]), (&"native_plan".into(), &"plan".into()));
        assert_eq!(said["effects"], serde_json::json!(["read", "write"]));
        assert_eq!(
            said["proposed"],
            serde_json::json!([offered[1]]),
            "only rules the sheet offers"
        );
        assert!(
            said["practices"].as_array().unwrap().len() >= offered.len(),
            "every practice to add"
        );
        assert_eq!(said["sense"]["sense"], "0.3.0");
        let (routes, candidates) = asked.borrow()[0].clone();
        assert_eq!(routes, ["plan", "direct", "goal"], "Sense picks among this harness's routes");
        assert_eq!(candidates, 0, "the heads first, alone");
        let (_, candidates) = asked.borrow()[1].clone();
        assert!(candidates >= offered.len(), "then every practice is a candidate");
        assert_eq!(asked.borrow().len(), 2, "the rules read once");
        // A route the person picked stays theirs.
        let goal = sheet(
            &repo,
            None,
            ("claude-code", "x"),
            (Some("native_goal"), None, None),
            &sense,
            &no_plan,
        )
        .unwrap();
        assert_eq!(goal["route"], "native_goal");
        let _ = std::fs::remove_dir_all(repo);
    }

    /// The heads stage reads Sense's heads alone, never a rule or a plan:
    /// its route, task and effects are set, and no rule is proposed yet.
    #[test]
    fn the_heads_stage_sets_route_task_and_effects_and_reads_no_rule_or_plan() {
        fixture_home();
        let repo = folder("sense-heads", true);
        std::fs::create_dir_all(repo.join("plans/export")).unwrap();
        std::fs::write(repo.join("plans/export/plan.md"), "# Export\n\nThe export.\n").unwrap();
        let asked = std::cell::RefCell::new(Vec::new());
        let sense = |r: &Reading| {
            asked.borrow_mut().push(r.rules.len());
            answered(Some("plan"), &["plan"], &["read", "write"], &[])
        };
        let words = ("claude-code", "plan the export");
        let said = heads_sheet(&repo, None, words, (None, None, None), &sense).unwrap();
        assert_eq!(said["stage"], "heads");
        assert_eq!((&said["route"], &said["task"]), (&"native_plan".into(), &"plan".into()));
        assert_eq!(said["effects"], serde_json::json!(["read", "write"]));
        assert_eq!((&said["proposed"], &said["plan"]), (&Value::Null, &Value::Null));
        assert_eq!(*asked.borrow(), [0], "one reading, of the heads alone");
        let picking = std::cell::Cell::new(0);
        let pick = |_: &str, c: &[(String, String)]| {
            picking.set(picking.get() + 1);
            c.first().map(|(slug, _)| slug.clone())
        };
        let full = sheet(&repo, None, words, (None, None, None), &sense, &pick).unwrap();
        assert_eq!((full["stage"].as_str(), picking.get()), (Some("full"), 1));
        assert_eq!(full["plan"]["slug"], "export", "the full read picks the plan");
        let _ = std::fs::remove_dir_all(repo);
    }

    /// The sheet lists the practices Sense ticks first, then every other one
    /// it scored, best first, then those it did not, in shipped order: the
    /// order desktop and web show `+ rule` in.
    #[test]
    fn the_sheet_lists_the_ticked_practices_first_then_the_rest_best_first() {
        fixture_home();
        let repo = folder("sense-ranks", true);
        let words = ("claude-code", "plan the export");
        let ids = |sheet: &Value| -> Vec<String> {
            let practices = sheet["practices"].as_array().unwrap().iter();
            practices.filter_map(|r| r["id"].as_str().map(str::to_string)).collect()
        };
        let plain =
            sheet(&repo, None, words, (Some("native_plan"), None, None), &no_sense, &no_plan)
                .unwrap();
        let offered = ids(&plain);
        let n = offered.len();
        assert!(n >= 4, "{plain}");
        let sense = |_: &Reading| {
            let mut a = sensed(Some("plan"), &["plan"], &["read", "write"], &[&offered[2]]);
            let score = |i: usize, p| weft_sense::ask::RuleScore { id: offered[i].clone(), p };
            a.unticked = vec![score(0, Some(0.1)), score(n - 1, Some(0.4))];
            Sensed::Answered(Box::new(a))
        };
        let said = sheet(&repo, None, words, (None, None, None), &sense, &no_plan).unwrap();
        let first = [&offered[2], &offered[n - 1], &offered[0]];
        let rest = offered.iter().filter(|id| !first.contains(id));
        let want: Vec<&String> = first.into_iter().chain(rest).collect();
        assert_eq!(ids(&said).iter().collect::<Vec<_>>(), want);
        assert_eq!(ids(&plain), offered, "without Sense, shipped order");
        let _ = std::fs::remove_dir_all(repo);
    }

    /// A person's own rule with no `when` is not Sense's to score: it is no
    /// candidate, so it gets no counts, and the sheet lists it after every
    /// rule Sense scored.
    #[test]
    fn a_persons_rule_with_no_when_is_listed_after_the_scored_and_never_read() {
        fixture_home();
        let repo = folder("own-no-when", true);
        let layers = serde_json::json!([{"layer": "weft", "ringframe": {"deltas": {
            "practices/software-development": {"entries": [
                {"id": "own.1", "text": "One commit per item.",
                 "applies_to": {"task": ["implement"]}}]}}}}])
        .to_string();
        let words = ("claude-code", "Fix the login bug.");
        let read = std::cell::RefCell::new(Vec::new());
        let sense = |r: &Reading| {
            *read.borrow_mut() = r.rules.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
            let mut a = sensed(Some("direct"), &["implement"], &["read", "write"], &[]);
            let scored = r.rules.iter().filter(|r| r.id != "practice.kiss");
            a.unticked = scored
                .map(|r| weft_sense::ask::RuleScore { id: r.id.clone(), p: Some(0.2) })
                .collect();
            Sensed::Answered(Box::new(a))
        };
        let picked = (Some("native_direct"), Some("implement"), None);
        let said = sheet(&repo, Some(&layers), words, picked, &sense, &no_plan).unwrap();
        assert!(!read.borrow().is_empty() && !read.borrow().contains(&"own.1".to_string()));
        let ids: Vec<&str> =
            said["practices"].as_array().unwrap().iter().filter_map(|r| r["id"].as_str()).collect();
        assert!(ids.contains(&"own.1"), "{ids:?}");
        let own = ids.iter().position(|id| *id == "own.1").unwrap();
        let scored = |id: &str| read.borrow().iter().any(|r| r == id) && id != "practice.kiss";
        assert!(ids[own + 1..].iter().all(|id| !scored(id)), "every scored rule first: {ids:?}");
        let _ = std::fs::remove_dir_all(repo);
    }

    /// A rule from the person's own layer reaches Sense with its `when`, is
    /// scored by the pinned bundle, and scores higher once kept on three Asks.
    #[test]
    fn a_persons_rule_is_scored_and_scores_higher_kept_on_three_asks() {
        fixture_home();
        let repo = folder("own-kept", true);
        assert_eq!(set_up(&repo), None);
        let tmp = tempfile::tempdir().unwrap();
        let weft_dir = tmp.path().join(".fab7/weft");
        let state =
            weft_core::state::WeftState { open_projects: vec![repo.clone()], ..Default::default() };
        crate::state::save(&weft_dir, &state).unwrap();
        let layers = serde_json::json!([{"layer": "weft", "ringframe": {"deltas": {
            "practices/software-development": {"entries": [
                {"id": "own.commits", "label": "Small Commits", "text": "One commit per item.",
                 "when": "The change touches several separate items of work.",
                 "applies_to": {"task": ["implement"]}}]}}}}])
        .to_string();
        let ws = workspace(&repo).unwrap();
        let rules =
            candidate_rules(&ws, "claude-code", None, &overrides(Some(&layers)).unwrap()).unwrap();
        assert!(rules.iter().any(|r| r.id == "own.commits"), "a candidate with its when");
        let bundle = tmp.path().join("bundle");
        crate::versions::tests::stage_sense_bundle(&bundle, crate::versions::tests::TESTED);
        let loaded =
            crate::memory::Loaded::new(Some(weft_sense::SenseModel::load(&bundle).unwrap()));
        let p = |rules: &[weft_sense::Rule]| {
            let guard = loaded.read().unwrap();
            let a = guard.as_ref().unwrap().ask("Fix the login bug.", &[], rules).unwrap();
            let own = a.rules.iter().chain(&a.unticked).find(|r| r.id == "own.commits");
            own.and_then(|r| r.p).expect("own.commits is scored")
        };
        let before = p(&crate::memory::with_counts(&loaded, &weft_dir, &rules));
        let mut d = crate::memory::tests::draft("", &["own.commits"]);
        for i in 0..3 {
            d.words = format!("Fix bug {i}.");
            compile(&repo, &d, Some(&layers)).unwrap();
            crate::memory::after_compile(&loaded, &weft_dir, &repo);
        }
        let counted = crate::memory::with_counts(&loaded, &weft_dir, &rules);
        let own = counted.iter().find(|r| r.id == "own.commits").unwrap();
        assert_eq!(own.counts, Some(weft_sense::Counts { candidate: 3, kept: 3 }));
        let after = p(&counted);
        assert!(after > before, "kept on three Asks: {before} then {after}");
        let _ = std::fs::remove_dir_all(repo);
    }

    /// A project of 25 plans, each with a title and a first paragraph.
    fn many_plans(name: &str) -> std::path::PathBuf {
        let repo = folder(name, true);
        for i in 0..24 {
            let dir = repo.join(format!("plans/p{i:02}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("plan.md"), format!("# Plan {i}\n\nWork item {i}.\n")).unwrap();
        }
        std::fs::create_dir_all(repo.join("plans/uptime")).unwrap();
        let md = "# Uptime endpoint\n\nServe /uptime with the process start time.\n";
        std::fs::write(repo.join("plans/uptime/plan.md"), md).unwrap();
        repo
    }

    /// A plan the words name is the sheet's plan with no model; else the plan
    /// head reads BM25's shortlist and the plan this harness's last Ask used,
    /// never all of them, and its pick, or none, is the sheet's plan.
    #[test]
    fn the_plan_is_the_one_named_or_the_plan_heads_pick_from_a_shortlist() {
        fixture_home();
        let repo = many_plans("plan-head");
        std::fs::create_dir_all(repo.join("plans/app/sync")).unwrap();
        let md = "# Offline sync\n\nNotes sync when devices reconnect.\n";
        std::fs::write(repo.join("plans/app/sync/plan.md"), md).unwrap();
        let read = std::cell::RefCell::new(Vec::new());
        let head = |_: &str, c: &[(String, String)]| {
            read.borrow_mut().push(c.to_vec());
            c.first().map(|(name, _)| name.clone())
        };
        let ask = |words: &str, head: PlanPicking| {
            let words = ("claude-code", words);
            sheet(&repo, None, words, (None, None, None), &no_sense, head).unwrap()
        };
        let said = ask("implement plans/uptime", &head);
        assert_eq!(said["plan"]["path"], "plans/uptime/plan.md");
        assert!(read.borrow().is_empty(), "a named plan needs no model");
        assert_eq!(ask("go on with the `sync` plan", &head)["plan"]["project"], "app");
        assert!(read.borrow().is_empty());

        let said = ask("serve the process start time", &head);
        assert_eq!(said["plan"]["path"], "plans/uptime/plan.md", "the head's pick");
        let candidates = read.borrow()[0].clone();
        assert_eq!(candidates.len(), PLAN_SHORTLIST, "a shortlist of 26 plans");
        assert_eq!(
            candidates[0],
            (
                "uptime".into(),
                "uptime: Uptime endpoint: Serve /uptime with the process start time.".into()
            )
        );
        let said = ask("devices that reconnect", &head);
        assert_eq!(said["plan"]["path"], "plans/app/sync/plan.md");
        let read_last = read.borrow()[1][0].clone();
        assert_eq!(read_last.1, "app / sync: Offline sync: Notes sync when devices reconnect.");

        // The plan the last Ask used is read too, whatever the words.
        let mut d = crate::memory::tests::draft("Work on it.", &[]);
        d.plan = Some("plans/p20/plan.md".into());
        assert_eq!(set_up(&repo), None);
        compile(&repo, &d, None).unwrap();
        let unsure = |_: &str, c: &[(String, String)]| {
            read.borrow_mut().push(c.to_vec());
            None
        };
        let said = ask("add a counter", &unsure);
        assert_eq!(said["plan"], Value::Null, "an unsure head gives no plan");
        let last = read.borrow().last().unwrap().clone();
        assert_eq!(last.len(), PLAN_SHORTLIST + 1);
        assert_eq!(last[PLAN_SHORTLIST].0, "p20");

        let mine = (None, None, Some(Some("plans/p03/plan.md")));
        let said = sheet(&repo, None, ("claude-code", "add a counter"), mine, &no_sense, &head);
        assert_eq!(said.unwrap()["plan"]["slug"], "p03", "the person's plan stays theirs");
        let _ = std::fs::remove_dir_all(repo);
    }

    /// A pane working in a project's folder is offered that project's plans
    /// and the flat ones; naming another project's plan still picks it.
    #[test]
    fn a_pane_in_a_projects_folder_reads_that_projects_plans() {
        fixture_home();
        let repo = folder("plan-scope", true);
        for plan in ["app/sync", "app/login", "web/sync-ui", "uptime"] {
            std::fs::create_dir_all(repo.join("plans").join(plan)).unwrap();
            let md = format!("# {plan}\n\nSync the {plan} pieces.\n");
            std::fs::write(repo.join("plans").join(plan).join("plan.md"), md).unwrap();
        }
        std::fs::create_dir_all(repo.join("app/src")).unwrap();
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        assert_eq!(plan_scope(&repo, &repo.join("app/src")).as_deref(), Some("app"));
        assert_eq!(plan_scope(&repo, &repo.join("docs")), None, "no plans/docs: every plan");
        assert_eq!(plan_scope(&repo, &repo), None, "the root reads every plan");
        assert_eq!(plan_scope(&repo, Path::new("/elsewhere")), None);

        let read = std::cell::RefCell::new(Vec::new());
        let head = |_: &str, c: &[(String, String)]| {
            let mut names: Vec<String> = c.iter().map(|(n, _)| n.clone()).collect();
            names.sort();
            read.borrow_mut().push(names);
            None
        };
        let ask = |words: &str, scope: Option<&str>| {
            let asked = ("claude-code", words);
            sheet_scoped(&repo, None, asked, (None, None, None), &no_sense, &head, scope).unwrap()
        };
        let said = ask("sync the pieces", Some("app"));
        assert_eq!(said["scope"], "app");
        assert_eq!(read.borrow()[0], ["app/login", "app/sync", "uptime"]);
        ask("sync the pieces", None);
        assert_eq!(read.borrow()[1].len(), 4, "the root reads every plan");
        let said = ask("go on with `sync-ui`", Some("app"));
        assert_eq!(said["plan"]["path"], "plans/web/sync-ui/plan.md", "named, from any folder");
        assert_eq!(said["plans"].as_array().unwrap().len(), 4, "the person may pick any plan");
        let _ = std::fs::remove_dir_all(repo);
    }

    /// A route the person picks, and a plan the sheet holds, reach Sense as
    /// the chosen line; its answer within them sets the task and the
    /// proposed rules, while Sense's own route stays the one it read.
    #[test]
    fn a_route_picked_by_hand_is_read_again_by_sense_within_it() {
        fixture_home();
        let repo = many_plans("chosen-line");
        let plain = sheet(
            &repo,
            None,
            ("claude-code", "x"),
            (Some("native_plan"), None, None),
            &no_sense,
            &no_plan,
        );
        let plan_rule = plain.unwrap()["practices"][0]["id"].as_str().unwrap().to_string();
        let readings = std::cell::RefCell::new(Vec::new());
        let sense = |r: &Reading| {
            let routes: Vec<String> = r.routes.iter().map(|c| c.to_string()).collect();
            readings.borrow_mut().push((
                r.route.map(str::to_string),
                r.plan.map(str::to_string),
                routes,
            ));
            match r.route {
                None => answered(Some("direct"), &["implement"], &["read", "write"], &[]),
                Some(_) => answered(Some("plan"), &["plan"], &["read", "write"], &[&plan_rule]),
            }
        };
        let words = ("claude-code", "add rate limiting to login");
        let picked = (Some("native_plan"), None, Some(None));
        let said = sheet(&repo, None, words, picked, &sense, &no_plan).unwrap();
        let readings = readings.borrow().clone();
        assert_eq!(readings[0], (None, None, vec!["plan".into(), "direct".into(), "goal".into()]));
        assert_eq!(
            readings[1],
            (Some("plan".into()), None, vec!["plan".into()]),
            "only the chosen route"
        );
        assert_eq!((&said["route"], &said["task"]), (&"native_plan".into(), &"plan".into()));
        assert_eq!(said["proposed"], serde_json::json!([plan_rule]));
        assert_eq!(said["sense"]["route"]["pick"], "direct", "Sense's own reading of the words");
        // A plan the sheet holds is read with them, as its candidate text.
        let held = |_: &str, _: &[(String, String)]| Some("uptime".to_string());
        let _ = sheet(&repo, None, words, (None, None, None), &sense, &held).unwrap();
        let _ = std::fs::remove_dir_all(repo);
    }

    /// A goal that works a plan gets its step loop; a goal without one does
    /// not, and still asks nothing of a plan.
    #[test]
    fn a_goal_with_a_plan_works_it_step_by_step_and_one_without_does_not() {
        fixture_home();
        let repo = many_plans("goal-plan");
        let labels = |picked| -> Vec<String> {
            let said =
                sheet(&repo, None, ("claude-code", "keep going"), picked, &no_sense, &no_plan)
                    .unwrap();
            said["route_rules"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["label"].as_str().unwrap().into())
                .collect()
        };
        let with = labels((Some("native_goal"), None, Some(Some("plans/uptime/plan.md"))));
        assert!(with.contains(&"Step Loop".to_string()), "{with:?}");
        let without = labels((Some("native_goal"), None, Some(None)));
        assert!(!without.contains(&"Step Loop".to_string()) && !without.is_empty(), "{without:?}");
        let _ = std::fs::remove_dir_all(repo);
    }

    /// RingFrame records Sense as the chooser only of what the person kept.
    #[test]
    fn sense_is_recorded_as_chooser_only_of_what_the_person_kept() {
        let shown = SenseShown {
            answer: sensed(Some("plan"), &["plan"], &["read"], &["practice.kiss"]),
            route: Some("native_plan".into()),
            task: Some("plan".into()),
            rules: vec!["practice.kiss".into()],
        };
        let mut d = AskDraft {
            words: "plan it".into(),
            harness: "codex".into(),
            route: "native_plan".into(),
            task: "plan".into(),
            effects: vec!["read".into()],
            plan: None,
            rules: vec!["practice.kiss".into()],
            link: None,
            sense: Some(shown.clone()),
            session: None,
        };
        let sense = |c: &api::Chooser| matches!(c, api::Chooser::Sense { version, .. } if version == "0.3.0");
        assert!(sense(&shown.classified_by(&d)) && sense(&shown.selected_by(&d)));
        d.route = "native_goal".into();
        d.rules.push("practice.yagni".into());
        assert!(!sense(&shown.classified_by(&d)) && !sense(&shown.selected_by(&d)));
    }

    /// When Sense fails, as when it takes too long, the route is direct and
    /// the person picks the practices: none is chosen, and every one is
    /// offered.
    #[test]
    fn when_sense_fails_the_route_is_direct_and_the_person_picks_practices() {
        fixture_home();
        let repo = folder("sense-failed", true);
        let failed = |_: &Reading| Sensed::Failed;
        let said = sheet(
            &repo,
            None,
            ("claude-code", "fix the typo"),
            (None, None, None),
            &failed,
            &no_plan,
        )
        .unwrap();
        assert_eq!(said["route"], "native_direct");
        assert_eq!(said["proposed"], serde_json::json!([]));
        assert!(said["practices"].as_array().is_some_and(|p| !p.is_empty()), "{said}");
        assert_eq!(said["sense"], Value::Null);
        let _ = std::fs::remove_dir_all(repo);
    }

    /// A route Sense cannot pick is direct: plan, goal and review are
    /// the person's to ask for.
    #[test]
    fn a_route_sense_cannot_pick_is_direct() {
        fixture_home();
        let repo = folder("sense-unsure", true);
        let words = ("claude-code", "fix the typo");
        let unsure = |_: &Reading| answered(None, &["implement"], &["read"], &[]);
        let said = sheet(&repo, None, words, (None, None, None), &unsure, &no_plan).unwrap();
        assert_eq!((&said["route"], &said["task"]), (&"native_direct".into(), &"implement".into()));
        let writes =
            |_: &Reading| answered(Some("direct"), &["implement"], &["read", "write"], &[]);
        let said = sheet(&repo, None, words, (None, None, None), &writes, &no_plan).unwrap();
        assert_eq!(said["route"], "native_direct");
        let _ = std::fs::remove_dir_all(repo);
    }

    /// An Ask sent from Weft records every practice rule it was offered, for
    /// any task and unticked too: what Sense counts each rule's keeps against.
    #[test]
    fn an_ask_records_every_rule_it_was_offered() {
        fixture_home();
        let repo = folder("offered", true);
        assert_eq!(set_up(&repo), None);
        let words = ("claude-code", "Fix the login bug.");
        let read = std::cell::RefCell::new(Vec::new());
        let sense = |r: &Reading| {
            *read.borrow_mut() = r.rules.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
            Sensed::Off
        };
        let said = sheet(&repo, None, words, (None, None, None), &sense, &no_plan).unwrap();
        let shown: Vec<String> = said["practices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().into())
            .collect();
        assert!(shown.len() > 1, "{shown:?}");
        let draft = AskDraft {
            words: words.1.into(),
            harness: words.0.into(),
            route: "native_direct".into(),
            task: "implement".into(),
            effects: vec!["read".into(), "write".into()],
            plan: None,
            rules: vec![shown[0].clone()],
            link: None,
            sense: None,
            session: None,
        };
        compile(&repo, &draft, None).unwrap();
        let ledger = std::fs::read_to_string(repo.join(".fab7/rf/ledger.jsonl")).unwrap();
        let compiled: Value = ledger
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|e| e["type"] == "ask.compiled")
            .unwrap();
        let offered: Vec<String> = compiled["data"]["compiler"]["offered"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_str().unwrap().into())
            .collect();
        // The rules Sense scores, and the ones listed for the task.
        let missing = |of: &[String]| of.iter().filter(|id| !offered.contains(id)).count();
        assert_eq!((missing(&read.borrow()), missing(&shown)), (0, 0), "{offered:?}");
        assert!(offered.len() > shown.len(), "rules for other tasks too: {offered:?}");
        let _ = std::fs::remove_dir_all(repo);
    }

    /// A Seal from Weft is RingFrame's record: its receipt carries the
    /// disposition and the reason the person picked, and a reason that is
    /// not one of the disposition's is refused.
    #[test]
    fn a_seal_carries_its_disposition_and_reason() {
        fixture_home();
        let repo = folder("seal", true);
        assert_eq!(set_up(&repo), None);
        let draft = AskDraft {
            words: "Fix the login bug.".into(),
            harness: "claude-code".into(),
            route: "native_plan".into(),
            task: "implement".into(),
            effects: vec!["read".into(), "write".into()],
            plan: None,
            rules: Vec::new(),
            link: None,
            sense: None,
            session: None,
        };
        compile(&repo, &draft, None).unwrap();
        let wrong = seal(&repo, "rejected", None, Some("finish_later"), None);
        assert!(
            matches!(&wrong, Err(Error::Refused { message, .. }) if message.contains("finish_later")),
            "{wrong:?}"
        );
        let receipt = seal(&repo, "rejected", None, Some("not_what_i_asked"), None).unwrap();
        assert_eq!(
            (&receipt["disposition"], &receipt["reason"]),
            (&Value::from("rejected"), &Value::from("not_what_i_asked"))
        );
        let _ = std::fs::remove_dir_all(repo);
    }

    /// An Eval Weft opens completes with no coordinator skill: each task is
    /// taken through the crate one at a time, answered in a file and
    /// submitted with it, until the Eval settles.
    #[test]
    fn an_eval_weft_opens_completes_task_by_task_through_the_crate() {
        fixture_home();
        let repo = folder("eval-tasks", true);
        assert_eq!(set_up(&repo), None);
        let draft = AskDraft {
            words: "Add the uptime export to src/uptime.js with a test.".into(),
            harness: "claude-code".into(),
            route: "native_direct".into(),
            task: "implement".into(),
            effects: vec!["read".into(), "write".into()],
            plan: None,
            rules: Vec::new(),
            link: None,
            sense: None,
            session: None,
        };
        let ask = compile(&repo, &draft, None).unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(
            repo.join("src/uptime.js"),
            "export const uptime = () => process.uptime();\n",
        )
        .unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .status()
                .unwrap();
            assert!(ok.success(), "git {args:?}");
        };
        git(&["add", "src"]);
        git(&["commit", "-q", "-m", "work"]);
        let eval_id = eval_open(&repo, &ask, None, None).unwrap();
        let ws = workspace(&repo).unwrap();
        let mut kinds = std::collections::BTreeSet::new();
        for _ in 0..20 {
            match api::eval_task(&ws, &eval_id, None).unwrap() {
                api::EvalStep::Settled { report } => {
                    assert!(report.is_object(), "{report}");
                    assert!(kinds.contains("requirements") && kinds.contains("read"), "{kinds:?}");
                    assert!(
                        kinds.contains("question"),
                        "an unsettled requirement is asked: {kinds:?}"
                    );
                    // The person's answer reaches the Seal as a decision.
                    let said = weft_core::ask::SealAnswer {
                        question: "q1".into(),
                        answer: "words".into(),
                        words: Some("only the server's uptime".into()),
                        ms: Some(3000),
                    };
                    let receipt =
                        seal(&repo, "accepted", Some(&eval_id), None, Some(&[said])).unwrap();
                    let d = &receipt["decisions"][0];
                    assert_eq!(
                        (&d["answer"], &d["words"]),
                        (&Value::from("words"), &Value::from("only the server's uptime"))
                    );
                    assert_eq!(d["ms"], 3000);
                    let _ = std::fs::remove_dir_all(repo);
                    return;
                }
                api::EvalStep::Waiting { .. } => panic!("nothing is out, so a task is ready"),
                api::EvalStep::Task(t) => {
                    kinds.insert(t.kind.clone());
                    let file = t.brief.to_string_lossy().replace(".brief.md", ".json");
                    let file: Value =
                        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
                    std::fs::write(&t.answer, answer(&t.kind, &file).to_string()).unwrap();
                    let said = api::eval_submit(&ws, &t.task_id, &t.answer).unwrap();
                    assert_eq!(said.outcome, api::Outcome::Accepted, "{:?}", said.errors);
                }
            }
        }
        panic!("the Eval did not settle");
    }

    /// A reader's answer to a task file: the requirements quoting the Ask's
    /// words; every unit serving the first requirement; every requirement
    /// unknown, with what would settle it.
    /// A question reader's answer, from its brief: each item in a question
    /// of its own, of the first kind it allows; a `what_did_you_mean` quotes
    /// the start of the first Ask.
    pub(crate) fn questions_for(brief: &str) -> Value {
        let words: String = brief
            .split("## The requests\n\n=== ")
            .nth(1)
            .and_then(|r| r.lines().nth(1))
            .unwrap_or_default()
            .chars()
            .take(20)
            .collect();
        let questions: Vec<Value> = brief
            .lines()
            .filter_map(|l| l.strip_prefix("### `"))
            .filter_map(|l| {
                let (key, rest) = l.split_once("` (allows ")?;
                Some((key.to_string(), rest.trim_end_matches(')').split(", ").next()?.to_string()))
            })
            .enumerate()
            .map(|(n, (key, kind))| {
                let mut q = serde_json::json!({"ask": kind, "items": [key],
                    "behaviour": format!("Behaviour number {n} changed."),
                    "why": format!("Reason number {n} for asking.")});
                if kind == "what_did_you_mean" {
                    q["quote"] = serde_json::json!(words);
                    q["readings"] = serde_json::json!(["one way", "another way"]);
                }
                q
            })
            .collect();
        serde_json::json!({"questions": questions})
    }

    fn answer(kind: &str, file: &Value) -> Value {
        let list = |key: &str| file[key].as_array().cloned().unwrap_or_default();
        if kind == "question" {
            let out = file["output"].as_str().unwrap_or_default();
            let brief = out.replace("/out/", "/tasks/").replace(".json", ".brief.md");
            return questions_for(&std::fs::read_to_string(brief).unwrap());
        }
        if kind == "requirements" {
            return serde_json::json!({"requirements": list("asks").iter().map(|a| {
                serde_json::json!({"ask": a["id"], "kind": "local", "text": "the uptime export",
                                   "quote": a["text"].as_str().unwrap().trim()})
            }).collect::<Vec<_>>()});
        }
        let items: Vec<String> =
            list("items").iter().filter_map(|i| i.as_str().map(str::to_string)).collect();
        let of = |q: &str| -> Vec<String> {
            items.iter().filter_map(|i| i.strip_prefix(q).map(str::to_string)).collect()
        };
        let (units, reqs) = (of("w|"), of("r|"));
        serde_json::json!({
            "windows": units.iter().map(|w| serde_json::json!({
                "id": w, "answer": "serves", "serves": [reqs[0]]})).collect::<Vec<_>>(),
            "requirements": reqs.iter().map(|r| serde_json::json!({
                "id": r, "vote": "unknown", "reason": format!("{r} is not shown either way"),
                "missing": format!("a test showing {r}")})).collect::<Vec<_>>(),
        })
    }

    /// The reasons Weft offers are RingFrame's, each with its words.
    #[test]
    fn the_seal_reasons_offered_are_ringframes() {
        for d in weft_core::tree::DISPOSITIONS {
            assert_eq!(weft_core::tree::reasons(d), ringframe::seal::reasons(d), "{d}");
            for r in weft_core::tree::reasons(d) {
                let key = format!("seal.r.{r}");
                assert!(weft_core::words::WORDS.iter().any(|(k, _)| *k == key), "{key}");
            }
        }
    }

    /// Preflight, Eval open, the prompt and a Seal check are the crate's calls:
    /// they take no outside world, so they run no program, `ringframe` least
    /// of all.
    #[test]
    fn the_crates_calls_run_no_program() {
        let dir = std::env::temp_dir().join(format!("weft-rf-api-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let no_git = ask_preflight(&dir);
        assert!(matches!(&no_git, Err(Error::Refused { code: 2, .. })), "{no_git:?}");
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .status()
                .unwrap();
            assert!(ok.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        std::fs::create_dir_all(dir.join(".fab7/rf")).unwrap();
        assert_eq!(ask_preflight(&dir), Ok(()));
        let refused = |r: Result<_, Error>| matches!(r, Err(Error::Refused { code: 2, .. }));
        assert!(refused(ask_copy(&dir, "ask_none").map(|_| ())));
        assert!(refused(ask_for_host(&dir, "ask_none", "codex").map(|_| ())));
        assert!(refused(eval_open(&dir, "ask_none", None, None).map(|_| ())));
        let checked = seal_checked(&dir, "sel_none").unwrap();
        assert_eq!(checked["codes"], serde_json::json!(["seal.receipt_missing"]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
