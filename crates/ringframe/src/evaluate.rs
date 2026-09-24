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
//! and what it counted as are recorded, so a reader — or Weft — sees the
//! decision instead of having to repeat it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::store::LedgerError;
use crate::workspace::Workspace;
use crate::{digest, ids, schema, sessions, store};

pub const BRIEF_SCHEMA: &str = "ringframe.eval-brief/1";
pub const INTENT_SCHEMA: &str = "ringframe.eval-intent/1";
pub const JUDGEMENT_SCHEMA: &str = "ringframe.eval-judgement/1";
pub const RECORD_SCHEMA: &str = "ringframe.eval/1";
pub const KINDS: [&str; 2] = ["git_commit", "worktree"];
pub const ITEM_STATUS: [&str; 3] = ["active", "revised", "withdrawn"];
pub const VOTES: [&str; 3] = ["yes", "no", "unknown"];
pub const CLASSIFICATIONS: [&str; 3] = ["required", "consequence", "unexplained"];
pub const INDEPENDENCE: [&str; 2] = ["sub_agent", "shared_context"];
pub const MIN_JUDGES: usize = 3;

pub use crate::ask::{AskError as EvalError, NeedsInput};

fn ledger(code: &str, detail: impl Into<String>) -> EvalError {
    EvalError::Ledger(LedgerError::new_public(code, detail))
}

fn need(ok: bool, code: &str, message: impl Into<String>) -> Result<(), EvalError> {
    if ok { Ok(()) } else { Err(ledger(code, message)) }
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn array_of<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// Round to two decimal places, ties to even. The confidence goes into a
/// published record, so it has to land on the same number every time.
fn round2(x: f64) -> f64 {
    (x * 100.0).round_ties_even() / 100.0
}

fn git(root: &Path, args: &[&str]) -> Result<String, EvalError> {
    Ok(String::from_utf8_lossy(&git_bytes(root, args, &[0])?).to_string())
}

/// Git's output as it printed it. `ok` lists the exit codes that are not
/// failures: `diff --no-index` exits 1 when the files differ.
fn git_bytes(root: &Path, args: &[&str], ok: &[i32]) -> Result<Vec<u8>, EvalError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| ledger("eval.no_git", format!("git: {e}")))?;
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

fn anchor_of(ws: &Workspace, asks: &[Value], explicit: Option<&str>) -> Result<Value, EvalError> {
    if let Some(reference) = explicit.filter(|r| !r.is_empty()) {
        return Ok(json!({"kind": "explicit", "ref": reference, "seal_id": null}));
    }
    let seals: Vec<Value> =
        store::events(ws)?.into_iter().filter(|e| e["type"] == "seal.created").collect();
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

fn count_lines(p: &Path) -> u64 {
    std::fs::read(p).map(|b| b.iter().filter(|c| **c == b'\n').count() as u64).unwrap_or(0)
}

/// Files changed between the anchor and the subject, with line counts. Never
/// their content.
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

const ROLES: [&str; 5] = ["context", "intent", "coverage", "drift", "adversary"];

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

/// The latest completed Eval whose basis shares an Ask with this one: the
/// loop's memory.
fn previous_record(
    ws: &Workspace,
    eval_id: &str,
    ask_ids: &[String],
) -> Result<Option<Value>, EvalError> {
    let wanted: BTreeSet<&String> = ask_ids.iter().collect();
    let found = list_records(ws)?.into_iter().rfind(|r| {
        r["state"] == "completed"
            && str_of(r, "eval_id") != eval_id
            && array_of(&r["basis"], "asks")
                .iter()
                .any(|a| wanted.contains(&a.as_str().unwrap_or_default().to_string()))
    });
    match found {
        Some(r) => load_record(ws, &str_of(&r, "eval_id")),
        None => Ok(None),
    }
}

fn event(
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
}

/// Write the facts-only brief over every open Ask and append `eval.opened`.
pub fn open_eval(ws: &Workspace, args: Open<'_>) -> Result<Value, EvalError> {
    let agents = args.agents.clone().unwrap_or_else(|| json!({}));
    validate_agents(&agents)?;
    // `ask` does not import `evaluate`.
    let asks = crate::ask::open_asks(ws)?;
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
    let mut dangling = list_records(ws)?.into_iter().filter(|r| {
        r["state"] == "opened"
            && array_of(&r["basis"], "asks").iter().map(str_of_value).collect::<Vec<_>>() == ask_ids
    });
    if let Some(open) = dangling.next_back() {
        return Err(ledger(
            "eval.already_open",
            format!(
                "{} is open over the same Asks and not closed; close it or continue with it",
                str_of(&open, "eval_id")
            ),
        ));
    }
    let anchor = anchor_of(ws, &asks, args.anchor)?;
    let anchor_ref = str_of(&anchor, "ref");
    git(&ws.root, &["rev-parse", "--verify", "--quiet", &format!("{anchor_ref}^{{commit}}")])
        .map_err(|_| {
            ledger("eval.anchor_missing", format!("{anchor_ref} is not a commit in this workspace"))
        })?;
    let subject =
        json!({"kind": kind, "ref": reference, "sha256": subject_digest(ws, &kind, &reference)?});
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
    // What the harness saw the agent run, in order. Only a fact about this
    // exact subject can say anything about the work being judged. A fact
    // records the worktree's content, and a clean commit at HEAD holds exactly
    // that, so tests run before the commit still describe it.
    let content = match kind.as_str() {
        "worktree" => Some(str_of(&subject, "sha256")),
        "git_commit" if default_subject(ws)? == (kind.clone(), reference.clone()) => {
            Some(worktree_content(ws)?)
        }
        _ => None,
    };
    let since = anchor_time(ws, &anchor);
    let facts: Vec<Value> = store::events(ws)?
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
        .collect();
    if facts.is_empty() {
        limitations
            .push("no tool facts were recorded; the result of a command cannot be judged".into());
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
    let brief = json!({
        "schema": BRIEF_SCHEMA, "eval_id": eval_id, "time": sessions::now(),
        "workspace": ws.describe(), "anchor": anchor, "subject": subject,
        "asks": asks.iter().enumerate().map(|(i, a)| json!({
            "ask_id": a["ask_id"], "order": i + 1, "title": a["title"],
            "prompt_path": a["prompt"]["path"], "compiled": a["time"],
            "confirmed": a["confirmed_at"], "submission": a["submission"], "links": a["links"],
            "unrecorded_prompts_after": counts[i],
        })).collect::<Vec<_>>(),
        "changes": changes(ws, &anchor_ref, &subject)?,
        "unrecorded_prompts_before": before,
        "previous_evals": previous, "facts": facts, "limitations": limitations,
        "agents": agents, "changes_patch": changes_patch,
    });
    let mut bytes = store::canonical(&brief);
    bytes.push(b'\n');
    let reference =
        store::publish(ws, &format!("evals/{eval_id}/brief.json"), &bytes, "eval_brief")?;
    let data = json!({
        "brief": reference, "changes_patch": changes_patch, "anchor": anchor, "subject": subject,
        "basis": {
            "asks": ask_ids,
            "unrecorded_prompts": before + counts.iter().sum::<usize>(),
            "unrecorded_prompts_before": before,
        },
    });
    let links: Vec<Value> =
        asks.iter().map(|a| json!({"rel": "evaluates", "id": a["ask_id"]})).collect();
    store::append(ws, &event("eval.opened", &eval_id, args.actor.as_ref(), data.clone(), links)?)?;
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

// ---- judgements ------------------------------------------------------------

fn check_judge(j: &Value, code: &str, where_: &str) -> Result<(), EvalError> {
    need(
        j.get("host").is_some_and(Value::is_string) && j.get("angle").is_some_and(Value::is_string),
        code,
        format!("{where_}: judge needs host and angle"),
    )?;
    need(
        INDEPENDENCE.contains(&str_of(j, "independence").as_str()),
        code,
        format!("{where_}: judge.independence must be one of {}", tuple_of(&INDEPENDENCE)),
    )
}

/// Formats a fixed set of allowed values as `('a', 'b', 'c')`; the messages
/// are part of the surface.
fn tuple_of(items: &[&str]) -> String {
    format!("({})", items.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", "))
}

fn list_of(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", "))
}

pub fn validate_intent(
    intent: &Value,
    brief_sha256: &str,
    ask_ids: &[String],
) -> Result<(), EvalError> {
    let code = "eval.intent";
    need(
        intent.is_object() && str_of(intent, "schema") == INTENT_SCHEMA,
        code,
        format!("schema must be {INTENT_SCHEMA}"),
    )?;
    need(
        str_of(intent, "brief_sha256") == brief_sha256,
        "eval.brief_mismatch",
        "intent.brief_sha256 is not this Eval's brief",
    )?;
    check_judge(intent.get("judge").unwrap_or(&Value::Null), code, "intent")?;
    let items = intent.get("items").and_then(Value::as_array);
    need(items.is_some_and(|i| !i.is_empty()), code, "items must be a non-empty list")?;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (i, it) in items.expect("checked").iter().enumerate() {
        let id = str_of(it, "id");
        need(
            it.is_object() && it.get("id").is_some_and(Value::is_string) && !seen.contains(&id),
            code,
            format!("items[{i}].id missing or duplicate"),
        )?;
        seen.insert(id);
        need(!str_of(it, "text").trim().is_empty(), code, format!("items[{i}].text missing"))?;
        need(
            ask_ids.contains(&str_of(it, "ask_id")),
            code,
            format!("items[{i}].ask_id is not an open Ask of this Eval"),
        )?;
        need(
            ITEM_STATUS.contains(&str_of(it, "status").as_str()),
            code,
            format!("items[{i}].status must be one of {}", tuple_of(&ITEM_STATUS)),
        )?;
        need(
            it.get("check").is_none_or(|c| c.as_str().is_some_and(|c| !c.trim().is_empty())),
            code,
            format!("items[{i}].check must name the command whose result the item is about"),
        )?;
    }
    Ok(())
}

