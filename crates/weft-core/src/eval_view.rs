//! The Eval view: one screen, requirements first (RingFrame ADR-0020, "The
//! Eval view in Weft"). A header with the verdict and both drift numbers;
//! then what RingFrame found and the changes nothing explains; then each part
//! of what was asked as a row of steps, each step one line with its state and
//! how many changes serve it. The same results read by file are one key away.
//!
//! Built from RingFrame's own files and never re-judged: a closed Eval's
//! states are its record's; while one runs, a step shows the first reading
//! RingFrame accepted for it, marked as such, or that it is still being
//! judged. What a row means is decided here; the client only draws the rows.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EvalView {
    pub eval_id: String,
    /// The record is written: every state below is final.
    pub closed: bool,
    pub verdict: Option<String>,
    pub confidence: Option<f64>,
    pub commission: Option<f64>,
    pub omission: Option<f64>,
    /// While it runs: each stage's tasks in, of those handed out so far.
    pub progress: Vec<Stage>,
    /// What RingFrame found, one line each.
    pub findings: Vec<String>,
    /// The changes nothing explains, by window id.
    pub unexplained: Vec<String>,
    pub groups: Vec<Group>,
    /// Every change, in file order.
    pub changes: Vec<Change>,
    /// The checks RingFrame ran, one line each.
    pub checks: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    pub name: String,
    pub done: usize,
    pub planned: usize,
}

/// One part of what was asked: a plan section, an Ask's own sentences, or the
/// rules that hold across the work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub key: String,
    pub title: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    /// What the step is called within its part: `7` of a phase.
    pub label: String,
    /// Its first sentence, plain.
    pub text: String,
    pub done_when: String,
    pub state: State,
    /// The changes that serve it, or undo it, by window id.
    pub changes: Vec<String>,
    pub readings: Vec<Reading>,
    /// What RingFrame found that names it.
    pub findings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Met,
    NotMet,
    /// Judged, and the readings did not settle it.
    Unsettled,
    /// Nothing read yet.
    Judging,
    /// A reading is in and the Eval has not closed: not yet its result.
    Read(String),
}

