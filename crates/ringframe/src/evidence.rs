//! What an Eval judges, prepared by RingFrame rather than by a model
//! (ADR-0019 §1): every repository the work touched, its change cut into
//! windows a judge can read whole, moved code shown once, mechanical changes
//! set aside, weakened tests found, and for each obligation the windows that
//! bear on it.
//!
//! Nothing here reads a model's output, and nothing here is a judgement: a
//! window's class and an evidence set are where to look, not what is true.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::evaluate::{EvalError, git, git_bytes, ledger, str_of};
use crate::workspace::Workspace;
use crate::{digest, store};

/// A window closes at the first boundary after this many changed lines.
pub const WINDOW_CHANGED: usize = 60;
/// No window, and no evidence set, is larger than this: under the smallest
/// file-read window seen in a harness (46,080 bytes), so no judge reads one in
/// part.
pub const BUDGET: usize = 32 * 1024;
/// A moved block is at least this many lines and this many non-space
/// characters; shorter runs repeat by chance (`}`, `Ok(())`).
const MOVE_MIN_LINES: usize = 3;
const MOVE_MIN_CHARS: usize = 40;
/// A line starts a move only with this many non-space characters.
const ANCHOR_MIN_CHARS: usize = 4;
/// A key is rare when it is in at most this share of the windows judges read.
const RARE_SHARE: f64 = 0.02;

const LOCK_FILES: [&str; 11] = [
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "uv.lock",
    "Gemfile.lock",
    "go.sum",
    "composer.lock",
    "flake.lock",
];

/// The classes a model never sees: RingFrame calls them `consequence`.
pub const MECHANICAL: [&str; 5] = ["lock", "generated", "binary", "whitespace", "moved"];

// ---- the diff -------------------------------------------------------------

#[derive(Clone, Debug)]
pub(crate) struct Line {
    pub(crate) tag: u8,
    pub(crate) text: String,
    pub(crate) old: u32,
    pub(crate) new: u32,
    /// The move this line belongs to, by index into the move list.
    pub(crate) moved: Option<usize>,
    /// The item a changed line is in, as `git diff -U0` names it (with no
    /// context, git looks for the header just above the changed line, not
    /// above a context line that may be the item's own header).
    pub(crate) item: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Hunk {
    pub(crate) header: String,
    pub(crate) lines: Vec<Line>,
}

#[derive(Clone, Debug)]
pub(crate) struct FileDiff {
    pub(crate) repo: String,
    /// `<repo>/<path>`, or the path alone in the workspace's own repository.
    pub(crate) path: String,
    pub(crate) status: &'static str,
    pub(crate) binary: bool,
    pub(crate) renamed_from: Option<String>,
    pub(crate) hunks: Vec<Hunk>,
    /// Where the file's test module starts, on each side (`#[cfg(test)]`),
    /// read from the file itself: the marker is seldom inside a hunk.
    pub(crate) test_from: (Option<u32>, Option<u32>),
}

#[derive(Clone, Debug)]
pub(crate) struct Move {
    pub(crate) from_path: String,
    pub(crate) from_line: u32,
    pub(crate) to_path: String,
    pub(crate) to_line: u32,
    pub(crate) lines: usize,
}

fn prefixed(repo: &str, path: &str) -> String {
    if repo == "." { path.to_string() } else { format!("{repo}/{path}") }
}

fn hunk_starts(header: &str) -> (u32, u32) {
    // "@@ -a[,b] +c[,d] @@ …"
    let mut old = 0;
    let mut new = 0;
    for part in header.split_whitespace().skip(1).take(2) {
        let n = part[1..].split(',').next().and_then(|n| n.parse().ok()).unwrap_or(0);
        if part.starts_with('-') {
            old = n;
        } else if part.starts_with('+') {
            new = n;
        }
    }
    (old, new)
}

/// A unified diff as Git prints it with `--no-color`, one entry per file.
fn parse(patch: &str, repo: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut cur: Option<FileDiff> = None;
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for raw in patch.split_inclusive('\n') {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(f) = cur.take() {
                files.push(f);
            }
            let b = rest.rsplit_once(" b/").map_or(rest, |(_, b)| b);
            cur = Some(FileDiff {
                repo: repo.to_string(),
                path: prefixed(repo, b),
                status: "modified",
                binary: false,
                renamed_from: None,
                hunks: Vec::new(),
                test_from: (None, None),
            });
            in_hunk = false;
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };
        if !in_hunk || line.starts_with("@@ ") {
            if line.starts_with("new file mode") {
                f.status = "added";
            } else if line.starts_with("deleted file mode") {
                f.status = "deleted";
            } else if let Some(p) = line.strip_prefix("rename from ") {
                f.status = "renamed";
                f.renamed_from = Some(prefixed(repo, p));
            } else if let Some(p) = line.strip_prefix("rename to ") {
                f.path = prefixed(repo, p);
            } else if let Some(p) = line.strip_prefix("+++ b/") {
                f.path = prefixed(repo, p);
            } else if line.starts_with("--- a/") && f.status == "deleted" {
                f.path = prefixed(repo, &line[6..]);
            } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                f.binary = true;
            } else if line.starts_with("@@ ") {
                (old, new) = hunk_starts(line);
                f.hunks.push(Hunk { header: line.to_string(), lines: Vec::new() });
                in_hunk = true;
            }
            continue;
        }
        let Some(tag) = line.bytes().next() else {
            continue;
        };
        let hunk = f.hunks.last_mut().expect("in a hunk");
        match tag {
            b' ' | b'+' | b'-' => {
                hunk.lines.push(Line {
                    tag,
                    text: line[1..].to_string(),
                    old,
                    new,
                    moved: None,
                    item: String::new(),
                });
                if tag != b'+' {
                    old += 1;
                }
                if tag != b'-' {
                    new += 1;
                }
            }
            b'\\' => {}
            _ => in_hunk = false,
        }
    }
    if let Some(f) = cur.take() {
        files.push(f);
    }
    files
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn solid(text: &str) -> usize {
    text.chars().filter(|c| !c.is_whitespace()).count()
}

/// Mark every block of removed lines that reappears, indentation aside, as a
/// block of added lines somewhere in the change. Such a block is shown once,
/// as one line, on each side.
fn detect_moves(files: &mut [FileDiff]) -> Vec<Move> {
    type Pos = (usize, usize, usize);
    // Runs of consecutive lines of one kind, as positions.
    let runs = |files: &[FileDiff], tag: u8| -> Vec<Vec<Pos>> {
        let mut out = Vec::new();
        for (fi, f) in files.iter().enumerate() {
            for (hi, h) in f.hunks.iter().enumerate() {
                let mut run = Vec::new();
                for (li, l) in h.lines.iter().enumerate() {
                    if l.tag == tag {
                        run.push((fi, hi, li));
                    } else if !run.is_empty() {
                        out.push(std::mem::take(&mut run));
                    }
                }
                if !run.is_empty() {
                    out.push(run);
                }
            }
        }
        out
    };
    let removed = runs(files, b'-');
    let added = runs(files, b'+');
    let norm =
        |files: &[FileDiff], p: Pos| squash(&files[p.0].hunks[p.1].lines[p.2].text).to_string();
    let mut index: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for (ri, run) in added.iter().enumerate() {
        for (k, &p) in run.iter().enumerate() {
            let t = norm(files, p);
            if solid(&t) >= ANCHOR_MIN_CHARS {
                let slot = index.entry(t).or_default();
                if slot.len() < 64 {
                    slot.push((ri, k));
                }
            }
        }
    }
    let mut used_added: BTreeSet<Pos> = BTreeSet::new();
    let mut moves = Vec::new();
    for run in &removed {
        let mut i = 0;
        while i < run.len() {
            let t = norm(files, run[i]);
            let mut best: Option<(usize, usize, usize)> = None;
            for &(ri, k) in index.get(&t).into_iter().flatten() {
                let target = &added[ri];
                let mut n = 0;
                while i + n < run.len()
                    && k + n < target.len()
                    && !used_added.contains(&target[k + n])
                    && norm(files, run[i + n]) == norm(files, target[k + n])
                {
                    n += 1;
                }
                if best.is_none_or(|(_, _, m)| n > m) {
                    best = Some((ri, k, n));
                }
            }
            let Some((ri, k, n)) = best else {
                i += 1;
                continue;
            };
            let chars: usize = (0..n).map(|j| solid(&norm(files, run[i + j]))).sum();
            if n < MOVE_MIN_LINES || chars < MOVE_MIN_CHARS {
                i += 1;
                continue;
            }
            let from = run[i];
            let to = added[ri][k];
            let m = moves.len();
            moves.push(Move {
                from_path: files[from.0].path.clone(),
                from_line: files[from.0].hunks[from.1].lines[from.2].old,
                to_path: files[to.0].path.clone(),
                to_line: files[to.0].hunks[to.1].lines[to.2].new,
                lines: n,
            });
            for j in 0..n {
                let (a, b) = (run[i + j], added[ri][k + j]);
                files[a.0].hunks[a.1].lines[a.2].moved = Some(m);
                files[b.0].hunks[b.1].lines[b.2].moved = Some(m);
                used_added.insert(b);
            }
            i += n;
        }
    }
    moves
}

