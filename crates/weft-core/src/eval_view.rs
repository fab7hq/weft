//! The Eval view: what an Eval leaves for the person, one item per line.
//!
//! "To look at" comes first: a question carried from an earlier Seal the
//! person was not sure at, then each change nothing asked for and each
//! requirement the change does not meet, then what the readings could not
//! settle. "Settled" follows: each requirement the change meets, and the
//! changes the Asks explain, gathered in one line. Each item opens to its own
//! detail, in plain words: what it is, why it is here, how sure the readings
//! are as one dot per reading, what the agent said of it, and the lines that
//! show it.
//!
//! Built from RingFrame's own files and never re-judged: a closed Eval's items
//! are its record's; while one runs, the readings in so far settle each item
//! the way RingFrame will, and an item being read again says so. What an item
//! means is decided here; the clients only draw it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::words::{fill, word};

/// What an item is, by the shape and colour it is drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A question the person was not sure at, at an earlier Seal: ↺ teal.
    Before,
    /// A change nothing asked for, a requirement not met, or a change against
    /// one of the person's decisions: ■ amber.
    #[default]
    Look,
    /// The readings could not settle it: ◆ blue.
    Unsettled,
    /// ● grey.
    Settled,
}

impl Kind {
    pub fn mark(self) -> &'static str {
        match self {
            Kind::Before => "↺",
            Kind::Look => "■",
            Kind::Unsettled => "◆",
            Kind::Settled => "●",
        }
    }

    /// What it is, in words: the start of its detail's first line.
    pub fn words(self) -> &'static str {
        word(match self {
            Kind::Before => "eval_view.kind.before",
            Kind::Look => "eval_view.kind.look",
            Kind::Unsettled => "eval_view.kind.unsettled",
            Kind::Settled => "eval_view.kind.settled",
        })
    }

    /// Listed under "To look at".
    pub fn to_look_at(self) -> bool {
        self != Kind::Settled
    }
}

/// What one reading said of an item, in plain words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Read {
    /// `First read`, `Second read`, `Third read`.
    pub who: String,
    pub said: String,
}

/// One line of the list, and its detail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Item {
    /// `w|<unit>`, `r|<requirement>`, `c|<decision>`, `check|<command>`, or
    /// `settled|<what>` for the changes gathered in one line.
    pub key: String,
    pub kind: Kind,
    /// The line in the list.
    pub text: String,
    /// How many things it stands for: 1, or the changes it gathers.
    #[serde(default)]
    pub count: usize,
    /// One per reading, filled when that reading agrees with the result.
    #[serde(default)]
    pub dots: Vec<bool>,
    /// It is being read again: no dots yet.
    #[serde(default)]
    pub again: bool,
    /// The detail's first line, after the mark.
    #[serde(default)]
    pub head: String,
    /// The detail's sentence.
    #[serde(default)]
    pub big: String,
    /// Why it is here, a sentence each.
    #[serde(default)]
    pub why: Vec<String>,
    /// How sure, beside the dots: `read twice, both agree`.
    #[serde(default)]
    pub sure: String,
    /// What the agent said of it, in its own words.
    #[serde(default)]
    pub said: Option<String>,
    /// The check that would settle it.
    #[serde(default)]
    pub check: Option<String>,
    /// The changes whose lines show it, by window id.
    #[serde(default)]
    pub windows: Vec<String>,
    /// What each reading said of it.
    #[serde(default)]
    pub reads: Vec<Read>,
}

/// A change an item points at: its file, and the words the readings quoted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub id: String,
    pub path: String,
    pub quotes: Vec<String>,
}

/// A fault, for a follow-up's words: a requirement not met, or a change
/// nothing asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fault {
    pub requirement: bool,
    /// A requirement's text, or a change's file.
    pub what: String,
    /// Why, as the reading that decided it said.
    pub why: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Light {
    Done,
    /// Under way: drawn breathing.
    Now,
    Next,
}

/// One step of an Eval, in quiet text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lit {
    pub name: String,
    pub light: Light,
    /// Its words, as a key of `words.json`.
    #[serde(default)]
    pub key: String,
}

/// A running Eval's progress: one ring with an arc per reading, and its steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// The readings, up to three: each in, under way, or still to come.
    pub arcs: Vec<Light>,
    /// Inside the ring: `2nd read`, and under it `reading`.
    pub label: String,
    pub note: String,
    pub steps: Vec<Lit>,
    /// The checks still running, a line each.
    pub tests: Vec<String>,
    /// How many of the items to read have a reading in, of how many.
    #[serde(default)]
    pub reads: (usize, usize),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EvalView {
    pub eval_id: String,
    /// The work it is of, by its first Ask's title.
    #[serde(default)]
    pub title: String,
    /// The record is written: every item below is final.
    pub closed: bool,
    /// "To look at" first, then "Settled".
    #[serde(default)]
    pub items: Vec<Item>,
    /// The changes the items point at.
    #[serde(default)]
    pub windows: Vec<Window>,
    #[serde(default)]
    pub faults: Vec<Fault>,
    /// How many questions the Seal asks.
    #[serde(default)]
    pub questions: usize,
    /// The harnesses that read it.
    #[serde(default)]
    pub readers: Vec<String>,
    /// While it runs.
    #[serde(default)]
    pub progress: Option<Progress>,
    /// While it runs, before the first reading is in: the rest of the list
    /// comes with it.
    #[serde(default)]
    pub waiting: bool,
    /// The checks RingFrame ran, one line each.
    #[serde(default)]
    pub checks: Vec<String>,
    /// The checks still running.
    #[serde(default)]
    pub checks_running: Vec<String>,
    /// Every check RingFrame ran succeeded, and none ran out of time.
    #[serde(default)]
    pub checks_passed: bool,
    #[serde(default)]
    pub limitations: Vec<String>,
}

/// RingFrame's files for one Eval, as they are on disk now.
#[derive(Debug, Clone, Copy)]
pub struct Sources<'a> {
    pub eval_id: &'a str,
    /// `changes.json`: the requirements.
    pub changes: &'a Value,
    /// `evidence.json`: every unit's id, path and class.
    pub evidence: &'a Value,
    /// `brief.json`: the Asks' titles, and the questions carried.
    pub brief: &'a Value,
    /// `record.json`, once the Eval has closed.
    pub record: Option<&'a Value>,
    /// Every accepted task output so far.
    pub outputs: &'a [Value],
    /// The tasks handed out so far, and those in (accepted or failed).
    pub planned: &'a [String],
    pub done: &'a [String],
    /// The checks RingFrame is still running for it, within their time.
    pub checks_running: &'a [String],
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

/// A text, plain and short: its first sentence, without Markdown's emphasis.
fn first_sentence(text: &str) -> String {
    let plain = text.replace("**", "");
    let end = plain.find(". ").map_or(plain.len(), |i| i + 1);
    plain[..end].trim().to_string()
}

