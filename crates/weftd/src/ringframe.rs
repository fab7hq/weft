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
    /// The practice rules the sheet offered, by id.
    pub offered: Vec<String>,
    /// The work it follows: `follows` or `remediates`, and that record's id.
    pub link: Option<(String, String)>,
    /// What Sense set in the sheet, when it answered.
    pub sense: Option<SenseShown>,
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
    fn request<'a>(&'a self, overrides: &'a Overrides) -> api::Compile<'a> {
        let result = if self.task == "plan" { "plan" } else { "workspace_change" };
        let by = |s: &SenseShown, f: fn(&SenseShown, &AskDraft) -> api::Chooser| f(s, self);
        let classified_by =
            self.sense.as_ref().map_or(api::Chooser::Person, |s| by(s, SenseShown::classified_by));
        let selected_by =
            self.sense.as_ref().map_or(api::Chooser::Person, |s| by(s, SenseShown::selected_by));
        api::Compile {
            source: &self.words,
            harness: api::Harness { name: &self.harness, version: None, session_ref: None },
            capability: &self.route,
            classification: api::Classification {
                task: vec![self.task.clone()],
                result: result.into(),
                effects: self.effects.clone(),
            },
            plan: self.plan.as_deref(),
            practices: self.rules.clone(),
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
    api::compose(&workspace(project)?, &d.request(&overrides)).map_err(refused)
}

/// Compile the Ask, confirmed by the person's Send in Weft, and say its id.
pub fn compile(project: &Path, d: &AskDraft, layers: Option<&str>) -> Result<String, Error> {
    let overrides = overrides(layers)?;
    let compiled = api::compile(&workspace(project)?, d.request(&overrides)).map_err(refused)?;
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

/// Sense on the words, given the harness's routes it may pick and the
/// practice rules it may propose.
pub type Sensing<'a> = &'a dyn Fn(&[&str], &[weft_sense::Rule]) -> Sensed;

/// What the Ask sheet offers for `harness` and these words: every route of
/// its profile; the kinds of work; the plan the words are most about, of the
/// plans held here; and, for the route and task given or their defaults, the
/// practice rules to choose from and the route's rules. With Sense, its
/// answer sets the defaults and proposes practices; a route it is unsure of,
/// or one needing an explicit request for its effects, is left unselected.
/// When Sense fails the person picks: no route and no practice is chosen.
/// Without Sense, the first route is the default and every practice is
/// offered.
pub fn sheet(
    project: &Path,
    layers: Option<&str>,
    (harness, words): (&str, &str),
    (route, task): (Option<&str>, Option<&str>),
    sense: Sensing,
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
    let sensed = sense(&classes, &candidates);
    let failed = sensed == Sensed::Failed;
    let answer = match sensed {
        Sensed::Answered(a) => Some(*a),
        Sensed::Off | Sensed::Failed => None,
    };
    let defaults = answer.as_ref().map(|a| SenseDefaults::of(a, &profile));
    let first = routes.first().copied().unwrap_or_default();
    let shown_route = match (route, &defaults) {
        (Some(r), _) => Some(r.to_string()),
        (None, Some(d)) => d.route.clone(),
        (None, None) => (!failed).then(|| first.to_string()),
    };
    let route = shown_route.as_deref().unwrap_or(first);
    let sensed_task = defaults.as_ref().and_then(|d| d.task.as_deref());
    let task = task.or(sensed_task);
    let task = task.unwrap_or(if route.contains("plan") { "plan" } else { "implement" });
    let effects =
        defaults.as_ref().map_or(vec!["read".into(), "write".into()], |d| d.effects.clone());
    // A plan folder with no plan.md has no path, and is no plan to work.
    let plans: Vec<weft_core::ask::Plan> = profile["plans"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let slug = p["slug"].as_str()?.to_string();
            let title = p["title"].as_str().unwrap_or(&slug).to_string();
            Some(weft_core::ask::Plan { path: p["path"].as_str()?.into(), title, slug })
        })
        .collect();
    let plan = weft_core::ask::plan_for(words, &plans).cloned();
    let result = if task == "plan" { "plan" } else { "workspace_change" };
    let classification =
        api::Classification { task: vec![task.into()], result: result.into(), effects };
    let rules = api::rules(&ws, harness, route, &classification, &overrides).map_err(refused)?;
    let rule = |r: &api::Rule| {
        serde_json::json!({"id": r.id, "label": r.label, "text": r.text, "family": r.family,
                           "layer": r.layer})
    };
    let offered = |id: &&String| rules.practices.iter().any(|r| &r.id == *id);
    let proposed: Option<Vec<&String>> = match &defaults {
        Some(d) => Some(d.rules.iter().filter(offered).collect()),
        None => failed.then(Vec::new),
    };
    Ok(serde_json::json!({
        "routes": routes, "route": shown_route, "tasks": api::tasks(), "task": task,
        "effects": classification.effects, "plan": plan, "plans": plans,
        "practices": rules.practices.iter().map(rule).collect::<Vec<_>>(),
        "proposed": proposed,
        "route_rules": rules.route.iter().map(rule).collect::<Vec<_>>(),
        "sense": answer,
    }))
}

/// What Sense sets in the sheet from its answer: the route it picked,
/// unless the profile says that route needs an explicit request for an
/// effect Sense found; its likeliest task; its effects; its proposed rules.
#[derive(Debug, Clone, PartialEq)]
pub struct SenseDefaults {
    pub route: Option<String>,
    pub task: Option<String>,
    pub effects: Vec<String>,
    pub rules: Vec<String>,
}

impl SenseDefaults {
    pub fn of(answer: &weft_sense::AskAnswer, profile: &Value) -> Self {
        let effects = answer.effects.on.clone();
        let needs_asking = |id: &str| {
            let capability = profile["capabilities"].as_array().into_iter().flatten();
            let capability = capability.into_iter().find(|c| c["id"] == id);
            let explicit = capability.map(|c| &c["requires_explicit_request_for_effects"]);
            let explicit = explicit.and_then(Value::as_array).into_iter().flatten();
            explicit.into_iter().any(|e| effects.iter().any(|on| e == on.as_str()))
        };
        let route = answer.route.pick.as_ref().map(|pick| format!("native_{pick}"));
        let route = route.filter(|id| !needs_asking(id));
        let likeliest = answer.task.on.iter().max_by(|a, b| {
            let p = |c: &String| answer.task.p.get(c).copied().unwrap_or(0.0);
            p(a).total_cmp(&p(b))
        });
        Self {
            route,
            task: likeliest.cloned(),
            effects: effects.clone(),
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
    let all = api::Classification {
        task: api::tasks().iter().map(|t| t.to_string()).collect(),
        result: "workspace_change".into(),
        effects: vec!["read".into(), "write".into()],
    };
    let rules = api::rules(ws, harness, route.unwrap_or_default(), &all, overrides);
    let rules = rules.map_err(refused)?;
    Ok(rules
        .practices
        .into_iter()
        .filter_map(|r| Some(weft_sense::Rule { id: r.id, when: r.when? }))
        .collect())
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

/// One ready task of an open Eval for a judge, of `role` when given; or that
/// none is ready, or that the Eval has settled.
pub fn eval_task(
    project: &Path,
    eval_id: &str,
    role: Option<&str>,
) -> Result<api::EvalStep, Error> {
    api::eval_task(&workspace(project)?, eval_id, role).map_err(refused)
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
) -> Result<Value, Error> {
    let req = api::SealCreate { disposition, eval_id, reason, note: None };
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
mod tests {
    use super::*;

    /// A scratch folder, with Git and one commit when `git` says so.
    fn folder(name: &str, git: bool) -> std::path::PathBuf {
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
    fn fixture_home() {
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
        assert_eq!(profile(&repo, "codex").unwrap()["invocation_prefix"], "$rf:");
        let observe = observe_tools(&repo, "claude-code").expect("claude-code's [observe]");
        assert!(observe.get("shell_tool").is_some(), "{observe}");
        let listed = rules(None).unwrap();
        assert!(listed["catalogs"].as_array().is_some_and(|c| !c.is_empty()), "{listed}");
        assert!(matches!(reads_overrides("not json"), Err(Error::Refused { code: 2, .. })));
        let _ = std::fs::remove_dir_all(repo);
    }

    /// The Ask sheet: the profile's routes, its first the default; the plan
    /// the words name; and the rules for the route and task.
    #[test]
    fn the_ask_sheet_offers_routes_a_plan_line_and_rules() {
        fixture_home();
        let repo = folder("sheet", true);
        std::fs::create_dir_all(repo.join("plans/uptime")).unwrap();
        std::fs::write(repo.join("plans/uptime/plan.md"), "# Uptime endpoint\n").unwrap();
        let said =
            sheet(&repo, None, ("claude-code", "add the uptime endpoint"), (None, None), &no_sense);
        let said = said.unwrap();
        assert_eq!(said["routes"][0], said["route"], "the first route is the default");
        assert_eq!(said["plan"]["path"], "plans/uptime/plan.md");
        assert!(said["practices"].as_array().is_some_and(|p| !p.is_empty()), "{said}");
        let goal =
            sheet(&repo, None, ("claude-code", "fix it"), (Some("native_goal"), None), &no_sense);
        let goal = goal.unwrap();
        assert_eq!((&goal["route"], &goal["task"]), (&"native_goal".into(), &"implement".into()));
        assert_eq!(goal["plan"], Value::Null, "no plan named");
        let labels: Vec<&str> = goal["route_rules"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["label"].as_str())
            .collect();
        assert_eq!(labels, ["Step Loop", "Done Check"]);
        let _ = std::fs::remove_dir_all(repo);
    }

    fn no_sense(_: &[&str], _: &[weft_sense::Rule]) -> Sensed {
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
        let plain = sheet(&repo, None, ("claude-code", "plan the export"), (None, None), &no_sense)
            .unwrap();
        let offered: Vec<String> = plain["practices"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["id"].as_str().map(str::to_string))
            .collect();
        assert!(offered.len() >= 2, "{plain}");
        let asked = std::cell::RefCell::new(Vec::new());
        let sense = |routes: &[&str], rules: &[weft_sense::Rule]| {
            asked
                .borrow_mut()
                .push((routes.iter().map(|r| r.to_string()).collect::<Vec<_>>(), rules.len()));
            answered(
                Some("plan"),
                &["plan"],
                &["read", "write"],
                &[&offered[1], "practice.unoffered"],
            )
        };
        let said =
            sheet(&repo, None, ("claude-code", "plan the export"), (None, None), &sense).unwrap();
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
        assert!(candidates >= offered.len(), "every practice is a candidate");
        // A route the person picked stays theirs.
        let goal =
            sheet(&repo, None, ("claude-code", "x"), (Some("native_goal"), None), &sense).unwrap();
        assert_eq!(goal["route"], "native_goal");
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
            offered: vec!["practice.kiss".into()],
            link: None,
            sense: Some(shown.clone()),
        };
        let sense = |c: &api::Chooser| matches!(c, api::Chooser::Sense { version, .. } if version == "0.3.0");
        assert!(sense(&shown.classified_by(&d)) && sense(&shown.selected_by(&d)));
        d.route = "native_goal".into();
        d.rules.push("practice.yagni".into());
        assert!(!sense(&shown.classified_by(&d)) && !sense(&shown.selected_by(&d)));
    }

    /// When Sense fails, as when it takes too long, the person picks the
    /// route and the practices: none is chosen, and every one is offered.
    #[test]
    fn when_sense_fails_the_person_picks() {
        fixture_home();
        let repo = folder("sense-failed", true);
        let failed = |_: &[&str], _: &[weft_sense::Rule]| Sensed::Failed;
        let said =
            sheet(&repo, None, ("claude-code", "fix the typo"), (None, None), &failed).unwrap();
        assert_eq!(said["route"], Value::Null);
        assert_eq!(said["proposed"], serde_json::json!([]));
        assert!(said["practices"].as_array().is_some_and(|p| !p.is_empty()), "{said}");
        assert_eq!(said["sense"], Value::Null);
        let _ = std::fs::remove_dir_all(repo);
    }

    /// An unsure route, or one the profile says needs an explicit request
    /// for Sense's effects, is shown unselected: the person picks.
    #[test]
    fn an_unsure_or_explicit_only_route_is_left_for_the_person() {
        fixture_home();
        let repo = folder("sense-unsure", true);
        let words = ("claude-code", "fix the typo");
        let unsure =
            |_: &[&str], _: &[weft_sense::Rule]| answered(None, &["implement"], &["read"], &[]);
        assert_eq!(sheet(&repo, None, words, (None, None), &unsure).unwrap()["route"], Value::Null);
        let writes = |_: &[&str], _: &[weft_sense::Rule]| {
            answered(Some("direct"), &["implement"], &["read", "write"], &[])
        };
        let said = sheet(&repo, None, words, (None, None), &writes).unwrap();
        assert_eq!(said["route"], Value::Null, "native_direct needs an explicit request to write");
        let reads = |_: &[&str], _: &[weft_sense::Rule]| {
            answered(Some("direct"), &["question"], &["read"], &[])
        };
        assert_eq!(
            sheet(&repo, None, words, (None, None), &reads).unwrap()["route"],
            "native_direct"
        );
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
            offered: Vec::new(),
            link: None,
            sense: None,
        };
        compile(&repo, &draft, None).unwrap();
        let wrong = seal(&repo, "rejected", None, Some("finish_later"));
        assert!(
            matches!(&wrong, Err(Error::Refused { message, .. }) if message.contains("finish_later")),
            "{wrong:?}"
        );
        let receipt = seal(&repo, "rejected", None, Some("not_what_i_asked")).unwrap();
        assert_eq!(
            (&receipt["disposition"], &receipt["reason"]),
            (&Value::from("rejected"), &Value::from("not_what_i_asked"))
        );
        // Its label row is read from the receipt.
        let row = crate::labels::sealed("claude-code", &receipt);
        assert_eq!(
            (&row["seal_id"], &row["disposition"], &row["reason"]),
            (&receipt["seal_id"], &Value::from("rejected"), &Value::from("not_what_i_asked"))
        );
        assert!(row["seal_id"].as_str().is_some_and(|s| s.starts_with("sel_")), "{row}");
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
            offered: Vec::new(),
            link: None,
            sense: None,
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
                api::EvalStep::Settled { verdict, .. } => {
                    assert!(!verdict.is_empty());
                    assert!(kinds.contains("requirements") && kinds.contains("map"), "{kinds:?}");
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

    /// A judge's answer to a task file: the requirements quoting the Ask's
    /// words; every window serving the first requirement; every requirement
    /// unknown, with what would settle it.
    fn answer(kind: &str, file: &Value) -> Value {
        let list = |key: &str| file[key].as_array().cloned().unwrap_or_default();
        match kind {
            "requirements" => serde_json::json!({"requirements": list("asks").iter().map(|a| {
                serde_json::json!({"ask": a["id"], "kind": "local", "text": "the uptime export",
                                   "quote": a["text"].as_str().unwrap().trim()})
            }).collect::<Vec<_>>()}),
            "map" => {
                let first = list("requirements")[0]["id"].clone();
                serde_json::json!({"windows": list("windows").iter().map(|w| {
                    serde_json::json!({"window": w["id"], "serves": [first]})
                }).collect::<Vec<_>>()})
            }
            _ => serde_json::json!({"requirements": list("requirements").iter().map(|r| {
                serde_json::json!({"id": r["id"], "vote": "unknown",
                                   "reason": format!("{} is not shown either way", r["id"]),
                                   "missing": format!("a test showing {}", r["id"])})
            }).collect::<Vec<_>>()}),
        }
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
