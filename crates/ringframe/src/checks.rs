//! The checks an Eval's Asks name, run once by RingFrame (ADR-0020 D8).
//!
//! A check is a command an Ask, or a step of a plan part it names, states
//! verbatim in backticks: "`cargo test -p app-core` passes". `eval open`
//! starts them in the background and returns: a check can take minutes, and
//! the Eval is advice that must not wait on one. Each result is a fact the
//! judges may cite once it is in, and a compiler or linter warning on a line
//! the change added is a finding on that window (`new_warning`). A check that
//! fails, times out or cannot start is recorded as such; one still running
//! when the Eval closes is said to be, and nothing waits for it.

use std::collections::BTreeSet;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::evaluate::{EvalError, array_of};
use crate::store;
use crate::workspace::Workspace;

/// At most this many checks an Eval: a list longer than that is a plan's
/// test matrix, not the Eval's to run.
const MAX_CHECKS: usize = 4;
/// A check that runs longer than this is stopped and recorded as timed out.
const CHECK_SECONDS: u64 = 600;
/// How much of a check's output its fact keeps.
const TAIL_BYTES: usize = 4096;
/// At most this many warnings become findings, per check.
const MAX_WARNINGS: usize = 40;

/// The programs a check starts with, and the words that make one a check
/// rather than any command. Tools, not harnesses.
const RUNNERS: &[&str] = &[
    "cargo",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "deno",
    "npx",
    "node",
    "pytest",
    "python",
    "python3",
    "go",
    "make",
    "just",
    "mvn",
    "gradle",
    "./gradlew",
    "dotnet",
    "swift",
    "tsc",
    "eslint",
    "ruff",
    "mypy",
];
const CHECKERS: &[&str] = &["pytest", "tsc", "eslint", "ruff", "mypy"];
const VERBS: &[&str] = &["test", "check", "clippy", "lint", "build", "vet", "typecheck"];

/// Where an Eval's check results are kept, once all are in.
pub fn results_path(ws: &Workspace, eval_id: &str) -> std::path::PathBuf {
    ws.rf_dir().join(format!("tmp/eval-{eval_id}/checks.json"))
}

/// Whether a code span reads as a command that checks the work.
fn is_check(span: &str) -> bool {
    // Run as a plain argument list, never through a shell: a span with shell
    // syntax or quoting is not taken as a check.
    if span.contains(['<', '>', '|', ';', '`', '\n', '&', '$', '(', ')', '\\', '"', '\'', '*'])
        || span.contains("...")
        || span.contains('…')
    {
        return false;
    }
    let words: Vec<&str> = span.split_whitespace().collect();
    let Some(first) = words.first() else { return false };
    RUNNERS.contains(first)
        && (CHECKERS.contains(first)
            || words[1..].iter().any(|w| VERBS.contains(&w.trim_start_matches('-'))))
}

/// The distinct checks the texts state, in the order they first appear.
pub fn find<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for text in texts {
        for (i, span) in text.split('`').enumerate() {
            let span = span.trim();
            if i % 2 == 1 && is_check(span) && seen.insert(span.to_string()) {
                out.push(span.to_string());
            }
        }
    }
    out.truncate(MAX_CHECKS);
    out
}

/// Start the checks for an Eval and return at once. The CLI runs them as a
/// detached `ringframe eval checks`; in tests they run here.
pub fn start(ws: &Workspace, eval_id: &str, commands: &[String]) -> Result<(), EvalError> {
    if commands.is_empty() {
        return Ok(());
    }
    let dir = ws.rf_dir().join(format!("tmp/eval-{eval_id}"));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("checks.plan.json"), json!({"commands": commands}).to_string())?;
    if cfg!(test) {
        return run(ws, eval_id);
    }
    let exe = std::env::current_exe().map_err(EvalError::from)?;
    let mut cmd = Command::new(exe);
    cmd.arg("--workspace")
        .arg(&ws.root)
        .args(["eval", "checks", "--eval", eval_id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group: the shell that ran `eval open` may be
        // stopped with its children once it returns.
        cmd.process_group(0);
    }
    if cmd.spawn().is_err() {
        // Fail-safe: the Eval goes on without them, and says so.
        let facts: Vec<Value> = commands
            .iter()
            .enumerate()
            .map(|(i, c)| fact(i, c, "not_run", None, "the check could not be started", 0))
            .collect();
        publish(ws, eval_id, &facts, &[])?;
    }
    Ok(())
}

