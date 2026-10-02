//! Eval as bounded tasks that RingFrame hands out and checks, over the change
//! DAG (ADR-0020 D4–D7): **map** reads the change once, in the file tree's
//! order, and says why each window is there; the **shuffle** gives each
//! requirement the windows the map assigned to it and RingFrame's own
//! evidence for it; **reduce** decides each requirement from that; **confirm**
//! re-reads, afresh and on the strong tier, every decisive negative and every
//! requirement whose evidence carries a finding. RingFrame merges the rest.
//!
//! `eval next` says which tasks are ready and writes each one's file, with
//! its evidence inline; a judge writes its output and runs `eval submit`,
//! which checks every citation against its source before anything counts.
//! No model decides the order, and no model writes the report.
//!
//! Working files live under `.fab7/rf/tmp/eval-<id>/`; every submitted output
//! is published as an artifact with an `eval.task` event, refused ones too, so
//! a fabrication stays on record after the judge corrects it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::evaluate::{EvalError, array_of, git, ledger, need, round2, str_of};
use crate::workspace::Workspace;
use crate::{sessions, store};

pub const TASK_SCHEMA: &str = "ringframe.eval-task/1";
pub const RECORD2_SCHEMA: &str = "ringframe.eval/2";
/// Window text in one map leaf, and in one reduce leaf.
const LEAF_BYTES: usize = 96 * 1024;
/// Requirements in one reduce leaf.
const REDUCE_MAX: usize = 12;
/// Evidence one requirement brings to a reduce leaf; more is split into parts.
const REQUIREMENT_BYTES: usize = 40 * 1024;
/// Window text in one confirm task.
const CONFIRM_BYTES: usize = 32 * 1024;
/// A window counts as evidence for at most this many of the requirements a
/// map says it serves, in the order the map gave them.
const SERVES_MAX: usize = 2;
/// At most this many requirements in one confirm task.
const CONFIRM_MAX: usize = 6;
/// At most this much of a requirement's evidence is RingFrame's own, beside
/// what the map gave it: enough to catch what the map missed, never the bulk.
const EXTRA_BYTES: usize = 16 * 1024;
/// How much of a requirement's text the map's index shows.
const INDEX_TEXT: usize = 160;
/// Submissions a task gets: the first and two corrections.
const ATTEMPTS: usize = 3;
/// A task handed out and not submitted in this long is handed out once more.
const TIMEOUT_SECS: i64 = 15 * 60;
pub const DEFAULT_PARALLEL: usize = 8;
const QUOTE_MIN: usize = 8;
const QUOTE_MAX: usize = 300;
/// The roles, and the tier each wants from a plugin's configuration.
pub const ROLES: [&str; 3] = ["map", "reduce", "confirm"];

/// The agent definition a role's tasks are spawned as: `rf-map`,
/// `rf-reduce`, `rf-confirm`, each a Markdown file of the harness's plugin.
pub fn agent_name(role: &str) -> String {
    format!("rf-{role}")
}

/// Each role's `model` and `effort`, from the agent definitions in `dir`:
/// `rf-<role>.md`, or `rf-<role>/agent.md`, with the two in its frontmatter.
/// A role with no file, or a key its file leaves out, is left unset.
pub fn agents_from(dir: &Path) -> Result<Value, String> {
    let mut out = serde_json::Map::new();
    for role in ROLES {
        let name = agent_name(role);
        let Some(text) = [dir.join(format!("{name}.md")), dir.join(&name).join("agent.md")]
            .iter()
            .find_map(|p| std::fs::read_to_string(p).ok())
        else {
            continue;
        };
        let mut keys = serde_json::Map::new();
        for (k, v) in frontmatter(&text) {
            if (k == "model" || k == "effort") && !v.is_empty() {
                keys.insert(k, json!(v));
            }
        }
        out.insert(role.to_string(), Value::Object(keys));
    }
    if out.is_empty() {
        return Err(format!(
            "{} holds no agent definition (rf-map, rf-reduce, rf-confirm)",
            dir.display()
        ));
    }
    Ok(Value::Object(out))
}

/// The `key: value` lines of a Markdown file's frontmatter.
fn frontmatter(text: &str) -> Vec<(String, String)> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Vec::new();
    }
    lines
        .take_while(|l| l.trim() != "---")
        .filter(|l| !l.starts_with([' ', '\t', '#', '-']))
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| {
            let v = v.split(" #").next().unwrap_or(v).trim().trim_matches(['"', '\'']);
            (k.trim().to_string(), v.to_string())
        })
        .collect()
}

/// `given` over `defaults`, key by key: what the person or a tool asked for
/// wins over the harness's agent definitions.
pub fn agents_over(given: Option<Value>, defaults: Option<Value>) -> Option<Value> {
    let Some(mut base) = defaults.map(|d| current_roles(&d)) else { return given };
    let Some(given) = given else { return Some(base) };
    for (role, keys) in current_roles(&given).as_object().into_iter().flatten() {
        let slot = &mut base[role.as_str()];
        if !slot.is_object() {
            *slot = json!({});
        }
        for (k, v) in keys.as_object().into_iter().flatten() {
            slot[k] = v.clone();
        }
    }
    Some(base)
}

/// `agents` with the earlier role names on the current ones: `trace` and
/// `drift` read windows (`map`), `coverage` judged requirements (`reduce`),
/// `adversary` looked for failures (`confirm`). `intent` and `context` no
/// longer run. A current name wins over an earlier one.
pub fn current_roles(agents: &Value) -> Value {
    let mut out = serde_json::Map::new();
    let Some(given) = agents.as_object() else { return json!({}) };
    for (role, keys) in given {
        let now = match role.as_str() {
            "trace" | "drift" => "map",
            "coverage" => "reduce",
            "adversary" => "confirm",
            r if ROLES.contains(&r) => r,
            _ => continue,
        };
        let slot = out.entry(now).or_insert_with(|| json!({}));
        for (k, v) in keys.as_object().into_iter().flatten() {
            if ROLES.contains(&role.as_str()) || slot.get(k).is_none() {
                slot[k] = v.clone();
            }
        }
    }
    Value::Object(out)
}

// ---- what a task is -----------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Task {
    pub id: String,
    pub kind: &'static str,
    pub role: &'static str,
    pub obligations: Vec<Value>,
    pub windows: Vec<String>,
}

struct Ctx {
    ws: Workspace,
    eval_id: String,
    brief: Value,
    evidence: Value,
    windows: BTreeMap<String, String>,
    /// The Eval's view of the change DAG (`changes.json`).
    changes: Value,
}

fn tmp(ws: &Workspace, eval_id: &str) -> PathBuf {
    ws.rf_dir().join("tmp").join(format!("eval-{eval_id}"))
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

impl Ctx {
    fn load(ws: &Workspace, eval_id: &str) -> Result<Ctx, EvalError> {
        let dir = ws.rf_dir().join(format!("evals/{eval_id}"));
        let brief = read_json(&dir.join("brief.json"))
            .ok_or_else(|| ledger("eval.missing", format!("{eval_id} is not an Eval here")))?;
        let evidence = read_json(&dir.join("evidence.json")).ok_or_else(|| {
            ledger(
                "eval.no_evidence",
                format!("{eval_id} was opened before tasks existed; close it with eval close"),
            )
        })?;
        let windows = read_json(&dir.join("windows.json"))
            .and_then(|w| w["windows"].as_object().cloned())
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| (k, v.as_str().unwrap_or_default().to_string()))
            .collect();
        let changes = read_json(&dir.join("changes.json")).ok_or_else(|| {
            ledger(
                "eval.no_changes",
                format!(
                    "{eval_id} has no change DAG (its brief's limitations say why); open a new \
                     Eval over its Asks"
                ),
            )
        })?;
        let mut changes = changes;
        // The checks' warnings on added lines, once they are in: findings on
        // their windows. They name no requirement, so the plan is the same
        // whether or not they are in yet.
        if let (Some(c), Some(list)) =
            (crate::checks::results(ws, eval_id), changes["findings"].as_array_mut())
        {
            list.extend(array_of(&c, "findings").iter().cloned());
        }
        Ok(Ctx { ws: ws.clone(), eval_id: eval_id.to_string(), brief, evidence, windows, changes })
    }

    /// What the judges may cite as a fact: the commands the working agent
    /// ran, and the checks RingFrame ran once they are in.
    fn facts(&self) -> Vec<Value> {
        let mut out = array_of(&self.brief, "facts").to_vec();
        if let Some(c) = crate::checks::results(&self.ws, &self.eval_id) {
            out.extend(array_of(&c, "facts").iter().cloned());
        }
        out
    }

    fn requirements(&self) -> Vec<&Value> {
        array_of(&self.changes, "requirements").iter().collect()
    }

    fn requirement(&self, id: &str) -> Option<&Value> {
        array_of(&self.changes, "requirements").iter().find(|r| r["id"] == id)
    }

    fn window_path(&self, id: &str) -> String {
        array_of(&self.evidence, "windows")
            .iter()
            .find(|w| w["id"] == id)
            .map(|w| str_of(w, "path"))
            .unwrap_or_default()
    }

    /// The units a window belongs to.
    fn units_of(&self, window: &str) -> Vec<&Value> {
        array_of(&self.changes, "units")
            .iter()
            .filter(|u| array_of(u, "windows").iter().any(|w| w == window))
            .collect()
    }

    /// The findings that concern a window: the ones that name it, or, for a
    /// finding that names no window, the ones on its units.
    fn findings_on(&self, window: &str) -> Vec<&Value> {
        let keys: BTreeSet<String> =
            self.units_of(window).iter().map(|u| str_of(u, "key")).collect();
        array_of(&self.changes, "findings")
            .iter()
            .filter(|f| match array_of(f, "windows") {
                [] => array_of(f, "units")
                    .iter()
                    .any(|u| keys.contains(u.as_str().unwrap_or_default())),
                ws => ws.iter().any(|w| w == window),
            })
            .collect()
    }

    /// The windows where a commit no step claims wrote over work a step
    /// claims, for that step: evidence for it whatever a map called them.
    fn undoing(&self, req: &str) -> Vec<String> {
        let mut out = Vec::new();
        for f in array_of(&self.changes, "findings").iter().filter(|f| {
            f["kind"] == "unclaimed_rewrite" && array_of(f, "requirements").iter().any(|r| r == req)
        }) {
            for w in array_of(f, "windows").iter().map(str_of_value) {
                if !out.contains(&w) {
                    out.push(w);
                }
            }
        }
        out
    }

    /// The findings that name a requirement.
    fn findings_naming(&self, req: &str) -> Vec<&Value> {
        array_of(&self.changes, "findings")
            .iter()
            .filter(|f| array_of(f, "requirements").iter().any(|r| r == req))
            .collect()
    }

    fn judged_windows(&self) -> Vec<&Value> {
        array_of(&self.evidence, "windows")
            .iter()
            .filter(|w| !crate::evidence::MECHANICAL.contains(&str_of(w, "class").as_str()))
            .collect()
    }

    fn window_bytes(&self, id: &str) -> usize {
        self.windows.get(id).map_or(0, String::len)
    }
}

/// Every `eval.task` event of this Eval, in order.
fn outcomes(ws: &Workspace, eval_id: &str) -> Result<Vec<Value>, EvalError> {
    Ok(store::events(ws)?
        .into_iter()
        .filter(|e| e["type"] == "eval.task" && e["data"]["eval_id"] == eval_id)
        .collect())
}

struct Status {
    accepted: BTreeMap<String, Value>,
    failed: BTreeSet<String>,
    attempts: BTreeMap<String, usize>,
    /// Windows of code moved away, and the window each follows from.
    moved: BTreeMap<String, String>,
    /// What the last judged Eval of these Asks settled and this one keeps.
    carried: Carried,
}

/// What the last judged Eval of these Asks settled, found by content, so this
/// one judges only what changed (ADR-0020 D9): a window whose text is the
/// same keeps its readings, and a requirement whose evidence is the same
/// keeps its verdict.
#[derive(Default)]
struct Carried {
    from: Option<String>,
    /// This Eval's window id → the readings it keeps, by role.
    windows: BTreeMap<String, BTreeMap<String, Value>>,
    /// Evidence key → the item it settled.
    items: BTreeMap<String, Value>,
}

/// A window's text, without the line numbers a change elsewhere in its file
/// shifts: what makes two windows the same.
fn window_key(text: &str) -> String {
    let norm: Vec<String> = text
        .lines()
        .map(|l| {
            if l.starts_with("@@") {
                // Drop the numbers, keep the item git names after them.
                let item = l.splitn(3, "@@").nth(2).unwrap_or_default();
                format!("@@{item}")
            } else if l.starts_with("~ moved ") {
                l.rsplit_once(':').map_or(l, |(head, _)| head).to_string()
            } else {
                l.to_string()
            }
        })
        .collect();
    crate::digest::sha256_bytes(norm.join("\n").as_bytes())[..16].to_string()
}

/// A requirement's evidence, as a key: its text and "Done when", the text of
/// its windows, and what RingFrame found on them or on it. The same key in a
/// later Eval means nothing it was judged on has changed.
fn evidence_key(ctx: &Ctx, id: &str, ev: &[String]) -> String {
    let r = ctx.requirement(id).cloned().unwrap_or(json!({}));
    let mut windows: Vec<String> =
        ev.iter().filter_map(|w| ctx.windows.get(w)).map(|t| window_key(t)).collect();
    windows.sort();
    let mut found: Vec<String> = ev
        .iter()
        .flat_map(|w| ctx.findings_on(w))
        .chain(ctx.findings_naming(id))
        .map(|f| format!("{}\0{}", str_of(f, "kind"), str_of(f, "detail")))
        .collect();
    found.sort();
    found.dedup();
    let doc = json!({"id": id, "text": r["text"], "done_when": r["done_when"],
                     "windows": windows, "found": found});
    crate::digest::sha256_bytes(&store::canonical(&doc))[..16].to_string()
}

/// The latest earlier Eval of any of these Asks that closed and was not
/// voided, and what of it this Eval keeps.
fn carried(ctx: &Ctx) -> Carried {
    let ws = &ctx.ws;
    let Ok(events) = store::events(ws) else { return Carried::default() };
    let voided = crate::evaluate::voided(ws).unwrap_or_default();
    let asks: BTreeSet<String> =
        array_of(&ctx.brief, "asks").iter().map(|a| str_of(a, "ask_id")).collect();
    let before = events
        .iter()
        .position(|e| e["type"] == "eval.opened" && str_of(e, "id") == ctx.eval_id)
        .unwrap_or(events.len());
    let Some((from, rec)) =
        events[..before].iter().rev().filter(|e| e["type"] == "eval.opened").find_map(|e| {
            let id = str_of(e, "id");
            let basis: BTreeSet<String> =
                array_of(&e["data"]["basis"], "asks").iter().map(str_of_value).collect();
            if basis.is_disjoint(&asks) || voided.contains_key(&id) {
                return None;
            }
            let rec = crate::evaluate::load_record(ws, &id).ok()??;
            (rec["schema"] == RECORD2_SCHEMA).then_some((id, rec))
        })
    else {
        return Carried::default();
    };
    let texts = read_json(&ws.rf_dir().join(format!("evals/{from}/windows.json")))
        .map(|d| d["windows"].clone())
        .unwrap_or_default();
    let key_of_prev = |w: &str| texts[w].as_str().map(window_key);
    let now: BTreeMap<String, String> = ctx
        .judged_windows()
        .iter()
        .map(|w| str_of(w, "id"))
        .filter_map(|id| ctx.windows.get(&id).map(|t| (window_key(t), id)))
        .collect();
    let mut out = Carried { from: Some(from.clone()), ..Default::default() };
    for w in array_of(&rec, "windows") {
        if matches!(w["result"].as_str(), Some("not_judged" | "mechanical") | None) {
            continue;
        }
        let Some(here) = key_of_prev(&str_of(w, "id")).and_then(|k| now.get(&k)) else { continue };
        let mut kept = BTreeMap::new();
        for (role, r) in w["readings"].as_object().into_iter().flatten() {
            let mut r = r.clone();
            if let Some(f) = r["follows_from"].as_str() {
                let Some(to) = key_of_prev(f).and_then(|k| now.get(&k)) else { continue };
                r["follows_from"] = json!(to);
            }
            if let Some(serves) = r["serves"].as_array() {
                let still: Vec<Value> = serves
                    .iter()
                    .filter(|s| ctx.requirement(&str_of_value(s)).is_some())
                    .cloned()
                    .collect();
                if still.is_empty() {
                    continue;
                }
                r["serves"] = json!(still);
            }
            r["window"] = json!(here);
            r["carried_from"] = json!(from);
            kept.insert(role.clone(), r);
        }
        if kept.contains_key("map") {
            out.windows.insert(here.clone(), kept);
        }
    }
    for i in array_of(&rec, "items") {
        if let Some(k) = i["evidence_key"].as_str()
            && matches!(i["result"].as_str(), Some("met" | "not_met"))
        {
            out.items.insert(k.to_string(), i.clone());
        }
    }
    out
}

fn status(ctx: &Ctx) -> Result<Status, EvalError> {
    let moved = ctx
        .judged_windows()
        .iter()
        .filter_map(|w| {
            let id = str_of(w, "id");
            moved_to(ctx, &id).map(|to| (id, to))
        })
        .collect();
    let mut st = Status {
        accepted: BTreeMap::new(),
        failed: BTreeSet::new(),
        attempts: BTreeMap::new(),
        moved,
        carried: carried(ctx),
    };
    for e in outcomes(&ctx.ws, &ctx.eval_id)? {
        let d = &e["data"];
        let id = str_of(d, "task_id");
        match str_of(d, "outcome").as_str() {
            "accepted" => {
                let path = ctx.ws.rf_dir().join(str_of(&d["artifact"], "path"));
                st.accepted.insert(id, read_json(&path).unwrap_or(Value::Null));
            }
            "failed" => {
                st.failed.insert(id);
            }
            _ => {}
        }
        if d["artifact"].is_object() {
            *st.attempts.entry(str_of(d, "task_id")).or_default() += 1;
        }
    }
    Ok(st)
}

// ---- the plan -------------------------------------------------------------------

pub fn task_id(eval_id: &str, local: &str) -> String {
    format!("{eval_id}~{local}")
}

fn eval_of(task: &str) -> Option<&str> {
    task.split_once('~').map(|(e, _)| e)
}

/// The requirements a reduce judges: local ones, and cross-cutting ones a map
/// said a window breaks.
fn local_ids(ctx: &Ctx) -> Vec<String> {
    ctx.requirements().iter().filter(|r| r["kind"] == "local").map(|r| str_of(r, "id")).collect()
}

/// The map's leaves: the change's windows in the file tree's order, a unit's
/// windows kept together, each window once, packed to `LEAF_BYTES`; the
/// leaves holding more of RingFrame's findings go first.
fn map_leaves(ctx: &Ctx, st: &Status) -> Vec<Vec<String>> {
    // Code moved away needs no judge: RingFrame answers it.
    let judged: BTreeSet<String> = ctx
        .judged_windows()
        .iter()
        .map(|w| str_of(w, "id"))
        .filter(|w| !st.moved.contains_key(w) && !st.carried.windows.contains_key(w))
        .collect();
    let mut units: Vec<&Value> =
        array_of(&ctx.changes, "units").iter().filter(|u| u["class"] != "mechanical").collect();
    units.sort_by(|a, b| {
        (str_of(a, "path"), str_of(a, "key")).cmp(&(str_of(b, "path"), str_of(b, "key")))
    });
    // Each unit's windows, in order; a window no unit holds is its own.
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut placed: BTreeSet<String> = BTreeSet::new();
    for u in units {
        let group: Vec<String> = array_of(u, "windows")
            .iter()
            .map(str_of_value)
            .filter(|w| judged.contains(w) && placed.insert(w.clone()))
            .collect();
        if !group.is_empty() {
            groups.push(group);
        }
    }
    for id in &judged {
        if placed.insert(id.clone()) {
            groups.push(vec![id.clone()]);
        }
    }
    // A leaf ends between units; only a unit larger than a leaf is cut.
    let mut leaves: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut bytes = 0;
    for group in groups {
        let size: usize = group.iter().map(|w| ctx.window_bytes(w)).sum();
        if !cur.is_empty() && bytes + size > LEAF_BYTES {
            leaves.push(std::mem::take(&mut cur));
            bytes = 0;
        }
        for w in group {
            let b = ctx.window_bytes(&w);
            if !cur.is_empty() && bytes + b > LEAF_BYTES {
                leaves.push(std::mem::take(&mut cur));
                bytes = 0;
            }
            bytes += b;
            cur.push(w);
        }
    }
    if !cur.is_empty() {
        leaves.push(cur);
    }
    let risk = |l: &Vec<String>| l.iter().map(|w| ctx.findings_on(w).len()).sum::<usize>();
    let mut ranked: Vec<(usize, Vec<String>)> = leaves.into_iter().enumerate().collect();
    ranked.sort_by(|(ia, a), (ib, b)| risk(b).cmp(&risk(a)).then(ia.cmp(ib)));
    ranked.into_iter().map(|(_, l)| l).collect()
}

