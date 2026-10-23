//! The bridge to RingFrame: its crate, through `ringframe::api`, and its CLI
//! for what the crate does not offer yet.
//!
//! Weft never writes `.fab7/rf/` itself; every record goes through RingFrame,
//! which validates, digests and refuses.

use std::path::Path;

use ringframe::api;

use crate::outside::Outside;

use serde_json::Value;
use weft_core::workspace::Needs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The CLI is not installed. Weft still runs panes; it just has no record.
    NotInstalled,
    Refused {
        code: i32,
        message: String,
    },
}

fn run(out: &dyn Outside, project: &Path, args: &[&str]) -> Result<Vec<u8>, Error> {
    run_program(out, "ringframe", project, args)
}

/// "No CLI" and "the CLI said no" are different situations, so a spawn failure
/// becomes `NotInstalled` and never a refusal.
fn run_program(
    outside: &dyn Outside,
    program: &str,
    project: &Path,
    args: &[&str],
) -> Result<Vec<u8>, Error> {
    let project = project.to_string_lossy();
    let argv: Vec<&str> =
        ["--workspace", project.as_ref()].into_iter().chain(args.iter().copied()).collect();
    let out = outside.run(program, &argv, None).map_err(|_| Error::NotInstalled)?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    // A refusal is said on stderr, or as JSON on stdout.
    let said = if out.stderr.is_empty() { &out.stdout } else { &out.stderr };
    Err(Error::Refused {
        code: out.status.code().unwrap_or(-1),
        message: String::from_utf8_lossy(said).trim().to_string(),
    })
}

/// Every practice and every rule, with the layer that last set each, and what
/// Eval judges must not do, through these `--override` layers:
/// `{"domains": […], "catalogs": […], "judges": {"deny": […], "from": …}}`.
pub fn rules(out: &dyn Outside, project: &Path, layers: Option<&str>) -> Result<Value, Error> {
    let with = |args: &[&str]| {
        let mut args = args.to_vec();
        if let Some(layers) = layers {
            args.extend(["--override", layers]);
        }
        json(out, project, &args)
    };
    let domains = with(&["deltas", "domains", "--json"])?;
    let catalogs = with(&["deltas", "list", "--json"])?;
    Ok(serde_json::json!({"domains": domains["domains"], "catalogs": catalogs["catalogs"],
        "judges": catalogs["judges"]}))
}

/// What RingFrame says of `host`: its routes, how each is delivered, its limits.
pub fn profile(
    out: &dyn Outside,
    project: &Path,
    layers: Option<&str>,
    host: &str,
) -> Result<Value, Error> {
    let mut args = vec!["profile", "show", "--host", host, "--json"];
    if let Some(layers) = layers {
        args.extend(["--override", layers]);
    }
    json(out, project, &args)
}

/// The Rules block RingFrame would write for `host`'s route `capability` and
/// a task of `task`, through these layers: `{"text", "kept", "left_out"}`,
/// the core rules it keeps and those its cap leaves out.
pub fn preview(
    out: &dyn Outside,
    project: &Path,
    layers: Option<&str>,
    (host, capability, task): (&str, &str, &str),
) -> Result<Value, Error> {
    let result = if task == "plan" { "plan" } else { "workspace_change" };
    let classification = serde_json::json!({"task": [task], "result": result,
        "interaction": "approval_gated", "horizon": "session", "effects": ["read", "write"]})
    .to_string();
    let mut args =
        vec!["deltas", "render", "--host", host, "--capability", capability, "--classification"];
    args.push(&classification);
    args.push("--json");
    if let Some(layers) = layers {
        args.extend(["--override", layers]);
    }
    let said = json(out, project, &args)?;
    let practice = &said["practice"];
    let kept = practice["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["tier"] != "situational")
        .count();
    let left_out = practice["dropped_by_budget"].as_array().map_or(0, Vec::len);
    Ok(serde_json::json!({"text": said["text"], "kept": kept, "left_out": left_out}))
}

/// Whether RingFrame reads these `--override` layers: every catalog listed
/// through them, which refuses what any command reading them would.
pub fn reads_overrides(out: &dyn Outside, project: &Path, layers: &str) -> Result<(), Error> {
    run(out, project, &["deltas", "list", "--override", layers, "--json"]).map(|_| ())
}

fn json(out: &dyn Outside, project: &Path, args: &[&str]) -> Result<Value, Error> {
    let bytes = run(out, project, args)?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Refused {
        code: 4,
        message: format!("could not read the CLI's answer: {e}"),
    })
}

