//! What an Eval of a work would read, said before it opens: where its range
//! starts, how many commits and files it holds, the files it counts but does
//! not read, where most of them are, and whether it is more than the readers
//! are given whole. Read with Git's listings alone; nothing is written.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::evaluate::{self, EvalError, git, need, str_of};
use crate::workspace::Workspace;

/// The files a range counts and does not read, by why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Skipped {
    /// Marked `linguist-generated` in `.gitattributes`.
    pub generated: usize,
    /// Marked `linguist-vendored`.
    pub vendored: usize,
    /// Not text: an image, an archive, a text file too large to read.
    pub binary: usize,
    /// A package manager's lock file.
    pub lock: usize,
}

impl Skipped {
    pub fn total(&self) -> usize {
        self.generated + self.vendored + self.binary + self.lock
    }
}

/// What the Eval of a work would read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalPreview {
    /// Where the range starts: `ask`, the base of the work's first Ask, or
    /// `seal`, its last Seal.
    pub since: &'static str,
    /// When that was recorded.
    pub when: Option<String>,
    /// Commits in the range that touch this project.
    pub commits: usize,
    /// Files in the range, the untracked ones made since the work began among them.
    pub files: usize,
    pub skipped: Skipped,
    /// The folder holding the most of the range's files, with a trailing
    /// `/`, and how many; empty when every file is at the top.
    pub folder: String,
    pub folder_files: usize,
    /// More text than the readers are given whole: past RingFrame's patch
    /// limit, a change's patch is cut and the rest is not read.
    pub large: bool,
    /// The later work the change ends at; none when it reads to now.
    pub until: Option<Until>,
}

/// A work's range: where it starts, and the files in it.
pub(crate) struct Range {
    pub since: &'static str,
    pub when: Option<String>,
    /// The commit the range starts from.
    pub anchor: String,
    /// When the work began, in milliseconds since the epoch.
    pub began: Option<i64>,
    /// The open Asks of the work.
    pub asks: Vec<Value>,
    /// The tracked files changed since the anchor, and whether Git counts
    /// their lines (a binary file's are not).
    pub tracked: Vec<(String, bool)>,
    /// The untracked files made since the work began.
    pub untracked: Vec<String>,
    /// Where the work's own change ends, when a later work began on top of
    /// its commits: that commit, the Ask that began there and when.
    pub until: Option<(String, Until)>,
}

/// The later work a work's own change ends at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Until {
    /// The first Ask of that work, its title, and when it was made.
    pub ask: String,
    pub title: String,
    pub time: String,
}

/// The range of the work `ask` is in, as its Eval would read it now.
pub(crate) fn range(ws: &Workspace, ask: &str) -> Result<Range, EvalError> {
    let unit = crate::ask::unit_of(ws, ask)?;
    let asks: Vec<Value> = crate::ask::open_asks(ws)?
        .into_iter()
        .filter(|a| unit.contains(&str_of(a, "ask_id")))
        .collect();
    need(
        !asks.is_empty(),
        "eval.no_open_ask",
        "nothing to evaluate: every Ask is sealed or cancelled",
    )?;
    git(&ws.root, &["rev-parse", "--git-dir"]).map_err(|_| {
        evaluate::ledger(
            "eval.no_git",
            "Eval reads the Git delta; this workspace is not a Git repository",
        )
    })?;
    let anchor = evaluate::anchor_of(ws, &asks, Some(&unit), None)?;
    let anchor_ref = str_of(&anchor, "ref");
    let (since, when) = if anchor["kind"] == "seal" {
        ("seal", evaluate::anchor_time(ws, &anchor))
    } else {
        let asked = asks
            .iter()
            .find(|a| a.get("base_commit").and_then(Value::as_str) == Some(anchor_ref.as_str()));
        ("ask", asked.map(|a| str_of(a, "time")).filter(|t| !t.is_empty()))
    };
    let began = evaluate::anchor_since(ws, &asks, &anchor);
    let root = ws.root.as_path();
    let next = evaluate::next_unit(ws, &unit, &anchor_ref, "worktree", "")?;
    let anchor_ref = evaluate::commit_of(root, &anchor_ref)?;
    let (numstat, untracked) = match &next {
        Some(n) => (
            git(root, &["diff", "--numstat", "-z", "--relative", &anchor_ref, &n.end, "--", "."])?,
            Vec::new(),
        ),
        None => (
            git(root, &["diff", "--numstat", "-z", "--relative", &anchor_ref, "--", "."])?,
            crate::untracked::listed(root, began)?,
        ),
    };
    Ok(Range {
        since,
        when,
        anchor: anchor_ref,
        began,
        asks,
        tracked: numstat_files(&numstat),
        untracked,
        until: next.map(|n| (n.end, Until { ask: n.ask, title: n.title, time: n.time })),
    })
}

