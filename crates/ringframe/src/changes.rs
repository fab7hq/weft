//! The change DAG (ADR-0020): the
//! project's change as units, the commits that made them, and the edges
//! between them, all read from git. An Eval publishes its view of it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::digest;
use crate::evaluate::EvalError;
use crate::evidence::{Evidence, FileDiff, MECHANICAL, hunk_item};
use crate::store;
use crate::workspace::Workspace;

/// One change to one syntactic item, as `git diff` names it, with the two
/// ends of any move it takes part in.
#[derive(Clone, Debug)]
pub struct Unit {
    /// `<path>#<item>`, the path as the Eval names it (another repository's
    /// paths carry its name first).
    pub key: String,
    pub repo: String,
    pub path: String,
    pub item: String,
    /// The first 12 hex of the SHA-256 of its changed lines, in order,
    /// without line numbers: a unit whose lines only shifted keeps it.
    pub hash: String,
    /// The keys merged into it: the other ends of its moves.
    pub also: Vec<String>,
    pub windows: Vec<String>,
    pub bytes: usize,
    /// `mechanical` when every window is, `test` when every window is test
    /// code, else `code`.
    pub class: String,
    /// Its removed and added lines, for names and history.
    pub(crate) lines: Vec<(u8, String)>,
    /// The new-side line numbers of its added lines, with their file (the
    /// two ends of a move are in two files).
    pub(crate) added_at: Vec<(String, u32)>,
    /// The commits in range that added its lines, most lines first.
    pub commits: Vec<String>,
}

impl Unit {
    pub fn to_json(&self) -> Value {
        json!({
            "key": self.key, "repo": self.repo, "path": self.path, "item": self.item,
            "hash": self.hash, "also": self.also, "windows": self.windows,
            "bytes": self.bytes, "class": self.class, "commits": self.commits,
        })
    }
}

struct Part {
    repo: String,
    path: String,
    item: String,
    lines: Vec<(u8, String)>,
    added_at: Vec<u32>,
    moves: BTreeSet<usize>,
}

fn find(par: &mut BTreeMap<String, String>, x: &str) -> String {
    let mut x = x.to_string();
    loop {
        let p = par.get(&x).cloned().unwrap_or_else(|| x.clone());
        if p == x {
            return x;
        }
        let gp = par.get(&p).cloned().unwrap_or_else(|| p.clone());
        par.insert(x.clone(), gp.clone());
        x = gp;
    }
}

/// The units of a prepared change.
pub fn units(ev: &Evidence) -> Vec<Unit> {
    let mut parts: BTreeMap<String, Part> = BTreeMap::new();
    for f in &ev.files {
        collect(f, &mut parts);
    }
    // A file with no hunks (binary, renamed only) is one unit of its own.
    for w in &ev.windows {
        if w.items.is_empty() {
            let key = format!("{}#<file>", w.path);
            parts.entry(key).or_insert_with(|| Part {
                repo: w.repo.clone(),
                path: w.path.clone(),
                item: "<file>".into(),
                lines: Vec::new(),
                added_at: Vec::new(),
                moves: BTreeSet::new(),
            });
        }
    }
    // The two ends of a move are one change.
    let mut par: BTreeMap<String, String> = BTreeMap::new();
    let mut by_move: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (k, p) in &parts {
        for m in &p.moves {
            by_move.entry(*m).or_default().push(k.clone());
        }
    }
    for keys in by_move.values() {
        for k in &keys[1..] {
            let (a, b) = (find(&mut par, &keys[0]), find(&mut par, k));
            if a != b {
                par.insert(b, a);
            }
        }
    }
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for k in parts.keys() {
        groups.entry(find(&mut par, k)).or_default().push(k.clone());
    }
    let windows: BTreeMap<&str, &crate::evidence::Window> =
        ev.windows.iter().map(|w| (w.id.as_str(), w)).collect();
    let mut out = Vec::new();
    for members in groups.values() {
        // Where the code is now names the unit: the member that added lines.
        let lead = members
            .iter()
            .max_by_key(|k| {
                (parts[*k].lines.iter().filter(|(t, _)| *t == b'+').count(), std::cmp::Reverse(*k))
            })
            .expect("a member");
        let p = &parts[lead];
        let mut text = String::new();
        let mut lines = Vec::new();
        let mut added_at = Vec::new();
        for k in members {
            for (tag, l) in &parts[k].lines {
                text.push(*tag as char);
                text.push_str(l);
                text.push('\n');
                lines.push((*tag, l.clone()));
            }
            added_at.extend(parts[k].added_at.iter().map(|n| (parts[k].path.clone(), *n)));
        }
        let mut ws: Vec<String> = Vec::new();
        for w in &ev.windows {
            let hit = members.iter().any(|k| {
                let q = &parts[k];
                w.path == q.path
                    && (w.items.contains(&q.item) || (q.item == "<file>" && w.items.is_empty()))
            });
            if hit {
                ws.push(w.id.clone());
            }
        }
        let classes: Vec<&str> =
            ws.iter().filter_map(|w| windows.get(w.as_str())).map(|w| w.class.as_str()).collect();
        let class = if !classes.is_empty() && classes.iter().all(|c| MECHANICAL.contains(c)) {
            "mechanical"
        } else if !classes.is_empty() && classes.iter().all(|c| *c == "test") {
            "test"
        } else {
            "code"
        };
        out.push(Unit {
            key: lead.clone(),
            repo: p.repo.clone(),
            path: p.path.clone(),
            item: p.item.clone(),
            hash: digest::sha256_bytes(text.as_bytes())[..12].to_string(),
            also: members.iter().filter(|k| *k != lead).cloned().collect(),
            bytes: ws.iter().filter_map(|w| windows.get(w.as_str())).map(|w| w.text.len()).sum(),
            windows: ws,
            class: class.into(),
            lines,
            added_at,
            commits: Vec::new(),
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

fn collect(f: &FileDiff, parts: &mut BTreeMap<String, Part>) {
    for h in &f.hunks {
        for l in h.lines.iter().filter(|l| l.tag != b' ') {
            let item = if l.item.is_empty() { hunk_item(&h.header) } else { l.item.clone() };
            let part = parts.entry(format!("{}#{}", f.path, item)).or_insert_with(|| Part {
                repo: f.repo.clone(),
                path: f.path.clone(),
                item: item.clone(),
                lines: Vec::new(),
                added_at: Vec::new(),
                moves: BTreeSet::new(),
            });
            part.lines.push((l.tag, l.text.clone()));
            if l.tag == b'+' {
                part.added_at.push(l.new);
            }
            if let Some(m) = l.moved {
                part.moves.insert(m);
            }
        }
    }
}

// ---- history ------------------------------------------------------------------

/// A commit in an Eval's range.
#[derive(Clone, Debug)]
pub struct Commit {
    pub repo: String,
    pub hash: String,
    /// Its place in its repository's range, oldest first.
    pub order: usize,
    pub subject: String,
}

/// The commits of each repository's range, and which rewrote which: commit
/// B rewrites commit A when B removes a line A added, both in range.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub commits: Vec<Commit>,
    /// `(b, a)`: b rewrote lines a added.
    pub rewrites: Vec<(String, String)>,
    /// Where each rewrite happened: the files, as the Eval names them, in
    /// which b removed lines a added.
    pub rewritten_in: BTreeMap<(String, String), BTreeSet<String>>,
    /// The first lines b removed of those a added, with their file: what an
    /// undo took away, which the net change may no longer show (a revert to
    /// the text before the range leaves no window).
    pub removed: BTreeMap<(String, String), Vec<(String, String)>>,
    /// Each window's added lines, by the commit in range that added them
    /// (`git blame` at the subject).
    pub window_commits: BTreeMap<String, BTreeSet<String>>,
}

/// A line long enough to be the same line, not a brace or a blank.
const SAME_LINE: usize = 8;

/// How many of the lines a rewrite removed its finding names.
const REMOVED_SHOWN: usize = 4;

/// Read each repository's commits, give each unit the commits that added its
/// lines (blame at the subject), and find the rewrites. A Git read that fails
/// fails the Eval: a repository left out would lose its commits, claims and
/// rewrites without a word (q05, Codex a3).
pub fn history(
    ev: &Evidence,
    workspace_repo: usize,
    units: &mut [Unit],
) -> Result<History, EvalError> {
    let mut out = History::default();
    let mut blame_of: BTreeMap<String, BTreeMap<u32, String>> = BTreeMap::new();
    for (i, r) in ev.repos.iter().enumerate() {
        let dir = r.root_for_diff(i == workspace_repo);
        let head = if r.subject_kind == "git_commit" { r.subject_ref.as_str() } else { "HEAD" };
        let range = format!("{}..{}", r.anchor, head);
        let list = crate::evaluate::git(&dir, &["rev-list", "--reverse", &range])?;
        let hashes: Vec<String> = list.split_whitespace().map(str::to_string).collect();
        let in_range: BTreeSet<&str> = hashes.iter().map(String::as_str).collect();
        for (order, h) in hashes.iter().enumerate() {
            let subject = crate::evaluate::git(&dir, &["log", "-1", "--format=%s", h])?;
            out.commits.push(Commit {
                repo: r.name.clone(),
                hash: h.clone(),
                order,
                subject: subject.trim().to_string(),
            });
        }
        // Rewrites: replay the range, remembering who added each line.
        let mut added_by: BTreeMap<(String, String), String> = BTreeMap::new();
        for h in &hashes {
            let diff = crate::evaluate::git(&dir, &["show", "--format=", "-U0", "--no-color", h])?;
            let mut path = String::new();
            let mut hit: BTreeSet<String> = BTreeSet::new();
            let named =
                |p: &str| if r.name == "." { p.to_string() } else { format!("{}/{p}", r.name) };
            for l in diff.lines() {
                if let Some(p) = l.strip_prefix("+++ ") {
                    path = p.trim_start_matches("b/").to_string();
                } else if let Some(t) = l.strip_prefix('-').filter(|_| !l.starts_with("---")) {
                    let key = (path.clone(), t.trim().to_string());
                    if key.1.len() > SAME_LINE
                        && let Some(a) = added_by.get(&key).filter(|a| *a != h)
                    {
                        hit.insert(a.clone());
                        out.rewritten_in
                            .entry((h.clone(), a.clone()))
                            .or_default()
                            .insert(named(&path));
                        let shown = out.removed.entry((h.clone(), a.clone())).or_default();
                        if shown.len() < REMOVED_SHOWN {
                            shown.push((named(&path), key.1.clone()));
                        }
                    }
                } else if let Some(t) = l.strip_prefix('+').filter(|_| !l.starts_with("+++")) {
                    added_by.insert((path.clone(), t.trim().to_string()), h.clone());
                }
            }
            out.rewrites.extend(hit.into_iter().map(|a| (h.clone(), a)));
        }
        // Introduced by: blame each changed file once, at the subject. A file
        // Git does not track yet was added by no commit.
        let tracked: BTreeSet<String> = if r.subject_kind == "git_commit" {
            BTreeSet::new()
        } else {
            crate::evaluate::git(&dir, &["ls-files", "-z"])?
                .split('\0')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect()
        };
        for u in units.iter().filter(|u| u.repo == r.name) {
            for (path, _) in &u.added_at {
                if blame_of.contains_key(path) {
                    continue;
                }
                let rel =
                    if r.name == "." { path.clone() } else { path[r.name.len() + 1..].to_string() };
                if r.subject_kind != "git_commit" && !tracked.contains(&rel) {
                    blame_of.insert(path.clone(), BTreeMap::new());
                    continue;
                }
                let mut args = vec!["blame", "--line-porcelain"];
                if r.subject_kind == "git_commit" {
                    args.push(&r.subject_ref);
                }
                args.extend(["--", rel.as_str()]);
                let mut lines = BTreeMap::new();
                for l in crate::evaluate::git(&dir, &args)?.lines() {
                    let mut w = l.split(' ');
                    if let (Some(h), Some(_), Some(n)) = (w.next(), w.next(), w.next())
                        && h.len() == 40
                        && h.bytes().all(|b| b.is_ascii_hexdigit())
                        && let Ok(n) = n.parse::<u32>()
                    {
                        let owner =
                            if in_range.contains(h) { h.to_string() } else { String::new() };
                        lines.insert(n, owner);
                    }
                }
                blame_of.insert(path.clone(), lines);
            }
        }
    }
    for u in units.iter_mut() {
        let mut count: BTreeMap<&str, usize> = BTreeMap::new();
        for (path, n) in &u.added_at {
            if let Some(h) = blame_of.get(path).and_then(|b| b.get(n)).filter(|h| !h.is_empty()) {
                *count.entry(h.as_str()).or_default() += 1;
            }
        }
        let mut ranked: Vec<(&str, usize)> = count.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        u.commits = ranked.into_iter().map(|(h, _)| h.to_string()).collect();
    }
    for w in &ev.windows {
        let Some(blame) = blame_of.get(&w.path) else { continue };
        let owners: BTreeSet<String> = w
            .added
            .iter()
            .filter_map(|(n, _)| blame.get(n).filter(|h| !h.is_empty()).cloned())
            .collect();
        if !owners.is_empty() {
            out.window_commits.insert(w.id.clone(), owners);
        }
    }
    Ok(out)
}

// ---- hints ---------------------------------------------------------------------

/// An edge between two units, or from a requirement: what links them and how.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub kind: &'static str,
    pub from: String,
    pub to: String,
    pub via: String,
}

impl Edge {
    pub fn to_json(&self) -> Value {
        json!({"kind": self.kind, "from": self.from, "to": self.to, "via": self.via})
    }
}

const DEFINES: [&str; 12] = [
    "fn",
    "struct",
    "enum",
    "trait",
    "type",
    "const",
    "static",
    "mod",
    "def",
    "class",
    "func",
    "macro_rules",
];
/// A name defined in more units than this is too common to link by.
const DEFINED_AT_MOST: usize = 2;
/// A name used in more units than this links too much.
const USED_AT_MOST: usize = 10;

pub(crate) fn idents(line: &str) -> Vec<&str> {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty() && !t.as_bytes()[0].is_ascii_digit())
        .collect()
}

/// The names a unit's changed lines define (`fn name`, `struct Name`, …).
pub(crate) fn defined(u: &Unit) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (_, l) in &u.lines {
        let ts = idents(l);
        for w in ts.windows(2) {
            if DEFINES.contains(&w[0]) && !DEFINES.contains(&w[1]) {
                out.insert(w[1].to_string());
            }
        }
    }
    out
}

