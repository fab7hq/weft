//! Which files an Eval's readers should and should not read, checked against
//! what `.gitignore` and `.gitattributes` say now. Each finding is a folder
//! or file that would fill what the readers read, with the one line that
//! keeps it out: ignored when it is output nobody committed, marked generated
//! when it is committed data, media or build output. A file a person writes
//! (code, docs, config) is never given a line. Read with Git's listings
//! alone; nothing is written until the person takes a line (`apply`).
//!
//! The one bar is RingFrame's own reading capacity, the patch limit an
//! Eval's change is cut at: a folder or file is a finding when its text,
//! written as a new file's diff, is over that limit (alone it would make a
//! change large), or, in a work's range, when the range is over the limit
//! and would not be without it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::evaluate::{self, EvalError, PATCH_LIMIT, git, git_bytes, git_input, str_of};
use crate::workspace::Workspace;

/// A line that keeps a finding out of what the readers read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    /// `.gitignore` or `.gitattributes`, at the project's root.
    pub file: String,
    /// The line, without its newline.
    pub line: String,
}

/// Something the readers would read that git could be told to skip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// `untracked_bulk`, `data_bulk`, `committed_media`, `committed_build`, `large_text` or
    /// `sub_repo`.
    pub kind: &'static str,
    /// `work`, seen in the range of the work asked about, or `project`,
    /// seen in the project as it is.
    pub scope: &'static str,
    /// The folder (with a trailing `/`, and `*` where siblings are taken
    /// together) or file, from the project's root.
    pub path: String,
    /// How many files it holds; for a nested repository, how many have
    /// uncommitted changes.
    pub files: usize,
    /// The text the readers would read of it, at least.
    pub bytes: u64,
    /// For a nested repository, its commits since the work began.
    pub commits: usize,
    /// Its files by extension, the most first.
    pub kinds_of_files: Vec<(String, usize)>,
    /// None when no line can keep it out without hiding something a person
    /// wrote, or when nothing to write would help (a nested repository).
    pub fix: Option<Fix>,
}

const IGNORE: &str = ".gitignore";
const ATTRIBUTES: &str = ".gitattributes";

/// Data: a file of records, or a model's or array's weights.
const DATA: &[&str] = &[
    "json",
    "jsonl",
    "ndjson",
    "geojson",
    "csv",
    "tsv",
    "parquet",
    "arrow",
    "feather",
    "avro",
    "orc",
    "npy",
    "npz",
    "pkl",
    "pickle",
    "h5",
    "hdf5",
    "safetensors",
    "pt",
    "pth",
    "ckpt",
    "onnx",
    "gguf",
    "tfrecord",
    "sqlite",
    "sqlite3",
    "db",
    "log",
];

/// Media and packed files: images, video, audio, documents, archives, fonts,
/// compiled objects.
const MEDIA: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico", "icns", "heic", "avif",
    "svg", "psd", "mp4", "mov", "webm", "mkv", "avi", "m4v", "mp3", "wav", "flac", "ogg", "m4a",
    "aac", "opus", "pdf", "zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "jar", "whl",
    "woff", "woff2", "ttf", "otf", "eot", "wasm", "so", "dylib", "dll", "exe", "o", "a", "class",
    "pyc",
];

/// Folders build tools and package managers write into.
const BUILD_DIRS: &[&str] = &[
    "dist",
    "build",
    "target",
    "out",
    "node_modules",
    "__pycache__",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    "coverage",
    ".venv",
    "venv",
    "site-packages",
    "storybook-static",
];

/// What people write and build tools do not: code, prose, configuration.
/// A folder holding one is never given a line. A file with no extension
/// (`Makefile`, `LICENSE`) counts as one.
const WRITTEN: &[&str] = &[
    "rs", "ts", "tsx", "jsx", "py", "go", "java", "kt", "kts", "swift", "c", "h", "cc", "cpp",
    "cxx", "hpp", "cs", "fs", "rb", "php", "md", "mdx", "rst", "adoc", "txt", "toml", "yaml",
    "yml", "ini", "cfg", "sh", "bash", "zsh", "fish", "ps1", "sql", "vue", "svelte", "astro",
    "lua", "ex", "exs", "erl", "zig", "dart", "scala", "clj", "hs", "ml", "nim", "r", "jl",
    "proto", "graphql", "gql", "tex", "bib", "nix", "tf", "scss", "sass", "less", "ipynb",
];

/// The pathspecs that leave out what `.gitattributes` already marks, so a
/// range's diff is read without it.
const UNMARKED: &[&str] = &[
    ":(exclude,attr:linguist-generated)",
    ":(exclude,attr:linguist-generated=true)",
    ":(exclude,attr:linguist-vendored)",
    ":(exclude,attr:linguist-vendored=true)",
];