// ---- windows ----------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Window {
    pub id: String,
    pub repo: String,
    pub path: String,
    pub class: String,
    pub text: String,
    pub changed: usize,
    pub cut: bool,
    pub old: u32,
    pub new: u32,
    /// Lines removed and added that are not moves, for integrity and classes.
    removed: Vec<(u32, String)>,
    pub(crate) added: Vec<(u32, String)>,
    moved_only: bool,
    test_region: bool,
    /// The items (as git names them in hunk headers) whose changes this
    /// window holds, in order.
    pub items: Vec<String>,
}

/// The item a hunk is in, as `git diff` names it after the second `@@`
/// (the function, type or section), or `<top>`.
pub(crate) fn hunk_item(header: &str) -> String {
    let rest = header.splitn(3, "@@").nth(2).unwrap_or("").trim();
    if rest.is_empty() { "<top>".to_string() } else { rest.to_string() }
}

/// The 1-based line of a file's `#[cfg(test)]`, if it has one.
fn test_start(text: Option<&str>) -> Option<u32> {
    let text = text?;
    text.lines().position(|l| l.contains("#[cfg(test)]")).map(|i| i as u32 + 1)
}

fn is_boundary(text: &str) -> bool {
    matches!(
        text.trim(),
        "" | "}" | "};" | "}," | "]" | "]," | ")" | ");" | "});" | "})" | "end" | "fi" | "done"
    )
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    lower.split('/').any(|seg| matches!(seg, "tests" | "test" | "__tests__" | "spec" | "specs"))
        || name == "tests.rs"
        || name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.ends_with("_spec.rb")
}

fn window_id(path: &str, old: u32, new: u32, text: &str) -> String {
    let h = digest::sha256_bytes(format!("{path}\0{old}\0{new}\0{text}").as_bytes());
    format!("w_{}", &h[..12])
}

struct Builder {
    repo: String,
    path: String,
    text: String,
    changed: usize,
    old: u32,
    new: u32,
    last_move: Option<usize>,
    removed: Vec<(u32, String)>,
    added: Vec<(u32, String)>,
    moved_only: bool,
    test_region: bool,
    items: Vec<String>,
}

impl Builder {
    fn new(f: &FileDiff, old: u32, new: u32, test_region: bool) -> Self {
        Builder {
            repo: f.repo.clone(),
            path: f.path.clone(),
            text: format!("--- {}\n", f.path),
            changed: 0,
            old,
            new,
            last_move: None,
            removed: Vec::new(),
            added: Vec::new(),
            moved_only: true,
            test_region,
            items: Vec::new(),
        }
    }

    fn item(&mut self, item: &str) {
        if self.items.last().map(String::as_str) != Some(item) {
            self.items.push(item.to_string());
        }
    }

    fn push(&mut self, line: &str) {
        self.text.push_str(line);
        self.text.push('\n');
    }

    fn finish(self, cut: bool) -> Option<Window> {
        if self.changed == 0 {
            return None;
        }
        Some(Window {
            id: window_id(&self.path, self.old, self.new, &self.text),
            repo: self.repo,
            path: self.path,
            class: String::new(),
            text: self.text,
            changed: self.changed,
            cut,
            old: self.old,
            new: self.new,
            removed: self.removed,
            added: self.added,
            moved_only: self.moved_only,
            test_region: self.test_region,
            items: self.items,
        })
    }
}

fn move_line(m: &Move, tag: u8) -> String {
    if tag == b'-' {
        format!("~ moved {} lines to {}:{}", m.lines, m.to_path, m.to_line)
    } else {
        format!("~ moved {} lines from {}:{}", m.lines, m.from_path, m.from_line)
    }
}

/// Cut one file's changes into windows of about `WINDOW_CHANGED` changed
/// lines, closing at a blank line or a closing bracket, never over `BUDGET`.
fn file_windows(f: &FileDiff, moves: &[Move]) -> Vec<Window> {
    let mut out = Vec::new();
    let single = |text: String| {
        let mut b = Builder::new(f, 0, 0, false);
        b.push(&text);
        b.changed = 1;
        b.finish(false)
    };
    if f.binary {
        out.extend(single(format!("~ binary file {}", f.status)));
        return out;
    }
    if f.hunks.is_empty() {
        if let Some(from) = &f.renamed_from {
            out.extend(single(format!("~ renamed from {from}, content unchanged")));
        }
        return out;
    }
    let mut test_region = is_test_path(&f.path);
    let mut cur: Option<Builder> = None;
    for h in &f.hunks {
        if let Some(b) = cur.as_mut() {
            b.push(&h.header);
            b.last_move = None;
        }
        for l in &h.lines {
            let (old_from, new_from) = f.test_from;
            let past = |from: Option<u32>, at: u32| from.is_some_and(|n| at >= n);
            if l.text.contains("#[cfg(test)]")
                || (l.tag == b'-' && past(old_from, l.old))
                || (l.tag != b'-' && past(new_from, l.new))
            {
                test_region = true;
            }
            let b = cur.get_or_insert_with(|| {
                let mut b = Builder::new(f, l.old, l.new, test_region);
                b.push(&format!("@@ -{} +{} @@", l.old, l.new));
                b
            });
            // What this line adds to the window, before it is added.
            let mut rendered = match (l.tag, l.moved) {
                (b' ', _) => format!(" {}", l.text),
                (tag, Some(m)) if b.last_move != Some(m) => move_line(&moves[m], tag),
                (_, Some(_)) => continue,
                (tag, None) => format!("{}{}", tag as char, l.text),
            };
            if b.text.len() + rendered.len() + 1 > BUDGET && b.changed > 0 {
                out.extend(cur.take().expect("a window").finish(true));
                let mut nb = Builder::new(f, l.old, l.new, test_region);
                nb.push(&format!("@@ -{} +{} @@", l.old, l.new));
                cur = Some(nb);
            }
            let b = cur.as_mut().expect("a window");
            if l.tag != b' ' {
                b.item(
                    if l.item.is_empty() { hunk_item(&h.header) } else { l.item.clone() }.as_str(),
                );
            }
            let mut cut = false;
            let room = BUDGET.saturating_sub(b.text.len() + 1);
            if rendered.len() > room {
                let mut end = room;
                while !rendered.is_char_boundary(end) {
                    end -= 1;
                }
                rendered.truncate(end);
                cut = true;
            }
            b.push(&rendered);
            match (l.tag, l.moved) {
                (b' ', _) => {}
                (_, Some(m)) => {
                    b.last_move = Some(m);
                    b.changed += 1;
                }
                (tag, None) => {
                    b.last_move = None;
                    b.changed += 1;
                    b.moved_only = false;
                    if tag == b'-' {
                        b.removed.push((l.old, l.text.clone()));
                    } else {
                        b.added.push((l.new, l.text.clone()));
                    }
                }
            }
            if l.moved.is_none() {
                b.last_move = None;
            }
            if cut || (b.changed >= WINDOW_CHANGED && l.moved.is_none() && is_boundary(&l.text)) {
                out.extend(cur.take().expect("a window").finish(cut));
            }
        }
    }
    if let Some(b) = cur.take() {
        out.extend(b.finish(false));
    }
    out
}

fn classify(w: &mut Window, lock: bool, generated: bool, binary: bool) {
    let whitespace = !w.moved_only && {
        let squashed = |v: &[(u32, String)]| {
            let mut s: Vec<String> = v
                .iter()
                .map(|(_, t)| t.chars().filter(|c| !c.is_whitespace()).collect::<String>())
                .collect();
            s.sort();
            s
        };
        squashed(&w.removed) == squashed(&w.added)
    };
    w.class = if lock {
        "lock"
    } else if generated {
        "generated"
    } else if binary {
        "binary"
    } else if whitespace {
        "whitespace"
    } else if w.moved_only {
        "moved"
    } else if w.test_region || is_test_path(&w.path) {
        "test"
    } else {
        "code"
    }
    .to_string();
}

// ---- test integrity ---------------------------------------------------------

