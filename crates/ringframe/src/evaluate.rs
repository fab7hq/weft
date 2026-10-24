//! Eval: a judged verdict with confidence over every open Ask.
//!
//! The CLI records facts exactly: the open Asks, the anchor, the subject
//! digest, the changed files, how many unrecorded prompts followed each Ask,
//! which judgements were submitted. Sub-agents of the eval skill judge: one
//! reconstructs the effective intent from the Asks; at least three, from
//! distinct angles, vote per item and classify each changed path. The CLI
//! aggregates majorities and agreement into `verdict` and `confidence`.
//! RingFrame runs none of the project's commands and knows nothing about its
//! stack.
//!
//! A `yes` that cites nothing is not decisive. A judge claiming an item is
//! met, having run no command, recorded no basis note, and named no changed
//! path, is asserting rather than checking: that vote counts as `unknown` in
//! the item majority. It is still a vote and still in the denominator, so
//! agreement falls rather than the judge disappearing. Both the vote as cast
//! and what it counted as are recorded, so a reader sees the
//! decision instead of having to repeat it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::store::LedgerError;
use crate::workspace::Workspace;
use crate::{digest, ids, schema, sessions, store};

pub const BRIEF_SCHEMA: &str = "ringframe.eval-brief/1";
pub const KINDS: [&str; 2] = ["git_commit", "worktree"];

pub use crate::ask::{AskError as EvalError, NeedsInput};

pub(crate) fn ledger(code: &str, detail: impl Into<String>) -> EvalError {
    EvalError::Ledger(LedgerError::new_public(code, detail))
}

pub(crate) fn need(ok: bool, code: &str, message: impl Into<String>) -> Result<(), EvalError> {
    if ok { Ok(()) } else { Err(ledger(code, message)) }
}

pub(crate) fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

pub(crate) fn array_of<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// Round to two decimal places, ties to even. The confidence goes into a
/// published record, so it has to land on the same number every time.
pub(crate) fn round2(x: f64) -> f64 {
    (x * 100.0).round_ties_even() / 100.0
}

pub(crate) fn git(root: &Path, args: &[&str]) -> Result<String, EvalError> {
    Ok(String::from_utf8_lossy(&git_bytes(root, args, &[0])?).to_string())
}

/// Git's output as it printed it. `ok` lists the exit codes that are not
/// failures: `diff --no-index` exits 1 when the files differ. RingFrame only
/// reads with Git, so a failed read is tried once more before it counts: a
/// read can fail for a moment ("Not a git repository" in a fresh clone),
/// and an Eval is advice that should not stop on one.
pub(crate) fn git_bytes(root: &Path, args: &[&str], ok: &[i32]) -> Result<Vec<u8>, EvalError> {
    let run = || {
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map_err(|e| ledger("eval.no_git", format!("git: {e}")))
    };
    let mut out = run()?;
    if !out.status.code().is_some_and(|c| ok.contains(&c)) {
        std::thread::sleep(std::time::Duration::from_millis(200));
        out = run()?;
    }
    if !out.status.code().is_some_and(|c| ok.contains(&c)) {
        return Err(ledger(
            "eval.git",
            format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()),
        ));
    }
    Ok(out.stdout)
}

pub fn subject_digest(ws: &Workspace, kind: &str, reference: &str) -> Result<String, EvalError> {
    match kind {
        "git_commit" => {
            let prefix = git(&ws.root, &["rev-parse", "--show-prefix"])?.trim().to_string();
            if !prefix.is_empty() {
                // Hash only this project, including the empty tree when absent in ref.
                let spec = format!(":(top,literal){}", prefix.trim_end_matches('/'));
                let listing =
                    git(&ws.root, &["ls-tree", "--full-tree", "-z", reference, "--", &spec])?;
                return Ok(digest::sha256_bytes(listing.as_bytes()));
            }
            Ok(git(&ws.root, &["rev-parse", &format!("{reference}^{{tree}}")])?.trim().to_string())
        }
        "worktree" => {
            let root = PathBuf::from(reference);
            let listing =
                git(&root, &["ls-files", "-z", "--cached", "--others", "--exclude-standard"])?;
            let mut names: Vec<&str> = listing.split('\0').filter(|n| !n.is_empty()).collect();
            names.sort_unstable();
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            for name in names {
                let p = root.join(name);
                if p.is_file() {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = p.metadata().map(|m| m.permissions().mode() & 0o777).unwrap_or(0);
                    let line = format!("{name}\0{mode:o}\0{}\n", digest::sha256_file(&p)?);
                    h.update(line.as_bytes());
                }
            }
            Ok(format!("{:x}", h.finalize()))
        }
        other => Err(ledger("subject.kind", format!("'{other}' not in {KINDS:?}"))),
    }
}

/// The worktree's files as they are, whatever the subject's kind.
fn worktree_content(ws: &Workspace) -> Result<String, EvalError> {
    subject_digest(ws, "worktree", &ws.root.to_string_lossy())
}

/// The current commit when the tree is clean, else the worktree.
pub fn default_subject(ws: &Workspace) -> Result<(String, String), EvalError> {
    if !git(&ws.root, &["status", "--porcelain", "--", "."])?.trim().is_empty() {
        return Ok(("worktree".into(), ws.root.to_string_lossy().to_string()));
    }
    Ok(("git_commit".into(), git(&ws.root, &["rev-parse", "HEAD"])?.trim().to_string()))
}

// ---- facts -----------------------------------------------------------------

/// Where the change starts: the anchor asked for; else the last Seal (of
/// this unit, when one is judged alone); else the first Ask's base.
fn anchor_of(
    ws: &Workspace,
    asks: &[Value],
    unit: Option<&[String]>,
    explicit: Option<&str>,
) -> Result<Value, EvalError> {
    if let Some(reference) = explicit.filter(|r| !r.is_empty()) {
        return Ok(json!({"kind": "explicit", "ref": reference, "seal_id": null}));
    }
    let ours = |e: &Value| {
        unit.is_none_or(|u| {
            e["data"]["basis"]["asks"].as_array().is_some_and(|a| {
                a.iter().any(|x| u.contains(&x.as_str().unwrap_or_default().to_string()))
            })
        })
    };
    let seals: Vec<Value> =
        store::events(ws)?.into_iter().filter(|e| e["type"] == "seal.created" && ours(e)).collect();
    if let Some(last) = seals.last()
        && last["data"]["subject"]["kind"] == "git_commit"
    {
        return Ok(json!({
            "kind": "seal", "ref": last["data"]["subject"]["ref"], "seal_id": last["id"]
        }));
    }
    let base = asks.iter().find_map(|a| a.get("base_commit").and_then(Value::as_str));
    match base {
        Some(base) => Ok(json!({"kind": "ask_base", "ref": base, "seal_id": null})),
        None => Err(NeedsInput {
            reason:
                "eval.anchor_unknown: no Seal and no Ask with a base commit; pass --anchor <commit>"
                    .into(),
        }
        .into()),
    }
}

/// `{old => new}` and `old => new` both name the file that now exists.
fn rename_target(path: &str) -> String {
    let mut out = path.to_string();
    while let (Some(open), Some(close)) = (out.find('{'), out.find('}')) {
        if close < open {
            break;
        }
        let inner = &out[open + 1..close];
        let Some((_, after)) = inner.split_once(" => ") else { break };
        out = format!("{}{}{}", &out[..open], after, &out[close + 1..]);
    }
    out.rsplit(" => ").next().unwrap_or(&out).to_string()
}

/// Files changed between the anchor and the subject, with line counts. Never
/// their content.
fn count_lines(p: &Path) -> u64 {
    std::fs::read(p).map_or(0, |b| b.iter().filter(|&&c| c == b'\n').count() as u64)
}