/// The names a unit's changed lines use, four characters or more.
pub(crate) fn used(u: &Unit) -> BTreeSet<String> {
    u.lines
        .iter()
        .flat_map(|(_, l)| idents(l))
        .filter(|t| t.len() >= 4 && !DEFINES.contains(t))
        .map(str::to_string)
        .collect()
}

/// `uses`: B uses a name A defines; `tests`: the same, where B is test code.
/// Hints only: they order and group, they decide nothing.
pub fn hints(units: &[Unit]) -> Vec<Edge> {
    let defs: Vec<BTreeSet<String>> = units.iter().map(defined).collect();
    let uses: Vec<BTreeSet<String>> = units.iter().map(used).collect();
    let mut defined_in: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, d) in defs.iter().enumerate() {
        for n in d {
            defined_in.entry(n.as_str()).or_default().push(i);
        }
    }
    let mut users: BTreeMap<&str, usize> = BTreeMap::new();
    for u in &uses {
        for n in u {
            *users.entry(n.as_str()).or_default() += 1;
        }
    }
    let mut out = Vec::new();
    for (n, at) in &defined_in {
        if at.len() > DEFINED_AT_MOST || users.get(n).copied().unwrap_or(0) > USED_AT_MOST {
            continue;
        }
        for (j, u) in uses.iter().enumerate() {
            if u.contains(*n) && !defs[j].contains(*n) {
                for &i in at {
                    if i != j {
                        let kind = if units[j].class == "test" { "tests" } else { "uses" };
                        out.push(Edge {
                            kind,
                            from: units[i].key.clone(),
                            to: units[j].key.clone(),
                            via: (*n).to_string(),
                        });
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| (a.kind, &a.from, &a.to, &a.via).cmp(&(b.kind, &b.from, &b.to, &b.via)));
    out.dedup();
    out
}

// ---- requirements -----------------------------------------------------------

/// A requirement: a step ("Done when" row) of a document part an Ask names,
/// or a sentence of an Ask's own text.
#[derive(Clone, Debug)]
pub struct Requirement {
    pub id: String,
    /// `local`, `cross`, `process` or `scope` (spec §4).
    pub kind: &'static str,
    pub ask: String,
    /// The document part (`<doc>#<part>`), or the Ask for its own sentences.
    pub part: String,
    pub text: String,
    pub done_when: String,
    /// The files and folders it names, lower case, without a leading `../`.
    pub paths: BTreeSet<String>,
    /// The commits its "As built" note names (a pointer, never evidence).
    pub claims: Vec<String>,
    names: BTreeSet<String>,
    hint: BTreeSet<String>,
    aliases: BTreeSet<String>,
}

impl Requirement {
    pub fn to_json(&self) -> Value {
        json!({"id": self.id, "kind": self.kind, "ask": self.ask, "part": self.part,
               "text": self.text, "done_when": self.done_when,
               "paths": self.paths, "claims": self.claims})
    }
}

/// Ordinary words too common to link anything, four letters or more.
const STOP: &[&str] = &[
    "also", "after", "again", "against", "almost", "along", "already", "always", "among",
    "another", "around", "because", "been", "before", "being", "below", "between", "both",
    "cannot", "change", "changes", "changed", "could", "does", "doing", "done", "down", "each",
    "either", "else", "every", "first", "from", "further", "gets", "given", "goes", "have",
    "having", "here", "into", "itself", "just", "keep", "kept", "know", "last", "least", "less",
    "like", "made", "make", "makes", "many", "more", "most", "much", "must", "need", "needs",
    "never", "next", "none", "nothing", "once", "only", "other", "over", "same", "says", "should",
    "since", "some", "still", "such", "than", "that", "their", "them", "then", "there", "these",
    "they", "thing", "this", "those", "though", "through", "under", "until", "upon", "used",
    "uses", "using", "very",
];
/// Words a plan uses about itself, not about the code.
const PLAN_WORDS: &[&str] = &[
    "step", "steps", "test", "tests", "pass", "passes", "phase", "plan", "built", "line", "lines",
    "note", "notes", "file",
];
const PATH_ENDS: [&str; 11] =
    [".rs", ".md", ".toml", ".sh", ".py", ".json", ".ts", ".js", ".mjs", ".snap", ".yml"];

/// The names a text links by, and which of them are code (snake_case,
/// CamelCase, a single backticked name, a path's stem): what `requirements`
/// learns and `links` reads.
#[derive(Clone, Debug, Default)]
pub struct Names {
    code: BTreeSet<String>,
}

impl Names {
    fn of(&mut self, text: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        // Backticked: a single name is code; a phrase also gives its snake_case.
        let mut rest = text;
        while let Some(i) = rest.find('`') {
            let Some(j) = rest[i + 1..].find('`') else { break };
            let tick = &rest[i + 1..i + 1 + j];
            rest = &rest[i + 2 + j..];
            let t = tick.trim();
            if t.contains(' ') {
                let snake = t.split_whitespace().collect::<Vec<_>>().join("_").to_lowercase();
                if snake.len() <= 60 {
                    out.insert(snake.clone());
                    self.code.insert(snake);
                }
            } else if (3..=60).contains(&t.len()) {
                let name = t.rsplit("::").next().unwrap_or(t).split('.').next().unwrap_or(t);
                let name = name.trim_end_matches("()").to_lowercase();
                if name.len() >= 3 && idents(&name).len() == 1 {
                    self.code.insert(name);
                }
            }
        }
        for tok in text.split(|c: char| {
            !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '/' || c == '-')
        }) {
            for piece in tok.split('/') {
                let base = piece.split('.').next().unwrap_or(piece);
                if PATH_ENDS.iter().any(|e| piece.ends_with(e)) && !base.is_empty() {
                    let stem = base.to_lowercase();
                    out.insert(stem.clone());
                    self.code.insert(stem);
                }
            }
            for id in idents(tok) {
                let camel = id
                    .chars()
                    .zip(id.chars().skip(1))
                    .any(|(a, b)| a.is_ascii_lowercase() && b.is_ascii_uppercase());
                if id.contains('_') || camel {
                    self.code.insert(id.to_lowercase());
                }
                let mut parts = vec![id.to_string()];
                parts.extend(split_camel(id));
                for t in parts {
                    let t = t.to_lowercase();
                    if t.len() >= 4
                        && !STOP.contains(&t.as_str())
                        && !PLAN_WORDS.contains(&t.as_str())
                    {
                        out.insert(t);
                    }
                }
            }
        }
        out
    }

    fn is_code(&self, t: &str) -> bool {
        t.contains('_') || self.code.contains(t)
    }
}

fn split_camel(id: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in id.chars() {
        if c == '_' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else if c.is_ascii_uppercase()
            && cur.chars().last().is_some_and(|p| p.is_ascii_lowercase())
        {
            out.push(std::mem::take(&mut cur));
            cur.push(c);
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The files and folders a text names: a token with a `/`, or one ending in a
/// known extension; lower case, without a leading `../` or a trailing `/`.
pub(crate) fn named_paths(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for tok in text.split(|c: char| c.is_whitespace() || "`()[],;:\"'*<>".contains(c)) {
        let t = tok.trim_end_matches(['.', '/']).trim_start_matches("./");
        let t = t.strip_prefix("../").unwrap_or(t);
        let word = t.chars().any(|c| c.is_ascii_alphabetic());
        let pathy =
            t.contains('/') && !t.contains("//") && !t.starts_with('/') && !t.starts_with('~');
        if word && (pathy || PATH_ENDS.iter().any(|e| t.ends_with(e))) && !t.starts_with("http") {
            out.insert(t.to_lowercase());
        }
    }
    out
}

fn kind_of(sentence: &str) -> &'static str {
    let s = sentence.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| s.contains(w));
    if s.starts_with("repositories:")
        || any(&[
            "work spans",
            "terminal condition",
            "priority order",
            "read the phase",
            "read them,",
            "first and follow",
            "do not restate",
        ])
    {
        "scope"
    } else if any(&[
        "commit",
        "push",
        "co-author",
        "signature",
        "as built",
        "mark each",
        "marked built",
        "measure",
        " w5 ",
        "pause for",
        "ask the owner",
        "in the plan instead",
        "say so in the plan",
        "record the ",
        "record a ",
    ]) {
        "process"
    } else if any(&[
        "every ",
        "never ",
        " no ",
        "nothing ",
        "keep ",
        "do not ",
        "don't ",
        "only ",
        "without ",
        "each one ",
        "one home",
    ]) {
        "cross"
    } else {
        "local"
    }
}

/// An Ask's own sentences, less the one that names what to build.
fn sentences(prompt: &str) -> Vec<String> {
    let flat = prompt.replace('\n', " ");
    let chars: Vec<char> = flat.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, c) in chars.iter().enumerate() {
        cur.push(*c);
        let next = chars.get(i + 1).copied().unwrap_or(' ');
        let after = chars[i + 1..].iter().find(|c| !c.is_whitespace()).copied();
        if matches!(c, '.' | ';')
            && next.is_whitespace()
            && after.is_some_and(|a| a.is_ascii_uppercase() || "`\"(".contains(a))
        {
            out.push(std::mem::take(&mut cur));
        }
    }
    out.push(cur);
    out.into_iter()
        .map(|s| s.trim().trim_start_matches("/goal").trim().to_string())
        .filter(|s| s.len() >= 25)
        .filter(|s| {
            let low = s.to_lowercase();
            !(low.starts_with("build phase")
                || low.starts_with("build slice")
                || low.starts_with("implement phase")
                || low.starts_with("implement slice"))
        })
        .collect()
}

/// The requirements of the Asks an Eval judges: each step of the parts they
/// name, and their own sentences. An Ask that only plans gives none.
pub fn requirements(
    asks: &[Value],
    prompts: &[(String, String)],
    obligations: &[crate::documents::Obligation],
    history: &History,
) -> (Vec<Requirement>, Names) {
    let planning: BTreeSet<String> = asks
        .iter()
        .filter(|a| a["capability"] == "native_plan")
        .map(|a| a["ask_id"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut names = Names::default();
    // Which harness a step names, and the folders its files are in: from the
    // installed profiles, never from this code.
    let harnesses = crate::profiles::aliases();
    let mut out = Vec::new();
    for o in obligations.iter().filter(|o| !planning.contains(&o.ask_id)) {
        let text = format!("{} {}", o.text, o.done_when);
        let low = text.to_lowercase();
        let claims = history
            .commits
            .iter()
            .filter(|c| {
                o.as_built
                    .split(|ch: char| !ch.is_ascii_hexdigit())
                    .any(|h| h.len() >= 7 && c.hash.starts_with(h))
            })
            .map(|c| c.hash.clone())
            .collect();
        out.push(Requirement {
            id: o.id.clone(),
            kind: "local",
            ask: o.ask_id.clone(),
            part: o.id.rsplit_once('/').map_or(o.id.clone(), |(p, _)| p.to_string()),
            text: o.text.clone(),
            done_when: o.done_when.clone(),
            paths: named_paths(&text),
            claims,
            names: names.of(&text),
            hint: names.of(&o.as_built),
            aliases: harnesses
                .iter()
                .filter(|(t, _)| low.contains(t.as_str()))
                .flat_map(|(_, p)| p.clone())
                .collect(),
        });
    }
    for (ask, prompt) in prompts.iter().filter(|(a, _)| !planning.contains(a)) {
        for (n, s) in sentences(prompt).into_iter().enumerate() {
            let low = s.to_lowercase();
            out.push(Requirement {
                id: format!("{ask}/s{n}"),
                kind: kind_of(&s),
                ask: ask.clone(),
                part: ask.clone(),
                paths: named_paths(&s),
                claims: Vec::new(),
                names: names.of(&s),
                hint: BTreeSet::new(),
                aliases: harnesses
                    .iter()
                    .filter(|(t, _)| low.contains(t.as_str()))
                    .flat_map(|(_, p)| p.clone())
                    .collect(),
                text: s,
                done_when: String::new(),
            });
        }
    }
    (out, names)
}

// ---- links ----------------------------------------------------------------------

/// A link between a requirement and a unit, with its score and what made it.
#[derive(Clone, Debug)]
pub struct Link {
    pub requirement: String,
    pub unit: String,
    pub score: f64,
    pub via: Vec<&'static str>,
}

impl Link {
    pub fn to_json(&self) -> Value {
        json!({"requirement": self.requirement, "unit": self.unit,
               "score": (self.score * 10.0).round() / 10.0, "via": self.via})
    }
}

/// What links requirements to units: each unit's home, each requirement's
/// evidence.
#[derive(Clone, Debug, Default)]
pub struct Links {
    pub homes: Vec<Link>,
    pub evidence: BTreeMap<String, Vec<Link>>,
}

/// The least score that links: what one name found in a single unit gives,
/// `ln((N+1)/2) + 1` for N units (about 6 on a change of 260 windows).
pub fn link_threshold(units: usize) -> f64 {
    ((units as f64 + 1.0) / 2.0).ln() + 1.0
}
/// How many units a requirement keeps as its evidence.
pub const EVIDENCE: usize = 4;
const K1: f64 = 1.2;
const B: f64 = 0.75;
/// A name in more of the units than this share links nothing.
const HUB: f64 = 0.35;

pub(crate) fn names_path(paths: &BTreeSet<String>, path: &str) -> bool {
    let path = path.to_lowercase();
    let path = path.strip_prefix("../").unwrap_or(&path);
    paths.iter().any(|p| {
        path == p
            || path.ends_with(&format!("/{p}"))
            || (p.contains('/')
                && (path.starts_with(&format!("{p}/"))
                    || format!("/{path}").contains(&format!("/{p}/"))))
    })
}

/// Score every local requirement against every unit (spec §4).
pub fn links(reqs: &[Requirement], names: &Names, units: &[Unit], history: &History) -> Links {
    let mut names = names.clone();
    let unit_names: Vec<BTreeSet<String>> = units
        .iter()
        .map(|u| {
            let text: String = u.lines.iter().map(|(_, l)| format!("{l}\n")).collect();
            let mut n = names.of(&text);
            n.extend(names.of(&u.path.replace('/', " ")));
            n
        })
        .collect();
    let messages: BTreeMap<&str, BTreeSet<String>> =
        history.commits.iter().map(|c| (c.hash.as_str(), names.of(&c.subject))).collect();
    let n = units.len().max(1) as f64;
    let mut df: BTreeMap<&str, usize> = BTreeMap::new();
    for ns in &unit_names {
        for t in ns {
            *df.entry(t.as_str()).or_default() += 1;
        }
    }
    let idf = |t: &str| -> f64 {
        let d = df.get(t).copied().unwrap_or(0) as f64;
        if d > HUB * n { 0.0 } else { ((n + 1.0) / (d + 1.0)).ln() + 1.0 }
    };
    let weight = |t: &str| idf(t) * if names.is_code(t) { 1.0 } else { 0.25 };
    let local: Vec<&Requirement> = reqs.iter().filter(|r| r.kind == "local").collect();
    let avg = local.iter().map(|r| r.names.len()).sum::<usize>() as f64 / local.len().max(1) as f64;
    let mut all: Vec<Link> = Vec::new();
    for (i, u) in units.iter().enumerate() {
        if u.class == "mechanical" {
            continue;
        }
        let parts: BTreeSet<String> =
            u.path.to_lowercase().split('/').map(str::to_string).collect();
        for r in &local {
            let shared: Vec<&String> = unit_names[i].intersection(&r.names).collect();
            let named = names_path(&r.paths, &u.path);
            let alias = r.aliases.iter().any(|a| parts.contains(a));
            if !(named || alias || shared.iter().any(|t| names.is_code(t) && idf(t) > 0.0)) {
                continue;
            }
            let norm = K1 * (1.0 - B + B * r.names.len() as f64 / avg.max(1.0));
            let mut score: f64 = shared.iter().map(|t| weight(t) * (K1 + 1.0) / (1.0 + norm)).sum();
            let mut via = vec![];
            if !shared.is_empty() {
                via.push("names");
            }
            if named {
                score += 15.0;
                via.push("path");
            }
            if alias {
                score += 8.0;
                via.push("alias");
            }
            let hinted: f64 = unit_names[i].intersection(&r.hint).map(|t| weight(t)).sum();
            if hinted > 0.0 {
                score += 0.3 * hinted;
                via.push("as_built");
            }
            if let Some(words) = u.commits.first().and_then(|c| messages.get(c.as_str())) {
                let said: f64 = words.intersection(&r.names).map(|t| weight(t)).sum();
                if said > 0.0 {
                    score += 0.5 * said;
                    via.push("commit");
                }
            }
            all.push(Link { requirement: r.id.clone(), unit: u.key.clone(), score, via });
        }
    }
    let mut out = Links::default();
    let mut best: BTreeMap<&str, &Link> = BTreeMap::new();
    for l in &all {
        let e = best.entry(l.unit.as_str()).or_insert(l);
        if l.score > e.score || (l.score == e.score && l.requirement < e.requirement) {
            *e = l;
        }
    }
    let link = link_threshold(units.len());
    out.homes = best.values().filter(|l| l.score >= link).map(|l| (*l).clone()).collect();
    for r in &local {
        let mut mine: Vec<&Link> =
            all.iter().filter(|l| l.requirement == r.id && l.score >= link).collect();
        mine.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.unit.cmp(&b.unit)));
        out.evidence.insert(r.id.clone(), mine.into_iter().take(EVIDENCE).cloned().collect());
    }
    out
}

// ---- findings -------------------------------------------------------------------

/// Something RingFrame found without a model, which a judge has to answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// `untouched_path`, `no_link`, `unclaimed_rewrite`, `scaffolding`,
    /// `unlinked` or `test_integrity`.
    pub kind: &'static str,
    pub requirements: Vec<String>,
    pub units: Vec<String>,
    /// The windows it concerns, when narrower than its units (an unclaimed
    /// rewrite: the windows whose lines the unclaimed commit wrote).
    pub windows: Vec<String>,
    pub detail: String,
}

impl Finding {
    pub fn to_json(&self) -> Value {
        json!({"kind": self.kind, "requirements": self.requirements, "units": self.units,
               "windows": self.windows, "detail": self.detail})
    }
}

fn scaffolding_in(u: &Unit) -> Vec<String> {
    let path = u.path.to_lowercase();
    let prints_ok =
        path.ends_with("main.rs") || path.contains("examples/") || path.contains("tests/");
    let mut out = Vec::new();
    let mut commented = 0;
    for (tag, l) in &u.lines {
        if *tag != b'+' {
            continue;
        }
        let t = l.trim();
        let comment = t.strip_prefix("//").or_else(|| t.strip_prefix('#')).map(str::trim);
        if let Some(c) = comment
            && (c.ends_with(';')
                || c.ends_with('{')
                || c.ends_with('}')
                || (c.contains('(') && c.ends_with(')')))
        {
            commented += 1;
            if commented == 3 {
                out.push(format!("commented-out code: `{t}`"));
            }
        } else {
            commented = 0;
        }
        let marker = ["TODO", "FIXME", "XXX"]
            .iter()
            .find(|m| t.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == **m));
        if let Some(m) = marker {
            out.push(format!("{m}: `{t}`"));
        } else if t.contains("dbg!(")
            || t.contains("console.log(")
            || t.starts_with("print(")
            || (t.contains("println!(") && !prints_ok)
        {
            out.push(format!("debug output: `{t}`"));
        }
    }
    out
}