pub fn validate_judgement(
    j: &Value,
    brief_sha256: &str,
    intent_sha256: &str,
    items: &[Value],
    where_: &str,
    changed_paths: &[String],
    fact_ids: &[String],
) -> Result<(), EvalError> {
    let code = "eval.judgement";
    need(
        j.is_object() && str_of(j, "schema") == JUDGEMENT_SCHEMA,
        code,
        format!("{where_}: schema must be {JUDGEMENT_SCHEMA}"),
    )?;
    need(
        str_of(j, "brief_sha256") == brief_sha256,
        "eval.brief_mismatch",
        format!("{where_}: brief_sha256 is not this Eval's brief"),
    )?;
    let declared = j.get("intent_sha256").cloned().unwrap_or(Value::Null);
    need(
        declared.is_null() || declared.as_str() == Some(intent_sha256),
        "eval.brief_mismatch",
        format!("{where_}: intent_sha256 is not this Eval's intent"),
    )?;
    check_judge(j.get("judge").unwrap_or(&Value::Null), code, where_)?;
    let all: BTreeSet<String> = items.iter().map(|it| str_of(it, "id")).collect();
    let active: BTreeSet<String> = items
        .iter()
        .filter(|it| str_of(it, "status") == "active")
        .map(|it| str_of(it, "id"))
        .collect();
    let votes = j.get("votes").and_then(Value::as_array);
    need(votes.is_some(), code, format!("{where_}: votes must be a list"))?;
    let mut voted: BTreeSet<String> = BTreeSet::new();
    for (i, v) in votes.expect("checked").iter().enumerate() {
        need(
            v.is_object() && all.contains(&str_of(v, "item")),
            code,
            format!(
                "{where_}: votes[{i}].item {:?} is not an intent item; use one of {}",
                str_of(v, "item"),
                all.iter().cloned().collect::<Vec<_>>().join(", ")
            ),
        )?;
        need(
            VOTES.contains(&str_of(v, "vote").as_str()),
            code,
            format!("{where_}: votes[{i}].vote must be one of {}", tuple_of(&VOTES)),
        )?;
        need(
            v.get("facts_cited").is_none_or(Value::is_array),
            code,
            format!("{where_}: votes[{i}].facts_cited must be a list"),
        )?;
        for id in array_of(v, "facts_cited").iter().map(str_of_value) {
            need(
                fact_ids.contains(&id),
                "eval.fact_unknown",
                format!(
                    "{where_}: votes[{i}] cites {id}, which is not a fact in this Eval's brief"
                ),
            )?;
        }
        voted.insert(str_of(v, "item"));
    }
    let unvoted: Vec<String> = active.difference(&voted).cloned().collect();
    need(
        unvoted.is_empty(),
        code,
        format!("{where_}: no vote for active items {}", list_of(&unvoted)),
    )?;
    let mut classified: BTreeSet<String> = BTreeSet::new();
    for (i, d) in array_of(j, "drift").iter().enumerate() {
        need(!str_of(d, "path").is_empty(), code, format!("{where_}: drift[{i}].path missing"))?;
        need(
            CLASSIFICATIONS.contains(&str_of(d, "classification").as_str()),
            code,
            format!(
                "{where_}: drift[{i}].classification must be one of {}",
                tuple_of(&CLASSIFICATIONS)
            ),
        )?;
        classified.insert(str_of(d, "path"));
    }
    let missing: Vec<String> =
        changed_paths.iter().filter(|p| !classified.contains(*p)).cloned().collect();
    need(
        missing.is_empty(),
        code,
        format!("{where_}: no drift classification for changed paths {}", list_of(&missing)),
    )?;
    for k in ["basis_notes", "commands_run"] {
        need(j.get(k).is_none_or(Value::is_array), code, format!("{where_}: {k} must be a list"))?;
    }
    Ok(())
}

/// The facts a vote cites that describe the subject being judged.
fn fresh_cited<'a>(vote: &Value, facts: &'a [Value]) -> impl Iterator<Item = &'a Value> {
    let ids: Vec<String> = array_of(vote, "facts_cited").iter().map(str_of_value).collect();
    facts.iter().filter(move |f| f["fresh"] == true && ids.contains(&str_of(f, "id")))
}

/// Whether a vote rests on something a reader can go and check.
///
/// A command the judge ran or a basis note it recorded counts for every vote
/// it cast; naming one of the changed paths, or citing a fact about this
/// subject, counts for that vote alone. Anything else is an assertion.
fn cited(judgement: &Value, vote: &Value, changed_paths: &[String], facts: &[Value]) -> bool {
    if !array_of(judgement, "commands_run").is_empty()
        || !array_of(judgement, "basis_notes").is_empty()
        || fresh_cited(vote, facts).next().is_some()
    {
        return true;
    }
    let reason = str_of(vote, "reason");
    changed_paths.iter().any(|p| !p.is_empty() && reason.contains(p.as_str()))
}

/// A `yes` on an item about a command's result stands only on the harness's
/// own record of that command succeeding on this subject — never on a judge
/// saying it read or traced the code.
fn backed_by_fact(vote: &Value, check: &str, facts: &[Value]) -> bool {
    fresh_cited(vote, facts)
        .any(|f| f["outcome"] == "succeeded" && str_of(f, "command").contains(check))
}

fn majority(values: &[String], tie: &str) -> (String, f64) {
    let mut counts: BTreeMap<&String, usize> = BTreeMap::new();
    for v in values {
        *counts.entry(v).or_default() += 1;
    }
    let best = counts.values().copied().max().unwrap_or(0);
    let winners: Vec<&&String> =
        counts.iter().filter(|(_, n)| **n == best).map(|(k, _)| k).collect();
    let name = if winners.len() == 1 { (*winners[0]).clone() } else { tie.to_string() };
    (name, best as f64 / values.len() as f64)
}

fn aggregate(
    items: &[Value],
    judgements: &[Value],
    changed_paths: &[String],
    facts: &[Value],
) -> Value {
    let active: Vec<&Value> = items.iter().filter(|it| str_of(it, "status") == "active").collect();
    let mut table: Vec<Value> = Vec::new();
    for it in &active {
        let mut votes: Vec<Value> = Vec::new();
        for j in judgements {
            let cast = array_of(j, "votes")
                .iter()
                .find(|v| str_of(v, "item") == str_of(it, "id"))
                .expect("every active item is voted on");
            let as_cast = str_of(cast, "vote");
            let stands = match it.get("check").and_then(Value::as_str) {
                Some(check) => backed_by_fact(cast, check, facts),
                None => cited(j, cast, changed_paths, facts),
            };
            let counted =
                if as_cast != "yes" || stands { as_cast.clone() } else { "unknown".to_string() };
            let mut row = json!({
                "angle": j["judge"]["angle"], "vote": as_cast,
                "reason": cast.get("reason").cloned().unwrap_or_else(|| json!("")),
                "counted_as": counted,
            });
            if counted != as_cast {
                row["uncited"] = json!(true);
            }
            votes.push(row);
        }
        let counted: Vec<String> = votes.iter().map(|v| str_of(v, "counted_as")).collect();
        let (maj, agr) = majority(&counted, "unknown");
        let mut row = (*it).clone();
        row["majority"] = json!(maj);
        row["agreement"] = json!(round2(agr));
        row["votes"] = json!(votes);
        table.push(row);
    }
    let mut by_path: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for j in judgements {
        for d in array_of(j, "drift") {
            by_path.entry(str_of(d, "path")).or_default().push(json!({
                "angle": j["judge"]["angle"], "classification": d["classification"],
                "finding": d.get("finding").cloned().unwrap_or_else(|| json!("")),
            }));
        }
    }
    let mut paths: Vec<Value> = Vec::new();
    for (path, findings) in &by_path {
        // Every judge classifies every changed path (validated); a judge
        // silent on a path outside the brief counts as `required` there, so
        // one loud judge cannot make a finding unanimous. Agreement is always
        // over every judge.
        let mut votes: Vec<String> = findings.iter().map(|f| str_of(f, "classification")).collect();
        votes
            .extend(std::iter::repeat_n("required".to_string(), judgements.len() - findings.len()));
        let (maj, agr) = majority(&votes, "unexplained");
        paths.push(json!({
            "path": path, "classification": maj, "agreement": round2(agr),
            "mentions": findings.len(), "findings": findings,
        }));
    }
    let unmentioned: Vec<&String> =
        changed_paths.iter().filter(|p| !by_path.contains_key(*p)).collect();
    let commission: Vec<&Value> =
        paths.iter().filter(|p| p["classification"] == "unexplained").collect();
    let omission: Vec<String> =
        table.iter().filter(|it| it["majority"] != "yes").map(|it| str_of(it, "id")).collect();
    let verdict = if active.is_empty() {
        "incomplete"
    } else if table.iter().any(|it| it["majority"] == "no") || !commission.is_empty() {
        "drifted"
    } else if table.iter().all(|it| it["majority"] == "yes") {
        "aligned"
    } else {
        "incomplete"
    };
    // Every active item decides; a path decides when its majority is a finding
    // or when any judge dissented from `required`.
    let mut deciding: Vec<f64> = table.iter().filter_map(|it| it["agreement"].as_f64()).collect();
    deciding.extend(
        paths
            .iter()
            .filter(|p| p["classification"] != "required" || p["agreement"].as_f64() < Some(1.0))
            .filter_map(|p| p["agreement"].as_f64()),
    );
    let confidence = deciding.iter().copied().fold(f64::INFINITY, f64::min);
    json!({
        "verdict": verdict,
        "confidence": if deciding.is_empty() { 0.0 } else { round2(confidence) },
        "items": table,
        "drift": {
            "omission": omission,
            "commission": commission.iter().map(|p| json!({
                "path": p["path"], "classification": p["classification"],
                "agreement": p["agreement"],
            })).collect::<Vec<_>>(),
            "paths": paths, "unmentioned": unmentioned,
        },
    })
}

fn normalized(text: &str) -> String {
    text.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tokens(text: &str) -> BTreeSet<String> {
    let lower = text.to_lowercase();
    lower
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| w.len() > 2)
        .map(str::to_string)
        .collect()
}

/// The previous item this one continues: same id and Ask, else same normalized
/// text, else the best token overlap (Jaccard >= 0.5) within the same Ask.
/// Judges reword; the record says how each match was made.
fn matched<'a>(item: &Value, candidates: &'a [Value]) -> Option<&'a Value> {
    let same_ask: Vec<&Value> =
        candidates.iter().filter(|c| c.get("ask_id") == item.get("ask_id")).collect();
    if let Some(c) = same_ask.iter().find(|c| str_of(c, "id") == str_of(item, "id")) {
        return Some(c);
    }
    if let Some(c) = candidates
        .iter()
        .find(|c| normalized(&str_of(c, "text")) == normalized(&str_of(item, "text")))
    {
        return Some(c);
    }
    let a = tokens(&str_of(item, "text"));
    let mut best: Option<&Value> = None;
    let mut score = 0.0;
    for c in same_ask {
        let b = tokens(&str_of(c, "text"));
        let union = a.union(&b).count();
        let j = if union == 0 { 0.0 } else { a.intersection(&b).count() as f64 / union as f64 };
        if j > score {
            best = Some(c);
            score = j;
        }
    }
    if score >= 0.5 { best } else { None }
}