impl State {
    /// The mark a row wears.
    pub fn mark(&self) -> &'static str {
        match self {
            State::Met => "✓",
            State::NotMet => "✗",
            State::Unsettled => "?",
            State::Judging | State::Read(_) => "…",
        }
    }

    pub fn words(&self) -> String {
        match self {
            State::Met => "met".into(),
            State::NotMet => "not met".into(),
            State::Unsettled => "unsettled".into(),
            State::Judging => "judging".into(),
            State::Read(v) => format!("{}, so far", v.replace('_', " ")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub role: String,
    pub vote: String,
    pub reason: String,
    pub missing: String,
    /// Whether RingFrame counted it toward the result.
    pub counted: bool,
    /// (window, quote) for each citation.
    pub cites: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub window: String,
    pub path: String,
    /// `required`, `consequence`, `unexplained`, `undoes`, `mechanical`,
    /// `not_judged`, or `judging` while it runs.
    pub result: String,
    /// The steps it serves, or undoes.
    pub serves: Vec<String>,
    /// Why nothing explains it, as a judge said.
    pub why: String,
    /// Every quote a judge cited from it.
    pub quotes: Vec<String>,
}

/// RingFrame's files for one Eval, as they are on disk now.
#[derive(Debug, Clone, Copy)]
pub struct Sources<'a> {
    pub eval_id: &'a str,
    /// `changes.json`: the requirements and what RingFrame found.
    pub changes: &'a Value,
    /// `evidence.json`: every window's id and path.
    pub evidence: &'a Value,
    /// `brief.json`: the Asks' titles.
    pub brief: &'a Value,
    /// `record.json`, once the Eval has closed.
    pub record: Option<&'a Value>,
    /// Every accepted task output so far.
    pub outputs: &'a [Value],
    /// The tasks handed out so far, and those in (accepted or failed).
    pub planned: &'a [String],
    pub done: &'a [String],
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn strs(v: &Value, k: &str) -> Vec<String> {
    list(v, k).iter().filter_map(Value::as_str).map(str::to_string).collect()
}

/// A step's text, plain and short: its first sentence, without Markdown's
/// emphasis.
fn first_sentence(text: &str) -> String {
    let plain = text.replace("**", "");
    let end = plain.find(". ").map_or(plain.len(), |i| i + 1);
    plain[..end].trim().to_string()
}

/// A finding as one line, and the steps it names.
fn finding_line(f: &Value, labels: &BTreeMap<String, String>) -> Option<String> {
    let kind = s(f, "kind");
    let shown = [
        "untouched_path",
        "no_link",
        "unclaimed_rewrite",
        "scaffolding",
        "new_warning",
        "test_integrity",
    ];
    if !shown.contains(&kind.as_str()) {
        return None;
    }
    let who: Vec<String> = strs(f, "requirements")
        .iter()
        .map(|r| labels.get(r).cloned().unwrap_or_else(|| r.clone()))
        .collect();
    let words = kind.replace('_', " ");
    Some(if who.is_empty() {
        format!("{words}: {}", s(f, "detail"))
    } else {
        format!("{words}: {} ({})", s(f, "detail"), who.join(", "))
    })
}

fn readings_of(votes: &[Value], role_of: impl Fn(&Value) -> String) -> Vec<Reading> {
    votes
        .iter()
        .map(|v| Reading {
            role: role_of(v),
            vote: s(v, "vote"),
            reason: s(v, "reason"),
            missing: s(v, "missing"),
            counted: v.get("counted").and_then(Value::as_bool).unwrap_or(false),
            cites: list(v, "citations")
                .iter()
                .filter(|c| c.get("window").is_some())
                .map(|c| (s(c, "window"), s(c, "quote")))
                .collect(),
        })
        .collect()
}

impl EvalView {
    pub fn build(src: Sources<'_>) -> EvalView {
        let record = src.record;
        let requirements: Vec<&Value> = list(src.changes, "requirements")
            .iter()
            .filter(|r| matches!(s(r, "kind").as_str(), "local" | "cross"))
            .collect();
        let titles: BTreeMap<String, String> =
            list(src.brief, "asks").iter().map(|a| (s(a, "ask_id"), s(a, "title"))).collect();

        // Each step's label, and the part it belongs to.
        let mut groups: Vec<Group> = Vec::new();
        let mut labels: BTreeMap<String, String> = BTreeMap::new();
        for r in &requirements {
            let id = s(r, "id");
            let part = s(r, "part");
            let (key, title) = if s(r, "kind") == "cross" {
                ("cross".to_string(), "Rules across the work".to_string())
            } else if !part.is_empty() {
                let (doc, section) = part.split_once('#').unwrap_or(("", part.as_str()));
                (part.clone(), format!("{section} · {}", doc.trim_start_matches("../")))
            } else {
                let ask = s(r, "ask");
                let title = titles.get(&ask).cloned().unwrap_or_else(|| ask.clone());
                (ask, format!("Ask: {title}"))
            };
            let label = id
                .strip_prefix(&format!("{part}/"))
                .map(str::to_string)
                .unwrap_or_else(|| id.rsplit('#').next().unwrap_or(&id).to_string());
            labels.insert(id.clone(), label.clone());
            let step = Step {
                id,
                label,
                text: first_sentence(&s(r, "text")),
                done_when: s(r, "done_when"),
                state: State::Judging,
                changes: Vec::new(),
                readings: Vec::new(),
                findings: Vec::new(),
            };
            match groups.iter_mut().find(|g| g.key == key) {
                Some(g) => g.steps.push(step),
                None => groups.push(Group { key, title, steps: vec![step] }),
            }
        }

        // Every change, with what the map said of it.
        let mut changes: Vec<Change> = list(src.evidence, "windows")
            .iter()
            .map(|w| Change {
                window: s(w, "id"),
                path: s(w, "path"),
                result: if s(w, "class") == "mechanical" {
                    "mechanical".into()
                } else {
                    "judging".into()
                },
                serves: Vec::new(),
                why: String::new(),
                quotes: Vec::new(),
            })
            .collect();
        changes.sort_by(|a, b| a.path.cmp(&b.path).then(a.window.cmp(&b.window)));
        let at: BTreeMap<String, usize> =
            changes.iter().enumerate().map(|(i, c)| (c.window.clone(), i)).collect();
        let mut read = |id: &str, entry: &Value, final_result: Option<String>| {
            let Some(c) = at.get(id).map(|i| &mut changes[*i]) else { return };
            let serves: Vec<String> =
                strs(entry, "serves").into_iter().chain(strs(entry, "undoes")).collect();
            if !serves.is_empty() {
                c.serves = serves;
            }
            if let Some(why) = entry.get("unexplained").and_then(Value::as_str) {
                c.why = why.to_string();
            }
            let q = s(entry, "quote");
            if !q.is_empty() && !c.quotes.contains(&q) {
                c.quotes.push(q);
            }
            c.result = final_result.unwrap_or_else(|| {
                if entry.get("unexplained").is_some_and(|u| !u.is_null()) {
                    "unexplained".into()
                } else if !strs(entry, "undoes").is_empty() {
                    "undoes".into()
                } else if entry.get("follows_from").is_some_and(|f| !f.is_null()) {
                    "consequence".into()
                } else {
                    "required".into()
                }
            });
        };
        if let Some(rec) = record {
            for w in list(rec, "windows") {
                let id = s(w, "id");
                let readings = &w["readings"];
                read(&id, &readings["map"], Some(s(w, "result")));
                if readings.get("confirm").is_some() {
                    read(&id, &readings["confirm"], Some(s(w, "result")));
                }
            }
        } else {
            for o in src.outputs.iter().filter(|o| s(o, "kind") == "map") {
                for entry in list(o, "windows") {
                    read(&s(entry, "window"), entry, None);
                }
            }
        }

        // Each step's state and readings: the record's once it is written,
        // else what is in so far.
        let items: BTreeMap<String, &Value> = record
            .map(|r| list(r, "items").iter().map(|i| (s(i, "id"), i)).collect())
            .unwrap_or_default();
        let mut live: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for o in src.outputs.iter().filter(|o| s(o, "kind") == "reduce") {
            let role = s(&o["judge"], "role");
            for v in list(o, "requirements") {
                let mut v = v.clone();
                v["role"] = Value::String(role.clone());
                live.entry(s(&v, "id")).or_default().push(v);
            }
        }
        let findings_src: &[Value] =
            record.map(|r| list(r, "findings")).unwrap_or_else(|| list(src.changes, "findings"));
        for g in &mut groups {
            for step in &mut g.steps {
                if let Some(item) = items.get(&step.id) {
                    step.state = match s(item, "result").as_str() {
                        "met" => State::Met,
                        "not_met" => State::NotMet,
                        _ => State::Unsettled,
                    };
                    step.readings = readings_of(list(item, "votes"), |v| s(v, "role"));
                } else if record.is_some() {
                    step.state = State::Unsettled;
                } else if let Some(votes) = live.get(&step.id) {
                    step.readings = readings_of(votes, |v| s(v, "role"));
                    step.state = State::Read(s(votes.last().expect("one"), "vote"));
                }
                step.changes = changes
                    .iter()
                    .filter(|c| c.serves.contains(&step.id))
                    .map(|c| c.window.clone())
                    .collect();
                step.findings = findings_src
                    .iter()
                    .filter(|f| strs(f, "requirements").contains(&step.id))
                    .filter_map(|f| finding_line(f, &labels))
                    .collect();
            }
        }
        // A cited quote is shown where it was cited.
        for g in &groups {
            for step in &g.steps {
                for r in &step.readings {
                    for (w, q) in &r.cites {
                        if let Some(c) = at.get(w).map(|i| &mut changes[*i])
                            && !q.is_empty()
                            && !c.quotes.contains(q)
                        {
                            c.quotes.push(q.clone());
                        }
                    }
                }
            }
        }

        let stage = |id: &str| match id.split('~').nth(1).map(|l| l.chars().next()) {
            Some(Some('m')) => "map",
            Some(Some('r')) => "reduce",
            _ => "confirm",
        };
        let progress = if record.is_some() {
            Vec::new()
        } else {
            ["map", "reduce", "confirm"]
                .iter()
                .map(|name| Stage {
                    name: name.to_string(),
                    planned: src.planned.iter().filter(|t| stage(t) == *name).count(),
                    done: src.done.iter().filter(|t| stage(t) == *name).count(),
                })
                .collect()
        };
        let drift = record.map(|r| &r["drift"]);
        EvalView {
            eval_id: src.eval_id.to_string(),
            closed: record.is_some(),
            verdict: record.map(|r| s(r, "verdict")).filter(|v| !v.is_empty()),
            confidence: record.and_then(|r| r["confidence"].as_f64()),
            commission: drift.and_then(|d| d["commission"].as_f64()),
            omission: drift.and_then(|d| d["omission"].as_f64()),
            progress,
            findings: findings_src.iter().filter_map(|f| finding_line(f, &labels)).collect(),
            unexplained: changes
                .iter()
                .filter(|c| c.result == "unexplained")
                .map(|c| c.window.clone())
                .collect(),
            groups,
            changes,
            checks: record
                .map(|r| {
                    list(r, "checks")
                        .iter()
                        .map(|c| {
                            format!(
                                "`{}`: {} in {} s",
                                s(c, "command"),
                                s(c, "outcome"),
                                c["seconds"]
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            limitations: record.map(|r| strs(r, "limitations")).unwrap_or_default(),
        }
    }

    /// The line the view opens with.
    pub fn header(&self) -> String {
        if self.closed {
            let pct = |x: Option<f64>| {
                x.map_or("—".to_string(), |v| format!("{}%", (v * 100.0).round()))
            };
            format!(
                "{} · confidence {} · drift: commission {}, omission {}",
                self.verdict.as_deref().unwrap_or("closed"),
                self.confidence.map_or("—".to_string(), |c| format!("{c:.2}")),
                pct(self.commission),
                pct(self.omission)
            )
        } else {
            let stages: Vec<String> = self
                .progress
                .iter()
                .map(|p| {
                    if p.planned == 0 {
                        format!("{} —", p.name)
                    } else {
                        format!("{} {} of {}", p.name, p.done, p.planned)
                    }
                })
                .collect();
            format!("judging · {}", stages.join(" · "))
        }
    }

    pub fn change(&self, window: &str) -> Option<&Change> {
        self.changes.iter().find(|c| c.window == window)
    }

    pub fn step(&self, id: &str) -> Option<&Step> {
        self.groups.iter().flat_map(|g| &g.steps).find(|s| s.id == id)
    }
}

// ---- rows ---------------------------------------------------------------------

/// What the list shows: the steps of each part, or the changes of each file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Steps,
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// A section's heading: FINDINGS, UNEXPLAINED CHANGES, a reading.
    Heading,
    /// A line to read, not to pick.
    Text,
    /// A part or a file: folds.
    Group,
    Step,
    Change,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    /// What picking it opens or folds: `g:<part>`, `s:<step>`, `f:<path>`,
    /// `c:<window>`; empty for a row that is only read.
    pub key: String,
    pub depth: u8,
    pub mark: String,
    pub text: String,
    /// Said on the right: a count, a result.
    pub aside: String,
    /// The state a mark stands for, so the client can colour it.
    pub state: Option<State>,
}

impl Row {
    pub fn pickable(&self) -> bool {
        !self.key.is_empty()
    }

    fn text(kind: RowKind, depth: u8, text: impl Into<String>) -> Row {
        Row {
            kind,
            key: String::new(),
            depth,
            mark: String::new(),
            text: text.into(),
            aside: String::new(),
            state: None,
        }
    }
}

fn change_row(c: &Change, depth: u8, with_path: bool) -> Row {
    let result = c.result.replace('_', " ");
    Row {
        kind: RowKind::Change,
        key: format!("c:{}", c.window),
        depth,
        mark: "·".into(),
        text: if with_path { format!("{}  {}", c.path, c.window) } else { c.window.clone() },
        aside: if c.why.is_empty() { result } else { format!("{result}: {}", c.why) },
        state: None,
    }
}

/// The view's main list, with the folded parts or files closed.
pub fn rows(view: &EvalView, mode: Mode, folded: &BTreeSet<String>) -> Vec<Row> {
    let mut out = Vec::new();
    if mode == Mode::Steps {
        if !view.findings.is_empty() {
            out.push(Row::text(RowKind::Heading, 0, "FINDINGS"));
            out.extend(view.findings.iter().map(|f| Row::text(RowKind::Text, 1, f.clone())));
        }
        if !view.unexplained.is_empty() {
            out.push(Row::text(RowKind::Heading, 0, "UNEXPLAINED CHANGES"));
            out.extend(
                view.unexplained
                    .iter()
                    .filter_map(|w| view.change(w))
                    .map(|c| change_row(c, 1, true)),
            );
        }
        for g in &view.groups {
            let key = format!("g:{}", g.key);
            let open = !folded.contains(&key);
            let count = |f: fn(&State) -> bool| g.steps.iter().filter(|s| f(&s.state)).count();
            let tally: Vec<String> = [
                (count(|s| *s == State::Met), "met"),
                (count(|s| *s == State::NotMet), "not met"),
                (count(|s| *s == State::Unsettled), "unsettled"),
                (count(|s| matches!(s, State::Judging | State::Read(_))), "judging"),
            ]
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, w)| format!("{n} {w}"))
            .collect();
            out.push(Row {
                kind: RowKind::Group,
                key,
                depth: 0,
                mark: if open { "▾" } else { "▸" }.into(),
                text: g.title.clone(),
                aside: tally.join(" · "),
                state: None,
            });
            if open {
                for st in &g.steps {
                    let n = st.changes.len();
                    out.push(Row {
                        kind: RowKind::Step,
                        key: format!("s:{}", st.id),
                        depth: 1,
                        mark: st.state.mark().into(),
                        text: format!("{}  {}", st.label, st.text),
                        aside: format!(
                            "{} · {n} change{}",
                            st.state.words(),
                            if n == 1 { "" } else { "s" }
                        ),
                        state: Some(st.state.clone()),
                    });
                }
            }
        }
    } else {
        let mut files: Vec<(&str, Vec<&Change>)> = Vec::new();
        for c in &view.changes {
            match files.last_mut() {
                Some((p, cs)) if *p == c.path => cs.push(c),
                _ => files.push((c.path.as_str(), vec![c])),
            }
        }
        for (path, cs) in files {
            let key = format!("f:{path}");
            let open = !folded.contains(&key);
            let odd = cs.iter().filter(|c| c.result == "unexplained").count();
            out.push(Row {
                kind: RowKind::Group,
                key,
                depth: 0,
                mark: if open { "▾" } else { "▸" }.into(),
                text: path.to_string(),
                aside: format!(
                    "{} change{}{}",
                    cs.len(),
                    if cs.len() == 1 { "" } else { "s" },
                    if odd > 0 { format!(" · {odd} unexplained") } else { String::new() }
                ),
                state: None,
            });
            if open {
                out.extend(cs.into_iter().map(|c| change_row(c, 1, false)));
            }
        }
    }
    if mode == Mode::Steps {
        if !view.checks.is_empty() {
            out.push(Row::text(RowKind::Heading, 0, "CHECKS RINGFRAME RAN"));
            out.extend(view.checks.iter().map(|c| Row::text(RowKind::Text, 1, c.clone())));
        }
        if !view.limitations.is_empty() {
            out.push(Row::text(RowKind::Heading, 0, "LIMITATIONS"));
            out.extend(view.limitations.iter().map(|l| Row::text(RowKind::Text, 1, l.clone())));
        }
    }
    out
}

/// One step, opened: what it asks, how each judge read it, and its changes.
pub fn step_rows(view: &EvalView, id: &str) -> Vec<Row> {
    let Some(st) = view.step(id) else { return Vec::new() };
    let mut out = vec![Row {
        kind: RowKind::Heading,
        key: String::new(),
        depth: 0,
        mark: st.state.mark().into(),
        text: format!("{}  {}", st.label, st.text),
        aside: st.state.words(),
        state: Some(st.state.clone()),
    }];
    if !st.done_when.is_empty() {
        out.push(Row::text(RowKind::Text, 1, format!("Done when: {}", st.done_when)));
    }
    if !st.findings.is_empty() {
        out.push(Row::text(RowKind::Heading, 0, "FOUND"));
        out.extend(st.findings.iter().map(|f| Row::text(RowKind::Text, 1, f.clone())));
    }
    if !st.readings.is_empty() {
        out.push(Row::text(RowKind::Heading, 0, "READINGS"));
        for r in &st.readings {
            let said = if r.reason.is_empty() { &r.missing } else { &r.reason };
            let mut row = Row::text(
                RowKind::Text,
                1,
                format!("{}: {}  {said}", r.role, r.vote.replace('_', " ")),
            );
            if !r.counted && view.closed {
                row.aside = "not counted".into();
            }
            out.push(row);
            if !r.missing.is_empty() && !r.reason.is_empty() {
                out.push(Row::text(RowKind::Text, 2, format!("missing: {}", r.missing)));
            }
        }
    }
    out.push(Row::text(RowKind::Heading, 0, format!("CHANGES ({})", st.changes.len())));
    out.extend(st.changes.iter().filter_map(|w| view.change(w)).map(|c| change_row(c, 1, true)));
    out
}

/// A change's hunk, each line with whether a judge's quote is on it.
pub fn hunk_lines(text: &str, quotes: &[String]) -> Vec<(String, bool)> {
    let squash = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
    let quotes: Vec<String> = quotes.iter().map(|q| squash(q)).filter(|q| !q.is_empty()).collect();
    text.lines()
        .map(|l| {
            let body = squash(l.get(1..).unwrap_or_default());
            let lit = !body.is_empty()
                && quotes.iter().any(|q| q.contains(&body) || body.contains(q.as_str()));
            (l.to_string(), lit)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const S7: &str = "../plans/app/plan.md#Phase 1/7";
    const S8: &str = "../plans/app/plan.md#Phase 1/8";

    fn changes() -> Value {
        json!({"requirements": [
            {"id": S7, "kind": "local", "part": "../plans/app/plan.md#Phase 1", "ask": "ask_1",
             "text": "**A failed send is told.** The person sees why.", "done_when": "A test with a refusing CLI."},
            {"id": S8, "kind": "local", "part": "../plans/app/plan.md#Phase 1", "ask": "ask_1",
             "text": "Keep the log short.", "done_when": ""},
            {"id": "ask_1#s0", "kind": "scope", "ask": "ask_1", "text": "Work in this repo."}
        ], "findings": [
            {"kind": "unclaimed_rewrite", "requirements": [S7], "detail": "commit 94d7989 rewrites lines of 758156f"},
            {"kind": "unlinked", "requirements": [], "detail": "noise"}
        ]})
    }

    fn evidence() -> Value {
        json!({"windows": [
            {"id": "w_b", "path": "src/server.rs", "class": "code"},
            {"id": "w_a", "path": "src/log.rs", "class": "code"},
            {"id": "w_t", "path": "src/telemetry.rs", "class": "code"}
        ]})
    }

    fn brief() -> Value {
        json!({"asks": [{"ask_id": "ask_1", "title": "Phase 1"}]})
    }

    fn built(
        record: Option<&Value>,
        outputs: &[Value],
        planned: &[&str],
        done: &[&str],
    ) -> EvalView {
        let (c, e, b) = (changes(), evidence(), brief());
        let planned: Vec<String> = planned.iter().map(|s| s.to_string()).collect();
        let done: Vec<String> = done.iter().map(|s| s.to_string()).collect();
        EvalView::build(Sources {
            eval_id: "evl_1",
            changes: &c,
            evidence: &e,
            brief: &b,
            record,
            outputs,
            planned: &planned,
            done: &done,
        })
    }

    fn map_output() -> Value {
        json!({"kind": "map", "judge": {"role": "map"}, "windows": [
            {"window": "w_b", "serves": [S7]},
            {"window": "w_a", "serves": [S8]},
            {"window": "w_t", "unexplained": "posts the path outside", "quote": "collect.example.com"}
        ]})
    }

    #[test]
    fn while_it_runs_a_step_shows_its_first_reading_as_so_far_and_the_stages_count_up() {
        let reduce = json!({"kind": "reduce", "judge": {"role": "reduce"}, "requirements": [
            {"id": S7, "vote": "not_met", "reason": "the refusal is dropped", "citations": []}
        ]});
        let v = built(
            None,
            &[map_output(), reduce],
            &["evl_1~m1", "evl_1~r1", "evl_1~r2"],
            &["evl_1~m1", "evl_1~r1"],
        );
        assert!(!v.closed);
        assert_eq!(v.header(), "judging · map 1 of 1 · reduce 1 of 2 · confirm —");
        let s7 = v.step(S7).unwrap();
        assert_eq!(s7.state, State::Read("not_met".into()));
        assert_eq!(s7.state.words(), "not met, so far");
        assert_eq!((s7.label.as_str(), s7.text.as_str()), ("7", "A failed send is told."));
        assert_eq!(s7.changes, ["w_b"]);
        assert_eq!(v.step(S8).unwrap().state, State::Judging);
        assert_eq!(v.unexplained, ["w_t"]);
        assert_eq!(v.findings, ["unclaimed rewrite: commit 94d7989 rewrites lines of 758156f (7)"]);
        assert!(v.step("ask_1#s0").is_none(), "only what a judge reads is a step");
    }

    #[test]
    fn a_closed_eval_is_its_record_and_reads_by_step_or_by_file() {
        let record = json!({"verdict": "drifted", "confidence": 0.75,
            "drift": {"commission": 0.01, "omission": 0.5},
            "items": [
                {"id": S7, "result": "not_met", "votes": [
                    {"role": "reduce", "vote": "met", "counted": true, "reason": "told", "citations": [{"window": "w_b", "quote": "let mut unrecorded = None;"}]},
                    {"role": "confirm", "vote": "not_met", "counted": true, "reason": "reverted", "missing": "the refusal is not kept", "citations": []}]},
                {"id": S8, "result": "met", "votes": []}],
            "windows": [
                {"id": "w_b", "path": "src/server.rs", "result": "required", "readings": {"map": {"serves": [S7]}}},
                {"id": "w_a", "path": "src/log.rs", "result": "required", "readings": {"map": {"serves": [S8]}}},
                {"id": "w_t", "path": "src/telemetry.rs", "result": "unexplained",
                 "readings": {"map": {"unexplained": "posts outside", "quote": "collect.example.com"}}}],
            "findings": changes()["findings"].clone(),
            "checks": [{"command": "cargo test", "outcome": "succeeded", "seconds": 12}],
            "limitations": ["m1: kept without 1 of its entries"]});
        let v = built(Some(&record), &[], &[], &[]);
        assert_eq!(v.header(), "drifted · confidence 0.75 · drift: commission 1%, omission 50%");
        assert_eq!(v.step(S7).unwrap().state, State::NotMet);
        let main = rows(&v, Mode::Steps, &BTreeSet::new());
        let texts: Vec<String> = main
            .iter()
            .map(|r| format!("{}{} {}", "  ".repeat(r.depth as usize), r.mark, r.text))
            .collect();
        assert_eq!(
            texts,
            [
                " FINDINGS",
                "   unclaimed rewrite: commit 94d7989 rewrites lines of 758156f (7)",
                " UNEXPLAINED CHANGES",
                "  · src/telemetry.rs  w_t",
                "▾ Phase 1 · plans/app/plan.md",
                "  ✗ 7  A failed send is told.",
                "  ✓ 8  Keep the log short.",
                " CHECKS RINGFRAME RAN",
                "   `cargo test`: succeeded in 12 s",
                " LIMITATIONS",
                "   m1: kept without 1 of its entries",
            ]
        );
        assert_eq!(main[4].aside, "1 met · 1 not met");
        let folded: BTreeSet<String> = [main[4].key.clone()].into();
        assert!(!rows(&v, Mode::Steps, &folded).iter().any(|r| r.kind == RowKind::Step), "folded");
        let by_file: Vec<String> =
            rows(&v, Mode::Files, &BTreeSet::new()).iter().map(|r| r.text.clone()).collect();
        assert_eq!(
            by_file,
            ["src/log.rs", "w_a", "src/server.rs", "w_b", "src/telemetry.rs", "w_t"]
        );
        let step: Vec<String> = step_rows(&v, S7).iter().map(|r| r.text.clone()).collect();
        assert!(step.contains(&"confirm: not met  reverted".to_string()), "{step:?}");
        assert!(step.contains(&"missing: the refusal is not kept".to_string()), "{step:?}");
        assert_eq!(step.last().unwrap(), "src/server.rs  w_b");
        assert_eq!(
            v.change("w_b").unwrap().quotes,
            ["let mut unrecorded = None;"],
            "a cited quote is kept on its change"
        );
    }

    #[test]
    fn a_hunk_lights_the_lines_a_judge_quoted() {
        let text = "--- src/server.rs\n@@ -1 +1,2 @@\n+let mut unrecorded = None;\n+let x = 1;\n";
        let lit: Vec<bool> =
            hunk_lines(text, &["let mut unrecorded =  None;".into()]).iter().map(|l| l.1).collect();
        assert_eq!(lit, [false, false, true, false]);
    }
}