/// The name a line declares as a test, if it does: `fn name(` under
/// `#[test]`, `it("name"`, `test("name"`, `def test_name(`, `func TestName(`.
fn test_name(prev: Option<&str>, line: &str) -> Option<String> {
    let t = line.trim();
    let ident = |s: &str| -> String {
        s.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect()
    };
    if prev.is_some_and(|p| p.trim().starts_with("#[") && p.contains("test")) {
        let after = t.strip_prefix("pub ").unwrap_or(t);
        let after = after.strip_prefix("async ").unwrap_or(after);
        if let Some(rest) = after.strip_prefix("fn ") {
            return Some(ident(rest)).filter(|n| !n.is_empty());
        }
    }
    if let Some(rest) = t.strip_prefix("def test_") {
        return Some(format!("test_{}", ident(rest)));
    }
    if let Some(rest) = t.strip_prefix("func Test") {
        return Some(format!("Test{}", ident(rest)));
    }
    for opener in ["it(", "test(", "it.only(", "test.only("] {
        if let Some(rest) = t.strip_prefix(opener) {
            let q = rest.chars().next()?;
            if matches!(q, '"' | '\'' | '`') {
                return rest[1..].split(q).next().map(str::to_string);
            }
        }
    }
    None
}

fn is_assert(text: &str) -> bool {
    let t = text.trim_start();
    t.contains("assert") || t.contains("expect(") || t.starts_with("should")
}

/// Tests taken away and asserts removed, not moved: the change a test suite
/// cannot see in itself.
fn integrity(windows: &[Window]) -> Vec<Value> {
    let declared = |pick: &dyn Fn(&Window) -> &Vec<(u32, String)>| {
        let mut out: Vec<(String, usize, u32, String)> = Vec::new();
        for (wi, w) in windows.iter().enumerate() {
            let lines = pick(w);
            for (i, (n, text)) in lines.iter().enumerate() {
                let prev = if i > 0 { Some(lines[i - 1].1.as_str()) } else { None };
                if let Some(name) = test_name(prev, text) {
                    out.push((name, wi, *n, text.clone()));
                }
            }
        }
        out
    };
    let removed = declared(&|w| &w.removed);
    let added: BTreeSet<String> = declared(&|w| &w.added).into_iter().map(|d| d.0).collect();
    let mut findings = Vec::new();
    for (name, wi, line, text) in removed {
        if !added.contains(&name) {
            let w = &windows[wi];
            findings.push(json!({"kind": "test_removed", "window": w.id, "path": w.path,
                                 "line": line, "name": name, "text": text.trim()}));
        }
    }
    for w in windows.iter().filter(|w| w.class == "test") {
        let gone: Vec<&(u32, String)> = w.removed.iter().filter(|(_, t)| is_assert(t)).collect();
        let came = w.added.iter().filter(|(_, t)| is_assert(t)).count();
        if gone.len() > came {
            let (line, text) = gone[0];
            findings.push(json!({"kind": "assert_removed", "window": w.id, "path": w.path,
                                 "line": line, "removed": gone.len(), "added": came,
                                 "text": text.trim()}));
        }
    }
    findings
}

// ---- repositories -----------------------------------------------------------

/// One repository the Eval reads: the workspace's own, or one the work touched.
#[derive(Clone, Debug)]
pub struct Repo {
    pub name: String,
    pub root: PathBuf,
    pub anchor: String,
    pub anchor_from: &'static str,
    pub subject_kind: String,
    pub subject_ref: String,
}

/// `to` relative to `from`, both absolute: `../fab7`.
fn relative(from: &Path, to: &Path) -> String {
    let a: Vec<_> = from.components().collect();
    let b: Vec<_> = to.components().collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".into(); a.len() - common];
    parts.extend(b[common..].iter().map(|c| c.as_os_str().to_string_lossy().to_string()));
    if parts.is_empty() { ".".into() } else { parts.join("/") }
}

pub fn git_root(dir: &Path) -> Option<PathBuf> {
    let out = git(dir, &["rev-parse", "--show-toplevel"]).ok()?;
    PathBuf::from(out.trim()).canonicalize().ok()
}

fn subject_of(root: &Path) -> Result<(String, String), EvalError> {
    if !git(root, &["status", "--porcelain"])?.trim().is_empty() {
        return Ok(("worktree".into(), root.to_string_lossy().to_string()));
    }
    Ok(("git_commit".into(), git(root, &["rev-parse", "HEAD"])?.trim().to_string()))
}

// ---- baselines ---------------------------------------------------------------

/// The Git roots of the paths a text names that exist: `../fab7/products`,
/// `/abs/repo/file.md`. The workspace's own root is not among them.
pub fn named_roots(ws: &Workspace, texts: &[&str]) -> Vec<PathBuf> {
    let own = git_root(&ws.root);
    let mut out: Vec<PathBuf> = Vec::new();
    for text in texts {
        for word in text.split_whitespace() {
            let token = crate::documents::trim_punct(word);
            if !token.contains('/') || token.contains("://") {
                continue;
            }
            let p = ws.root.join(token);
            let dir = if p.is_dir() {
                p
            } else if p.exists() {
                p.parent().map(Path::to_path_buf).unwrap_or(p)
            } else {
                continue;
            };
            if let Some(root) = git_root(&dir)
                && Some(&root) != own.as_ref()
                && !out.contains(&root)
            {
                out.push(root);
            }
        }
    }
    out
}

/// What an Ask records when it is compiled: the commit each repository is at
/// (the workspace's own and every one its words name), and whether anything
/// in it was uncommitted then. The Eval reads each one's change with
/// `git diff` from that commit, whatever made the change: an edit tool, the
/// shell, a script or a sub-agent. What was uncommitted at the Ask is in that
/// diff too; `dirty` lets the Eval say so.
pub fn baselines(ws: &Workspace, texts: &[&str]) -> (Vec<Value>, Vec<String>) {
    let base = ws.root.canonicalize().unwrap_or_else(|_| ws.root.clone());
    let mut roots: Vec<(String, PathBuf)> = Vec::new();
    if let Some(own) = git_root(&ws.root) {
        roots.push((".".into(), own));
    }
    for root in named_roots(ws, texts) {
        roots.push((relative(&base, &root), root));
    }
    let mut out = Vec::new();
    let mut limitations = Vec::new();
    for (name, root) in roots {
        let Ok(head) = git(&root, &["rev-parse", "--verify", "--quiet", "HEAD"]) else {
            limitations.push(format!("{name} has no commit yet; its change cannot be read"));
            continue;
        };
        let dirty = git(&root, &["status", "--porcelain"]).is_ok_and(|s| !s.trim().is_empty());
        out.push(json!({"repo": name, "root": root.to_string_lossy(),
                        "commit": head.trim(), "dirty": dirty}));
    }
    (out, limitations)
}

/// The repositories besides the workspace's that the open Asks recorded a
/// baseline for, each read from the first one's, and any named with
/// `--repo <path>[=<anchor>]`.
pub fn other_repos(
    ws: &Workspace,
    asks: &[Value],
    named: &[String],
    first_ask_time: Option<&str>,
    limitations: &mut Vec<String>,
) -> Result<Vec<Repo>, EvalError> {
    let own = git_root(&ws.root);
    let base = ws.root.canonicalize().unwrap_or_else(|_| ws.root.clone());
    let mut found: BTreeMap<PathBuf, Repo> = BTreeMap::new();
    for b in asks.iter().flat_map(|a| a["baselines"].as_array().into_iter().flatten()) {
        let root = PathBuf::from(str_of(b, "root"));
        if str_of(b, "repo") == "." || Some(&root) == own.as_ref() || found.contains_key(&root) {
            continue;
        }
        let name = relative(&base, &root);
        if b["dirty"] == true {
            limitations.push(format!(
                "{name} had uncommitted changes when an Ask named it; they are read as part of the work"
            ));
        }
        found.insert(
            root.clone(),
            Repo {
                name,
                root,
                anchor: str_of(b, "commit"),
                anchor_from: "ask",
                subject_kind: String::new(),
                subject_ref: String::new(),
            },
        );
    }
    for spec in named {
        let (path, anchor) = match spec.split_once('=') {
            Some((p, a)) => (p, Some(a.to_string())),
            None => (spec.as_str(), None),
        };
        let dir = ws.root.join(path);
        let Some(root) = git_root(&dir) else {
            return Err(ledger("eval.repo", format!("{path} is not in a Git repository")));
        };
        if Some(&root) == own.as_ref() {
            continue;
        }
        let (anchor, from) = match anchor {
            Some(a) => (a, "explicit"),
            None => {
                let before = first_ask_time.map(|t| format!("--before={t}"));
                let mut args = vec!["rev-list", "-1"];
                if let Some(b) = before.as_deref() {
                    args.push(b);
                }
                args.push("HEAD");
                (git(&root, &args)?.trim().to_string(), "first_ask")
            }
        };
        found.insert(
            root.clone(),
            Repo {
                name: relative(&base, &root),
                root,
                anchor,
                anchor_from: from,
                subject_kind: String::new(),
                subject_ref: String::new(),
            },
        );
    }
    let mut out = Vec::new();
    for (_, mut r) in found {
        let ok = !r.anchor.is_empty()
            && git(
                &r.root,
                &["rev-parse", "--verify", "--quiet", &format!("{}^{{commit}}", r.anchor)],
            )
            .is_ok();
        if !ok {
            limitations.push(format!(
                "{}: its anchor {} is not a commit there; it is not read",
                r.name, r.anchor
            ));
            continue;
        }
        (r.subject_kind, r.subject_ref) = subject_of(&r.root)?;
        out.push(r);
    }
    Ok(out)
}