fn delta(current: &Value, previous: Option<&Value>) -> Value {
    let Some(previous) = previous else { return Value::Null };
    let prev_items = array_of(previous, "items");
    let (mut closed, mut opened, mut new) = (Vec::new(), Vec::new(), Vec::new());
    for it in array_of(current, "items") {
        let text = str_of(it, "text");
        match matched(it, prev_items) {
            None => {
                new.push(text.clone());
                if it["majority"] != "yes" {
                    opened.push(text);
                }
            }
            Some(before) => {
                if it["majority"] == "yes" && before["majority"] != "yes" {
                    closed.push(text);
                } else if it["majority"] != "yes" && before["majority"] == "yes" {
                    opened.push(text);
                }
            }
        }
    }
    let commission = |rec: &Value| -> BTreeSet<String> {
        array_of(&rec["drift"], "commission").iter().map(|c| str_of(c, "path")).collect()
    };
    let (before, after) = (commission(previous), commission(current));
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    json!({
        "closed": sorted(closed), "opened": sorted(opened), "new_items": sorted(new),
        "commission_removed": before.difference(&after).cloned().collect::<Vec<_>>(),
        "commission_added": after.difference(&before).cloned().collect::<Vec<_>>(),
        "matching": "items matched to the previous Eval by id within the same Ask, then normalized text, then token overlap >= 0.5 within the same Ask",
    })
}

/// What the working agent ran, as the harness's own post-tool hook saw it: the
/// command, whether it succeeded, and the subject it ran against. Output is
/// never read, and RingFrame decides nothing about what the command was for.
/// Only while some Ask is open, and never for RingFrame's own commands.
pub fn record_fact(
    ws: &Workspace,
    host: &str,
    payload: &Value,
) -> Result<Option<Value>, EvalError> {
    let outcome = match str_of(payload, "hook_event_name").as_str() {
        "PostToolUse" => "succeeded",
        "PostToolUseFailure" => "failed",
        _ => return Ok(None),
    };
    let command = str_of(&payload["tool_input"], "command");
    let first = command.split_whitespace().next();
    if str_of(payload, "tool_name") != "Bash" || first.is_none() || first == Some("ringframe") {
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
        "host": host, "session_ref": str_of(payload, "session_id"),
        "tool_use_id": str_of(payload, "tool_use_id"),
    });
    let actor = json!({"kind": "agent", "id": host});
    let ev = event("tool.fact", &ids::new_id("fct"), Some(&actor), data, Vec::new())?;
    store::append(ws, &ev)?;
    Ok(Some(ev))
}

/// Coverage, drift, adversary, then any other angle, so every item reads alike.
fn angle_rank(angle: &str) -> usize {
    ["coverage", "drift", "adversary"].iter().position(|a| *a == angle).unwrap_or(3)
}

/// What an Eval found, for a harness that never saw it. A view of the record,
/// never a second source: rendered only from the published record and brief,
/// so the same record always reads the same.
pub fn render_eval_md(record: &Value, brief: &Value) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let subject = &record["subject"];
    let sha = str_of(subject, "sha256");
    let _ = writeln!(
        out,
        "# Eval {}: {}, {} agreed\n",
        str_of(record, "eval_id"),
        str_of(record, "verdict"),
        record["confidence"]
    );
    let _ = writeln!(
        out,
        "Subject: {} {} · {}",
        str_of(subject, "kind"),
        &sha[..sha.len().min(12)],
        str_of(record, "time")
    );
    let asks = array_of(&record["basis"], "asks");
    let _ =
        writeln!(out, "Judged: {} open Ask{}", asks.len(), if asks.len() == 1 { "" } else { "s" });
    for id in asks {
        let id = str_of_value(id);
        let title = array_of(brief, "asks")
            .iter()
            .find(|a| str_of(a, "ask_id") == id)
            .map(|a| str_of(a, "title"))
            .unwrap_or_default();
        let _ = writeln!(out, "- {id} {title} — .fab7/rf/asks/{id}/prompt.txt");
    }
    for (heading, majority) in [("Not met", "no"), ("Unresolved", "unknown")] {
        let items: Vec<&Value> =
            array_of(record, "items").iter().filter(|it| it["majority"] == majority).collect();
        if items.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\n## {heading}");
        for it in items {
            let _ = writeln!(
                out,
                "- {} {} (from {})",
                str_of(it, "id"),
                str_of(it, "text"),
                str_of(it, "ask_id")
            );
            let mut votes: Vec<&Value> = array_of(it, "votes").iter().collect();
            votes.sort_by_key(|v| angle_rank(&str_of(v, "angle")));
            for v in votes {
                let counted = str_of(v, "counted_as");
                let cast = str_of(v, "vote");
                let note = if cast == counted {
                    String::new()
                } else {
                    format!(" (cast {cast}, uncited)")
                };
                let _ = writeln!(
                    out,
                    "  - {} {counted}{note}: {}",
                    str_of(v, "angle"),
                    str_of(v, "reason")
                );
            }
        }
    }
    let unexplained: Vec<&Value> = array_of(&record["drift"], "paths")
        .iter()
        .filter(|p| p["classification"] == "unexplained")
        .collect();
    if !unexplained.is_empty() {
        let _ = writeln!(out, "\n## Changed with no Ask to explain it");
        for p in unexplained {
            let _ = writeln!(out, "- {}", str_of(p, "path"));
            let mut findings: Vec<&Value> = array_of(p, "findings").iter().collect();
            findings.sort_by_key(|f| angle_rank(&str_of(f, "angle")));
            for f in findings {
                let _ = writeln!(out, "  - {}: {}", str_of(f, "angle"), str_of(f, "finding"));
            }
        }
    }
    let limitations = array_of(record, "limitations");
    if !limitations.is_empty() {
        let _ = writeln!(out, "\n## Limitations");
        for l in limitations {
            let _ = writeln!(out, "- {}", str_of_value(l));
        }
    }
    out
}

/// Validate the intent and the judgements, aggregate, append `eval.completed`.
/// Publish the change map a context agent wrote, so a debate in any harness
/// reads it by Eval id.
pub fn gather(
    ws: &Workspace,
    eval_id: &str,
    map: &[u8],
    host: &str,
    actor: Option<&Value>,
) -> Result<Value, EvalError> {
    let events = store::events(ws)?;
    let has = |t: &str| events.iter().any(|e| e["type"] == t && str_of(e, "id") == eval_id);
    need(
        has("eval.opened") && !has("eval.completed"),
        "eval.not_open",
        format!("{eval_id} is not an open Eval"),
    )?;
    need(
        !has("eval.gathered"),
        "eval.already_gathered",
        format!("{eval_id} already has a change map"),
    )?;
    need(
        std::str::from_utf8(map).is_ok_and(|t| !t.trim().is_empty()),
        "eval.context",
        "the change map must be non-empty UTF-8",
    )?;
    let context_map =
        store::publish(ws, &format!("evals/{eval_id}/context.md"), map, "context_map")?;
    let data = json!({"eval_id": eval_id, "context_map": context_map, "host": host});
    store::append(ws, &event("eval.gathered", eval_id, actor, data, Vec::new())?)?;
    Ok(json!({
        "eval_id": eval_id,
        "context_map": ws.rf_dir().join(str_of(&context_map, "path")).to_string_lossy(),
    }))
}