/// What the Eval of the work `ask` is in would read now.
pub fn preview(ws: &Workspace, ask: &str) -> Result<EvalPreview, EvalError> {
    let Range { since, when, anchor: anchor_ref, tracked, untracked, until, .. } = range(ws, ask)?;
    let root = ws.root.as_path();
    let end = until.as_ref().map_or("HEAD", |(end, _)| end.as_str());
    let refs: Vec<&str> = match &until {
        Some((end, _)) => vec![anchor_ref.as_str(), end.as_str()],
        None => vec![anchor_ref.as_str()],
    };
    let commits = git(root, &["rev-list", "--count", &format!("{anchor_ref}..{end}"), "--", "."])?
        .trim()
        .parse()
        .unwrap_or(0);

    // Every file in the range: tracked ones as Git counts their lines, `-`
    // for a binary one; then the untracked ones made since the work began.
    let mut binary: BTreeSet<String> =
        tracked.iter().filter(|(_, text)| !text).map(|(p, _)| p.clone()).collect();
    let mut paths: Vec<String> = tracked.into_iter().map(|(p, _)| p).collect();
    let mut untracked_bytes = 0usize;
    for name in untracked {
        let path = root.join(&name);
        if evaluate::untracked_is_text(&path) {
            untracked_bytes += std::fs::metadata(&path).map_or(0, |m| m.len() as usize);
        } else {
            binary.insert(name.clone());
        }
        paths.push(name);
    }
    paths.sort();
    paths.dedup();

    let mut skipped = Skipped::default();
    let mut not_read: BTreeSet<&str> = BTreeSet::new();
    for (p, kind) in why_not_read(root, &paths, &binary)? {
        *match kind {
            "lock" => &mut skipped.lock,
            "generated" => &mut skipped.generated,
            "vendored" => &mut skipped.vendored,
            _ => &mut skipped.binary,
        } += 1;
        not_read.insert(p);
    }
    let (folder, folder_files) = most_in(&paths);
    let limit = evaluate::PATCH_LIMIT;
    let large = untracked_bytes > limit
        || diff_bytes(root, &refs, &not_read, limit - untracked_bytes)? > limit - untracked_bytes;
    Ok(EvalPreview {
        since,
        when,
        commits,
        files: paths.len(),
        skipped,
        folder,
        folder_files,
        large,
        until: until.map(|(_, u)| u),
    })
}

/// Files an Eval counted and did not read, of one kind, in one folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotRead {
    /// `generated`, `vendored`, `binary` or `lock`.
    pub kind: &'static str,
    /// The folder they are in, from the project's root, with a trailing
    /// `/`: the deepest that holds them all. The file itself when it is one;
    /// empty for several at the top.
    pub folder: String,
    pub files: usize,
}

/// The kinds of file counted and not read, in the order they are said.
const NOT_READ: [&str; 4] = ["generated", "vendored", "binary", "lock"];

/// Each of `paths` that is counted and not read, with why: a lock file, one
/// marked generated or vendored in `.gitattributes`, or one in `binary`, in
/// that order.
fn why_not_read<'a>(
    root: &Path,
    paths: &'a [String],
    binary: &BTreeSet<String>,
) -> Result<Vec<(&'a str, &'static str)>, EvalError> {
    let (generated, vendored) = marked(root, paths)?;
    Ok(paths
        .iter()
        .filter_map(|p| {
            let name = p.rsplit('/').next().unwrap_or(p);
            let kind = if crate::evidence::LOCK_FILES.contains(&name) {
                "lock"
            } else if generated.contains(p) {
                "generated"
            } else if vendored.contains(p) {
                "vendored"
            } else if binary.contains(p) {
                "binary"
            } else {
                return None;
            };
            Some((p.as_str(), kind))
        })
        .collect())
}