fn changes(ws: &Workspace, anchor: &str, subject: &Value) -> Result<Value, EvalError> {
    let kind = str_of(subject, "kind");
    let reference = str_of(subject, "ref");
    let root = if kind == "git_commit" { ws.root.clone() } else { PathBuf::from(&reference) };
    let mut args: Vec<&str> = vec!["diff", "--relative", "--name-status", "-M", anchor];
    if kind == "git_commit" {
        args.push(&reference);
    }
    args.extend(["--", "."]);
    let mut status: BTreeMap<String, String> = BTreeMap::new();
    for line in git(&root, &args)?.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let code = parts[0].chars().next().unwrap_or(' ');
            let named = match code {
                'A' => "added".to_string(),
                'M' => "modified".to_string(),
                'D' => "deleted".to_string(),
                'R' => "renamed".to_string(),
                other => other.to_lowercase().to_string(),
            };
            status.insert(parts[parts.len() - 1].to_string(), named);
        }
    }
    let mut args: Vec<&str> = vec!["diff", "--relative", "--numstat", "-M", anchor];
    if kind == "git_commit" {
        args.push(&reference);
    }
    args.extend(["--", "."]);
    let mut files: BTreeMap<String, Value> = BTreeMap::new();
    for line in git(&root, &args)?.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() == 3 {
            let name = rename_target(parts[2]);
            let num = |s: &str| if s == "-" { 0 } else { s.parse::<u64>().unwrap_or(0) };
            files.insert(
                name.clone(),
                json!({
                    "path": name,
                    "status": status.get(&name).cloned().unwrap_or_else(|| "modified".into()),
                    "added": num(parts[0]), "removed": num(parts[1]),
                }),
            );
        }
    }
    if kind == "worktree" {
        for name in git(&root, &["ls-files", "--others", "--exclude-standard", "-z"])?.split('\0') {
            if !name.is_empty() && !files.contains_key(name) {
                files.insert(
                    name.to_string(),
                    json!({
                        "path": name, "status": "added",
                        "added": count_lines(&root.join(name)), "removed": 0,
                    }),
                );
            }
        }
    }
    let ordered: Vec<Value> = files.into_values().collect();
    let sum = |key: &str| -> u64 { ordered.iter().filter_map(|f| f[key].as_u64()).sum() };
    Ok(json!({"files": ordered, "total_added": sum("added"), "total_removed": sum("removed")}))
}

const PATCH_LIMIT: usize = 1 << 20;

/// The exact anchor-to-subject diff, untracked files as new-file hunks, and
/// whether it had to be cut to stay under the limit.
fn patch(anchor: &str, subject: &Value, ws: &Workspace) -> Result<(Vec<u8>, bool), EvalError> {
    let kind = str_of(subject, "kind");
    let reference = str_of(subject, "ref");
    let root = if kind == "git_commit" { ws.root.clone() } else { PathBuf::from(&reference) };
    let mut args = vec!["diff", "--relative", "--binary", anchor];
    if kind == "git_commit" {
        args.push(&reference);
    }
    args.extend(["--", "."]);
    let mut out = git_bytes(&root, &args, &[0])?;
    if kind == "worktree" {
        for name in git(&root, &["ls-files", "--others", "--exclude-standard", "-z"])?.split('\0') {
            if !name.is_empty() {
                out.extend(git_bytes(
                    &root,
                    &["diff", "--no-index", "--binary", "/dev/null", name],
                    &[0, 1],
                )?);
            }
        }
    }
    if out.len() <= PATCH_LIMIT {
        return Ok((out, false));
    }
    // Cut at the last whole file hunk that ends within the limit.
    let starts: Vec<usize> = (0..out.len())
        .filter(|&i| out[i..].starts_with(b"diff --git ") && (i == 0 || out[i - 1] == b'\n'))
        .collect();
    let end = starts.iter().copied().filter(|&i| i <= PATCH_LIMIT).max().unwrap_or(0);
    out.truncate(end);
    Ok((out, true))
}

/// The roles a caller may name: the current three, and the earlier names
/// `tasks::current_roles` maps onto them.
const ROLES: [&str; 9] =
    ["map", "reduce", "confirm", "context", "intent", "coverage", "drift", "adversary", "trace"];

/// What the caller asked each role to run on: its shape, never a catalogue.
fn validate_agents(agents: &Value) -> Result<(), EvalError> {
    let code = "eval.agents";
    let map = agents.as_object().ok_or_else(|| ledger(code, "agents must be an object"))?;
    for (role, keys) in map {
        need(
            ROLES.contains(&role.as_str()),
            code,
            format!("agents.{role} is not a role; use one of {}", ROLES.join(", ")),
        )?;
        let keys = keys
            .as_object()
            .ok_or_else(|| ledger(code, format!("agents.{role} must be an object")))?;
        for (key, value) in keys {
            need(
                key == "model" || key == "effort",
                code,
                format!("agents.{role}.{key} is not model or effort"),
            )?;
            need(
                value.as_str().is_some_and(|v| !v.is_empty()),
                code,
                format!("agents.{role}.{key} must be a non-empty string"),
            )?;
        }
    }
    Ok(())
}

/// What a brief or a Seal says when no prompt and no tool was observed at
/// all.
pub const NOTHING_OBSERVED: &str =
    "no prompt or tool was observed: nothing ran `ringframe observe`";

/// Who observed these Asks' submissions, in words ("submission observed by
/// <name>", the name `observe --by` gave); or, when nothing was observed in
/// this workspace at all, that nothing was.
pub fn observed_limits(ws: &Workspace, ask_ids: &[String]) -> Result<Vec<String>, EvalError> {
    let events = store::events(ws)?;
    let observers: BTreeSet<String> = events
        .iter()
        .filter(|e| e["type"] == "ask.submission" && e["data"]["state"] == "observed")
        .filter(|e| ask_ids.contains(&str_of(e, "id")))
        .map(|e| str_of(&e["data"], "observed_by"))
        .filter(|by| !by.is_empty())
        .collect();
    let mut out: Vec<String> =
        observers.iter().map(|by| format!("submission observed by {by}")).collect();
    let prompts = std::fs::read_dir(ws.rf_dir().join("sessions"))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|host| std::fs::read_dir(host.path()).into_iter().flatten().flatten())
        .any(|session| session.path().join("prompts.jsonl").is_file());
    if out.is_empty() && !prompts && !events.iter().any(|e| e["type"] == "tool.fact") {
        out.push(NOTHING_OBSERVED.to_string());
    }
    Ok(out)
}

/// How many prompts that were neither `/rf:` invocations nor observed Ask
/// submissions bear on this Eval.
///
/// Measured from the anchor, which is where the delta is measured from, so
/// the two facts describe one span. Prompts between the anchor and the first
/// open Ask are their own count: they were said before that Ask existed and
/// are not its doing.
fn unrecorded_prompts(
    ws: &Workspace,
    asks: &[Value],
    since: Option<&str>,
) -> Result<(usize, Vec<usize>), EvalError> {
    let submitted: BTreeSet<String> = store::events(ws)?
        .iter()
        .filter(|e| e["type"] == "ask.submission")
        .map(|e| str_of(&e["data"], "prompt_sha256"))
        .collect();
    let mut times: Vec<String> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    let base = ws.rf_dir().join("sessions");
    for host in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        for session in std::fs::read_dir(host.path()).into_iter().flatten().flatten() {
            let p = session.path().join("prompts.jsonl");
            if p.is_file() {
                paths.push(p);
            }
        }
    }
    paths.sort();
    for path in paths {
        for line in std::fs::read_to_string(&path).unwrap_or_default().lines() {
            let rec: Value = serde_json::from_str(line).unwrap_or(Value::Null);
            if rec.get("prompt").is_none() && !submitted.contains(&str_of(&rec, "sha256")) {
                times.push(str_of(&rec, "time"));
            }
        }
    }
    let count_in =
        |start: &str, end: &str| times.iter().filter(|t| &***t >= start && &***t < end).count();
    let before = match (since, asks.first()) {
        (Some(since), Some(first)) => count_in(since, &str_of(first, "time")),
        _ => 0,
    };
    let per_ask = asks
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let end = asks.get(i + 1).map_or_else(|| "9".to_string(), |next| str_of(next, "time"));
            count_in(&str_of(a, "time"), &end)
        })
        .collect();
    Ok((before, per_ask))
}

/// When the anchor happened, for the prompts to be counted from. Only a Seal
/// names a moment in the ledger; an Ask's base commit and a hand-given one do
/// not, so those keep counting from the first open Ask.
fn anchor_time(ws: &Workspace, anchor: &Value) -> Option<String> {
    let seal_id = anchor.get("seal_id")?.as_str()?;
    store::events(ws)
        .ok()?
        .into_iter()
        .find(|e| e["type"] == "seal.created" && str_of(e, "id") == seal_id)
        .map(|e| str_of(&e, "time"))
}

pub(crate) fn event(
    type_: &str,
    id: &str,
    actor: Option<&Value>,
    data: Value,
    links: Vec<Value>,
) -> Result<Value, EvalError> {
    let ev = json!({
        "schema": store::SCHEMA, "event_id": ids::new_id("evt"), "type": type_,
        "time": sessions::now(), "id": id,
        "actor": actor.cloned().unwrap_or_else(||
            json!({"kind": "human", "id": "local-user", "authority": "interactive"})),
        "links": links, "data": data
    });
    schema::validate_event(&ev)?;
    Ok(ev)
}

