//! The product's knowledge for the Eval and the Seal: the rule files in force
//! in each repository the work touches, and the files each of the person's
//! decisions is pinned to. It is kept as a view, `.fab7/rf/knowledge/view.json`,
//! built from the ledger and the repositories, refreshed only over what changed
//! and rebuilt whole at any time: the ledger and the repositories are the truth,
//! so losing the view loses nothing.
//!
//! A pin is a file's content at a commit. It is stale at another commit when
//! the file there holds something else; a file renamed in between is followed
//! to its new path first. Staleness is read per commit, so one decision can be
//! stale on one branch and not on another.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::digest;
use crate::store::{self, LedgerError};
use crate::workspace::Workspace;

const VIEW: &str = "knowledge/view.json";

/// The instruction file names every harness profile lists (`instructions`),
/// once each, sorted. Empty when no configuration is installed. A product is
/// shared by several agents, so a rule file of any harness is in force.
pub fn instruction_names() -> Vec<String> {
    let Ok(names) = crate::profiles::names() else { return Vec::new() };
    let mut out = BTreeSet::new();
    for n in names {
        if let Ok(p) = crate::profiles::load(&n) {
            for v in p.get("instructions").and_then(Value::as_array).into_iter().flatten() {
                if let Some(s) = v.as_str() {
                    out.insert(s.to_string());
                }
            }
        }
    }
    out.into_iter().collect()
}

fn git_bytes(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().ok()?;
    out.status.success().then_some(out.stdout)
}

fn git_text(root: &Path, args: &[&str]) -> Option<String> {
    git_bytes(root, args).map(|b| String::from_utf8_lossy(&b).to_string())
}

/// The full hash of a commit-ish in `root`, or None.
pub fn commit_of(root: &Path, reference: &str) -> Option<String> {
    let r = format!("{reference}^{{commit}}");
    git_text(root, &["rev-parse", "--verify", "--quiet", &r]).map(|s| s.trim().to_string())
}

/// The SHA-256 of a file's content at a commit, or None when it is not there.
pub fn content_sha256(root: &Path, commit: &str, path: &str) -> Option<String> {
    git_bytes(root, &["cat-file", "blob", &format!("{commit}:{path}")])
        .map(|b| digest::sha256_bytes(&b))
}

/// Every file at `commit` whose name is one of `names`, by path.
pub fn instruction_files(root: &Path, commit: &str, names: &[String]) -> Vec<String> {
    if names.is_empty() {
        return Vec::new();
    }
    let listed =
        git_text(root, &["ls-tree", "-r", "--name-only", "-z", commit]).unwrap_or_default();
    listed
        .split('\0')
        .filter(|p| !p.is_empty() && names.iter().any(|n| file_name(p) == n))
        .map(str::to_string)
        .collect()
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// The instruction files in force for `changed`: those in its folder or a
/// parent, up to the repository's root.
pub fn in_force<'a>(files: &'a [String], changed: &str) -> Vec<&'a String> {
    let mut at = folder(changed);
    let mut out = Vec::new();
    loop {
        out.extend(files.iter().filter(|f| folder(f) == at));
        if at.is_empty() {
            break;
        }
        at = folder(at);
    }
    out
}

/// The Markdown files a document links by relative path inside an `adr/` or
/// `spec/` folder, resolved against the document's own folder.
pub fn linked(doc_path: &str, text: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find("](") {
        rest = &rest[i + 2..];
        let Some(end) = rest.find(')') else { break };
        let target = rest[..end].split('#').next().unwrap_or("").trim();
        rest = &rest[end..];
        if target.is_empty() || target.contains("://") || target.starts_with('/') {
            continue;
        }
        if !target.ends_with(".md") {
            continue;
        }
        let Some(path) = resolve(folder(doc_path), target) else { continue };
        if path.split('/').any(|seg| seg == "adr" || seg == "spec") {
            out.insert(path);
        }
    }
    out.into_iter().collect()
}