/// The findings of a view (spec §5).
pub fn findings(
    ev: &Evidence,
    units: &[Unit],
    reqs: &[Requirement],
    links: &Links,
    history: &History,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let local: Vec<&Requirement> = reqs.iter().filter(|r| r.kind == "local").collect();
    // A file's old name counts as touched: a split, a move out of it or a
    // rename changed it, even when its lines now live elsewhere.
    let touched: Vec<&str> = units
        .iter()
        .map(|u| u.path.as_str())
        .chain(units.iter().flat_map(|u| u.also.iter().map(|k| k.split('#').next().unwrap_or(k))))
        .chain(ev.files.iter().map(|f| f.path.as_str()))
        .chain(ev.files.iter().filter_map(|f| f.renamed_from.as_deref()))
        .collect();
    for r in &local {
        let untouched: Vec<&String> = r
            .paths
            .iter()
            .filter(|p| !touched.iter().any(|t| names_path(&BTreeSet::from([(*p).clone()]), t)))
            .collect();
        if !r.paths.is_empty() && untouched.len() == r.paths.len() {
            out.push(Finding {
                kind: "untouched_path",
                windows: vec![],
                requirements: vec![r.id.clone()],
                units: vec![],
                detail: format!(
                    "names {}, which the change never touches",
                    untouched.iter().map(|p| format!("`{p}`")).collect::<Vec<_>>().join(", ")
                ),
            });
        }
        if links.evidence.get(&r.id).is_none_or(Vec::is_empty) {
            out.push(Finding {
                kind: "no_link",
                windows: vec![],
                requirements: vec![r.id.clone()],
                units: vec![],
                detail: "no unit of the change shares a code name with it or is in a path it names"
                    .into(),
            });
        }
    }
    let claimed_by: BTreeMap<&str, Vec<&str>> = reqs.iter().fold(BTreeMap::new(), |mut m, r| {
        for c in &r.claims {
            m.entry(c.as_str()).or_insert_with(Vec::new).push(r.id.as_str());
        }
        m
    });
    // One finding per commit no step claims, with every claimed commit it
    // rewrote and the steps that claim them.
    // Per unclaimed commit: the claimed commits it rewrote, the steps that
    // claim them, and the files it rewrote them in.
    type Rewrote<'a> = (BTreeSet<&'a str>, BTreeSet<&'a str>, BTreeSet<&'a str>);
    let mut unclaimed: BTreeMap<&str, Rewrote> = BTreeMap::new();
    for (b, a) in &history.rewrites {
        if claimed_by.contains_key(b.as_str()) {
            continue;
        }
        if let Some(steps) = claimed_by.get(a.as_str()) {
            let e = unclaimed.entry(b.as_str()).or_default();
            e.0.insert(a.as_str());
            e.1.extend(steps.iter().copied());
            let key = (b.clone(), a.clone());
            e.2.extend(history.rewritten_in.get(&key).into_iter().flatten().map(String::as_str));
        }
    }
    let short = |h: &str| h.chars().take(7).collect::<String>();
    for (b, (rewritten, steps, files)) in &unclaimed {
        let removed: Vec<String> = rewritten
            .iter()
            .flat_map(|a| {
                history.removed.get(&(b.to_string(), a.to_string())).into_iter().flatten()
            })
            .take(REMOVED_SHOWN)
            .map(|(path, line)| format!("`{line}` ({path})"))
            .collect();
        let subject =
            history.commits.iter().find(|c| c.hash == *b).map_or("", |c| c.subject.as_str());
        out.push(Finding {
            kind: "unclaimed_rewrite",
            requirements: steps.iter().map(|s| s.to_string()).collect(),
            // Where it rewrote them: its units in the files it removed claimed
            // lines from (or moved them out of), not every unit it touched.
            units: units
                .iter()
                .filter(|u| {
                    u.commits.iter().any(|c| c == b)
                        && (files.contains(u.path.as_str())
                            || u.also.iter().any(|o| files.contains(o.split('#').next().unwrap_or(o))))
                })
                .map(|u| u.key.clone())
                .collect(),
            // And exactly where: the windows whose lines it wrote there.
            windows: ev
                .windows
                .iter()
                .filter(|w| {
                    files.contains(w.path.as_str())
                        && history.window_commits.get(&w.id).is_some_and(|cs| cs.contains(*b))
                })
                .map(|w| w.id.clone())
                .collect(),
            detail: format!(
                "commit {} (\"{subject}\"), which no step claims, rewrites lines of {}, which these steps \
                 claim; it removes {}",
                short(b),
                rewritten.iter().map(|a| short(a)).collect::<Vec<_>>().join(", "),
                removed.join(", ")
            ),
        });
    }
    let added: BTreeSet<&str> =
        ev.files.iter().filter(|f| f.status == "added").map(|f| f.path.as_str()).collect();
    for u in units.iter().filter(|u| u.class != "mechanical") {
        let mut found = scaffolding_in(u);
        let name = u.path.rsplit('/').next().unwrap_or(&u.path).to_lowercase();
        if added.contains(u.path.as_str())
            && ["tmp", "scratch", "debug", "probe"].iter().any(|w| name.contains(w))
        {
            found.push(format!("a new file named like a probe: `{name}`"));
        }
        if !found.is_empty() {
            out.push(Finding {
                kind: "scaffolding",
                windows: vec![],
                requirements: vec![],
                units: vec![u.key.clone()],
                detail: found.join("; "),
            });
        }
    }
    let homed: BTreeSet<&str> = links.homes.iter().map(|l| l.unit.as_str()).collect();
    for u in units.iter().filter(|u| u.class != "mechanical" && !homed.contains(u.key.as_str())) {
        out.push(Finding {
            kind: "unlinked",
            windows: vec![],
            requirements: vec![],
            units: vec![u.key.clone()],
            detail: "no requirement shares a code name with it or names its path".into(),
        });
    }
    for f in &ev.integrity {
        let w = f["window"].as_str().unwrap_or_default();
        out.push(Finding {
            kind: "test_integrity",
            windows: vec![],
            requirements: vec![],
            units: units
                .iter()
                .filter(|u| u.windows.iter().any(|x| x == w))
                .map(|u| u.key.clone())
                .collect(),
            detail: format!(
                "{}: `{}` ({}:{})",
                f["kind"].as_str().unwrap_or_default(),
                f["text"].as_str().unwrap_or_default(),
                f["path"].as_str().unwrap_or_default(),
                f["line"]
            ),
        });
    }
    out
}