/// The findings for the project at `ws`, and, with `ask`, for the range of
/// the work that Ask is in. Empty outside Git, or when Git cannot be read.
pub fn check(ws: &Workspace, ask: Option<&str>) -> Vec<Finding> {
    let Some(project) = Project::read(&ws.root) else { return Vec::new() };
    let units = project.units();
    let mut found = Vec::new();
    let range = ask.and_then(|a| crate::reads::range(ws, a).ok());
    if let Some(range) = &range {
        let costs = project.range_costs(&ws.root, range);
        found.extend(project.findings(&units, &costs, "work"));
    }
    let costs: HashMap<String, u64> =
        project.files.iter().map(|f| (f.path.clone(), f.cost())).collect();
    for f in project.findings(&units, &costs, "project") {
        if !found.iter().any(|g: &Finding| g.kind == f.kind && g.path == f.path) {
            found.push(f);
        }
    }
    // A file inside a folder already found is said with its folder.
    let folders: Vec<String> =
        found.iter().filter(|f| f.path.ends_with('/')).map(|f| f.path.clone()).collect();
    found.retain(|f| f.path.ends_with('/') || !folders.iter().any(|d| under(&f.path, d)));
    found.sort_by(|a, b| {
        (a.scope != "work", a.kind == "sub_repo", std::cmp::Reverse(a.bytes), &a.path).cmp(&(
            b.scope != "work",
            b.kind == "sub_repo",
            std::cmp::Reverse(b.bytes),
            &b.path,
        ))
    });
    found.extend(sub_repos(&ws.root, &project, range.as_ref()));
    found
}

/// Add `fix`'s line to its file at the project's root, made when absent. A
/// line already there is not added again; the file's content is kept byte
/// for byte, a newline put after its last line when it has none.
pub fn apply(ws: &Workspace, fix: &Fix) -> Result<(), EvalError> {
    evaluate::need(
        fix.file == IGNORE || fix.file == ATTRIBUTES,
        "readcheck.file",
        format!("a line goes in {IGNORE} or {ATTRIBUTES}, not {}", fix.file),
    )?;
    let line = fix.line.trim_end();
    evaluate::need(
        !line.trim().is_empty() && !line.contains(['\n', '\r']),
        "readcheck.line",
        "a fix is one line",
    )?;
    let path = ws.root.join(&fix.file);
    let fail =
        |e: std::io::Error| evaluate::ledger("readcheck.io", format!("{}: {e}", path.display()));
    let had = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(fail(e)),
    };
    if String::from_utf8_lossy(&had).lines().any(|l| l.trim_end() == line) {
        return Ok(());
    }
    let mut out = had;
    if !out.is_empty() && !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.extend_from_slice(line.as_bytes());
    out.push(b'\n');
    std::fs::write(&path, out).map_err(fail)
}

/// One file of the project, tracked or untracked and not ignored.
struct File {
    path: String,
    tracked: bool,
    /// Read as text: what Git diffs line by line.
    text: bool,
    size: u64,
    /// Already counted and not read: marked generated or vendored, or a
    /// lock file.
    skipped: bool,
}

impl File {
    /// The text the readers would read were it new: its diff header and its
    /// text, at least (each line also gains a `+`).
    fn cost(&self) -> u64 {
        if self.skipped || !self.text { 0 } else { self.size + header_len(&self.path) }
    }

    fn sort(&self) -> Sort {
        let ext = ext_of(&self.path);
        if self.written() {
            Sort::Other
        } else if DATA.contains(&ext.as_str()) {
            Sort::Data
        } else if MEDIA.contains(&ext.as_str()) {
            Sort::Media
        } else if !self.text && self.size > 0 {
            // Git sees it as data: not text, and of no kind named above.
            Sort::Data
        } else {
            Sort::Other
        }
    }