fn resolve(base: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// One rule in force, read at `commit`.
fn rule(root: &Path, repo: &str, commit: &str, path: &str, kind: &str) -> Option<Value> {
    let sha256 = content_sha256(root, commit, path)?;
    Some(json!({"repo": repo, "path": path, "commit": commit, "sha256": sha256, "kind": kind}))
}

/// The rules in force for an Eval's changed files in one repository: the
/// instruction files above each, then each plan document and the decision
/// records and specifications it links. Read at `anchor`, each once.
pub fn rules_for(
    root: &Path,
    repo: &str,
    anchor: &str,
    changed: &[String],
    plans: &[String],
    names: &[String],
) -> Vec<Value> {
    let files = instruction_files(root, anchor, names);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for c in changed {
        for f in in_force(&files, c) {
            if seen.insert(f.clone())
                && let Some(r) = rule(root, repo, anchor, f, "instruction")
            {
                out.push(r);
            }
        }
    }
    for plan in plans {
        let Some(text) = git_text(root, &["cat-file", "blob", &format!("{anchor}:{plan}")]) else {
            continue;
        };
        if seen.insert(plan.clone())
            && let Some(r) = rule(root, repo, anchor, plan, "plan")
        {
            out.push(r);
        }
        for l in linked(plan, &text) {
            if seen.insert(l.clone())
                && let Some(r) = rule(root, repo, anchor, &l, "linked")
            {
                out.push(r);
            }
        }
    }
    out
}

// ---- pins ---------------------------------------------------------------------

/// A pin on `path` as it is at `commit`, or as it is on disk when `commit` is
/// the worktree (`None`): then it names the HEAD it sits on.
pub fn pin(root: &Path, repo: &str, commit: Option<&str>, path: &str) -> Option<Value> {
    let (commit, sha256) = match commit {
        Some(c) => (c.to_string(), content_sha256(root, c, path)?),
        None => {
            let bytes = std::fs::read(root.join(path)).ok()?;
            (commit_of(root, "HEAD")?, digest::sha256_bytes(&bytes))
        }
    };
    Some(json!({"repo": repo, "path": path, "commit": commit, "sha256": sha256}))
}

/// Where a pinned file is at `at`: its path there (followed through renames),
/// and whether its content is still the pinned one.
pub fn state(root: &Path, pin: &Value, at: &str) -> Value {
    state_at(root, pin, Some(at))
}

/// As [`state`], at a commit, or in the worktree as it is on disk (`None`).
pub fn state_at(root: &Path, pin: &Value, at: Option<&str>) -> Value {
    let path = pin["path"].as_str().unwrap_or_default();
    let from = pin["commit"].as_str().unwrap_or_default();
    let want = pin["sha256"].as_str().unwrap_or_default();
    let read = |p: &str| match at {
        Some(c) => content_sha256(root, c, p),
        None => std::fs::read(root.join(p)).ok().map(|b| digest::sha256_bytes(&b)),
    };
    let mut now = path.to_string();
    if read(path).is_none() {
        let mut args = vec!["diff", "--name-status", "-M", from];
        args.extend(at);
        args.push("--");
        let diff = git_text(root, &args).unwrap_or_default();
        for line in diff.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() == 3 && cols[0].starts_with('R') && cols[1] == path {
                now = cols[2].to_string();
            }
        }
    }
    let got = read(&now);
    json!({"path": now, "stale": got.as_deref() != Some(want), "deleted": got.is_none(),
           "moved": now != path})
}

/// Whether any of a decision's pins is stale at the commit each repository
/// is read at. A decision with no pins is never stale.
pub fn decision_stale(ws: &Workspace, pins: &[Value], at: &BTreeMap<String, String>) -> bool {
    !stale_files(ws, pins, at).is_empty()
}

/// The files of `pins` that have changed at `at`: `src/session.rs`, or
/// `api/src/session.rs` in another repository.
pub fn stale_files(ws: &Workspace, pins: &[Value], at: &BTreeMap<String, String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in pins {
        let repo = p["repo"].as_str().unwrap_or(".");
        let root = repo_root(ws, repo);
        let commit = at.get(repo).cloned().or_else(|| commit_of(&root, "HEAD"));
        if commit.is_none_or(|c| state(&root, p, &c)["stale"] == json!(true)) {
            let path = p["path"].as_str().unwrap_or_default();
            let shown = if repo == "." { path.to_string() } else { format!("{repo}/{path}") };
            if !out.contains(&shown) {
                out.push(shown);
            }
        }
    }
    out
}