/// Where a window's code went, when the window is nothing but code moved away:
/// removed lines and `~ moved N lines to <path>:<line>` markers, and no added
/// line. It follows from the window at that path in its own unit (else the
/// first at that path), settled here without a judge.
fn moved_to(ctx: &Ctx, w: &str) -> Option<String> {
    let text = ctx.windows.get(w)?;
    let body: Vec<&str> =
        text.lines().filter(|l| !l.starts_with("--- ") && !l.starts_with("@@")).collect();
    if body.iter().any(|l| l.starts_with('+')) {
        return None;
    }
    let dest = body.iter().find_map(|l| {
        let rest = l.strip_prefix("~ moved ")?;
        let to = rest.split(" to ").nth(1)?;
        Some(to.rsplit_once(':').map_or(to, |(p, _)| p).to_string())
    })?;
    let own: Vec<String> = ctx
        .units_of(w)
        .iter()
        .flat_map(|u| array_of(u, "windows").iter().map(str_of_value))
        .collect();
    let at_dest = |id: &String| id != w && ctx.window_path(id) == dest;
    own.iter()
        .find(|id| at_dest(id))
        .cloned()
        .or_else(|| ctx.judged_windows().iter().map(|x| str_of(x, "id")).find(|id| at_dest(id)))
}

/// What the map said of each window, from accepted map outputs, by role;
/// code moved away is answered by RingFrame (`moved_to`).
fn map_answers(st: &Status) -> BTreeMap<String, BTreeMap<String, Value>> {
    let mut out: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    for (w, to) in &st.moved {
        out.entry(w.clone())
            .or_default()
            .insert("map".into(), json!({"window": w, "follows_from": to, "by": "ringframe"}));
    }
    for (w, readings) in &st.carried.windows {
        let slot = out.entry(w.clone()).or_default();
        for (role, r) in readings {
            slot.entry(role.clone()).or_insert_with(|| r.clone());
        }
    }
    for doc in st.accepted.values().filter(|o| o["kind"] == "map") {
        let role = str_of(&doc["judge"], "role");
        for w in array_of(doc, "windows") {
            out.entry(str_of(w, "window")).or_default().insert(role.clone(), w.clone());
        }
    }
    out
}