/// A date as the person reads it: `2026-11-03` is `3 Nov`.
pub fn day(date: &str) -> String {
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut parts = date.get(..10).unwrap_or(date).split('-');
    let (_, m, d) = (parts.next(), parts.next(), parts.next());
    match (m.and_then(|m| m.parse::<usize>().ok()), d.and_then(|d| d.parse::<u32>().ok())) {
        (Some(m @ 1..=12), Some(d)) => format!("{d} {}", MONTHS[m - 1]),
        _ => date.to_string(),
    }
}

/// How a record begins the limitation it writes for checks it stopped
/// waiting for.
const UNFINISHED_CHECKS: &str = "the checks did not finish in their time";

/// Every check in the record succeeded, and none ran out of time before it
/// closed.
fn checks_passed(record: &Value) -> bool {
    list(record, "checks").iter().all(|c| s(c, "outcome") == "succeeded")
        && !strs(record, "limitations").iter().any(|l| l.starts_with(UNFINISHED_CHECKS))
}

/// The classes RingFrame never asks a reader about.
const MECHANICAL: [&str; 5] = ["lock", "generated", "binary", "whitespace", "moved"];

/// An Ask's own sentence, `<ask>/s<n>`.
fn is_sentence(id: &str) -> bool {
    id.rsplit_once('/').is_some_and(|(ask, n)| {
        ask.starts_with("ask_")
            && n.strip_prefix('s')
                .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// What the Eval reads against, as RingFrame does: `changes.json`'s
/// requirements, with the Asks' sentences replaced by the list a reader made
/// of them once that list is accepted. RingFrame names each by its Ask and the
/// words it quotes (`<ask>/f<digest>`, the text too when two quote the same
/// words).
fn requirements(changes: &Value, outputs: &[Value]) -> Vec<Value> {
    use sha2::{Digest, Sha256};
    let mut all = list(changes, "requirements").to_vec();
    let Some(listed) = outputs.iter().find(|o| s(o, "kind") == "requirements") else {
        return all;
    };
    all.retain(|r| !is_sentence(&s(r, "id")));
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for r in list(listed, "requirements") {
        let (ask, quote) = (s(r, "ask"), s(r, "quote"));
        let digest = |words: &str| {
            let squashed = words.split_whitespace().collect::<Vec<_>>().join(" ");
            let d = Sha256::digest(format!("{ask}\0{squashed}").as_bytes());
            let hex: String = d.iter().take(6).map(|b| format!("{b:02x}")).collect();
            format!("{ask}/f{hex}")
        };
        let mut id = digest(&quote);
        if ids.contains(&id) {
            id = digest(&format!("{quote}\0{}", s(r, "text")));
        }
        ids.insert(id.clone());
        all.push(serde_json::json!({"id": id, "kind": r["kind"], "text": r["text"]}));
    }
    all
}

// ---- settling, as RingFrame does --------------------------------------------------

/// What one reading said of an item, as RingFrame classes it: a unit
/// `explained`, `unexplained` or `elsewhere`; a requirement `met`, `not_met`
/// or `unknown`.
fn class(unit: bool, answer: &Value) -> &'static str {
    if unit {
        match answer["answer"].as_str() {
            Some("unexplained") => "unexplained",
            Some("elsewhere") => "elsewhere",
            _ => "explained",
        }
    } else {
        match answer["vote"].as_str() {
            Some("met") => "met",
            Some("not_met") => "not_met",
            _ => "unknown",
        }
    }
}

/// An item's result at face value: what most of its readings said. No
/// majority, or a majority of `unknown`, is `unsettled`.
fn result_of(said: &[Option<&'static str>]) -> &'static str {
    let mut count: BTreeMap<&'static str, usize> = BTreeMap::new();
    for s in said.iter().flatten() {
        *count.entry(s).or_default() += 1;
    }
    let answered: usize = count.values().sum();
    match count.into_iter().find(|(_, n)| 2 * n > answered) {
        Some((s, _)) if s != "unknown" => s,
        _ => "unsettled",
    }
}

/// Whether RingFrame reads an item again: never read, or read once with no
/// answer, as `unexplained` or as `unknown`; or read twice and split.
fn open_again(said: &[Option<&'static str>]) -> bool {
    match said {
        [] => true,
        [one] => matches!(one, None | Some("unexplained" | "unknown")),
        [a, b] => a.is_none() || a != b,
        _ => false,
    }
}

/// A unit or a requirement, with its result and each reading's answer.
struct Read1 {
    key: String,
    unit: bool,
    /// A unit's file, or a requirement's text.
    what: String,
    result: String,
    answers: Vec<Option<Value>>,
}

impl Read1 {
    fn said(&self) -> Vec<Option<&'static str>> {
        self.answers.iter().map(|a| a.as_ref().map(|a| class(self.unit, a))).collect()
    }

    fn id(&self) -> &str {
        self.key.split_once('|').map_or(&self.key, |(_, id)| id)
    }

    /// The last answer that says `result`, else the last answer.
    fn deciding(&self, result: &str) -> Option<&Value> {
        let all = || self.answers.iter().rev().flatten();
        all().find(|a| class(self.unit, a) == result).or_else(|| all().next())
    }

    fn dots(&self) -> Vec<bool> {
        self.said().iter().map(|s| *s == Some(class_word(&self.result))).collect()
    }

    /// How sure, beside the dots.
    fn sure(&self) -> String {
        let said = self.said();
        let n = said.len();
        if n == 0 {
            return String::new();
        }
        let times = word(match n {
            1 => "eval_view.times.1",
            2 => "eval_view.times.2",
            _ => "eval_view.times.3",
        });
        if self.result == "unsettled" {
            let told: BTreeSet<&str> = said.iter().flatten().copied().collect();
            let key = if told.len() > 1 { "eval_view.sure.differ" } else { "eval_view.sure.cant" };
            return fill(key, &[("times", times)]);
        }
        let agreed = self.dots().iter().filter(|d| **d).count();
        match (n, agreed) {
            (1, _) => word("eval_view.sure.once").into(),
            (n, a) if a == n && n == 2 => word("eval_view.sure.both").into(),
            (n, a) if a == n => fill("eval_view.sure.all", &[("times", times)]),
            (_, a) => fill(
                "eval_view.sure.some",
                &[
                    ("times", times),
                    ("n", word(if a == 1 { "eval_view.n.1" } else { "eval_view.n.2" })),
                ],
            ),
        }
    }

    /// What each reading said, in plain words.
    fn reads(&self) -> Vec<Read> {
        let who = ["eval_view.read.1", "eval_view.read.2", "eval_view.read.3"];
        self.answers
            .iter()
            .zip(who)
            .map(|(a, who)| Read { who: word(who).into(), said: self.read_words(a.as_ref()) })
            .collect()
    }

    fn read_words(&self, answer: Option<&Value>) -> String {
        let Some(a) = answer else { return word("eval_view.read.none").into() };
        let reason = s(a, "reason");
        let or = |key: &str| if reason.is_empty() { word(key).to_string() } else { reason.clone() };
        if self.unit {
            return match s(a, "answer").as_str() {
                "serves" => or("eval_view.read.serves"),
                "follows_from" => or("eval_view.read.follows"),
                "elsewhere" => or("eval_view.read.elsewhere"),
                _ => reason,
            };
        }
        let missing = s(a, "missing");
        match class(false, a) {
            "unknown" => {
                let why = if reason.is_empty() { missing } else { reason };
                fill("eval_view.read.cant", &[("why", &why)])
            }
            _ if reason.is_empty() => missing,
            _ => reason,
        }
    }

    /// The changes whose lines show it, with the words quoted from each.
    fn quotes(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for a in self.answers.iter().flatten() {
            if self.unit {
                out.push((self.id().to_string(), s(a, "quote")));
            }
            for c in list(a, "citations") {
                out.push((s(c, "window"), s(c, "quote")));
            }
        }
        if self.unit && out.is_empty() {
            out.push((self.id().to_string(), String::new()));
        }
        out
    }
}

/// A result's class as a reading says it: a unit `explained` is `explained`.
fn class_word(result: &str) -> &'static str {
    match result {
        "explained" => "explained",
        "unexplained" => "unexplained",
        "elsewhere" => "elsewhere",
        "met" => "met",
        "not_met" => "not_met",
        _ => "",
    }
}

/// A question carried from an earlier Seal.
struct Carried {
    key: String,
    behaviour: String,
    date: String,
    windows: Vec<String>,
}

/// What the report adds to the readings.
#[derive(Default)]
struct Report<'a> {
    declared: Vec<(Vec<String>, String)>,
    against: Vec<&'a Value>,
    checks: BTreeMap<String, String>,
    questions: &'a [Value],
    carried: Vec<Carried>,
    again: BTreeSet<String>,
    failed_checks: Vec<String>,
}

impl Report<'_> {
    /// The question asked of this item alone.
    fn sole(&self, key: &str) -> Option<&Value> {
        self.questions.iter().find(|q| strs(q, "items") == [key])
    }

    fn of(&self, key: &str) -> Option<&Value> {
        self.questions.iter().find(|q| strs(q, "items").iter().any(|i| i == key))
    }

    fn said(&self, id: &str) -> Option<String> {
        self.declared
            .iter()
            .find(|(items, _)| items.iter().any(|i| i == id))
            .map(|(_, q)| q.clone())
    }
}

/// Every item, "To look at" first, and the changes they point at.
fn items_of(reads: &[Read1], rep: &Report<'_>) -> (Vec<Item>, Vec<(String, String)>) {
    let mut items = Vec::new();
    let mut quoted: Vec<(String, String)> = Vec::new();
    for c in &rep.carried {
        let date = day(&c.date);
        items.push(Item {
            key: c.key.clone(),
            kind: Kind::Before,
            text: fill("eval_view.before_line", &[("what", &c.behaviour)]),
            count: 1,
            head: fill("eval_view.head.before", &[("date", &date)]),
            big: c.behaviour.clone(),
            why: vec![fill("eval_view.before_why", &[("date", &date)])],
            windows: c.windows.clone(),
            ..Item::default()
        });
        quoted.extend(c.windows.iter().map(|w| (w.clone(), String::new())));
    }
    let against =
        |id: &str| rep.against.iter().find(|a| strs(a, "windows").iter().any(|w| w == id)).copied();
    let mut look: Vec<Item> = Vec::new();
    let mut unsettled: Vec<Item> = Vec::new();
    let mut settled: Vec<Item> = Vec::new();
    let (mut explained, mut elsewhere) = (0usize, 0usize);
    // Units before requirements, as the readings take them.
    let mut ordered: Vec<&Read1> = reads.iter().filter(|r| r.unit).collect();
    ordered.extend(reads.iter().filter(|r| !r.unit));
    for r in ordered {
        let against = if r.unit { against(r.id()) } else { None };
        let kind = match (r.result.as_str(), against) {
            (_, Some(_)) | ("unexplained" | "not_met", _) => Kind::Look,
            ("unsettled", _) => Kind::Unsettled,
            ("explained" | "mechanical", _) => {
                explained += 1;
                continue;
            }
            ("elsewhere", _) => {
                elsewhere += 1;
                continue;
            }
            _ => Kind::Settled,
        };
        let sole = rep.sole(&r.key);
        let any = rep.of(&r.key);
        let deciding = r.deciding(class_word(&r.result));
        let reason = deciding.map(|a| s(a, "reason")).unwrap_or_default();
        let own = if r.unit {
            if reason.is_empty() { r.what.clone() } else { reason.clone() }
        } else {
            first_sentence(&r.what)
        };
        let behaviour = sole.map(|q| s(q, "behaviour")).filter(|b| !b.is_empty());
        let mut why: Vec<String> = Vec::new();
        let mut head = kind.words().to_string();
        if let Some(a) = against {
            let date = day(&s(a, "date"));
            head = fill("eval_view.head.against", &[("date", &date)]);
            why.push(fill(
                "eval_view.against_why",
                &[("date", &date), ("what", &s(a, "behaviour"))],
            ));
        }
        let q_why = any.map(|q| s(q, "why")).filter(|w| !w.is_empty());
        match (q_why, kind) {
            (Some(w), _) => why.push(w),
            (None, Kind::Look | Kind::Settled) if !r.unit && !reason.is_empty() => {
                why.push(reason.clone())
            }
            _ => {}
        }
        let missing = deciding.map(|a| s(a, "missing")).unwrap_or_default();
        if kind == Kind::Look && !r.unit && !missing.is_empty() {
            why.push(fill("eval_view.missing", &[("what", &missing)]));
        }
        let text = match (kind, sole.map(|q| s(q, "quote")).filter(|q| !q.is_empty())) {
            (Kind::Unsettled, Some(quote)) => fill("eval_view.differ_line", &[("quote", &quote)]),
            _ => behaviour.clone().unwrap_or_else(|| own.clone()),
        };
        let big = match kind {
            Kind::Unsettled if !r.unit => own.clone(),
            _ => behaviour.unwrap_or(own),
        };
        let mut windows: Vec<String> = Vec::new();
        for (w, q) in r.quotes() {
            if !windows.contains(&w) {
                windows.push(w.clone());
            }
            quoted.push((w, q));
        }
        // A change brought back against a decision that the readings did not
        // flag shows no dots: no reading agrees with it being here.
        let shows_reads = against.is_none() || r.result == "unexplained";
        let item = Item {
            key: r.key.clone(),
            kind,
            text,
            count: 1,
            dots: if shows_reads { r.dots() } else { Vec::new() },
            again: rep.again.contains(&r.key),
            head,
            big,
            why,
            sure: if shows_reads { r.sure() } else { String::new() },
            said: rep.said(r.id()),
            check: rep
                .checks
                .get(&r.key)
                .filter(|c| !c.is_empty())
                .map(|c| fill("eval_view.settle_by", &[("check", c)])),
            windows,
            reads: r.reads(),
        };
        match kind {
            Kind::Look => look.push(item),
            Kind::Unsettled => unsettled.push(item),
            _ => settled.push(item),
        }
    }
    for c in &rep.failed_checks {
        let text = fill("eval_view.check_failed", &[("check", c)]);
        look.push(Item {
            key: format!("check|{c}"),
            kind: Kind::Look,
            text: text.clone(),
            count: 1,
            head: Kind::Look.words().into(),
            big: text,
            ..Item::default()
        });
    }
    let others =
        !items.is_empty() || !look.is_empty() || !unsettled.is_empty() || !settled.is_empty();
    let gathered = |n: usize, key: &str, one: &str, many: &str, why: &str| Item {
        key: format!("settled|{key}"),
        kind: Kind::Settled,
        text: if n == 1 { word(one).into() } else { fill(many, &[("n", &n.to_string())]) },
        count: n,
        head: Kind::Settled.words().into(),
        big: if n == 1 { word(one).into() } else { fill(many, &[("n", &n.to_string())]) },
        why: vec![word(why).into()],
        ..Item::default()
    };
    if explained > 0 {
        let (one, many) = if others {
            ("eval_view.explained_one", "eval_view.explained_n")
        } else {
            ("eval_view.explained_only_one", "eval_view.explained_only_n")
        };
        settled.push(gathered(explained, "explained", one, many, "eval_view.explained_why"));
    }
    if elsewhere > 0 {
        settled.push(gathered(
            elsewhere,
            "elsewhere",
            "eval_view.elsewhere_one",
            "eval_view.elsewhere_n",
            "eval_view.elsewhere_why",
        ));
    }
    items.extend(look);
    items.extend(unsettled);
    items.extend(settled);
    (items, quoted)
}

/// The changes quoted, in the order first pointed at.
fn windows_of(quoted: Vec<(String, String)>, path: impl Fn(&str) -> String) -> Vec<Window> {
    let mut out: Vec<Window> = Vec::new();
    for (id, q) in quoted {
        if id.is_empty() {
            continue;
        }
        let at = match out.iter().position(|w| w.id == id) {
            Some(at) => at,
            None => {
                out.push(Window { id: id.clone(), path: path(&id), quotes: Vec::new() });
                out.len() - 1
            }
        };
        if !q.is_empty() && !out[at].quotes.contains(&q) {
            out[at].quotes.push(q);
        }
    }
    out
}

/// A task's own part of its id: `rd1` of `evl_1~rd1`.
fn local(task: &str) -> &str {
    task.split_once('~').map_or(task, |(_, l)| l)
}

impl EvalView {
    pub fn build(src: Sources<'_>) -> EvalView {
        let title = list(src.brief, "asks").first().map(|a| s(a, "title")).unwrap_or_default();
        match src.record {
            Some(record) => EvalView { title, ..EvalView::of_record(src.eval_id, record) },
            None => EvalView::live(src, title),
        }
    }

    /// A closed Eval, from its record alone.
    pub fn of_record(eval_id: &str, record: &Value) -> EvalView {
        let answers = |x: &Value| -> Vec<Option<Value>> {
            list(x, "readings")
                .iter()
                .map(|r| r.get("answer").filter(|a| !a.is_null()).cloned())
                .collect()
        };
        let mut reads: Vec<Read1> = list(record, "units")
            .iter()
            .map(|u| Read1 {
                key: format!("w|{}", s(u, "id")),
                unit: true,
                what: s(u, "path"),
                result: s(u, "result"),
                answers: answers(u),
            })
            .collect();
        reads.extend(list(record, "requirements").iter().map(|r| Read1 {
            key: format!("r|{}", s(r, "id")),
            unit: false,
            what: s(r, "text"),
            result: match s(r, "result").as_str() {
                "unknown" => "unsettled".into(),
                other => other.into(),
            },
            answers: answers(r),
        }));
        let report = &record["report"];
        let questions = list(record, "questions");
        let carried = questions
            .iter()
            .filter(|q| q["asked_before"].is_object())
            .map(|q| {
                let items = strs(q, "items");
                let key = items
                    .iter()
                    .find(|i| i.starts_with("c|"))
                    .cloned()
                    .unwrap_or_else(|| format!("q|{}", s(q, "id")));
                Carried {
                    key,
                    behaviour: s(q, "behaviour"),
                    date: s(&q["asked_before"], "date"),
                    windows: items
                        .iter()
                        .filter_map(|i| i.strip_prefix("w|"))
                        .map(str::to_string)
                        .collect(),
                }
            })
            .collect();
        let failed_checks: Vec<String> = list(record, "checks")
            .iter()
            .filter(|c| s(c, "outcome") != "succeeded")
            .map(|c| s(c, "command"))
            .collect();
        let rep = Report {
            declared: list(report, "declared")
                .iter()
                .map(|d| (strs(d, "items"), s(d, "quote")))
                .collect(),
            against: list(report, "against_decisions").iter().collect(),
            checks: list(report, "cannot_settle")
                .iter()
                .map(|c| (s(c, "item"), s(c, "check")))
                .collect(),
            questions,
            carried,
            again: BTreeSet::new(),
            failed_checks,
        };
        let (items, quoted) = items_of(&reads, &rep);
        let paths: BTreeMap<String, String> =
            list(record, "units").iter().map(|u| (s(u, "id"), s(u, "path"))).collect();
        let faults = list(report, "faults")
            .iter()
            .filter_map(|f| {
                let item = s(f, "item");
                let (q, id) = item.split_once('|')?;
                let r = reads.iter().find(|r| r.key == item)?;
                Some(Fault {
                    requirement: q == "r",
                    what: if q == "r" { first_sentence(&r.what) } else { paths.get(id)?.clone() },
                    why: s(f, "reason"),
                })
            })
            .collect();
        let mut readers: Vec<String> = Vec::new();
        for r in list(record, "readers") {
            let host = s(r, "host");
            if !host.is_empty() && !readers.contains(&host) {
                readers.push(host);
            }
        }
        EvalView {
            eval_id: eval_id.to_string(),
            title: String::new(),
            closed: true,
            items,
            windows: windows_of(quoted, |id| paths.get(id).cloned().unwrap_or_default()),
            faults,
            questions: questions.len(),
            readers,
            progress: None,
            waiting: false,
            checks: list(record, "checks")
                .iter()
                .map(|c| {
                    format!("`{}`: {} in {} s", s(c, "command"), s(c, "outcome"), c["seconds"])
                })
                .collect(),
            checks_running: Vec::new(),
            checks_passed: checks_passed(record),
            limitations: strs(record, "limitations"),
        }
    }

    /// A running Eval: the readings in so far, settled as RingFrame will.
    fn live(src: Sources<'_>, title: String) -> EvalView {
        let mut readings: Vec<&Value> = src
            .outputs
            .iter()
            .filter(|o| matches!(s(o, "kind").as_str(), "read" | "reread"))
            .collect();
        readings.sort_by_key(|o| local(&s(o, "task_id")).to_string());
        let (mechanical, units): (Vec<&Value>, Vec<&Value>) = list(src.evidence, "windows")
            .iter()
            .partition(|w| MECHANICAL.contains(&s(w, "class").as_str()));
        let reqs: Vec<Value> = requirements(src.changes, src.outputs)
            .into_iter()
            .filter(|r| matches!(s(r, "kind").as_str(), "local" | "cross"))
            .collect();
        let mut reads: Vec<Read1> = units
            .iter()
            .map(|w| Read1 {
                key: format!("w|{}", s(w, "id")),
                unit: true,
                what: s(w, "path"),
                result: String::new(),
                answers: Vec::new(),
            })
            .collect();
        reads.extend(reqs.iter().map(|r| Read1 {
            key: format!("r|{}", s(r, "id")),
            unit: false,
            what: s(r, "text"),
            result: String::new(),
            answers: Vec::new(),
        }));
        // Each reading answers the items RingFrame handed it: every item
        // first, then those still open.
        for (k, out) in readings.iter().enumerate() {
            let said: BTreeMap<String, &Value> = list(out, "windows")
                .iter()
                .map(|w| (format!("w|{}", s(w, "id")), w))
                .chain(list(out, "requirements").iter().map(|r| (format!("r|{}", s(r, "id")), r)))
                .collect();
            for r in &mut reads {
                if k == 0 || open_again(&r.said()) {
                    let answer = said.get(&r.key).map(|v| (*v).clone());
                    r.answers.push(answer);
                }
            }
        }
        for r in &mut reads {
            r.result =
                if r.answers.is_empty() { String::new() } else { result_of(&r.said()).to_string() };
        }
        let task = |l: &str| format!("{}~{l}", src.eval_id);
        let planned = |l: &str| src.planned.contains(&task(l));
        let done = |l: &str| src.done.contains(&task(l));
        let rereading = ["rr1", "rr2"].iter().any(|l| planned(l) && !done(l));
        let again: BTreeSet<String> = if rereading && !readings.is_empty() {
            reads.iter().filter(|r| open_again(&r.said())).map(|r| r.key.clone()).collect()
        } else {
            BTreeSet::new()
        };
        let carried = list(src.brief, "carried")
            .iter()
            .map(|d| Carried {
                key: format!("c|{}", s(d, "id")),
                behaviour: s(d, "behaviour"),
                date: s(d, "date"),
                windows: Vec::new(),
            })
            .collect();
        let declared = readings
            .iter()
            .flat_map(|o| list(o, "declared"))
            .map(|d| (strs(d, "items"), s(d, "quote")))
            .collect();
        let rep = Report { declared, carried, again, ..Report::default() };
        let answered = reads.iter().filter(|r| r.answers.iter().any(Option::is_some)).count();
        let reads_of = (answered, reads.len());
        let waiting = readings.is_empty();
        let mut read: Vec<Read1> = if waiting { Vec::new() } else { reads };
        if !waiting {
            // The units RingFrame never asks about follow from the rest.
            read.extend(mechanical.iter().map(|w| Read1 {
                key: format!("w|{}", s(w, "id")),
                unit: true,
                what: s(w, "path"),
                result: "mechanical".into(),
                answers: Vec::new(),
            }));
        }
        let (items, quoted) = items_of(&read, &rep);
        let paths: BTreeMap<String, String> =
            list(src.evidence, "windows").iter().map(|w| (s(w, "id"), s(w, "path"))).collect();
        let progress = Progress {
            reads: reads_of,
            ..progress(&planned, &done, readings.len(), src.checks_running)
        };
        EvalView {
            eval_id: src.eval_id.to_string(),
            title,
            closed: false,
            items,
            windows: windows_of(quoted, |id| paths.get(id).cloned().unwrap_or_default()),
            faults: Vec::new(),
            questions: 0,
            readers: Vec::new(),
            progress: Some(progress),
            waiting,
            checks: Vec::new(),
            checks_running: src.checks_running.to_vec(),
            checks_passed: false,
            limitations: Vec::new(),
        }
    }

    /// How many items are to look at.
    pub fn to_look_at(&self) -> usize {
        self.items.iter().filter(|i| i.kind.to_look_at()).count()
    }

    /// How many things are settled: each requirement, and each change gathered.
    pub fn settled(&self) -> usize {
        self.items.iter().filter(|i| !i.kind.to_look_at()).map(|i| i.count.max(1)).sum()
    }

    /// One sentence: `5 things to look at · 12 settled`.
    pub fn headline(&self) -> String {
        let look = match self.to_look_at() {
            0 => word("eval_view.look_none").to_string(),
            1 => word("eval_view.look_one").to_string(),
            n => fill("eval_view.look_n", &[("n", &n.to_string())]),
        };
        match self.settled() {
            0 => look,
            n => format!("{look} · {}", fill("eval_view.settled_n", &[("n", &n.to_string())])),
        }
    }

    /// While it runs, its progress. None once it has closed.
    pub fn running(&self) -> Option<&Progress> {
        self.progress.as_ref().filter(|_| !self.closed)
    }

    pub fn item(&self, key: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.key == key)
    }

    pub fn window(&self, id: &str) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }
}