/// The pins of a decision: the files of its windows, as the Eval's change
/// had them at its subject. `changes` is the Eval's change view.
pub fn pins_for(ws: &Workspace, changes: &Value, windows: &[String]) -> Vec<Value> {
    let repos: BTreeMap<String, &Value> = changes["repos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Some((r["name"].as_str()?.to_string(), r)))
        .collect();
    let mut files = BTreeSet::new();
    for u in changes["units"].as_array().into_iter().flatten() {
        let mine = u["windows"].as_array().into_iter().flatten().filter_map(Value::as_str);
        if mine.into_iter().any(|w| windows.iter().any(|x| x == w)) {
            let repo = u["repo"].as_str().unwrap_or(".").to_string();
            let path = u["path"].as_str().unwrap_or_default().to_string();
            if !path.is_empty() {
                files.insert((repo, path));
            }
        }
    }
    files
        .into_iter()
        .filter_map(|(repo, path)| {
            let root = repo_root(ws, &repo);
            let subject = repos.get(&repo).map(|r| &r["subject"]);
            let commit = match subject {
                Some(s) if s["kind"] == "git_commit" => {
                    s["ref"].as_str().and_then(|r| commit_of(&root, r))
                }
                _ => None,
            };
            pin(&root, &repo, commit.as_deref(), &path)
        })
        .collect()
}

/// The pins of a decision about requirements: the plan document each comes
/// from, as the Eval's change had it at its subject. A requirement from an
/// Ask's own words has no document and no pin.
pub fn pins_for_requirements(ws: &Workspace, changes: &Value, ids: &[String]) -> Vec<Value> {
    let mut names: Vec<&str> = changes["repos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["name"].as_str())
        .collect();
    names.sort_by_key(|r| std::cmp::Reverse(r.len()));
    let subjects = subjects(ws, changes);
    let mut files = BTreeSet::new();
    for r in changes["requirements"].as_array().into_iter().flatten() {
        if !ids.iter().any(|i| r["id"].as_str() == Some(i)) {
            continue;
        }
        let Some((doc, _)) = r["part"].as_str().and_then(|p| p.split_once('#')) else { continue };
        let (repo, path) = names
            .iter()
            .filter(|n| **n != ".")
            .find_map(|n| doc.strip_prefix(&format!("{n}/")).map(|p| (n.to_string(), p)))
            .unwrap_or((".".to_string(), doc));
        files.insert((repo, path.to_string()));
    }
    files
        .into_iter()
        .filter_map(|(repo, path)| {
            let root = repo_root(ws, &repo);
            let commit = subjects.get(&repo).cloned().flatten();
            pin(&root, &repo, commit.as_deref(), &path)
        })
        .collect()
}

// ---- what an Eval is given ---------------------------------------------------

/// Each repository of an Eval's change with the commit its subject is read at:
/// a commit, or `None` for a worktree read as it is on disk.
fn subjects(ws: &Workspace, changes: &Value) -> BTreeMap<String, Option<String>> {
    changes["repos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let name = r["name"].as_str()?.to_string();
            let at = match r["subject"]["kind"].as_str() {
                Some("git_commit") => {
                    r["subject"]["ref"].as_str().and_then(|s| commit_of(&repo_root(ws, &name), s))
                }
                _ => None,
            };
            Some((name, at))
        })
        .collect()
}

/// The files of the change, by repository.
fn changed_files(changes: &Value) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for u in changes["units"].as_array().into_iter().flatten() {
        if let (Some(repo), Some(path)) = (u["repo"].as_str(), u["path"].as_str()) {
            out.entry(repo.to_string()).or_default().insert(path.to_string());
        }
    }
    out
}

/// The standing decisions, split three ways at the Eval's subject: those that
/// stand; those stale on a file this change touches, to be asked again; and
/// those stale on files it does not touch, left out without a question.
pub fn split_decisions(
    ws: &Workspace,
    standing: Vec<Value>,
    changes: &Value,
) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let at = subjects(ws, changes);
    let touched = changed_files(changes);
    let (mut fresh, mut again, mut left) = (Vec::new(), Vec::new(), Vec::new());
    for d in standing {
        let pins = d["pins"].as_array().cloned().unwrap_or_default();
        let mut stale_touched = false;
        let mut stale = false;
        let mut changed: Vec<String> = Vec::new();
        for p in &pins {
            let repo = p["repo"].as_str().unwrap_or(".");
            let root = repo_root(ws, repo);
            let commit = match at.get(repo) {
                Some(c) => c.clone(),
                None => commit_of(&root, "HEAD"),
            };
            let s = match (&commit, at.get(repo)) {
                (_, Some(None)) => state_at(&root, p, None),
                (Some(c), _) => state_at(&root, p, Some(c)),
                (None, _) => json!({"stale": true, "path": p["path"]}),
            };
            if s["stale"] == json!(true) {
                stale = true;
                let path = s["path"].as_str().unwrap_or_default();
                let files = touched.get(repo);
                if files.is_some_and(|f| {
                    f.contains(path) || f.contains(p["path"].as_str().unwrap_or_default())
                }) {
                    stale_touched = true;
                    let shown = p["path"].as_str().unwrap_or(path).to_string();
                    if !changed.contains(&shown) {
                        changed.push(shown);
                    }
                }
            }
        }
        match (stale, stale_touched) {
            (false, _) => fresh.push(d),
            (true, true) => {
                let mut d = d;
                d["stale"] = json!(true);
                d["changed"] = json!(changed);
                again.push(d);
            }
            (true, false) => left.push(d),
        }
    }
    (fresh, again, left)
}