/// The requirements a window serves, following `follows_from`.
fn served(answers: &BTreeMap<String, BTreeMap<String, Value>>, w: &str) -> Vec<String> {
    let mut cur = w.to_string();
    for _ in 0..5 {
        let Some(a) = answers.get(&cur).and_then(|r| r.get("map")) else { return Vec::new() };
        // The first two only: a window listed for every step it touches
        // makes each step's evidence large and splits it into parts (q03,
        // once the map stopped quoting: windows serving two or more steps
        // went from 45 to 95, and reduce tasks from 15 to 28).
        if let Some(s) = a["serves"].as_array().filter(|s| !s.is_empty()) {
            return s.iter().take(SERVES_MAX).map(str_of_value).collect();
        }
        // A window that undoes a step's work is evidence for that step.
        if let Some(u) = a["undoes"].as_array().filter(|u| !u.is_empty()) {
            return u.iter().take(SERVES_MAX).map(str_of_value).collect();
        }
        match a["follows_from"].as_str() {
            Some(next) if next != cur => cur = next.to_string(),
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

/// Each requirement's evidence, each window once, up to `bytes`: first the
/// windows the map assigned to it and those it said break it, all of them
/// (a requirement whose own windows are over a leaf is judged in parts); then
/// RingFrame's own, from its strongest units and the units in paths it names,
/// up to `EXTRA_BYTES` and only while the whole stays within
/// `REQUIREMENT_BYTES`, so they never make parts. Of RingFrame's, windows that carry a finding come first, and a
/// window the map gave another requirement is left out unless it carries one.
fn evidence_for(ctx: &Ctx, st: &Status, req: &str, bytes: usize) -> Vec<String> {
    let answers = map_answers(st);
    let judged: BTreeSet<String> = ctx.judged_windows().iter().map(|w| str_of(w, "id")).collect();
    let mut own: Vec<String> = Vec::new();
    let mut elsewhere: BTreeSet<String> = BTreeSet::new();
    for id in &judged {
        let serves = served(&answers, id);
        if serves.iter().any(|r| r == req) {
            own.push(id.clone());
        } else if !serves.is_empty() {
            elsewhere.insert(id.clone());
        }
    }
    // Windows a map said break a cross-cutting requirement.
    for doc in st.accepted.values().filter(|o| o["kind"] == "map") {
        for b in array_of(doc, "breaks") {
            if b["requirement"] == req {
                own.push(str_of(b, "window"));
            }
        }
    }
    // Windows that undo work bearing on it, whatever the map called them.
    own.extend(ctx.undoing(req));
    let units = array_of(&ctx.changes, "units");
    let windows_of = |key: &str| -> Vec<String> {
        units
            .iter()
            .find(|u| u["key"] == key)
            .map_or(Vec::new(), |u| array_of(u, "windows").iter().map(str_of_value).collect())
    };
    let mut theirs: Vec<String> = Vec::new();
    for l in array_of(&ctx.changes["links"]["evidence"], req) {
        theirs.extend(windows_of(&str_of(l, "unit")));
    }
    if let Some(r) = ctx.requirement(req) {
        let paths: BTreeSet<String> = array_of(r, "paths").iter().map(str_of_value).collect();
        for u in units {
            if crate::changes::names_path(&paths, &str_of(u, "path")) {
                theirs.extend(array_of(u, "windows").iter().map(str_of_value));
            }
        }
    }
    let found = |w: &String| !ctx.findings_on(w).is_empty();
    theirs.retain(|w| found(w) || !elsewhere.contains(w));
    theirs.sort_by_key(|w| !found(w));
    let mut out: Vec<String> = Vec::new();
    let (mut size, mut extra) = (0, 0);
    for (w, mine) in
        own.into_iter().map(|w| (w, true)).chain(theirs.into_iter().map(|w| (w, false)))
    {
        if !judged.contains(&w) || out.contains(&w) {
            continue;
        }
        let b = ctx.window_bytes(&w);
        let fits = if mine {
            size + b <= bytes
        } else {
            size + b <= bytes.min(REQUIREMENT_BYTES) && extra + b <= EXTRA_BYTES
        };
        if !fits && !out.is_empty() {
            continue;
        }
        if !mine {
            extra += b;
        }
        size += b;
        out.push(w);
    }
    out
}

/// A requirement as a reduce or confirm task carries it.
fn requirement_entry(
    ctx: &Ctx,
    id: &str,
    windows: &[String],
    part: Option<(usize, usize)>,
) -> Value {
    let r = ctx.requirement(id).cloned().unwrap_or(json!({"id": id}));
    let mut e = json!({"id": id, "kind": r["kind"], "text": r["text"], "done_when": r["done_when"],
                       "windows": windows});
    if let Some((k, n)) = part {
        e["part"] = json!(format!("{k}/{n}"));
    }
    e
}

/// The requirements a reduce judges, and each one's evidence.
fn reduce_targets(ctx: &Ctx, st: &Status) -> Vec<(String, Vec<String>)> {
    let answers = map_answers(st);
    every_target(ctx, st).into_iter().filter(|(id, ev)| !keeps(ctx, st, &answers, id, ev)).collect()
}

/// Whether a requirement keeps the verdict an earlier Eval gave it: its
/// evidence key is one that Eval settled, and no window the graph ties to it
/// (its linked units, the units in paths it names) is new here and was left
/// without an answer, since then its evidence may lack what changed.
fn keeps(
    ctx: &Ctx,
    st: &Status,
    answers: &BTreeMap<String, BTreeMap<String, Value>>,
    id: &str,
    ev: &[String],
) -> bool {
    if !st.carried.items.contains_key(&evidence_key(ctx, id, ev)) {
        return false;
    }
    let units = array_of(&ctx.changes, "units");
    let mut tied: Vec<String> = Vec::new();
    for l in array_of(&ctx.changes["links"]["evidence"], id) {
        if let Some(u) = units.iter().find(|u| u["key"] == l["unit"]) {
            tied.extend(array_of(u, "windows").iter().map(str_of_value));
        }
    }
    if let Some(r) = ctx.requirement(id) {
        let paths: BTreeSet<String> = array_of(r, "paths").iter().map(str_of_value).collect();
        for u in units.iter().filter(|u| crate::changes::names_path(&paths, &str_of(u, "path"))) {
            tied.extend(array_of(u, "windows").iter().map(str_of_value));
        }
    }
    let judged: BTreeSet<String> = ctx.judged_windows().iter().map(|w| str_of(w, "id")).collect();
    tied.iter().all(|w| !judged.contains(w) || answers.contains_key(w))
}

/// The items the last judged Eval settled on the same evidence, kept as they
/// were and marked with where they came from.
fn carried_items(ctx: &Ctx, st: &Status) -> Vec<Value> {
    let answers = map_answers(st);
    every_target(ctx, st)
        .into_iter()
        .filter_map(|(id, ev)| {
            if !keeps(ctx, st, &answers, &id, &ev) {
                return None;
            }
            let key = evidence_key(ctx, &id, &ev);
            let mut item = st.carried.items.get(&key)?.clone();
            item["carried_from"] = json!(st.carried.from);
            Some(item)
        })
        .collect()
}

/// Every requirement a reduce would judge, with its evidence.
fn every_target(ctx: &Ctx, st: &Status) -> Vec<(String, Vec<String>)> {
    let mut ids = local_ids(ctx);
    for doc in st.accepted.values().filter(|o| o["kind"] == "map") {
        for b in array_of(doc, "breaks") {
            let r = str_of(b, "requirement");
            if !ids.contains(&r) {
                ids.push(r);
            }
        }
    }
    ids.into_iter()
        .map(|id| {
            let ev = evidence_for(ctx, st, &id, usize::MAX);
            (id, ev)
        })
        .collect()
}

/// The reduce's leaves: requirements in plan order with their evidence,
/// packed to `LEAF_BYTES` and `REDUCE_MAX`; a requirement whose evidence is
/// over `REQUIREMENT_BYTES` is split into parts, each judged in its own leaf.
fn reduce_leaves(ctx: &Ctx, st: &Status) -> Vec<Vec<Value>> {
    let mut leaves: Vec<Vec<Value>> = Vec::new();
    let mut cur: Vec<Value> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut bytes = 0;
    let flush = |cur: &mut Vec<Value>,
                 seen: &mut BTreeSet<String>,
                 bytes: &mut usize,
                 leaves: &mut Vec<Vec<Value>>| {
        if !cur.is_empty() {
            leaves.push(std::mem::take(cur));
            seen.clear();
            *bytes = 0;
        }
    };
    for (id, ev) in reduce_targets(ctx, st) {
        let total: usize = ev.iter().map(|w| ctx.window_bytes(w)).sum();
        if total > REQUIREMENT_BYTES {
            // Parts, each alone in a leaf.
            let mut parts: Vec<Vec<String>> = Vec::new();
            let mut p: Vec<String> = Vec::new();
            let mut pb = 0;
            for w in ev {
                let b = ctx.window_bytes(&w);
                if !p.is_empty() && pb + b > REQUIREMENT_BYTES {
                    parts.push(std::mem::take(&mut p));
                    pb = 0;
                }
                pb += b;
                p.push(w);
            }
            if !p.is_empty() {
                parts.push(p);
            }
            let n = parts.len();
            for (k, part) in parts.into_iter().enumerate() {
                flush(&mut cur, &mut seen, &mut bytes, &mut leaves);
                leaves.push(vec![requirement_entry(ctx, &id, &part, Some((k + 1, n)))]);
            }
            continue;
        }
        let add: usize =
            ev.iter().filter(|w| !seen.contains(*w)).map(|w| ctx.window_bytes(w)).sum();
        if !cur.is_empty() && (cur.len() >= REDUCE_MAX || bytes + add > LEAF_BYTES) {
            flush(&mut cur, &mut seen, &mut bytes, &mut leaves);
        }
        bytes +=
            ev.iter().filter(|w| !seen.contains(*w)).map(|w| ctx.window_bytes(w)).sum::<usize>();
        seen.extend(ev.iter().cloned());
        cur.push(requirement_entry(ctx, &id, &ev, None));
    }
    flush(&mut cur, &mut seen, &mut bytes, &mut leaves);
    leaves
}

fn windows_of_entries(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .flat_map(|e| array_of(e, "windows").iter().map(str_of_value))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The reduce's votes on each requirement, one per part, from accepted outputs.
fn reduce_votes(st: &Status, role: &str) -> BTreeMap<String, Vec<Value>> {
    let mut out: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for doc in st.accepted.values().filter(|o| o["kind"] == "reduce" && o["judge"]["role"] == role)
    {
        for v in array_of(doc, "requirements") {
            out.entry(str_of(v, "id")).or_default().push(v.clone());
        }
    }
    out
}

/// One requirement's result from its parts' votes: `not_met` if any part is
/// not met, `met` when every part is met (or met for its part) and one is
/// met, else unknown.
fn combine(parts: &[Value]) -> &'static str {
    let votes: Vec<&str> = parts.iter().map(|v| v["vote"].as_str().unwrap_or("unknown")).collect();
    if votes.contains(&"not_met") {
        "not_met"
    } else if !votes.is_empty()
        && votes.iter().all(|v| matches!(*v, "met" | "part_met"))
        && votes.contains(&"met")
    {
        "met"
    } else {
        "unknown"
    }
}

/// Confirms packed as the reduce is: each requirement
/// still judged on its own windows, by a judge not shown the other votes.
fn confirm_batches(ctx: &Ctx, confirms: Vec<(Value, Vec<String>)>) -> Vec<Vec<Value>> {
    let mut batches: Vec<Vec<Value>> = Vec::new();
    let (mut cur, mut seen, mut bytes) = (Vec::new(), BTreeSet::<String>::new(), 0);
    for (entry, ev) in confirms {
        let add: usize =
            ev.iter().filter(|w| !seen.contains(*w)).map(|w| ctx.window_bytes(w)).sum();
        if !cur.is_empty() && (cur.len() >= CONFIRM_MAX || bytes + add > LEAF_BYTES) {
            batches.push(std::mem::take(&mut cur));
            seen.clear();
            bytes = 0;
        }
        bytes +=
            ev.iter().filter(|w| !seen.contains(*w)).map(|w| ctx.window_bytes(w)).sum::<usize>();
        seen.extend(ev.iter().cloned());
        cur.push(entry);
    }
    if !cur.is_empty() {
        batches.push(cur);
    }
    batches
}

/// The votes of these tasks' accepted outputs, by requirement.
fn votes_of(st: &Status, tasks: &[Task]) -> BTreeMap<String, Vec<Value>> {
    let mut out: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for t in tasks {
        for v in st.accepted.get(&t.id).map_or(&[][..], |o| array_of(o, "requirements")) {
            out.entry(str_of(v, "id")).or_default().push(v.clone());
        }
    }
    out
}

/// Every task the Eval needs, given what is in so far. A later stage appears
/// only once the earlier one is all in, so the plan is the same every time it
/// is computed from the same record.
fn plan(ctx: &Ctx, st: &Status) -> Vec<Task> {
    let e = &ctx.eval_id;
    let done = |id: &str| st.accepted.contains_key(id) || st.failed.contains(id);
    let index: Vec<Value> = ctx
        .requirements()
        .iter()
        .filter(|r| matches!(r["kind"].as_str(), Some("local" | "cross")))
        .map(|r| {
            let text: String = str_of(r, "text").chars().take(INDEX_TEXT).collect();
            json!({"id": r["id"], "kind": r["kind"], "text": text})
        })
        .collect();
    let mut tasks: Vec<Task> = map_leaves(ctx, st)
        .into_iter()
        .enumerate()
        .map(|(n, windows)| Task {
            id: task_id(e, &format!("m{}", n + 1)),
            kind: "map",
            role: "map",
            obligations: index.clone(),
            windows,
        })
        .collect();
    if !tasks.iter().all(|t| done(&t.id)) {
        return tasks;
    }
    let reduces: Vec<Task> = reduce_leaves(ctx, st)
        .into_iter()
        .enumerate()
        .map(|(n, entries)| Task {
            id: task_id(e, &format!("r{}", n + 1)),
            kind: "reduce",
            role: "reduce",
            windows: windows_of_entries(&entries),
            obligations: entries,
        })
        .collect();
    let reduced = reduces.iter().all(|t| done(&t.id));
    tasks.extend(reduces);
    if !reduced {
        return tasks;
    }
    // Confirm: every decisive negative, every requirement a finding names or
    // whose evidence carries one, and every requirement whose parts disagree;
    // each alone, read afresh from its own evidence.
    let votes = reduce_votes(st, "reduce");
    let mut confirms: Vec<(Value, Vec<String>)> = Vec::new();
    for (id, _) in reduce_targets(ctx, st) {
        let parts = votes.get(&id).cloned().unwrap_or_default();
        let result = combine(&parts);
        let ev = evidence_for(ctx, st, &id, CONFIRM_BYTES);
        // Every first reading that is not `met` (not met, or unknown), every
        // requirement a finding names, and every one whose parts disagree. A
        // `met` whose evidence merely carries a finding is not read again:
        // on q03 those re-reads were two thirds of the confirms and changed
        // one in eight (a balance of the result against time and
        // tokens); the reduce was shown the findings.
        // `no_link` says only that the graph found nothing; a cited `met`
        // from the map's evidence has already cleared it.
        let named = ctx.findings_naming(&id).iter().any(|f| f["kind"] != "no_link");
        let split = parts.len() > 1 && parts.iter().any(|v| v["vote"] != parts[0]["vote"]);
        // Work it asks for was undone: a map said a window undoes it, or a
        // commit no step claims rewrote a commit it claims. A `met` is read
        // again (q04: the reduce voted step 7 met past the undoing window).
        let undone = !ctx.undoing(&id).is_empty()
            || map_answers(st)
                .values()
                .filter_map(|r| r.get("map"))
                .any(|a| array_of(a, "undoes").iter().any(|u| u == &json!(id)));
        if result != "met" || named || split || undone {
            confirms.push((requirement_entry(ctx, &id, &ev, None), ev));
        }
    }
    let firsts: Vec<Task> = confirm_batches(ctx, confirms)
        .into_iter()
        .enumerate()
        .map(|(k, entries)| Task {
            id: task_id(e, &format!("cr{}", k + 1)),
            kind: "reduce",
            role: "confirm",
            windows: windows_of_entries(&entries),
            obligations: entries,
        })
        .collect();
    // A confirm that contradicts the first reading is read once more, by a
    // fresh judge, once every confirm is in: judges differ by model, effort
    // and harness, so neither reading wins alone and the majority decides
    // (`settle`). q06: a correct `not_met` overturned by one confirm.
    if firsts.iter().all(|t| done(&t.id)) {
        let second = votes_of(st, &firsts);
        let decisive = |v: &str| match v {
            "met" | "part_met" => "met",
            "not_met" => "not_met",
            _ => "",
        };
        let mut ties: Vec<(Value, Vec<String>)> = Vec::new();
        for (id, _) in reduce_targets(ctx, st) {
            let first = decisive(combine(&votes.get(&id).cloned().unwrap_or_default()));
            let again = second
                .get(&id)
                .and_then(|v| v.first())
                .map_or("", |c| decisive(c["vote"].as_str().unwrap_or_default()));
            if !first.is_empty() && !again.is_empty() && first != again {
                let ev = evidence_for(ctx, st, &id, CONFIRM_BYTES);
                ties.push((requirement_entry(ctx, &id, &ev, None), ev));
            }
        }
        tasks.extend(firsts);
        for (k, entries) in confirm_batches(ctx, ties).into_iter().enumerate() {
            tasks.push(Task {
                id: task_id(e, &format!("ct{}", k + 1)),
                kind: "reduce",
                role: "confirm",
                windows: windows_of_entries(&entries),
                obligations: entries,
            });
        }
    } else {
        tasks.extend(firsts);
    }
    // A window the map called unexplained is read again.
    let answers = map_answers(st);
    let unexplained: Vec<String> = ctx
        .judged_windows()
        .iter()
        .map(|w| str_of(w, "id"))
        .filter(|w| answers.get(w).is_some_and(needs_window_confirm))
        .collect();
    let mut cur: Vec<String> = Vec::new();
    let mut bytes = 0;
    let mut batches: Vec<Vec<String>> = Vec::new();
    for w in unexplained {
        let b = ctx.window_bytes(&w);
        if !cur.is_empty() && bytes + b > CONFIRM_BYTES {
            batches.push(std::mem::take(&mut cur));
            bytes = 0;
        }
        bytes += b;
        cur.push(w);
    }
    if !cur.is_empty() {
        batches.push(cur);
    }
    for (k, windows) in batches.into_iter().enumerate() {
        tasks.push(Task {
            id: task_id(e, &format!("cw{}", k + 1)),
            kind: "map",
            role: "confirm",
            obligations: index.clone(),
            windows,
        });
    }
    tasks
}

/// A window the map called unexplained is read again, unless an earlier Eval's
/// confirm is carried for it. A confirm given in this Eval keeps the window in
/// its batch, so the batches, and their task ids, do not change as confirms
/// land: with several out at once, a `cw2` must still be `cw2` after `cw1`.
fn needs_window_confirm(a: &BTreeMap<String, Value>) -> bool {
    let carried = a.get("confirm").is_some_and(|c| c.get("carried_from").is_some_and(|f| !f.is_null()));
    !carried && a.get("map").is_some_and(|m| m.get("unexplained").is_some_and(|u| !u.is_null()))
}

// ---- instructions ---------------------------------------------------------------

/// What a judge must not do when no override layer says otherwise. A layer's
/// `eval.deny` replaces the whole list; an empty one denies nothing.
pub const DEFAULT_DENY: &[&str] = &[
    "Change the work: edit, create, move or delete a file of any repository this Eval \
     reads. What a build or a test writes into ignored folders, such as `target/`, is fine.",
    "Change a repository with `git`: commit, amend, checkout, switch, reset, restore, stash, \
     merge, rebase, cherry-pick, tag, branch, clean or push.",
    "Install, upgrade or remove packages, toolchains or tools.",
    "Send the work, its files or any secret to a network service.",
    "Run the project's whole test suite or its full build: RingFrame runs the checks the Asks \
     name, once, and their results are among your recorded facts.",
];

/// The judges' rules as `eval open` records them in the brief.
pub fn judges(deny: Option<&(&str, Vec<String>)>) -> Value {
    match deny {
        Some((from, rules)) => json!({"deny": rules, "from": from}),
        None => json!({"deny": DEFAULT_DENY, "from": "default"}),
    }
}

const COMMON: &str = "\
Everything you judge is in this brief: the requirements, what RingFrame found, \
the recorded facts, and the windows (hunks of the change, each starting at a \
line `=== <window id> <path>`). The windows it hands you count as read: cite \
them without fetching them.

Judge from this brief. Look beyond it only when it cannot settle an answer, \
within your effort's budget: at `low`, no lookup unless a citation needs one; at \
`medium`, up to 5; at `high` and above, up to 15. A lookup is any call beyond \
reading this brief: \
`ringframe eval window --eval <eval> --window <w> --task <task id>`, \
`ringframe eval grep --eval <eval> --text '<text>'`, a file read, a search, \
`git log`, a test run, or any other command. Count them. When the budget is \
spent, answer `unknown` and say what is missing; do not keep looking. Scripts, test programs and other scratch files go under \
`scratch/<task id>/` beside this file. What a test you run shows tells you where \
to look: it is not a citation.

A step's text says what was wrong and what must change; its \"Done when\" is how \
to check it. It is met only when the change does what the text says must change \
and the Done when holds.

Do your task yourself: never hand it, or a part of it, to another agent. Write \
your output file yourself, not with a script: RingFrame checks every quote \
against its source and refuses text repeated across items.

When your output is written, run `ringframe eval submit --task <task id>`. If it \
refuses, fix exactly what it names and submit again. Send no message before \
then: your only message, and your last, is one line: `<task id> accepted` or \
`<task id> failed: <reason>`. Nothing else.

A citation RingFrame can check is one of:
- `{\"window\": \"w_…\", \"quote\": \"…\"}`: 8 to 300 characters copied exactly \
from a window of this brief (or one you looked up with `ringframe eval window`);
- `{\"file\": \"<path as a window names it>\", \"lines\": [first, last], \"quote\": \"…\"}`: \
from that file as it is now;
- `{\"fact\": \"fct_…\"}`: a command the harness recorded, listed in this brief;
- `{\"commits\": \"<repo>\", \"quote\": \"…\"}`: from `ringframe eval commits --eval <eval>`.
";

const MAP_MD: &str = "\
# RingFrame Eval task: map

For every window in this brief, say why that change is there, from the \
requirements this brief lists (every requirement of the Eval, one line \
each). When your role is `confirm`, another reading found no requirement for \
these windows: read them afresh.

Write your output file:

```json
{\"windows\": [
   {\"window\": \"w_…\", \"serves\": [\"<requirement id>\"]},
   {\"window\": \"w_…\", \"follows_from\": \"w_…\"},
   {\"window\": \"w_…\", \"unexplained\": \"<what it does that no requirement asks for>\", \"quote\": \"…\"},
   {\"window\": \"w_…\", \"undoes\": [\"<requirement id>\"], \"quote\": \"…\"}],
 \"breaks\": [{\"requirement\": \"<a cross-cutting requirement id>\", \"window\": \"w_…\",
             \"quote\": \"…\", \"reason\": \"…\"}]}
```

- `serves`: the requirement the window most directly serves, and a second only \
when it serves both as much; never a list of every step it touches. RingFrame \
reads the first two.
- `undoes`: the requirement whose work the window takes back (a fix reverted, a \
check removed, a behaviour a step asks for put back as it was). Where this brief \
lists a window as an unclaimed rewrite (a commit no step claims that wrote over \
claimed work), ask first whether it undoes a requirement: that is `undoes`, not \
`unexplained` and not `follows_from`.
- Exactly one of `serves`, `follows_from` (another window of the Eval it follows \
from: a call site of a function another window adds, an import, a moved piece), \
`unexplained` or `undoes` per window, and every window of the task.
- `quote`, for `unexplained` and `undoes` only: 8 to 300 characters copied \
exactly from that window, showing what no requirement asks for, or what it \
takes back. `serves` and `follows_from` take no quote.
- `breaks`: a cross-cutting requirement (kind `cross`) a window goes against; \
empty when none does.
- Where this brief lists something RingFrame found on a window, say in its \
entry what the window does about it.
";

const REDUCE_MD: &str = "\
# RingFrame Eval task: reduce

Decide, for each requirement in this brief, whether the change meets it, \
from the windows listed for it, each requirement on its own windows. When \
your role is `confirm`, read each requirement afresh, and first answer each \
thing RingFrame found.

Write your output file:

```json
{\"findings\": [{\"finding\": \"<what RingFrame found>\", \"means\": \"<what it means here>\"}],
 \"requirements\": [
   {\"id\": \"<id>\", \"vote\": \"met\", \"reason\": \"<what the evidence shows>\",
    \"citations\": [{\"window\": \"w_…\", \"quote\": \"…\"}]},
   {\"id\": \"<id>\", \"vote\": \"not_met\", \"reason\": \"…\", \"missing\": \"<what is absent>\"},
   {\"id\": \"<id>\", \"vote\": \"unknown\", \"reason\": \"…\", \"missing\": \"<what would settle it>\"}]}
```

- One vote per requirement: `met`, `not_met`, `unknown`, or, for a requirement \
this brief marks as one `part` of several, `part_met` (this part of it is done).
- `met` and `part_met` need at least one citation that checks, showing the \
change doing what the requirement says.
- `not_met` needs a citation showing it does not hold, or `missing` saying what \
the windows listed for it lack. Evidence that it is not met is `not_met`, never \
`unknown`.
- `unknown` needs `missing`.
- `findings` (role `confirm`): one entry for each thing this brief lists \
RingFrame found on this requirement or its windows, before you vote; a `met` \
has to hold despite it.
- Each reason is about its own requirement: one sentence, at most 200 \
characters.
";

fn instructions(kind: &str, deny: &[String]) -> String {
    let body = if kind == "reduce" { REDUCE_MD } else { MAP_MD };
    let rules = if deny.is_empty() {
        String::new()
    } else {
        let items: String = deny.iter().map(|r| format!("- {r}\n")).collect();
        format!("\nYou must not:\n{items}")
    };
    format!("{body}\n{COMMON}{rules}")
}

/// Everything a judge needs, in one file and in reading order: the task,
/// what to do, what to judge, what RingFrame found, and the windows. A judge
/// that had to fetch its instructions, its task file and its windows piece
/// by piece spent half its time reading (q03: 4–8 calls, each a model turn).
fn brief(t: &Task, file: &Value, windows: &str, deny: &[String]) -> String {
    use std::fmt::Write as _;
    let mut b = String::new();
    let _ = writeln!(b, "# RingFrame Eval task `{}`\n", t.id);
    let _ = writeln!(
        b,
        "You are the `{}` judge of this task. Read this whole file before you start: if your \
         file reader stops before the line `=== end of brief ===`, read on from where it \
         stopped.\n",
        t.role
    );
    let _ = writeln!(b, "- Task id: `{}`", t.id);
    let _ = writeln!(b, "- Eval: `{}`", str_of(file, "eval_id"));
    if let Some(e) = file["effort"].as_str() {
        let _ = writeln!(b, "- Your effort: `{e}`");
    }
    let _ = writeln!(b, "- Your output file: `{}`\n", str_of(file, "output"));
    let rules = instructions(t.kind, deny);
    let rules = rules.split_once('\n').map_or(rules.as_str(), |(_, rest)| rest);
    b.push_str("## What to do\n");
    b.push_str(rules);
    b.push_str("\n## Requirements\n\n");
    for r in array_of(file, "requirements") {
        let _ = write!(b, "- `{}` ({}): {}", str_of(r, "id"), str_of(r, "kind"), str_of(r, "text"));
        if let Some(p) = r["part"].as_str() {
            let _ = write!(b, " [part {p}]");
        }
        b.push('\n');
        if let Some(d) = r["done_when"].as_str().filter(|d| !d.is_empty()) {
            let _ = writeln!(b, "  Done when: {d}");
        }
        let ws: Vec<String> = array_of(r, "windows").iter().map(str_of_value).collect();
        if !ws.is_empty() {
            let _ = writeln!(b, "  Its windows: {}", ws.join(", "));
        }
    }
    let found = array_of(file, "found");
    if !found.is_empty() {
        b.push_str("\n## What RingFrame found\n\n");
        for f in found {
            let on: Vec<String> = array_of(f, "windows")
                .iter()
                .map(str_of_value)
                .chain(f["requirement"].as_str().map(str::to_string))
                .collect();
            let _ = writeln!(
                b,
                "- {}: {} (on {})",
                str_of(f, "kind"),
                str_of(f, "detail"),
                on.join(", ")
            );
        }
    }
    let facts = array_of(file, "facts");
    if !facts.is_empty() {
        b.push_str("\n## Recorded facts\n\n");
        for f in facts {
            let _ = writeln!(b, "- `{}`: {}", str_of(f, "id"), f);
        }
    }
    b.push_str("\n## Windows\n\n");
    b.push_str(windows);
    b.push_str("=== end of brief ===\n");
    b
}

/// The brief's rules, or the default for an Eval opened before it had any.
fn deny_of(brief: &Value) -> Vec<String> {
    match brief["judges"]["deny"].as_array() {
        Some(rules) => rules.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        None => DEFAULT_DENY.iter().map(|r| r.to_string()).collect(),
    }
}

// ---- next -----------------------------------------------------------------------

fn state_path(ws: &Workspace, eval_id: &str) -> PathBuf {
    tmp(ws, eval_id).join("state.json")
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Record the harness that runs this Eval's tasks, as its skill names it.
pub fn note_host(ws: &Workspace, eval_id: &str, host: &str) -> Result<(), EvalError> {
    let path = state_path(ws, eval_id);
    let mut state = read_json(&path).unwrap_or_else(|| json!({"handed": {}}));
    let hosts = state["hosts"].as_array().cloned().unwrap_or_default();
    if !hosts.iter().any(|h| h == host) {
        let mut hosts = hosts;
        hosts.push(json!(host));
        state["hosts"] = json!(hosts);
        write(&path, &store::canonical(&state))?;
    }
    Ok(())
}

/// Whether this Eval runs by tasks: `eval next` has been asked for it.
pub fn is_tasked(ws: &Workspace, eval_id: &str) -> bool {
    state_path(ws, eval_id).exists()
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), EvalError> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

/// The task file, and its windows as text. The windows go in a file of their
/// own, as lines: a harness's file reader cuts long lines and reads a long file
/// by line ranges, so one line of JSON holding the change would be read in part.
fn task_file(ctx: &Ctx, t: &Task, facts: &[Value]) -> (Value, String) {
    let dir = tmp(&ctx.ws, &ctx.eval_id);
    let mut v = json!({
        "task_id": t.id, "eval_id": ctx.eval_id, "kind": t.kind, "role": t.role,
        "output": dir.join("out").join(format!("{}.json", t.id)).to_string_lossy(),
        "windows_file": dir.join("tasks").join(format!("{}.windows.txt", t.id)).to_string_lossy(),
    });
    // How hard to work, for a harness that takes no effort per sub-agent: the
    // judge reads it here, and the instructions say what each level means.
    if let Some(effort) = ctx.brief["agents"][t.role]["effort"].as_str() {
        v["effort"] = json!(effort);
    }
    // The evidence, handed over: what RingFrame hands a judge counts as read.
    let unit_names =
        |w: &str| -> Vec<String> { ctx.units_of(w).iter().map(|u| str_of(u, "item")).collect() };
    let mut text = String::new();
    let mut line = 1;
    let mut listed = Vec::new();
    for w in &t.windows {
        let body = ctx.windows.get(w).cloned().unwrap_or_default();
        let body = body.trim_end_matches('\n');
        let head = format!("=== {w} {}", ctx.window_path(w));
        let n = 1 + body.lines().count();
        listed.push(json!({"id": w, "path": ctx.window_path(w), "items": unit_names(w),
                           "lines": [line, line + n - 1]}));
        text.push_str(&head);
        text.push('\n');
        text.push_str(body);
        text.push_str("\n\n");
        line += n + 1;
    }
    v["windows"] = json!(listed);
    // Each finding once, with the windows of this task it concerns.
    let mut found: Vec<Value> = Vec::new();
    for w in &t.windows {
        for f in ctx.findings_on(w) {
            match found.iter_mut().find(|e| {
                e["kind"] == f["kind"] && e["detail"] == f["detail"] && e.get("windows").is_some()
            }) {
                Some(e) => e["windows"].as_array_mut().into_iter().for_each(|a| a.push(json!(w))),
                None => {
                    found.push(json!({"kind": f["kind"], "detail": f["detail"], "windows": [w]}))
                }
            }
        }
    }
    match t.kind {
        "map" => {
            v["requirements"] = json!(t.obligations);
        }
        _ => {
            v["requirements"] = json!(t.obligations);
            for r in &t.obligations {
                for f in ctx.findings_naming(&str_of(r, "id")) {
                    let entry =
                        json!({"kind": f["kind"], "detail": f["detail"], "requirement": r["id"]});
                    if !found.contains(&entry) {
                        found.push(entry);
                    }
                }
            }
            v["facts"] = json!(facts);
        }
    }
    v["found"] = json!(found);
    (v, text)
}

/// The tasks that are ready, at most `parallel` out at once; `done` with the
/// report once every task is in.
/// `agents` are the roles of whoever runs the tasks, filling what `eval open`
/// left unset: an Eval opened by one caller is judged on the tiers of the
/// harness that runs its tasks.
pub fn next(
    ws: &Workspace,
    eval_id: &str,
    parallel: usize,
    agents: Option<&Value>,
    returned: &[String],
) -> Result<Value, EvalError> {
    if crate::evaluate::load_record(ws, eval_id)?.is_some() {
        return Ok(json!({"state": "done", "eval_id": eval_id}));
    }
    need(
        !crate::evaluate::voided(ws)?.contains_key(eval_id),
        "eval.voided",
        format!("{eval_id} was voided"),
    )?;
    let mut ctx = Ctx::load(ws, eval_id)?;
    // What a harness running the tasks gave, kept for the rest of the Eval:
    // the tiers its tasks run on, and what the record says each role ran on.
    let state_file = state_path(ws, eval_id);
    let given = agents.map(current_roles);
    if let Some(g) = &given {
        let mut state = read_json(&state_file).unwrap_or_else(|| json!({"handed": {}}));
        state["agents"] = g.clone();
        write(&state_file, &store::canonical(&state))?;
    }
    let agents = given.or_else(|| read_json(&state_file).map(|s| s["agents"].clone()));
    if let Some(given) = agents.as_ref().and_then(Value::as_object) {
        let mut merged = ctx.brief["agents"].as_object().cloned().unwrap_or_default();
        for (role, keys) in given {
            let slot = merged.entry(role.clone()).or_insert_with(|| json!({}));
            for (k, v) in keys.as_object().into_iter().flatten() {
                if slot.get(k).is_none() {
                    slot[k] = v.clone();
                }
            }
        }
        ctx.brief["agents"] = Value::Object(merged);
    }
    let mut st = status(&ctx)?;
    let mut state = read_json(&state_file).unwrap_or_else(|| json!({"handed": {}}));
    let now = now_secs();
    // A sub-agent that has finished gives its task back, whatever it replied:
    // one with no submission counts as back now, not when its lease ends.
    for id in returned {
        if state["handed"][id].is_array()
            && !st.accepted.contains_key(id)
            && !st.failed.contains(id)
        {
            let slot = &mut state["returned"][id];
            if !slot.is_array() {
                *slot = json!([]);
            }
            slot.as_array_mut().expect("an array").push(json!(now));
        }
    }
    let last_of = |state: &Value, key: &str, id: &str| {
        state[key][id].as_array().and_then(|h| h.last()).and_then(Value::as_i64)
    };
    let is_back = |id: &str, state: &Value| {
        let handed = last_of(state, "handed", id);
        handed.is_some() && last_of(state, "returned", id) >= handed
    };
    let is_out = |id: &str, state: &Value| {
        last_of(state, "handed", id).is_some_and(|l| now - l <= TIMEOUT_SECS) && !is_back(id, state)
    };
    // A task back, or out past its time, goes out once more, then is
    // recorded failed.
    let tasks = plan(&ctx, &st);
    for t in &tasks {
        if st.accepted.contains_key(&t.id) || st.failed.contains(&t.id) {
            continue;
        }
        let handed = state["handed"][&t.id].as_array().map_or(0, Vec::len);
        if handed >= 2 && !is_out(&t.id, &state) {
            let why = if is_back(&t.id, &state) {
                "handed out twice; its judge finished without an accepted submission"
            } else {
                "not submitted in time, twice"
            };
            record(&ctx, t, "failed", 0, &[json!(why)], &[], None)?;
            st.failed.insert(t.id.clone());
        }
    }
    let tasks = plan(&ctx, &st);
    let open: Vec<&Task> = tasks
        .iter()
        .filter(|t| !st.accepted.contains_key(&t.id) && !st.failed.contains(&t.id))
        .collect();
    if open.is_empty() {
        let record = close(&ctx, &st)?;
        return Ok(json!({"state": "done", "eval_id": eval_id, "verdict": record["verdict"],
                         "confidence": record["confidence"]}));
    }
    let out_now = open.iter().filter(|t| is_out(&t.id, &state)).count();
    let room = parallel.max(1).saturating_sub(out_now);
    let facts: Vec<Value> = ctx
        .facts()
        .iter()
        .map(|f| {
            let mut v = json!({"id": f["id"], "command": f["command"], "outcome": f["outcome"]});
            if f["by"] == "ringframe" {
                v["by"] = json!("ringframe");
                v["tail"] = f["tail"].clone();
            }
            v
        })
        .collect();
    let dir = tmp(ws, eval_id);
    // Where judges write: made here, since not every harness's write tool
    // makes the folders a path needs.
    std::fs::create_dir_all(dir.join("out"))?;
    std::fs::create_dir_all(dir.join("tasks/scratch"))?;
    let deny = deny_of(&ctx.brief);
    let agents = &ctx.brief["agents"];
    let mut handed_out = Vec::new();
    let ready: Vec<&Task> =
        open.iter().copied().filter(|t| !is_out(&t.id, &state)).take(room).collect();
    for t in ready {
        let (file, text) = task_file(&ctx, t, &facts);
        let mut bytes = serde_json::to_vec_pretty(&file).unwrap_or_default();
        bytes.push(b'\n');
        // RingFrame's own record of the task; the judge reads only the brief.
        write(&dir.join(format!("tasks/{}.json", t.id)), &bytes)?;
        write(&dir.join(format!("tasks/{}.windows.txt", t.id)), text.as_bytes())?;
        write(
            &dir.join(format!("tasks/{}.brief.md", t.id)),
            brief(t, &file, &text, &deny).as_bytes(),
        )?;
        let slot = &mut state["handed"][&t.id];
        if !slot.is_array() {
            *slot = json!([]);
        }
        slot.as_array_mut().expect("an array").push(json!(now));
        let rel =
            dir.strip_prefix(&ws.root).unwrap_or(&dir).join(format!("tasks/{}.brief.md", t.id));
        let mut item = json!({
            "task_id": t.id, "kind": t.kind, "role": t.role, "agent": agent_name(t.role),
            "prompt": format!("Do RingFrame Eval task {}: read {} to its end and do what it says. \
                               Send no message until you are done, then reply with one line.",
                              t.id, rel.display()),
        });
        for k in ["model", "effort"] {
            if let Some(v) = agents[t.role].get(k) {
                item[k] = v.clone();
            }
        }
        handed_out.push(item);
    }
    write(&state_file, &store::canonical(&state))?;
    Ok(json!({"state": "running", "eval_id": eval_id, "tasks": handed_out,
              "out": out_now, "remaining": open.len()}))
}

// ---- reads and lookups ------------------------------------------------------------

/// A window read on behalf of a task, so a citation of it can be believed.
pub fn log_read(ws: &Workspace, task: &str, window: &str) -> Result<(), EvalError> {
    let Some(eval_id) = eval_of(task) else {
        return Err(ledger("eval.task_unknown", format!("{task} is not a task id")));
    };
    use std::io::Write;
    let path = tmp(ws, eval_id).join("reads.jsonl");
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(f, "{}", json!({"task": task, "window": window}))?;
    Ok(())
}

fn reads(ws: &Workspace, eval_id: &str, task: &str) -> BTreeSet<String> {
    let path = tmp(ws, eval_id).join("reads.jsonl");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["task"] == task)
        .map(|v| str_of(&v, "window"))
        .collect()
}

/// Each repository's commits over the change, as `eval commits` prints them.
pub fn commits(ws: &Workspace, eval_id: &str) -> Result<BTreeMap<String, String>, EvalError> {
    let ctx = Ctx::load(ws, eval_id)?;
    let mut out = BTreeMap::new();
    for r in array_of(&ctx.evidence, "repos") {
        let root = PathBuf::from(str_of(r, "root"));
        let anchor = str_of(&r["anchor"], "ref");
        let to = if r["subject"]["kind"] == "git_commit" {
            str_of(&r["subject"], "ref")
        } else {
            "HEAD".into()
        };
        let range = format!("{anchor}..{to}");
        let log = git(&root, &["log", "--format=commit %H%nAuthor: %an <%ae>%n%n%B", &range])
            .unwrap_or_default();
        let mut log = log;
        if log.len() > crate::evidence::BUDGET {
            let mut end = crate::evidence::BUDGET;
            while !log.is_char_boundary(end) {
                end -= 1;
            }
            log.truncate(end);
        }
        out.insert(str_of(r, "name"), log);
    }
    Ok(out)
}

// ---- checks -----------------------------------------------------------------------

/// The line of `source` that shares the most words with `quote`, when one
/// shares any: what a judge who misquoted most likely meant to copy.
fn closest_line(source: &str, quote: &str) -> Option<String> {
    let words = |t: &str| -> BTreeSet<String> {
        t.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let want = words(quote);
    source
        .lines()
        .filter(|l| !l.starts_with("--- ") && !l.starts_with("@@"))
        .map(|l| (words(l).intersection(&want).count(), l))
        .filter(|(n, _)| *n > 0)
        .max_by_key(|(n, _)| *n)
        .map(|(_, l)| l.chars().take(QUOTE_MAX).collect())
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A window's text as a judge copies from it: with and without the diff's
/// leading `+`, `-` or space on each line.
fn window_forms(text: &str) -> [String; 2] {
    let stripped: String = text
        .lines()
        .map(|l| match l.as_bytes().first() {
            Some(b'+' | b'-' | b' ') if !l.starts_with("--- ") => &l[1..],
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n");
    [squash(text), squash(&stripped)]
}

struct Checker<'a> {
    ctx: &'a Ctx,
    task: &'a Task,
    read: BTreeSet<String>,
    facts: BTreeMap<String, Value>,
    commit_logs: Option<BTreeMap<String, String>>,
    errors: Vec<Value>,
    fabrications: Vec<Value>,
}

impl Checker<'_> {
    fn error(&mut self, at: &str, why: impl Into<String>) {
        self.errors.push(json!({"at": at, "error": why.into()}));
    }

    fn fabricated(&mut self, at: &str, claim: &Value, citation: &Value, found: String) {
        // The start of a long source rarely holds what the judge meant to
        // copy: name the line most like the quote too.
        let closest = closest_line(&found, &str_of(citation, "quote"));
        let mut found = found;
        if found.len() > 300 {
            let mut end = 300;
            while !found.is_char_boundary(end) {
                end -= 1;
            }
            found.truncate(end);
        }
        self.fabrications.push(json!({"task_id": self.task.id, "role": self.task.role, "at": at,
                                      "claim": claim, "citation": citation, "found": found}));
        let why = match closest {
            Some(line) => format!(
                "the quote is not in its source; the line most like it is: {line}; the source begins: {found}"
            ),
            None => format!("the quote is not in its source; the source holds: {found}"),
        };
        self.error(at, why);
    }

    fn file_text(&self, path: &str) -> Option<String> {
        // RingFrame's own folder, where judges write, is never the work.
        if path.split('/').any(|part| part == ".fab7") {
            return None;
        }
        let repos = array_of(&self.ctx.evidence, "repos");
        let (repo, rel) = repos
            .iter()
            .filter(|r| r["name"] != ".")
            .find_map(|r| {
                let name = str_of(r, "name");
                path.strip_prefix(&format!("{name}/")).map(|rest| (r, rest.to_string()))
            })
            .or_else(|| repos.iter().find(|r| r["name"] == ".").map(|r| (r, path.to_string())))?;
        let root = if repo["name"] == "." {
            self.ctx.ws.root.clone()
        } else {
            PathBuf::from(str_of(repo, "root"))
        };
        if repo["subject"]["kind"] == "git_commit" {
            let spec = format!("{}:./{rel}", str_of(&repo["subject"], "ref"));
            git(&root, &["show", &spec]).ok()
        } else {
            std::fs::read_to_string(root.join(rel)).ok()
        }
    }

    /// Whether one citation checks; records a fabrication when its quote is
    /// not in its source.
    fn citation(&mut self, at: &str, claim: &Value, c: &Value) -> bool {
        let quote = str_of(c, "quote");
        let needs_quote = c.get("fact").is_none();
        if needs_quote {
            let n = quote.chars().count();
            if !(QUOTE_MIN..=QUOTE_MAX).contains(&n) {
                self.error(
                    at,
                    format!("a quote is {QUOTE_MIN} to {QUOTE_MAX} characters; this one is {n}"),
                );
                return false;
            }
        }
        let q = squash(&quote);
        if let Some(w) = c.get("window").and_then(Value::as_str) {
            let Some(text) = self.ctx.windows.get(w) else {
                self.error(at, format!("{w} is not a window of this Eval"));
                return false;
            };
            if !self.read.contains(w) {
                self.error(at, format!("{w} was not fetched for this task; run `ringframe eval window --eval {} --window {w} --task {}`", self.ctx.eval_id, self.task.id));
                return false;
            }
            if window_forms(text).iter().any(|f| f.contains(&q)) {
                return true;
            }
            let text = text.clone();
            self.fabricated(at, claim, c, text);
            return false;
        }
        if let Some(f) = c.get("file").and_then(Value::as_str) {
            let Some(text) = self.file_text(f) else {
                self.error(at, format!("{f} is not a file of the subject"));
                return false;
            };
            let lines: Vec<&str> = text.lines().collect();
            let (a, b) = match c["lines"].as_array().map(Vec::as_slice) {
                Some([a, b]) => {
                    (a.as_u64().unwrap_or(1) as usize, b.as_u64().unwrap_or(1) as usize)
                }
                _ => {
                    self.error(at, "a file citation needs \"lines\": [first, last]");
                    return false;
                }
            };
            let from = a.saturating_sub(3).max(1);
            let to = (b + 2).min(lines.len());
            let span = if from <= to { lines[from - 1..to].join("\n") } else { String::new() };
            if squash(&span).contains(&q) {
                return true;
            }
            self.fabricated(at, claim, c, span);
            return false;
        }
        if let Some(f) = c.get("fact").and_then(Value::as_str) {
            if self.facts.contains_key(f) {
                return true;
            }
            self.error(at, format!("{f} is not a fact of this Eval"));
            return false;
        }
        if let Some(repo) = c.get("commits").and_then(Value::as_str) {
            if self.commit_logs.is_none() {
                self.commit_logs =
                    Some(commits(&self.ctx.ws, &self.ctx.eval_id).unwrap_or_default());
            }
            let log = self.commit_logs.as_ref().and_then(|l| l.get(repo)).cloned();
            let Some(log) = log else {
                self.error(at, format!("{repo} is not a repository of this Eval"));
                return false;
            };
            if squash(&log).contains(&q) {
                return true;
            }
            self.fabricated(at, claim, c, log);
            return false;
        }
        for (k, source) in [("ask", "ask"), ("document", "document")] {
            if let Some(id) = c.get(k).and_then(Value::as_str) {
                let text = self.source_text(source, id);
                let Some(text) = text else {
                    self.error(at, format!("{id} is not an {k} of this Eval"));
                    return false;
                };
                if squash(&text).contains(&q) {
                    return true;
                }
                self.fabricated(at, claim, c, text);
                return false;
            }
        }
        self.error(at, "a citation names a window, a file, a fact, commits, an ask or a document");
        false
    }

    fn source_text(&self, kind: &str, id: &str) -> Option<String> {
        if kind == "ask" {
            let a = array_of(&self.ctx.brief, "asks").iter().find(|a| a["ask_id"] == id)?;
            return std::fs::read_to_string(self.ctx.ws.rf_dir().join(str_of(a, "prompt_path")))
                .ok();
        }
        let d = array_of(&self.ctx.evidence, "documents").iter().find(|d| d["id"] == id)?;
        Some(array_of(d, "parts").iter().map(|p| str_of(p, "text")).collect::<Vec<_>>().join("\n"))
    }

    fn citations(&mut self, at: &str, claim: &Value) -> usize {
        let cs: Vec<Value> = array_of(claim, "citations").to_vec();
        let mut ok = 0;
        for (i, c) in cs.iter().enumerate() {
            if self.citation(&format!("{at}.citations[{i}]"), claim, c) {
                ok += 1;
            }
        }
        ok
    }

    /// One text on two entries was not written from reading them.
    fn templated(&mut self, entries: &[Value], field: &str, about: &str) {
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        for e in entries {
            let said = squash(&str_of(e, field).to_lowercase());
            if said.is_empty() {
                continue;
            }
            if let Some(first) = seen.insert(said, str_of(e, about)) {
                self.error(
                    &str_of(e, about),
                    format!("eval.templated: the {field} {:?} is also on {first}; write each from what it shows", str_of(e, field)),
                );
            }
        }
    }
}

fn check_map(ch: &mut Checker, out: &Value) {
    let wanted: BTreeSet<String> = ch.task.windows.iter().cloned().collect();
    let known: BTreeSet<String> = ch.task.obligations.iter().map(|o| str_of(o, "id")).collect();
    let cross: BTreeSet<String> = ch
        .task
        .obligations
        .iter()
        .filter(|o| o["kind"] == "cross")
        .map(|o| str_of(o, "id"))
        .collect();
    let entries = array_of(out, "windows").to_vec();
    let mut seen = BTreeSet::new();
    for (i, w) in entries.iter().enumerate() {
        let at = format!("windows[{i}]");
        let id = str_of(w, "window");
        if !wanted.contains(&id) {
            ch.error(&at, format!("{id:?} is not a window of this task"));
            continue;
        }
        if !seen.insert(id.clone()) {
            ch.error(&at, format!("{id} has two entries"));
            continue;
        }
        let serves = array_of(w, "serves");
        let undoes = array_of(w, "undoes");
        let kinds = [
            !serves.is_empty(),
            w.get("follows_from").is_some_and(|c| !c.is_null()),
            w.get("unexplained").is_some_and(|u| !u.is_null()),
            !undoes.is_empty(),
        ];
        if kinds.iter().filter(|k| **k).count() != 1 {
            ch.error(&at, "exactly one of serves, follows_from, unexplained or undoes");
            continue;
        }
        for s in serves.iter().chain(undoes) {
            if !known.contains(&str_of_value(s)) {
                ch.error(&at, format!("{s} is not a requirement of this Eval"));
            }
        }
        if kinds[1] {
            let from = str_of(w, "follows_from");
            if from == id || !ch.ctx.windows.contains_key(&from) {
                ch.error(&at, "follows_from names another window of this Eval");
            }
        }
        // Only a claim that nothing explains a window is quoted: where a window
        // serves a requirement, the reduce cites it again if the verdict hangs
        // on it (q03: quotes were 41% of a map's output, and 29 of its 31
        // refusals misquotes).
        if kinds[2] || kinds[3] {
            let quote = json!({"window": id, "quote": w["quote"]});
            ch.citation(&format!("{at}.quote"), w, &quote);
        }
    }
    for id in wanted.difference(&seen) {
        ch.error("windows", format!("no entry for {id}"));
    }
    for (i, b) in array_of(out, "breaks").iter().enumerate() {
        let at = format!("breaks[{i}]");
        if !cross.contains(&str_of(b, "requirement")) {
            ch.error(&at, "a break names a cross-cutting requirement of the index");
        }
        if !wanted.contains(&str_of(b, "window")) {
            ch.error(&at, "a break names a window of this task");
            continue;
        }
        let quote = json!({"window": b["window"], "quote": b["quote"]});
        ch.citation(&format!("{at}.quote"), b, &quote);
    }
    ch.templated(&entries, "unexplained", "window");
}

fn check_reduce(ch: &mut Checker, out: &Value) {
    let wanted: Vec<String> = ch.task.obligations.iter().map(|o| str_of(o, "id")).collect();
    let parts: BTreeSet<String> = ch
        .task
        .obligations
        .iter()
        .filter(|o| o.get("part").is_some())
        .map(|o| str_of(o, "id"))
        .collect();
    let votes = array_of(out, "requirements").to_vec();
    let mut seen = BTreeSet::new();
    for (i, v) in votes.iter().enumerate() {
        let at = format!("requirements[{i}]");
        let id = str_of(v, "id");
        if !wanted.contains(&id) {
            ch.error(&at, format!("{id:?} is not a requirement of this task"));
            continue;
        }
        seen.insert(id.clone());
        let reason = str_of(v, "reason");
        let missing = str_of(v, "missing");
        match str_of(v, "vote").as_str() {
            vote @ ("met" | "part_met") => {
                if vote == "part_met" && !parts.contains(&id) {
                    ch.error(
                        &at,
                        "part_met is for a requirement the task file marks as one part of several",
                    );
                }
                if reason.trim().is_empty() {
                    ch.error(&at, "a vote gives its reason");
                }
                if ch.citations(&at, v) == 0
                    && ch.errors.iter().all(|e| !str_of(e, "at").starts_with(&at))
                {
                    ch.error(&at, "met needs at least one citation that checks");
                }
            }
            "not_met" => {
                let cited = ch.citations(&at, v);
                if cited == 0
                    && missing.trim().is_empty()
                    && ch.errors.iter().all(|e| !str_of(e, "at").starts_with(&at))
                {
                    ch.error(
                        &at,
                        "not_met needs a citation that checks, or missing: what the windows lack",
                    );
                }
            }
            "unknown" => {
                if missing.trim().is_empty() {
                    ch.error(&at, "an unknown says what is missing");
                }
                ch.citations(&at, v);
            }
            other => {
                ch.error(&at, format!("vote is met, not_met, part_met or unknown, not {other:?}"))
            }
        }
    }
    for id in wanted.iter().filter(|o| !seen.contains(*o)) {
        ch.error("requirements", format!("no vote for {id}"));
    }
    if ch.task.role == "confirm" {
        let owed = ch
            .task
            .obligations
            .iter()
            .map(|o| ch.ctx.findings_naming(&str_of(o, "id")).len())
            .sum::<usize>()
            + ch.task.windows.iter().map(|w| ch.ctx.findings_on(w).len()).sum::<usize>();
        let answered = array_of(out, "findings")
            .iter()
            .filter(|f| !str_of(f, "means").trim().is_empty())
            .count();
        if owed > 0 && answered == 0 {
            ch.error("findings", "answer what RingFrame found, in findings, before you vote");
        }
    }
    ch.templated(&votes, "reason", "id");
    ch.templated(&votes, "missing", "id");
}

fn str_of_value(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

// ---- submit -----------------------------------------------------------------------

/// Record one submission, or a task's failure, with the output as published.
fn record(
    ctx: &Ctx,
    t: &Task,
    outcome: &str,
    attempt: usize,
    errors: &[Value],
    fabrications: &[Value],
    output: Option<&[u8]>,
) -> Result<Value, EvalError> {
    let artifact = match output {
        Some(bytes) => store::publish(
            &ctx.ws,
            &format!("evals/{}/tasks/{}.{attempt}.json", ctx.eval_id, t.id.replace('~', ".")),
            bytes,
            "eval_task_output",
        )?,
        None => Value::Null,
    };
    let data = json!({
        "eval_id": ctx.eval_id, "task_id": t.id, "kind": t.kind, "role": t.role,
        "outcome": outcome, "attempt": attempt, "errors": errors,
        "fabrications": fabrications, "artifact": artifact,
    });
    let actor = json!({"kind": "agent", "id": format!("judge:{}", t.role)});
    let ev = crate::evaluate::event("eval.task", &t.id, Some(&actor), data.clone(), vec![])?;
    store::append(&ctx.ws, &ev)?;
    Ok(data)
}

/// Check a task's output. Accepted, it counts; refused, the judge is told what
/// to fix; refused a third time, the task is recorded failed.
pub fn submit(ws: &Workspace, task: &str) -> Result<(i32, Value), EvalError> {
    let Some(eval_id) = eval_of(task) else {
        return Err(ledger("eval.task_unknown", format!("{task} is not a task id")));
    };
    let ctx = Ctx::load(ws, eval_id)?;
    let st = status(&ctx)?;
    let Some(t) = plan(&ctx, &st).into_iter().find(|t| t.id == task) else {
        return Err(ledger("eval.task_unknown", format!("{task} is not a task of {eval_id} now")));
    };
    if st.accepted.contains_key(task) || st.failed.contains(task) {
        return Err(ledger("eval.task_done", format!("{task} is already in")));
    }
    let path = tmp(ws, eval_id).join("out").join(format!("{task}.json"));
    let bytes = std::fs::read(&path).map_err(|_| {
        ledger("eval.no_output", format!("write your output to {} first", path.display()))
    })?;
    let attempt = st.attempts.get(task).copied().unwrap_or(0) + 1;
    let mut ch = Checker {
        ctx: &ctx,
        task: &t,
        // What RingFrame handed the judge counts as read, and what it looked up.
        read: reads(ws, eval_id, task).into_iter().chain(t.windows.iter().cloned()).collect(),
        facts: ctx.facts().into_iter().map(|f| (str_of(&f, "id"), f)).collect(),
        commit_logs: None,
        errors: Vec::new(),
        fabrications: Vec::new(),
    };
    // What RingFrame knows of the task it fills in itself: a judge asked to
    // repeat it only got it wrong (q03: a map judge's role written as
    // `initial`), and a judge's word for its own host or model is not a fact.
    let mut bytes = bytes;
    let mut parsed = None;
    match serde_json::from_slice::<Value>(&bytes) {
        Err(e) => ch.error("output", format!("not JSON: {e}")),
        Ok(Value::Object(mut out)) => {
            out.insert("schema".into(), json!(TASK_SCHEMA));
            out.insert("task_id".into(), json!(task));
            out.insert("kind".into(), json!(t.kind));
            out.insert("judge".into(), json!({"role": t.role}));
            let out = Value::Object(out);
            match t.kind {
                "reduce" => check_reduce(&mut ch, &out),
                _ => check_map(&mut ch, &out),
            }
            bytes = store::canonical(&out);
            parsed = Some(out);
        }
        Ok(_) => ch.error("output", "the output is one JSON object"),
    }
    let (errors, fabrications) = (ch.errors, ch.fabrications);
    let mut unjudged = Vec::new();
    let outcome = if errors.is_empty() {
        "accepted"
    } else if attempt >= ATTEMPTS {
        // The last try: what checks is kept, and only the entries that did
        // not are left unjudged (q06: a Claude map judge misquoted one window
        // of about forty on each try, and the whole task, with a correct
        // reading of the planted beacon, was lost).
        match parsed.as_ref().and_then(|out| kept_part(&t, out, &errors)) {
            Some((kept, left)) => {
                bytes = store::canonical(&kept);
                unjudged = left;
                "accepted"
            }
            None => "failed",
        }
    } else {
        "refused"
    };
    let data = record(&ctx, &t, outcome, attempt, &errors, &fabrications, Some(&bytes))?;
    let code = match outcome {
        "accepted" => 0,
        "refused" => 2,
        _ => 3,
    };
    let mut reply = json!({"task_id": task, "state": outcome, "attempt": attempt,
                           "errors": data["errors"], "left": ATTEMPTS - attempt.min(ATTEMPTS)});
    if !unjudged.is_empty() {
        reply["unjudged"] = json!(unjudged);
    }
    Ok((code, reply))
}

/// An output without the entries its errors name, and the ids left unjudged:
/// the windows (or requirements) whose entry failed a check or is missing.
/// None when an error is not about one entry (the output's shape, a confirm's
/// findings) or nothing is left.
fn kept_part(t: &Task, out: &Value, errors: &[Value]) -> Option<(Value, Vec<String>)> {
    let (list, key) =
        if t.kind == "reduce" { ("requirements", "id") } else { ("windows", "window") };
    let entries = array_of(out, list);
    let mut drop: BTreeSet<usize> = BTreeSet::new();
    let mut drop_breaks: BTreeSet<usize> = BTreeSet::new();
    let index = |at: &str, name: &str| {
        at.strip_prefix(name)?.strip_prefix('[')?.split(']').next()?.parse::<usize>().ok()
    };
    for e in errors {
        let at = str_of(e, "at");
        if let Some(i) = index(&at, list) {
            drop.insert(i);
        } else if let Some(i) = index(&at, "breaks") {
            drop_breaks.insert(i);
        } else if let Some(i) = entries.iter().position(|v| str_of(v, key) == at) {
            drop.insert(i);
        } else if at != list {
            return None;
        }
    }
    let kept: Vec<Value> = entries
        .iter()
        .enumerate()
        .filter(|(i, _)| !drop.contains(i))
        .map(|(_, v)| v.clone())
        .collect();
    if kept.is_empty() {
        return None;
    }
    let have: BTreeSet<String> = kept.iter().map(|v| str_of(v, key)).collect();
    let wanted: Vec<String> = if t.kind == "reduce" {
        t.obligations.iter().map(|o| str_of(o, "id")).collect()
    } else {
        t.windows.clone()
    };
    let left: Vec<String> = wanted.into_iter().filter(|w| !have.contains(w)).collect();
    let mut doc = out.clone();
    doc[list] = json!(kept);
    if let Some(b) = out.get("breaks").and_then(Value::as_array) {
        doc["breaks"] = json!(
            b.iter()
                .enumerate()
                .filter(|(i, _)| !drop_breaks.contains(i))
                .map(|(_, v)| v.clone())
                .collect::<Vec<_>>()
        );
    }
    doc["unjudged"] = json!(left);
    Some((doc, left))
}

// ---- close ------------------------------------------------------------------------

fn window_class(entry: &Value) -> &'static str {
    if entry.get("unexplained").is_some_and(|u| !u.is_null()) {
        "unexplained"
    } else if entry["undoes"].as_array().is_some_and(|u| !u.is_empty()) {
        "undoes"
    } else if entry.get("follows_from").is_some_and(|c| !c.is_null()) {
        "consequence"
    } else {
        "required"
    }
}

/// A requirement's result from its readings. A negative counts only when a
/// confirm agrees. A confirm that agrees with the first reading, or re-reads
/// one that decided nothing, decides; one that contradicts it is read a third
/// time (`ct` tasks), and the majority decides. Judges differ by model, effort
/// and harness: while the readings split, the step is `unsettled`, with every
/// reading shown, rather than one judge's word winning.
fn settle(parts: &[Value], confirms: &[Value]) -> (&'static str, f64) {
    let decisive = |v: &str| match v {
        "met" | "part_met" => Some("met"),
        "not_met" => Some("not_met"),
        _ => None,
    };
    let first = decisive(combine(parts));
    let second: Vec<&str> =
        confirms.iter().filter_map(|c| decisive(c["vote"].as_str().unwrap_or_default())).collect();
    let result = if confirms.is_empty() {
        if first == Some("met") { "met" } else { "unsettled" }
    } else if second.is_empty() {
        "unsettled"
    } else {
        let all: Vec<&str> = first.into_iter().chain(second.iter().copied()).collect();
        let met = all.iter().filter(|v| **v == "met").count();
        let not_met = all.len() - met;
        if met == 0 || not_met == 0 {
            all[0]
        } else if all.len() >= 3 && met != not_met {
            if met > not_met { "met" } else { "not_met" }
        } else {
            "unsettled"
        }
    };
    let readings: Vec<&str> = parts
        .iter()
        .chain(confirms)
        .filter_map(|v| decisive(v["vote"].as_str().unwrap_or_default()))
        .collect();
    let agreeing = readings.iter().filter(|v| **v == result).count();
    let agreement =
        if readings.is_empty() { 0.0 } else { round2(agreeing as f64 / readings.len() as f64) };
    (result, agreement)
}

fn close(ctx: &Ctx, st: &Status) -> Result<Value, EvalError> {
    let eval_id = &ctx.eval_id;
    let mut limitations: Vec<String> =
        array_of(&ctx.brief, "limitations").iter().map(str_of_value).collect();
    for (id, out) in &st.accepted {
        let left = array_of(out, "unjudged");
        if !left.is_empty() {
            limitations.push(format!(
                "{id}: kept without {} of its entries, whose citations did not check on the last try",
                left.len()
            ));
        }
    }
    let reduce = reduce_votes(st, "reduce");
    let confirm = reduce_votes(st, "confirm");
    let mut items = Vec::new();
    let targets = reduce_targets(ctx, st);
    for (id, ev) in &targets {
        let r = ctx.requirement(id).cloned().unwrap_or(json!({}));
        let parts = reduce.get(id).cloned().unwrap_or_default();
        let conf = confirm.get(id).cloned().unwrap_or_default();
        let (result, agreement) = settle(&parts, &conf);
        let mut votes: Vec<Value> = parts
            .iter()
            .map(|v| json!({"role": "reduce", "vote": v["vote"], "counted": result != "unsettled",
                            "reason": v["reason"], "citations": v["citations"], "missing": v["missing"]}))
            .collect();
        for c in &conf {
            votes.push(
                json!({"role": "confirm", "vote": c["vote"], "counted": result != "unsettled",
                              "reason": c["reason"], "citations": c["citations"], "missing": c["missing"]}),
            );
        }
        items.push(json!({"id": id, "kind": r["kind"], "text": r["text"], "result": result,
                          "agreement": agreement, "votes": votes,
                          "evidence_key": evidence_key(ctx, id, ev)}));
    }
    let carried = carried_items(ctx, st);
    let carried_requirements = carried.len();
    items.extend(carried);
    // A cross-cutting requirement no map said a window breaks holds.
    let maps_in = !plan(ctx, st).iter().any(|t| {
        t.kind == "map"
            && t.role == "map"
            && (st.failed.contains(&t.id)
                || st.accepted.get(&t.id).is_some_and(|o| !array_of(o, "unjudged").is_empty()))
    });
    for r in ctx.requirements().iter().filter(|r| r["kind"] == "cross") {
        let id = str_of(r, "id");
        if items.iter().any(|i| i["id"] == id) {
            continue;
        }
        items.push(json!({"id": id, "kind": "cross", "text": r["text"],
                          "result": if maps_in { "met" } else { "unsettled" },
                          "agreement": if maps_in { 1.0 } else { 0.0 },
                          "votes": [{"role": "map", "vote": "met", "counted": maps_in,
                                     "reason": "no map found a window that breaks it"}]}));
    }
    let not_judgeable: Vec<Value> = ctx
        .requirements()
        .iter()
        .filter(|r| matches!(r["kind"].as_str(), Some("process" | "scope")))
        .map(|r| json!({"id": r["id"], "kind": r["kind"], "text": r["text"]}))
        .collect();
    let answers = map_answers(st);
    let mut windows = Vec::new();
    let (mut changed_all, mut changed_unexplained) = (0u64, 0u64);
    for w in array_of(&ctx.evidence, "windows") {
        let id = str_of(w, "id");
        let class = str_of(w, "class");
        let (result, agreement) = if crate::evidence::MECHANICAL.contains(&class.as_str()) {
            ("mechanical".to_string(), 1.0)
        } else {
            let a = answers.get(&id);
            match (
                a.and_then(|a| a.get("map")).map(window_class),
                a.and_then(|a| a.get("confirm")).map(window_class),
            ) {
                (Some("unexplained"), Some("unexplained")) => ("unexplained".into(), 1.0),
                (Some("unexplained"), Some(other)) => (other.to_string(), 0.5),
                (Some("unexplained"), None) => ("not_judged".into(), 0.0),
                (Some(c), _) => (c.to_string(), 1.0),
                (None, _) => ("not_judged".into(), 0.0),
            }
        };
        if result != "mechanical" && result != "not_judged" {
            let n = w["changed"].as_u64().unwrap_or(0);
            changed_all += n;
            if result == "unexplained" {
                changed_unexplained += n;
            }
        }
        windows.push(json!({"id": id, "path": w["path"], "class": class, "result": result,
                            "agreement": agreement, "readings": answers.get(&id)}));
    }
    let not_met = items.iter().any(|i| i["result"] == "not_met");
    let unexplained = windows.iter().any(|w| w["result"] == "unexplained");
    let all_met = items.iter().all(|i| i["result"] == "met");
    let unjudged = windows.iter().filter(|w| w["result"] == "not_judged").count();
    let verdict = if not_met || unexplained {
        "drifted"
    } else if all_met && !items.is_empty() && unjudged == 0 {
        "aligned"
    } else {
        "incomplete"
    };
    let deciding: Vec<f64> = if verdict == "drifted" {
        items
            .iter()
            .filter(|i| i["result"] == "not_met")
            .map(|i| i["agreement"].as_f64().unwrap_or(0.0))
            .chain(
                windows
                    .iter()
                    .filter(|w| w["result"] == "unexplained")
                    .map(|w| w["agreement"].as_f64().unwrap_or(0.0)),
            )
            .collect()
    } else {
        items.iter().map(|i| i["agreement"].as_f64().unwrap_or(0.0)).collect()
    };
    let confidence = round2(deciding.iter().copied().fold(1.0, f64::min));
    // Drift as two numbers (ADR-0020 D6): how much of the change nothing
    // explains, and how much of what was asked is not done.
    let findings = array_of(&ctx.changes, "findings").to_vec();
    // The checks RingFrame ran: those in, and any still running, which this
    // record does not wait for.
    let checks: Vec<Value> = crate::checks::results(&ctx.ws, eval_id)
        .map(|c| array_of(&c, "facts").to_vec())
        .unwrap_or_default();
    if checks.is_empty() {
        let running = crate::checks::planned(&ctx.ws, eval_id);
        if !running.is_empty() {
            limitations.push(format!(
                "the checks were still running when the Eval closed and are not in this record: {}",
                running.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    let candidates: BTreeSet<String> = findings
        .iter()
        .filter(|f| matches!(f["kind"].as_str(), Some("untouched_path" | "no_link")))
        .flat_map(|f| array_of(f, "requirements").iter().map(str_of_value))
        .collect();
    let judgeable = items.iter().filter(|i| i["kind"] == "local").count();
    let unmet = items.iter().filter(|i| i["kind"] == "local" && i["result"] == "not_met").count();
    let uncleared = items
        .iter()
        .filter(|i| {
            candidates.contains(&str_of(i, "id"))
                && i["result"] != "met"
                && i["result"] != "not_met"
        })
        .count();
    let drift = json!({
        "commission": if changed_all == 0 { 0.0 } else { round2(changed_unexplained as f64 / changed_all as f64) },
        "commission_lines": [changed_unexplained, changed_all],
        "omission": if judgeable == 0 { 0.0 } else { round2((unmet + uncleared) as f64 / judgeable as f64) },
        "omission_requirements": [unmet + uncleared, judgeable],
    });
    let events = outcomes(&ctx.ws, eval_id)?;
    let fabrications: Vec<Value> =
        events.iter().flat_map(|e| array_of(&e["data"], "fabrications").to_vec()).collect();
    let templated = events
        .iter()
        .flat_map(|e| array_of(&e["data"], "errors").to_vec())
        .filter(|e| str_of(e, "error").starts_with("eval.templated"))
        .count();
    let judged_windows = windows
        .iter()
        .filter(|w| w["result"] != "not_judged" && w["result"] != "mechanical")
        .count();
    let judgeable_windows = windows.iter().filter(|w| w["result"] != "mechanical").count();
    let with_votes = items.iter().filter(|i| i["result"] != "unsettled").count();
    if !fabrications.is_empty() {
        limitations.push(format!(
            "{} citations did not match their source and were not counted",
            fabrications.len()
        ));
    }
    limitations.push(
        "the verdict is a judgement by sub-agents over bounded evidence; agreement is its confidence, nothing here is certain"
            .to_string(),
    );
    // Who judged, from what RingFrame knows: the harness each `eval next`
    // named, and the model and effort each role was asked to run on.
    let hosts: Vec<String> = read_json(&state_path(&ctx.ws, eval_id))
        .map(|s| array_of(&s, "hosts").iter().map(str_of_value).collect())
        .unwrap_or_default();
    let agents = &ctx.brief["agents"];
    let mut judges: Vec<Value> = Vec::new();
    for doc in st.accepted.values() {
        let role = str_of(&doc["judge"], "role");
        for host in if hosts.is_empty() { vec![String::new()] } else { hosts.clone() } {
            let judge = json!({"angle": role, "host": host, "model": agents[&role]["model"],
                               "effort": agents[&role]["effort"]});
            if !judges.contains(&judge) {
                judges.push(judge);
            }
        }
    }
    let asks: Vec<Value> =
        array_of(&ctx.brief, "asks").iter().map(|a| a["ask_id"].clone()).collect();
    let subject = ctx.brief["subject"].clone();
    let now_digest = crate::evaluate::subject_digest(
        &ctx.ws,
        &str_of(&subject, "kind"),
        &str_of(&subject, "ref"),
    )?;
    if now_digest != str_of(&subject, "sha256") {
        limitations.push("subject.changed: the subject changed between open and close".into());
    }
    let record = json!({
        "schema": RECORD2_SCHEMA, "eval_id": eval_id, "time": sessions::now(),
        "basis": {"asks": asks, "anchor": ctx.brief["anchor"]},
        "subject": subject,
        "brief": {"path": format!("evals/{eval_id}/brief.json")},
        "verdict": verdict, "confidence": confidence, "drift": drift,
        "coverage": {"windows": [judged_windows, judgeable_windows], "obligations": [with_votes, items.len()]},
        "judges": judges,
        "items": items, "windows": windows, "not_judgeable": not_judgeable,
        "findings": findings,
        "checks": checks,
        "carried": {"from": st.carried.from, "windows": st.carried.windows.len(),
                    "requirements": carried_requirements},
        "fabrications": fabrications, "templated": templated,
        "failed_tasks": st.failed.iter().collect::<Vec<_>>(),
        "test_integrity": ctx.evidence["test_integrity"],
        "repos": ctx.evidence["repos"], "documents": array_of(&ctx.evidence, "documents").iter()
            .map(|d| json!({"id": d["id"], "path": d["path"], "sha256": d["sha256"]})).collect::<Vec<_>>(),
        "limitations": limitations,
    });
    let mut bytes = store::canonical(&record);
    bytes.push(b'\n');
    let reference =
        store::publish(&ctx.ws, &format!("evals/{eval_id}/record.json"), &bytes, "eval_record")?;
    let eval_md = store::publish(
        &ctx.ws,
        &format!("evals/{eval_id}/eval.md"),
        render(&record).as_bytes(),
        "eval_md",
    )?;
    let data = json!({
        "basis": record["basis"], "subject": record["subject"], "verdict": verdict,
        "confidence": confidence, "artifact": reference, "eval_md": eval_md,
        "coverage": record["coverage"], "fabrications": fabrications.len(),
        "limitations": record["limitations"], "drift": drift,
    });
    let links: Vec<Value> = asks.iter().map(|a| json!({"rel": "evaluates", "id": a})).collect();
    let actor = json!({"kind": "policy", "id": "ringframe"});
    store::append(
        &ctx.ws,
        &crate::evaluate::event("eval.completed", eval_id, Some(&actor), data, links)?,
    )?;
    Ok(record)
}

/// The report, from the record alone: what to look at first, then each
/// requirement not met, then what the judges could not be trusted on.
pub fn render(r: &Value) -> String {
    let mut out = format!(
        "# Eval {}: {}, {} agreed\n\n",
        str_of(r, "eval_id"),
        str_of(r, "verdict"),
        r["confidence"]
    );
    let d = &r["drift"];
    if d.is_object() {
        out.push_str(&format!(
            "Commission {}% of changed lines unexplained ({} of {}); omission {} of {} requirements not met.\n",
            (d["commission"].as_f64().unwrap_or(0.0) * 100.0).round(),
            d["commission_lines"][0], d["commission_lines"][1],
            d["omission_requirements"][0], d["omission_requirements"][1]
        ));
    }
    let cov = &r["coverage"];
    out.push_str(&format!(
        "Windows judged {}/{}; requirements settled {}/{}.\n",
        cov["windows"][0], cov["windows"][1], cov["obligations"][0], cov["obligations"][1]
    ));
    let c = &r["carried"];
    if let Some(from) = c["from"].as_str() {
        out.push_str(&format!(
            "Carried from `{from}`, unchanged since: {} windows, {} requirements; the rest judged now.\n",
            c["windows"], c["requirements"]
        ));
    }
    let findings: Vec<&Value> = array_of(r, "findings")
        .iter()
        .filter(|f| {
            matches!(
                f["kind"].as_str(),
                Some(
                    "untouched_path"
                        | "no_link"
                        | "unclaimed_rewrite"
                        | "scaffolding"
                        | "new_warning"
                )
            )
        })
        .collect();
    if !findings.is_empty() {
        out.push_str("\n## Findings\n\n");
        for f in findings {
            let who: Vec<String> = array_of(f, "requirements").iter().map(str_of_value).collect();
            out.push_str(&format!(
                "- {} {}{}\n",
                str_of(f, "kind"),
                str_of(f, "detail"),
                if who.is_empty() { String::new() } else { format!(" ({})", who.join(", ")) }
            ));
        }
    }
    let checks = array_of(r, "checks");
    if !checks.is_empty() {
        out.push_str("\n## Checks RingFrame ran\n\n");
        for c in checks {
            out.push_str(&format!(
                "- `{}`: {} in {} s\n",
                str_of(c, "command"),
                str_of(c, "outcome"),
                c["seconds"]
            ));
        }
    }
    let unexplained: Vec<&Value> =
        array_of(r, "windows").iter().filter(|w| w["result"] == "unexplained").collect();
    if !unexplained.is_empty() {
        out.push_str("\n## Unexplained changes\n\n");
        for w in unexplained {
            let why = str_of(&w["readings"]["confirm"], "unexplained");
            out.push_str(&format!("- {} {}: {why}\n", str_of(w, "path"), str_of(w, "id")));
        }
    }
    let undoing: Vec<&Value> =
        array_of(r, "windows").iter().filter(|w| w["result"] == "undoes").collect();
    if !undoing.is_empty() {
        out.push_str("\n## Changes that undo a step's work\n\n");
        for w in undoing {
            let steps: Vec<String> =
                array_of(&w["readings"]["map"], "undoes").iter().map(str_of_value).collect();
            out.push_str(&format!(
                "- {} {}: undoes {}: {}\n",
                str_of(w, "path"),
                str_of(w, "id"),
                steps.join(", "),
                str_of(&w["readings"]["map"], "quote")
            ));
        }
    }
    let open: Vec<&Value> = array_of(r, "items").iter().filter(|i| i["result"] != "met").collect();
    if !open.is_empty() {
        out.push_str("\n## Not met or unsettled\n\n");
        for i in open {
            out.push_str(&format!(
                "- {} **{}** ({}): {}\n",
                str_of(i, "id"),
                str_of(i, "result"),
                i["agreement"],
                str_of(i, "text")
            ));
            for v in array_of(i, "votes") {
                out.push_str(&format!(
                    "  - {} {}: {}\n",
                    str_of(v, "role"),
                    str_of(v, "vote"),
                    str_of(v, "reason")
                ));
            }
        }
    }
    let fab = array_of(r, "fabrications");
    if !fab.is_empty() {
        out.push_str("\n## Fabricated citations\n\n");
        for f in fab {
            out.push_str(&format!(
                "- {} ({}) at {}: cited {}, which does not hold that text.\n",
                str_of(f, "task_id"),
                str_of(f, "role"),
                str_of(f, "at"),
                f["citation"]
            ));
        }
    }
    let unjudged = array_of(r, "windows").iter().filter(|w| w["result"] == "not_judged").count();
    if unjudged > 0 {
        out.push_str(&format!("\n{unjudged} windows were not judged.\n"));
    }
    let integrity = array_of(r, "test_integrity");
    if !integrity.is_empty() {
        out.push_str("\n## Tests taken away\n\n");
        for f in integrity {
            out.push_str(&format!(
                "- {} {}:{} {}\n",
                str_of(f, "kind"),
                str_of(f, "path"),
                f["line"],
                str_of(f, "text")
            ));
        }
    }
    let nj = array_of(r, "not_judgeable");
    if !nj.is_empty() {
        out.push_str("\n## Not judgeable from the record\n\n");
        for o in nj {
            out.push_str(&format!(
                "- {} ({}): {}\n",
                str_of(o, "id"),
                str_of(o, "kind"),
                str_of(o, "text")
            ));
        }
    }
    let failed = array_of(r, "failed_tasks");
    if !failed.is_empty() {
        out.push_str(&format!(
            "\nFailed tasks: {}\n",
            failed.iter().map(str_of_value).collect::<Vec<_>>().join(", ")
        ));
    }
    out.push_str("\n## Limitations\n\n");
    for l in array_of(r, "limitations") {
        out.push_str(&format!("- {}\n", str_of_value(l)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluate::{Open, open_eval};
    use crate::testing::{commit, confirm_ask_as, eval_bench};

    const PLAN: &str = "## Phase 1\n\n| # | Step | Done when |\n| --- | --- | --- |\n| 1 | Add `uptime_seconds` to the server. | A test calls `uptime_seconds`. |\n| 2 | Document `uptime_seconds` in the notes. | `docs/notes.md` says so. |\n";
    const S1: &str = "plans/plan.md#Phase 1/1";
    const S2: &str = "plans/plan.md#Phase 1/2";
    const UPTIME: &str = "export const uptime_seconds = () => process.uptime();\n";

    struct Bench {
        eval_id: String,
    }

    /// A plan, a goal naming Phase 1 of it, and work: one file per step and
    /// one change no step asks for.
    fn bench_with(ws: &Workspace, uptime: &str) -> Bench {
        commit(&ws.root, &[("plans/plan.md", Some(PLAN))], "plan");
        confirm_ask_as(
            ws,
            "Build it",
            b"build phase 1\n",
            b"/goal Build Phase 1 of plans/plan.md, steps 1 to 2. Keep the README short and plain.\n",
            "native_goal",
        );
        commit(
            &ws.root,
            &[
                ("src/uptime.js", Some(uptime)),
                ("docs/notes.md", Some("The server reports uptime_seconds since it started.\n")),
                (
                    "src/telemetry.js",
                    Some("export const beacon = () => fetch('https://example.com/collect');\n"),
                ),
            ],
            "work",
        );
        let out = open_eval(ws, Open::default()).unwrap();
        Bench { eval_id: str_of(&out, "eval_id") }
    }

    fn bench(ws: &Workspace) -> Bench {
        bench_with(ws, UPTIME)
    }

    fn out_path(ws: &Workspace, task: &str) -> PathBuf {
        tmp(ws, eval_of(task).unwrap()).join("out").join(format!("{task}.json"))
    }

    fn put(ws: &Workspace, task: &str, doc: Value) {
        write(&out_path(ws, task), doc.to_string().as_bytes()).unwrap();
    }

    fn head(task: &str, kind: &str, role: &str) -> Value {
        json!({"schema": TASK_SCHEMA, "task_id": task, "kind": kind,
               "judge": {"host": "test", "model": "m", "role": role}})
    }

    fn window_for(ws: &Workspace, eval_id: &str, path: &str) -> String {
        let ctx = Ctx::load(ws, eval_id).unwrap();
        array_of(&ctx.evidence, "windows")
            .iter()
            .find(|w| w["path"] == path)
            .map(|w| str_of(w, "id"))
            .unwrap()
    }

    fn ids(v: &Value) -> Vec<String> {
        array_of(v, "tasks").iter().map(|t| str_of(t, "task_id")).collect()
    }

    fn task_of(ws: &Workspace, task: &str) -> Value {
        read_json(&tmp(ws, eval_of(task).unwrap()).join(format!("tasks/{task}.json"))).unwrap()
    }

    /// Answer the map: every window serves step 1 but the telemetry beacon.
    fn map_all(ws: &Workspace, e: &str) {
        let tele = window_for(ws, e, "src/telemetry.js");
        let notes = window_for(ws, e, "docs/notes.md");
        for t in ids(&next(ws, e, 8, None, &[]).unwrap()) {
            let file = task_of(ws, &t);
            let entries: Vec<Value> = array_of(&file, "windows")
                .iter()
                .map(|w| {
                    let id = str_of(w, "id");
                    if id == tele {
                        json!({"window": id, "unexplained": "sends a beacon to example.com",
                               "quote": "fetch('https://example.com/collect')"})
                    } else if id == notes {
                        json!({"window": id, "serves": [S2], "quote": "reports uptime_seconds"})
                    } else {
                        json!({"window": id, "serves": [S1]})
                    }
                })
                .collect();
            let mut doc = head(&t, "map", "map");
            doc["windows"] = json!(entries);
            put(ws, &t, doc);
            assert_eq!(submit(ws, &t).unwrap().0, 0, "{t}");
        }
    }

    fn vote(id: &str, v: &str, w: &str, quote: &str, reason: &str) -> Value {
        json!({"id": id, "vote": v, "reason": reason, "citations": [{"window": w, "quote": quote}]})
    }

    #[test]
    fn a_whole_eval_maps_reduces_confirms_and_merges() {
        eval_bench(|ws| {
            let b = bench(ws);
            let e = &b.eval_id;
            // The map first: the change, once, with every requirement listed.
            let first = next(ws, e, 8, None, &[]).unwrap();
            assert_eq!(ids(&first), [task_id(e, "m1")]);
            let prompt = str_of(&array_of(&first, "tasks")[0], "prompt");
            let m1_id = task_id(e, "m1");
            assert!(
                prompt.contains(".brief.md")
                    && prompt.contains(&m1_id)
                    && prompt.contains("reply with one line"),
                "{prompt}"
            );
            let m1 = task_id(e, "m1");
            // One brief holds it all: the rules, the requirements, every window.
            let brief =
                std::fs::read_to_string(tmp(ws, e).join(format!("tasks/{m1}.brief.md"))).unwrap();
            assert!(brief.contains("Judge from this brief") && brief.contains(S1), "{brief}");
            assert!(brief.trim_end().ends_with("=== end of brief ==="));
            let file = task_of(ws, &m1);
            // The evidence is handed over as lines, each window where the
            // task file says.
            let text = std::fs::read_to_string(str_of(&file, "windows_file")).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            for w in array_of(&file, "windows") {
                let first = w["lines"][0].as_u64().unwrap() as usize;
                assert_eq!(
                    lines[first - 1],
                    format!("=== {} {}", str_of(w, "id"), str_of(w, "path"))
                );
            }
            assert!(
                std::fs::read_to_string(tmp(ws, e).join(format!("tasks/{m1}.json")))
                    .unwrap()
                    .lines()
                    .count()
                    > 5,
                "the task file is lines too"
            );
            let listed: Vec<String> =
                array_of(&file, "requirements").iter().map(|r| str_of(r, "id")).collect();
            assert!(
                listed.contains(&S1.to_string()) && listed.contains(&S2.to_string()),
                "{listed:?}"
            );
            assert!(
                array_of(&file, "requirements").iter().any(|r| r["kind"] == "cross"),
                "the Ask's own rule"
            );

            let up = window_for(ws, e, "src/uptime.js");
            let notes = window_for(ws, e, "docs/notes.md");
            let tele = window_for(ws, e, "src/telemetry.js");
            // A quote the window does not hold is a fabrication; the same
            // text on two windows is templated.
            let mut doc = head(&m1, "map", "map");
            doc["windows"] = json!([
                {"window": up, "unexplained": "reads the wall clock", "quote": "uptime_seconds = () => Date.now()"},
                {"window": notes, "unexplained": "adds a note nobody asked for", "quote": "reports uptime_seconds"},
                {"window": tele, "unexplained": "adds a note nobody asked for", "quote": "fetch('https://example.com/collect')"},
            ]);
            put(ws, &m1, doc);
            let (code, r) = submit(ws, &m1).unwrap();
            assert_eq!(code, 2, "{r}");
            let errs = r["errors"].to_string();
            assert!(
                errs.contains("not in its source") && errs.contains("eval.templated"),
                "{errs}"
            );
            // Handed windows count as read: no fetch needed to cite them.
            let mut doc = head(&m1, "map", "map");
            doc["windows"] = json!([
                {"window": up, "serves": [S1]},
                {"window": notes, "serves": [S2], "quote": "a serves quote is not checked"},
                {"window": tele, "unexplained": "sends a beacon to example.com", "quote": "fetch('https://example.com/collect')"},
            ]);
            put(ws, &m1, doc);
            assert_eq!(submit(ws, &m1).unwrap().0, 0);

            // Reduce: each requirement from the windows the map gave it.
            let second = next(ws, e, 8, None, &[]).unwrap();
            assert_eq!(ids(&second), [task_id(e, "r1")]);
            let r1 = task_id(e, "r1");
            let file = task_of(ws, &r1);
            let reqs = array_of(&file, "requirements");
            let s2 = reqs.iter().find(|r| r["id"] == S2).unwrap();
            assert!(array_of(s2, "windows").iter().any(|w| w == &json!(notes)), "{s2}");
            let mut doc = head(&r1, "reduce", "reduce");
            doc["requirements"] = json!([
                vote(S1, "met", &up, "export const uptime_seconds", "the server exports it"),
                {"id": S2, "vote": "not_met", "reason": "the notes name it but never say how to read it",
                 "missing": "where the value is read from"},
            ]);
            put(ws, &r1, doc);
            assert_eq!(submit(ws, &r1).unwrap().0, 0);

            // Confirm: the lone negative, and the unexplained window.
            let third = next(ws, e, 8, None, &[]).unwrap();
            let got = ids(&third);
            assert!(got.contains(&task_id(e, "cw1")), "{got:?}");
            let cr: Vec<String> = got.iter().filter(|t| t.contains("~cr")).cloned().collect();
            assert!(!cr.is_empty(), "{got:?}");
            assert_eq!(cr.len(), 1, "confirms are packed: {got:?}");
            for t in &cr {
                let file = task_of(ws, t);
                let found = !array_of(&file, "found").is_empty();
                let mut doc = head(t, "reduce", "confirm");
                doc["requirements"] = json!(
                    array_of(&file, "requirements")
                        .iter()
                        .map(|r| {
                            let id = str_of(r, "id");
                            if id == S2 {
                                json!({"id": S2, "vote": "not_met",
                                   "reason": "it names the value but not where to read it",
                                   "missing": "where the value is read from"})
                            } else {
                                vote(
                                    &id,
                                    "met",
                                    &up,
                                    "export const uptime_seconds",
                                    &format!("{id} read afresh"),
                                )
                            }
                        })
                        .collect::<Vec<_>>()
                );
                if found {
                    doc["findings"] =
                        json!([{"finding": "as listed", "means": "it does not change the vote"}]);
                }
                put(ws, t, doc);
                assert_eq!(submit(ws, t).unwrap().0, 0, "{t}");
            }
            let cw = task_id(e, "cw1");
            let mut doc = head(&cw, "map", "confirm");
            doc["windows"] = json!([{"window": tele, "unexplained": "posts to an outside address no step names",
                                     "quote": "https://example.com/collect"}]);
            put(ws, &cw, doc);
            assert_eq!(submit(ws, &cw).unwrap().0, 0);

            // The old close refuses an Eval that runs by tasks.
            let e2 = crate::evaluate::close_eval(ws, e, &json!({}), &[], None, None).unwrap_err();
            assert!(e2.to_string().contains("eval.use_next"));

            let done = next(ws, e, 8, None, &[]).unwrap();
            assert_eq!(done["state"], "done");
            assert_eq!(done["verdict"], "drifted");
            let rec = crate::evaluate::load_record(ws, e).unwrap().unwrap();
            assert_eq!(rec["schema"], RECORD2_SCHEMA);
            let result = |id: &str| {
                array_of(&rec, "items").iter().find(|i| i["id"] == id).unwrap()["result"].clone()
            };
            assert_eq!(result(S1), "met");
            assert_eq!(result(S2), "not_met");
            let cross = array_of(&rec, "items").iter().find(|i| i["kind"] == "cross").unwrap();
            assert_eq!(cross["result"], "met", "no map found a break: {cross}");
            let w = array_of(&rec, "windows").iter().find(|w| w["id"] == json!(tele)).unwrap();
            assert_eq!(w["result"], "unexplained");
            assert_eq!(array_of(&rec, "fabrications").len(), 1, "{}", rec["fabrications"]);
            assert_eq!(rec["templated"], 1);
            assert_eq!(rec["drift"]["omission_requirements"], json!([1, 2]));
            assert!(rec["drift"]["commission"].as_f64().unwrap() > 0.0, "{}", rec["drift"]);
            let roles: BTreeSet<&str> =
                array_of(&rec, "judges").iter().map(|j| j["angle"].as_str().unwrap()).collect();
            assert_eq!(roles, BTreeSet::from(["map", "reduce", "confirm"]));
            let md = crate::evaluate::show_eval(ws, e).unwrap();
            assert!(
                md.contains("## Unexplained changes") && md.contains("## Fabricated citations"),
                "{md}"
            );
            assert!(md.contains("Commission") && md.contains(S2), "{md}");
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
            assert_eq!(next(ws, e, 8, None, &[]).unwrap()["state"], "done");
        });
    }

    #[test]
    fn a_requirement_a_finding_names_is_confirmed_and_a_met_one_it_only_touches_is_not() {
        eval_bench(|ws| {
            // Step 2's note claims commit A; commit B, which no step claims,
            // rewrites A's line in the file that serves step 1.
            commit(&ws.root, &[("plans/plan.md", Some(PLAN))], "plan");
            confirm_ask_as(
                ws,
                "Build it",
                b"build phase 1\n",
                b"/goal Build Phase 1 of plans/plan.md, steps 1 to 2.\n",
                "native_goal",
            );
            let a = commit(
                &ws.root,
                &[
                    ("src/uptime.js", Some(UPTIME)),
                    (
                        "docs/notes.md",
                        Some("The server reports uptime_seconds since it started.\n"),
                    ),
                    (
                        "src/telemetry.js",
                        Some("export const beacon = () => fetch('https://example.com/collect');\n"),
                    ),
                ],
                "work",
            );
            let claimed = PLAN.replace("says so. |", &format!("says so. **Built.** {a} |"));
            commit(&ws.root, &[("plans/plan.md", Some(&claimed))], "as built");
            commit(
                &ws.root,
                &[("src/uptime.js", Some("export const uptime_seconds = () => 0; // cheaper\n"))],
                "tidy",
            );
            let b = Bench { eval_id: str_of(&open_eval(ws, Open::default()).unwrap(), "eval_id") };
            let e = &b.eval_id;
            map_all(ws, e);
            let r1 = task_id(e, "r1");
            assert_eq!(ids(&next(ws, e, 8, None, &[]).unwrap()), std::slice::from_ref(&r1));
            let up = window_for(ws, e, "src/uptime.js");
            let notes = window_for(ws, e, "docs/notes.md");
            let mut doc = head(&r1, "reduce", "reduce");
            doc["requirements"] = json!([
                vote(S1, "met", &up, "export const uptime_seconds", "the server exports it"),
                vote(S2, "met", &notes, "reports uptime_seconds", "the notes say so"),
            ]);
            put(ws, &r1, doc);
            assert_eq!(submit(ws, &r1).unwrap().0, 0);
            let confirms = ids(&next(ws, e, 8, None, &[]).unwrap());
            let confirming = |req: &str| {
                confirms.iter().find(|t| {
                    t.contains("~cr")
                        && array_of(&task_of(ws, t), "requirements").iter().any(|r| r["id"] == req)
                })
            };
            assert!(
                confirming(S1).is_none(),
                "step 1 was met with a citation; the rewrite only touches its window"
            );
            let t = confirming(S2)
                .expect("step 2 is confirmed though met: the unclaimed rewrite names it")
                .clone();
            let file = task_of(ws, &t);
            assert!(
                array_of(&file, "found").iter().any(|f| f["kind"] == "unclaimed_rewrite"),
                "{file}"
            );
            // A confirm answers what RingFrame found before it votes.
            let mut doc = head(&t, "reduce", "confirm");
            doc["requirements"] = json!(
                array_of(&file, "requirements")
                    .iter()
                    .map(|r| {
                        let id = str_of(r, "id");
                        if id == S1 {
                            vote(S1, "met", &up, "export const uptime_seconds", "read afresh")
                        } else if id == S2 {
                            vote(
                                S2,
                                "met",
                                &notes,
                                "reports uptime_seconds",
                                "the notes say so, read afresh",
                            )
                        } else {
                            vote(
                                &id,
                                "met",
                                &notes,
                                "reports uptime_seconds",
                                "the notes say so, again",
                            )
                        }
                    })
                    .collect::<Vec<_>>()
            );
            put(ws, &t, doc.clone());
            let (code, r) = submit(ws, &t).unwrap();
            assert_eq!(code, 2);
            assert!(r["errors"].to_string().contains("answer what RingFrame found"), "{r}");
            doc["findings"] = json!([{"finding": "commit tidy rewrote A",
                                      "means": "the export still returns a number"}]);
            put(ws, &t, doc);
            assert_eq!(submit(ws, &t).unwrap().0, 0);
        });
    }

    #[test]
    fn the_agent_definitions_give_each_role_its_tiers_under_what_was_asked() {
        let dir = crate::testing::tmp_dir();
        write(
            &dir.path().join("rf-map.md"),
            b"---\nname: rf-map\nmodel: haiku\neffort: low  # classification\ntools:\n  - Read\n---\n\nMap.\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("rf-confirm")).unwrap();
        write(&dir.path().join("rf-confirm/agent.md"), b"---\nmodel: \"pro\"\n---\nConfirm.\n")
            .unwrap();
        let defs = agents_from(dir.path()).unwrap();
        assert_eq!(
            defs,
            json!({"map": {"model": "haiku", "effort": "low"}, "confirm": {"model": "pro"}})
        );
        let asked = json!({"confirm": {"effort": "high"}, "trace": {"model": "sonnet"}});
        assert_eq!(
            agents_over(Some(asked), Some(defs)),
            Some(json!({"map": {"model": "sonnet", "effort": "low"},
                        "confirm": {"model": "pro", "effort": "high"}})),
            "what was asked wins key by key, earlier names too"
        );
        assert!(agents_from(crate::testing::tmp_dir().path()).is_err(), "no definition at all");
    }

    /// A function moved to a new file: the window it left holds only removed
    /// lines and the move, so RingFrame answers it and no judge reads it.
    #[test]
    fn code_moved_away_follows_from_where_it_went_without_a_judge() {
        eval_bench(|ws| {
            let body = "pub fn tally(votes: &[u32]) -> u32 {\n    let mut total = 0;\n    for v in votes {\n        total += v;\n    }\n    total\n}\n";
            commit(
                &ws.root,
                &[
                    ("plans/plan.md", Some(PLAN)),
                    ("src/lib.rs", Some(&format!("pub mod keep;\n\n{body}"))),
                ],
                "plan",
            );
            confirm_ask_as(
                ws,
                "Build it",
                b"build phase 1\n",
                b"/goal Build Phase 1 of plans/plan.md, steps 1 to 2.\n",
                "native_goal",
            );
            commit(
                &ws.root,
                &[
                    ("src/lib.rs", Some("pub mod keep;\n")),
                    ("src/main.rs", Some("mod tally;\n")),
                    ("src/tally.rs", Some(body)),
                    ("src/uptime.js", Some(UPTIME)),
                ],
                "work",
            );
            let e = &str_of(&open_eval(ws, Open::default()).unwrap(), "eval_id");
            let ctx = Ctx::load(ws, e).unwrap();
            let st = status(&ctx).unwrap();
            let lib = window_for(ws, e, "src/lib.rs");
            let dest = window_for(ws, e, "src/tally.rs");
            assert!(ctx.windows[&lib].contains("~ moved"), "{}", ctx.windows[&lib]);
            assert_eq!(st.moved.get(&lib), Some(&dest), "{:?}", st.moved);
            let first = next(ws, e, 8, None, &[]).unwrap();
            assert_eq!(array_of(&first, "tasks")[0]["agent"], "rf-map");
            for t in ids(&first) {
                let handed: Vec<String> =
                    array_of(&task_of(ws, &t), "windows").iter().map(|w| str_of(w, "id")).collect();
                assert!(!handed.contains(&lib), "no judge reads code moved away");
            }
            assert_eq!(map_answers(&st)[&lib]["map"]["by"], "ringframe");
        });
    }

    /// Answer every task until the Eval closes: the map as `map_all` does, each
    /// requirement met, each unexplained window unexplained again.
    fn close_all(ws: &Workspace, e: &str, out_already: &[String]) -> Value {
        let ctx = Ctx::load(ws, e).unwrap();
        let text = |w: &str| ctx.windows.get(w).cloned().unwrap_or_default();
        let quote_of = |w: &str| -> String {
            let t = text(w);
            let line = t.lines().find(|l| l.starts_with('+') && l.len() > 12).unwrap_or("+");
            line[1..].chars().take(24).collect()
        };
        let mut returned = out_already.to_vec();
        for _ in 0..10 {
            let out = next(ws, e, 8, None, &std::mem::take(&mut returned)).unwrap();
            if out["state"] == "done" {
                return crate::evaluate::load_record(ws, e).unwrap().unwrap();
            }
            for t in ids(&out) {
                let file = task_of(ws, &t);
                let (kind, role) = (str_of(&file, "kind"), str_of(&file, "role"));
                let mut doc = head(&t, &kind, &role);
                if kind == "map" {
                    doc["windows"] = json!(array_of(&file, "windows").iter().map(|w| {
                        let id = str_of(w, "id");
                        if str_of(w, "path") == "src/telemetry.js" {
                            json!({"window": id, "unexplained": format!("posts outside ({t})"), "quote": quote_of(&id)})
                        } else if str_of(w, "path") == "docs/notes.md" {
                            json!({"window": id, "serves": [S2]})
                        } else {
                            json!({"window": id, "serves": [S1]})
                        }
                    }).collect::<Vec<_>>());
                } else {
                    doc["requirements"] = json!(
                        array_of(&file, "requirements")
                            .iter()
                            .map(|r| {
                                let id = str_of(r, "id");
                                let w = str_of_value(&array_of(r, "windows")[0]);
                                vote(
                                    &id,
                                    "met",
                                    &w,
                                    &quote_of(&w),
                                    &format!("{id} is done ({role})"),
                                )
                            })
                            .collect::<Vec<_>>()
                    );
                    if !array_of(&file, "found").is_empty() {
                        doc["findings"] =
                            json!([{"finding": "as listed", "means": "no change to the vote"}]);
                    }
                }
                put(ws, &t, doc);
                assert_eq!(submit(ws, &t).unwrap().0, 0, "{t}");
            }
        }
        panic!("the Eval did not close");
    }

    #[test]
    fn a_window_stays_in_its_confirm_batch_once_its_confirm_lands() {
        let map = json!({"unexplained": "posts to an outside address"});
        let reading = |confirm: Option<Value>| {
            let mut a = BTreeMap::from([("map".to_string(), map.clone())]);
            if let Some(c) = confirm {
                a.insert("confirm".to_string(), c);
            }
            a
        };
        assert!(needs_window_confirm(&reading(None)), "not yet confirmed");
        assert!(
            needs_window_confirm(&reading(Some(json!({"unexplained": "still"})))),
            "confirmed in this Eval: its batch keeps it, so later batches keep their ids"
        );
        assert!(
            !needs_window_confirm(&reading(Some(json!({"unexplained": "x", "carried_from": "evl_1"})))),
            "carried from an earlier Eval: read again by no one"
        );
        let explained = BTreeMap::from([("map".to_string(), json!({"serves": ["s1"]}))]);
        assert!(!needs_window_confirm(&explained));
    }

    #[test]
    fn a_window_key_drops_line_numbers_but_keeps_the_item_and_the_path() {
        let w = |path: &str, at: &str, item: &str| {
            format!("--- {path}\n@@ {at} @@ {item}\n     let a = 1;\n+    log(a);\n")
        };
        let key = |t: String| window_key(&t);
        assert_eq!(
            key(w("src/a.rs", "-1,2 +1,3", "fn alpha() {")),
            key(w("src/a.rs", "-40,2 +52,3", "fn alpha() {")),
            "only the numbers moved"
        );
        assert_ne!(
            key(w("src/a.rs", "-1,2 +1,3", "fn alpha() {")),
            key(w("src/a.rs", "-9,2 +9,3", "fn beta() {")),
            "the same lines in another item"
        );
        assert_ne!(
            key(w("src/a.rs", "-1,2 +1,3", "fn alpha() {")),
            key(w("src/b.rs", "-1,2 +1,3", "fn alpha() {")),
            "the same lines in another file"
        );
    }

    /// A second Eval of the same Asks judges only what changed: here the
    /// beacon alone, so both steps keep their verdicts and no reduce runs.
    #[test]
    fn a_later_eval_judges_only_what_changed_and_carries_the_rest() {
        eval_bench(|ws| {
            let first = bench(ws).eval_id;
            let rec1 = close_all(ws, &first, &[]);
            assert_eq!(rec1["carried"]["from"], Value::Null);
            commit(
                &ws.root,
                &[(
                    "src/telemetry.js",
                    Some("export const beacon = () => fetch('https://example.com/v2');\n"),
                )],
                "beacon v2",
            );
            let second = str_of(&open_eval(ws, Open::default()).unwrap(), "eval_id");
            let out = next(ws, &second, 8, None, &[]).unwrap();
            let handed: Vec<String> = ids(&out)
                .iter()
                .flat_map(|t| {
                    array_of(&task_of(ws, t), "windows")
                        .iter()
                        .map(|w| str_of(w, "path"))
                        .collect::<Vec<_>>()
                })
                .collect();
            assert_eq!(handed, ["src/telemetry.js"], "only the changed window is mapped");
            let rec2 = close_all(ws, &second, &ids(&out));
            assert_eq!(rec2["carried"]["from"], json!(first));
            assert_eq!(rec2["carried"]["requirements"], 2, "{}", rec2["carried"]);
            for id in [S1, S2] {
                let item = array_of(&rec2, "items").iter().find(|i| i["id"] == id).unwrap().clone();
                assert_eq!(
                    (item["result"].clone(), item["carried_from"].clone()),
                    (json!("met"), json!(first))
                );
            }
            let tasks: Vec<String> = std::fs::read_dir(tmp(ws, &second).join("tasks"))
                .unwrap()
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|n| n.ends_with(".json"))
                .collect();
            assert!(tasks.iter().all(|n| !n.contains("~r")), "no reduce: {tasks:?}");
            assert!(render(&rec2).contains("Carried from"), "the report says what was carried");
        });
    }

    /// A judge writes only its answers: RingFrame fills in the task, its kind
    /// and role, and takes who judged from the harness the skill named and
    /// the model the role was asked to run on, never from the judge's word.
    #[test]
    fn a_judge_writes_only_its_answers_and_the_record_names_who_judged_from_facts() {
        eval_bench(|ws| {
            let e = &bench(ws).eval_id;
            note_host(ws, e, "codex").unwrap();
            let agents = json!({"map": {"model": "gpt-6-luna", "effort": "medium"}});
            let first = next(ws, e, 8, Some(&agents), &[]).unwrap();
            let m1 = task_id(e, "m1");
            let file = task_of(ws, &m1);
            let windows: Vec<Value> = array_of(&file, "windows")
                .iter()
                .map(|w| {
                    let id = str_of(w, "id");
                    if str_of(w, "path") == "src/telemetry.js" {
                        json!({"window": id, "unexplained": "posts outside", "quote": "https://example.com/collect"})
                    } else if str_of(w, "path") == "docs/notes.md" {
                        json!({"window": id, "serves": [S2]})
                    } else {
                        json!({"window": id, "serves": [S1]})
                    }
                })
                .collect();
            // No schema, no task id, a wrong role and a made-up model.
            put(
                ws,
                &m1,
                json!({"windows": windows, "judge": {"role": "initial", "model": "haiku"}}),
            );
            assert_eq!(submit(ws, &m1).unwrap().0, 0, "{}", ids(&first).join(","));
            let rec = close_all(ws, e, &[]);
            let judges = array_of(&rec, "judges");
            assert!(
                judges.iter().any(|j| j["angle"] == "map"
                    && j["host"] == "codex"
                    && j["model"] == "gpt-6-luna"),
                "{judges:?}"
            );
            assert!(judges.iter().all(|j| j["model"] != "haiku"), "{judges:?}");
        });
    }

    /// A window a map says undoes a step's work is evidence for that step,
    /// needs a quote that checks, and sends the step to be read again even
    /// when the reduce calls it met (q04: undoing windows were called
    /// unexplained or filed under another step, and step 7 passed).
    #[test]
    fn a_window_that_undoes_a_step_is_its_evidence_and_it_is_read_again() {
        eval_bench(|ws| {
            let e = &bench(ws).eval_id;
            let first = next(ws, e, 8, None, &[]).unwrap();
            let m1 = ids(&first)[0].clone();
            let up = window_for(ws, e, "src/uptime.js");
            let notes = window_for(ws, e, "docs/notes.md");
            let tele = window_for(ws, e, "src/telemetry.js");
            let mut doc = head(&m1, "map", "map");
            doc["windows"] = json!([
                {"window": up, "serves": [S1]},
                {"window": notes, "undoes": [S2]},
                {"window": tele, "unexplained": "posts outside", "quote": "https://example.com/collect"},
            ]);
            put(ws, &m1, doc.clone());
            let (_, r) = submit(ws, &m1).unwrap();
            assert!(
                r["errors"].to_string().contains("a quote is 8 to 300"),
                "undoes is quoted: {r}"
            );
            doc["windows"][1]["quote"] = json!("reports uptime_seconds");
            put(ws, &m1, doc);
            assert_eq!(submit(ws, &m1).unwrap().0, 0);
            let r1 = task_id(e, "r1");
            next(ws, e, 8, None, &[]).unwrap();
            let s2 = array_of(&task_of(ws, &r1), "requirements")
                .iter()
                .find(|r| r["id"] == S2)
                .unwrap()
                .clone();
            assert!(
                array_of(&s2, "windows").iter().any(|w| w == &json!(notes)),
                "its evidence: {s2}"
            );
            let mut doc = head(&r1, "reduce", "reduce");
            doc["requirements"] = json!([
                vote(S1, "met", &up, "export const uptime_seconds", "the server exports it"),
                vote(S2, "met", &notes, "reports uptime_seconds", "the notes say so"),
            ]);
            put(ws, &r1, doc);
            assert_eq!(submit(ws, &r1).unwrap().0, 0);
            let confirmed: Vec<String> = ids(&next(ws, e, 8, None, &[]).unwrap())
                .iter()
                .filter(|t| t.contains("~cr"))
                .flat_map(|t| {
                    array_of(&task_of(ws, t), "requirements")
                        .iter()
                        .map(|r| str_of(r, "id"))
                        .collect::<Vec<_>>()
                })
                .collect();
            assert!(
                confirmed.contains(&S2.to_string()),
                "undone, read again though met: {confirmed:?}"
            );
            assert!(!confirmed.contains(&S1.to_string()), "{confirmed:?}");
        });
    }

    #[test]
    fn parts_combine_and_a_negative_needs_a_second_reading() {
        let v = |x: &str| json!({"vote": x});
        assert_eq!(combine(&[v("met")]), "met");
        assert_eq!(combine(&[v("part_met"), v("met")]), "met");
        assert_eq!(
            combine(&[v("part_met"), v("part_met")]),
            "unknown",
            "one part must meet the whole"
        );
        assert_eq!(combine(&[v("met"), v("not_met")]), "not_met");
        assert_eq!(settle(&[v("not_met")], &[]).0, "unsettled", "a lone negative does not count");
        assert_eq!(settle(&[v("not_met")], &[v("not_met")]), ("not_met", 1.0));
        assert_eq!(settle(&[v("met")], &[]), ("met", 1.0));
        assert_eq!(settle(&[v("unknown")], &[v("not_met")]).0, "not_met", "a re-read decides");
        assert_eq!(settle(&[v("met")], &[v("unknown")]).0, "unsettled");
        // A confirm that contradicts the first reading does not win alone: a
        // third reading decides, and while they split the step is unsettled.
        assert_eq!(settle(&[v("met")], &[v("not_met")]), ("unsettled", 0.0));
        assert_eq!(settle(&[v("not_met")], &[v("met")]).0, "unsettled");
        assert_eq!(settle(&[v("met")], &[v("not_met"), v("not_met")]), ("not_met", 0.67));
        assert_eq!(settle(&[v("not_met")], &[v("met"), v("not_met")]).0, "not_met");
        assert_eq!(settle(&[v("not_met")], &[v("met"), v("unknown")]).0, "unsettled");
    }

    #[test]
    fn a_roles_effort_is_in_its_task_file_and_its_instructions() {
        eval_bench(|ws| {
            let b = bench(ws);
            let e = &b.eval_id;
            let agents = json!({"map": {"model": "m", "effort": "high"}});
            let first = next(ws, e, 8, Some(&agents), &[]).unwrap();
            assert_eq!(first["tasks"][0]["effort"], "high");
            assert_eq!(task_of(ws, &task_id(e, "m1"))["effort"], "high");
            let m1 = task_id(e, "m1");
            let md =
                std::fs::read_to_string(tmp(ws, e).join(format!("tasks/{m1}.brief.md"))).unwrap();
            assert!(md.contains("Your effort: `high`"), "{md}");
            assert!(md.contains("within your effort's budget") && md.contains("up to 15"), "{md}");
        });
    }

    #[test]
    fn the_roles_of_whoever_runs_the_tasks_fill_what_the_open_left_unset() {
        eval_bench(|ws| {
            let b = bench(ws);
            let mine = json!({"map": {"model": "strong", "effort": "high"}});
            let first = next(ws, &b.eval_id, 8, Some(&mine), &[]).unwrap();
            assert_eq!(first["tasks"][0]["model"], "strong");
            assert_eq!(first["tasks"][0]["effort"], "high");
        });
    }

    #[test]
    fn the_judges_are_told_what_they_must_not_do_as_the_eval_recorded_it() {
        let open_with = |deny: Option<(&'static str, Vec<String>)>| {
            let mut md = String::new();
            eval_bench(|ws| {
                commit(&ws.root, &[("plans/plan.md", Some(PLAN))], "plan");
                confirm_ask_as(
                    ws,
                    "Build it",
                    b"build\n",
                    b"/goal Build Phase 1 of plans/plan.md.\n",
                    "native_goal",
                );
                commit(&ws.root, &[("src/uptime.js", Some(UPTIME))], "work");
                let out = open_eval(ws, Open { deny: deny.clone(), ..Default::default() }).unwrap();
                let e = str_of(&out, "eval_id");
                next(ws, &e, 8, None, &[]).unwrap();
                assert!(tmp(ws, &e).join("tasks/scratch").is_dir(), "judges have a scratch folder");
                let m1 = task_id(&e, "m1");
                md = std::fs::read_to_string(tmp(ws, &e).join(format!("tasks/{m1}.brief.md")))
                    .unwrap();
            });
            md
        };
        let md = open_with(None);
        assert!(md.contains("scratch/<task id>/"), "{md}");
        assert!(md.contains("You must not:\n- Change the work:"), "{md}");
        let md = open_with(Some(("mine", vec!["Run the network tests.".into()])));
        assert!(md.contains("You must not:\n- Run the network tests.\n"), "{md}");
        assert!(!md.contains("Change the work:"), "a layer's list replaces the default: {md}");
        let md = open_with(Some(("mine", Vec::new())));
        assert!(!md.contains("You must not"), "an empty list denies nothing: {md}");
    }

    #[test]
    fn a_file_in_ringframes_own_folder_is_not_a_citation() {
        eval_bench(|ws| {
            // Work left uncommitted, so a cited file is read from disk.
            commit(&ws.root, &[("plans/plan.md", Some(PLAN))], "plan");
            confirm_ask_as(
                ws,
                "Build it",
                b"build\n",
                b"/goal Build Phase 1 of plans/plan.md, steps 1 to 2.\n",
                "native_goal",
            );
            for (path, text) in [
                ("src/uptime.js", UPTIME),
                ("docs/notes.md", "The server reports uptime_seconds since it started.\n"),
                (
                    "src/telemetry.js",
                    "export const beacon = () => fetch('https://example.com/collect');\n",
                ),
            ] {
                write(&ws.root.join(path), text.as_bytes()).unwrap();
            }
            let e = str_of(&open_eval(ws, Open::default()).unwrap(), "eval_id");
            map_all(ws, &e);
            let r1 = task_id(&e, "r1");
            next(ws, &e, 8, None, &[]).unwrap();
            let planted = tmp(ws, &e).join("tasks/scratch/proof.txt");
            write(&planted, b"uptime_seconds is exported and tested\n").unwrap();
            let rel = planted.strip_prefix(&ws.root).unwrap().to_string_lossy().to_string();
            let mut doc = head(&r1, "reduce", "reduce");
            doc["requirements"] = json!([
                {"id": S1, "vote": "met", "reason": "a judge wrote it",
                 "citations": [{"file": rel, "lines": [1, 1], "quote": "uptime_seconds is exported"}]},
                {"id": S2, "vote": "met", "reason": "the notes say so",
                 "citations": [{"file": "docs/notes.md", "lines": [1, 1], "quote": "The server reports uptime_seconds"}]},
            ]);
            put(ws, &r1, doc);
            let (_, r) = submit(ws, &r1).unwrap();
            let errors = r["errors"].as_array().cloned().unwrap_or_default();
            assert!(
                errors.iter().any(|e| e.to_string().contains("is not a file of the subject")),
                "{r}"
            );
            assert!(
                errors.iter().all(|e| !e.to_string().contains("requirements[1]")),
                "the real file checks: {r}"
            );
        });
    }

    #[test]
    fn a_met_needs_a_citation_that_checks_and_an_unknown_says_what_is_missing() {
        eval_bench(|ws| {
            let b = bench(ws);
            let e = &b.eval_id;
            map_all(ws, e);
            next(ws, e, 8, None, &[]).unwrap();
            let r1 = task_id(e, "r1");
            let mut doc = head(&r1, "reduce", "reduce");
            doc["requirements"] = json!([
                {"id": S1, "vote": "met", "reason": "it is there", "citations": []},
                {"id": S2, "vote": "unknown", "reason": "cannot tell"},
            ]);
            put(ws, &r1, doc);
            let (_, r) = submit(ws, &r1).unwrap();
            let errors = r["errors"].to_string();
            assert!(errors.contains("at least one citation"), "{errors}");
            assert!(errors.contains("what is missing"), "{errors}");
        });
    }

    #[test]
    fn a_map_answers_every_window_with_exactly_one_reason() {
        eval_bench(|ws| {
            let b = bench(ws);
            let e = &b.eval_id;
            next(ws, e, 8, None, &[]).unwrap();
            let m1 = task_id(e, "m1");
            let up = window_for(ws, e, "src/uptime.js");
            let mut doc = head(&m1, "map", "map");
            doc["windows"] = json!([
                {"window": up, "serves": [S1], "follows_from": up, "quote": "uptime_seconds = ()"},
            ]);
            put(ws, &m1, doc);
            let (_, r) = submit(ws, &m1).unwrap();
            let errors = r["errors"].to_string();
            assert!(errors.contains("exactly one of serves"), "{errors}");
            assert!(errors.contains("no entry for"), "every window: {errors}");
            // A quote the window does not hold, and a break of a local step.
            let mut doc = head(&m1, "map", "map");
            doc["windows"] = json!([{"window": up, "unexplained": "counts minutes",
                                     "quote": "uptime_minutes = ()"}]);
            doc["breaks"] = json!([{"requirement": S1, "window": up,
                                   "quote": "uptime_seconds = ()", "reason": "r"}]);
            put(ws, &m1, doc);
            let (_, r) = submit(ws, &m1).unwrap();
            let errors = r["errors"].to_string();
            assert!(errors.contains("the quote is not in its source"), "{errors}");
            assert!(
                errors.contains("the line most like it is: +export const uptime_seconds"),
                "a misquote is shown what it most likely meant: {errors}"
            );
            assert!(errors.contains("a break names a cross-cutting requirement"), "{errors}");
        });
    }

    /// Two large files, one with a TODO: each is one unit and stays in one map
    /// leaf, the leaf with the finding goes out first, and a step whose
    /// evidence is over `REQUIREMENT_BYTES` is reduced in parts.
    #[test]
    fn leaves_keep_units_whole_go_riskiest_first_and_split_a_large_requirement() {
        eval_bench(|ws| {
            // One object: one definition, so one unit.
            let big = |name: &str, todo: bool| {
                let mut s = format!("export const table_{name} = {{\n");
                for i in 0..800 {
                    if todo && i == 790 {
                        s.push_str("  // TODO tidy\n");
                    }
                    s.push_str(&format!("  {name}_{i}: \"{}\",\n", "x".repeat(60)));
                }
                s.push_str("};\n");
                s
            };
            commit(&ws.root, &[("plans/plan.md", Some(PLAN))], "plan");
            confirm_ask_as(
                ws,
                "Build it",
                b"build phase 1\n",
                b"/goal Build Phase 1 of plans/plan.md, steps 1 to 2.\n",
                "native_goal",
            );
            commit(
                &ws.root,
                &[
                    ("src/a_big.js", Some(&big("a", false))),
                    ("src/b_big.js", Some(&big("b", true))),
                    ("src/uptime.js", Some(UPTIME)),
                    (
                        "docs/notes.md",
                        Some("The server reports uptime_seconds since it started.\n"),
                    ),
                ],
                "work",
            );
            let e = &str_of(&open_eval(ws, Open::default()).unwrap(), "eval_id");
            let ctx = Ctx::load(ws, e).unwrap();
            let windows_in = |path: &str| -> BTreeSet<String> {
                array_of(&ctx.evidence, "windows")
                    .iter()
                    .filter(|w| w["path"] == path)
                    .map(|w| str_of(w, "id"))
                    .collect()
            };
            let (a, b) = (windows_in("src/a_big.js"), windows_in("src/b_big.js"));
            assert!(a.len() > 1 && b.len() > 1, "each file is several windows");
            let maps = ids(&next(ws, e, 8, None, &[]).unwrap());
            assert!(maps.len() >= 2, "{maps:?}");
            let leaf = |t: &str| -> BTreeSet<String> {
                array_of(&task_of(ws, t), "windows").iter().map(|w| str_of(w, "id")).collect()
            };
            for unit in [&a, &b] {
                let holding: Vec<&String> =
                    maps.iter().filter(|t| !leaf(t).is_disjoint(unit)).collect();
                assert_eq!(holding.len(), 1, "one unit, one leaf: {holding:?}");
            }
            assert!(b.is_subset(&leaf(&maps[0])), "the TODO's leaf goes out first");
            let notes = window_for(ws, e, "docs/notes.md");
            for t in &maps {
                let entries: Vec<Value> = array_of(&task_of(ws, t), "windows")
                    .iter()
                    .map(|w| {
                        let id = str_of(w, "id");
                        let text = ctx.windows.get(&id).cloned().unwrap_or_default();
                        let serves = if id == notes { S2 } else { S1 };
                        let line = text.lines().find(|l| l.starts_with('+') && l.len() > 20);
                        json!({"window": id, "serves": [serves], "quote": &line.unwrap()[1..21]})
                    })
                    .collect();
                let mut doc = head(t, "map", "map");
                doc["windows"] = json!(entries);
                put(ws, t, doc);
                assert_eq!(submit(ws, t).unwrap().0, 0, "{t}");
            }
            let reduces: Vec<Value> =
                ids(&next(ws, e, 8, None, &[]).unwrap()).iter().map(|t| task_of(ws, t)).collect();
            let parts: Vec<String> = reduces
                .iter()
                .flat_map(|f| array_of(f, "requirements").to_vec())
                .filter(|r| r["id"] == S1)
                .map(|r| str_of(&r, "part"))
                .collect();
            assert!(parts.len() >= 2 && parts.iter().all(|p| p.contains('/')), "{parts:?}");
            for f in &reduces {
                let reqs = array_of(f, "requirements");
                if reqs.iter().any(|r| r["id"] == S1) {
                    assert_eq!(reqs.len(), 1, "a part is alone in its task");
                }
            }
        });
    }

    #[test]
    fn a_check_the_plan_names_runs_once_and_its_warnings_on_added_lines_are_findings() {
        eval_bench(|ws| {
            let plan =
                PLAN.replace("| A test calls `uptime_seconds`. |", "| `make check` is clean. |");
            commit(&ws.root, &[("plans/plan.md", Some(&plan))], "plan");
            confirm_ask_as(
                ws,
                "Build it",
                b"build phase 1\n",
                b"/goal Build Phase 1 of plans/plan.md, steps 1 to 2.\n",
                "native_goal",
            );
            commit(
                &ws.root,
                &[
                    ("src/uptime.js", Some(UPTIME)),
                    (
                        "docs/notes.md",
                        Some("The server reports uptime_seconds since it started.\n"),
                    ),
                    (
                        "src/telemetry.js",
                        Some("export const beacon = () => fetch('https://example.com/collect');\n"),
                    ),
                    (
                        "Makefile",
                        Some(
                            "check:\n\t@echo 'src/uptime.js:1:14: warning: uptime is never tested'\n\t@echo 'src/old.js:3:1: warning: not in the change'\n",
                        ),
                    ),
                ],
                "work",
            );
            let e = str_of(&open_eval(ws, Open::default()).unwrap(), "eval_id");
            let ctx = Ctx::load(ws, &e).unwrap();
            assert_eq!(ctx.brief["checks"], json!(["make check"]));
            let facts = ctx.facts();
            let check = facts.iter().find(|f| f["by"] == "ringframe").expect("the check is a fact");
            assert_eq!(
                (str_of(check, "command"), str_of(check, "outcome")),
                ("make check".into(), "succeeded".into())
            );
            let uptime = window_for(ws, &e, "src/uptime.js");
            let warned: Vec<&Value> = array_of(&ctx.changes, "findings")
                .iter()
                .filter(|f| f["kind"] == "new_warning")
                .collect();
            assert_eq!(warned.len(), 1, "only the warning on a line the change added: {warned:?}");
            assert_eq!(warned[0]["windows"], json!([uptime]));
            map_all(ws, &e);
            let reduces = ids(&next(ws, &e, 8, None, &[]).unwrap());
            assert!(
                array_of(&task_of(ws, &reduces[0]), "facts").iter().any(|f| f["by"] == "ringframe"),
                "a judge that votes has the check among its facts"
            );
            let rec = close_all(ws, &e, &reduces);
            assert_eq!(array_of(&rec, "checks").len(), 1);
            let md = render(&rec);
            assert!(
                md.contains("## Checks RingFrame ran") && md.contains("`make check`: succeeded"),
                "{md}"
            );
            assert!(
                md.contains("new_warning `make check`: warning: uptime is never tested"),
                "{md}"
            );
        });
    }

    #[test]
    fn a_check_still_running_when_the_eval_closes_is_said_and_waited_for_by_nothing() {
        eval_bench(|ws| {
            let b = bench(ws);
            let e = &b.eval_id;
            write(
                &tmp(ws, e).join("checks.plan.json"),
                json!({"commands": ["cargo test -p slow"]}).to_string().as_bytes(),
            )
            .unwrap();
            let rec = close_all(ws, e, &[]);
            assert!(array_of(&rec, "checks").is_empty());
            assert!(
                array_of(&rec, "limitations").iter().any(|l| {
                    let l = str_of_value(l);
                    l.contains("still running when the Eval closed")
                        && l.contains("`cargo test -p slow`")
                }),
                "{rec}"
            );
        });
    }

    #[test]
    fn a_last_try_keeps_the_answers_that_check_and_leaves_only_the_rest_unjudged() {
        eval_bench(|ws| {
            let b = bench(ws);
            let e = &b.eval_id;
            let tele = window_for(ws, e, "src/telemetry.js");
            let m1 = task_id(e, "m1");
            next(ws, e, 8, None, &[]).unwrap();
            let entries: Vec<Value> = array_of(&task_of(ws, &m1), "windows")
                .iter()
                .map(|w| {
                    let id = str_of(w, "id");
                    if id == tele {
                        json!({"window": id, "unexplained": "sends a beacon out",
                               "quote": "a line this window does not hold"})
                    } else if str_of(w, "path") == "docs/notes.md" {
                        json!({"window": id, "serves": [S2]})
                    } else {
                        json!({"window": id, "serves": [S1]})
                    }
                })
                .collect();
            let mut doc = head(&m1, "map", "map");
            doc["windows"] = json!(entries);
            put(ws, &m1, doc);
            assert_eq!(submit(ws, &m1).unwrap().0, 2);
            assert_eq!(submit(ws, &m1).unwrap().0, 2);
            let (code, out) = submit(ws, &m1).unwrap();
            assert_eq!((code, out["state"].as_str()), (0, Some("accepted")), "{out}");
            assert_eq!(out["unjudged"], json!([tele]), "only the window that did not check");
            let rec = close_all(ws, e, &[]);
            let result = |path: &str| {
                array_of(&rec, "windows")
                    .iter()
                    .find(|w| w["path"] == path)
                    .map(|w| str_of(w, "result"))
            };
            assert_eq!(result("src/telemetry.js").as_deref(), Some("not_judged"));
            assert_eq!(result("src/uptime.js").as_deref(), Some("required"), "{rec}");
            assert!(
                array_of(&rec, "limitations")
                    .iter()
                    .any(|l| str_of_value(l).contains("kept without 1 of its entries")),
                "the record says what went unjudged"
            );
        });
    }

    #[test]
    fn a_confirm_that_contradicts_the_first_reading_is_read_a_third_time() {
        for (third, want) in [("not_met", "not_met"), ("met", "met")] {
            eval_bench(|ws| {
                let b = bench(ws);
                let e = &b.eval_id;
                map_all(ws, e);
                let ctx = Ctx::load(ws, e).unwrap();
                let quote_of = |w: &str| -> String {
                    let t = ctx.windows.get(w).cloned().unwrap_or_default();
                    let line =
                        t.lines().find(|l| l.starts_with('+') && l.len() > 12).unwrap_or("+");
                    line[1..].chars().take(24).collect()
                };
                let mut asked = Vec::new();
                for _ in 0..10 {
                    let out = next(ws, e, 8, None, &[]).unwrap();
                    if out["state"] == "done" {
                        break;
                    }
                    for t in ids(&out) {
                        let file = task_of(ws, &t);
                        let (kind, role) = (str_of(&file, "kind"), str_of(&file, "role"));
                        let local = t.split('~').nth(1).unwrap().to_string();
                        asked.push(local.chars().take(2).collect::<String>());
                        let mut doc = head(&t, &kind, &role);
                        if kind == "map" {
                            doc["windows"] = json!(array_of(&file, "windows").iter().map(|w| {
                                let id = str_of(w, "id");
                                json!({"window": id, "unexplained": format!("posts outside ({t})"),
                                       "quote": quote_of(&id)})
                            }).collect::<Vec<_>>());
                        } else {
                            let vote_for = |id: &str| -> &str {
                                match (local.as_str(), id == S1) {
                                    (l, true) if l.starts_with("cr") => "met",
                                    (l, true) if l.starts_with("ct") => third,
                                    (_, true) => "not_met",
                                    _ => "met",
                                }
                            };
                            doc["requirements"] = json!(array_of(&file, "requirements").iter().map(|r| {
                                let id = str_of(r, "id");
                                let w = str_of_value(&array_of(r, "windows")[0]);
                                match vote_for(&id) {
                                    "met" => vote(&id, "met", &w, &quote_of(&w), &format!("shown in {t}")),
                                    v => json!({"id": id, "vote": v, "reason": format!("{t}: absent"),
                                                "missing": format!("{t}: the test")}),
                                }
                            }).collect::<Vec<_>>());
                            if role == "confirm" {
                                doc["findings"] = json!(array_of(&file, "found").iter().enumerate()
                                    .map(|(i, _)| json!({"finding": "as listed", "means": format!("{t} {i}")}))
                                    .collect::<Vec<_>>());
                            }
                        }
                        put(ws, &t, doc);
                        assert_eq!(
                            submit(ws, &t).unwrap().0,
                            0,
                            "{t}: {}",
                            submit(ws, &t).unwrap_err()
                        );
                    }
                }
                assert!(asked.contains(&"ct".to_string()), "a third reading was asked: {asked:?}");
                let rec = crate::evaluate::load_record(ws, e).unwrap().unwrap();
                let item = array_of(&rec, "items").iter().find(|i| i["id"] == S1).unwrap().clone();
                assert_eq!(str_of(&item, "result"), want, "{item}");
                let roles: Vec<String> =
                    array_of(&item, "votes").iter().map(|v| str_of(v, "role")).collect();
                assert_eq!(roles, ["reduce", "confirm", "confirm"], "every reading is shown");
            });
        }
    }

    #[test]
    fn a_task_refused_three_times_fails_and_its_windows_are_not_judged() {
        eval_bench(|ws| {
            let b = bench(ws);
            let e = &b.eval_id;
            next(ws, e, 8, None, &[]).unwrap();
            let m1 = task_id(e, "m1");
            put(ws, &m1, json!({"not": "a task output"}));
            assert_eq!(submit(ws, &m1).unwrap().0, 2);
            assert_eq!(submit(ws, &m1).unwrap().0, 2);
            assert_eq!(submit(ws, &m1).unwrap().0, 3);
            refused(submit(ws, &m1).unwrap_err(), "eval.task_done");
            let ctx = Ctx::load(ws, e).unwrap();
            assert!(status(&ctx).unwrap().failed.contains(&m1));
        });
    }

    #[test]
    fn a_task_whose_judge_finished_without_submitting_goes_out_again_at_once_then_fails() {
        eval_bench(|ws| {
            let b = bench(ws);
            let e = &b.eval_id;
            let t = task_id(e, "m1");
            assert_eq!(ids(&next(ws, e, 8, None, &[]).unwrap()), std::slice::from_ref(&t));
            assert!(ids(&next(ws, e, 8, None, &[]).unwrap()).is_empty(), "out, so not again");
            let back = std::slice::from_ref(&t);
            assert_eq!(ids(&next(ws, e, 8, None, back).unwrap()), back, "back: out again now");
            let after = next(ws, e, 8, None, back).unwrap();
            assert!(!ids(&after).contains(&t), "back a second time: failed");
            let ctx = Ctx::load(ws, e).unwrap();
            assert!(status(&ctx).unwrap().failed.contains(&t));
            let failed = store::events(ws)
                .unwrap()
                .into_iter()
                .rfind(|ev| ev["type"] == "eval.task" && ev["data"]["task_id"] == t)
                .unwrap();
            assert!(
                failed["data"].to_string().contains("finished without an accepted"),
                "{failed}"
            );
        });
    }

    #[test]
    fn a_judge_that_finished_with_its_task_accepted_changes_nothing() {
        eval_bench(|ws| {
            let b = bench(ws);
            let e = &b.eval_id;
            map_all(ws, e);
            let t = task_id(e, "m1");
            let round = next(ws, e, 8, None, std::slice::from_ref(&t)).unwrap();
            assert!(!ids(&round).contains(&t));
            let st = status(&Ctx::load(ws, e).unwrap()).unwrap();
            assert!(st.accepted.contains_key(&t) && !st.failed.contains(&t));
            assert!(read_json(&state_path(ws, e)).unwrap()["returned"].get(&t).is_none());
        });
    }

    #[test]
    fn a_task_not_submitted_in_time_goes_out_once_more_then_fails() {
        eval_bench(|ws| {
            let b = bench(ws);
            let e = &b.eval_id;
            next(ws, e, 8, None, &[]).unwrap();
            let t = task_id(e, "m1");
            let path = state_path(ws, e);
            let old = now_secs() - TIMEOUT_SECS - 1;
            write(&path, json!({"handed": {&t: [old]}}).to_string().as_bytes()).unwrap();
            assert_eq!(
                ids(&next(ws, e, 8, None, &[]).unwrap()),
                std::slice::from_ref(&t),
                "once more"
            );
            write(&path, json!({"handed": {&t: [old, old]}}).to_string().as_bytes()).unwrap();
            let after = next(ws, e, 8, None, &[]).unwrap();
            assert!(!ids(&after).contains(&t), "failed, not handed out a third time");
            let ctx = Ctx::load(ws, e).unwrap();
            assert!(status(&ctx).unwrap().failed.contains(&t));
        });
    }

    fn refused(e: EvalError, code: &str) {
        assert!(e.to_string().contains(code), "{e}");
    }
}