/// One ring with an arc per reading, and the steps in quiet text.
fn progress(
    planned: &dyn Fn(&str) -> bool,
    done: &dyn Fn(&str) -> bool,
    reads_in: usize,
    checks_running: &[String],
) -> Progress {
    let reads = ["rd1", "rr1", "rr2"];
    let arcs: Vec<Light> = reads
        .iter()
        .enumerate()
        .map(|(i, l)| match (done(l), planned(l) || i == 0) {
            (true, _) => Light::Done,
            (false, true) => Light::Now,
            (false, false) => Light::Next,
        })
        .collect();
    let rings = ["eval_view.ring.1", "eval_view.ring.2", "eval_view.ring.3"];
    let questions_now = planned("qs1") && !done("qs1");
    let (label, note) = match arcs.iter().position(|a| *a == Light::Now) {
        _ if questions_now => (word("eval_view.ring.questions"), word("eval_view.ring.writing")),
        Some(i) => (word(rings[i]), word("eval_view.ring.reading")),
        None => (word(rings[reads_in.clamp(1, 3) - 1]), word("eval_view.ring.read")),
    };
    let lit = |light: Light, [done_, now, next]: [&str; 3]| {
        let key = match light {
            Light::Done => done_,
            Light::Now => now,
            Light::Next => next,
        };
        Lit { name: word(key).into(), light, key: key.into() }
    };
    let asked = if planned("q1") && !done("q1") { Light::Now } else { Light::Done };
    let read = if done("rd1") {
        Light::Done
    } else if asked == Light::Done {
        Light::Now
    } else {
        Light::Next
    };
    let rereads: Vec<&str> = ["rr1", "rr2"].into_iter().filter(|l| planned(l)).collect();
    let again = if rereads.iter().any(|l| !done(l)) {
        lit(Light::Now, ["", "eval_view.step.again_now", ""])
    } else if !rereads.is_empty() {
        lit(Light::Done, ["eval_view.step.again", "", ""])
    } else if done("rd1") && (planned("qs1") || reads_in > 0) {
        lit(Light::Done, ["eval_view.step.again_none", "", ""])
    } else {
        lit(Light::Next, ["", "", "eval_view.step.again_next"])
    };
    let questions = if done("qs1") {
        Light::Done
    } else if questions_now {
        Light::Now
    } else {
        Light::Next
    };
    Progress {
        arcs,
        label: label.into(),
        note: note.into(),
        steps: vec![
            lit(asked, ["eval_view.step.asked", "eval_view.step.asking", "eval_view.step.asked"]),
            lit(read, ["eval_view.step.read", "eval_view.step.reading", "eval_view.step.reading"]),
            again,
            lit(
                questions,
                [
                    "eval_view.step.questions",
                    "eval_view.step.questions_now",
                    "eval_view.step.questions_next",
                ],
            ),
        ],
        tests: checks_running
            .iter()
            .map(|c| fill("eval_view.test_running", &[("check", c)]))
            .collect(),
        reads: (0, 0),
    }
}