/// What an Eval's reader is shown of the product: for each repository of the
/// change, read-only copies before and after it, and the rules in force for
/// its changed files (with the plan documents the Asks named that live in
/// it). A copy that cannot be made is named in `problems`; the Eval goes on.
pub fn product_for(ws: &Workspace, eval: &str, changes: &Value, plans: &[String]) -> Value {
    product_with(
        ws,
        eval,
        changes,
        plans,
        &instruction_names(),
        &|root, commit| crate::snapshots::at_commit(ws, root, commit, eval),
        &|root| crate::snapshots::of_worktree(ws, root, eval),
    )
}

type Snap<'a> = &'a dyn Fn(&Path, &str) -> Result<PathBuf, String>;
type Tree<'a> = &'a dyn Fn(&Path) -> Result<PathBuf, String>;

fn product_with(
    ws: &Workspace,
    _eval: &str,
    changes: &Value,
    plans: &[String],
    names: &[String],
    snap: Snap<'_>,
    tree: Tree<'_>,
) -> Value {
    let touched = changed_files(changes);
    let mut snapshots = Vec::new();
    let mut rules = Vec::new();
    let mut problems = Vec::new();
    let repos: Vec<&Value> = changes["repos"].as_array().into_iter().flatten().collect();
    for r in &repos {
        let Some(name) = r["name"].as_str() else { continue };
        let root = repo_root(ws, name);
        let Some(anchor) = r["anchor"].as_str().and_then(|a| commit_of(&root, a)) else {
            problems.push(format!("{name}: its anchor is not a commit"));
            continue;
        };
        let before = snap(&root, &anchor);
        let after = match r["subject"]["kind"].as_str() {
            Some("git_commit") => {
                match r["subject"]["ref"].as_str().and_then(|s| commit_of(&root, s)) {
                    Some(c) => snap(&root, &c),
                    None => Err("its subject is not a commit".to_string()),
                }
            }
            _ => tree(&root),
        };
        match (before, after) {
            (Ok(b), Ok(a)) => snapshots.push(json!({"repo": name, "anchor": anchor,
                "before": b.to_string_lossy(), "after": a.to_string_lossy()})),
            (Err(e), _) | (_, Err(e)) => problems.push(format!("{name}: {e}")),
        }
        let mine: Vec<String> =
            touched.get(name).map(|s| s.iter().cloned().collect()).unwrap_or_default();
        let own_plans: Vec<String> = plans
            .iter()
            .filter_map(|p| {
                if name == "." {
                    let other = repos
                        .iter()
                        .filter_map(|x| x["name"].as_str())
                        .any(|n| n != "." && p.starts_with(&format!("{n}/")));
                    (!other).then(|| p.clone())
                } else {
                    p.strip_prefix(&format!("{name}/")).map(str::to_string)
                }
            })
            .collect();
        rules.extend(rules_for(&root, name, &anchor, &mine, &own_plans, names));
    }
    json!({"snapshots": snapshots, "rules": rules, "problems": problems})
}

/// A pin on the file a context source names, `<repo>/<path>@<commit>`, with
/// the repository one of the change's.
pub fn pin_source(ws: &Workspace, changes: &Value, source: &str) -> Option<Value> {
    let (place, commit) = source.rsplit_once('@')?;
    let mut repos: Vec<&str> =
        changes["repos"].as_array()?.iter().filter_map(|r| r["name"].as_str()).collect();
    repos.sort_by_key(|r| std::cmp::Reverse(r.len()));
    let (repo, path) =
        repos.iter().find_map(|r| place.strip_prefix(&format!("{r}/")).map(|p| (*r, p)))?;
    let root = repo_root(ws, repo);
    let full = commit_of(&root, commit)?;
    pin(&root, repo, Some(&full), path)
}