/// RingFrame's own exit code for "refused by a rule", as its help documents.
const REFUSED_BY_A_RULE: i32 = 2;

/// Set the project up for RingFrame, as a person would with `ringframe init`,
/// and say what it still needs if RingFrame refused. A hook never makes the
/// workspace; Weft does, through the CLI, when the project opens. One that is
/// there already is not asked about again.
pub fn set_up(outside: &dyn Outside, project: &Path) -> Option<Needs> {
    if project.join(".fab7/rf").is_dir() {
        return None;
    }
    let root = project.to_string_lossy();
    let Ok(out) = outside.run("ringframe", &["--workspace", root.as_ref(), "init"], None) else {
        return Some(Needs::refused(None));
    };
    if out.status.success() {
        return None;
    }
    // The refusal is RingFrame's JSON on stdout, whatever it also says on stderr.
    let said: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    Some(Needs::refused(Some(said["error"].as_str().unwrap_or_default())))
}

/// One `ringframe observe` call, as weftd forwards a signal,
/// and what RingFrame answered, which for a prompt says whose submission it
/// is. Weft writes no record itself: RingFrame decides what the
/// fact means.
pub fn observe(
    out: &dyn Outside,
    project: &Path,
    call: &weft_core::signal::Forward,
) -> Result<Value, Error> {
    let project = project.to_string_lossy();
    let argv: Vec<&str> = ["--workspace", project.as_ref()]
        .into_iter()
        .chain(call.args.iter().map(String::as_str))
        .collect();
    let said = match &call.input {
        Some(input) => out.run_input("ringframe", &argv, input.as_bytes()),
        None => out.run("ringframe", &argv, None),
    };
    let said = said.map_err(|_| Error::NotInstalled)?;
    if said.status.success() {
        return Ok(serde_json::from_slice(&said.stdout).unwrap_or(Value::Null));
    }
    // Only the code: nothing a prompt said is logged.
    Err(Error::Refused { code: said.status.code().unwrap_or(-1), message: String::new() })
}

/// The tools RingFrame makes something of for this harness, its profile's
/// `[observe]`, or none when RingFrame cannot say.
pub fn observe_tools(out: &dyn Outside, project: &Path, host: &str) -> Option<Value> {
    let v = json(out, project, &["profile", "show", "--host", host, "--json"]).ok()?;
    Some(v.get("observe").cloned().unwrap_or(Value::Null))
}

pub fn installed(out: &dyn Outside) -> bool {
    out.run("ringframe", &["--version"], None).is_ok()
}