/// Git's built-in function-name drivers, by extension, so a hunk header
/// names the function, type or section it is in. A repository's own
/// `.gitattributes` still wins over this file. Any other file gets a driver
/// that names nothing (git's default would name the last line starting with
/// a letter), and TOML one for its tables.
const FUNCTION_NAMES: &str = "\
* diff=ringframe-top
*.toml diff=ringframe-toml
*.rs diff=rust
*.py diff=python
*.go diff=golang
*.java diff=java
*.kt diff=kotlin
*.c diff=cpp
*.h diff=cpp
*.cc diff=cpp
*.cpp diff=cpp
*.hpp diff=cpp
*.cs diff=csharp
*.rb diff=ruby
*.php diff=php
*.sh diff=bash
*.bash diff=bash
*.md diff=markdown
*.css diff=css
*.html diff=html
*.ex diff=elixir
*.exs diff=elixir
*.tex diff=tex
";

/// The attributes file `git diff` reads for function names, written once.
pub(crate) fn function_names_file() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "ringframe-function-names-{}.attributes",
        &digest::sha256_bytes(FUNCTION_NAMES.as_bytes())[..8]
    ));
    if !path.exists() {
        let _ = std::fs::write(&path, FUNCTION_NAMES);
    }
    path
}

/// Name each changed line's item from a `-U0` diff of the same range.
fn name_items(files: &mut [FileDiff], zero: &[FileDiff]) {
    type Span = (u32, u32, u32, u32, String);
    let spans: BTreeMap<&str, Vec<Span>> = zero
        .iter()
        .map(|f| {
            let spans = f
                .hunks
                .iter()
                .map(|h| {
                    let side = |tag: u8| -> (u32, u32) {
                        let v: Vec<u32> = h
                            .lines
                            .iter()
                            .filter(|l| l.tag == tag)
                            .map(|l| if tag == b'-' { l.old } else { l.new })
                            .collect();
                        (v.first().copied().unwrap_or(0), v.last().copied().unwrap_or(0))
                    };
                    let ((o1, o2), (n1, n2)) = (side(b'-'), side(b'+'));
                    (o1, o2, n1, n2, hunk_item(&h.header))
                })
                .collect();
            (f.path.as_str(), spans)
        })
        .collect();
    for f in files {
        let Some(sp) = spans.get(f.path.as_str()) else { continue };
        for h in &mut f.hunks {
            let fallback = hunk_item(&h.header);
            for l in h.lines.iter_mut().filter(|l| l.tag != b' ') {
                let hit = sp.iter().find(|(o1, o2, n1, n2, _)| {
                    if l.tag == b'-' {
                        *o1 > 0 && (*o1..=*o2).contains(&l.old)
                    } else {
                        *n1 > 0 && (*n1..=*n2).contains(&l.new)
                    }
                });
                l.item = hit.map_or_else(|| fallback.clone(), |s| s.4.clone());
            }
            // Git names a hunk from the line above it, so a hunk that starts
            // at the top of a file (a new file) names nothing: take the
            // nearest definition above the line, within the hunk.
            let mut current: Option<String> = None;
            for l in &mut h.lines {
                if l.tag != b'-' && is_definition(&l.text, &f.path) {
                    current = Some(l.text.trim().to_string());
                }
                if l.tag != b' '
                    && l.item == "<top>"
                    && let Some(c) = &current
                {
                    l.item = c.clone();
                }
            }
        }
    }
}

/// A line that starts a named item: a function, type, class or module, or a
/// Markdown heading. Only for lines git names nothing for.
pub(crate) fn is_definition(text: &str, path: &str) -> bool {
    const STARTS: [&str; 16] = [
        "fn",
        "struct",
        "enum",
        "trait",
        "impl",
        "mod",
        "type",
        "macro_rules",
        "def",
        "class",
        "func",
        "function",
        "interface",
        "const",
        "static",
        "async",
    ];
    const BEFORE: [&str; 6] = ["pub", "crate", "export", "async", "unsafe", "default"];
    if path.ends_with(".md") {
        return text.starts_with('#');
    }
    let t = text.trim_start();
    if t.len() == text.len() || text.len() - t.len() <= 4 {
        let words: Vec<&str> = t
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|w| !w.is_empty())
            .collect();
        let first = words.iter().position(|w| !BEFORE.contains(w)).unwrap_or(0);
        words.get(first).is_some_and(|w| STARTS.contains(w)) && words.len() > first + 1
    } else {
        false
    }
}

/// The change in one repository, with one line of context: `git diff` from
/// its anchor to its commit, or to what is on disk plus untracked files.
fn repo_patch(r: &Repo, relative_to_workspace: bool) -> Result<String, EvalError> {
    repo_patch_with(r, relative_to_workspace, "-U1")
}

/// The drivers `FUNCTION_NAMES` names that git does not ship.
const DRIVERS: [&str; 2] =
    ["diff.ringframe-top.xfuncname=^\\b$", "diff.ringframe-toml.xfuncname=^\\[[^]]+\\]"];

fn repo_patch_with(
    r: &Repo,
    relative_to_workspace: bool,
    context: &str,
) -> Result<String, EvalError> {
    let attrs = format!("core.attributesFile={}", function_names_file().display());
    let mut args = vec!["-c", attrs.as_str(), "-c", DRIVERS[0], "-c", DRIVERS[1]];
    args.extend(["diff", context, "--no-color", "--no-ext-diff", "--binary"]);
    if relative_to_workspace {
        args.push("--relative");
    }
    args.push(&r.anchor);
    if r.subject_kind == "git_commit" {
        args.push(&r.subject_ref);
    }
    args.extend(["--", "."]);
    let root = r.root_for_diff(relative_to_workspace);
    let mut out = git_bytes(&root, &args, &[0])?;
    if r.subject_kind == "worktree" {
        for name in git(&root, &["ls-files", "--others", "--exclude-standard", "-z"])?.split('\0') {
            if !name.is_empty() {
                out.extend(git_bytes(
                    &root,
                    &[
                        "-c",
                        &attrs,
                        "-c",
                        DRIVERS[0],
                        "-c",
                        DRIVERS[1],
                        "diff",
                        context,
                        "--no-color",
                        "--binary",
                        "--no-index",
                        "/dev/null",
                        name,
                    ],
                    &[0, 1],
                )?);
            }
        }
    }
    Ok(String::from_utf8_lossy(&out).to_string())
}

impl Repo {
    /// The workspace's own repository is read from the workspace directory, so
    /// its paths stay the workspace's (`--relative`); another from its root.
    pub(crate) fn root_for_diff(&self, workspace: bool) -> PathBuf {
        if workspace && self.subject_kind == "worktree" {
            PathBuf::from(&self.subject_ref)
        } else {
            self.root.clone()
        }
    }
}

fn generated_paths(root: &Path, paths: &[String]) -> BTreeSet<String> {
    if paths.is_empty() {
        return BTreeSet::new();
    }
    let mut args = vec!["check-attr", "-z", "linguist-generated", "linguist-vendored", "--"];
    args.extend(paths.iter().map(String::as_str));
    let Ok(out) = git(root, &args) else { return BTreeSet::new() };
    let parts: Vec<&str> = out.split('\0').collect();
    parts
        .chunks(3)
        .filter(|c| c.len() == 3 && matches!(c[2], "set" | "true"))
        .map(|c| c[0].to_string())
        .collect()
}

// ---- the evidence -----------------------------------------------------------

/// Everything `eval open` prepares: the windows' index and their text.
pub struct Evidence {
    pub windows: Vec<Window>,
    pub repos: Vec<Repo>,
    pub integrity: Vec<Value>,
    pub moves: usize,
    /// The parsed patch, which the change DAG is built from.
    pub(crate) files: Vec<FileDiff>,
}