/// A context citation's source, `<repo>/<path>@<commit>`, checked against the
/// product: the repository is one of the change's, the commit its anchor or
/// subject, and the quote is in the file there. The error says what failed.
pub fn check_context(
    ws: &Workspace,
    product: &Value,
    source: &str,
    quote: &str,
) -> Result<(), String> {
    let Some((place, commit)) = source.rsplit_once('@') else {
        return Err("a context source is <repo>/<path>@<commit>".into());
    };
    let snaps: Vec<&Value> = product["snapshots"].as_array().into_iter().flatten().collect();
    let mut repos: Vec<&str> = snaps.iter().filter_map(|s| s["repo"].as_str()).collect();
    repos.sort_by_key(|r| std::cmp::Reverse(r.len()));
    let Some((repo, path)) =
        repos.iter().find_map(|r| place.strip_prefix(&format!("{r}/")).map(|p| (*r, p)))
    else {
        return Err(format!("{place} is in no repository of this change"));
    };
    let root = repo_root(ws, repo);
    let Some(full) = commit_of(&root, commit) else {
        return Err(format!("{commit} is not a commit of {repo}"));
    };
    let ruled = product["rules"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|r| r["repo"] == json!(repo) && r["commit"] == json!(full));
    let snap = snaps.iter().find(|s| s["repo"] == json!(repo));
    let at_anchor = snap.is_some_and(|s| s["anchor"] == json!(full));
    let text = if at_anchor || ruled || commit_of(&root, "HEAD").as_deref() == Some(full.as_str()) {
        git_text(&root, &["cat-file", "blob", &format!("{full}:{path}")])
    } else {
        None
    };
    let Some(text) = text else {
        return Err(format!("{path} is not in {repo} at {commit}"));
    };
    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let q = squash(quote.trim_start_matches(['+', '-', ' ']));
    let n = quote.trim().chars().count();
    if !(8..=300).contains(&n) {
        return Err("a context quote is 8 to 300 characters".into());
    }
    if !squash(&text).contains(&q) {
        return Err(format!("the quote is not in {path} at {commit}"));
    }
    Ok(())
}

// ---- the view -----------------------------------------------------------------

/// A repository's root, by the name an Eval gives it (`.` or a path relative
/// to the workspace).
pub fn repo_root(ws: &Workspace, name: &str) -> PathBuf {
    if name == "." { ws.root.clone() } else { ws.root.join(name) }
}

fn view_path(ws: &Workspace) -> PathBuf {
    ws.rf_dir().join(VIEW)
}

fn read_view(ws: &Workspace) -> Value {
    std::fs::read(view_path(ws))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}))
}

fn write_view(ws: &Workspace, view: &Value) -> Result<(), LedgerError> {
    let path = view_path(ws);
    let dir = path.parent().expect("the view has a folder");
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("view.{}.{}", std::process::id(), crate::ids::random_hex(4)));
    let mut bytes = store::canonical(view);
    bytes.push(b'\n');
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The repositories the knowledge covers: the workspace's own, every one an
/// Eval read, and every one a decision is pinned in.
fn repos(ws: &Workspace, decisions: &[Value]) -> BTreeSet<String> {
    let mut out = BTreeSet::from([".".to_string()]);
    if let Ok(dirs) = std::fs::read_dir(ws.rf_dir().join("evals")) {
        for d in dirs.flatten() {
            let Ok(bytes) = std::fs::read(d.path().join("changes.json")) else { continue };
            let Ok(doc) = serde_json::from_slice::<Value>(&bytes) else { continue };
            for r in doc["repos"].as_array().into_iter().flatten() {
                if let Some(n) = r["name"].as_str() {
                    out.insert(n.to_string());
                }
            }
        }
    }
    for d in decisions {
        for p in d["pins"].as_array().into_iter().flatten() {
            if let Some(n) = p["repo"].as_str() {
                out.insert(n.to_string());
            }
        }
    }
    out
}

/// Bring the view up to each repository's HEAD, reading only what changed
/// since it was last refreshed; `whole` reads everything again.
fn refresh_with(ws: &Workspace, names: &[String], whole: bool) -> Result<Value, LedgerError> {
    store::locked(ws, || {
        let old = if whole { json!({}) } else { read_view(ws) };
        let same_names = old["names"] == json!(names);
        let decisions = crate::decisions::all(ws);
        let mut refreshed = serde_json::Map::new();
        let mut rules: Vec<Value> = Vec::new();
        for repo in repos(ws, &decisions) {
            let root = repo_root(ws, &repo);
            let Some(head) = commit_of(&root, "HEAD") else { continue };
            let prev = old["refreshed"][&repo].as_str().map(str::to_string);
            let kept: Vec<Value> = old["rules"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| r["repo"] == json!(repo))
                .cloned()
                .collect();
            let mine = match prev {
                Some(p) if same_names && p == head => kept,
                Some(p) if same_names => match changed_since(&root, &p, &head) {
                    Some(paths) => update(&root, &repo, &head, kept, &paths, names),
                    None => full(&root, &repo, &head, names),
                },
                _ => full(&root, &repo, &head, names),
            };
            refreshed.insert(repo.clone(), json!(head));
            rules.extend(mine);
        }
        let mut pinned = serde_json::Map::new();
        for d in &decisions {
            if let Some(pins) = d["pins"].as_array().filter(|p| !p.is_empty()) {
                pinned.insert(
                    d["id"].as_str().unwrap_or_default().to_string(),
                    json!({"pins": pins}),
                );
            }
        }
        let view =
            json!({"names": names, "refreshed": refreshed, "rules": rules, "decisions": pinned});
        write_view(ws, &view)?;
        Ok(view)
    })
}