// ---- what a device is sent --------------------------------------------------------

/// The whole view as a device draws it: the list, each item's detail, and
/// the lines of each change an item points at, with the ones a reading
/// quoted lit. `hunks` holds the text of each change read.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Payload {
    pub eval_id: String,
    pub title: String,
    pub closed: bool,
    pub headline: String,
    pub waiting: bool,
    pub items: Vec<Item>,
    pub lines: BTreeMap<String, Lines>,
    /// While it runs.
    pub progress: Option<Progress>,
}

/// One change's file and its lines.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Lines {
    pub path: String,
    pub lines: Vec<Line>,
}

/// One line of a change, with its number on the side it is on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Line {
    pub n: Option<u32>,
    pub text: String,
    /// A reading quoted it.
    pub lit: bool,
}

pub fn payload(view: &EvalView, hunks: &BTreeMap<String, String>) -> Payload {
    let lines = view
        .windows
        .iter()
        .filter_map(|w| {
            let lines = numbered(hunks.get(&w.id)?, &w.quotes);
            Some((w.id.clone(), Lines { path: w.path.clone(), lines }))
        })
        .collect();
    Payload {
        eval_id: view.eval_id.clone(),
        title: view.title.clone(),
        closed: view.closed,
        headline: if view.closed { view.headline() } else { String::new() },
        waiting: view.waiting,
        items: view.items.clone(),
        lines,
        progress: view.running().cloned(),
    }
}