#[derive(Default)]
pub struct Open<'a> {
    pub anchor: Option<&'a str>,
    pub subject_kind: Option<&'a str>,
    pub subject_ref: Option<&'a str>,
    pub actor: Option<Value>,
    /// Model and effort per role, as the caller asked; recorded, never checked
    /// against a harness.
    pub agents: Option<Value>,
    /// Repositories no Ask named, `<path>[=<anchor>]`.
    pub repos: Vec<String>,
    /// The harness that opened it. Given, the Eval is gathered as it opens:
    /// what RingFrame prepared is the change every later task reads, in this
    /// harness or another, so no agent gathers it again.
    pub host: Option<&'a str>,
    /// An Ask of the unit of work to judge: only that unit's open Asks are
    /// judged, and the other open Asks are named as work it does not judge.
    /// None judges every open Ask.
    pub ask: Option<&'a str>,
    /// What the judges must not do, and where that came from: an override
    /// layer's name, or `default`. Recorded once, so every task of this Eval,
    /// in any harness, is given the same rules.
    pub deny: Option<(&'a str, Vec<String>)>,
}

/// Write the facts-only brief over every open Ask and append `eval.opened`.
pub fn open_eval(ws: &Workspace, args: Open<'_>) -> Result<Value, EvalError> {
    let agents = args.agents.clone().unwrap_or_else(|| json!({}));
    validate_agents(&agents)?;
    let agents = crate::tasks::current_roles(&agents);
    // `ask` does not import `evaluate`.
    let asks = crate::ask::open_asks(ws)?;
    let unit = args.ask.map(|id| crate::ask::unit_of(ws, id)).transpose()?;
    let (asks, elsewhere): (Vec<Value>, Vec<Value>) = match &unit {
        Some(unit) => asks.into_iter().partition(|a| unit.contains(&str_of(a, "ask_id"))),
        None => (asks, Vec::new()),
    };
    need(
        !asks.is_empty(),
        "eval.no_open_ask",
        "nothing to evaluate: every Ask is sealed or cancelled",
    )?;
    need(
        args.subject_kind.is_some() == args.subject_ref.is_some(),
        "subject.kind",
        "--subject-kind and --subject-ref go together",
    )?;
    if let Some(kind) = args.subject_kind {
        need(KINDS.contains(&kind), "subject.kind", format!("'{kind}' not in {KINDS:?}"))?;
    }
    git(&ws.root, &["rev-parse", "--git-dir"]).map_err(|_| {
        ledger("eval.no_git", "Eval reads the Git delta; this workspace is not a Git repository")
    })?;
    let (kind, reference) = match (args.subject_kind, args.subject_ref) {
        // A worktree subject is a path, and it is hashed and diffed. It has to
        // be the opened directory or inside it, or the record would describe a
        // tree this workspace does not hold.
        (Some(k), Some(r)) if k == "worktree" => (
            k.to_string(),
            crate::workspace::within(ws, std::path::Path::new(r))
                .map_err(|e| ledger(&e.code, e.detail))?
                .to_string_lossy()
                .to_string(),
        ),
        (Some(k), Some(r)) => (k.to_string(), r.to_string()),
        _ => default_subject(ws)?,
    };
    let ask_ids: Vec<String> = asks.iter().map(|a| str_of(a, "ask_id")).collect();
    let subject =
        json!({"kind": kind, "ref": reference, "sha256": subject_digest(ws, &kind, &reference)?});
    // An Eval left open over the same Asks is continued while it judges this
    // same work. Once the work has moved on, continuing it would judge what is
    // gone, so a new Eval opens and says which one it supersedes.
    let dangling: Vec<Value> = list_records(ws)?
        .into_iter()
        .filter(|r| {
            r["state"] == "opened"
                && array_of(&r["basis"], "asks").iter().map(str_of_value).collect::<Vec<_>>()
                    == ask_ids
        })
        .collect();
    let same_work = |r: &Value| {
        r["subject"]["kind"] == subject["kind"] && r["subject"]["sha256"] == subject["sha256"]
    };
    if let Some(open) = dangling.iter().rfind(|r| same_work(r)) {
        return Err(ledger(
            "eval.already_open",
            format!(
                "{} is open over the same Asks and the same work, and not closed; close it or continue with it",
                str_of(open, "eval_id")
            ),
        ));
    }
    let superseded: Vec<String> = dangling.iter().map(|r| str_of(r, "eval_id")).collect();
    let anchor = anchor_of(ws, &asks, unit.as_deref(), args.anchor)?;
    let anchor_ref = str_of(&anchor, "ref");
    git(&ws.root, &["rev-parse", "--verify", "--quiet", &format!("{anchor_ref}^{{commit}}")])
        .map_err(|_| {
            ledger("eval.anchor_missing", format!("{anchor_ref} is not a commit in this workspace"))
        })?;
    let eval_id = ids::new_id("evl");
    let (before, counts) = unrecorded_prompts(ws, &asks, anchor_time(ws, &anchor).as_deref())?;
    let shares = |r: &Value| {
        array_of(&r["basis"], "asks").iter().any(|a| ask_ids.contains(&str_of_value(a)))
    };
    let previous: Vec<Value> = list_records(ws)?
        .into_iter()
        .filter(|r| r["state"] == "completed" && shares(r))
        .map(|r| {
            json!({"eval_id": r["eval_id"], "verdict": r["verdict"],
                        "confidence": r["confidence"], "time": r["completed_at"]})
        })
        .collect();
    let mut limitations = vec![
        "the brief describes the ledger and the Git delta only; RingFrame runs none of the project's commands".to_string(),
        "plain prompts are counted, never stored; changes no Ask explains may follow an unrecorded instruction".to_string(),
    ];
    if kind == "worktree" {
        limitations.push("subject is the uncommitted worktree".into());
    }
    if !superseded.is_empty() {
        limitations.push(format!(
            "supersedes {}, left open over the same Asks and earlier work",
            superseded.join(", ")
        ));
    }
    // What the harness saw the agent run, in order. Only a fact about this
    // exact subject can say anything about the work being judged. A fact
    // records the worktree's content, and a clean commit at HEAD holds exactly
    // that, so tests run before the commit still describe it. The brief lists
    // only those: a long run records every command it ran, and listing the
    // stale ones made a brief too large to read for no evidence.
    let content = match kind.as_str() {
        "worktree" => Some(str_of(&subject, "sha256")),
        "git_commit" if default_subject(ws)? == (kind.clone(), reference.clone()) => {
            Some(worktree_content(ws)?)
        }
        _ => None,
    };
    let since = anchor_time(ws, &anchor);
    let (facts, stale): (Vec<Value>, Vec<Value>) = store::events(ws)?
        .iter()
        .filter(|e| e["type"] == "tool.fact")
        .filter(|e| since.as_deref().is_none_or(|t| str_of(e, "time").as_str() >= t))
        .map(|e| {
            let about = &e["data"]["subject"];
            json!({
                "id": e["id"], "command": e["data"]["command"], "outcome": e["data"]["outcome"],
                "time": e["time"],
                "fresh": (about["kind"] == subject["kind"] && about["sha256"] == subject["sha256"])
                    || content.as_deref().is_some_and(|c| about["content"] == c),
            })
        })
        .partition(|f| f["fresh"] == true);
    if facts.is_empty() && stale.is_empty() {
        limitations
            .push("no tool facts were recorded; the result of a command cannot be judged".into());
    }
    limitations.extend(observed_limits(ws, &ask_ids)?);
    if !stale.is_empty() {
        limitations.push(format!(
            "{} of the recorded commands ran against other work than the subject and are not listed",
            stale.len()
        ));
    }
    let (patch_bytes, cut) = patch(&anchor_ref, &subject, ws)?;
    if cut {
        limitations.push("the patch is cut at 1 MiB; read the remaining paths with git".into());
    }
    let changes_patch = store::publish(
        ws,
        &format!("evals/{eval_id}/changes.patch"),
        &patch_bytes,
        "changes_patch",
    )?;
    // RingFrame's own reading of the work, for judges that read it in bounded
    // pieces. An Eval still opens without it.
    let (evidence, evidence_refs) = match crate::evidence::prepare(
        ws,
        &eval_id,
        &asks,
        &anchor_ref,
        &kind,
        &reference,
        &args.repos,
        &mut limitations,
    ) {
        Ok(v) => v,
        Err(e) => {
            limitations.push(format!("the evidence could not be prepared: {e}"));
            (Value::Null, json!({}))
        }
    };
    // The checks the Asks and their plan steps state, which RingFrame runs
    // once: found here, started once the Eval is on record.
    let checks = {
        let changes: Value =
            std::fs::read(ws.rf_dir().join(format!("evals/{eval_id}/changes.json")))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(Value::Null);
        let prompts: Vec<String> = asks
            .iter()
            .filter_map(|a| {
                std::fs::read_to_string(ws.rf_dir().join(str_of(&a["prompt"], "path"))).ok()
            })
            .collect();
        let steps = array_of(&changes, "requirements")
            .iter()
            .flat_map(|r| [str_of(r, "text"), str_of(r, "done_when")])
            .collect::<Vec<_>>();
        crate::checks::find(prompts.iter().chain(steps.iter()).map(String::as_str))
    };
    let brief = json!({
        "schema": BRIEF_SCHEMA, "eval_id": eval_id, "time": sessions::now(),
        "checks": checks,
        "workspace": ws.describe(), "anchor": anchor, "subject": subject,
        "asks": asks.iter().enumerate().map(|(i, a)| json!({
            "ask_id": a["ask_id"], "order": i + 1, "title": a["title"],
            "prompt_path": a["prompt"]["path"], "compiled": a["time"],
            "confirmed": a["confirmed_at"], "submission": a["submission"], "links": a["links"],
            "unrecorded_prompts_after": counts[i],
        })).collect::<Vec<_>>(),
        "changes": changes(ws, &anchor_ref, &subject)?,
        // Other units' open Asks: their work may be in this change, and is
        // not judged here.
        "elsewhere": elsewhere.iter().map(|a| json!({
            "ask_id": a["ask_id"], "title": a["title"],
        })).collect::<Vec<_>>(),
        "unrecorded_prompts_before": before,
        "previous_evals": previous, "facts": facts, "limitations": limitations,
        "agents": agents, "changes_patch": changes_patch, "evidence": evidence,
        "judges": crate::tasks::judges(args.deny.as_ref()),
    });
    let mut bytes = store::canonical(&brief);
    bytes.push(b'\n');
    let reference =
        store::publish(ws, &format!("evals/{eval_id}/brief.json"), &bytes, "eval_brief")?;
    let mut data = json!({
        "brief": reference, "changes_patch": changes_patch, "anchor": anchor, "subject": subject,
        "basis": {
            "asks": ask_ids,
            "unrecorded_prompts": before + counts.iter().sum::<usize>(),
            "unrecorded_prompts_before": before,
        },
    });
    for (k, v) in evidence_refs.as_object().into_iter().flatten() {
        data[k] = v.clone();
    }
    let links: Vec<Value> = asks
        .iter()
        .map(|a| json!({"rel": "evaluates", "id": a["ask_id"]}))
        .chain(superseded.iter().map(|id| json!({"rel": "supersedes", "id": id})))
        .collect();
    store::append(ws, &event("eval.opened", &eval_id, args.actor.as_ref(), data.clone(), links)?)?;
    // Fail-safe: an Eval whose checks cannot start goes on without them.
    let _ = crate::checks::start(ws, &eval_id, &checks);
    if let (Some(host), Some(evidence)) =
        (args.host.filter(|h| !h.is_empty()), evidence_refs.get("evidence"))
    {
        let gathered = json!({"eval_id": eval_id, "context_map": evidence, "host": host});
        store::append(
            ws,
            &event("eval.gathered", &eval_id, args.actor.as_ref(), gathered, Vec::new())?,
        )?;
    }
    let mut out = json!({
        "eval_id": eval_id,
        "brief_path": ws.rf_dir().join(str_of(&reference, "path")).to_string_lossy(),
        "agents": agents,
        "changes": {
            "files": array_of(&brief["changes"], "files").len(),
            "added": brief["changes"]["total_added"], "removed": brief["changes"]["total_removed"],
        },
    });
    for (k, v) in data.as_object().into_iter().flatten() {
        out[k] = v.clone();
    }
    out["changes_patch"] =
        json!(ws.rf_dir().join(str_of(&changes_patch, "path")).to_string_lossy());
    Ok(out)
}