fn changed_since(root: &Path, from: &str, to: &str) -> Option<BTreeSet<String>> {
    let diff = git_text(root, &["diff", "--name-status", "-M", from, to, "--"])?;
    Some(diff.lines().flat_map(|l| l.split('\t').skip(1).map(str::to_string)).collect())
}

fn full(root: &Path, repo: &str, head: &str, names: &[String]) -> Vec<Value> {
    instruction_files(root, head, names)
        .iter()
        .filter_map(|p| rule(root, repo, head, p, "instruction"))
        .collect()
}

/// The kept rules with each changed path read again at `head`; a rule on a
/// path that did not change keeps its entry, moved to `head`.
fn update(
    root: &Path,
    repo: &str,
    head: &str,
    kept: Vec<Value>,
    changed: &BTreeSet<String>,
    names: &[String],
) -> Vec<Value> {
    let mut out: Vec<Value> = kept
        .into_iter()
        .filter(|r| !changed.contains(r["path"].as_str().unwrap_or_default()))
        .map(|mut r| {
            r["commit"] = json!(head);
            r
        })
        .collect();
    for p in changed {
        if names.iter().any(|n| file_name(p) == n)
            && let Some(r) = rule(root, repo, head, p, "instruction")
        {
            out.push(r);
        }
    }
    out.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    out
}

/// Refresh the view over what changed; a missing or unreadable view is
/// rebuilt, never an error.
pub fn refresh(ws: &Workspace) -> Result<Value, LedgerError> {
    refresh_with(ws, &instruction_names(), false)
}

/// Write the view from the ledger and the repositories alone.
pub fn rebuild(ws: &Workspace) -> Result<Value, LedgerError> {
    refresh_with(ws, &instruction_names(), true)
}

/// The view as the person reads it: the rule files, and each decision with
/// whether it is stale at `at` (each repository's HEAD when not given).
pub fn show(ws: &Workspace, at: Option<&str>) -> Result<Value, LedgerError> {
    let view = refresh(ws)?;
    let mut commits = BTreeMap::new();
    if let Some(at) = at {
        let root = repo_root(ws, ".");
        if let Some(c) = commit_of(&root, at) {
            commits.insert(".".to_string(), c);
        }
    }
    let decisions: Vec<Value> = crate::decisions::all(ws)
        .into_iter()
        .filter(|d| d["pins"].as_array().is_some_and(|p| !p.is_empty()))
        .map(|d| {
            let pins = d["pins"].as_array().cloned().unwrap_or_default();
            let changed = stale_files(ws, &pins, &commits);
            json!({"id": d["id"], "behaviour": d["behaviour"], "answer": d["answer"],
                   "date": d["date"], "pins": pins, "stale": !changed.is_empty(),
                   "changed": changed})
        })
        .collect();
    Ok(json!({"refreshed": view["refreshed"], "rules": view["rules"], "decisions": decisions}))
}

// ---- secrets ------------------------------------------------------------------

/// Text with what looks like a secret replaced by `[secret]`, before any quote
/// is written: known key prefixes, bearer tokens, private key blocks and
/// credentials inside URLs.
pub fn scrub(text: &str) -> String {
    const PREFIXES: [&str; 9] =
        ["sk-", "ghp_", "gho_", "github_pat_", "xai-", "AIza", "xox", "AKIA", "glpat-"];
    let mut out = String::with_capacity(text.len());
    let mut in_key = false;
    for line in text.split_inclusive('\n') {
        if line.contains("-----BEGIN ") && line.contains("PRIVATE KEY") {
            in_key = true;
        }
        if in_key {
            if line.contains("-----END ") {
                in_key = false;
            }
            out.push_str("[secret]");
            if line.ends_with('\n') {
                out.push('\n');
            }
            continue;
        }
        out.push_str(&scrub_line(line, &PREFIXES));
    }
    out
}

fn scrub_line(line: &str, prefixes: &[&str]) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let mut after_bearer = false;
    let flush = |word: &mut String, out: &mut String, after_bearer: &mut bool| {
        if word.is_empty() {
            return;
        }
        let token_chars = word.chars().all(|c| c.is_ascii_alphanumeric() || "-_.=/+".contains(c));
        let secret =
            (prefixes.iter().any(|p| word.starts_with(p)) && word.len() >= 16 && token_chars)
                || (*after_bearer && word.len() >= 16 && token_chars)
                || url_with_credentials(word);
        *after_bearer = word.eq_ignore_ascii_case("bearer");
        out.push_str(if secret { "[secret]" } else { word });
        word.clear();
    };
    for c in line.chars() {
        if c.is_whitespace() || "\"'`,;()<>".contains(c) {
            flush(&mut word, &mut out, &mut after_bearer);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out, &mut after_bearer);
    out
}

