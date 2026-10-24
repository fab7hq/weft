//! The bridge to RingFrame: its crate, through `ringframe::api`.
//!
//! Weft never writes `.fab7/rf/` itself; every record goes through RingFrame,
//! which validates, digests and refuses.

use std::path::Path;

use ringframe::api;
use ringframe::config::Overrides;

use crate::outside::Outside;

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
    let project = project.to_string_lossy();
    let argv = ["--workspace", project.as_ref(), "ask", "confirm", "--ask", ask_id];
    let said = out.run("ringframe", &argv, None).map_err(|e| Error::Refused {
        code: -1,
        message: format!("ringframe would not run: {e}"),
    })?;
    if said.status.success() {
        return Ok(());
    }
    let text = if said.stderr.is_empty() { &said.stdout } else { &said.stderr };
    Err(Error::Refused {
        code: said.status.code().unwrap_or(-1),
        message: String::from_utf8_lossy(text).trim().to_string(),
    })
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
pub fn invocation_prefix(project: &Path, host: &str) -> Result<String, Error> {
    profile(project, host)?
        .get("invocation_prefix")
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
        assert_eq!(invocation_prefix(&repo, "codex"), Ok("$rf:".into()));
        assert_eq!(invocation_prefix(&repo, "claude-code"), Ok("/rf:".into()));
        let observe = observe_tools(&repo, "claude-code").expect("claude-code's [observe]");
        assert!(observe.get("shell_tool").is_some(), "{observe}");
        let listed = rules(None).unwrap();
        assert!(listed["catalogs"].as_array().is_some_and(|c| !c.is_empty()), "{listed}");
        assert!(matches!(reads_overrides("not json"), Err(Error::Refused { code: 2, .. })));
        let _ = std::fs::remove_dir_all(repo);
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