fn str_of_value(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// What the working agent ran, as someone saw it (`ringframe observe tool` on
/// the host's shell tool): the command, whether it succeeded, and the subject
/// it ran against. Output is never read, and RingFrame decides nothing about
/// what the command was for. Only while some Ask is open, and never for
/// RingFrame's own commands. Whether it succeeded, and who saw it, are the
/// caller's to say.
pub fn record_fact(
    ws: &Workspace,
    host: &str,
    session: &str,
    outcome: &str,
    command: &str,
    by: &str,
) -> Result<Option<Value>, EvalError> {
    if !["succeeded", "failed"].contains(&outcome) {
        return Ok(None);
    }
    let first = command.split_whitespace().next();
    if first.is_none() || first == Some("ringframe") {
        return Ok(None);
    }
    let open = crate::ask::open_asks(ws).map_err(|e| ledger("fact.open_asks", e.to_string()))?;
    if open.is_empty() {
        return Ok(None);
    }
    let (kind, reference) = default_subject(ws)?;
    let sha = subject_digest(ws, &kind, &reference)?;
    let data = json!({
        "command": command, "outcome": outcome,
        "subject": {"kind": kind, "ref": reference, "sha256": sha,
                    "content": worktree_content(ws)?},
        "host": host, "session_ref": session, "observed_by": by,
    });
    let actor = json!({"kind": "agent", "id": host});
    let ev = event("tool.fact", &ids::new_id("fct"), Some(&actor), data, Vec::new())?;
    store::append(ws, &ev)?;
    Ok(Some(ev))
}

pub fn load_record(ws: &Workspace, eval_id: &str) -> Result<Option<Value>, EvalError> {
    let p = ws.rf_dir().join(format!("evals/{eval_id}/record.json"));
    match std::fs::read(&p) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).map_err(|e| ledger("eval.missing", e.to_string()))?,
        )),
        Err(_) => Ok(None),
    }
}

/// Every Eval in this workspace, opened or completed, oldest first.
pub fn list_records(ws: &Workspace) -> Result<Vec<Value>, EvalError> {
    let base = ws.rf_dir().join("evals");
    let mut briefs: Vec<(String, Value, Vec<u8>)> = Vec::new();
    for entry in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        let p = entry.path().join("brief.json");
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let Ok(brief) = serde_json::from_slice::<Value>(&bytes) else { continue };
        briefs.push((str_of(&brief, "time"), brief, bytes));
    }
    briefs.sort_by(|a, b| a.0.cmp(&b.0));
    let voided = voided(ws)?;
    let mut out = Vec::new();
    for (_, brief, bytes) in briefs {
        let eval_id = str_of(&brief, "eval_id");
        let rec = load_record(ws, &eval_id)?;
        let field = |key: &str| rec.as_ref().map_or(Value::Null, |r| r[key].clone());
        // Every reader that wants an Eval to build on asks for `completed` or
        // `opened`, so a voided one is skipped by all of them at once.
        let state = if voided.contains_key(&eval_id) {
            "voided"
        } else if rec.is_some() {
            "completed"
        } else {
            "opened"
        };
        out.push(json!({
            "eval_id": eval_id, "state": state,
            "voided": voided.get(&eval_id).cloned().unwrap_or(Value::Null),
            "opened_at": brief["time"],
            "basis": {"asks": array_of(&brief, "asks").iter()
                .map(|a| a["ask_id"].clone()).collect::<Vec<_>>()},
            "anchor": brief["anchor"], "subject": brief["subject"],
            "verdict": field("verdict"), "confidence": field("confidence"),
            "completed_at": field("time"),
            "brief": {"path": format!("evals/{eval_id}/brief.json"), "sha256": digest::sha256_bytes(&bytes)},
            "gathered": ws.rf_dir().join(format!("evals/{eval_id}/context.md")).exists(),
            "path": if rec.is_some() {
                format!("evals/{eval_id}/record.json")
            } else {
                format!("evals/{eval_id}/brief.json")
            },
        }));
    }
    Ok(out)
}

/// Every voided Eval, by id: its reason and when.
pub fn voided(ws: &Workspace) -> Result<BTreeMap<String, Value>, EvalError> {
    Ok(store::events(ws)?
        .iter()
        .filter(|e| e["type"] == "eval.voided")
        .map(|e| (str_of(e, "id"), json!({"reason": e["data"]["reason"], "time": e["time"]})))
        .collect())
}

/// Withdraw an Eval that should not be built on. It stays in the record; it is
/// no longer carried over, followed, remedied, sealed on or continued. Only a
/// person withdraws one: a model that judged badly does not get to decide that
/// its judgement did not count.
pub fn void_eval(
    ws: &Workspace,
    eval_id: &str,
    reason: &str,
    actor: Option<&Value>,
) -> Result<Value, EvalError> {
    need(!reason.trim().is_empty(), "eval.void_reason", "say why with --reason")?;
    let actor = actor.cloned().unwrap_or_else(
        || json!({"kind": "human", "id": "local-user", "authority": "interactive"}),
    );
    need(
        actor["kind"] == "human",
        "eval.void_needs_person",
        "only a person voids an Eval; run it without --actor, or with --actor human:<id>",
    )?;
    need(
        store::events(ws)?.iter().any(|e| e["type"] == "eval.opened" && str_of(e, "id") == eval_id),
        "eval.missing",
        format!("{eval_id} is not an Eval in this workspace"),
    )?;
    if let Some(v) = voided(ws)?.get(eval_id) {
        return Err(ledger(
            "eval.already_voided",
            format!("{eval_id} was voided at {}", str_of(v, "time")),
        ));
    }
    store::append(
        ws,
        &event("eval.voided", eval_id, Some(&actor), json!({"reason": reason.trim()}), vec![])?,
    )?;
    Ok(json!({"eval_id": eval_id, "state": "voided", "reason": reason.trim()}))
}