/// Whether this workspace could finish an Ask at all, asked of RingFrame.
///
/// `ask compile` has always refused a workspace with no Git repository or no
/// commit, but it runs at the end. Weft asks first, so Ask is dim with a
/// reason rather than typing an invocation that costs a turn and then fails.
pub fn ask_preflight(project: &Path) -> Result<(), Error> {
    api::preflight(&workspace(project)?).map_err(refused)
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

/// The exact bytes to type. Never altered, never regenerated by Weft.
/// Record the person's yes for an Ask whose confirmation surface never got
/// one. RingFrame writes the record; Weft only carries the decision, which
/// the person made by pressing the key.
pub fn ask_confirm(out: &dyn Outside, project: &Path, ask_id: &str) -> Result<(), Error> {
    run(out, project, &["ask", "confirm", "--ask", ask_id]).map(|_| ())
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
    let overrides = match over {
        Some(o) => ringframe::config::Overrides::parse(o)
            .map_err(|e| Error::Refused { code: REFUSED_BY_A_RULE, message: e.0 })?,
        None => ringframe::config::Overrides::default(),
    };
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

/// How this harness's skills are invoked. Read from the profile rather than
/// assumed, because RingFrame owns the answer and it differs by host.
pub fn invocation_prefix(out: &dyn Outside, project: &Path, host: &str) -> Result<String, Error> {
    let v = json(out, project, &["profile", "show", "--host", host, "--json"])?;
    v.get("invocation_prefix")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(Error::Refused { code: 4, message: "the profile names no invocation prefix".into() })
}

/// Everything RingFrame says about a receipt when it re-verifies it.
pub fn seal_checked(project: &Path, seal_id: &str) -> Result<Value, Error> {
    Ok(api::seal_check(&workspace(project)?, seal_id).map_err(refused)?.report)
}

/// What Weft types to start each step. The intent is appended for `ask`.
/// The invocation, with the space that closes it.
///
/// A harness offers completions while a skill token is still being typed, and
/// the first Enter after a bare token is taken by that chooser rather than
/// submitting: `$rf:eval` sat in a harness's composer, unsent, and Weft had no way
/// to tell. The trailing space ends the token, so Enter means send. Weft never
/// presses Enter twice to find out.
pub fn skill_command(prefix: &str, skill: &str) -> String {
    format!("{prefix}{skill} ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skill_command_is_the_profiles_prefix_and_the_verb() {
        assert_eq!(skill_command("/rf:", "eval"), "/rf:eval ");
        assert_eq!(skill_command("$rf:", "ask"), "$rf:ask ");
    }

    #[test]
    fn a_bare_invocation_ends_with_the_space_that_closes_it() {
        // Found against Codex: the first Enter after a bare `$rf:eval` was
        // taken by the completion chooser, and the prompt sat there unsent.
        for skill in ["eval", "seal"] {
            assert!(skill_command("$rf:", skill).ends_with(' '), "{skill}");
        }
    }

    use crate::outside::Fixed;

    #[test]
    fn a_missing_cli_is_reported_as_missing_not_as_a_refusal() {
        let got = run_program(&Fixed::new(), "ringframe", Path::new("/tmp"), &["ask", "list"]);
        assert_eq!(got, Err(Error::NotInstalled));
        assert!(!installed(&Fixed::new()));
    }

    #[test]
    fn setting_a_project_up_says_what_ringframe_refused_with() {
        use weft_core::workspace::Needs;
        let dir = std::env::temp_dir().join(format!("weft-set-up-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let refusing = |code: &str| {
            let said = format!(r#"{{"error": "{code}", "detail": "RingFrame's words"}}"#);
            Fixed::new().answers("ringframe", &["init"], 2, &said)
        };
        assert_eq!(set_up(&Fixed::new(), &dir), Some(Needs::refused(None)), "no ringframe");
        for code in ["workspace.no_git", "workspace.no_commit"] {
            assert_eq!(set_up(&refusing(code), &dir), Some(Needs::refused(Some(code))));
        }
        let done = Fixed::new().answers("ringframe", &["init"], 0, "{}");
        assert_eq!(set_up(&done, &dir), None, "set up");
        // A workspace that is there is not set up again: nothing is asked.
        std::fs::create_dir_all(dir.join(".fab7/rf")).unwrap();
        assert_eq!(set_up(&Fixed::new(), &dir), None);
        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn the_prefix_is_the_profiles() {
        let world = Fixed::new().answers(
            "ringframe",
            &["profile", "show", "--host", "codex"],
            0,
            r#"{"invocation_prefix": "$rf:"}"#,
        );
        assert_eq!(invocation_prefix(&world, Path::new("/p"), "codex"), Ok("$rf:".into()));
    }

    #[test]
    fn an_unknown_host_degrades_rather_than_inventing_a_prefix() {
        let world = Fixed::new().exists("ringframe");
        let got = invocation_prefix(&world, Path::new("/p"), "not-a-harness");
        assert!(matches!(got, Err(Error::Refused { .. })), "{got:?}");
    }
}