    fn written(&self) -> bool {
        let ext = ext_of(&self.path);
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        (ext.is_empty() && !name.starts_with('.') && self.text)
            || (WRITTEN.contains(&ext.as_str()) && !name.ends_with(".d.ts"))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Data,
    Media,
    Other,
}

/// The project as Git lists it: its files, its folders, which folders are
/// wholly untracked, and the repositories inside it.
struct Project {
    files: Vec<File>,
    /// Folders with no tracked file, as Git collapses them, nested
    /// repositories left out.
    untracked_dirs: Vec<String>,
    /// Folders holding another repository: untracked, ignored or a
    /// submodule.
    nested: Vec<String>,
}

/// A folder or file that could be given a line.
struct Unit {
    path: String,
    kind: &'static str,
    /// The line it would be given, if any can be.
    line: Option<Fix>,
    /// The folders it takes together (more than one for siblings).
    members: Vec<String>,
}

impl Project {
    fn read(root: &Path) -> Option<Project> {
        let listed = |args: &[&str]| -> Option<Vec<String>> {
            let out = git_bytes(root, args, &[0]).ok()?;
            Some(
                String::from_utf8_lossy(&out)
                    .split('\0')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
            )
        };
        // Four listings, each walking the worktree or the index, at once.
        let (staged, untracked, open_dirs, ignored_dirs) = std::thread::scope(|s| {
            let staged = s.spawn(|| listed(&["ls-files", "-s", "-z"]));
            let untracked = s.spawn(|| listed(&["ls-files", "-o", "--exclude-standard", "-z"]));
            let open_dirs = s.spawn(|| {
                listed(&[
                    "ls-files",
                    "-o",
                    "--exclude-standard",
                    "--directory",
                    "--no-empty-directory",
                    "-z",
                ])
            });
            let ignored_dirs = s.spawn(|| {
                listed(&["ls-files", "-o", "-i", "--exclude-standard", "--directory", "-z"])
            });
            let join =
                |h: std::thread::ScopedJoinHandle<'_, Option<Vec<String>>>| h.join().ok().flatten();
            (join(staged), join(untracked), join(open_dirs), join(ignored_dirs))
        });
        let (staged, untracked, open_dirs, ignored_dirs) =
            (staged?, untracked?, open_dirs?, ignored_dirs?);
        let mut tracked = BTreeSet::new();
        let mut nested = BTreeSet::new();
        for entry in &staged {
            let Some((meta, path)) = entry.split_once('\t') else { continue };
            if meta.starts_with("160000") {
                nested.insert(format!("{path}/"));
            } else {
                tracked.insert(path.to_string());
            }
        }
        let mut untracked_dirs = Vec::new();
        for d in open_dirs.iter().chain(&ignored_dirs).filter(|d| d.ends_with('/')) {
            if root.join(d).join(".git").exists() {
                nested.insert(d.clone());
            } else if open_dirs.contains(d) {
                untracked_dirs.push(d.clone());
            }
        }

        let paths: Vec<&str> = tracked.iter().chain(&untracked).map(String::as_str).collect();
        let attrs = attributes(root, &paths);
        let mut files = Vec::new();
        for (path, is_tracked) in
            tracked.iter().map(|p| (p, true)).chain(untracked.iter().map(|p| (p, false)))
        {
            let full = root.join(path);
            let meta = std::fs::symlink_metadata(&full).ok();
            let size = meta.as_ref().filter(|m| m.is_file()).map_or(0, |m| m.len());
            let a = attrs.get(path.as_str());
            let name = path.rsplit('/').next().unwrap_or(path);
            let skipped =
                a.is_some_and(|a| a.marked) || crate::evidence::LOCK_FILES.contains(&name);
            // What is skipped already is never read, so not opened.
            let text = if skipped {
                false
            } else if is_tracked {
                // As Git judges: no NUL in the first 8000 bytes, unless the
                // attributes say it has no text diff.
                !a.is_some_and(|a| a.no_diff) && size > 0 && starts_as_text(&full, 8000)
            } else {
                evaluate::untracked_is_text(&full)
            };
            files.push(File { path: path.clone(), tracked: is_tracked, text, size, skipped });
        }
        Some(Project { files, untracked_dirs, nested: nested.into_iter().collect() })
    }

    /// Every folder or file that could be given a line, judged on the whole
    /// project so a line never hides a file a person wrote anywhere it
    /// reaches.
    fn units(&self) -> Vec<Unit> {
        // Per folder: whether everything in it is data or media, whether it
        // holds a written file, and how many data and media files.
        #[derive(Default)]
        struct Held {
            tracked: bool,
            other: bool,
            written: bool,
            data: usize,
            media: usize,
        }
        let mut held: BTreeMap<String, Held> = BTreeMap::new();
        for f in &self.files {
            let sort = f.sort();
            for dir in folders_of(&f.path) {
                let h = held.entry(dir.to_string()).or_default();
                h.tracked |= f.tracked;
                // An empty file (`.gitkeep`) holds nothing a line could hide.
                if !f.skipped && f.size > 0 {
                    h.other |= sort == Sort::Other;
                    h.written |= f.written();
                }
                match sort {
                    Sort::Data => h.data += 1,
                    Sort::Media => h.media += 1,
                    Sort::Other => {}
                }
            }
        }
        let pure = |d: &str| held.get(d).is_some_and(|h| !h.other);
        let kind_of = |dirs: &[String]| {
            let (data, media) = dirs
                .iter()
                .filter_map(|d| held.get(d))
                .fold((0, 0), |(d, m), h| (d + h.data, m + h.media));
            if data >= media { "data_bulk" } else { "committed_media" }
        };
        let mut units = Vec::new();

        // Build output: the outermost folder a build tool writes, holding
        // nothing a person wrote.
        let builds: Vec<String> = held
            .iter()
            .filter(|(d, h)| {
                h.tracked
                    && !h.written
                    && BUILD_DIRS.contains(&last_name(d))
                    && !folders_of(d)
                        .any(|up| up != d.as_str() && BUILD_DIRS.contains(&last_name(up)))
            })
            .map(|(d, _)| d.clone())
            .collect();
        for d in &builds {
            units.push(Unit {
                path: d.clone(),
                kind: "committed_build",
                line: marked_line(d),
                members: vec![d.clone()],
            });
        }

        // Data and media: the outermost folders holding nothing else.
        let tops: Vec<String> = held
            .keys()
            .filter(|d| pure(d) && parent_of(d).is_none_or(|p| !pure(p)))
            // A folder Git tracks nothing in is output to ignore, below.
            .filter(|d| held.get(*d).is_some_and(|h| h.tracked))
            .filter(|d| !builds.iter().any(|b| under(d, b)))
            .cloned()
            .collect();
        // Siblings taken together: folders alike but for one name, when
        // every folder of that shape holds data or media alone.
        let mut shapes: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for d in &tops {
            let names: Vec<&str> = d.trim_end_matches('/').split('/').collect();
            for i in 0..names.len() {
                let mut shape = names.clone();
                shape[i] = "*";
                shapes.entry(format!("{}/", shape.join("/"))).or_default().push(d.clone());
            }
        }
        let mut shapes: Vec<(String, Vec<String>)> =
            shapes.into_iter().filter(|(_, m)| m.len() > 1).collect();
        shapes.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
        for (shape, members) in shapes {
            let every_one_pure = held.keys().filter(|d| matches_shape(d, &shape)).all(|d| pure(d));
            if every_one_pure {
                units.push(Unit {
                    kind: kind_of(&members),
                    line: if members.iter().all(|m| plain(m)) {
                        Some(Fix {
                            file: ATTRIBUTES.into(),
                            line: format!("{shape}** linguist-generated=true"),
                        })
                    } else {
                        None
                    },
                    path: shape,
                    members,
                });
            }
        }
        for d in &tops {
            units.push(Unit {
                path: d.clone(),
                kind: kind_of(std::slice::from_ref(d)),
                line: marked_line(d),
                members: vec![d.clone()],
            });
        }

        // Output nobody committed: a folder Git tracks nothing in.
        for d in &self.untracked_dirs {
            let written = self.files.iter().any(|f| !f.tracked && under(&f.path, d) && f.written());
            units.push(Unit {
                path: d.clone(),
                kind: "untracked_bulk",
                line: (!written && plain(d))
                    .then(|| Fix { file: IGNORE.into(), line: format!("/{d}") }),
                members: vec![d.clone()],
            });
        }

        // One file alone.
        for f in self.files.iter().filter(|f| !f.skipped && f.text) {
            let line = (f.sort() != Sort::Other && !f.written() && plain(&f.path)).then(|| Fix {
                file: ATTRIBUTES.into(),
                line: format!("{} linguist-generated=true", f.path),
            });
            units.push(Unit {
                path: f.path.clone(),
                kind: "large_text",
                line,
                members: vec![f.path.clone()],
            });
        }
        units
    }