/// A report's head, then each section's heading and how many items it
/// holds, then where the whole report is: what a harness shows in a
/// conversation, since typing the whole report again cost minutes of output
/// (q03: 40 KB, 3.7 minutes).
pub fn summary_of(report: &str, eval_id: &str) -> String {
    let (head, rest) = report.split_once("\n## ").map_or((report, ""), |(h, r)| (h, r));
    let mut out = head.trim_end().to_string();
    out.push_str("\n\n");
    for section in rest.split("\n## ") {
        let mut lines = section.lines();
        let Some(title) = lines.next() else { continue };
        let items = lines.filter(|l| l.starts_with("- ") || l.starts_with("| ")).count();
        out.push_str(&format!("- {}: {items}\n", title.trim_start_matches("## ").trim()));
    }
    out.push_str(&format!(
        "\nThe whole report: `.fab7/rf/evals/{eval_id}/eval.md`, or `ringframe eval show --eval {eval_id}`.\n"
    ));
    out
}

/// The report of a completed Eval, as RingFrame wrote it. A harness shows this
/// and composes nothing of its own: a coordinator that wrote its
/// own report from a record it had read in part invented sixteen obligations.
pub fn show_eval(ws: &Workspace, eval_id: &str) -> Result<String, EvalError> {
    let path = ws.rf_dir().join(format!("evals/{eval_id}/eval.md"));
    let Ok(text) = std::fs::read_to_string(&path) else {
        let opened = store::events(ws)?
            .iter()
            .any(|e| e["type"] == "eval.opened" && str_of(e, "id") == eval_id);
        return Err(if opened {
            ledger("eval.not_completed", format!("{eval_id} is open; it has no report yet"))
        } else {
            ledger("eval.missing", format!("{eval_id} is not an Eval in this workspace"))
        });
    };
    Ok(match voided(ws)?.get(eval_id) {
        Some(v) => format!(
            "> Voided {}: {}. Nothing builds on this Eval.\n\n{text}",
            str_of(v, "time"),
            str_of(v, "reason")
        ),
        None => text,
    })
}