pub fn close_eval(
    ws: &Workspace,
    eval_id: &str,
    intent: &Value,
    judgements: &[Value],
    agents: Option<&Value>,
    actor: Option<&Value>,
) -> Result<Value, EvalError> {
    let agents = agents.cloned().unwrap_or_else(|| json!({}));
    validate_agents(&agents)?;
    let brief_path = ws.rf_dir().join(format!("evals/{eval_id}/brief.json"));
    let opened =
        store::events(ws)?.iter().any(|e| e["type"] == "eval.opened" && str_of(e, "id") == eval_id);
    if !brief_path.exists() || !opened {
        return Err(ledger("eval.missing", eval_id));
    }
    if ws.rf_dir().join(format!("evals/{eval_id}/record.json")).exists() {
        return Err(ledger("ledger.immutable", format!("evals/{eval_id}/record.json")));
    }
    let brief_sha = digest::sha256_file(&brief_path)?;
    let brief: Value = serde_json::from_slice(&std::fs::read(&brief_path)?)
        .map_err(|e| ledger("eval.missing", e.to_string()))?;
    let ask_ids: Vec<String> =
        array_of(&brief, "asks").iter().map(|a| str_of(a, "ask_id")).collect();
    need(
        judgements.len() >= MIN_JUDGES,
        "eval.too_few_judges",
        format!(
            "{} judgements; at least {MIN_JUDGES} independent angles are required",
            judgements.len()
        ),
    )?;
    validate_intent(intent, &brief_sha, &ask_ids)?;
    let mut intent_bytes = store::canonical(intent);
    intent_bytes.push(b'\n');
    let intent_sha = digest::sha256_bytes(&intent_bytes);
    let changed_paths: Vec<String> =
        array_of(&brief["changes"], "files").iter().map(|f| str_of(f, "path")).collect();
    let items = array_of(intent, "items");
    let facts = array_of(&brief, "facts");
    let fact_ids: Vec<String> = facts.iter().map(|f| str_of(f, "id")).collect();
    for (n, j) in judgements.iter().enumerate() {
        validate_judgement(
            j,
            &brief_sha,
            &intent_sha,
            items,
            &format!("the {} judgement (--judgement {})", str_of(&j["judge"], "angle"), n + 1),
            &changed_paths,
            &fact_ids,
        )?;
    }
    let subject = brief["subject"].clone();
    let now_digest = subject_digest(ws, &str_of(&subject, "kind"), &str_of(&subject, "ref"))?;
    let mut limitations = vec![
        format!(
            "the verdict is a judgement by {} sub-agents; agreement is its confidence, nothing here is certain",
            judgements.len()
        ),
        "RingFrame ran none of the project's commands; commands_run in a judgement is that judge's own report".to_string(),
    ];
    // The harness owns the fallback; the record says where a judge ran on
    // something other than what its role asked for.
    let judges = std::iter::once(&intent["judge"]).chain(judgements.iter().map(|j| &j["judge"]));
    for judge in judges {
        let angle = str_of(judge, "angle");
        let asked = str_of(&agents[angle.as_str()], "model");
        let reported = str_of(judge, "model");
        if !asked.is_empty() && !reported.to_lowercase().contains(&asked.to_lowercase()) {
            limitations
                .push(format!("the {angle} judge reported {reported}; its role asked for {asked}"));
        }
    }
    let mut agg = aggregate(items, judgements, &changed_paths, facts);
    if now_digest != str_of(&subject, "sha256") {
        agg["verdict"] = json!("incomplete");
        limitations.push("subject.changed: the subject changed between open and close".into());
    }
    if array_of(&agg, "items").is_empty() {
        limitations.push("no active intent item: nothing to judge".into());
    }
    if judgements.iter().any(|j| j["judge"]["independence"] == "shared_context") {
        limitations
            .push("some judges shared one context; their agreement overstates independence".into());
    }
    let before = brief["unrecorded_prompts_before"].as_u64().unwrap_or(0);
    let after: u64 = array_of(&brief, "asks")
        .iter()
        .filter_map(|a| a["unrecorded_prompts_after"].as_u64())
        .sum();
    let n_unrecorded = before + after;
    if n_unrecorded > 0 {
        limitations.push(format!(
            "{after} unrecorded prompts followed the open Asks, and {before} more came between \
             the anchor and the first of them; unexplained changes may follow either"
        ));
    }
    let intent_ref =
        store::publish(ws, &format!("evals/{eval_id}/intent.json"), &intent_bytes, "eval_intent")?;
    let mut j_refs = Vec::new();
    for (n, j) in judgements.iter().enumerate() {
        let mut bytes = store::canonical(j);
        bytes.push(b'\n');
        j_refs.push(store::publish(
            ws,
            &format!("evals/{eval_id}/judgement-{}.json", n + 1),
            &bytes,
            "eval_judgement",
        )?);
    }
    let previous = previous_record(ws, eval_id, &ask_ids)?;
    let gathered = store::events(ws)?
        .into_iter()
        .find(|e| e["type"] == "eval.gathered" && str_of(e, "id") == eval_id);
    let mut subject_at_close = subject.clone();
    subject_at_close["sha256_at_close"] = json!(now_digest);
    let record = json!({
        "schema": RECORD_SCHEMA, "eval_id": eval_id, "time": sessions::now(),
        "basis": {"asks": ask_ids, "anchor": brief["anchor"],
                  "unrecorded_prompts": n_unrecorded, "unrecorded_prompts_before": before},
        "subject": subject_at_close,
        "brief": {"path": format!("evals/{eval_id}/brief.json"), "sha256": brief_sha},
        "intent": {"path": intent_ref["path"], "sha256": intent_sha, "judge": intent["judge"],
                   "items": items.len(), "active": array_of(&agg, "items").len()},
        "judgements": j_refs.iter().zip(judgements).map(|(r, j)| json!({
            "path": r["path"], "sha256": r["sha256"], "judge": j["judge"]
        })).collect::<Vec<_>>(),
        "verdict": agg["verdict"], "confidence": agg["confidence"],
        "items": agg["items"], "drift": agg["drift"],
        "follows": previous.as_ref().map_or(Value::Null, |p| p["eval_id"].clone()),
        "delta": delta(&agg, previous.as_ref()), "limitations": limitations,
        "agents": agents,
    });
    let mut record = record;
    if let Some(g) = gathered {
        record["context_map"] = g["data"]["context_map"].clone();
    }
    let mut bytes = store::canonical(&record);
    bytes.push(b'\n');
    let reference =
        store::publish(ws, &format!("evals/{eval_id}/record.json"), &bytes, "eval_record")?;
    let eval_md = store::publish(
        ws,
        &format!("evals/{eval_id}/eval.md"),
        render_eval_md(&record, &brief).as_bytes(),
        "eval_md",
    )?;
    let data = json!({
        "basis": record["basis"], "subject": record["subject"], "verdict": record["verdict"],
        "confidence": record["confidence"],
        "items": {
            "active": array_of(&agg, "items").len(),
            "met": array_of(&agg, "items").iter().filter(|it| it["majority"] == "yes").count(),
            "omission": array_of(&agg["drift"], "omission").len(),
        },
        "commission": array_of(&agg["drift"], "commission").iter()
            .map(|c| c["path"].clone()).collect::<Vec<_>>(),
        "judges": judgements.iter().map(|j| j["judge"].clone()).collect::<Vec<_>>(),
        "intent": intent_ref, "judgements": j_refs, "artifact": reference, "eval_md": eval_md,
        "limitations": limitations,
    });
    let mut links: Vec<Value> = record["basis"]["asks"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|a| json!({"rel": "evaluates", "id": a}))
        .collect();
    if let Some(p) = &previous {
        links.push(json!({"rel": "supersedes", "id": p["eval_id"]}));
    }
    store::append(ws, &event("eval.completed", eval_id, actor, data, links)?)?;
    Ok(record)
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
    let mut out = Vec::new();
    for (_, brief, bytes) in briefs {
        let eval_id = str_of(&brief, "eval_id");
        let rec = load_record(ws, &eval_id)?;
        let field = |key: &str| rec.as_ref().map_or(Value::Null, |r| r[key].clone());
        out.push(json!({
            "eval_id": eval_id, "state": if rec.is_some() { "completed" } else { "opened" },
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
    use crate::testing::{
        CHANGED, commit, eval_bench, head, intent_doc, judgement, judgement_over, opened,
        two_asks_and_work, with_config_home,
    };

    fn close(
        ws: &Workspace,
        eval_id: &str,
        intent: &Value,
        js: &[Value],
    ) -> Result<Value, EvalError> {
        close_eval(ws, eval_id, intent, js, None, None)
    }

    fn angles() -> [&'static str; 3] {
        ["coverage", "drift", "adversary"]
    }

    fn one_item(a: &str) -> Value {
        json!([{"id": "i1", "text": "Expose an uptime endpoint", "ask_id": a, "status": "active"}])
    }

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
                crate::sessions::capture(
                    ws,
                    "claude-code",
                    &json!({"session_id": "s9", "prompt": text}),
                    None,
                )
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
                    &str_of(&sub, "sha256")
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
            let items = one_item(&o.a);
            let js: Vec<Value> =
                angles().iter().map(|ang| judgement(&o.brief_sha, ang, &[("i1", "yes")])).collect();
            close(ws, &str_of(&o.out, "eval_id"), &intent_doc(&o.brief_sha, items), &js).unwrap();
            crate::seal::create(ws, "accepted", None, None, None).unwrap();

            // Two plain prompts, then a commit, then the next Ask.
            for text in ["also update the docs", "and drop the cache bit"] {
                crate::sessions::capture(
                    ws,
                    "claude-code",
                    &json!({"session_id": "s9", "prompt": text}),
                    None,
                )
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
            crate::sessions::capture(
                ws,
                "claude-code",
                &json!({"session_id": "s9", "prompt": "a plain prompt"}),
                None,
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
            let items = one_item(&o.a);
            let js: Vec<Value> =
                angles().iter().map(|ang| judgement(&o.brief_sha, ang, &[("i1", "yes")])).collect();
            close(ws, &str_of(&o.out, "eval_id"), &intent_doc(&o.brief_sha, items), &js).unwrap();
            // Closed: a new Eval may open.
            assert_ne!(open_eval(ws, Open::default()).unwrap()["eval_id"], o.out["eval_id"]);
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

    #[test]
    fn open_anchors_on_the_last_seal_when_one_exists() {
        eval_bench(|ws| {
            let (a, _b, sha) = two_asks_and_work(ws);
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
            let sha2 = commit(&ws.root, &[("src/more.js", Some("x\n"))], "after seal");
            let out = open_eval(ws, Open::default()).unwrap();
            assert_eq!(out["anchor"], json!({"kind": "seal", "ref": sha, "seal_id": "sel_old"}));
            assert_eq!(paths_of(&brief_of(ws, &out)["changes"]), ["src/more.js"]);

            let items = json!([{"id": "i1", "text": "More", "ask_id": a, "status": "active"}]);
            let brief_sha = str_of(&out["brief"], "sha256");
            let js: Vec<Value> = angles()
                .iter()
                .map(|ang| judgement_over(&brief_sha, ang, &[("i1", "yes")], &[], &["src/more.js"]))
                .collect();
            close(ws, &str_of(&out, "eval_id"), &intent_doc(&brief_sha, items.clone()), &js)
                .unwrap();

            let explicit =
                open_eval(ws, Open { anchor: Some(&sha2), ..Default::default() }).unwrap();
            assert_eq!(explicit["anchor"]["kind"], "explicit");
            assert!(paths_of(&brief_of(ws, &explicit)["changes"]).is_empty());
            // The explicit one is still open.
            let e = open_eval(ws, Open { anchor: Some(&"0".repeat(40)), ..Default::default() })
                .unwrap_err();
            refused(e, "eval.already_open");

            let brief_sha2 = str_of(&explicit["brief"], "sha256");
            let js2: Vec<Value> = angles()
                .iter()
                .map(|ang| judgement_over(&brief_sha2, ang, &[("i1", "yes")], &[], &[]))
                .collect();
            close(ws, &str_of(&explicit, "eval_id"), &intent_doc(&brief_sha2, items), &js2)
                .unwrap();
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
            let e = anchor_of(ws, &[json!({"ask_id": "ask_old", "base_commit": null})], None)
                .unwrap_err();
            assert!(
                matches!(&e, EvalError::Needs(n) if n.reason.contains("eval.anchor_unknown")),
                "{e}"
            );
        });
    }

    #[test]
    fn close_validates_the_intent_and_the_judgements() {
        eval_bench(|ws| {
            let o = opened(ws);
            let eid = str_of(&o.out, "eval_id");
            let items = one_item(&o.a);
            let good = |angle: &str| judgement(&o.brief_sha, angle, &[("i1", "yes")]);
            let three = [good("a"), good("b"), good("c")];
            let ok_intent = intent_doc(&o.brief_sha, items.clone());

            refused(
                close(ws, &eid, &ok_intent, &[good("a"), good("b")]).unwrap_err(),
                "eval.too_few_judges",
            );
            refused(
                close(ws, &eid, &intent_doc(&"0".repeat(64), items.clone()), &three).unwrap_err(),
                "eval.brief_mismatch",
            );
            let mut wrong_ask = items.clone();
            wrong_ask[0]["ask_id"] = json!("ask_nope");
            refused(
                close(ws, &eid, &intent_doc(&o.brief_sha, wrong_ask), &three).unwrap_err(),
                "eval.intent",
            );
            let bad_vote = judgement(&o.brief_sha, "c", &[("i1", "maybe")]);
            refused(
                close(ws, &eid, &ok_intent, &[good("a"), good("b"), bad_vote]).unwrap_err(),
                "eval.judgement",
            );
            let no_vote = judgement(&o.brief_sha, "c", &[]);
            refused(
                close(ws, &eid, &ok_intent, &[good("a"), good("b"), no_vote]).unwrap_err(),
                "no vote for active items",
            );
            let bad_class = judgement_over(
                &o.brief_sha,
                "c",
                &[("i1", "yes")],
                &[("docs/notes.md", "fine")],
                &CHANGED,
            );
            refused(
                close(ws, &eid, &ok_intent, &[good("a"), good("b"), bad_class]).unwrap_err(),
                "eval.judgement",
            );
            let missing_path =
                judgement_over(&o.brief_sha, "c", &[("i1", "yes")], &[], &["src/uptime.js"]);
            refused(
                close(ws, &eid, &ok_intent, &[good("a"), good("b"), missing_path]).unwrap_err(),
                "no drift classification for changed paths",
            );
            let mut bad_judge = good("c");
            bad_judge["judge"] = json!({"host": "x", "angle": "c", "independence": "telepathy"});
            refused(
                close(ws, &eid, &ok_intent, &[good("a"), good("b"), bad_judge]).unwrap_err(),
                "eval.judgement",
            );
            refused(close(ws, "evl_nope", &ok_intent, &three).unwrap_err(), "eval.missing");

            assert!(!ws.rf_dir().join(format!("evals/{eid}/record.json")).exists());
            assert_eq!(store::events(ws).unwrap().pop().unwrap()["type"], "eval.opened");
        });
    }

    #[test]
    fn close_aligned_with_full_agreement() {
        eval_bench(|ws| {
            let o = opened(ws);
            let eid = str_of(&o.out, "eval_id");
            let items = json!([
                {"id": "i1", "text": "Expose an uptime endpoint", "ask_id": o.a, "status": "active"},
                {"id": "i2", "text": "Cache the result", "ask_id": o.a, "status": "withdrawn",
                 "by_ask_id": o.b, "note": "B says skip the cache"},
                {"id": "i3", "text": "Test the endpoint", "ask_id": o.a, "status": "active"},
            ]);
            let drift = [("docs/notes.md", "consequence")];
            let js: Vec<Value> = angles()
                .iter()
                .map(|ang| {
                    judgement_over(
                        &o.brief_sha,
                        ang,
                        &[("i1", "yes"), ("i3", "yes")],
                        &drift,
                        &CHANGED,
                    )
                })
                .collect();
            let rec = close(ws, &eid, &intent_doc(&o.brief_sha, items.clone()), &js).unwrap();
            assert_eq!(rec["verdict"], "aligned");
            assert_eq!(rec["confidence"], 1.0);
            assert_eq!(
                array_of(&rec, "items").iter().map(|it| str_of(it, "id")).collect::<Vec<_>>(),
                ["i1", "i3"]
            );
            assert_eq!(
                rec["items"][0]["votes"][1],
                json!({"angle": "drift", "vote": "yes", "reason": "drift says yes",
                       "counted_as": "yes"})
            );
            assert_eq!(rec["drift"]["omission"], json!([]));
            assert_eq!(rec["drift"]["commission"], json!([]));
            assert_eq!(rec["drift"]["unmentioned"], json!([]));
            let classified: Vec<(String, String, Value)> = array_of(&rec["drift"], "paths")
                .iter()
                .map(|p| (str_of(p, "path"), str_of(p, "classification"), p["agreement"].clone()))
                .collect();
            assert_eq!(
                classified,
                [
                    ("docs/notes.md".to_string(), "consequence".to_string(), json!(1.0)),
                    ("src/uptime.js".to_string(), "required".to_string(), json!(1.0)),
                    ("tests/uptime.test.js".to_string(), "required".to_string(), json!(1.0)),
                ]
            );
            assert_eq!(rec["intent"]["items"], 3);
            assert_eq!(rec["intent"]["active"], 2);
            assert_eq!(array_of(&rec, "judgements").len(), 3);
            assert_eq!(rec["follows"], json!(null));
            assert_eq!(rec["delta"], json!(null));
            assert_eq!(rec["basis"]["asks"], json!([o.a, o.b]));
            assert_eq!(rec["subject"]["sha256_at_close"], rec["subject"]["sha256"]);

            let last = store::events(ws).unwrap().pop().unwrap();
            assert_eq!(last["type"], "eval.completed");
            assert_eq!(last["data"]["verdict"], "aligned");
            assert_eq!(last["data"]["confidence"], 1.0);
            assert_eq!(last["data"]["items"], json!({"active": 2, "met": 2, "omission": 0}));
            assert_eq!(
                last["links"].as_array().unwrap()[..2],
                [json!({"rel": "evaluates", "id": o.a}), json!({"rel": "evaluates", "id": o.b})]
            );
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
            for name in
                ["brief.json", "intent.json", "judgement-1.json", "judgement-3.json", "record.json"]
            {
                assert!(ws.rf_dir().join(format!("evals/{eid}/{name}")).exists(), "{name}");
            }
            let listed = list_records(ws).unwrap().remove(0);
            assert_eq!(listed["state"], "completed");
            assert_eq!(listed["verdict"], "aligned");
            assert_eq!(listed["confidence"], 1.0);
            assert_eq!(listed["path"], format!("evals/{eid}/record.json"));
            refused(
                close(ws, &eid, &intent_doc(&o.brief_sha, items), &js).unwrap_err(),
                "ledger.immutable",
            );
        });
    }

    /// The same judgement, with its citations removed.
    fn uncited(mut j: Value) -> Value {
        j["commands_run"] = json!([]);
        j["basis_notes"] = json!([]);
        j
    }

    #[test]
    fn a_yes_that_cites_nothing_is_not_decisive() {
        // A judge that ran nothing, noted nothing and named no changed path is
        // asserting, not checking.
        eval_bench(|ws| {
            let o = opened(ws);
            let js: Vec<Value> = angles()
                .iter()
                .map(|ang| uncited(judgement(&o.brief_sha, ang, &[("i1", "yes")])))
                .collect();
            let rec = close(
                ws,
                &str_of(&o.out, "eval_id"),
                &intent_doc(&o.brief_sha, one_item(&o.a)),
                &js,
            )
            .unwrap();
            let item = &rec["items"][0];
            let votes = array_of(item, "votes");
            assert_eq!(
                votes.iter().map(|v| str_of(v, "vote")).collect::<Vec<_>>(),
                ["yes", "yes", "yes"],
                "recorded as cast"
            );
            assert_eq!(
                votes.iter().map(|v| str_of(v, "counted_as")).collect::<Vec<_>>(),
                ["unknown", "unknown", "unknown"],
                "and counted as unknown"
            );
            assert!(votes.iter().all(|v| v["uncited"] == json!(true)));
            assert_eq!(item["majority"], "unknown");
            assert_eq!(rec["verdict"], "incomplete");
        });
    }

    #[test]
    fn a_vote_is_cited_by_a_command_its_judge_ran() {
        eval_bench(|ws| {
            let o = opened(ws);
            let js: Vec<Value> =
                angles().iter().map(|ang| judgement(&o.brief_sha, ang, &[("i1", "yes")])).collect();
            let rec = close(
                ws,
                &str_of(&o.out, "eval_id"),
                &intent_doc(&o.brief_sha, one_item(&o.a)),
                &js,
            )
            .unwrap();
            let item = &rec["items"][0];
            assert_eq!(
                array_of(item, "votes").iter().map(|v| str_of(v, "counted_as")).collect::<Vec<_>>(),
                ["yes", "yes", "yes"]
            );
            assert!(item["votes"][0].get("uncited").is_none());
            assert_eq!(item["majority"], "yes");
        });
    }

    #[test]
    fn a_vote_is_cited_by_naming_a_changed_path() {
        // A basis note or a command covers every vote a judge cast; a path in
        // the reason covers that vote.
        eval_bench(|ws| {
            let o = opened(ws);
            let mut js: Vec<Value> = angles()
                .iter()
                .map(|ang| uncited(judgement(&o.brief_sha, ang, &[("i1", "yes")])))
                .collect();
            js[0]["votes"][0]["reason"] = json!("src/uptime.js exports uptime()");
            js[1]["basis_notes"] = json!(["read the diff"]);
            let rec = close(
                ws,
                &str_of(&o.out, "eval_id"),
                &intent_doc(&o.brief_sha, one_item(&o.a)),
                &js,
            )
            .unwrap();
            let counted: Vec<String> = array_of(&rec["items"][0], "votes")
                .iter()
                .map(|v| str_of(v, "counted_as"))
                .collect();
            assert_eq!(counted, ["yes", "yes", "unknown"]);
            assert_eq!(rec["items"][0]["majority"], "yes", "two cited yes votes still carry it");
        });
    }

    #[test]
    fn only_a_yes_is_downgraded_and_agreement_keeps_every_judge() {
        // A `no` needs no citation to be worth hearing, and a downgraded vote
        // stays in the denominator.
        eval_bench(|ws| {
            let o = opened(ws);
            let js: Vec<Value> = [("coverage", "no"), ("drift", "no"), ("adversary", "yes")]
                .iter()
                .map(|(ang, vote)| uncited(judgement(&o.brief_sha, ang, &[("i1", vote)])))
                .collect();
            let rec = close(
                ws,
                &str_of(&o.out, "eval_id"),
                &intent_doc(&o.brief_sha, one_item(&o.a)),
                &js,
            )
            .unwrap();
            let item = &rec["items"][0];
            assert_eq!(
                array_of(item, "votes").iter().map(|v| str_of(v, "counted_as")).collect::<Vec<_>>(),
                ["no", "no", "unknown"]
            );
            assert_eq!(item["majority"], "no");
            assert_eq!(item["agreement"], 0.67, "two of three judges, not two of two");
            assert_eq!(rec["verdict"], "drifted");
        });
    }

    #[test]
    fn close_drifted_reports_disagreement_as_confidence() {
        eval_bench(|ws| {
            let o = opened(ws);
            let items = json!([
                {"id": "i1", "text": "Expose an uptime endpoint", "ask_id": o.a, "status": "active"},
                {"id": "i2", "text": "Test the endpoint", "ask_id": o.a, "status": "active"},
            ]);
            let flagged = [("docs/notes.md", "unexplained")];
            let mut adversary = judgement_over(
                &o.brief_sha,
                "adversary",
                &[("i1", "no"), ("i2", "unknown")],
                &[("docs/notes.md", "consequence")],
                &CHANGED,
            );
            adversary["basis_notes"] = json!(["i2 is vague"]);
            adversary["commands_run"] = json!(["git log"]);
            let js = vec![
                judgement_over(
                    &o.brief_sha,
                    "coverage",
                    &[("i1", "yes"), ("i2", "no")],
                    &flagged,
                    &CHANGED,
                ),
                judgement_over(
                    &o.brief_sha,
                    "drift",
                    &[("i1", "yes"), ("i2", "no")],
                    &flagged,
                    &CHANGED,
                ),
                adversary,
            ];
            let rec = close(ws, &str_of(&o.out, "eval_id"), &intent_doc(&o.brief_sha, items), &js)
                .unwrap();
            assert_eq!(rec["verdict"], "drifted");
            assert_eq!(rec["confidence"], 0.67);
            let table: Vec<(String, String, Value)> = array_of(&rec, "items")
                .iter()
                .map(|it| (str_of(it, "id"), str_of(it, "majority"), it["agreement"].clone()))
                .collect();
            assert_eq!(
                table,
                [
                    ("i1".to_string(), "yes".to_string(), json!(0.67)),
                    ("i2".to_string(), "no".to_string(), json!(0.67)),
                ]
            );
            assert_eq!(rec["drift"]["omission"], json!(["i2"]));
            assert_eq!(
                rec["drift"]["commission"],
                json!([{"path": "docs/notes.md", "classification": "unexplained",
                        "agreement": 0.67}])
            );
            assert_eq!(rec["drift"]["unmentioned"], json!([]));
            assert!(
                array_of(&rec, "limitations")
                    .iter()
                    .any(|l| str_of_value(l).contains("sub-agents"))
            );
        });
    }

    #[test]
    fn close_is_incomplete_on_ties_unknowns_or_a_changed_subject() {
        eval_bench(|ws| {
            let o = opened(ws);
            let items = one_item(&o.a);
            let js = vec![
                judgement(&o.brief_sha, "coverage", &[("i1", "yes")]),
                judgement(&o.brief_sha, "drift", &[("i1", "no")]),
                judgement(&o.brief_sha, "adversary", &[("i1", "unknown")]),
            ];
            let rec = close(
                ws,
                &str_of(&o.out, "eval_id"),
                &intent_doc(&o.brief_sha, items.clone()),
                &js,
            )
            .unwrap();
            assert_eq!(rec["verdict"], "incomplete");
            assert_eq!(rec["items"][0]["majority"], "unknown");
            assert_eq!(rec["confidence"], 0.33);

            // A worktree subject that changed between open and close is never
            // aligned.
            std::fs::write(ws.root.join("src/uptime.js"), "dirty\n").unwrap();
            let out2 = open_eval(ws, Open::default()).unwrap();
            let sha2 = str_of(&out2["brief"], "sha256");
            assert_eq!(out2["subject"]["kind"], "worktree");
            std::fs::write(ws.root.join("src/uptime.js"), "dirtier\n").unwrap();
            let js2: Vec<Value> =
                angles().iter().map(|ang| judgement(&sha2, ang, &[("i1", "yes")])).collect();
            let rec2 =
                close(ws, &str_of(&out2, "eval_id"), &intent_doc(&sha2, items), &js2).unwrap();
            assert_eq!(rec2["verdict"], "incomplete");
            assert!(
                array_of(&rec2, "limitations")
                    .iter()
                    .any(|l| str_of_value(l).starts_with("subject.changed"))
            );
        });
    }

    #[test]
    fn one_loud_judge_cannot_make_a_path_unanimous() {
        eval_bench(|ws| {
            let o = opened(ws);
            let items = one_item(&o.a);
            let js = vec![
                judgement(&o.brief_sha, "coverage", &[("i1", "yes")]),
                judgement(&o.brief_sha, "drift", &[("i1", "yes")]),
                judgement_over(
                    &o.brief_sha,
                    "adversary",
                    &[("i1", "yes")],
                    &[("docs/notes.md", "unexplained")],
                    &CHANGED,
                ),
            ];
            let rec = close(
                ws,
                &str_of(&o.out, "eval_id"),
                &intent_doc(&o.brief_sha, items.clone()),
                &js,
            )
            .unwrap();
            let docs = array_of(&rec["drift"], "paths")
                .iter()
                .find(|p| p["path"] == "docs/notes.md")
                .unwrap()
                .clone();
            assert_eq!(docs["classification"], "required");
            assert_eq!(docs["agreement"], 0.67);
            assert_eq!(docs["mentions"], 3);
            assert_eq!(rec["drift"]["commission"], json!([]));
            assert_eq!(rec["verdict"], "aligned");
            assert_eq!(rec["confidence"], 0.67);

            // Two of three flagging it is a finding, at their agreement.
            let out2 = open_eval(ws, Open::default()).unwrap();
            let sha2 = str_of(&out2["brief"], "sha256");
            let flagged = [("docs/notes.md", "unexplained")];
            let js2 = vec![
                judgement_over(&sha2, "coverage", &[("i1", "yes")], &flagged, &CHANGED),
                judgement_over(&sha2, "drift", &[("i1", "yes")], &flagged, &CHANGED),
                judgement(&sha2, "adversary", &[("i1", "yes")]),
            ];
            let rec2 =
                close(ws, &str_of(&out2, "eval_id"), &intent_doc(&sha2, items), &js2).unwrap();
            assert_eq!(
                rec2["drift"]["commission"],
                json!([{"path": "docs/notes.md", "classification": "unexplained",
                        "agreement": 0.67}])
            );
            assert_eq!(rec2["verdict"], "drifted");
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
    fn shared_context_judges_are_recorded_as_a_limitation() {
        eval_bench(|ws| {
            let o = opened(ws);
            let js: Vec<Value> = angles()
                .iter()
                .map(|ang| {
                    let mut j = judgement(&o.brief_sha, ang, &[("i1", "yes")]);
                    j["judge"]["independence"] = json!("shared_context");
                    j
                })
                .collect();
            let rec = close(
                ws,
                &str_of(&o.out, "eval_id"),
                &intent_doc(&o.brief_sha, one_item(&o.a)),
                &js,
            )
            .unwrap();
            assert_eq!(rec["verdict"], "aligned");
            assert!(
                array_of(&rec, "limitations")
                    .iter()
                    .any(|l| str_of_value(l).contains("shared one context"))
            );
        });
    }

    #[test]
    fn a_second_eval_follows_the_first_and_reports_the_delta() {
        eval_bench(|ws| {
            let o = opened(ws);
            let items = json!([
                {"id": "i1", "text": "Expose an uptime endpoint", "ask_id": o.a, "status": "active"},
                {"id": "i2", "text": "Test the endpoint", "ask_id": o.a, "status": "active"},
            ]);
            let flagged = [("docs/notes.md", "unexplained")];
            let js: Vec<Value> = angles()
                .iter()
                .map(|ang| {
                    judgement_over(
                        &o.brief_sha,
                        ang,
                        &[("i1", "yes"), ("i2", "no")],
                        &flagged,
                        &CHANGED,
                    )
                })
                .collect();
            let first =
                close(ws, &str_of(&o.out, "eval_id"), &intent_doc(&o.brief_sha, items), &js)
                    .unwrap();
            assert_eq!(first["verdict"], "drifted");

            commit(
                &ws.root,
                &[("tests/uptime.test.js", Some("real test\n")), ("docs/notes.md", None)],
                "fix",
            );
            let out2 = open_eval(ws, Open::default()).unwrap();
            let sha2 = str_of(&out2["brief"], "sha256");
            let brief2 = brief_of(ws, &out2);
            assert_eq!(
                brief2["previous_evals"],
                json!([{"eval_id": first["eval_id"], "verdict": "drifted", "confidence": 1.0,
                        "time": first["time"]}])
            );
            // docs/notes.md no longer differs from the anchor.
            assert_eq!(paths_of(&brief2["changes"]), ["src/uptime.js", "tests/uptime.test.js"]);

            // The second intent judge numbers the items differently and
            // rewords one; matching falls back to text, then token overlap.
            let items2 = json!([
                {"id": "x1", "text": "Test the uptime endpoint with a unit test", "ask_id": o.a,
                 "status": "active"},
                {"id": "x2", "text": "Expose an uptime endpoint", "ask_id": o.a, "status": "active"},
            ]);
            let js2: Vec<Value> = angles()
                .iter()
                .map(|ang| {
                    judgement_over(
                        &sha2,
                        ang,
                        &[("x1", "yes"), ("x2", "yes")],
                        &[],
                        &["src/uptime.js", "tests/uptime.test.js"],
                    )
                })
                .collect();
            let second =
                close(ws, &str_of(&out2, "eval_id"), &intent_doc(&sha2, items2), &js2).unwrap();
            assert_eq!(second["verdict"], "aligned");
            assert_eq!(second["follows"], first["eval_id"]);
            assert_eq!(
                second["delta"]["closed"],
                json!(["Test the uptime endpoint with a unit test"])
            );
            assert_eq!(second["delta"]["opened"], json!([]));
            assert_eq!(second["delta"]["new_items"], json!([]));
            assert_eq!(second["delta"]["commission_removed"], json!(["docs/notes.md"]));
            assert_eq!(second["delta"]["commission_added"], json!([]));
            let last = store::events(ws).unwrap().pop().unwrap();
            assert!(
                array_of(&last, "links")
                    .contains(&json!({"rel": "supersedes", "id": first["eval_id"]}))
            );
            assert_eq!(
                list_records(ws).unwrap().iter().map(|r| str_of(r, "eval_id")).collect::<Vec<_>>(),
                [str_of(&first, "eval_id"), str_of(&second, "eval_id")]
            );
            assert_eq!(
                latest_for(ws, std::slice::from_ref(&o.a)).unwrap().unwrap()["eval_id"],
                second["eval_id"]
            );
            assert!(latest_for(ws, &["ask_other".to_string()]).unwrap().is_none());
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

    /// A hook payload in the documented hook-input shape, carrying output
    /// that must never reach the ledger.
    fn hook(event_name: &str, tool: &str, command: &str) -> Value {
        json!({"session_id": "s1", "cwd": "/ignored", "hook_event_name": event_name,
               "tool_name": tool, "tool_input": {"command": command},
               "tool_response": {"stdout": "SECRET-OUT", "stderr": "SECRET-ERR"},
               "tool_use_id": "toolu_1"})
    }

    #[test]
    fn a_shell_command_during_open_work_is_a_fact_about_the_subject() {
        eval_bench(|ws| {
            two_asks_and_work(ws);
            let ok = record_fact(ws, "claude-code", &hook("PostToolUse", "Bash", "npm test"))
                .unwrap()
                .unwrap();
            let (kind, reference) = default_subject(ws).unwrap();
            let sha = subject_digest(ws, &kind, &reference).unwrap();
            assert_eq!(ok["type"], "tool.fact");
            assert!(str_of(&ok, "id").starts_with("fct_"), "{}", ok["id"]);
            assert_eq!(
                ok["data"],
                json!({"command": "npm test", "outcome": "succeeded",
                       "subject": {"kind": kind, "ref": reference, "sha256": sha,
                                   "content": worktree_content(ws).unwrap()},
                       "host": "claude-code", "session_ref": "s1", "tool_use_id": "toolu_1"})
            );
            let failed =
                record_fact(ws, "claude-code", &hook("PostToolUseFailure", "Bash", "npm test"))
                    .unwrap()
                    .unwrap();
            assert_eq!(failed["data"]["outcome"], "failed");
            let ledger = std::fs::read_to_string(ws.rf_dir().join("ledger.jsonl")).unwrap();
            assert!(!ledger.contains("SECRET"), "no output reaches the ledger");
            let facts =
                store::events(ws).unwrap().iter().filter(|e| e["type"] == "tool.fact").count();
            assert_eq!(facts, 2);
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
    }

    struct Facts {
        eval_id: String,
        brief_sha: String,
        fresh: String,
        stale: String,
        a: String,
    }

    /// Work, an `npm test` that succeeded on it, then an edit and a second
    /// `npm test` (of `outcome`) on the edited tree, then an open Eval.
    fn with_facts(ws: &Workspace, outcome: &str) -> Facts {
        let (a, _, _) = two_asks_and_work(ws);
        let stale = record_fact(ws, "claude-code", &hook("PostToolUse", "Bash", "npm test"))
            .unwrap()
            .unwrap();
        std::fs::write(ws.root.join("src/uptime.js"), "export const uptime = () => 2;\n").unwrap();
        let hook_name = if outcome == "succeeded" { "PostToolUse" } else { "PostToolUseFailure" };
        let fresh =
            record_fact(ws, "claude-code", &hook(hook_name, "Bash", "npm test")).unwrap().unwrap();
        let out = open_eval(ws, Open::default()).unwrap();
        Facts {
            eval_id: str_of(&out, "eval_id"),
            brief_sha: str_of(&out["brief"], "sha256"),
            fresh: str_of(&fresh, "id"),
            stale: str_of(&stale, "id"),
            a,
        }
    }

    fn citing(brief_sha: &str, angle: &str, votes: &[(&str, &str)], facts: Value) -> Value {
        let mut j = judgement(brief_sha, angle, votes);
        j["votes"][0]["facts_cited"] = facts;
        j
    }

    #[test]
    fn the_brief_lists_each_fact_and_whether_it_describes_the_subject() {
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
            assert_eq!(
                facts,
                [
                    (f.stale.clone(), "succeeded".to_string(), json!(false)),
                    (f.fresh.clone(), "failed".to_string(), json!(true)),
                ]
            );
            assert_eq!(array_of(&brief, "facts")[0]["command"], "npm test");
            assert!(array_of(&brief, "facts")[0]["time"].is_string());
            let says = |b: &Value| {
                array_of(b, "limitations").iter().any(|l| {
                    str_of_value(l)
                        == "no tool facts were recorded; the result of a command cannot be judged"
                })
            };
            assert!(!says(&brief));
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
                let tested =
                    record_fact(ws, "claude-code", &hook("PostToolUse", "Bash", "npm test"))
                        .unwrap()
                        .unwrap();
                assert_eq!(tested["data"]["subject"]["kind"], "worktree");
                commit(&ws.root, extra, "tested work");
                let out = open_eval(ws, Open::default()).unwrap();
                assert_eq!(out["subject"]["kind"], "git_commit");
                brief_of(ws, &out)["facts"][0]["fresh"].clone()
            })
        };
        assert_eq!(fresh_after(&[]), json!(true));
        assert_eq!(fresh_after(&[("src/uptime.js", Some("changed after\n"))]), json!(false));
    }

    #[test]
    fn a_command_obligation_is_met_only_by_a_fresh_succeeded_fact_for_it() {
        eval_bench(|ws| {
            let f = with_facts(ws, "succeeded");
            let items = json!([
                {"id": "i1", "text": "npm test passes", "ask_id": f.a, "status": "active",
                 "check": "npm test"},
                {"id": "i2", "text": "cargo test passes", "ask_id": f.a, "status": "active",
                 "check": "cargo test"},
                {"id": "i3", "text": "Expose an uptime endpoint", "ask_id": f.a, "status": "active"},
            ]);
            let votes = [("i1", "yes"), ("i2", "yes"), ("i3", "yes")];
            let mut coverage = citing(&f.brief_sha, "coverage", &votes, json!([f.fresh]));
            coverage["votes"][1]["facts_cited"] = json!([f.fresh]);
            let js = vec![
                coverage,
                citing(&f.brief_sha, "drift", &votes, json!([f.stale])),
                citing(&f.brief_sha, "adversary", &votes, json!([])),
            ];
            let rec = close(ws, &f.eval_id, &intent_doc(&f.brief_sha, items), &js).unwrap();
            let counted = |n: usize| -> Vec<String> {
                array_of(&array_of(&rec, "items")[n], "votes")
                    .iter()
                    .map(|v| str_of(v, "counted_as"))
                    .collect()
            };
            assert_eq!(
                counted(0),
                ["yes", "unknown", "unknown"],
                "fresh counts; stale and uncited do not"
            );
            assert_eq!(counted(1), ["unknown", "unknown", "unknown"], "a fact for another command");
            assert_eq!(
                counted(2),
                ["yes", "yes", "yes"],
                "an item without a check keeps today's rule"
            );
            assert_eq!(array_of(&rec, "items")[0]["votes"][1]["uncited"], true);
        });
    }

    #[test]
    fn a_failed_fact_supports_a_no_and_a_no_is_never_downgraded() {
        eval_bench(|ws| {
            let f = with_facts(ws, "failed");
            let items = json!([{"id": "i1", "text": "npm test passes", "ask_id": f.a,
                                "status": "active", "check": "npm test"}]);
            let js = vec![
                citing(&f.brief_sha, "coverage", &[("i1", "no")], json!([f.fresh])),
                citing(&f.brief_sha, "drift", &[("i1", "no")], json!([])),
                citing(&f.brief_sha, "adversary", &[("i1", "yes")], json!([f.fresh])),
            ];
            let rec = close(ws, &f.eval_id, &intent_doc(&f.brief_sha, items), &js).unwrap();
            let item = &array_of(&rec, "items")[0];
            let counted: Vec<String> =
                array_of(item, "votes").iter().map(|v| str_of(v, "counted_as")).collect();
            assert_eq!(counted, ["no", "no", "unknown"]);
            assert_eq!(item["majority"], "no");
        });
    }

    fn agents(v: Value) -> Open<'static> {
        Open { agents: Some(v), ..Open::default() }
    }

    #[test]
    fn open_records_the_agents_it_was_asked_for_and_refuses_what_is_not_a_role() {
        eval_bench(|ws| {
            two_asks_and_work(ws);
            let asked = json!({"context": {"model": "claude-sonnet-5", "effort": "low"},
                               "adversary": {"effort": "high"}});
            let out = open_eval(ws, agents(asked.clone())).unwrap();
            assert_eq!(out["agents"], asked);
            assert_eq!(brief_of(ws, &out)["agents"], asked);
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

    /// A close over every changed path, all yes: enough to finish an Eval.
    fn close_all_yes(ws: &Workspace, o: &crate::testing::Opened) -> Result<Value, EvalError> {
        let js: Vec<Value> = angles()
            .iter()
            .map(|a| judgement_over(&o.brief_sha, a, &[("i1", "yes")], &[], &CHANGED))
            .collect();
        close(
            ws,
            str_of(&o.out, "eval_id").as_str(),
            &intent_doc(&o.brief_sha, one_item(&o.a)),
            &js,
        )
    }

    #[test]
    fn a_change_map_is_published_once_for_an_open_eval_and_carried_into_the_record() {
        eval_bench(|ws| {
            let o = opened(ws);
            let eid = str_of(&o.out, "eval_id");
            let out =
                gather(ws, &eid, b"## src/uptime.js\nAdds uptime().\n", "codex", None).unwrap();
            assert_eq!(out["eval_id"], eid.as_str());
            let map = ws.rf_dir().join(format!("evals/{eid}/context.md"));
            assert_eq!(std::fs::read(&map).unwrap(), b"## src/uptime.js\nAdds uptime().\n");
            let ev = store::events(ws)
                .unwrap()
                .into_iter()
                .find(|e| e["type"] == "eval.gathered")
                .unwrap();
            assert_eq!(ev["data"]["eval_id"], eid.as_str());
            assert_eq!(ev["data"]["host"], "codex");
            assert_eq!(ev["data"]["context_map"]["role"], "context_map");
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
            refused(
                gather(ws, &eid, b"again", "codex", None).unwrap_err(),
                "eval.already_gathered",
            );
            let rec = close_all_yes(ws, &o).unwrap();
            assert_eq!(rec["context_map"], ev["data"]["context_map"]);
            refused(gather(ws, "evl_nowhere", b"m", "codex", None).unwrap_err(), "eval.not_open");
        });
        eval_bench(|ws| {
            let o = opened(ws);
            let eid = str_of(&o.out, "eval_id");
            refused(gather(ws, &eid, b"  \n", "codex", None).unwrap_err(), "eval.context");
            let rec = close_all_yes(ws, &o).unwrap();
            assert!(rec.get("context_map").is_none(), "no map, no ref");
            refused(gather(ws, &eid, b"late", "codex", None).unwrap_err(), "eval.not_open");
        });
    }

    #[test]
    fn close_records_the_debate_agents_and_says_where_a_judge_ran_on_another_model() {
        let says = |rec: &Value| -> Vec<String> {
            array_of(rec, "limitations")
                .iter()
                .map(str_of_value)
                .filter(|l| l.contains("its role asked for"))
                .collect()
        };
        eval_bench(|ws| {
            let o = opened(ws);
            let js: Vec<Value> = angles()
                .iter()
                .map(|a| judgement_over(&o.brief_sha, a, &[("i1", "yes")], &[], &CHANGED))
                .collect();
            let asked = json!({"intent": {"model": "claude-opus-5-5", "effort": "high"},
                               "coverage": {"model": "CLAUDE-SONNET-5"},
                               "adversary": {"model": "claude-opus-5-5"}, "drift": {"effort": "high"}});
            let intent = intent_doc(&o.brief_sha, one_item(&o.a));
            let eid = str_of(&o.out, "eval_id");
            let bad = json!({"judge": {"model": "m"}});
            refused(
                close_eval(ws, &eid, &intent, &js, Some(&bad), None).unwrap_err(),
                "eval.agents",
            );
            let rec = close_eval(ws, &eid, &intent, &js, Some(&asked), None).unwrap();
            assert_eq!(rec["agents"], asked);
            assert_eq!(
                says(&rec),
                [
                    "the intent judge reported claude-sonnet-5; its role asked for claude-opus-5-5",
                    "the adversary judge reported claude-sonnet-5; its role asked for claude-opus-5-5",
                ]
            );
        });
        eval_bench(|ws| {
            let rec = close_all_yes(ws, &opened(ws)).unwrap();
            assert_eq!(rec["agents"], json!({}));
            assert!(says(&rec).is_empty());
        });
    }

    /// A coordinator told `judgement[1]` corrected the wrong file twice in
    /// G8: a refusal names the judgement by its angle and its place among the
    /// `--judgement` flags, and says what the vote named and what it may name.
    #[test]
    fn a_refused_judgement_is_named_by_its_angle_and_flag() {
        eval_bench(|ws| {
            let o = opened(ws);
            let mut js: Vec<Value> =
                angles().iter().map(|a| judgement(&o.brief_sha, a, &[("i1", "yes")])).collect();
            js[1]["votes"][0] = json!({"id": "i1", "vote": "yes", "reason": "r"});
            let e = close(
                ws,
                str_of(&o.out, "eval_id").as_str(),
                &intent_doc(&o.brief_sha, one_item(&o.a)),
                &js,
            )
            .unwrap_err();
            refused(
                e,
                "the drift judgement (--judgement 2): votes[0].item \"\" is not an intent item; use one of i1",
            );
        });
    }

    #[test]
    fn a_cited_fact_must_be_in_the_brief_and_a_check_must_name_a_command() {
        eval_bench(|ws| {
            let f = with_facts(ws, "succeeded");
            let items = json!([{"id": "i1", "text": "npm test passes", "ask_id": f.a,
                                "status": "active", "check": "npm test"}]);
            let good: Vec<Value> = angles()
                .iter()
                .map(|a| citing(&f.brief_sha, a, &[("i1", "yes")], json!([f.fresh])))
                .collect();
            let mut bad = good.clone();
            bad[2]["votes"][0]["facts_cited"] = json!(["fct_nowhere"]);
            refused(
                close(ws, &f.eval_id, &intent_doc(&f.brief_sha, items.clone()), &bad).unwrap_err(),
                "eval.fact_unknown",
            );
            let mut blank = items;
            blank[0]["check"] = json!("");
            refused(
                close(ws, &f.eval_id, &intent_doc(&f.brief_sha, blank), &good).unwrap_err(),
                "eval.intent",
            );
        });
    }

    #[test]
    fn nothing_else_is_a_fact() {
        eval_bench(|ws| {
            let none = |p: Value| assert_eq!(record_fact(ws, "claude-code", &p).unwrap(), None);
            none(hook("PostToolUse", "Bash", "npm test"));
            crate::testing::confirm_ask(ws, "t", b"s\n", b"p\n");
            none(hook("PostToolUse", "Bash", "ringframe eval open"));
            none(hook("PostToolUse", "Bash", "  ringframe ask list --json"));
            none(hook("PostToolUse", "Read", "npm test"));
            none(hook("PreToolUse", "Bash", "npm test"));
            none(hook("PostToolUse", "Bash", ""));
            assert!(store::events(ws).unwrap().iter().all(|e| e["type"] != "tool.fact"));
        });
    }

    #[test]
    fn eval_md_lists_what_did_not_pass_and_nothing_that_did() {
        let vote = |angle: &str, cast: &str, counted: &str, reason: &str| {
            let mut v =
                json!({"angle": angle, "vote": cast, "counted_as": counted, "reason": reason});
            if cast != counted {
                v["uncited"] = json!(true);
            }
            v
        };
        let record = json!({
            "eval_id": "evl_1", "verdict": "drifted", "confidence": 0.67,
            "time": "2026-09-24T00:00:00Z",
            "subject": {"kind": "worktree", "ref": "/w", "sha256": "aab05fddc1129999"},
            "basis": {"asks": ["ask_1"]},
            "items": [
                {"id": "i1", "text": "Build it", "ask_id": "ask_1", "majority": "yes",
                 "votes": [vote("coverage", "yes", "yes", "done")]},
                {"id": "i2", "text": "Test it", "ask_id": "ask_1", "majority": "no",
                 "votes": [vote("adversary", "no", "no", "r3"), vote("coverage", "no", "no", "r1"),
                           vote("drift", "yes", "unknown", "r2")]},
                {"id": "i3", "text": "Document it", "ask_id": "ask_1", "majority": "unknown",
                 "votes": [vote("coverage", "unknown", "unknown", "cannot see")]},
            ],
            "drift": {"paths": [
                {"path": "a.md", "classification": "unexplained", "findings": [
                    {"angle": "drift", "classification": "unexplained", "finding": "nobody asked"},
                    {"angle": "coverage", "classification": "unexplained", "finding": "no Ask"}]},
                {"path": "b.rs", "classification": "required", "findings": []},
            ]},
            "limitations": ["first", "second"],
        });
        let brief = json!({"asks": [
            {"ask_id": "ask_1", "title": "Build the thing", "prompt_path": "asks/ask_1/prompt.txt"}
        ]});
        assert_eq!(
            render_eval_md(&record, &brief),
            "# Eval evl_1: drifted, 0.67 agreed\n\
             \n\
             Subject: worktree aab05fddc112 · 2026-09-24T00:00:00Z\n\
             Judged: 1 open Ask\n\
             - ask_1 Build the thing — .fab7/rf/asks/ask_1/prompt.txt\n\
             \n\
             ## Not met\n\
             - i2 Test it (from ask_1)\n\
             \x20 - coverage no: r1\n\
             \x20 - drift unknown (cast yes, uncited): r2\n\
             \x20 - adversary no: r3\n\
             \n\
             ## Unresolved\n\
             - i3 Document it (from ask_1)\n\
             \x20 - coverage unknown: cannot see\n\
             \n\
             ## Changed with no Ask to explain it\n\
             - a.md\n\
             \x20 - coverage: no Ask\n\
             \x20 - drift: nobody asked\n\
             \n\
             ## Limitations\n\
             - first\n\
             - second\n"
        );
    }

    #[test]
    fn the_demo_record_renders_to_its_golden_file() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/eval-md");
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
        let record: Value = serde_json::from_str(&read("record.json")).unwrap();
        let brief: Value = serde_json::from_str(&read("brief.json")).unwrap();
        let once = render_eval_md(&record, &brief);
        assert_eq!(once, render_eval_md(&record, &brief), "the same record renders the same bytes");
        assert_eq!(once, read("eval.md"));
    }

    fn files_under(root: &std::path::Path) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.insert(path.strip_prefix(root).unwrap().to_string_lossy().into_owned());
                }
            }
        }
        out
    }

    #[test]
    fn close_writes_eval_md_beside_the_record_and_nowhere_else() {
        eval_bench(|ws| {
            let o = opened(ws);
            let eval_id = str_of(&o.out, "eval_id");
            let js: Vec<Value> =
                angles().iter().map(|a| judgement(&o.brief_sha, a, &[("i1", "no")])).collect();
            let root = ws.root.to_string_lossy().into_owned();
            let subject_before = subject_digest(ws, "worktree", &root).unwrap();
            let files_before = files_under(&ws.root);
            let rec = close(ws, &eval_id, &intent_doc(&o.brief_sha, one_item(&o.a)), &js).unwrap();

            let md_path = ws.rf_dir().join(format!("evals/{eval_id}/eval.md"));
            let md = std::fs::read_to_string(&md_path).unwrap();
            assert_eq!(md, render_eval_md(&rec, &brief_of(ws, &o.out)));
            let completed = store::events(ws)
                .unwrap()
                .into_iter()
                .find(|e| e["type"] == "eval.completed")
                .unwrap();
            assert_eq!(
                completed["data"]["eval_md"]["path"],
                json!(format!("evals/{eval_id}/eval.md"))
            );
            assert_eq!(
                str_of(&completed["data"]["eval_md"], "sha256"),
                digest::sha256_file(&md_path).unwrap()
            );

            assert_eq!(subject_digest(ws, "worktree", &root).unwrap(), subject_before);
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new(), "eval.md is referenced");
            let rf = ws.rf_dir().strip_prefix(&ws.root).unwrap().to_string_lossy().into_owned();
            let own = format!("{rf}/evals/{eval_id}/");
            for added in files_under(&ws.root).difference(&files_before) {
                assert!(added.starts_with(&own), "{added} was written outside {own}");
            }
        });
    }
}