/// Read every repository's change and cut it into classified windows.
pub fn gather(repos: &[Repo], workspace_repo: usize) -> Result<Evidence, EvalError> {
    let mut files = Vec::new();
    let mut raw_paths: Vec<(usize, Vec<String>)> = Vec::new();
    for (i, r) in repos.iter().enumerate() {
        let patch = repo_patch(r, i == workspace_repo)?;
        let mut parsed = parse(&patch, &r.name);
        let zero = parse(&repo_patch_with(r, i == workspace_repo, "-U0")?, &r.name);
        name_items(&mut parsed, &zero);
        let strip = |p: &str| -> String {
            if r.name == "." { p.to_string() } else { p[r.name.len() + 1..].to_string() }
        };
        raw_paths.push((i, parsed.iter().map(|f| strip(&f.path)).collect()));
        files.extend(parsed);
    }
    for f in &mut files {
        let Some(r) = repos.iter().position(|r| r.name == f.repo) else { continue };
        let repo = &repos[r];
        let rel = if repo.name == "." {
            f.path.clone()
        } else {
            f.path[repo.name.len() + 1..].to_string()
        };
        let dir = repo.root_for_diff(r == workspace_repo);
        let at = |rev: &str| git(&dir, &["show", &format!("{rev}:./{rel}")]).ok();
        let new = if repo.subject_kind == "git_commit" {
            at(&repo.subject_ref)
        } else {
            std::fs::read_to_string(dir.join(&rel)).ok()
        };
        f.test_from = (test_start(at(&repo.anchor).as_deref()), test_start(new.as_deref()));
    }
    let moves = detect_moves(&mut files);
    let mut generated: BTreeSet<String> = BTreeSet::new();
    for (i, paths) in &raw_paths {
        let r = &repos[*i];
        let dir = r.root_for_diff(*i == workspace_repo);
        for p in generated_paths(&dir, paths) {
            generated.insert(prefixed(&r.name, &p));
        }
    }
    let mut windows = Vec::new();
    for f in &files {
        let name = f.path.rsplit('/').next().unwrap_or(&f.path);
        let lock = LOCK_FILES.contains(&name);
        for mut w in file_windows(f, &moves) {
            classify(&mut w, lock, generated.contains(&f.path), f.binary);
            windows.push(w);
        }
    }
    let integrity = integrity(&windows);
    Ok(Evidence { windows, repos: repos.to_vec(), integrity, moves: moves.len(), files })
}

/// Prepare what the judges read and publish it beside the brief:
/// `windows.json` (each window's text) and `evidence.json` (the index, the
/// documents, their obligations, each one's evidence set, and the windows no
/// obligation claimed). Returns the brief's summary and the refs the opened
/// event carries.
#[allow(clippy::too_many_arguments)]
pub fn prepare(
    ws: &Workspace,
    eval_id: &str,
    asks: &[Value],
    anchor: &str,
    kind: &str,
    reference: &str,
    named: &[String],
    limitations: &mut Vec<String>,
) -> Result<(Value, Value), EvalError> {
    let mut repos = vec![Repo {
        name: ".".into(),
        root: ws.root.clone(),
        anchor: anchor.into(),
        anchor_from: "workspace",
        subject_kind: kind.into(),
        subject_ref: reference.into(),
    }];
    let first_time = asks.first().map(|a| str_of(a, "time"));
    let own_dirty = asks.first().is_some_and(|a| {
        a["baselines"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|b| b["repo"] == "." && b["dirty"] == true)
    });
    if own_dirty {
        limitations.push(
            "the workspace had uncommitted changes when the first open Ask was compiled; they are read as part of the work".into(),
        );
    }
    repos.extend(other_repos(ws, asks, named, first_time.as_deref(), limitations)?);
    let ev = gather(&repos, 0)?;
    let prompts: Vec<(String, String)> = asks
        .iter()
        .map(|a| {
            let path = ws.rf_dir().join(str_of(&a["prompt"], "path"));
            (str_of(a, "ask_id"), std::fs::read_to_string(path).unwrap_or_default())
        })
        .collect();
    let (docs, obligations) = crate::documents::read(ws, &prompts, limitations);
    let pairs: Vec<(String, BTreeSet<String>)> = obligations
        .iter()
        .map(|o| (o.id.clone(), keys(&[&o.text, &o.done_when, &o.as_built])))
        .collect();
    let (sets, unclaimed) = retrieve(&pairs, &ev.windows);
    let unclaimed: BTreeSet<String> = unclaimed.into_iter().collect();
    let flagged: BTreeSet<String> = ev.integrity.iter().map(|f| str_of(f, "window")).collect();
    let trace: Vec<&str> = ev
        .windows
        .iter()
        .filter(|w| unclaimed.contains(&w.id) || flagged.contains(&w.id))
        .map(|w| w.id.as_str())
        .collect();
    let mut mechanical: BTreeMap<&str, usize> = BTreeMap::new();
    for w in ev.windows.iter().filter(|w| MECHANICAL.contains(&w.class.as_str())) {
        *mechanical
            .entry(MECHANICAL.iter().find(|c| **c == w.class).expect("mechanical"))
            .or_default() += 1;
    }
    let texts: serde_json::Map<String, Value> =
        ev.windows.iter().map(|w| (w.id.clone(), Value::String(w.text.clone()))).collect();
    let windows_doc = json!({"schema": "ringframe.eval-windows/1", "windows": texts});
    let evidence_doc = json!({
        "schema": "ringframe.eval-evidence/1",
        "repos": ev.repos.iter().map(|r| json!({
            "name": r.name, "root": r.root.to_string_lossy(),
            "anchor": {"ref": r.anchor, "from": r.anchor_from},
            "subject": {"kind": r.subject_kind, "ref": r.subject_ref},
        })).collect::<Vec<_>>(),
        "windows": ev.windows.iter().map(window_index).collect::<Vec<_>>(),
        "moves": ev.moves,
        "mechanical": mechanical,
        "test_integrity": ev.integrity,
        "documents": docs.iter().map(crate::documents::Document::to_json).collect::<Vec<_>>(),
        "obligations": obligations.iter().map(crate::documents::Obligation::to_json).collect::<Vec<_>>(),
        "evidence_sets": sets,
        "trace": trace,
    });
    let publish = |name: &str, doc: &Value, role: &str| {
        let mut bytes = store::canonical(doc);
        bytes.push(b'\n');
        store::publish(ws, &format!("evals/{eval_id}/{name}"), &bytes, role)
    };
    let windows_ref = publish("windows.json", &windows_doc, "eval_windows")?;
    let evidence_ref = publish("evidence.json", &evidence_doc, "eval_evidence")?;
    let (changes, changes_ref) =
        crate::changes::view(ws, eval_id, &ev, asks, &prompts, &obligations)?;
    let summary = json!({
        "repos": ev.repos.iter().map(|r| r.name.clone()).collect::<Vec<_>>(),
        "windows": ev.windows.len(),
        "mechanical": mechanical.values().sum::<usize>(),
        "obligations": obligations.len(),
        "trace": trace.len(),
        "test_integrity": ev.integrity.len(),
        "evidence": evidence_ref,
        "windows_text": windows_ref,
        "change_dag": changes,
    });
    Ok((summary, json!({"evidence": evidence_ref, "windows": windows_ref, "changes": changes_ref})))
}

/// The text of one window of an Eval.
pub fn window_text(ws: &Workspace, eval_id: &str, window: &str) -> Result<String, EvalError> {
    let doc = read_windows(ws, eval_id)?;
    doc["windows"][window].as_str().map(str::to_string).ok_or_else(|| {
        ledger("eval.window_unknown", format!("{window} is not a window of {eval_id}"))
    })
}