fn url_with_credentials(word: &str) -> bool {
    let Some((scheme, rest)) = word.split_once("://") else { return false };
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+') {
        return false;
    }
    let host_part = rest.split('/').next().unwrap_or("");
    host_part.split_once('@').is_some_and(|(cred, _)| cred.contains(':'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{commit, repo, run, ws_for};

    fn names() -> Vec<String> {
        vec!["AGENTS.md".into(), "CLAUDE.md".into()]
    }

    #[test]
    fn instruction_files_in_a_folder_or_a_parent_are_in_force_up_to_the_root() {
        let files: Vec<String> =
            ["AGENTS.md", "src/AGENTS.md", "src/ui/CLAUDE.md", "docs/AGENTS.md"]
                .map(String::from)
                .to_vec();
        let got: Vec<&str> =
            in_force(&files, "src/ui/view.rs").iter().map(|s| s.as_str()).collect();
        assert_eq!(got, ["src/ui/CLAUDE.md", "src/AGENTS.md", "AGENTS.md"]);
        let top: Vec<&str> = in_force(&files, "main.rs").iter().map(|s| s.as_str()).collect();
        assert_eq!(top, ["AGENTS.md"]);
    }

    #[test]
    fn a_plan_links_its_decision_records_and_specifications() {
        let text = "See [ADR-1](adr/0001-x.md), [spec](../shared/spec/api.md#auth), \
                    [notes](notes.md), [site](https://x.dev/adr/a.md) and [img](adr/fig.png).";
        assert_eq!(
            linked("plans/app/plan.md", text),
            ["plans/app/adr/0001-x.md", "plans/shared/spec/api.md"]
        );
    }

    #[test]
    fn rules_for_a_change_are_its_instruction_files_then_its_plan_and_links() {
        let dir = repo();
        let root = dir.path();
        let anchor = commit(
            root,
            &[
                ("AGENTS.md", Some("be careful\n")),
                ("src/CLAUDE.md", Some("small functions\n")),
                ("other/AGENTS.md", Some("not here\n")),
                ("plans/app/plan.md", Some("Follow [ADR-1](adr/0001-x.md).\n")),
                ("plans/app/adr/0001-x.md", Some("No globals.\n")),
            ],
            "base",
        );
        let got = rules_for(
            root,
            ".",
            &anchor,
            &["src/a.rs".into()],
            &["plans/app/plan.md".into()],
            &names(),
        );
        let paths: Vec<(&str, &str)> = got
            .iter()
            .map(|r| (r["path"].as_str().unwrap(), r["kind"].as_str().unwrap()))
            .collect();
        assert_eq!(
            paths,
            [
                ("src/CLAUDE.md", "instruction"),
                ("AGENTS.md", "instruction"),
                ("plans/app/plan.md", "plan"),
                ("plans/app/adr/0001-x.md", "linked")
            ]
        );
        assert!(got.iter().all(|r| r["commit"] == json!(anchor) && r["repo"] == json!(".")));
    }

    #[test]
    fn a_changed_file_stales_its_pin_and_a_renamed_one_keeps_it() {
        let dir = repo();
        let root = dir.path();
        let base = commit(
            root,
            &[("src/a.rs", Some("fn a() {}\n")), ("src/b.rs", Some("fn b() {}\n"))],
            "base",
        );
        let pa = pin(root, ".", Some(&base), "src/a.rs").unwrap();
        let pb = pin(root, ".", Some(&base), "src/b.rs").unwrap();
        run(&["git", "-C", &root.to_string_lossy(), "mv", "src/b.rs", "src/c.rs"]);
        let later = commit(root, &[("src/a.rs", Some("fn a() { 1 }\n"))], "change a, move b");
        assert_eq!(state(root, &pa, &later)["stale"], json!(true), "a's content changed");
        let b = state(root, &pb, &later);
        assert_eq!(
            (b["stale"].clone(), b["moved"].clone(), b["path"].clone()),
            (json!(false), json!(true), json!("src/c.rs"))
        );
        assert_eq!(state(root, &pa, &base)["stale"], json!(false));
        let gone = commit(root, &[("src/c.rs", None)], "delete");
        assert_eq!(state(root, &pb, &gone)["deleted"], json!(true));
    }

    #[test]
    fn a_pin_is_stale_on_one_branch_and_fresh_on_another() {
        let dir = repo();
        let root = dir.path();
        let r = root.to_string_lossy().to_string();
        let base = commit(root, &[("a.txt", Some("one\n"))], "base");
        let p = pin(root, ".", Some(&base), "a.txt").unwrap();
        run(&["git", "-C", &r, "checkout", "-qb", "other"]);
        let other = commit(root, &[("a.txt", Some("two\n"))], "other");
        run(&["git", "-C", &r, "checkout", "-q", "-"]);
        let same = commit(root, &[("b.txt", Some("x\n"))], "main moves on");
        assert_eq!(state(root, &p, &other)["stale"], json!(true));
        assert_eq!(state(root, &p, &same)["stale"], json!(false));
    }

    #[test]
    fn a_worktree_pin_names_its_head_and_the_files_content() {
        let dir = repo();
        let root = dir.path();
        let head = commit(root, &[("a.txt", Some("one\n"))], "base");
        std::fs::write(root.join("a.txt"), "edited\n").unwrap();
        let p = pin(root, ".", None, "a.txt").unwrap();
        assert_eq!(p["commit"], json!(head));
        assert_eq!(p["sha256"], json!(digest::sha256_bytes(b"edited\n")));
    }

    #[test]
    fn a_decision_without_pins_is_never_stale() {
        let dir = repo();
        let ws = ws_for(dir.path());
        assert!(!decision_stale(&ws, &[], &BTreeMap::new()));
    }

    #[test]
    fn an_unchanged_product_refreshes_nothing_and_a_rebuild_equals_the_refreshed_view() {
        let dir = repo();
        let root = dir.path();
        let ws = ws_for(root);
        commit(root, &[("AGENTS.md", Some("one\n")), ("src/CLAUDE.md", Some("two\n"))], "base");
        let first = refresh_with(&ws, &names(), false).unwrap();
        assert_eq!(first["rules"].as_array().unwrap().len(), 2);
        let mtime = std::fs::metadata(view_path(&ws)).unwrap().modified().unwrap();
        let again = refresh_with(&ws, &names(), false).unwrap();
        assert_eq!(again, first);
        commit(
            root,
            &[("src/CLAUDE.md", Some("three\n")), ("lib/AGENTS.md", Some("four\n"))],
            "more",
        );
        let moved = refresh_with(&ws, &names(), false).unwrap();
        let whole = refresh_with(&ws, &names(), true).unwrap();
        assert_eq!(moved, whole, "a refreshed view equals a rebuilt one");
        assert_eq!(moved["rules"].as_array().unwrap().len(), 3);
        assert!(std::fs::metadata(view_path(&ws)).unwrap().modified().unwrap() >= mtime);
    }

    #[test]
    fn a_missing_or_broken_view_is_rebuilt() {
        let dir = repo();
        let root = dir.path();
        let ws = ws_for(root);
        commit(root, &[("AGENTS.md", Some("one\n"))], "base");
        std::fs::create_dir_all(view_path(&ws).parent().unwrap()).unwrap();
        std::fs::write(view_path(&ws), "{not json").unwrap();
        let v = refresh_with(&ws, &names(), false).unwrap();
        assert_eq!(v["rules"][0]["path"], json!("AGENTS.md"));
    }

    #[test]
    fn two_writers_never_tear_the_view() {
        let dir = repo();
        let root = dir.path().to_path_buf();
        let ws = ws_for(&root);
        commit(&root, &[("AGENTS.md", Some("one\n"))], "base");
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let root = root.clone();
                std::thread::spawn(move || {
                    let ws = crate::workspace::resolve(Some(&root), None).unwrap();
                    for _ in 0..5 {
                        refresh_with(&ws, &names(), false).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let v: Value = serde_json::from_slice(&std::fs::read(view_path(&ws)).unwrap()).unwrap();
        assert_eq!(v["rules"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn instruction_names_come_from_every_harness_profile() {
        crate::testing::with_config_home(|_| {
            let got = instruction_names();
            assert!(
                got.contains(&"AGENTS.md".to_string()) && got.contains(&"CLAUDE.md".to_string()),
                "{got:?}"
            );
        });
    }

    #[test]
    fn secrets_are_scrubbed_before_a_quote_is_written() {
        let text = "key = \"sk-abcdefghijklmnopqrstuv\"\nAuthorization: Bearer abcdefghijklmnop1234\n\
                    url https://user:pass@host.example/x and https://host.example/ok\n\
                    -----BEGIN RSA PRIVATE KEY-----\nMIIabc\n-----END RSA PRIVATE KEY-----\nplain sk-short\n";
        let got = scrub(text);
        assert!(
            !got.contains("sk-abcdef")
                && !got.contains("abcdefghijklmnop1234")
                && !got.contains("user:pass"),
            "{got}"
        );
        assert!(!got.contains("MIIabc"), "{got}");
        assert!(got.contains("https://host.example/ok") && got.contains("plain sk-short"), "{got}");
        assert_eq!(scrub("fn main() {}\n"), "fn main() {}\n");
    }
}