/// What the change from `anchor` to `commit`, or to the worktree with the
/// untracked files made since `began`, holds that an Eval counts and does
/// not read, by kind and folder: each kind's largest first.
pub(crate) fn not_read(
    root: &Path,
    anchor: &str,
    commit: Option<&str>,
    began: Option<i64>,
) -> Result<Vec<NotRead>, EvalError> {
    let mut args = vec!["diff", "--numstat", "-z", "--relative", anchor];
    args.extend(commit);
    args.extend(["--", "."]);
    let tracked = numstat_files(&git(root, &args)?);
    let mut binary: BTreeSet<String> =
        tracked.iter().filter(|(_, text)| !text).map(|(p, _)| p.clone()).collect();
    let mut paths: Vec<String> = tracked.into_iter().map(|(p, _)| p).collect();
    if commit.is_none() {
        for name in crate::untracked::listed(root, began)? {
            if !evaluate::untracked_is_text(&root.join(&name)) {
                binary.insert(name.clone());
            }
            paths.push(name);
        }
    }
    paths.sort();
    paths.dedup();
    let mut by_kind: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (p, kind) in why_not_read(root, &paths, &binary)? {
        by_kind.entry(kind).or_default().push(p);
    }
    let mut out = Vec::new();
    for kind in NOT_READ {
        let mut groups = folders(by_kind.remove(kind).unwrap_or_default());
        groups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.extend(groups.into_iter().map(|(folder, files)| NotRead { kind, folder, files }));
    }
    Ok(out)
}

/// `paths` gathered by the folder at the top they are in, each group said by
/// the deepest folder that holds all of it, or by its one file. The files
/// at the top are one group, said by its one file or as empty.
fn folders(paths: Vec<&str>) -> Vec<(String, usize)> {
    let mut tops: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for p in paths {
        let top = p.split_once('/').map_or("", |(t, _)| t);
        tops.entry(top).or_default().push(p);
    }
    tops.into_iter()
        .map(|(top, group)| {
            if let [one] = group.as_slice() {
                return (one.to_string(), 1);
            }
            if top.is_empty() {
                return (String::new(), group.len());
            }
            let mut at = format!("{top}/");
            loop {
                let next: BTreeSet<&str> = group
                    .iter()
                    .map(|p| &p[at.len()..])
                    .map(|rest| rest.split_once('/').map_or("", |(d, _)| d))
                    .collect();
                match next.into_iter().collect::<Vec<_>>().as_slice() {
                    [one] if !one.is_empty() => at = format!("{at}{one}/"),
                    _ => return (at, group.len()),
                }
            }
        })
        .collect()
}

/// Each file `git diff --numstat -z` lists, and whether Git counted its
/// lines (a binary file's are `-`). A rename names the file as it is now.
fn numstat_files(listing: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut fields = listing.split('\0');
    while let Some(entry) = fields.next() {
        let mut cols = entry.splitn(3, '\t');
        let (Some(added), Some(_), Some(path)) = (cols.next(), cols.next(), cols.next()) else {
            continue;
        };
        let path = if path.is_empty() {
            // `-z` puts a rename's two paths in the fields that follow.
            let _from = fields.next();
            fields.next().unwrap_or_default().to_string()
        } else {
            path.to_string()
        };
        if !path.is_empty() {
            out.push((path, added != "-"));
        }
    }
    out
}

/// The paths marked generated, and those marked vendored, in
/// `.gitattributes`. The paths go on Git's input: a range can hold more of
/// them than a command line takes.
fn marked(
    root: &Path,
    paths: &[String],
) -> Result<(BTreeSet<String>, BTreeSet<String>), EvalError> {
    let (mut generated, mut vendored) = (BTreeSet::new(), BTreeSet::new());
    if paths.is_empty() {
        return Ok((generated, vendored));
    }
    let fail = |e: std::io::Error| evaluate::ledger("eval.no_git", format!("git: {e}"));
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-attr", "--stdin", "-z", "linguist-generated", "linguist-vendored"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(fail)?;
    let mut input = Vec::new();
    for p in paths {
        input.extend_from_slice(p.as_bytes());
        input.push(0);
    }
    let mut stdin = child.stdin.take().expect("piped");
    // Written beside the read, so neither side waits on a full pipe.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output().map_err(fail)?;
    let _ = writer.join();
    let text = String::from_utf8_lossy(&out.stdout);
    let parts: Vec<&str> = text.split('\0').collect();
    for c in parts.chunks(3).filter(|c| c.len() == 3 && matches!(c[2], "set" | "true")) {
        match c[1] {
            "linguist-generated" => generated.insert(c[0].to_string()),
            _ => vendored.insert(c[0].to_string()),
        };
    }
    Ok((generated, vendored))
}