fn read_windows(ws: &Workspace, eval_id: &str) -> Result<Value, EvalError> {
    let path = ws.rf_dir().join(format!("evals/{eval_id}/windows.json"));
    let bytes = std::fs::read(&path).map_err(|_| {
        ledger(
            "eval.no_evidence",
            format!("{eval_id} has no windows; it was opened before they existed"),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|e| ledger("eval.no_evidence", e.to_string()))
}

/// At most 20 lines, over every window and every named document part, that
/// contain `text` (case aside).
pub fn grep(ws: &Workspace, eval_id: &str, text: &str) -> Result<Vec<String>, EvalError> {
    if text.trim().chars().count() < 3 {
        return Err(ledger("eval.grep_short", "search for three characters or more"));
    }
    let doc = read_windows(ws, eval_id)?;
    let mut out = Vec::new();
    for (id, body) in doc["windows"].as_object().into_iter().flatten() {
        out.extend(grep_window(id, body.as_str().unwrap_or_default(), text));
        if out.len() >= 20 {
            out.truncate(20);
            return Ok(out);
        }
    }
    let evidence = ws.rf_dir().join(format!("evals/{eval_id}/evidence.json"));
    if let Ok(bytes) = std::fs::read(evidence) {
        let ev: Value = serde_json::from_slice(&bytes).unwrap_or_default();
        let needle = text.to_lowercase();
        for d in ev["documents"].as_array().into_iter().flatten() {
            for p in d["parts"].as_array().into_iter().flatten() {
                let first = p["line"].as_u64().unwrap_or(1);
                for (n, line) in p["text"].as_str().unwrap_or_default().lines().enumerate() {
                    if line.to_lowercase().contains(&needle) {
                        out.push(format!(
                            "{} {}:{} {}",
                            str_of(d, "id"),
                            str_of(d, "path"),
                            first + n as u64,
                            line.trim()
                        ));
                        if out.len() >= 20 {
                            return Ok(out);
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

// ---- retrieval --------------------------------------------------------------

/// The keys an obligation names: identifiers of five or more characters and
/// paths inside backticks.
pub fn keys(texts: &[&str]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for text in texts {
        for (i, quoted) in text.split('`').enumerate() {
            if i % 2 == 0 {
                continue;
            }
            let q = quoted.trim();
            if q.contains('/') || q.contains('.') && q.len() > 4 {
                let path = q.split(':').next().unwrap_or(q);
                if path.len() >= 5 {
                    out.insert(path.to_string());
                }
            }
            let mut word = String::new();
            for c in q.chars().chain([' ']) {
                if c.is_alphanumeric() || c == '_' {
                    word.push(c);
                } else {
                    let starts = word.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_');
                    if word.chars().count() >= 5 && starts {
                        out.insert(std::mem::take(&mut word));
                    }
                    word.clear();
                }
            }
        }
    }
    out
}

/// For each obligation, the windows that name its rare keys, best first, up to
/// the budget; and the windows no obligation claimed.
pub fn retrieve(
    obligations: &[(String, BTreeSet<String>)],
    windows: &[Window],
) -> (BTreeMap<String, Value>, Vec<String>) {
    let items: Vec<Item> = windows
        .iter()
        .filter(|w| !MECHANICAL.contains(&w.class.as_str()))
        .map(|w| Item { id: w.id.clone(), path: w.path.clone(), text: w.text.clone() })
        .collect();
    retrieve_items(obligations, &items, BUDGET)
}

/// A window as retrieval sees it.
pub struct Item {
    pub id: String,
    pub path: String,
    pub text: String,
}

/// `retrieve` over windows already read back from `windows.json`.
pub fn retrieve_items(
    obligations: &[(String, BTreeSet<String>)],
    items: &[Item],
    budget: usize,
) -> (BTreeMap<String, Value>, Vec<String>) {
    let read: Vec<&Item> = items.iter().collect();
    let limit = ((read.len() as f64) * RARE_SHARE).ceil().max(1.0) as usize;
    let hits = |key: &str| -> Vec<usize> {
        read.iter()
            .enumerate()
            .filter(|(_, w)| w.text.contains(key) || w.path.ends_with(key))
            .map(|(i, _)| i)
            .collect()
    };
    let mut cache: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut sets = BTreeMap::new();
    let mut claimed: BTreeSet<usize> = BTreeSet::new();
    for (id, ks) in obligations {
        let mut score: BTreeMap<usize, usize> = BTreeMap::new();
        for k in ks {
            let h = cache.entry(k.clone()).or_insert_with(|| hits(k));
            if h.is_empty() || h.len() > limit {
                continue;
            }
            for &i in h.iter() {
                *score.entry(i).or_default() += 1;
            }
        }
        let mut ranked: Vec<(usize, usize)> = score.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut chosen = Vec::new();
        let mut bytes = 0;
        for (i, _) in ranked {
            let w = read[i];
            if bytes + w.text.len() > budget {
                continue;
            }
            bytes += w.text.len();
            chosen.push(w.id.clone());
            claimed.insert(i);
        }
        sets.insert(id.clone(), json!({"windows": chosen, "bytes": bytes}));
    }
    let unclaimed = read
        .iter()
        .enumerate()
        .filter(|(i, _)| !claimed.contains(i))
        .map(|(_, w)| w.id.clone())
        .collect();
    (sets, unclaimed)
}

// ---- lookups for judges -----------------------------------------------------

pub fn window_index(w: &Window) -> Value {
    let mut v = json!({"id": w.id, "repo": w.repo, "path": w.path, "class": w.class,
                       "changed": w.changed, "bytes": w.text.len(), "old": w.old, "new": w.new});
    if w.cut {
        v["cut"] = json!(true);
    }
    v
}

/// The lines of a window that contain `needle` (case aside), with the line
/// number on the side each is on.
pub fn grep_window(id: &str, text: &str, needle: &str) -> Vec<String> {
    let needle = needle.to_lowercase();
    let mut out = Vec::new();
    let mut path = String::new();
    let (mut old, mut new) = (0u32, 0u32);
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("--- ") {
            path = p.to_string();
            continue;
        }
        if line.starts_with("@@ ") {
            (old, new) = hunk_starts(line);
            continue;
        }
        let (tag, body) = line.split_at(line.len().min(1));
        let at = if tag == "-" { old } else { new };
        if body.to_lowercase().contains(&needle) {
            out.push(format!("{id} {path}:{at} {tag}{body}"));
        }
        match tag {
            "-" => old += 1,
            "+" => new += 1,
            " " => {
                old += 1;
                new += 1;
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(files: &[(&str, &[&str])]) -> String {
        // A patch with one hunk per file: lines are given with their tag.
        let mut out = String::new();
        for (path, lines) in files {
            let old = lines.iter().filter(|l| !l.starts_with('+')).count();
            let new = lines.iter().filter(|l| !l.starts_with('-')).count();
            out.push_str(&format!(
                "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1,{old} +1,{new} @@\n"
            ));
            for l in *lines {
                out.push_str(l);
                out.push('\n');
            }
        }
        out
    }

    fn windows_of(patch: &str) -> (Vec<Window>, usize) {
        let mut files = parse(patch, ".");
        let moves = detect_moves(&mut files);
        let mut out = Vec::new();
        for f in &files {
            for mut w in file_windows(f, &moves) {
                classify(&mut w, false, false, f.binary);
                out.push(w);
            }
        }
        (out, moves.len())
    }

    const BODY: [&str; 4] = [
        "fn render_status_bar(frame: &mut Frame, area: Rect) {",
        "    let line = Line::from(status_text(area.width));",
        "    frame.render_widget(Paragraph::new(line), area);",
        "}",
    ];

    #[test]
    fn a_moved_function_is_one_line_on_each_side() {
        let removed: Vec<String> = BODY.iter().map(|l| format!("-{l}")).collect();
        let added: Vec<String> = BODY.iter().map(|l| format!("+    {l}")).collect();
        let r: Vec<&str> = removed.iter().map(String::as_str).collect();
        let a: Vec<&str> = added.iter().map(String::as_str).collect();
        let (ws, moves) = windows_of(&diff(&[("src/ui.rs", &r), ("src/ui/bar.rs", &a)]));
        assert_eq!(moves, 1);
        assert_eq!(ws.len(), 2);
        assert!(ws.iter().all(|w| w.class == "moved"), "{ws:?}");
        assert!(ws[0].text.contains("~ moved 4 lines to src/ui/bar.rs:1"), "{}", ws[0].text);
        assert!(ws[1].text.contains("~ moved 4 lines from src/ui.rs:1"), "{}", ws[1].text);
        assert!(!ws[1].text.contains("render_widget"));
    }

    #[test]
    fn a_changed_line_inside_a_moved_block_stays_as_it_is() {
        let removed: Vec<String> = BODY.iter().map(|l| format!("-{l}")).collect();
        let mut added: Vec<String> = BODY.iter().map(|l| format!("+{l}")).collect();
        added[1] = "+    let line = Line::from(status_text(area.width - 2));".into();
        let r: Vec<&str> = removed.iter().map(String::as_str).collect();
        let a: Vec<&str> = added.iter().map(String::as_str).collect();
        let (ws, moves) = windows_of(&diff(&[("src/ui.rs", &r), ("src/ui/bar.rs", &a)]));
        assert_eq!(moves, 0, "two lines alike is not a move");
        assert!(ws.iter().all(|w| w.class == "code"));
        assert!(ws[1].text.contains("area.width - 2"));
    }

    #[test]
    fn no_window_is_over_the_budget_and_ids_are_stable() {
        let big: Vec<String> =
            (0..3000).map(|i| format!("+let value_{i} = compute_{i}(input_{i});")).collect();
        let lines: Vec<&str> = big.iter().map(String::as_str).collect();
        let patch = diff(&[("src/big.rs", &lines)]);
        let (a, _) = windows_of(&patch);
        let (b, _) = windows_of(&patch);
        assert!(a.len() > 1);
        assert!(a.iter().all(|w| w.text.len() <= BUDGET), "a window over the budget");
        assert_eq!(
            a.iter().map(|w| &w.id).collect::<Vec<_>>(),
            b.iter().map(|w| &w.id).collect::<Vec<_>>()
        );
        let changed: usize = a.iter().map(|w| w.changed).sum();
        assert_eq!(changed, 3000, "every changed line is in a window");
    }

    #[test]
    fn a_window_closes_at_a_boundary_after_sixty_changed_lines() {
        let mut lines: Vec<String> = Vec::new();
        for f in 0..4 {
            lines.push(format!("+fn f{f}() {{"));
            for i in 0..29 {
                lines.push(format!("+    step_{f}_{i}();"));
            }
            lines.push("+}".into());
        }
        let l: Vec<&str> = lines.iter().map(String::as_str).collect();
        let (ws, _) = windows_of(&diff(&[("src/a.rs", &l)]));
        assert_eq!(ws.len(), 2, "{:?}", ws.iter().map(|w| w.changed).collect::<Vec<_>>());
        assert!(ws[0].text.trim_end().ends_with("+}"));
        assert_eq!(ws[0].changed, 62);
    }

    #[test]
    fn classes_lock_generated_whitespace_and_test() {
        let (mut ws, _) = windows_of(&diff(&[("Cargo.lock", &["-a = 1", "+a = 2"])]));
        classify(&mut ws[0], true, false, false);
        assert_eq!(ws[0].class, "lock");
        classify(&mut ws[0], false, true, false);
        assert_eq!(ws[0].class, "generated");
        let (ws, _) = windows_of(&diff(&[("src/a.rs", &["-let x=1;", "+let x = 1;"])]));
        assert_eq!(ws[0].class, "whitespace");
        let (ws, _) = windows_of(&diff(&[("tests/detach.rs", &["-old();", "+new();"])]));
        assert_eq!(ws[0].class, "test");
        let (ws, _) = windows_of(&diff(&[(
            "src/lib.rs",
            &[" #[cfg(test)]", " mod tests {", "-    old();", "+    new();"],
        )]));
        assert_eq!(ws[0].class, "test", "inside a test module");
    }

    /// A test module's marker is seldom inside a hunk: where it starts is
    /// read from the file, so a change far below it is still test code.
    #[test]
    fn a_change_below_a_test_module_the_hunk_does_not_show_is_test_code() {
        let patch = diff(&[(
            "src/record.rs",
            &["     let item = &r.items[0];", "-    assert_eq!(item.uncited(), 1);", " }"],
        )]);
        let mut files = parse(&patch, ".");
        files[0].test_from = (Some(1), Some(1));
        let moves = detect_moves(&mut files);
        let mut ws: Vec<Window> = file_windows(&files[0], &moves);
        classify(&mut ws[0], false, false, false);
        assert_eq!(ws[0].class, "test");
        let found = integrity(&ws);
        assert_eq!(found[0]["kind"], "assert_removed", "{found:?}");
        files[0].test_from = (None, None);
        let mut ws: Vec<Window> = file_windows(&files[0], &moves);
        classify(&mut ws[0], false, false, false);
        assert_eq!(ws[0].class, "code", "no test module: not test code");
    }

    #[test]
    fn a_removed_test_and_a_removed_assert_are_found() {
        let (ws, _) = windows_of(&diff(&[(
            "tests/uptime.rs",
            &[
                "-#[test]",
                "-fn uptime_counts_seconds() {",
                "-    assert_eq!(uptime(), 1);",
                "-}",
                " #[test]",
                " fn uptime_is_positive() {",
                "-    assert!(uptime() > 0);",
                "+    let _ = uptime();",
                " }",
            ],
        )]));
        let found = integrity(&ws);
        let kinds: Vec<&str> = found.iter().map(|f| f["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["test_removed", "assert_removed"], "{found:?}");
        assert_eq!(found[0]["name"], "uptime_counts_seconds");
        assert_eq!(found[1]["removed"], 2);
    }

    #[test]
    fn a_moved_test_is_not_a_removed_one() {
        let body = [
            "#[test]",
            "fn uptime_counts_seconds_since_start() {",
            "    assert_eq!(uptime_since(start), 1);",
            "}",
        ];
        let r: Vec<String> = body.iter().map(|l| format!("-{l}")).collect();
        let a: Vec<String> = body.iter().map(|l| format!("+{l}")).collect();
        let r: Vec<&str> = r.iter().map(String::as_str).collect();
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        let (ws, _) = windows_of(&diff(&[("src/a.rs", &r), ("tests/a.rs", &a)]));
        assert_eq!(integrity(&ws), Vec::<Value>::new());
    }

    #[test]
    fn keys_are_the_long_names_and_paths_in_backticks() {
        let k = keys(&[
            "The send waits on the tick: `type_it` (`crates/weftd/src/server.rs:462`) and `Pane::gone_quiet`; `tick`.",
        ]);
        assert!(k.contains("type_it"));
        assert!(k.contains("crates/weftd/src/server.rs"));
        assert!(k.contains("gone_quiet"));
        assert!(!k.contains("tick"), "too short to be rare");
    }

    #[test]
    fn retrieval_picks_the_windows_that_name_an_obligations_rare_keys() {
        let lines: Vec<String> =
            (0..120).map(|i| format!("+fn common_{i}() {{ shared_helper(); }}")).collect();
        let mut l: Vec<&str> = lines.iter().map(String::as_str).collect();
        l.push("+fn gone_quiet() -> bool { true }");
        let (ws, _) = windows_of(&diff(&[
            ("src/pane.rs", &l),
            ("src/other.rs", &["+fn unrelated_change() {}"]),
        ]));
        let obligations = vec![(
            "plan.md#Phase 1/2".to_string(),
            keys(&["`Pane::gone_quiet` decides", "`shared_helper` everywhere"]),
        )];
        let (sets, unclaimed) = retrieve(&obligations, &ws);
        let set = &sets["plan.md#Phase 1/2"];
        let chosen: Vec<&str> =
            set["windows"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let with_key: Vec<&str> =
            ws.iter().filter(|w| w.text.contains("gone_quiet")).map(|w| w.id.as_str()).collect();
        assert_eq!(chosen, with_key, "only the rare key's window; shared_helper is everywhere");
        assert!(set["bytes"].as_u64().unwrap() as usize <= BUDGET);
        let other = ws.iter().find(|w| w.path == "src/other.rs").unwrap();
        assert!(unclaimed.contains(&other.id));
    }

    #[test]
    fn grep_names_the_window_path_and_line() {
        let (ws, _) =
            windows_of(&diff(&[("src/a.rs", &[" keep();", "-old_call();", "+new_call();"])]));
        let hits = grep_window(&ws[0].id, &ws[0].text, "NEW_CALL");
        assert_eq!(hits, [format!("{} src/a.rs:2 +new_call();", ws[0].id)]);
        let hits = grep_window(&ws[0].id, &ws[0].text, "old_call");
        assert_eq!(hits, [format!("{} src/a.rs:2 -old_call();", ws[0].id)]);
    }

    #[test]
    fn relative_names_a_sibling_repository() {
        assert_eq!(relative(Path::new("/w/fab7/weft"), Path::new("/w/fab7/fab7")), "../fab7");
        assert_eq!(relative(Path::new("/w/a"), Path::new("/w/a")), ".");
    }
}

#[cfg(test)]
mod open_tests {
    use super::*;
    use crate::evaluate::{Open, open_eval};
    use crate::testing::{commit, confirm_ask, eval_bench, repo, two_asks_and_work};

    fn evidence_of(ws: &Workspace, out: &Value) -> Value {
        let id = str_of(out, "eval_id");
        serde_json::from_slice(
            &std::fs::read(ws.rf_dir().join(format!("evals/{id}/evidence.json"))).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn open_publishes_the_windows_and_the_ledger_stays_whole() {
        eval_bench(|ws| {
            two_asks_and_work(ws);
            let out = open_eval(ws, Open::default()).unwrap();
            let id = str_of(&out, "eval_id");
            let ev = evidence_of(ws, &out);
            assert_eq!(ev["repos"][0]["name"], ".");
            let windows = ev["windows"].as_array().unwrap();
            let paths: BTreeSet<&str> =
                windows.iter().map(|w| w["path"].as_str().unwrap()).collect();
            assert_eq!(paths, ["docs/notes.md", "src/uptime.js", "tests/uptime.test.js"].into());
            let test = windows.iter().find(|w| w["path"] == "tests/uptime.test.js").unwrap();
            assert_eq!(test["class"], "test");
            let first = str_of(&windows[0], "id");
            assert!(window_text(ws, &id, &first).unwrap().starts_with("--- "));
            refused(window_text(ws, &id, "w_nope").unwrap_err(), "eval.window_unknown");
            let hits = grep(ws, &id, "uptime = ()").unwrap();
            assert_eq!(hits.len(), 1, "{hits:?}");
            assert!(hits[0].contains("src/uptime.js:1"), "{hits:?}");
            let brief: Value = serde_json::from_slice(
                &std::fs::read(ws.rf_dir().join(format!("evals/{id}/brief.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(brief["evidence"]["windows"], json!(windows.len()));
            assert_eq!(
                store::verify(ws).unwrap(),
                Vec::<Value>::new(),
                "every artifact referenced"
            );
        });
    }

    fn refused(e: EvalError, code: &str) {
        let t = e.to_string();
        assert!(t.contains(code), "wanted {code}, got {t}");
    }

    /// RingFrame gathers the change as the Eval opens; the harness that
    /// opened it is on record, and no agent maps it again.
    #[test]
    fn an_eval_opened_by_a_harness_is_gathered_as_it_opens() {
        eval_bench(|ws| {
            two_asks_and_work(ws);
            let out = open_eval(ws, Open { host: Some("codex"), ..Default::default() }).unwrap();
            let id = str_of(&out, "eval_id");
            let gathered = store::events(ws)
                .unwrap()
                .into_iter()
                .find(|e| e["type"] == "eval.gathered" && e["id"] == json!(id))
                .unwrap();
            assert_eq!(gathered["data"]["host"], "codex");
            assert_eq!(
                gathered["data"]["context_map"]["path"],
                format!("evals/{id}/evidence.json")
            );
            let again =
                crate::evaluate::gather(ws, &id, b"## a map\n", "claude-code", None).unwrap_err();
            refused(again, "eval.already_gathered");
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
        eval_bench(|ws| {
            two_asks_and_work(ws);
            open_eval(ws, Open::default()).unwrap();
            assert!(
                store::events(ws).unwrap().iter().all(|e| e["type"] != "eval.gathered"),
                "no harness named, nothing claimed"
            );
        });
    }

    #[test]
    fn a_repository_an_ask_names_is_read_from_its_commit_at_the_ask() {
        eval_bench(|ws| {
            let other = repo();
            let before = commit(other.path(), &[("products/x.toml", Some("a = 1\n"))], "base");
            let named = other.path().join("products/x.toml");
            let prompt = format!("Change {} and the server.\n", named.display());
            let ask = confirm_ask(ws, "Both", b"change both\n", prompt.as_bytes());
            let baselines = store::events(ws)
                .unwrap()
                .into_iter()
                .find(|e| e["type"] == "ask.compiled" && e["id"] == ask["ask_id"])
                .unwrap()["data"]["baselines"]
                .clone();
            let repos: Vec<&str> =
                baselines.as_array().unwrap().iter().map(|b| b["repo"].as_str().unwrap()).collect();
            assert_eq!(repos.len(), 2, "{baselines}");
            assert_eq!(repos[0], ".");
            assert_eq!(baselines[1]["commit"], before);
            assert_eq!(baselines[1]["dirty"], false);
            // Work made any way at all: a commit, then an edit left on disk.
            commit(other.path(), &[("products/y.toml", Some("b = 1\n"))], "more");
            std::fs::write(&named, "a = 2\n").unwrap();
            commit(
                &ws.root,
                &[("src/uptime.js", Some("export const uptime = () => 1;\n"))],
                "work",
            );
            let out = open_eval(ws, Open::default()).unwrap();
            let ev = evidence_of(ws, &out);
            let name = str_of(&ev["repos"][1], "name");
            assert_eq!(ev["repos"][1]["anchor"], json!({"ref": before, "from": "ask"}));
            let paths: BTreeSet<String> = ev["windows"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|w| w["repo"] == json!(name))
                .map(|w| str_of(w, "path"))
                .collect();
            assert_eq!(
                paths,
                [format!("{name}/products/x.toml"), format!("{name}/products/y.toml")].into()
            );
            assert_eq!(store::verify(ws).unwrap(), Vec::<Value>::new());
        });
    }

    /// The mechanism is `git diff` from the commit each repository was at: what
    /// was uncommitted then is in the diff, so the Eval says so.
    #[test]
    fn uncommitted_work_at_the_ask_is_named_as_a_limitation() {
        eval_bench(|ws| {
            let other = repo();
            commit(other.path(), &[("a.md", Some("one\n"))], "base");
            std::fs::write(other.path().join("a.md"), "left from before\n").unwrap();
            std::fs::write(ws.root.join("scratch.txt"), "also before\n").unwrap();
            let prompt = format!("Tidy {}.\n", other.path().join("a.md").display());
            confirm_ask(ws, "Tidy", b"tidy\n", prompt.as_bytes());
            let out = open_eval(ws, Open::default()).unwrap();
            let id = str_of(&out, "eval_id");
            let brief: Value = serde_json::from_slice(
                &std::fs::read(ws.rf_dir().join(format!("evals/{id}/brief.json"))).unwrap(),
            )
            .unwrap();
            let says = |needle: &str| {
                brief["limitations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|l| l.as_str().unwrap().contains(needle))
            };
            assert!(says(
                "the workspace had uncommitted changes when the first open Ask was compiled"
            ));
            assert!(says("had uncommitted changes when an Ask named it"));
        });
    }

    #[test]
    fn a_repository_named_with_repo_is_read_from_the_anchor_given() {
        eval_bench(|ws| {
            let other = repo();
            let base = commit(other.path(), &[("a.md", Some("one\n"))], "base");
            commit(other.path(), &[("a.md", Some("two\n"))], "work");
            two_asks_and_work(ws);
            let spec = format!("{}={base}", other.path().display());
            let out = open_eval(ws, Open { repos: vec![spec], ..Default::default() }).unwrap();
            let ev = evidence_of(ws, &out);
            assert_eq!(ev["repos"][1]["anchor"]["from"], "explicit");
            assert!(
                ev["windows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|w| str_of(w, "path").ends_with("/a.md"))
            );
        });
        eval_bench(|ws| {
            let other = repo();
            two_asks_and_work(ws);
            let bad =
                format!("{}=0000000000000000000000000000000000000000", other.path().display());
            let out = open_eval(ws, Open { repos: vec![bad], ..Default::default() }).unwrap();
            let ev = evidence_of(ws, &out);
            assert_eq!(
                ev["repos"].as_array().unwrap().len(),
                1,
                "a repository with no such anchor is not read"
            );
            let id = str_of(&out, "eval_id");
            let brief: Value = serde_json::from_slice(
                &std::fs::read(ws.rf_dir().join(format!("evals/{id}/brief.json"))).unwrap(),
            )
            .unwrap();
            assert!(
                brief["limitations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|l| { l.as_str().unwrap().contains("is not a commit there") })
            );
        });
    }

    #[test]
    fn an_ask_that_names_a_plan_part_gets_its_rows_and_their_evidence() {
        eval_bench(|ws| {
            let plan = "## Phase 1\n\n| # | Step | Done when |\n| --- | --- | --- |\n| 1 | Add `uptime_seconds` to the server. | A test calls `uptime_seconds`. |\n| 2 | Document it. | `docs/notes.md` says so. |\n";
            commit(&ws.root, &[("plans/plan.md", Some(plan))], "plan");
            confirm_ask(
                ws,
                "Build it",
                b"build phase 1\n",
                b"Build Phase 1 of plans/plan.md, steps 1 to 2.\n",
            );
            commit(
                &ws.root,
                &[
                    ("src/uptime.js", Some("export const uptime_seconds = () => 1;\n")),
                    ("docs/notes.md", Some("uptime\n")),
                ],
                "work",
            );
            let out = open_eval(ws, Open::default()).unwrap();
            let ev = evidence_of(ws, &out);
            let ids: Vec<String> =
                ev["obligations"].as_array().unwrap().iter().map(|o| str_of(o, "id")).collect();
            assert_eq!(ids, ["plans/plan.md#Phase 1/1", "plans/plan.md#Phase 1/2"]);
            let set = &ev["evidence_sets"]["plans/plan.md#Phase 1/1"]["windows"];
            let win = ev["windows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|w| w["path"] == "src/uptime.js")
                .unwrap();
            assert_eq!(set, &json!([win["id"]]));
            assert!(
                !ev["trace"].as_array().unwrap().contains(&win["id"]),
                "claimed windows are not traced"
            );
        });
    }
}