fn fact(i: usize, command: &str, outcome: &str, code: Option<i32>, tail: &str, secs: u64) -> Value {
    json!({"id": format!("chk_{}", i + 1), "command": command, "outcome": outcome,
           "exit": code, "seconds": secs, "tail": tail, "by": "ringframe"})
}

/// Run every planned check once, in the workspace, one after another, and
/// write their facts and findings.
pub fn run(ws: &Workspace, eval_id: &str) -> Result<(), EvalError> {
    let dir = ws.rf_dir().join(format!("tmp/eval-{eval_id}"));
    let plan: Value = serde_json::from_slice(&std::fs::read(dir.join("checks.plan.json"))?)
        .unwrap_or(Value::Null);
    let windows = added_lines(ws, eval_id);
    let mut facts = Vec::new();
    let mut findings = Vec::new();
    for (i, c) in array_of(&plan, "commands").iter().enumerate() {
        let command = c.as_str().unwrap_or_default();
        let (outcome, code, output, secs) = execute(ws, command);
        let tail = tail_of(&output);
        facts.push(fact(i, command, outcome, code, &tail, secs));
        for (path, line, text) in warnings(&output).into_iter().take(MAX_WARNINGS) {
            if let Some(w) = windows.iter().find(|w| w.0 == path && w.1.contains(&line)) {
                findings.push(json!({
                    "kind": "new_warning", "requirements": [], "units": [], "windows": [w.2],
                    "detail": format!("`{command}`: {text} ({path}:{line})"),
                }));
            }
        }
    }
    publish(ws, eval_id, &facts, &findings)
}

fn publish(
    ws: &Workspace,
    eval_id: &str,
    facts: &[Value],
    findings: &[Value],
) -> Result<(), EvalError> {
    let path = results_path(ws, eval_id);
    let tmp = path.with_extension("json.part");
    std::fs::write(&tmp, store::canonical(&json!({"facts": facts, "findings": findings})))?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Run one check, as its words: no shell. Its outcome, exit code, output
/// (standard output, then standard error) and seconds.
fn execute(ws: &Workspace, command: &str) -> (&'static str, Option<i32>, String, u64) {
    let started = Instant::now();
    let words: Vec<&str> = command.split_whitespace().collect();
    let Some((program, args)) = words.split_first() else {
        return ("not_run", None, "the check is empty".into(), 0);
    };
    let child = Command::new(program)
        .args(args)
        .current_dir(&ws.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        return ("not_run", None, "the check could not be started".into(), 0);
    };
    let drain = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = r.read_to_end(&mut buf);
            buf
        })
    };
    let out = drain(Box::new(child.stdout.take().expect("piped")));
    let err = drain(Box::new(child.stderr.take().expect("piped")));
    let limit = Duration::from_secs(CHECK_SECONDS);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(_) => break None,
        }
    };
    let mut bytes = out.join().unwrap_or_default();
    bytes.extend(err.join().unwrap_or_default());
    let output = String::from_utf8_lossy(&bytes).to_string();
    let secs = started.elapsed().as_secs();
    match status {
        Some(s) if s.success() => ("succeeded", s.code(), output, secs),
        Some(s) => ("failed", s.code(), output, secs),
        None => ("timed_out", None, output, secs),
    }
}

fn tail_of(output: &str) -> String {
    if output.len() <= TAIL_BYTES {
        return output.to_string();
    }
    let mut start = output.len() - TAIL_BYTES;
    while !output.is_char_boundary(start) {
        start += 1;
    }
    output[start..].to_string()
}