// ---- the view ---------------------------------------------------------------------

const VIEW_SCHEMA: &str = "ringframe.eval-changes/1";

/// The latest earlier view of any of these Asks, if one was published.
fn previous_view(ws: &Workspace, asks: &BTreeSet<String>) -> Option<(String, Value)> {
    let events = store::events(ws).ok()?;
    events.iter().rev().find_map(|e| {
        if e["type"] != "eval.opened" {
            return None;
        }
        let basis: BTreeSet<String> = e["data"]["basis"]["asks"]
            .as_array()?
            .iter()
            .filter_map(|a| a.as_str().map(str::to_string))
            .collect();
        let path = e["data"]["changes"]["path"].as_str()?;
        if basis.is_disjoint(asks) {
            return None;
        }
        let doc: Value =
            serde_json::from_slice(&std::fs::read(ws.rf_dir().join(path)).ok()?).ok()?;
        Some((e["id"].as_str()?.to_string(), doc))
    })
}

/// What an earlier view already judged: units unchanged, units changed
/// again, and the requirements they were evidence for (spec §5).
fn reuse(previous: Option<&(String, Value)>, units: &[Unit]) -> Value {
    let Some((id, doc)) = previous else {
        return json!({"previous": null, "unchanged": [], "changed_again": [], "stale_requirements": []});
    };
    let before: BTreeMap<&str, &str> = doc["units"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| Some((u["key"].as_str()?, u["hash"].as_str()?)))
        .collect();
    let mut unchanged = Vec::new();
    let mut again = BTreeSet::new();
    for u in units {
        match before.get(u.key.as_str()) {
            Some(h) if *h == u.hash => unchanged.push(u.key.clone()),
            Some(_) => {
                again.insert(u.key.clone());
            }
            None => {}
        }
    }
    let mut stale = BTreeSet::new();
    for l in doc["links"]["homes"].as_array().into_iter().flatten() {
        if again.contains(l["unit"].as_str().unwrap_or_default()) {
            stale.insert(l["requirement"].as_str().unwrap_or_default().to_string());
        }
    }
    for (req, ls) in doc["links"]["evidence"].as_object().into_iter().flatten() {
        if ls
            .as_array()
            .into_iter()
            .flatten()
            .any(|l| again.contains(l["unit"].as_str().unwrap_or_default()))
        {
            stale.insert(req.clone());
        }
    }
    json!({"previous": id, "unchanged": unchanged, "changed_again": again, "stale_requirements": stale})
}