/// The latest completed Eval whose basis shares an Ask with the given set.
pub fn latest_for(ws: &Workspace, ask_ids: &[String]) -> Result<Option<Value>, EvalError> {
    let found = list_records(ws)?.into_iter().rfind(|r| {
        r["state"] == "completed"
            && array_of(&r["basis"], "asks").iter().any(|a| ask_ids.contains(&str_of_value(a)))
    });
    match found {
        Some(r) => load_record(ws, &str_of(&r, "eval_id")),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{commit, eval_bench, head, opened, two_asks_and_work, with_config_home};

    fn brief_of(ws: &Workspace, out: &Value) -> Value {
        serde_json::from_slice(
            &std::fs::read(ws.rf_dir().join(str_of(&out["brief"], "path"))).unwrap(),
        )
        .unwrap()
    }

    fn refused(e: EvalError, expect: &str) {
        let text = e.to_string();
        assert!(text.contains(expect), "wanted {expect:?}, got {text:?}");
    }

    fn paths_of(v: &Value) -> Vec<String> {
        array_of(v, "files").iter().map(|f| str_of(f, "path")).collect()
    }

    #[test]
    fn subject_digests() {
        eval_bench(|ws| {
            assert_eq!(subject_digest(ws, "git_commit", &head(&ws.root)).unwrap().len(), 40);
            let before = subject_digest(ws, "worktree", &ws.root.to_string_lossy()).unwrap();
            std::fs::write(ws.root.join("new.txt"), "x").unwrap();
            assert_ne!(subject_digest(ws, "worktree", &ws.root.to_string_lossy()).unwrap(), before);
            std::fs::write(ws.root.join(".gitignore"), "ignored.txt\n").unwrap();
            std::fs::write(ws.root.join("ignored.txt"), "y").unwrap();
            let with_ignore = subject_digest(ws, "worktree", &ws.root.to_string_lossy()).unwrap();
            std::fs::write(ws.root.join("ignored.txt"), "z").unwrap();
            assert_eq!(
                subject_digest(ws, "worktree", &ws.root.to_string_lossy()).unwrap(),
                with_ignore,
                "an ignored file is not part of the subject"
            );
            refused(subject_digest(ws, "planet", "mars").unwrap_err(), "subject.kind");
        });
    }

    #[test]
    fn open_refuses_without_an_open_ask() {
        eval_bench(|ws| {
            refused(open_eval(ws, Open::default()).unwrap_err(), "eval.no_open_ask");
            let out = crate::testing::confirm_ask(ws, "t", b"s\n", b"p\n");
            crate::ask::cancel(ws, &str_of(&out, "ask_id"), None, None, false).unwrap();
            refused(open_eval(ws, Open::default()).unwrap_err(), "eval.no_open_ask");
        });
    }

    #[test]
    fn open_writes_a_facts_only_brief_over_the_open_asks() {
        eval_bench(|ws| {
            let base = head(&ws.root);
            let (a, b, sha) = two_asks_and_work(ws);
            // Three plain prompts after Ask B, one observed submission of A's
            // prompt (not counted), one /rf: invocation (not counted).
            let say = |text: &str| {
                crate::sessions::observe_prompt(ws, "claude-code", "s9", text, None, "weft")
                    .unwrap()
                    .unwrap()
            };
            say("please also update the docs");
            say("run the tests");
            say("/rf:eval");
            let sub = say("Fix the login bug.\n");
            assert_eq!(
                crate::ask::submission_from_capture(
                    ws,
                    "claude-code",
                    "s9",
                    &str_of(&sub, "sha256"),
                    "hook:UserPromptSubmit"
                )
                .unwrap()
                .unwrap()["state"],
                "observed"
            );
            say("now tidy up");

            let out = open_eval(ws, Open::default()).unwrap();
            assert!(str_of(&out, "eval_id").starts_with("evl_"));
            assert_eq!(out["basis"]["asks"], json!([a, b]));
            assert_eq!(out["basis"]["unrecorded_prompts"], 3);
            assert_eq!(out["anchor"], json!({"kind": "ask_base", "ref": base, "seal_id": null}));
            assert_eq!(out["subject"]["kind"], "git_commit");
            assert_eq!(out["subject"]["ref"], sha);

            let brief = brief_of(ws, &out);
            assert_eq!(brief["schema"], BRIEF_SCHEMA);
            assert_eq!(
                array_of(&brief, "asks").iter().map(|x| x["order"].clone()).collect::<Vec<_>>(),
                [json!(1), json!(2)]
            );
            let first = &array_of(&brief, "asks")[0];
            assert!(str_of(first, "prompt_path").ends_with("prompt.txt"));
            assert!(first["confirmed"].is_string());
            assert_eq!(first["submission"], "observed");
            assert_eq!(
                array_of(&brief, "asks")
                    .iter()
                    .map(|x| x["unrecorded_prompts_after"].clone())
                    .collect::<Vec<_>>(),
                [json!(0), json!(3)]
            );
            assert_eq!(
                array_of(&brief["changes"], "files")
                    .iter()
                    .map(|f| (str_of(f, "path"), str_of(f, "status"), f["added"].clone()))
                    .collect::<Vec<_>>(),
                [
                    ("docs/notes.md".to_string(), "added".to_string(), json!(1)),
                    ("src/uptime.js".to_string(), "added".to_string(), json!(1)),
                    ("tests/uptime.test.js".to_string(), "added".to_string(), json!(1)),
                ]
            );
            assert_eq!(brief["previous_evals"], json!([]));
            // The brief names no tool and no build file: it describes the
            // ledger and the Git delta only.
            let text = brief.to_string().replace(&ws.root.to_string_lossy().to_string(), "");
            for banned in ["npm", "pytest", "package.json", "Makefile"] {
                assert!(!text.contains(banned), "the brief mentions {banned}");
            }
            let last = store::events(ws).unwrap().pop().unwrap();
            assert_eq!(last["type"], "eval.opened");
            assert_eq!(
                last["links"],
                json!([{"rel": "evaluates", "id": a}, {"rel": "evaluates", "id": b}])
            );
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
            let listed = list_records(ws).unwrap();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0]["state"], "opened");
            assert_eq!(listed[0]["verdict"], json!(null));
            // A debate in another harness has only this to go on.
            assert_eq!(listed[0]["brief"]["sha256"], out["brief"]["sha256"]);
            assert_eq!(listed[0]["brief"]["path"], out["brief"]["path"]);
            assert_eq!(listed[0]["gathered"], false);
        });
    }

    #[test]
    fn prompts_between_a_seal_and_the_next_ask_are_counted() {
        // A Seal closes the previous Asks and the next one does not exist
        // yet. Everything said in between used to fall outside every window
        // and be counted against nothing, which read as reassurance.
        eval_bench(|ws| {
            let o = opened(ws);
            crate::testing::settled(ws, &str_of(&o.out, "eval_id"), false);
            crate::seal::create(ws, "accepted", None, None, None, None).unwrap();

            // Two plain prompts, then a commit, then the next Ask.
            for text in ["also update the docs", "and drop the cache bit"] {
                crate::sessions::observe_prompt(ws, "claude-code", "s9", text, None, "weft")
                    .unwrap();
            }
            commit(&ws.root, &[("docs/notes.md", Some("edited by chat\n"))], "chat edit");
            crate::testing::confirm_ask(ws, "Next", b"do the next thing\n", b"Do it.\n");

            let out = open_eval(ws, Open::default()).unwrap();
            assert_eq!(out["anchor"]["kind"], "seal", "the anchor is the Seal");
            assert_eq!(out["basis"]["unrecorded_prompts_before"], 2);
            assert_eq!(out["basis"]["unrecorded_prompts"], 2, "the total includes them");
            let brief = brief_of(ws, &out);
            assert_eq!(brief["unrecorded_prompts_before"], 2);
            // They belong to no Ask: the Ask did not exist when they were said.
            assert_eq!(
                array_of(&brief, "asks")
                    .iter()
                    .map(|a| a["unrecorded_prompts_after"].clone())
                    .collect::<Vec<_>>(),
                [json!(0)]
            );
        });
    }

    #[test]
    fn without_a_seal_the_count_is_unchanged() {
        // An Ask's base commit names no moment in the ledger, so counting
        // still starts at the first open Ask, exactly as before.
        eval_bench(|ws| {
            two_asks_and_work(ws);
            crate::sessions::observe_prompt(
                ws,
                "claude-code",
                "s9",
                "a plain prompt",
                None,
                "weft",
            )
            .unwrap();
            let out = open_eval(ws, Open::default()).unwrap();
            assert_eq!(out["anchor"]["kind"], "ask_base");
            assert_eq!(out["basis"]["unrecorded_prompts_before"], 0);
        });
    }

    #[test]
    fn open_refuses_while_an_eval_over_the_same_asks_is_still_open() {
        eval_bench(|ws| {
            let o = opened(ws);
            let e = open_eval(ws, Open::default()).unwrap_err();
            refused(e, "eval.already_open");
            crate::testing::settled(ws, &str_of(&o.out, "eval_id"), false);
            // Closed: a new Eval may open.
            assert_ne!(open_eval(ws, Open::default()).unwrap()["eval_id"], o.out["eval_id"]);
        });
    }

    /// An open Eval judges the work it was opened on. Once that work has
    /// changed, opening again starts a new Eval that names the one it replaces.
    #[test]
    fn open_supersedes_an_open_eval_over_earlier_work() {
        eval_bench(|ws| {
            let o = opened(ws);
            let old = str_of(&o.out, "eval_id");
            std::fs::write(ws.root.join("src/uptime.js"), "export const uptime = () => 2;\n")
                .unwrap();
            let out = open_eval(ws, Open::default()).unwrap();
            let new = str_of(&out, "eval_id");
            assert_ne!(new, old);
            assert!(array_of(&brief_of(ws, &out), "limitations").iter().any(|l| str_of_value(l)
                == format!("supersedes {old}, left open over the same Asks and earlier work")));
            let opened = store::events(ws)
                .unwrap()
                .into_iter()
                .rfind(|e| e["type"] == "eval.opened")
                .unwrap();
            assert!(array_of(&opened, "links").contains(&json!({"rel": "supersedes", "id": old})));
            // The new one judges this work, so it is the one to continue.
            refused(open_eval(ws, Open::default()).unwrap_err(), "eval.already_open");
        });
    }

    #[test]
    fn open_on_a_dirty_tree_takes_the_worktree_and_counts_untracked_files() {
        eval_bench(|ws| {
            two_asks_and_work(ws);
            std::fs::write(ws.root.join("src/uptime.js"), "export const uptime = () => 2;\nmore\n")
                .unwrap();
            std::fs::write(ws.root.join("scratch.txt"), "a\nb\n").unwrap();
            let out = open_eval(ws, Open::default()).unwrap();
            assert_eq!(out["subject"]["kind"], "worktree");
            let brief = brief_of(ws, &out);
            let files = array_of(&brief["changes"], "files");
            let find = |name: &str| files.iter().find(|f| f["path"] == name).unwrap().clone();
            assert_eq!(
                find("scratch.txt"),
                json!({"path": "scratch.txt", "status": "added", "added": 2, "removed": 0})
            );
            // Worktree against the anchor: the base had no file.
            assert_eq!(find("src/uptime.js")["added"], 2);
            assert_eq!(find("src/uptime.js")["removed"], 0);
            assert!(
                array_of(&brief, "limitations")
                    .iter()
                    .any(|l| l == "subject is the uncommitted worktree")
            );
        });
    }

    /// An Eval on one unit of work covers that unit's open Asks, its
    /// follow-ups among them, anchors on that unit's own Seal (here none, so
    /// its root Ask's base), and names every other open Ask as work it does
    /// not judge.
    #[test]
    fn an_eval_on_one_unit_covers_its_asks_and_names_the_others() {
        eval_bench(|ws| {
            let base = head(&ws.root);
            let (a, b, sha) = two_asks_and_work(ws);
            let follow = |id: &str| vec![json!({"rel": "follows", "id": id})];
            let c = str_of(
                &crate::testing::confirm_ask_linked(ws, "Also the docs", follow(&a)),
                "ask_id",
            );
            // Another unit's Seal is not where this unit's change starts.
            store::append(ws, &json!({
                "schema": store::SCHEMA, "event_id": ids::new_id("evt"), "type": "seal.created",
                "time": sessions::now(), "id": "sel_other",
                "actor": {"kind": "human", "id": "local-user"}, "links": [],
                "data": {"basis": {"asks": ["ask_older"]}, "eval": null,
                         "subject": {"kind": "git_commit", "ref": sha}, "disposition": "accepted",
                         "authority": {"kind": "human", "id": "local-user",
                                       "authority": "interactive"},
                         "artifact": {"role": "seal_receipt", "path": "seals/sel_other.json",
                                      "bytes": 0, "sha256": "0".repeat(64)}}
            })).unwrap();

            let out = open_eval(ws, Open { ask: Some(&c), ..Default::default() }).unwrap();
            assert_eq!(out["basis"]["asks"], json!([a, c]), "the unit, from any Ask in it");
            assert_eq!(out["anchor"], json!({"kind": "ask_base", "ref": base, "seal_id": null}));
            let brief = brief_of(ws, &out);
            let others: Vec<String> =
                array_of(&brief, "elsewhere").iter().map(|x| str_of(x, "ask_id")).collect();
            assert_eq!(others, std::slice::from_ref(&b), "named, not judged");

            // A follow-up of the unit's Eval is in the unit too.
            let eval_id = str_of(&out, "eval_id");
            let d = str_of(
                &crate::testing::confirm_ask_linked(ws, "And the 503", follow(&eval_id)),
                "ask_id",
            );
            assert_eq!(crate::ask::unit_of(ws, &d).unwrap(), [a.clone(), c, d]);
            assert_eq!(crate::ask::unit_of(ws, &b).unwrap(), [b]);
        });
    }

    #[test]
    fn open_anchors_on_the_last_seal_when_one_exists() {
        eval_bench(|ws| {
            let (_a, _b, sha) = two_asks_and_work(ws);
            store::append(ws, &json!({
                "schema": store::SCHEMA, "event_id": ids::new_id("evt"), "type": "seal.created",
                "time": sessions::now(), "id": "sel_old",
                "actor": {"kind": "human", "id": "local-user"}, "links": [],
                "data": {"basis": {"asks": ["ask_older"]}, "eval": null,
                         "subject": {"kind": "git_commit", "ref": sha}, "disposition": "accepted",
                         "authority": {"kind": "human", "id": "local-user",
                                       "authority": "interactive"},
                         "artifact": {"role": "seal_receipt", "path": "seals/sel_old.json",
                                      "bytes": 0, "sha256": "0".repeat(64)}}
            })).unwrap();
            let sha2 = commit(
                &ws.root,
                &[("src/more.js", Some("export const more = 1;\n"))],
                "after seal",
            );
            let out = open_eval(ws, Open::default()).unwrap();
            assert_eq!(out["anchor"], json!({"kind": "seal", "ref": sha, "seal_id": "sel_old"}));
            assert_eq!(paths_of(&brief_of(ws, &out)["changes"]), ["src/more.js"]);

            crate::testing::settled(ws, &str_of(&out, "eval_id"), false);

            let explicit =
                open_eval(ws, Open { anchor: Some(&sha2), ..Default::default() }).unwrap();
            assert_eq!(explicit["anchor"]["kind"], "explicit");
            assert!(paths_of(&brief_of(ws, &explicit)["changes"]).is_empty());
            // The explicit one is still open.
            let e = open_eval(ws, Open { anchor: Some(&"0".repeat(40)), ..Default::default() })
                .unwrap_err();
            refused(e, "eval.already_open");

            crate::testing::settled(ws, &str_of(&explicit, "eval_id"), false);
            let e = open_eval(ws, Open { anchor: Some(&"0".repeat(40)), ..Default::default() })
                .unwrap_err();
            refused(e, "eval.anchor_missing");
        });
    }

    #[test]
    fn open_is_refused_when_git_disappears_after_the_ask() {
        // `compile` now requires Git, so reach the Eval guard the only way
        // left: the repository is removed.
        eval_bench(|ws| {
            crate::testing::confirm_ask(ws, "t", b"s\n", b"p\n");
            std::fs::remove_dir_all(ws.root.join(".git")).unwrap();
            refused(open_eval(ws, Open::default()).unwrap_err(), "eval.no_git");
        });
    }

    #[test]
    fn a_missing_anchor_needs_input() {
        eval_bench(|ws| {
            let e = anchor_of(ws, &[json!({"ask_id": "ask_old", "base_commit": null})], None, None)
                .unwrap_err();
            assert!(
                matches!(&e, EvalError::Needs(n) if n.reason.contains("eval.anchor_unknown")),
                "{e}"
            );
        });
    }

    #[test]
    fn an_explicit_worktree_subject_diffs_that_root() {
        eval_bench(|ws| {
            two_asks_and_work(ws);
            std::fs::write(ws.root.join("scratch.txt"), "a\n").unwrap();
            let root = ws.root.to_string_lossy().to_string();
            let out = open_eval(
                ws,
                Open {
                    subject_kind: Some("worktree"),
                    subject_ref: Some(&root),
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(paths_of(&brief_of(ws, &out)["changes"]).contains(&"scratch.txt".to_string()));
            assert_eq!(out["subject"]["kind"], "worktree");
            let e = open_eval(ws, Open { subject_kind: Some("worktree"), ..Default::default() })
                .unwrap_err();
            refused(e, "subject.kind");
        });
    }

    #[test]
    fn a_nested_project_eval_excludes_its_siblings() {
        with_config_home(|_| {
            let repo = crate::testing::repo();
            let anchor = commit(
                repo.path(),
                &[("test/app.py", Some("old\n")), ("sibling.py", Some("old\n"))],
                "change",
            );
            let ws = crate::testing::ws_for(&repo.path().join("test"));
            let before = subject_digest(&ws, "git_commit", &anchor).unwrap();
            let sibling = commit(repo.path(), &[("sibling.py", Some("new\n"))], "change");
            assert_eq!(subject_digest(&ws, "git_commit", &sibling).unwrap(), before);
            std::fs::write(repo.path().join("sibling.py"), "dirty sibling\n").unwrap();
            assert_eq!(default_subject(&ws).unwrap().0, "git_commit");
            std::fs::write(repo.path().join("test/app.py"), "new\n").unwrap();
            let subject = json!({"kind": "worktree", "ref": ws.root.to_string_lossy()});
            let changed = changes(&ws, &anchor, &subject).unwrap();
            assert_eq!(paths_of(&changed), ["app.py"]);
            assert_eq!(changed["total_added"], 1);
            assert_eq!(changed["total_removed"], 1);

            let project =
                commit(repo.path(), &[("test/app.py", Some("committed change\n"))], "change");
            assert_ne!(subject_digest(&ws, "git_commit", &project).unwrap(), before);
            let committed =
                changes(&ws, &anchor, &json!({"kind": "git_commit", "ref": project})).unwrap();
            assert_eq!(paths_of(&committed), ["app.py"]);
        });
    }

    #[test]
    fn a_rename_names_the_file_that_now_exists() {
        assert_eq!(rename_target("src/{old.rs => new.rs}"), "src/new.rs");
        assert_eq!(rename_target("old.rs => new.rs"), "new.rs");
        assert_eq!(rename_target("plain.rs"), "plain.rs");
    }

    /// A shell command someone saw run in session `s1`, as `observe tool`
    /// passes it.
    fn fact(ws: &Workspace, outcome: &str, command: &str) -> Option<Value> {
        record_fact(ws, "claude-code", "s1", outcome, command, "weft").unwrap()
    }

    #[test]
    fn a_shell_command_during_open_work_is_a_fact_about_the_subject() {
        eval_bench(|ws| {
            two_asks_and_work(ws);
            let ok = fact(ws, "succeeded", "npm test").unwrap();
            let (kind, reference) = default_subject(ws).unwrap();
            let sha = subject_digest(ws, &kind, &reference).unwrap();
            assert_eq!(ok["type"], "tool.fact");
            assert!(str_of(&ok, "id").starts_with("fct_"), "{}", ok["id"]);
            assert_eq!(
                ok["data"],
                json!({"command": "npm test", "outcome": "succeeded",
                       "subject": {"kind": kind, "ref": reference, "sha256": sha,
                                   "content": worktree_content(ws).unwrap()},
                       "host": "claude-code", "session_ref": "s1", "observed_by": "weft"})
            );
            let failed = fact(ws, "failed", "npm test").unwrap();
            assert_eq!(failed["data"]["outcome"], "failed");
            let facts =
                store::events(ws).unwrap().iter().filter(|e| e["type"] == "tool.fact").count();
            assert_eq!(facts, 2);
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
    }

    struct Facts {
        eval_id: String,
        fresh: String,
    }

    /// Work, an `npm test` that succeeded on it, then an edit and a second
    /// `npm test` (of `outcome`) on the edited tree, then an open Eval.
    fn with_facts(ws: &Workspace, outcome: &str) -> Facts {
        two_asks_and_work(ws);
        fact(ws, "succeeded", "npm test").unwrap();
        std::fs::write(ws.root.join("src/uptime.js"), "export const uptime = () => 2;\n").unwrap();
        let fresh = fact(ws, outcome, "npm test").unwrap();
        let out = open_eval(ws, Open::default()).unwrap();
        Facts { eval_id: str_of(&out, "eval_id"), fresh: str_of(&fresh, "id") }
    }

    #[test]
    fn the_brief_lists_the_facts_about_the_subject_and_counts_the_rest() {
        eval_bench(|ws| {
            let f = with_facts(ws, "failed");
            let brief: Value = serde_json::from_slice(
                &std::fs::read(ws.rf_dir().join(format!("evals/{}/brief.json", f.eval_id)))
                    .unwrap(),
            )
            .unwrap();
            let facts: Vec<(String, String, Value)> = array_of(&brief, "facts")
                .iter()
                .map(|x| (str_of(x, "id"), str_of(x, "outcome"), x["fresh"].clone()))
                .collect();
            assert_eq!(facts, [(f.fresh.clone(), "failed".to_string(), json!(true))]);
            assert_eq!(array_of(&brief, "facts")[0]["command"], "npm test");
            assert!(array_of(&brief, "facts")[0]["time"].is_string());
            let says = |b: &Value, text: &str| {
                array_of(b, "limitations").iter().any(|l| str_of_value(l) == text)
            };
            assert!(!says(
                &brief,
                "no tool facts were recorded; the result of a command cannot be judged"
            ));
            assert!(says(
                &brief,
                "1 of the recorded commands ran against other work than the subject and are not listed"
            ));
        });
        eval_bench(|ws| {
            let o = opened(ws);
            assert!(array_of(&brief_of(ws, &o.out), "limitations").iter().any(|l| str_of_value(l)
                == "no tool facts were recorded; the result of a command cannot be judged"));
            assert_eq!(brief_of(ws, &o.out)["facts"], json!([]));
        });
    }

    /// Tests pass, then the agent commits exactly what it tested: the fact
    /// still describes the subject. Committing something else makes it stale.
    #[test]
    fn a_fact_stays_fresh_when_the_tested_work_is_committed_unchanged() {
        let fresh_after = |extra: &[(&str, Option<&str>)]| {
            eval_bench(|ws| {
                two_asks_and_work(ws);
                std::fs::write(ws.root.join("src/uptime.js"), "export const uptime = () => 2;\n")
                    .unwrap();
                let tested = fact(ws, "succeeded", "npm test").unwrap();
                assert_eq!(tested["data"]["subject"]["kind"], "worktree");
                commit(&ws.root, extra, "tested work");
                let out = open_eval(ws, Open::default()).unwrap();
                assert_eq!(out["subject"]["kind"], "git_commit");
                array_of(&brief_of(ws, &out), "facts").iter().any(|x| x["id"] == tested["id"])
            })
        };
        assert!(fresh_after(&[]));
        assert!(!fresh_after(&[("src/uptime.js", Some("changed after\n"))]));
    }

    fn agents(v: Value) -> Open<'static> {
        Open { agents: Some(v), ..Open::default() }
    }

    #[test]
    fn open_records_the_agents_it_was_asked_for_and_refuses_what_is_not_a_role() {
        eval_bench(|ws| {
            two_asks_and_work(ws);
            let asked = json!({"map": {"model": "claude-sonnet-5", "effort": "low"},
                               "confirm": {"effort": "high"}});
            let out = open_eval(ws, agents(asked.clone())).unwrap();
            assert_eq!(out["agents"], asked);
            assert_eq!(brief_of(ws, &out)["agents"], asked);
        });
        eval_bench(|ws| {
            two_asks_and_work(ws);
            // Earlier names land on their roles now; a role that no longer
            // runs is dropped, and a current name wins.
            let earlier = json!({"context": {"model": "a"}, "trace": {"effort": "low"},
                                 "adversary": {"model": "b"}, "confirm": {"model": "c"}});
            let out = open_eval(ws, agents(earlier)).unwrap();
            assert_eq!(out["agents"], json!({"map": {"effort": "low"}, "confirm": {"model": "c"}}));
        });
        eval_bench(|ws| {
            two_asks_and_work(ws);
            let out = open_eval(ws, Open::default()).unwrap();
            assert_eq!(brief_of(ws, &out)["agents"], json!({}));
        });
        eval_bench(|ws| {
            two_asks_and_work(ws);
            refused(
                open_eval(ws, agents(json!({"judge": {"model": "m"}}))).unwrap_err(),
                "eval.agents",
            );
            refused(
                open_eval(ws, agents(json!({"drift": {"temperature": "1"}}))).unwrap_err(),
                "eval.agents",
            );
            refused(
                open_eval(ws, agents(json!({"drift": {"model": ""}}))).unwrap_err(),
                "eval.agents",
            );
        });
    }

    fn patch_of(ws: &Workspace, out: &Value) -> (Value, Vec<u8>) {
        let reference = brief_of(ws, out)["changes_patch"].clone();
        let bytes = std::fs::read(ws.rf_dir().join(str_of(&reference, "path"))).unwrap();
        (reference, bytes)
    }

    #[test]
    fn open_writes_the_exact_diff_beside_the_brief() {
        eval_bench(|ws| {
            let o = opened(ws);
            let (reference, bytes) = patch_of(ws, &o.out);
            assert_eq!(reference["role"], "changes_patch");
            assert_eq!(str_of(&reference, "sha256"), crate::digest::sha256_bytes(&bytes));
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.contains("diff --git a/src/uptime.js b/src/uptime.js"), "{text}");
            assert!(text.contains("+export const uptime = () => 1;"), "{text}");
            assert_eq!(
                o.out["changes_patch"],
                json!(ws.rf_dir().join(str_of(&reference, "path")).to_string_lossy())
            );
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
        eval_bench(|ws| {
            two_asks_and_work(ws);
            std::fs::write(ws.root.join("src/uptime.js"), "export const uptime = () => 2;\n")
                .unwrap();
            std::fs::write(ws.root.join("fresh.txt"), "new file\n").unwrap();
            let out = open_eval(ws, Open::default()).unwrap();
            assert_eq!(out["subject"]["kind"], "worktree");
            let text = String::from_utf8(patch_of(ws, &out).1).unwrap();
            assert!(text.contains("+export const uptime = () => 2;"), "{text}");
            assert!(text.contains("b/fresh.txt") && text.contains("+new file"), "{text}");
        });
    }

    #[test]
    fn a_patch_over_the_limit_is_cut_on_a_whole_file() {
        eval_bench(|ws| {
            crate::testing::confirm_ask(ws, "big", b"s\n", b"p\n");
            let big = "x".repeat(80) + "\n";
            commit(
                &ws.root,
                &[("a.txt", Some("small\n")), ("b.txt", Some(&big.repeat(14_000)))],
                "big",
            );
            let out = open_eval(ws, Open::default()).unwrap();
            let text = String::from_utf8(patch_of(ws, &out).1).unwrap();
            assert!(text.contains("b/a.txt") && !text.contains("b/b.txt"), "cut before b.txt");
            assert!(array_of(&brief_of(ws, &out), "limitations").iter().any(|l| str_of_value(l)
                == "the patch is cut at 1 MiB; read the remaining paths with git"));
        });
    }

    /// A voided Eval stays in the record and nothing builds on it: not the next
    /// Eval's carry-over, not its `follows`, not `latest_for` (so not a Seal).
    #[test]
    fn a_voided_eval_is_kept_and_nothing_builds_on_it() {
        eval_bench(|ws| {
            let o = opened(ws);
            let first = str_of(&o.out, "eval_id");
            crate::testing::settled(ws, &first, false);
            let ask_ids = [o.a.clone(), o.b.clone()];
            assert_eq!(str_of(&latest_for(ws, &ask_ids).unwrap().unwrap(), "eval_id"), first);

            refused(void_eval(ws, &first, "  ", None).unwrap_err(), "eval.void_reason");
            let agent = json!({"kind": "agent", "id": "judge", "authority": "interactive"});
            refused(
                void_eval(ws, &first, "scripted judges", Some(&agent)).unwrap_err(),
                "eval.void_needs_person",
            );
            refused(void_eval(ws, "evl_nowhere", "why", None).unwrap_err(), "eval.missing");
            let out = void_eval(ws, &first, "the judges were scripts", None).unwrap();
            assert_eq!(out["state"], "voided");
            refused(void_eval(ws, &first, "again", None).unwrap_err(), "eval.already_voided");
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());

            let listed = list_records(ws).unwrap();
            assert_eq!(listed[0]["state"], "voided");
            assert_eq!(listed[0]["voided"]["reason"], "the judges were scripts");
            assert!(latest_for(ws, &ask_ids).unwrap().is_none(), "a Seal finds no Eval");
            let shown = show_eval(ws, &first).unwrap();
            assert!(shown.starts_with("> Voided ") && shown.contains("the judges were scripts"));

            let second = open_eval(ws, Open::default()).unwrap();
            assert_eq!(brief_of(ws, &second)["previous_evals"], json!([]), "not carried over");
            let id2 = str_of(&second, "eval_id");
            refused(show_eval(ws, &id2).unwrap_err(), "eval.not_completed");
            crate::testing::settled(ws, &id2, false);
            assert_eq!(str_of(&latest_for(ws, &ask_ids).unwrap().unwrap(), "eval_id"), id2);
        });
    }

    /// A voided Eval left open no longer holds the Asks: a new Eval opens over
    /// the same work instead of continuing it.
    #[test]
    fn a_voided_open_eval_is_not_continued() {
        eval_bench(|ws| {
            let o = opened(ws);
            let first = str_of(&o.out, "eval_id");
            refused(open_eval(ws, Open::default()).unwrap_err(), "eval.already_open");
            void_eval(ws, &first, "opened by mistake", None).unwrap();
            assert_ne!(str_of(&open_eval(ws, Open::default()).unwrap(), "eval_id"), first);
        });
    }

    /// The brief names who observed a submission, and says so
    /// when nothing ran `ringframe observe` at all.
    #[test]
    fn a_brief_names_who_observed_and_says_when_nothing_was() {
        let limits = |ws: &Workspace| {
            let out = open_eval(ws, Open::default()).unwrap();
            array_of(&brief_of(ws, &out), "limitations")
                .iter()
                .map(str_of_value)
                .collect::<Vec<_>>()
        };
        eval_bench(|ws| {
            two_asks_and_work(ws);
            let got = limits(ws);
            assert!(got.contains(&NOTHING_OBSERVED.to_string()), "{got:?}");
            assert!(!got.iter().any(|l| l.starts_with("submission observed by")), "{got:?}");
        });
        eval_bench(|ws| {
            two_asks_and_work(ws);
            crate::observe::prompt(ws, "weft", "claude-code", "s1", "Skip the cache.\n", None)
                .unwrap();
            let got = limits(ws);
            assert!(got.contains(&"submission observed by weft".to_string()), "{got:?}");
            assert!(!got.contains(&NOTHING_OBSERVED.to_string()), "{got:?}");
        });
    }

    #[test]
    fn nothing_else_is_a_fact() {
        eval_bench(|ws| {
            assert_eq!(fact(ws, "succeeded", "npm test"), None, "no Ask is open");
            crate::testing::confirm_ask(ws, "t", b"s\n", b"p\n");
            assert_eq!(fact(ws, "succeeded", "ringframe eval open"), None);
            assert_eq!(fact(ws, "succeeded", "  ringframe ask list --json"), None);
            assert_eq!(fact(ws, "succeeded", ""), None);
            // An outcome that is neither of the two is nothing.
            assert_eq!(fact(ws, "", "npm test"), None);
            assert!(store::events(ws).unwrap().iter().all(|e| e["type"] != "tool.fact"));
        });
    }
}