/// A change's lines without the window's own `---` and `@@` headings, each
/// numbered from its hunk: a removed line on the old side, the rest on the new.
pub fn numbered(text: &str, quotes: &[String]) -> Vec<Line> {
    let (mut old, mut new) = (0u32, 0u32);
    let at = |h: &str, side: char| {
        h.split_whitespace()
            .find_map(|p| p.strip_prefix(side))
            .and_then(|p| p.split(',').next()?.parse().ok())
    };
    let mut out = Vec::new();
    for (line, lit) in hunk_lines(text, quotes) {
        if line.starts_with("--- ") {
            continue;
        }
        if line.starts_with("@@ ") {
            (old, new) = (at(&line, '-').unwrap_or(old), at(&line, '+').unwrap_or(new));
            continue;
        }
        let n = match line.chars().next() {
            Some('-') => (old += 1, old - 1).1,
            Some('+') => (new += 1, new - 1).1,
            Some(' ') => (old += 1, new += 1, new - 1).2,
            _ => {
                out.push(Line { n: None, text: line, lit });
                continue;
            }
        };
        out.push(Line { n: Some(n), text: line, lit });
    }
    out
}

/// A change's hunk, each line with whether a reading's quote is on it.
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

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/eval-v4/record.json")).unwrap()
    }

    fn closed() -> EvalView {
        let (record, none) = (fixture(), json!({}));
        let brief = json!({"asks": [{"ask_id": "ask_1", "title": "fix login"}]});
        EvalView::build(Sources {
            eval_id: "evl_v4",
            changes: &none,
            evidence: &none,
            brief: &brief,
            record: Some(&record),
            outputs: &[],
            planned: &[],
            done: &[],
            checks_running: &[],
        })
    }

    fn listed(v: &EvalView) -> Vec<(Kind, String)> {
        v.items.iter().map(|i| (i.kind, i.text.clone())).collect()
    }

    #[test]
    fn a_closed_eval_lists_what_to_look_at_first_the_question_from_an_earlier_seal_on_top() {
        let v = closed();
        assert_eq!(v.title, "fix login");
        assert_eq!(
            listed(&v),
            [
                (
                    Kind::Before,
                    "Asked before · The sign-in page remembers the last email address typed on this device."
                        .to_string()
                ),
                (Kind::Look, "Signed-in sessions end after 1 hour instead of 24.".into()),
                (
                    Kind::Look,
                    "A failed sign-in now writes the email address that was typed into the server log."
                        .into()
                ),
                (Kind::Look, "Staying signed in on this device is not in the change.".into()),
                (Kind::Unsettled, "“show an error” — the reads differ".into()),
                (Kind::Settled, "The sign-in button is off while it sends.".into()),
                (Kind::Settled, "3 other changes your Ask explains".into()),
            ]
        );
        assert_eq!(v.headline(), "5 things to look at · 4 settled");
        let marks: Vec<&str> = v.items.iter().map(|i| i.kind.mark()).collect();
        assert_eq!(marks, ["↺", "■", "■", "■", "◆", "●", "●"]);
        assert_eq!((v.questions, v.readers.clone()), (5, vec!["claude".to_string()]));
        assert!(v.checks_passed && v.progress.is_none() && !v.waiting);
    }

    #[test]
    fn each_item_s_detail_says_what_it_is_why_how_sure_and_what_the_agent_said() {
        let v = closed();
        let before = v.item("c|sel_1/d2").unwrap();
        assert_eq!(before.head, "Asked before · not sure at your Seal of 10 Nov");
        assert_eq!(
            before.big,
            "The sign-in page remembers the last email address typed on this device."
        );
        assert!(
            before.why[0].starts_with("When you sealed this work on 10 Nov"),
            "{:?}",
            before.why
        );
        assert!(before.dots.is_empty() && before.sure.is_empty(), "no reading of its own");

        let sess = v.item("w|w_sess").unwrap();
        assert_eq!(sess.head, "To look at");
        assert_eq!(sess.dots, [true, true]);
        assert_eq!(sess.sure, "read twice, both agree");
        assert_eq!(sess.why, ["The agent didn't mention it, and your Ask doesn't cover it."]);
        assert_eq!(sess.said, None);
        assert_eq!(sess.windows, ["w_sess"]);
        assert_eq!(
            v.window("w_sess").unwrap().quotes,
            ["maxAgeHours: 1,", "rolling: false,"],
            "every quote a reading gave lights its line"
        );
        assert_eq!(
            sess.reads,
            [
                Read {
                    who: "First read".into(),
                    said: "Signed-in sessions end after 1 hour instead of 24.".into()
                },
                Read { who: "Second read".into(), said: "Sessions are cut to an hour.".into() },
            ]
        );

        let log = v.item("w|w_log").unwrap();
        assert_eq!(log.head, "To look at · against your decision of 3 Nov");
        assert_eq!(
            log.why,
            [
                "It goes against your decision of 3 Nov, when you said you didn’t want this: Emails are written to the server log.",
                "Your Ask doesn't mention it either.",
            ]
        );
        assert_eq!(
            log.said.as_deref(),
            Some("Failed sign-ins are now logged with the email so we can spot attacks."),
            "what the agent declared is quoted, and the item stays to look at"
        );

        let stay = v.item("r|ask_2/f0").unwrap();
        assert_eq!(
            stay.why,
            ["You asked for it in your second Ask.", "What is missing: a remembered session"]
        );
        assert_eq!((stay.dots.as_slice(), stay.sure.as_str()), (&[true][..], "read once"));

        let error = v.item("r|ask_1/f0").unwrap();
        assert_eq!(error.head, "Could not settle");
        assert_eq!(error.big, "A wrong password shows an error.");
        assert_eq!(error.dots, [false, false, false], "none agree on an unsettled item");
        assert_eq!(error.sure, "read three times, the reads differ");
        assert_eq!(
            error.check.as_deref(),
            Some(
                "What would settle it: type a wrong password on the sign-in page and see what appears"
            )
        );
        assert_eq!(
            error.reads[0].said,
            "Can’t tell: a message appears under the field, but the Ask may mean a pop-up"
        );
        assert_eq!(error.windows, ["w_form"], "the lines a reading cited");

        let button = v.item("r|ask_1/f1").unwrap();
        assert_eq!((button.head.as_str(), button.sure.as_str()), ("Settled", "read once"));
        let gathered = v.item("settled|explained").unwrap();
        assert_eq!(gathered.count, 3, "two explained and one never asked about");
        assert!(gathered.reads.is_empty() && gathered.windows.is_empty());
    }

    #[test]
    fn nothing_on_the_view_says_done_met_aligned_or_a_number_for_how_sure() {
        let v = closed();
        let wire = serde_json::to_string(&payload(&v, &BTreeMap::new())).unwrap().to_lowercase();
        for word in ["verdict", "aligned", "confidence", "\"met\"", "%"] {
            assert!(!wire.contains(word), "{word} in {wire}");
        }
        for i in &v.items {
            let said = format!("{} {} {} {}", i.text, i.head, i.big, i.sure).to_lowercase();
            assert!(!said.contains(" met") && !said.contains("done"), "{said}");
        }
    }

    #[test]
    fn a_fault_is_named_for_a_follow_up() {
        let v = closed();
        assert_eq!(
            v.faults,
            [
                Fault {
                    requirement: false,
                    what: "src/auth/session.ts".into(),
                    why: "Sessions are cut to an hour.".into()
                },
                Fault {
                    requirement: true,
                    what: "Keep me signed in on this device.".into(),
                    why: "no unit keeps the session past the browser closing".into()
                },
            ]
        );
    }

    #[test]
    fn a_record_without_the_new_fields_reads_as_nothing_to_look_at() {
        let v = EvalView::of_record("evl_0", &json!({"schema": "ringframe.eval/2"}));
        assert!(v.items.is_empty() && v.closed);
        assert_eq!(v.headline(), "Nothing to look at");
    }

    #[test]
    fn checks_passed_when_every_check_succeeded_and_none_ran_out_of_time() {
        let closed = |checks: Value, limitations: Value| {
            EvalView::of_record("e", &json!({"checks": checks, "limitations": limitations}))
        };
        let ok = json!({"command": "cargo test", "outcome": "succeeded", "seconds": 1});
        let failed = json!({"command": "cargo clippy", "outcome": "failed", "seconds": 1});
        assert!(closed(json!([ok]), json!([])).checks_passed);
        assert!(closed(json!([]), json!([])).checks_passed, "nothing to run, nothing failed");
        let v = closed(json!([ok, failed]), json!([]));
        assert!(!v.checks_passed);
        assert_eq!(listed(&v), [(Kind::Look, "cargo clippy did not pass".to_string())]);
        assert!(!closed(
            json!([]),
            json!(["the checks did not finish in their time and are not in this record: `cargo test`"])
        )
        .checks_passed);
    }

    // ---- while it runs --------------------------------------------------------------

    fn evidence() -> Value {
        json!({"windows": [
            {"id": "w_sess", "path": "src/auth/session.ts", "class": "code"},
            {"id": "w_btn", "path": "src/ui/button.tsx", "class": "code"},
            {"id": "w_lock", "path": "package-lock.json", "class": "lock"}
        ]})
    }

    fn changes() -> Value {
        json!({"requirements": [
            {"id": "R1", "kind": "local", "text": "A wrong password shows an error. Under the field."},
            {"id": "R2", "kind": "local", "text": "The button is off while it sends."},
            {"id": "ask_1/s0", "kind": "scope", "text": "Work in this repo."}
        ]})
    }

    fn rd1() -> Value {
        json!({"kind": "read", "task_id": "evl_1~rd1",
            "windows": [
                {"id": "w_sess", "answer": "unexplained", "quote": "maxAgeHours: 1,", "reason": "Sessions end after an hour."},
                {"id": "w_btn", "answer": "serves", "serves": ["R2"]}],
            "requirements": [
                {"id": "R1", "vote": "unknown", "reason": "", "missing": "which error", "citations": []},
                {"id": "R2", "vote": "met", "reason": "disabled while sending", "citations": [{"window": "w_btn", "quote": "disabled={sending}"}]}]})
    }

    fn running(outputs: &[Value], planned: &[&str], done: &[&str], checks: &[String]) -> EvalView {
        let (c, e) = (changes(), evidence());
        let brief = json!({"asks": [{"ask_id": "ask_1", "title": "fix login"}],
            "carried": [{"id": "sel_1/d2", "behaviour": "Remembers the last email.", "date": "2026-11-10"}]});
        let planned: Vec<String> = planned.iter().map(|t| format!("evl_1~{t}")).collect();
        let done: Vec<String> = done.iter().map(|t| format!("evl_1~{t}")).collect();
        EvalView::build(Sources {
            eval_id: "evl_1",
            changes: &c,
            evidence: &e,
            brief: &brief,
            record: None,
            outputs,
            planned: &planned,
            done: &done,
            checks_running: checks,
        })
    }

    fn steps(p: &Progress) -> Vec<(&str, Light)> {
        p.steps.iter().map(|s| (s.name.as_str(), s.light)).collect()
    }

    #[test]
    fn before_the_first_reading_only_the_carried_question_is_listed_and_the_first_arc_breathes() {
        let v = running(&[], &["rd1"], &[], &[]);
        assert!(!v.closed && v.waiting);
        assert_eq!(
            listed(&v),
            [(Kind::Before, "Asked before · Remembers the last email.".to_string())]
        );
        let p = v.running().unwrap();
        assert_eq!(p.arcs, [Light::Now, Light::Next, Light::Next]);
        assert_eq!((p.label.as_str(), p.note.as_str()), ("1st read", "reading"));
        assert_eq!(
            steps(p),
            [
                ("What you asked", Light::Done),
                ("Reading the change, all of it at once", Light::Now),
                ("Reading again where unsure, if needed", Light::Next),
                ("Questions for your Seal, if needed", Light::Next),
            ]
        );
        let asking = running(&[], &["q1"], &[], &[]);
        assert_eq!(steps(asking.running().unwrap())[0], ("Reading what you asked", Light::Now));
    }

    #[test]
    fn the_list_fills_in_as_readings_come_back_and_an_item_read_again_says_so() {
        let v = running(&[rd1()], &["rd1", "rr1"], &["rd1"], &["npm test".to_string()]);
        assert!(!v.waiting);
        assert_eq!(
            listed(&v),
            [
                (Kind::Before, "Asked before · Remembers the last email.".to_string()),
                (Kind::Look, "Sessions end after an hour.".into()),
                (Kind::Unsettled, "A wrong password shows an error.".into()),
                (Kind::Settled, "The button is off while it sends.".into()),
                (Kind::Settled, "2 other changes your Ask explains".into()),
            ]
        );
        let again: Vec<&str> = v.items.iter().filter(|i| i.again).map(|i| i.key.as_str()).collect();
        assert_eq!(again, ["w|w_sess", "r|R1"], "the unexplained unit and the unknown requirement");
        let p = v.running().unwrap();
        assert_eq!(p.arcs, [Light::Done, Light::Now, Light::Next]);
        assert_eq!((p.label.as_str(), p.note.as_str()), ("2nd read", "reading"));
        assert_eq!(steps(p)[1], ("Read the change", Light::Done));
        assert_eq!(steps(p)[2], ("Reading again where unsure", Light::Now));
        assert_eq!(p.tests, ["npm test is running"]);

        // The second reading agrees on the unit and settles the requirement.
        let rr1 = json!({"kind": "reread", "task_id": "evl_1~rr1",
            "windows": [{"id": "w_sess", "answer": "unexplained", "quote": "rolling: false,", "reason": "Cut to an hour."}],
            "requirements": [{"id": "R1", "vote": "not_met", "reason": "a pop-up", "citations": []}]});
        let v = running(&[rd1(), rr1.clone()], &["rd1", "rr1", "rr2"], &["rd1", "rr1"], &[]);
        assert_eq!(v.item("w|w_sess").unwrap().dots, [true, true]);
        assert!(!v.item("w|w_sess").unwrap().again, "two that agree are not read again");
        let r1 = v.item("r|R1").unwrap();
        assert_eq!((r1.kind, r1.again), (Kind::Unsettled, true), "a split is read a third time");
        assert_eq!(v.running().unwrap().label, "3rd read");

        // Once the readings are in, the questions are written.
        let v = running(&[rd1(), rr1], &["rd1", "rr1", "qs1"], &["rd1", "rr1"], &[]);
        let p = v.running().unwrap();
        assert_eq!(p.arcs, [Light::Done, Light::Done, Light::Next]);
        assert_eq!((p.label.as_str(), p.note.as_str()), ("Questions", "being written"));
        assert_eq!(steps(p)[2], ("Read again where unsure", Light::Done));
        assert_eq!(steps(p)[3], ("Writing questions for your Seal", Light::Now));
    }

    #[test]
    fn a_device_gets_every_item_and_the_lines_of_each_change_they_point_at() {
        let v = closed();
        let hunks: BTreeMap<String, String> = [(
            "w_sess".to_string(),
            "--- src/auth/session.ts\n@@ -10,3 +10,4 @@\n export const SESSION = {\n-  maxAgeHours: 24,\n+  maxAgeHours: 1,\n+  rolling: false,\n".to_string(),
        )]
        .into();
        let p = payload(&v, &hunks);
        assert_eq!(p.headline, "5 things to look at · 4 settled");
        assert_eq!(p.items, v.items);
        let lines = &p.lines["w_sess"];
        assert_eq!(lines.path, "src/auth/session.ts");
        let lit: Vec<(Option<u32>, bool)> = lines.lines.iter().map(|l| (l.n, l.lit)).collect();
        assert_eq!(lit, [(Some(10), false), (Some(11), false), (Some(11), true), (Some(12), true)]);
        assert!(p.progress.is_none());
        let wire = serde_json::to_value(&p).unwrap();
        assert_eq!(wire["items"][0]["kind"], "before");
    }

    #[test]
    fn a_date_reads_as_a_day_and_a_month() {
        assert_eq!(day("2026-11-03"), "3 Nov");
        assert_eq!(day("2026-01-17T10:00:00Z"), "17 Jan");
        assert_eq!(day("soon"), "soon");
    }

    #[test]
    fn a_change_is_numbered_on_the_side_each_line_is_on() {
        let text = "--- src/send.rs\n@@ -88 +88 @@\n fn send() {\n-    let _ = w();\n+    if let Err(e) = w() {\n+        return Err(e);\n+    }\n     Ok(())\n";
        let lines = numbered(text, &[]);
        let n: Vec<Option<u32>> = lines.iter().map(|l| l.n).collect();
        assert_eq!(n, [Some(88), Some(89), Some(89), Some(90), Some(91), Some(92)]);
        assert_eq!(lines[1].text, "-    let _ = w();");
    }

    #[test]
    fn a_hunk_lights_the_lines_a_reading_quoted() {
        let text = "--- src/server.rs\n@@ -1 +1,2 @@\n+let mut unrecorded = None;\n+let x = 1;\n";
        let lit: Vec<bool> =
            hunk_lines(text, &["let mut unrecorded =  None;".into()]).iter().map(|l| l.1).collect();
        assert_eq!(lit, [false, false, true, false]);
    }
}