    /// What each file of a work's range adds to what the readers read: a
    /// tracked file its diff, an untracked one its text as a new file. A
    /// file the range deletes is in it too: it adds to the range's total.
    fn range_costs(&self, root: &Path, range: &crate::reads::Range) -> HashMap<String, u64> {
        let skipped: BTreeSet<&str> =
            self.files.iter().filter(|f| f.skipped).map(|f| f.path.as_str()).collect();
        let binary: BTreeSet<&str> =
            range.tracked.iter().filter(|(_, text)| !text).map(|(p, _)| p.as_str()).collect();
        let mut costs: HashMap<String, u64> = HashMap::new();
        for (path, bytes) in diff_bytes_by_file(root, &range.anchor) {
            if !skipped.contains(path.as_str()) && !binary.contains(path.as_str()) {
                *costs.entry(path).or_default() += bytes;
            }
        }
        let by_path: HashMap<&str, &File> =
            self.files.iter().map(|f| (f.path.as_str(), f)).collect();
        for name in &range.untracked {
            if let Some(f) = by_path.get(name.as_str()) {
                *costs.entry(name.clone()).or_default() += f.cost();
            }
        }
        costs
    }

    /// The units over the bar, given what each file costs the readers in
    /// this scope.
    fn findings(
        &self,
        units: &[Unit],
        costs: &HashMap<String, u64>,
        scope: &'static str,
    ) -> Vec<Finding> {
        let limit = PATCH_LIMIT as u64;
        // Per folder and file: its files, its cost, and its kinds of files.
        let mut per: HashMap<&str, (usize, u64, BTreeMap<String, usize>)> = HashMap::new();
        for f in self.files.iter().filter(|f| !f.skipped) {
            let cost = costs.get(f.path.as_str()).copied();
            if scope == "work" && cost.is_none() {
                continue;
            }
            let cost = cost.unwrap_or(0);
            for at in folders_of(&f.path).chain([f.path.as_str()]) {
                let e = per.entry(at).or_default();
                e.0 += 1;
                e.1 += cost;
                *e.2.entry(ext_of(&f.path)).or_default() += 1;
            }
        }
        let mut measured: Vec<(&Unit, usize, u64, BTreeMap<String, usize>)> = Vec::new();
        for u in units {
            let (mut files, mut bytes, mut kinds) = (0, 0, BTreeMap::new());
            for m in &u.members {
                if let Some((n, b, k)) = per.get(m.as_str()) {
                    files += n;
                    bytes += b;
                    for (ext, c) in k {
                        *kinds.entry(ext.clone()).or_default() += c;
                    }
                }
            }
            if files > 0 && bytes > 0 {
                measured.push((u, files, bytes, kinds));
            }
        }
        // The largest first, so a folder is said before what is inside it.
        measured.sort_by_key(|m| std::cmp::Reverse(m.2));

        // What the range would still hand the readers, with every line
        // taken so far.
        let mut left: u64 = costs.values().sum();
        let mut found: Vec<Finding> = Vec::new();
        let mut taken: Vec<&str> = Vec::new();
        for (u, files, bytes, kinds) in measured {
            if u.members.iter().any(|m| taken.iter().any(|t| under(m, t))) {
                continue;
            }
            // Alone it would make a change large; or, in a work, the change
            // is large and with this line it would not be.
            let alone = bytes > limit;
            let tips = scope == "work" && u.line.is_some() && left > limit && left - bytes <= limit;
            if !alone && !tips {
                continue;
            }
            taken.extend(u.members.iter().map(String::as_str));
            if u.line.is_some() {
                left = left.saturating_sub(bytes);
            }
            let mut kinds: Vec<(String, usize)> = kinds.into_iter().collect();
            kinds.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            found.push(Finding {
                kind: u.kind,
                scope,
                path: u.path.clone(),
                files,
                bytes,
                commits: 0,
                kinds_of_files: kinds,
                fix: u.line.clone(),
            });
        }
        found
    }
}

/// The nested repositories with changes the range cannot see: uncommitted
/// files, and, for a work, commits since it began. One the work's Asks named
/// is read by its Eval, and is not said.
fn sub_repos(root: &Path, project: &Project, range: Option<&crate::reads::Range>) -> Vec<Finding> {
    let seen: BTreeSet<PathBuf> = range
        .into_iter()
        .flat_map(|r| &r.asks)
        .flat_map(|a| a["baselines"].as_array().into_iter().flatten())
        .map(|b| str_of(b, "root"))
        .filter_map(|r| PathBuf::from(r).canonicalize().ok())
        .collect();
    let began = range.and_then(|r| r.began);
    let scope = if range.is_some() { "work" } else { "project" };
    let mut found: Vec<Finding> = std::thread::scope(|s| {
        let handles: Vec<_> = project
            .nested
            .iter()
            .filter(|d| root.join(d).canonicalize().is_ok_and(|c| !seen.contains(&c)))
            .map(|d| {
                s.spawn(move || {
                    let dir = root.join(d);
                    let status = git_bytes(
                        &dir,
                        &["-c", "core.fsmonitor=false", "status", "--porcelain=v1", "-z"],
                        &[0],
                    )
                    .ok()?;
                    let changed = changed_paths(&String::from_utf8_lossy(&status));
                    let commits = began.map_or(0, |ms| {
                        git(
                            &dir,
                            &["rev-list", "--count", &format!("--since=@{}", ms / 1000), "HEAD"],
                        )
                        .ok()
                        .and_then(|n| n.trim().parse().ok())
                        .unwrap_or(0)
                    });
                    if changed.is_empty() && commits == 0 {
                        return None;
                    }
                    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
                    for p in &changed {
                        *kinds.entry(ext_of(p)).or_default() += 1;
                    }
                    let mut kinds: Vec<(String, usize)> = kinds.into_iter().collect();
                    kinds.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                    Some(Finding {
                        kind: "sub_repo",
                        scope,
                        path: d.clone(),
                        files: changed.len(),
                        bytes: 0,
                        commits,
                        kinds_of_files: kinds,
                        fix: None,
                    })
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
    });
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// What `.gitattributes` says of each path: marked generated or vendored,
/// and whether it is diffed as text. The paths go on Git's input: a project
/// can hold more of them than a command line takes.
struct Attrs {
    marked: bool,
    no_diff: bool,
}

fn attributes<'a>(root: &Path, paths: &[&'a str]) -> HashMap<&'a str, Attrs> {
    let mut out: HashMap<&'a str, Attrs> = HashMap::new();
    if paths.is_empty() {
        return out;
    }
    let mut input = Vec::new();
    for p in paths {
        input.extend_from_slice(p.as_bytes());
        input.push(0);
    }
    let args = ["check-attr", "--stdin", "-z", "linguist-generated", "linguist-vendored", "diff"];
    let Some(text) = git_input(root, &args, &input) else { return out };
    let text = String::from_utf8_lossy(&text);
    let by_name: HashMap<&str, &'a str> = paths.iter().map(|p| (*p, *p)).collect();
    let parts: Vec<&str> = text.split('\0').collect();
    for c in parts.chunks(3).filter(|c| c.len() == 3) {
        let Some(&path) = by_name.get(c[0]) else { continue };
        let a = out.entry(path).or_insert(Attrs { marked: false, no_diff: false });
        match (c[1], c[2]) {
            ("linguist-generated" | "linguist-vendored", "set" | "true") => a.marked = true,
            ("diff", "unset") => a.no_diff = true,
            _ => {}
        }
    }
    out
}

/// The bytes each file adds to the range's diff from `anchor`, what
/// `.gitattributes` marks left out, read in one pass.
fn diff_bytes_by_file(root: &Path, anchor: &str) -> Vec<(String, u64)> {
    let mut args = vec![
        "-c",
        "core.quotePath=false",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--relative",
        anchor,
        "--",
        ".",
    ];
    args.extend(UNMARKED);
    let Ok(mut child) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let mut lines = std::io::BufReader::new(child.stdout.take().expect("piped"));
    let mut out: Vec<(String, u64)> = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        match lines.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if let Some(header) = line.strip_prefix(b"diff --git ") {
            let header = String::from_utf8_lossy(header);
            let path = header.trim_end().rsplit_once(" b/").map_or("", |(_, p)| p);
            out.push((path.to_string(), 0));
        }
        if let Some(last) = out.last_mut() {
            last.1 += line.len() as u64;
        }
    }
    let _ = child.wait();
    out
}

/// Whether a file's first `n` bytes hold no NUL, as Git tells text from
/// binary.
fn starts_as_text(path: &Path, n: usize) -> bool {
    let mut head = vec![0u8; n];
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut read = 0;
    while read < n {
        match f.read(&mut head[read..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => read += k,
        }
    }
    !head[..read].contains(&0)
}

/// How long the header Git writes before a new text file's lines is.
fn header_len(path: &str) -> u64 {
    format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\nindex 0000000..0000000\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1 @@\n"
    )
    .len() as u64
}

/// A file's extension, lower case; empty when it has none.
fn ext_of(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// Every folder a path is in, the outermost first, each with its trailing
/// `/`. A folder path's own folder is among them.
fn folders_of(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(move |(i, _)| &path[..=i])
}

fn parent_of(dir: &str) -> Option<&str> {
    let inner = dir.trim_end_matches('/');
    inner.rfind('/').map(|i| &dir[..=i])
}

fn last_name(dir: &str) -> &str {
    dir.trim_end_matches('/').rsplit('/').next().unwrap_or(dir)
}

/// Whether `path` is `dir` or inside it; a `*` in `dir` stands for any one
/// name.
fn under(path: &str, dir: &str) -> bool {
    if !dir.contains('*') {
        return path.starts_with(dir);
    }
    let names: Vec<&str> = path.split('/').collect();
    let shape: Vec<&str> = dir.trim_end_matches('/').split('/').collect();
    names.len() > shape.len() && names.iter().zip(&shape).all(|(n, s)| *s == "*" || n == s)
}

/// The paths `git status --porcelain=v1 -z` names, each once: a rename's
/// or copy's old path follows it and is not counted again.
fn changed_paths(status: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut entries = status.split('\0');
    while let Some(e) = entries.next() {
        let (Some(xy), Some(path)) = (e.get(..2), e.get(3..)) else { continue };
        if xy.starts_with(['R', 'C']) {
            let _from = entries.next();
        }
        if !path.is_empty() {
            out.push(path.to_string());
        }
    }
    out
}

/// Whether a folder is of `shape`: as deep, each name the same but where
/// the shape has `*`.
fn matches_shape(dir: &str, shape: &str) -> bool {
    let a: Vec<&str> = dir.trim_end_matches('/').split('/').collect();
    let b: Vec<&str> = shape.trim_end_matches('/').split('/').collect();
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| *y == "*" || x == y)
}

/// Whether a path can go in a line as it is: no space, quote, backslash or
/// pattern character, and no `#` or `!` first.
fn plain(path: &str) -> bool {
    !path.chars().any(|c| c.is_whitespace() || "\"\\*?[]".contains(c))
        && !path.starts_with(['#', '!'])
}

fn marked_line(dir: &str) -> Option<Fix> {
    plain(dir)
        .then(|| Fix { file: ATTRIBUTES.into(), line: format!("{dir}** linguist-generated=true") })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{commit, repo, run, ws_for};

    /// About `bytes` of text in lines, as data or code would be.
    fn text(bytes: usize) -> String {
        let line = "{\"id\": 1, \"name\": \"a record of some data\", \"ok\": true}\n";
        line.repeat(bytes / line.len() + 1)
    }

    /// `n` files of `each` bytes under `dir`, named `<i>.<ext>`.
    fn many(dir: &str, ext: &str, n: usize, each: usize) -> Vec<(String, String)> {
        (0..n).map(|i| (format!("{dir}{i}.{ext}"), text(each))).collect()
    }

    fn commit_all(root: &Path, files: &[(String, String)]) {
        let files: Vec<(&str, Option<&str>)> =
            files.iter().map(|(p, t)| (p.as_str(), Some(t.as_str()))).collect();
        commit(root, &files, "files");
    }

    fn write_all(root: &Path, files: &[(String, String)]) {
        for (p, t) in files {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        }
    }

    /// Over the limit, spread over files that are each under it.
    const OVER: usize = PATCH_LIMIT / 30 + 1024;

    fn lines(found: &[Finding]) -> Vec<(&str, &str)> {
        found
            .iter()
            .filter_map(|f| f.fix.as_ref())
            .map(|x| (x.file.as_str(), x.line.as_str()))
            .collect()
    }

    fn kinds(found: &[Finding]) -> Vec<(&str, &str)> {
        found.iter().map(|f| (f.kind, f.path.as_str())).collect()
    }

    fn attr(root: &Path, path: &str) -> String {
        let out = git(root, &["check-attr", "linguist-generated", "--", path]).unwrap();
        out.trim().rsplit(": ").next().unwrap_or_default().to_string()
    }

    #[test]
    fn a_clean_project_has_no_finding() {
        let r = repo();
        let ws = ws_for(r.path());
        commit_all(
            r.path(),
            &[
                ("src/main.rs".into(), "fn main() {}\n".into()),
                ("package.json".into(), "{\"name\": \"x\"}\n".into()),
                ("assets/logo.png".into(), "\u{89}PNG\0\0".into()),
                ("Cargo.lock".into(), text(2 * PATCH_LIMIT)),
            ],
        );
        std::fs::write(r.path().join("notes.md"), "a note\n").unwrap();
        assert_eq!(check(&ws, None), []);
    }

    /// Code, prose and configuration a person wrote get no line, however
    /// much of it there is.
    #[test]
    fn source_is_never_given_a_line() {
        let r = repo();
        let ws = ws_for(r.path());
        let mut files = many("crates/a/src/", "rs", 30, OVER);
        files.extend(many("plans/x/", "md", 30, OVER));
        files.extend(many("web/src/", "ts", 30, OVER));
        files.extend(many("build/", "rs", 30, OVER));
        files.push(("src/huge.rs".into(), text(PATCH_LIMIT + 1)));
        files.push(("docs/big.md".into(), text(PATCH_LIMIT + 1)));
        commit_all(r.path(), &files);
        write_all(r.path(), &many("new/src/", "rs", 30, OVER));
        let found = check(&ws, None);
        assert_eq!(lines(&found), [], "{found:#?}");
        // The two large files and the untracked code are said, with no line.
        let mut said = kinds(&found);
        said.sort();
        assert_eq!(
            said,
            [
                ("large_text", "docs/big.md"),
                ("large_text", "src/huge.rs"),
                ("untracked_bulk", "new/")
            ]
        );
    }

    #[test]
    fn an_untracked_folder_of_output_is_ignored() {
        let r = repo();
        let ws = ws_for(r.path());
        commit_all(r.path(), &[("src/main.rs".into(), "fn main() {}\n".into())]);
        write_all(r.path(), &many("runs/a/", "log", 30, OVER));
        let found = check(&ws, None);
        assert_eq!(kinds(&found), [("untracked_bulk", "runs/")]);
        assert_eq!(found[0].files, 30);
        assert!(found[0].bytes > PATCH_LIMIT as u64);
        assert_eq!(found[0].kinds_of_files, [("log".to_string(), 30)]);
        let fix = found[0].fix.clone().unwrap();
        assert_eq!((fix.file.as_str(), fix.line.as_str()), (".gitignore", "/runs/"));
        apply(&ws, &fix).unwrap();
        assert!(git(r.path(), &["check-ignore", "runs/a/0.log"]).is_ok());
        assert_eq!(check(&ws, None), [], "ignored, it is not said again");
    }

    #[test]
    fn data_committed_in_bulk_is_marked_generated() {
        let r = repo();
        let ws = ws_for(r.path());
        let mut files = many("data/set/", "jsonl", 30, OVER);
        files.push(("data/set/.gitkeep".into(), String::new()));
        files.push(("src/main.rs".into(), "fn main() {}\n".into()));
        commit_all(r.path(), &files);
        let found = check(&ws, None);
        assert_eq!(kinds(&found), [("data_bulk", "data/")], "{found:#?}");
        let fix = found[0].fix.clone().unwrap();
        assert_eq!(fix.line, "data/** linguist-generated=true");
        apply(&ws, &fix).unwrap();
        assert_eq!(attr(r.path(), "data/set/0.jsonl"), "true");
        assert_eq!(attr(r.path(), "src/main.rs"), "unspecified");
        assert_eq!(check(&ws, None), [], "marked, it is not said again");
    }

    /// Folders alike but for one name are taken together when every folder
    /// of that shape holds data alone, and not when one holds a note.
    #[test]
    fn sibling_data_folders_share_one_line() {
        let r = repo();
        let ws = ws_for(r.path());
        let mut files = many("papers/a/evidence/", "json", 15, OVER);
        files.extend(many("papers/b/evidence/", "csv", 15, OVER));
        files.push(("papers/a/main.md".into(), "# A\n".into()));
        files.push(("papers/b/main.md".into(), "# B\n".into()));
        commit_all(r.path(), &files);
        let found = check(&ws, None);
        assert_eq!(
            lines(&found),
            [(".gitattributes", "papers/*/evidence/** linguist-generated=true")]
        );
        assert_eq!(found[0].files, 30);

        commit_all(r.path(), &[("papers/c/evidence/notes.md".into(), "why\n".into())]);
        let found = check(&ws, None);
        assert_eq!(found, [], "neither half is over the limit alone: {found:#?}");
    }

    #[test]
    fn committed_build_output_is_marked_generated() {
        let r = repo();
        let ws = ws_for(r.path());
        let mut files = many("web/dist/assets/", "js", 30, OVER);
        files.push(("web/dist/index.html".into(), "<html></html>\n".into()));
        files.push(("web/src/main.ts".into(), "export {}\n".into()));
        commit_all(r.path(), &files);
        let found = check(&ws, None);
        assert_eq!(kinds(&found), [("committed_build", "web/dist/")]);
        assert_eq!(lines(&found), [(".gitattributes", "web/dist/** linguist-generated=true")]);

        // Media whose text the readers would read: a folder of drawings.
        let r = repo();
        let ws = ws_for(r.path());
        commit_all(r.path(), &many("icons/", "svg", 30, OVER));
        let found = check(&ws, None);
        assert_eq!(kinds(&found), [("committed_media", "icons/")]);
        assert_eq!(lines(&found), [(".gitattributes", "icons/** linguist-generated=true")]);
    }

    #[test]
    fn one_large_data_file_is_marked_alone() {
        let r = repo();
        let ws = ws_for(r.path());
        commit_all(
            r.path(),
            &[
                ("data/big.csv".into(), text(PATCH_LIMIT + 1)),
                ("data/README.md".into(), "What the data is.\n".into()),
                ("data/small.json".into(), text(1000)),
            ],
        );
        let found = check(&ws, None);
        assert_eq!(kinds(&found), [("large_text", "data/big.csv")]);
        assert_eq!(lines(&found), [(".gitattributes", "data/big.csv linguist-generated=true")]);
        apply(&ws, found[0].fix.as_ref().unwrap()).unwrap();
        assert_eq!(attr(r.path(), "data/small.json"), "unspecified");
        assert_eq!(check(&ws, None), []);
    }

    #[test]
    fn what_git_already_skips_is_not_said() {
        let r = repo();
        let ws = ws_for(r.path());
        std::fs::write(r.path().join(".gitignore"), "/out/\n").unwrap();
        let mut files = many("data/", "json", 30, OVER);
        files.push((".gitattributes".into(), "data/** linguist-vendored\n".into()));
        files.push(("img/big.bin".into(), "\0".repeat(2 * PATCH_LIMIT)));
        commit_all(r.path(), &files);
        write_all(r.path(), &many("out/", "log", 30, OVER));
        assert_eq!(check(&ws, None), [], "ignored, marked, or not text");
    }

    #[test]
    fn a_nested_repository_with_changes_is_said() {
        let r = repo();
        let ws = ws_for(r.path());
        std::fs::write(r.path().join(".gitignore"), "/quiet/\n").unwrap();
        commit_all(r.path(), &[("src/main.rs".into(), "fn main() {}\n".into())]);
        for name in ["inner", "quiet", "clean"] {
            let dir = r.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let d = dir.to_string_lossy().to_string();
            run(&["git", "init", "-q", &d]);
            std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
            run(&["git", "-C", &d, "add", "-A"]);
            run(&[
                "git",
                "-C",
                &d,
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "a",
            ]);
        }
        std::fs::write(r.path().join("inner/a.rs"), "fn a() { 1; }\n").unwrap();
        std::fs::write(r.path().join("inner/b.ts"), "export {}\n").unwrap();
        std::fs::write(r.path().join("quiet/c.md"), "c\n").unwrap();
        let found = check(&ws, None);
        assert_eq!(kinds(&found), [("sub_repo", "inner/"), ("sub_repo", "quiet/")]);
        assert_eq!((found[0].files, found[0].commits, found[0].fix.clone()), (2, 0, None));
    }

    /// In a work's range, a folder is said when the range is large and
    /// would not be without it, though the folder alone is not over the
    /// limit; a nested repository with commits since the work began is
    /// said, unless an Ask of the work named it.
    #[test]
    fn a_works_range_is_checked_too() {
        crate::testing::eval_bench(|ws| {
            let (a, _, _) = crate::testing::two_asks_and_work(ws);
            let mut files = many("fixtures/", "json", 20, PATCH_LIMIT / 40);
            files.extend(many("src/", "rs", 20, PATCH_LIMIT / 40));
            commit_all(&ws.root, &files);
            assert_eq!(check(ws, None), [], "the project alone is under the bar");
            let found = check(ws, Some(&a));
            assert_eq!(kinds(&found), [("data_bulk", "fixtures/")], "{found:#?}");
            assert_eq!(found[0].scope, "work");
            assert_eq!(lines(&found), [(".gitattributes", "fixtures/** linguist-generated=true")]);

            let dir = ws.root.join("lib");
            let d = dir.to_string_lossy().to_string();
            run(&["git", "init", "-q", &d]);
            std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
            run(&["git", "-C", &d, "add", "-A"]);
            run(&[
                "git",
                "-C",
                &d,
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "a",
            ]);
            let found = check(ws, Some(&a));
            let nested: Vec<&Finding> = found.iter().filter(|f| f.kind == "sub_repo").collect();
            assert_eq!(nested.len(), 1, "{found:#?}");
            assert_eq!(
                (nested[0].path.as_str(), nested[0].commits, nested[0].files),
                ("lib/", 1, 0)
            );
            assert!(
                check(ws, None).iter().all(|f| f.kind != "sub_repo"),
                "clean, it has no change"
            );
        });
    }

    #[test]
    fn a_line_is_added_once_keeping_the_file() {
        let r = repo();
        let ws = ws_for(r.path());
        let fix =
            Fix { file: ".gitattributes".into(), line: "data/** linguist-generated=true".into() };
        apply(&ws, &fix).unwrap();
        let path = r.path().join(".gitattributes");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "data/** linguist-generated=true\n");
        apply(&ws, &fix).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "data/** linguist-generated=true\n");

        let ignore = r.path().join(".gitignore");
        std::fs::write(&ignore, b"# kept\r\n*.tmp  \n/last").unwrap();
        let fix = Fix { file: ".gitignore".into(), line: "/runs/".into() };
        apply(&ws, &fix).unwrap();
        apply(&ws, &fix).unwrap();
        assert_eq!(std::fs::read(&ignore).unwrap(), b"# kept\r\n*.tmp  \n/last\n/runs/\n");

        for (file, line) in [("src/x", "/runs/"), (".gitignore", "a\nb"), (".gitignore", " ")] {
            let fix = Fix { file: file.into(), line: line.into() };
            assert!(apply(&ws, &fix).is_err(), "{file}: {line:?}");
        }
    }

    #[test]
    fn folders_and_shapes() {
        assert_eq!(folders_of("a/b/c.rs").collect::<Vec<_>>(), ["a/", "a/b/"]);
        assert!(under("p/x/e/1.json", "p/*/e/"));
        assert!(!under("p/x/f/1.json", "p/*/e/"));
        assert!(under("p/x/1.json", "p/"));
        assert_eq!(ext_of("a/.gitignore"), "");
        assert_eq!(ext_of("a/B.JSON"), "json");
        assert_eq!(
            changed_paths(" M a.rs\0R  new.rs\0old.rs\0?? dir/\0"),
            ["a.rs", "new.rs", "dir/"]
        );
        assert!(!plain("my data/") && !plain("a[1]/") && plain("a-b_c.d/"));
    }
}