/// Build the change DAG of a prepared change and publish the Eval's view of
/// it as `evals/<id>/changes.json`. Returns a summary for the brief and the
/// reference the opened event carries.
pub fn view(
    ws: &Workspace,
    eval_id: &str,
    ev: &Evidence,
    asks: &[Value],
    prompts: &[(String, String)],
    obligations: &[crate::documents::Obligation],
) -> Result<(Value, Value), EvalError> {
    let mut us = units(ev);
    let h = history(ev, 0, &mut us)?;
    let edges = hints(&us);
    let (reqs, names) = requirements(asks, prompts, obligations, &h);
    let ls = links(&reqs, &names, &us, &h);
    let fs = findings(ev, &us, &reqs, &ls, &h);
    let ask_ids: BTreeSet<String> =
        asks.iter().filter_map(|a| a["ask_id"].as_str().map(str::to_string)).collect();
    let previous = previous_view(ws, &ask_ids);
    let reused = reuse(previous.as_ref(), &us);
    let claimed_by = |hash: &str| -> Vec<&str> {
        reqs.iter().filter(|r| r.claims.iter().any(|c| c == hash)).map(|r| r.id.as_str()).collect()
    };
    let mut all_edges: Vec<Value> = edges.iter().map(Edge::to_json).collect();
    all_edges.extend(
        h.rewrites.iter().map(|(b, a)| json!({"kind": "rewrites", "from": b, "to": a, "via": ""})),
    );
    let doc = json!({
        "schema": VIEW_SCHEMA,
        "eval_id": eval_id,
        "repos": ev.repos.iter().map(|r| json!({"name": r.name, "anchor": r.anchor,
            "subject": {"kind": r.subject_kind, "ref": r.subject_ref}})).collect::<Vec<_>>(),
        "units": us.iter().map(Unit::to_json).collect::<Vec<_>>(),
        "commits": h.commits.iter().map(|c| json!({"repo": c.repo, "hash": c.hash, "order": c.order,
            "subject": c.subject, "claimed_by": claimed_by(&c.hash)})).collect::<Vec<_>>(),
        "edges": all_edges,
        "requirements": reqs.iter().map(Requirement::to_json).collect::<Vec<_>>(),
        "links": {
            "homes": ls.homes.iter().map(Link::to_json).collect::<Vec<_>>(),
            "evidence": ls.evidence.iter().map(|(k, v)| (k.clone(), Value::Array(v.iter().map(Link::to_json).collect())))
                .collect::<serde_json::Map<_, _>>(),
        },
        "findings": fs.iter().map(Finding::to_json).collect::<Vec<_>>(),
        "reuse": reused,
    });
    let mut bytes = store::canonical(&doc);
    bytes.push(b'\n');
    let reference =
        store::publish(ws, &format!("evals/{eval_id}/changes.json"), &bytes, "eval_changes")?;
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &fs {
        *by_kind.entry(f.kind).or_default() += 1;
    }
    let summary = json!({
        "units": us.len(), "commits": h.commits.len(), "requirements": reqs.len(),
        "homed": ls.homes.len(), "findings": by_kind,
        "reuse": {"unchanged": reused["unchanged"].as_array().map_or(0, Vec::len),
                  "changed_again": reused["changed_again"].as_array().map_or(0, Vec::len),
                  "stale_requirements": reused["stale_requirements"].as_array().map_or(0, Vec::len)},
        "changes": reference,
    });
    Ok((summary, reference))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::{Repo, gather};
    use crate::testing::{commit, repo};

    fn units_between(root: &std::path::Path, anchor: &str, subject: &str) -> Vec<Unit> {
        let r = Repo {
            name: ".".into(),
            root: root.to_path_buf(),
            anchor: anchor.into(),
            anchor_from: "workspace",
            subject_kind: "git_commit".into(),
            subject_ref: subject.into(),
        };
        units(&gather(&[r], 0).unwrap())
    }

    fn history_between(
        root: &std::path::Path,
        anchor: &str,
        subject: &str,
    ) -> (Vec<Unit>, History) {
        let r = Repo {
            name: ".".into(),
            root: root.to_path_buf(),
            anchor: anchor.into(),
            anchor_from: "workspace",
            subject_kind: "git_commit".into(),
            subject_ref: subject.into(),
        };
        let ev = gather(&[r], 0).unwrap();
        let mut us = units(&ev);
        let h = history(&ev, 0, &mut us).unwrap();
        (us, h)
    }

    #[test]
    fn a_history_git_cannot_read_fails_instead_of_leaving_the_repository_out() {
        let dir = repo();
        let a = commit(dir.path(), &[("src/lib.rs", Some(LIB))], "a");
        let b =
            commit(dir.path(), &[("src/lib.rs", Some(&format!("{LIB}\nfn more() {{}}\n")))], "b");
        let r = Repo {
            name: ".".into(),
            root: dir.path().to_path_buf(),
            anchor: a,
            anchor_from: "workspace",
            subject_kind: "git_commit".into(),
            subject_ref: b,
        };
        let mut ev = gather(&[r], 0).unwrap();
        let mut us = units(&ev);
        ev.repos[0].anchor = "0".repeat(40);
        assert!(
            history(&ev, 0, &mut us).is_err(),
            "no commits read is no history, not an empty one"
        );
    }

    #[test]
    fn a_function_added_and_called_elsewhere_is_a_uses_edge_and_a_test_calling_it_tests() {
        let dir = repo();
        let a = commit(
            dir.path(),
            &[
                ("src/lib.rs", Some(LIB)),
                ("src/main.rs", Some("fn main() {\n    run();\n}\n")),
                ("tests/it.rs", Some("#[test]\nfn works() {\n    assert!(true);\n}\n")),
            ],
            "a",
        );
        let b = commit(
            dir.path(),
            &[
                (
                    "src/lib.rs",
                    Some(&format!(
                        "{LIB}\npub fn tell_the_person(n: u32) -> u32 {{\n    n + 1\n}}\n"
                    )),
                ),
                ("src/main.rs", Some("fn main() {\n    run();\n    tell_the_person(2);\n}\n")),
                (
                    "tests/it.rs",
                    Some("#[test]\nfn works() {\n    assert_eq!(lib::tell_the_person(1), 2);\n}\n"),
                ),
            ],
            "b",
        );
        let us = units_between(dir.path(), &a, &b);
        let es = hints(&us);
        let got: Vec<(&str, &str, &str)> =
            es.iter().map(|e| (e.kind, e.to.split('#').next().unwrap(), e.via.as_str())).collect();
        assert!(got.contains(&("uses", "src/main.rs", "tell_the_person")), "{got:?}");
        assert!(got.contains(&("tests", "tests/it.rs", "tell_the_person")), "{got:?}");
        assert!(es.iter().all(|e| e.from.starts_with("src/lib.rs#")), "{es:?}");
    }

    fn ob(id: &str, ask: &str, text: &str, done: &str) -> crate::documents::Obligation {
        crate::documents::Obligation {
            id: id.into(),
            document: "d_x".into(),
            part: "Phase 1".into(),
            row: id.rsplit('/').next().unwrap().into(),
            text: text.into(),
            done_when: done.into(),
            line: 1,
            ask_id: ask.into(),
            as_built: String::new(),
        }
    }

    fn unit(path: &str, lines: &[&str]) -> Unit {
        Unit {
            key: format!("{path}#<top>"),
            repo: ".".into(),
            path: path.into(),
            item: "<top>".into(),
            hash: String::new(),
            also: vec![],
            windows: vec![],
            bytes: 100,
            class: "code".into(),
            lines: lines.iter().map(|l| (b'+', l.to_string())).collect(),
            added_at: vec![],
            commits: vec![],
        }
    }

    #[test]
    fn a_planning_ask_gives_no_requirement_and_sentences_get_their_kinds() {
        let asks = vec![
            json!({"ask_id": "ask_plan", "capability": "native_plan"}),
            json!({"ask_id": "ask_goal", "capability": "native_goal"}),
        ];
        let prompts = vec![
            (
                "ask_plan".to_string(),
                "Turn the report into a detailed plan for the whole project.".to_string(),
            ),
            (
                "ask_goal".to_string(),
                "/goal Build Phase 1 of plans/app/plan.md: steps 1 to 3. \
                 Commit as the owner with no co-author, messages describing the change only. \
                 Repositories: this workspace and ../fab7. \
                 Keep keys, words on screen and file locations as they are. \
                 The status bar shows the pane's harness and its model name."
                    .to_string(),
            ),
        ];
        let obs = vec![
            ob(
                "plans/app/plan.md#Phase 1/1",
                "ask_goal",
                "Add `uptime_seconds`.",
                "A test calls it.",
            ),
            ob("plans/other.md#Phase 9/1", "ask_plan", "Plan the thing.", "It is planned."),
        ];
        let (reqs, _) = requirements(&asks, &prompts, &obs, &History::default());
        assert!(reqs.iter().all(|r| r.ask == "ask_goal"), "{reqs:?}");
        let kind = |start: &str| reqs.iter().find(|r| r.text.starts_with(start)).map(|r| r.kind);
        assert_eq!(kind("Add `uptime_seconds`"), Some("local"));
        assert_eq!(kind("Commit as the owner"), Some("process"));
        assert_eq!(kind("Repositories:"), Some("scope"));
        assert_eq!(kind("Keep keys"), Some("cross"));
        assert_eq!(kind("The status bar"), Some("local"));
        assert!(kind("Build Phase 1").is_none(), "the scope sentence is not a requirement");
    }

    #[test]
    fn a_step_naming_a_file_links_to_it_and_a_long_step_does_not_win_on_length() {
        let obs = vec![
            ob(
                "p.md#Phase 1/1",
                "a",
                "**Big files split**: `app.rs` into input and notices.",
                "Each file is smaller.",
            ),
            ob(
                "p.md#Phase 1/2",
                "a",
                "Rename `render_status_bar` to `draw_status`.",
                "No `render_status_bar` left.",
            ),
            ob(
                "p.md#Phase 1/3",
                "a",
                "Rewrite the whole status area: `render_status_bar`, the colours, the spacing, the \
                 borders, the alignment, the truncation, the tooltips, the clock, the counters, the \
                 badges, the icons, the margins and the padding of every widget.",
                "Everything looks right.",
            ),
        ];
        let (reqs, names) = requirements(&[], &[], &obs, &History::default());
        // Enough unrelated units for names to be rare, as in real work.
        let filler: Vec<Unit> = (0..30)
            .map(|i| {
                unit(
                    &format!("crates/core/src/f{i}.rs"),
                    &[&format!("fn helper_{i}(value_{i}: u32) {{}}")],
                )
            })
            .collect();
        let mut units = vec![
            unit("crates/tui/src/app.rs", &["fn handle_input(key: Key) {}"]),
            unit(
                "crates/tui/src/bar.rs",
                &["pub fn draw_status(frame: &mut Frame) {", "    // was render_status_bar"],
            ),
            unit("crates/tui/src/other.rs", &["fn unrelated() {}", "fn more_unrelated() {}"]),
        ];
        units.extend(filler);
        let ls = links(&reqs, &names, &units, &History::default());
        let home = |path: &str| {
            ls.homes.iter().find(|l| l.unit.starts_with(path)).map(|l| l.requirement.as_str())
        };
        assert_eq!(home("crates/tui/src/app.rs"), Some("p.md#Phase 1/1"), "{:?}", ls.homes);
        assert_eq!(
            home("crates/tui/src/bar.rs"),
            Some("p.md#Phase 1/2"),
            "the short step, not the long one"
        );
        assert_eq!(home("crates/tui/src/other.rs"), None, "no shared code name, no link");
        assert!(ls.evidence["p.md#Phase 1/1"][0].via.contains(&"path"));
    }

    #[test]
    fn each_finding_on_a_fixture_made_for_it() {
        let dir = repo();
        let test_file = "#[cfg(test)]\nmod tests {\n    #[test]\n    fn counts() {\n        let n = 3;\n        \
                         assert_eq!(n, 3);\n        assert!(n > 0);\n    }\n}\n";
        let anchor = commit(
            dir.path(),
            &[("src/lib.rs", Some(LIB)), ("src/check.rs", Some(test_file))],
            "base",
        );
        let told = LIB.replace("    a + b\n", "    let told = tell_the_person(a);\n    a + b\n");
        let a = commit(dir.path(), &[("src/lib.rs", Some(&told))], "Tell the person");
        let b = commit(
            dir.path(),
            &[
                (
                    "src/lib.rs",
                    Some(&format!(
                        "{LIB}\npub fn stray() {{\n    dbg!(1);\n    // TODO remove\n}}\n"
                    )),
                ),
                ("src/check.rs", Some(&test_file.replace("        assert_eq!(n, 3);\n", ""))),
                ("scratch_notes.txt", Some("zzz\n")),
            ],
            "Follow-up work",
        );
        let ev = gather(
            &[Repo {
                name: ".".into(),
                root: dir.path().to_path_buf(),
                anchor: anchor.clone(),
                anchor_from: "workspace",
                subject_kind: "git_commit".into(),
                subject_ref: b.clone(),
            }],
            0,
        )
        .unwrap();
        let mut us = units(&ev);
        let h = history(&ev, 0, &mut us).unwrap();
        let mut told_step = ob(
            "p.md#Phase 1/1",
            "x",
            "A failed send tells the person: `tell_the_person`.",
            "A test.",
        );
        told_step.as_built = format!("Built in {}.", &a[..7]);
        let obs = vec![
            told_step,
            ob("p.md#Phase 1/2", "x", "Document it in `docs/guide.md`.", "The guide says so."),
            ob("p.md#Phase 1/3", "x", "Make the zebra gallop.", "It gallops."),
        ];
        let (reqs, names) = requirements(&[], &[], &obs, &h);
        let ls = links(&reqs, &names, &us, &h);
        let fs = findings(&ev, &us, &reqs, &ls, &h);
        let kinds = |k: &str| fs.iter().filter(|f| f.kind == k).collect::<Vec<_>>();
        assert!(
            kinds("untouched_path").iter().any(|f| f.requirements == ["p.md#Phase 1/2"]),
            "{fs:?}"
        );
        assert!(kinds("no_link").iter().any(|f| f.requirements == ["p.md#Phase 1/3"]), "{fs:?}");
        let rw = kinds("unclaimed_rewrite");
        assert_eq!(rw.len(), 1, "{fs:?}");
        assert_eq!(rw[0].requirements, ["p.md#Phase 1/1"]);
        assert!(rw[0].detail.contains("Follow-up work"), "{}", rw[0].detail);
        assert!(
            rw[0].detail.contains("it removes `let told = tell_the_person(a);` (src/lib.rs)"),
            "the finding names what it took away: {}",
            rw[0].detail
        );
        assert!(
            !rw[0].units.is_empty() && rw[0].units.iter().all(|u| u.starts_with("src/lib.rs#")),
            "only where it rewrote, not every file it touched: {:?}",
            rw[0].units
        );
        let lib_windows: Vec<&str> =
            ev.windows.iter().filter(|w| w.path == "src/lib.rs").map(|w| w.id.as_str()).collect();
        assert!(
            !rw[0].windows.is_empty()
                && rw[0].windows.iter().all(|w| lib_windows.contains(&w.as_str())),
            "exactly the windows it wrote: {:?}",
            rw[0].windows
        );
        let sc = kinds("scaffolding");
        assert!(
            sc.iter().any(|f| f.detail.contains("dbg!") && f.detail.contains("TODO")),
            "{sc:?}"
        );
        assert!(sc.iter().any(|f| f.detail.contains("scratch_notes.txt")), "{sc:?}");
        assert!(
            kinds("unlinked")
                .iter()
                .any(|f| f.units.iter().any(|u| u.starts_with("scratch_notes.txt"))),
            "{fs:?}"
        );
        let ti = kinds("test_integrity");
        assert!(
            ti.iter().any(|f| f.detail.contains("assert_eq!(n, 3)") && !f.units.is_empty()),
            "{ti:?}"
        );
    }

    #[test]
    fn a_second_eval_marks_what_it_already_judged_and_what_changed_again() {
        use crate::evaluate::{Open, open_eval};
        use crate::testing::{confirm_ask_as, eval_bench};
        let plan = "## Phase 1\n\n| # | Step | Done when |\n| --- | --- | --- |\n\
                    | 1 | Add `uptime_seconds` to the server. | A test calls `uptime_seconds`. |\n\
                    | 2 | Add `notes_line` for the notes. | `notes_line` returns a line. |\n";
        eval_bench(|ws| {
            commit(&ws.root, &[("plans/plan.md", Some(plan))], "plan");
            confirm_ask_as(
                ws,
                "Build it",
                b"build\n",
                b"/goal Build Phase 1 of plans/plan.md, steps 1 to 2.\n",
                "native_goal",
            );
            let uptime =
                "pub fn uptime_seconds() -> u64 {\n    let started = 1;\n    started * 60\n}\n";
            let notes = "pub fn notes_line() -> String {\n    let line = \"up\";\n    line.to_string()\n}\n";
            // Enough other code for names to be rare, as in real work.
            let mut files: Vec<(String, String)> = (0..30)
                .map(|i| {
                    (
                        format!("src/f{i}.rs"),
                        format!("pub fn helper_{i}(value_{i}: u32) -> u32 {{\n    value_{i}\n}}\n"),
                    )
                })
                .collect();
            files.push(("src/uptime.rs".into(), uptime.into()));
            files.push(("src/notes.rs".into(), notes.into()));
            let refs: Vec<(&str, Option<&str>)> =
                files.iter().map(|(p, t)| (p.as_str(), Some(t.as_str()))).collect();
            commit(&ws.root, &refs, "work");
            let first = open_eval(ws, Open::default()).unwrap();
            commit(
                &ws.root,
                &[("src/uptime.rs", Some(&uptime.replace("started * 60", "started * 3600")))],
                "more",
            );
            let second = open_eval(ws, Open::default()).unwrap();
            let e = second["eval_id"].as_str().unwrap();
            let doc: Value = serde_json::from_slice(
                &std::fs::read(ws.rf_dir().join(format!("evals/{e}/changes.json"))).unwrap(),
            )
            .unwrap();
            let r = &doc["reuse"];
            assert_eq!(r["previous"], first["eval_id"], "{r}");
            let has = |k: &str, prefix: &str| {
                r[k].as_array().unwrap().iter().any(|v| v.as_str().unwrap().starts_with(prefix))
            };
            assert!(has("unchanged", "src/notes.rs#"), "{r}");
            assert!(has("changed_again", "src/uptime.rs#"), "{r}");
            assert!(!has("changed_again", "src/notes.rs#"), "{r}");
            assert_eq!(r["stale_requirements"], json!(["plans/plan.md#Phase 1/1"]), "{r}");
            assert!(
                doc["units"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|u| u["key"].as_str().unwrap().contains("uptime_seconds"))
            );
        });
    }

    #[test]
    fn an_undo_back_to_the_text_before_the_range_has_no_window_but_its_finding_names_the_lines() {
        let dir = repo();
        let anchor = commit(dir.path(), &[("src/lib.rs", Some(LIB))], "base");
        let told = LIB.replace("    a + b\n", "    let told = tell_the_person(a);\n    a + b\n");
        let a = commit(
            dir.path(),
            &[("src/lib.rs", Some(&told)), ("docs/guide.md", Some("Tell the person.\n"))],
            "Tell the person",
        );
        let b = commit(dir.path(), &[("src/lib.rs", Some(LIB))], "Follow-up work");
        let (us, h) = history_between(dir.path(), &anchor, &b);
        let ev = gather(
            &[Repo {
                name: ".".into(),
                root: dir.path().to_path_buf(),
                anchor,
                anchor_from: "workspace",
                subject_kind: "git_commit".into(),
                subject_ref: b,
            }],
            0,
        )
        .unwrap();
        assert!(ev.windows.iter().all(|w| w.path != "src/lib.rs"), "the net change shows no undo");
        let mut step = ob("p.md#Phase 1/1", "x", "A failed send tells the person.", "A test.");
        step.as_built = format!("Built in {}.", &a[..7]);
        let (reqs, names) = requirements(&[], &[], &[step], &h);
        let ls = links(&reqs, &names, &us, &h);
        let fs = findings(&ev, &us, &reqs, &ls, &h);
        let rw: Vec<_> = fs.iter().filter(|f| f.kind == "unclaimed_rewrite").collect();
        assert_eq!(rw.len(), 1, "{fs:?}");
        assert!(rw[0].windows.is_empty());
        assert!(
            rw[0].detail.contains("`let told = tell_the_person(a);` (src/lib.rs)"),
            "{}",
            rw[0].detail
        );
    }

    #[test]
    fn a_commit_that_removes_a_line_another_added_rewrites_it_and_blame_gives_the_commits() {
        let dir = repo();
        let anchor = commit(dir.path(), &[("src/lib.rs", Some(LIB))], "base");
        let told = LIB.replace("    a + b\n", "    let told = tell_the_person(a);\n    a + b\n");
        let a = commit(dir.path(), &[("src/lib.rs", Some(&told))], "Tell the person");
        let b = commit(dir.path(), &[("src/lib.rs", Some(LIB))], "Follow-up work");
        let c = commit(
            dir.path(),
            &[("src/lib.rs", Some(&LIB.replace("c * d", "c * d * 2")))],
            "Double beta",
        );
        let (us, h) = history_between(dir.path(), &anchor, &c);
        assert_eq!(h.commits.len(), 3);
        assert_eq!(
            h.rewrites,
            [(b.clone(), a.clone())],
            "the follow-up removed what the first added"
        );
        let beta = us.iter().find(|u| u.item.contains("beta")).unwrap();
        assert_eq!(beta.commits, std::slice::from_ref(&c), "blame: the line c added");
    }

    const LIB: &str = "pub fn alpha() -> u32 {\n    let a = 1;\n    let b = 2;\n    a + b\n}\n\n\
        pub fn beta() -> u32 {\n    let c = 3;\n    let d = 4;\n    c * d\n}\n";

    #[test]
    fn a_hunk_in_a_rust_function_names_it_and_an_unknown_extension_is_top() {
        let dir = repo();
        let a = commit(
            dir.path(),
            &[("src/lib.rs", Some(LIB)), ("notes.xyz", Some("one\ntwo\nthree\n"))],
            "a",
        );
        let b = commit(
            dir.path(),
            &[
                ("src/lib.rs", Some(&LIB.replace("let b = 2;", "let b = 20;"))),
                ("notes.xyz", Some("one\n2\nthree\n")),
            ],
            "b",
        );
        let us = units_between(dir.path(), &a, &b);
        let keys: Vec<&str> = us.iter().map(|u| u.key.as_str()).collect();
        assert!(keys.contains(&"src/lib.rs#pub fn alpha() -> u32 {"), "{keys:?}");
        assert!(keys.contains(&"notes.xyz#<top>"), "{keys:?}");
    }

    #[test]
    fn a_toml_change_names_its_table() {
        let dir = repo();
        let before = "[roles]\nintent = 1\ncoverage = 2\n\n[eval]\nparallel = 8\n";
        let a = commit(dir.path(), &[("c.toml", Some(before))], "a");
        let b = commit(dir.path(), &[("c.toml", Some(&before.replace("= 8", "= 3")))], "b");
        let keys: Vec<String> =
            units_between(dir.path(), &a, &b).into_iter().map(|u| u.key).collect();
        assert_eq!(keys, ["c.toml#[eval]"]);
    }

    #[test]
    fn hunks_in_one_function_are_one_unit_and_in_two_functions_two() {
        let dir = repo();
        let long = format!(
            "pub fn alpha() -> u32 {{\n{}    0\n}}\n\npub fn beta() -> u32 {{\n    1\n}}\n",
            (0..20).map(|i| format!("    let v{i} = {i};\n")).collect::<String>()
        );
        let a = commit(dir.path(), &[("src/lib.rs", Some(&long))], "a");
        let changed = long
            .replace("let v2 = 2;", "let v2 = 200;")
            .replace("let v17 = 17;", "let v17 = 1700;")
            .replace("    1\n}", "    10\n}");
        let b = commit(dir.path(), &[("src/lib.rs", Some(&changed))], "b");
        let us = units_between(dir.path(), &a, &b);
        let alpha: Vec<&Unit> = us.iter().filter(|u| u.item.contains("alpha")).collect();
        assert_eq!(
            alpha.len(),
            1,
            "two hunks in alpha are one unit: {:?}",
            us.iter().map(|u| &u.key).collect::<Vec<_>>()
        );
        assert_eq!(alpha[0].lines.len(), 4, "both hunks' lines");
        assert!(us.iter().any(|u| u.item.contains("beta")), "beta is its own unit");
    }

    #[test]
    fn a_moved_function_and_its_origin_are_one_unit_and_a_shift_keeps_the_hash() {
        let dir = repo();
        let body = "pub fn render_status_bar(frame: &mut Frame, area: Rect) {\n    \
            let line = Line::from(status_text(area.width));\n    \
            frame.render_widget(Paragraph::new(line), area);\n    frame.flush();\n}\n";
        let a = commit(
            dir.path(),
            &[
                ("src/ui.rs", Some(&format!("pub fn keep() {{}}\n\n{body}"))),
                ("src/bar.rs", Some("pub fn other() {}\n")),
            ],
            "a",
        );
        let b = commit(
            dir.path(),
            &[
                ("src/ui.rs", Some("pub fn keep() {}\n")),
                ("src/bar.rs", Some(&format!("pub fn other() {{}}\n\n{body}"))),
            ],
            "b",
        );
        let us = units_between(dir.path(), &a, &b);
        let moved: Vec<&Unit> = us.iter().filter(|u| !u.also.is_empty()).collect();
        assert_eq!(
            moved.len(),
            1,
            "{:?}",
            us.iter().map(|u| (&u.key, &u.also)).collect::<Vec<_>>()
        );
        assert!(moved[0].path.ends_with("bar.rs"), "named where the code is now: {}", moved[0].key);
        assert!(moved[0].also[0].starts_with("src/ui.rs#"), "{:?}", moved[0].also);

        // The same edit, three lines lower: key and hash stay.
        let dir2 = repo();
        let pad = "pub fn pad0() {}\npub fn pad1() {}\npub fn pad2() {}\n";
        let a2 = commit(dir2.path(), &[("src/lib.rs", Some(&format!("{pad}{LIB}")))], "a");
        let b2 = commit(
            dir2.path(),
            &[("src/lib.rs", Some(&format!("{pad}{}", LIB.replace("let b = 2;", "let b = 20;"))))],
            "b",
        );
        let a1 = commit(dir.path(), &[("src/lib.rs", Some(LIB))], "c");
        let b1 = commit(
            dir.path(),
            &[("src/lib.rs", Some(&LIB.replace("let b = 2;", "let b = 20;")))],
            "d",
        );
        let one = units_between(dir.path(), &a1, &b1);
        let two = units_between(dir2.path(), &a2, &b2);
        let pick = |us: &[Unit]| {
            us.iter().find(|u| u.item.contains("alpha")).map(|u| (u.key.clone(), u.hash.clone()))
        };
        assert_eq!(pick(&one), pick(&two));
        assert!(pick(&one).is_some());
    }
}