/// The folder holding the most of `paths`: the top folder with the most,
/// then, inside it, the folder with the most for as long as that one holds
/// more than half of them all, or all its parent holds. None when every path
/// is at the top.
fn most_in(paths: &[String]) -> (String, usize) {
    let mut at = String::new();
    let mut held = 0;
    loop {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for p in paths.iter().filter_map(|p| p.strip_prefix(at.as_str())) {
            if let Some((folder, _)) = p.split_once('/') {
                *counts.entry(folder).or_default() += 1;
            }
        }
        // The first of the largest, in order, so the answer does not move.
        let best = counts.iter().fold(None, |best: Option<(&str, usize)>, (f, n)| match best {
            Some((_, m)) if m >= *n => best,
            _ => Some((f, *n)),
        });
        match best {
            Some((folder, n)) if at.is_empty() || n * 2 > paths.len() || n == held => {
                at = format!("{at}{folder}/");
                held = n;
            }
            _ => return (at, held),
        }
    }
}

/// How many bytes of the range's diff the readers would read, the files in
/// `skip` left out: counted until past `room`, then Git is stopped.
fn diff_bytes(
    root: &Path,
    refs: &[&str],
    skip: &BTreeSet<&str>,
    room: usize,
) -> Result<usize, EvalError> {
    let fail = |e: std::io::Error| evaluate::ledger("eval.no_git", format!("git: {e}"));
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--no-color", "--no-ext-diff", "--relative"])
        .args(refs)
        .args(["--", "."])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(fail)?;
    let mut lines = std::io::BufReader::new(child.stdout.take().expect("piped"));
    let (mut bytes, mut counted, mut line) = (0usize, true, Vec::new());
    loop {
        line.clear();
        if lines.read_until(b'\n', &mut line).map_err(fail)? == 0 || bytes > room {
            break;
        }
        if let Some(header) = line.strip_prefix(b"diff --git ") {
            let header = String::from_utf8_lossy(header);
            let path = header.trim_end().rsplit_once(" b/").map_or("", |(_, p)| p);
            counted = !skip.contains(path);
        }
        if counted {
            bytes += line.len();
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{commit, eval_bench};

    #[test]
    fn the_folder_with_the_most_is_followed_down_while_it_holds_most() {
        let paths: Vec<String> =
            ["a.rs", "src/x.rs", "data/e/1/a.json", "data/e/2/b.json", "data/e/c.json"]
                .iter()
                .map(|p| p.to_string())
                .collect();
        assert_eq!(most_in(&paths), ("data/e/".to_string(), 3));
        let spread: Vec<String> =
            ["a/1", "b/1", "b/2", "c/1"].iter().map(|p| p.to_string()).collect();
        assert_eq!(most_in(&spread), ("b/".to_string(), 2));
        assert_eq!(most_in(&["top.rs".to_string()]), (String::new(), 0));
    }

    #[test]
    fn what_is_not_read_is_said_by_the_deepest_folder_that_holds_it() {
        let said = folders(vec![
            "research/papers/x/evidence/X01/a.json",
            "research/papers/x/evidence/X02/b.json",
            "weft/Cargo.lock",
            "logo.png",
            "art/a.png",
            "art/b/c.png",
        ]);
        assert_eq!(
            said,
            [
                ("logo.png".to_string(), 1),
                ("art/".to_string(), 2),
                ("research/papers/x/evidence/".to_string(), 2),
                ("weft/Cargo.lock".to_string(), 1),
            ]
        );
        assert_eq!(folders(vec!["a.png", "b.png"]), [(String::new(), 2)], "several at the top");
    }

    /// An opened Eval records what its readers count and do not read, by
    /// kind and folder: each kind's largest first, the untracked files made
    /// for the work among them.
    #[test]
    fn an_opened_eval_records_what_it_did_not_read() {
        eval_bench(|ws| {
            crate::testing::two_asks_and_work(ws);
            let mut files: Vec<(String, String)> = (0..3)
                .map(|i| (format!("data/run/{i}/out.json"), format!("{{\"n\": {i}}}\n")))
                .collect();
            files.push(("data/one.json".into(), "{}\n".into()));
            files.push((".gitattributes".into(), "data/** linguist-generated\n".into()));
            files.push(("web/package-lock.json".into(), "{}\n".into()));
            let files: Vec<(&str, Option<&str>)> =
                files.iter().map(|(p, t)| (p.as_str(), Some(t.as_str()))).collect();
            commit(&ws.root, &files, "data");
            std::fs::create_dir_all(ws.root.join("art")).unwrap();
            for name in ["art/a.png", "art/b.png", "logo.png"] {
                std::fs::write(ws.root.join(name), [0x89u8, b'P', b'N', b'G', 0, 0, 1]).unwrap();
            }
            let out = crate::evaluate::open_eval(ws, Default::default()).unwrap();
            let said: Vec<(String, String, u64)> = out["not_read"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| (str_of(n, "kind"), str_of(n, "folder"), n["files"].as_u64().unwrap()))
                .collect();
            let row = |k: &str, f: &str, n: u64| (k.to_string(), f.to_string(), n);
            assert_eq!(
                said,
                [
                    row("generated", "data/", 4),
                    row("binary", "art/", 2),
                    row("binary", "logo.png", 1),
                    row("lock", "web/package-lock.json", 1),
                ]
            );
            let opened = crate::store::events(ws)
                .unwrap()
                .into_iter()
                .rfind(|e| e["type"] == "eval.opened")
                .unwrap();
            assert_eq!(opened["data"]["not_read"], out["not_read"], "on the record");
        });
    }

    #[test]
    fn a_rename_is_named_as_it_is_now() {
        let listing = "1\t2\tsrc/a.rs\0-\t-\timg.png\x000\t0\t\0old.rs\0new.rs\0";
        assert_eq!(
            numstat_files(listing),
            [("src/a.rs".into(), true), ("img.png".into(), false), ("new.rs".into(), true)]
        );
    }

    /// The range since the work's first Ask: its commits and files, the
    /// files counted and not read by why, and the folder most of it is in.
    /// Nothing is written.
    #[test]
    fn an_evals_reads_are_said_before_it_opens() {
        eval_bench(|ws| {
            crate::testing::two_asks_and_work(ws);
            let ask = str_of(&crate::ask::open_asks(ws).unwrap()[0], "ask_id");
            let before = crate::store::events(ws).unwrap().len();
            let data: Vec<(String, String)> = (0..5)
                .map(|i| (format!("data/set/{i}.json"), format!("{{\"n\": {i}}}\n")))
                .collect();
            let mut files: Vec<(&str, Option<&str>)> =
                data.iter().map(|(p, t)| (p.as_str(), Some(t.as_str()))).collect();
            files.extend([
                (".gitattributes", Some("gen/** linguist-generated\nthird/** linguist-vendored\n")),
                ("gen/out.rs", Some("pub fn made() {}\n")),
                ("third/lib.js", Some("var x = 1;\n")),
                ("Cargo.lock", Some("version = 3\n")),
            ]);
            commit(&ws.root, &files, "more work");
            std::fs::write(ws.root.join("logo.png"), [0x89u8, b'P', b'N', b'G', 0, 0, 1]).unwrap();
            let seen = preview(ws, &ask).unwrap();
            assert_eq!(seen.since, "ask");
            assert!(seen.when.is_some());
            assert_eq!(seen.commits, 2, "the work's commit and this one");
            assert_eq!(
                seen.skipped,
                Skipped { generated: 1, vendored: 1, binary: 1, lock: 1 },
                "{seen:?}"
            );
            assert_eq!((seen.folder.as_str(), seen.folder_files), ("data/set/", 5));
            assert!(!seen.large);
            // The work's three files, the five data files, the attributes,
            // the three marked or locked, and the image.
            assert_eq!(seen.files, 3 + 5 + 1 + 3 + 1, "{seen:?}");
            assert_eq!(crate::store::events(ws).unwrap().len(), before, "nothing is recorded");

            // Past the patch limit, it is large; marked generated, it is not.
            let big = "x".repeat(4096) + "\n";
            let lines: String = std::iter::repeat_n(big.as_str(), 300).collect();
            commit(&ws.root, &[("dump/big.txt", Some(&lines))], "a big file");
            assert!(preview(ws, &ask).unwrap().large);
            commit(
                &ws.root,
                &[(
                    ".gitattributes",
                    Some("gen/** linguist-generated\ndump/** linguist-generated\n"),
                )],
                "mark it",
            );
            assert!(!preview(ws, &ask).unwrap().large);
        });
    }
}