/// The warnings in a check's output, as (path, line, text): rustc and clippy
/// (`warning: …` then ` --> path:line:col`), and the `path:line:col: warning`
/// form of most compilers and linters.
fn warnings(output: &str) -> Vec<(String, u32, String)> {
    let lines: Vec<&str> = output.lines().collect();
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(msg) = l.strip_prefix("warning: ") {
            if msg.ends_with("generated") || msg.contains("warnings emitted") {
                continue;
            }
            let at =
                lines[i + 1..].iter().take(4).find_map(|n| n.trim_start().strip_prefix("--> "));
            if let Some((path, line)) = at.and_then(place) {
                out.push((path, line, format!("warning: {msg}")));
            }
        } else if let Some(pos) = l.find(": warning")
            && let Some((path, line)) = place(&l[..pos])
        {
            out.push((path, line, l[pos + 2..].trim().to_string()));
        }
    }
    out
}

/// `path:line` or `path:line:col`, as (path, line).
fn place(at: &str) -> Option<(String, u32)> {
    let mut parts = at.trim().rsplitn(3, ':').collect::<Vec<_>>();
    parts.reverse();
    let (path, line) = match parts.as_slice() {
        [p, l, c] if c.parse::<u32>().is_ok() => (p.to_string(), l.parse().ok()?),
        [p, l] => (p.to_string(), l.parse().ok()?),
        [p, l, _] => (p.to_string(), l.parse().ok()?),
        _ => return None,
    };
    Some((path.trim_start_matches("./").to_string(), line))
}

/// Each window of the workspace's repository: its path, the new-side line
/// numbers of the lines it adds, and its id.
fn added_lines(ws: &Workspace, eval_id: &str) -> Vec<(String, BTreeSet<u32>, String)> {
    let path = ws.rf_dir().join(format!("evals/{eval_id}/windows.json"));
    let Some(doc) = std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (id, text) in doc["windows"].as_object().into_iter().flatten() {
        let text = text.as_str().unwrap_or_default();
        let mut file = String::new();
        let mut lines = BTreeSet::new();
        let mut next = 0u32;
        for l in text.lines() {
            if let Some(p) = l.strip_prefix("+++ ").or_else(|| l.strip_prefix("--- ")) {
                if file.is_empty() {
                    file = p.trim_start_matches("b/").trim_start_matches("a/").to_string();
                }
            } else if let Some(h) = l.strip_prefix("@@ ") {
                next = h
                    .split_whitespace()
                    .find_map(|p| p.strip_prefix('+'))
                    .and_then(|p| p.split(',').next())
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
            } else if l.starts_with('+') {
                lines.insert(next);
                next += 1;
            } else if l.starts_with(' ') {
                next += 1;
            }
        }
        if !file.is_empty() && !lines.is_empty() {
            out.push((file, lines, id.clone()));
        }
    }
    out
}

/// What the checks left, once all are in.
pub fn results(ws: &Workspace, eval_id: &str) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(results_path(ws, eval_id)).ok()?).ok()
}

/// The checks planned for an Eval, whether or not they are in.
pub fn planned(ws: &Workspace, eval_id: &str) -> Vec<String> {
    let path = ws.rf_dir().join(format!("tmp/eval-{eval_id}/checks.plan.json"));
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .map(|v| {
            array_of(&v, "commands")
                .iter()
                .map(|c| c.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_is_a_command_the_text_states_in_backticks() {
        let found = find([
            "Add `uptime_seconds`; `cargo test -p app-core` passes.",
            "`npm run lint` is clean, then `cargo test -p app-core` again, and `node --test`.",
            "Run `cargo test <crate>` or `pytest ...`; `ls -la`; `git status`.",
            "Not `cargo test && curl x`, `npm test $(id)` or `make \"check\"`.",
        ]);
        assert_eq!(found, ["cargo test -p app-core", "npm run lint", "node --test"]);
    }

    #[test]
    fn warnings_are_read_in_both_forms() {
        let out = "warning: value assigned to `unrecorded` is never read\n   --> crates/app/src/server.rs:542:17\n    |\nsrc/app.ts:12:5: warning: 'x' is unused\nwarning: `app` (lib) generated 1 warning\n";
        assert_eq!(
            warnings(out),
            [
                (
                    "crates/app/src/server.rs".into(),
                    542,
                    "warning: value assigned to `unrecorded` is never read".into()
                ),
                ("src/app.ts".into(), 12, "warning: 'x' is unused".into())
            ]
        );
    }
}
