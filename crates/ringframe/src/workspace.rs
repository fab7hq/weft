//! Workspace root resolution and the `.fab7/rf/` directory.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

/// Every file that crosses the boundary names the schema it is written to, and
/// these are the versions this release reads. Nothing else gates on a version.
pub const BUNDLE_SCHEMA: &str = "ringframe.bundle/2";
pub const PROFILE_SCHEMA: &str = "ringframe.profile/1";
pub const DELTAS_SCHEMA: &str = "ringframe.deltas/1";

pub const BUNDLE_REPO: &str = "fab7hq/fab7";
/// This CLI's subtree in the marketplace.
pub const BUNDLE_PRODUCT: &str = "ringframe";
/// The marketplace releases as a whole, so tags are plain versions.
pub const BUNDLE_TAG_PREFIX: &str = "v";

#[derive(Debug)]
pub struct WorkspaceError {
    pub code: String,
    pub detail: String,
}

impl WorkspaceError {
    pub fn new(code: &str, detail: impl Into<String>) -> Self {
        WorkspaceError { code: code.to_string(), detail: detail.into() }
    }
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            write!(f, "{}", self.code)
        } else {
            write!(f, "{}: {}", self.code, self.detail)
        }
    }
}

impl std::error::Error for WorkspaceError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub root: PathBuf,
    pub rule: String,
}

impl Workspace {
    pub fn rf_dir(&self) -> PathBuf {
        self.root.join(".fab7").join("rf")
    }

    /// Whether this workspace already holds RingFrame's record. A hook writes
    /// only into one that does: it never creates the record itself.
    pub fn exists(&self) -> bool {
        self.rf_dir().is_dir()
    }

    pub fn ensure(&self) -> std::io::Result<&Self> {
        let rf = self.rf_dir();
        std::fs::create_dir_all(&rf)?;
        set_private(&rf)?;
        let ignore = rf.join(".gitignore");
        if !ignore.exists() {
            std::fs::write(&ignore, "*\n")?;
        }
        for sub in ["tmp", "asks", "evals", "seals", "sessions"] {
            std::fs::create_dir_all(rf.join(sub))?;
        }
        Ok(self)
    }

    pub fn describe(&self) -> Value {
        json!({"root": self.root.to_string_lossy(), "rule": self.rule})
    }
}

pub fn resolve(cwd: Option<&Path>, explicit: Option<&Path>) -> std::io::Result<Workspace> {
    if let Some(path) = explicit {
        return Ok(Workspace { root: canonical_path(path)?, rule: "explicit".into() });
    }
    let here = match cwd {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let here = canonical_path(&here)?;
    if let Some(found) = discover(&here) {
        let rule = if found == here { "cwd" } else { "found" };
        return Ok(Workspace { root: found, rule: rule.into() });
    }
    Ok(Workspace { root: here, rule: "cwd".into() })
}

/// The nearest directory at or above `start` that already holds a RingFrame
/// record, looking no higher than the Git repository `start` is in. A harness
/// run from a subdirectory of a project, or a shell that moved into one, is
/// still that project's: without this each such directory grew a record of
/// its own. A repository nested in another keeps its own, and a directory
/// outside any repository is looked at alone.
pub fn discover(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        if dir.join(".fab7").join("rf").is_dir() {
            return Some(dir.to_path_buf());
        }
        if dir.join(".git").exists() {
            return None;
        }
    }
    None
}

/// The plans this project already holds, by name, sorted.
///
/// `plans/<slug>/` is the layout each host's Plan As Files rule writes, and a
/// plan that is already written is a terminal condition: the Ask that names
/// it builds it rather than planning it again. Which wording named it — "the
/// crypto trading agent plan", "plans/crypto-trading-agent", "crypto trading
/// agent" — is not something the record can settle, so RingFrame reports what
/// is on disk and the routing reads it from there.
///
/// A workspace of several projects keeps each one's plans under
/// `plans/<project>/<slug>/`: a folder with no `plan.md` whose own folders
/// hold one is a project, and its plans are named `<project>/<slug>`.
///
/// A project that keeps its plans somewhere else simply has none to report,
/// which is what this said before there was a list at all.
///
/// Each comes with its path and its title (the plan's first `# ` heading):
/// a slug alone left the model to guess that "the --json flag" meant
/// `cli-json-greeting` (q01, Codex), and the path is what the next Ask names.
pub fn plans(ws: &Workspace) -> Vec<serde_json::Value> {
    let folders = |dir: &Path| -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort();
        names
    };
    let root = ws.root.join("plans");
    let mut held: Vec<(Option<String>, String)> = Vec::new();
    for name in folders(&root) {
        let plans_within: Vec<String> = match root.join(&name).join("plan.md").exists() {
            true => Vec::new(),
            false => folders(&root.join(&name))
                .into_iter()
                .filter(|slug| root.join(&name).join(slug).join("plan.md").exists())
                .collect(),
        };
        match plans_within.is_empty() {
            true => held.push((None, name)),
            false => held.extend(plans_within.into_iter().map(|slug| (Some(name.clone()), slug))),
        }
    }
    held.into_iter()
        .map(|(project, slug)| {
            let name = project.as_ref().map_or(slug.clone(), |p| format!("{p}/{slug}"));
            let rel = format!("plans/{name}/plan.md");
            let text = std::fs::read_to_string(ws.root.join(&rel)).ok();
            let title = text.as_deref().and_then(|t| {
                t.lines().find_map(|l| l.strip_prefix("# ")).map(|h| h.trim().to_string())
            });
            let paragraph = text.as_deref().map(first_paragraph);
            serde_json::json!({"name": name, "project": project, "slug": slug,
                               "path": text.is_some().then_some(rel), "title": title,
                               "paragraph": paragraph})
        })
        .collect()
}

/// A plan's first paragraph after its title, its lines joined, in at most
/// 300 characters: what Sense reads of a plan beside its slug and title.
fn first_paragraph(text: &str) -> String {
    // As sense's labels read it: with no title, from the top.
    let title = text.lines().position(|l| l.starts_with("# "));
    let after = text.lines().skip(title.map_or(0, |i| i + 1));
    let lines = after.skip_while(|l| l.trim().is_empty()).take_while(|l| !l.trim().is_empty());
    lines.map(str::trim).collect::<Vec<_>>().join(" ").chars().take(300).collect()
}

/// Absolute, with symlinks followed. A workspace root reached two ways has to
/// be one root, or the record forks.
pub fn canonical_path(path: &Path) -> std::io::Result<PathBuf> {
    match path.canonicalize() {
        Ok(p) => Ok(p),
        // `resolve()` tolerates a path that does not exist yet; so does this.
        Err(_) if !path.is_absolute() => Ok(std::env::current_dir()?.join(path)),
        Err(_) => Ok(path.to_path_buf()),
    }
}

pub fn set_private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

fn git(root: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("git").arg("-C").arg(root).args(args).output()
}

/// A path a caller named, confined to the opened directory.
///
/// Everything RingFrame itself reads and writes stays inside the workspace, but
/// for its own cache of read-only snapshots (`snapshots.rs`), which no caller
/// names. A
/// staged directory is read and then deleted, so one that escaped would have
/// RingFrame remove something nobody opened; a subject outside it would have
/// the record describe a tree this workspace does not hold. Symlinks are
/// followed first, so a link inside pointing out does not get through.
///
/// What a harness reads or writes once the prompt reaches it is not this —
/// that is the harness's own permission model, and RingFrame does not own it.
pub fn within(ws: &Workspace, path: &Path) -> Result<PathBuf, WorkspaceError> {
    // Not `canonical_path`: it tolerates a path that does not exist and leaves
    // `..` in it, and `starts_with` reads those components literally. A path
    // RingFrame is about to read or delete has to resolve for real.
    let full = path.canonicalize().map_err(|e| {
        WorkspaceError::new("workspace.outside", format!("{}: {e}", path.display()))
    })?;
    if !full.starts_with(&ws.root) {
        return Err(WorkspaceError::new(
            "workspace.outside",
            format!(
                "{} is outside the workspace at {}; RingFrame reads and writes only \
                 inside the directory it was opened in",
                full.display(),
                ws.root.display()
            ),
        ));
    }
    Ok(full)
}

/// Git is a hard requirement: Eval diffs it and Seal names a commit as the
/// subject. Refused here rather than at Eval, so that no Ask is ever recorded
/// in a workspace whose work could not later be evaluated or sealed.
/// What every command but `init` says in a repository that has not been set
/// up. Only `ringframe init` makes a workspace: a hook or a command that
/// records something writes into one that exists, and never creates it.
pub fn uninitialized(ws: &Workspace) -> String {
    format!(
        "{} has no RingFrame workspace. Go to your repository's root and run          `ringframe init` once, then try again.",
        ws.root.display()
    )
}

pub fn require_initialized(ws: &Workspace) -> Result<(), WorkspaceError> {
    if ws.exists() {
        Ok(())
    } else {
        Err(WorkspaceError::new("workspace.uninitialized", uninitialized(ws)))
    }
}

pub fn require_git(ws: &Workspace) -> Result<(), WorkspaceError> {
    let ok = |args: &[&str]| matches!(git(&ws.root, args), Ok(out) if out.status.success());
    if !ok(&["rev-parse", "--git-dir"]) {
        return Err(WorkspaceError::new(
            "workspace.no_git",
            format!(
                "{} is not in a Git repository; RingFrame evaluates and seals the Git delta. \
                 Run `git init` and make one commit, or run from a repository.",
                ws.root.display()
            ),
        ));
    }
    // Git discovery walks up. Without this, a directory inside someone else's
    // repository becomes a workspace whose anchor, HEAD and history all belong
    // to a repository nobody opened — and RingFrame is meant to run inside the
    // directory it was given, not above it.
    let prefix = git(&ws.root, &["rev-parse", "--show-prefix"])
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if !prefix.is_empty() {
        let top = git(&ws.root, &["rev-parse", "--show-toplevel"])
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        return Err(WorkspaceError::new(
            "workspace.not_repo_root",
            format!(
                "{} is inside the repository at {}, not its root; RingFrame anchors every \
                 Eval and Seal to a commit, and that commit would belong to a repository \
                 this directory does not contain. Run `git init` here to make it its own \
                 repository, or open {} instead.",
                ws.root.display(),
                top,
                top
            ),
        ));
    }
    if !ok(&["rev-parse", "--verify", "--quiet", "HEAD"]) {
        return Err(WorkspaceError::new(
            "workspace.no_commit",
            format!(
                "{} is a Git repository with no commit; RingFrame needs one commit to anchor an Eval. \
                 Make the first commit, then retry.",
                ws.root.display()
            ),
        ));
    }
    Ok(())
}

/// Sort key for `vX.Y.Z`. Unparseable tags sort lowest rather than raising.
fn version_key(tag: &str) -> (u8, Vec<u64>) {
    let rest = &tag[BUNDLE_TAG_PREFIX.len()..];
    match rest.split('.').map(str::parse::<u64>).collect::<Result<Vec<_>, _>>() {
        Ok(parts) => (1, parts),
        Err(_) => (0, Vec::new()),
    }
}

/// The tag names `git ls-remote --tags --refs` lists, one `<sha>\trefs/tags/<name>`
/// a line.
fn tags_listed(listing: &str) -> Vec<&str> {
    listing
        .lines()
        .filter_map(|l| l.split_once("\trefs/tags/").map(|(_, name)| name.trim()))
        .collect()
}

/// The manifest the marketplace keeps beside `config/`, or inside it if
/// someone puts it there. A `bundle.yaml` in its place is refused by name.
fn bundle_manifest(source: &Path) -> Result<Option<String>, WorkspaceError> {
    let beside = source.parent().map(|p| p.join("bundle.toml"));
    let inside = source.join("bundle.toml");
    for c in beside.into_iter().chain(std::iter::once(inside)) {
        if let Some(detail) = crate::config::yaml_left(&c) {
            return Err(WorkspaceError::new("config.yaml_found", detail));
        }
        if c.is_file() {
            return Ok(std::fs::read_to_string(c).ok());
        }
    }
    Ok(None)
}

/// Every file must name a schema this release reads, before anything is
/// replaced.
fn validate(staged: &Path, bundle: Option<&str>) -> Result<(), WorkspaceError> {
    use crate::config;

    for rel in ["harnesses", "deltas/practices"] {
        if !staged.join(rel).is_dir() {
            return Err(WorkspaceError::new(
                "config.bundle",
                format!("configuration has no {rel}/ directory"),
            ));
        }
    }
    if let Some(text) = bundle {
        let doc = config::load_toml_text(text, "bundle.toml")
            .map_err(|e| WorkspaceError::new("config.unreadable", e.to_string()))?;
        let found = doc.get("schema").and_then(serde_json::Value::as_str);
        if found != Some(BUNDLE_SCHEMA) {
            return Err(schema_refusal("bundle.toml", found, BUNDLE_SCHEMA));
        }
    }
    let mut expected: Vec<(PathBuf, &str)> = Vec::new();
    for (rel, schema) in [
        ("harnesses", PROFILE_SCHEMA),
        ("deltas", DELTAS_SCHEMA),
        ("deltas/practices", DELTAS_SCHEMA),
    ] {
        for stem in config::stems(&staged.join(rel)) {
            expected.push((staged.join(rel).join(format!("{stem}.toml")), schema));
        }
    }
    for (path, schema) in expected {
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        if let Some(detail) = config::yaml_left(&path) {
            return Err(WorkspaceError::new("config.yaml_found", detail));
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| WorkspaceError::new("config.unreadable", format!("{name}: {e}")))?;
        let doc = config::load_toml_text(&text, &name)
            .map_err(|e| WorkspaceError::new("config.unreadable", e.to_string()))?;
        let found = doc.get("schema").and_then(serde_json::Value::as_str);
        if found != Some(schema) {
            return Err(schema_refusal(&name, found, schema));
        }
    }
    Ok(())
}

/// The refusal that holds the era boundary: an older core meeting a newer
/// bundle says this and stops.
fn schema_refusal(name: &str, found: Option<&str>, want: &str) -> WorkspaceError {
    WorkspaceError::new(
        "config.schema",
        format!(
            "{name} declares {}; this release reads '{want}'. \
             Upgrade ringframe, or install a configuration it can read.",
            found.map_or("None".to_string(), |s| format!("'{s}'"))
        ),
    )
}

/// One install at a time in a process: a long-running program can be asked twice at once,
/// and two installs swapping the same folders would undo each other.
static INSTALLING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Swap in the staged tree, putting the previous one back if the swap fails. The backup's
/// name is this swap's own, and backups an earlier swap left beside `target` go first.
/// A backup that cannot be removed afterwards is returned, to be said.
fn replace(staged: &Path, target: &Path) -> std::io::Result<Option<PathBuf>> {
    let name = target.file_name().unwrap_or_default().to_string_lossy().to_string();
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let mine = format!(".{name}.previous-");
    if let Ok(entries) = std::fs::read_dir(parent) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            // An earlier swap's backup of this folder; `.previous-<pid>` is the name RingFrame
            // 0.8.0 used before, for the configuration only.
            if n.starts_with(&mine) || (name == "config" && n.starts_with(".previous-")) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let previous = target.exists().then(|| {
        parent.join(format!("{mine}{}-{}", std::process::id(), crate::ids::random_hex(4)))
    });
    if let Some(prev) = &previous {
        std::fs::rename(target, prev)?;
    }
    if let Err(e) = std::fs::rename(staged, target) {
        if let Some(prev) = &previous {
            std::fs::rename(prev, target)?;
        }
        return Err(e);
    }
    Ok(previous.filter(|prev| std::fs::remove_dir_all(prev).is_err()))
}

pub(crate) fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// `curl` and `tar`, rather than an HTTP stack and a decompressor linked in.
///
/// Both are on every machine that can run `install.sh`, which fetches this
/// binary the same way. Carrying TLS, gzip and tar inside the core to do what
/// two ubiquitous tools already do would be weight with nothing to show for it.
fn fetch(url: &str, timeout: &str) -> Result<Vec<u8>, WorkspaceError> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", timeout, url])
        .output()
        .map_err(|e| WorkspaceError::new("config.fetch", format!("curl: {e}")))?;
    if !out.status.success() {
        return Err(WorkspaceError::new(
            "config.fetch",
            format!("{url}: {}", String::from_utf8_lossy(&out.stderr).trim()),
        ));
    }
    Ok(out.stdout)
}

/// The release tags, newest first, asked of the repository with `git ls-remote` rather than
/// GitHub's REST API: the API allows sixty unauthenticated calls an hour to a machine, and a
/// tool that checks often used them up ("could not reach the latest release" was that limit).
fn release_tags() -> Result<Vec<String>, WorkspaceError> {
    let out = Command::new("git")
        .args(["ls-remote", "--tags", "--refs", &format!("https://github.com/{BUNDLE_REPO}.git")])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| WorkspaceError::new("config.fetch", format!("git: {e}")))?;
    if !out.status.success() {
        return Err(WorkspaceError::new(
            "config.fetch",
            format!("git ls-remote {BUNDLE_REPO}: {}", String::from_utf8_lossy(&out.stderr).trim()),
        ));
    }
    Ok(newest_first(tags_listed(&String::from_utf8_lossy(&out.stdout))))
}

/// The `vX.Y.Z` tags, highest first. A listing does not promise an order.
pub fn newest_first<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut tags: Vec<&str> =
        names.into_iter().filter(|n| n.starts_with(BUNDLE_TAG_PREFIX)).collect();
    tags.sort_by_key(|n| std::cmp::Reverse(version_key(n)));
    tags.into_iter().map(str::to_string).collect()
}

/// This build's version, which a release's `[requires] ringframe` is read against.
pub const THIS_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whether `a` is a newer `X.Y.Z` than `b`, part by part.
fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim_start_matches(BUNDLE_TAG_PREFIX).split('.').map(|p| p.parse().unwrap_or(0)).collect()
    };
    parts(a) > parts(b)
}

/// The RingFrame a release's `bundle.toml` says it needs (`[requires] ringframe`), if any.
fn needs_ringframe(bundle: &str) -> Result<Option<String>, WorkspaceError> {
    let doc = crate::config::load_toml_text(bundle, "bundle.toml")
        .map_err(|e| WorkspaceError::new("config.unreadable", e.to_string()))?;
    let found = doc.get("schema").and_then(Value::as_str);
    if found != Some(BUNDLE_SCHEMA) {
        return Err(schema_refusal("bundle.toml", found, BUNDLE_SCHEMA));
    }
    Ok(doc.pointer("/requires/ringframe").and_then(Value::as_str).map(str::to_string))
}

/// A release this build runs with: one whose `bundle.toml` it reads, needing no newer RingFrame.
fn runs_with(bundle: &str) -> Result<(), WorkspaceError> {
    match needs_ringframe(bundle)? {
        Some(need) if newer(&need, THIS_VERSION) => Err(WorkspaceError::new(
            "config.needs_ringframe",
            format!("it needs RingFrame {need}; this is {THIS_VERSION}"),
        )),
        _ => Ok(()),
    }
}

/// What installing the latest configuration is doing, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SyncStep {
    /// Asking which release is the newest.
    Finding,
    /// Downloading its archive.
    Downloading,
    /// Unpacking, checking and putting it in place.
    Installing,
}

/// Where releases come from: the tags newest first, a tag's `bundle.toml`, and its archive.
struct Releases<'a> {
    tags: &'a dyn Fn() -> Result<Vec<String>, WorkspaceError>,
    bundle: &'a dyn Fn(&str) -> Result<Vec<u8>, WorkspaceError>,
    archive: &'a dyn Fn(&str) -> Result<Vec<u8>, WorkspaceError>,
}

/// The repository's own releases, on the network.
fn github_releases<T>(then: impl FnOnce(&Releases<'_>) -> T) -> T {
    let bundle = |tag: &str| {
        fetch(
            &format!(
                "https://raw.githubusercontent.com/{BUNDLE_REPO}/{tag}/products/{BUNDLE_PRODUCT}/bundle.toml"
            ),
            "30",
        )
    };
    let archive = |tag: &str| {
        fetch(&format!("https://codeload.github.com/{BUNDLE_REPO}/tar.gz/refs/tags/{tag}"), "120")
    };
    then(&Releases { tags: &release_tags, bundle: &bundle, archive: &archive })
}

/// How many releases are read before giving up on finding one this build runs with.
const TRIED: usize = 12;

/// The releases this build runs with, newest first: each tag's `bundle.toml` is read before
/// anything is downloaded, and one it cannot run with is passed over for the next older.
fn compatible(from: &Releases<'_>) -> Result<Vec<String>, WorkspaceError> {
    let tags = (from.tags)()?;
    let mut ok = Vec::new();
    let mut last = None;
    for tag in tags.iter().take(TRIED) {
        let checked = (from.bundle)(tag).and_then(|b| runs_with(&String::from_utf8_lossy(&b)));
        match checked {
            Ok(()) => ok.push(tag.clone()),
            Err(e) if e.code == "config.fetch" && ok.is_empty() && last.is_none() => return Err(e),
            Err(e) => last = Some(format!("{tag}: {}", e.detail)),
        }
    }
    if ok.is_empty() {
        return Err(WorkspaceError::new(
            "config.no_release",
            match last {
                Some(why) => {
                    format!("no release of {BUNDLE_REPO} this RingFrame runs with ({why})")
                }
                None => format!("{BUNDLE_REPO} has no {BUNDLE_TAG_PREFIX}* release tag"),
            },
        ));
    }
    Ok(ok)
}

/// A part of a fab7 release installed beside RingFrame's configuration: the folder under
/// `products/` it comes from, and where it goes: what a program built on RingFrame keeps
/// beside its configuration.
pub struct Part {
    pub from: String,
    pub to: PathBuf,
}

/// Unpack `tag`'s archive into a work folder beside `near`, then copy its `products/` folders
/// out: RingFrame's configuration into `dest`, and each part into its own staged folder.
/// Returns the bundle manifest text.
fn unpack(
    tag: &str,
    raw: &[u8],
    near: &Path,
    dest: &Path,
    parts: &[(PathBuf, &Part)],
) -> Result<Option<String>, WorkspaceError> {
    let work = near.join(format!(".extract-{}-{}", std::process::id(), crate::ids::random_hex(4)));
    let outcome = (|| {
        std::fs::create_dir_all(&work).map_err(io("config.bundle"))?;
        let archive = work.join("bundle.tar.gz");
        std::fs::write(&archive, raw).map_err(io("config.bundle"))?;
        let status = Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&work)
            .status()
            .map_err(|e| WorkspaceError::new("config.bundle", format!("tar: {e}")))?;
        if !status.success() {
            return Err(WorkspaceError::new("config.bundle", format!("{tag} did not unpack")));
        }
        // GitHub wraps the tree in one directory named for the repository and tag.
        let top = std::fs::read_dir(&work)
            .map_err(io("config.bundle"))?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.is_dir())
            .ok_or_else(|| WorkspaceError::new("config.bundle", format!("{tag} is empty")))?;
        let products = top.join("products");
        let config = products.join(BUNDLE_PRODUCT).join("config");
        if !config.is_dir() {
            return Err(WorkspaceError::new(
                "config.bundle",
                format!("{tag} contains no products/{BUNDLE_PRODUCT}/config/"),
            ));
        }
        copy_tree(&config, dest).map_err(io("config.bundle"))?;
        stage_parts(&products, parts, tag)?;
        // The manifest sits beside config/, so it is read but never mirrored:
        // the installed tree stays exactly the config/ tree.
        bundle_manifest(&config)
    })();
    let _ = std::fs::remove_dir_all(&work);
    outcome
}

/// Copy each part out of `products` into its staged folder.
fn stage_parts(
    products: &Path,
    parts: &[(PathBuf, &Part)],
    from: &str,
) -> Result<(), WorkspaceError> {
    for (staged, part) in parts {
        let src = products.join(&part.from);
        if !src.is_dir() {
            return Err(WorkspaceError::new(
                "config.bundle",
                format!("{from} contains no products/{}/", part.from),
            ));
        }
        copy_tree(&src, staged).map_err(io("config.bundle"))?;
    }
    Ok(())
}

/// What `sync` would install, against what is installed. Reads, never writes.
pub fn check_config() -> Result<Value, WorkspaceError> {
    github_releases(|from| {
        let latest = compatible(from)?.remove(0);
        let manifest = fetch(
            &format!(
                "https://raw.githubusercontent.com/{BUNDLE_REPO}/{latest}/.claude-plugin/marketplace.json"
            ),
            "30",
        )?;
        let revision = std::fs::read_to_string(crate::config::config_dir().join(".revision"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "none".into());
        Ok(freshness(&revision, &latest, &manifest))
    })
}

/// Anything but the latest release is behind, `local` (a configuration
/// installed from a folder) too: installing or syncing always replaces it.
fn freshness(revision: &str, latest: &str, manifest: &[u8]) -> Value {
    let plugin = serde_json::from_slice::<Value>(manifest).ok().and_then(|m| {
        m["plugins"].as_array()?.iter().find(|p| p["name"] == "rf")?["version"]
            .as_str()
            .map(str::to_string)
    });
    json!({
        "revision": revision,
        "latest": latest,
        "plugin": plugin,
        "behind": revision != latest,
    })
}

/// Replace the synced config layer.
pub fn install_config(source: Option<&Path>) -> Result<Value, WorkspaceError> {
    install_config_with(source, &|_| {})
}

/// As `install_config`, telling `progress` each step as it starts: a
/// configuration from a folder is only installed.
pub fn install_config_with(
    source: Option<&Path>,
    progress: &dyn Fn(SyncStep),
) -> Result<Value, WorkspaceError> {
    install_release(source, &[], progress)
}

/// Install one fab7 release whole: RingFrame's configuration and every part beside it, from
/// the newest release this build runs with (one refused once downloaded gives way to the next
/// older), or from a folder (`.../ringframe/config`, its parts under the folder two up).
/// Each installed folder records the release in `.revision`.
pub fn install_release(
    source: Option<&Path>,
    parts: &[Part],
    progress: &dyn Fn(SyncStep),
) -> Result<Value, WorkspaceError> {
    github_releases(|from| install_from(source, parts, from, progress))
}

fn install_from(
    source: Option<&Path>,
    parts: &[Part],
    from: &Releases<'_>,
    progress: &dyn Fn(SyncStep),
) -> Result<Value, WorkspaceError> {
    use crate::config;

    let _one = INSTALLING.lock().unwrap_or_else(|e| e.into_inner());
    let home_dir = config::home();
    std::fs::create_dir_all(&home_dir).map_err(io("config.source"))?;
    set_private(&home_dir).map_err(io("config.source"))?;
    let stage_of = |near: &Path| {
        near.join(format!(".staging-{}-{}", std::process::id(), crate::ids::random_hex(4)))
    };
    let staged = stage_of(&home_dir);
    let staged_parts: Vec<(PathBuf, &Part)> = parts
        .iter()
        .map(|p| (stage_of(p.to.parent().unwrap_or_else(|| Path::new("."))), p))
        .collect();
    let clear = || {
        let _ = std::fs::remove_dir_all(&staged);
        for (s, _) in &staged_parts {
            let _ = std::fs::remove_dir_all(s);
        }
    };
    for (_, p) in &staged_parts {
        if let Some(parent) = p.to.parent() {
            std::fs::create_dir_all(parent).map_err(io("config.source"))?;
        }
    }
    let outcome = (|| {
        let (revision, bundle) = match source {
            Some(src) => {
                progress(SyncStep::Installing);
                let src = canonical_path(src).map_err(io("config.source"))?;
                if !src.join("harnesses").is_dir() {
                    return Err(WorkspaceError::new(
                        "config.source",
                        format!("{} has no harnesses/ directory", src.display()),
                    ));
                }
                copy_tree(&src, &staged).map_err(io("config.source"))?;
                if !staged_parts.is_empty() {
                    let products = src.parent().and_then(Path::parent).unwrap_or(&src);
                    stage_parts(products, &staged_parts, &src.display().to_string())?;
                }
                let bundle = bundle_manifest(&src)?;
                validate(&staged, bundle.as_deref())?;
                (folder_revision(&src), bundle)
            }
            None => {
                progress(SyncStep::Finding);
                let tags = compatible(from)?;
                let mut tried = None;
                let mut got = None;
                for tag in tags.iter().take(3) {
                    progress(SyncStep::Downloading);
                    let raw = (from.archive)(tag)?;
                    progress(SyncStep::Installing);
                    let unpacked = unpack(tag, &raw, &home_dir, &staged, &staged_parts)
                        .and_then(|b| validate(&staged, b.as_deref()).map(|()| b));
                    match unpacked {
                        Ok(b) => {
                            got = Some((tag.clone(), b));
                            break;
                        }
                        // Refused once read: the next older release may be one this build reads.
                        Err(e)
                            if e.code == "config.schema" || e.code == "config.needs_ringframe" =>
                        {
                            clear();
                            tried = Some(e);
                        }
                        Err(e) => return Err(e),
                    }
                }
                match (got, tried) {
                    (Some(found), _) => found,
                    (None, Some(e)) => return Err(e),
                    (None, None) => {
                        return Err(WorkspaceError::new(
                            "config.no_release",
                            "no release to install",
                        ));
                    }
                }
            }
        };
        let requires = bundle.as_deref().map(needs_ringframe).transpose()?.flatten();
        let mut leftovers = Vec::new();
        let mut put = |staged: &Path, target: &Path| -> Result<(), WorkspaceError> {
            std::fs::write(staged.join(".revision"), format!("{revision}\n"))
                .map_err(io("config.source"))?;
            if let Some(need) = &requires {
                std::fs::write(staged.join(".requires"), format!("{need}\n"))
                    .map_err(io("config.source"))?;
            }
            if let Some(left) = replace(staged, target).map_err(io("config.source"))? {
                leftovers.push(left.to_string_lossy().to_string());
            }
            Ok(())
        };
        put(&staged, &config::config_dir())?;
        for (s, p) in &staged_parts {
            put(s, &p.to)?;
        }
        Ok((revision, leftovers))
    })();
    clear();
    let (revision, leftovers) = outcome?;
    Ok(json!({
        "rf_dir": home_dir.to_string_lossy(),
        "config": config::config_dir().to_string_lossy(),
        "revision": revision,
        "parts": parts.iter().map(|p| p.to.to_string_lossy().to_string()).collect::<Vec<_>>(),
        "leftovers": leftovers,
    }))
}

/// What a configuration installed from a folder is recorded as: `bundled <tag>` when the folder
/// says the release it was copied from (in `.revision` two folders up), else `local`.
fn folder_revision(config: &Path) -> String {
    config
        .parent()
        .and_then(Path::parent)
        .and_then(|root| std::fs::read_to_string(root.join(".revision")).ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .map_or_else(|| "local".to_string(), |t| format!("bundled {t}"))
}

fn io(code: &'static str) -> impl Fn(std::io::Error) -> WorkspaceError {
    move |e| WorkspaceError::new(code, e.to_string())
}

#[cfg(test)]
mod discover_tests {
    use super::*;

    fn git_repo() -> crate::testing::TempDir {
        let dir = crate::testing::tmp_dir();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        dir
    }

    #[test]
    fn a_subdirectory_resolves_to_the_workspace_above_it() {
        let repo = git_repo();
        std::fs::create_dir_all(repo.path().join(".fab7/rf")).unwrap();
        let sub = repo.path().join("crates/app");
        std::fs::create_dir_all(&sub).unwrap();
        let ws = resolve(Some(&sub), None).unwrap();
        assert_eq!(ws.root, canonical_path(repo.path()).unwrap());
        assert_eq!(ws.rule, "found");
        assert!(ws.exists());
    }

    #[test]
    fn with_no_record_anywhere_the_directory_is_the_workspace_and_nothing_is_made() {
        let repo = git_repo();
        let sub = repo.path().join("crates");
        std::fs::create_dir_all(&sub).unwrap();
        let ws = resolve(Some(&sub), None).unwrap();
        assert_eq!(ws.root, canonical_path(&sub).unwrap());
        assert_eq!(ws.rule, "cwd");
        assert!(!ws.exists());
        assert!(!sub.join(".fab7").exists() && !repo.path().join(".fab7").exists());
    }

    #[test]
    fn the_search_stops_at_the_repository_it_starts_in() {
        let outer = git_repo();
        std::fs::create_dir_all(outer.path().join(".fab7/rf")).unwrap();
        let inner = outer.path().join("vendor/lib");
        std::fs::create_dir_all(inner.join(".git")).unwrap();
        let ws = resolve(Some(&inner), None).unwrap();
        assert_eq!(ws.root, canonical_path(&inner).unwrap(), "a nested repository keeps its own");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Syncing tells finding the release, downloading it and installing it,
    /// in that order; a failed download installs nothing.
    #[test]
    fn syncing_tells_each_step_in_order() {
        crate::testing::with_config_home(|home| {
            // A release archive as GitHub serves one: a top folder holding the product.
            let tree = home.join("release");
            let config = tree.join("fab7-v9/products").join(BUNDLE_PRODUCT).join("config");
            copy_tree(&crate::testing::fixture_config(), &config).unwrap();
            let archive = home.join("release.tar.gz");
            let packed = Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(&tree)
                .arg("fab7-v9")
                .status()
                .unwrap();
            assert!(packed.success());
            let bytes = std::fs::read(&archive).unwrap();
            let tags = || Ok(vec!["v9.0.0".to_string()]);
            let bundle = |_: &str| Ok(b"schema = \"ringframe.bundle/2\"\n".to_vec());
            let archive = |tag: &str| {
                assert_eq!(tag, "v9.0.0");
                Ok(bytes.clone())
            };
            let seen = std::cell::RefCell::new(Vec::new());
            let releases = Releases { tags: &tags, bundle: &bundle, archive: &archive };
            let out = install_from(None, &[], &releases, &|s| seen.borrow_mut().push(s)).unwrap();
            assert_eq!(out["revision"], "v9.0.0");
            assert_eq!(
                seen.into_inner(),
                [SyncStep::Finding, SyncStep::Downloading, SyncStep::Installing]
            );

            let gone = |_: &str| Err(WorkspaceError::new("config.fetch", "offline"));
            let seen = std::cell::RefCell::new(Vec::new());
            let releases = Releases { tags: &tags, bundle: &bundle, archive: &gone };
            assert!(install_from(None, &[], &releases, &|s| seen.borrow_mut().push(s)).is_err());
            assert_eq!(seen.into_inner(), [SyncStep::Finding, SyncStep::Downloading]);
        });
    }

    /// A release archive as GitHub serves one, `fab7-<tag>/products/...`, with RingFrame's
    /// configuration and a part beside it.
    fn release_archive(home: &Path, tag: &str) -> Vec<u8> {
        let tree = home.join(format!("release-{tag}"));
        let products = tree.join(format!("fab7-{tag}/products"));
        copy_tree(&crate::testing::fixture_config(), &products.join(BUNDLE_PRODUCT).join("config"))
            .unwrap();
        std::fs::create_dir_all(products.join("tool/files")).unwrap();
        std::fs::write(products.join("tool/files/codex.toml"), format!("# {tag}\n")).unwrap();
        let archive = home.join(format!("{tag}.tar.gz"));
        let packed = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&tree)
            .arg(format!("fab7-{tag}"))
            .status()
            .unwrap();
        assert!(packed.success());
        std::fs::read(&archive).unwrap()
    }

    /// The newest release this build runs with, read from each tag's bundle.toml before any
    /// download: a release needing a newer RingFrame, or one of another era, gives way to an older.
    #[test]
    fn the_newest_release_this_ringframe_runs_with_is_taken() {
        let tags = || Ok(vec!["v99.0.0".into(), "v9.0.1".into(), "v9.0.0".into()]);
        let bundle = |tag: &str| {
            Ok(match tag {
                "v99.0.0" => {
                    b"schema = \"ringframe.bundle/2\"\n[requires]\nringframe = \"99.0.0\"\n"
                        .to_vec()
                }
                "v9.0.1" => b"schema = \"ringframe.bundle/3\"\n".to_vec(),
                _ => format!(
                    "schema = \"ringframe.bundle/2\"\n[requires]\nringframe = \"{THIS_VERSION}\"\n"
                )
                .into_bytes(),
            })
        };
        let none = |_: &str| -> Result<Vec<u8>, WorkspaceError> { unreachable!() };
        let releases = Releases { tags: &tags, bundle: &bundle, archive: &none };
        assert_eq!(compatible(&releases).unwrap(), ["v9.0.0"]);

        let offline = || Err(WorkspaceError::new("config.fetch", "offline"));
        let releases = Releases { tags: &offline, bundle: &bundle, archive: &none };
        assert_eq!(compatible(&releases).unwrap_err().code, "config.fetch");
        let only_newer = || Ok(vec!["v99.0.0".to_string()]);
        let releases = Releases { tags: &only_newer, bundle: &bundle, archive: &none };
        let refused = compatible(&releases).unwrap_err();
        assert_eq!(refused.code, "config.no_release");
        assert!(refused.detail.contains("needs RingFrame 99.0.0"), "{}", refused.detail);
    }

    /// One release is one unit: the configuration and each part beside it are installed from
    /// the same tag, each recording it; a release refused once unpacked gives way to the older.
    #[test]
    fn a_release_installs_its_parts_together_and_a_refused_one_gives_way() {
        crate::testing::with_config_home(|home| {
            let good = release_archive(home, "v9.0.0");
            let tags = || Ok(vec!["v9.0.1".into(), "v9.0.0".into()]);
            let bundle = |_: &str| Ok(b"schema = \"ringframe.bundle/2\"\n".to_vec());
            // v9.0.1 says it is readable, but its files are of another era once unpacked.
            let bad = {
                let tree = home.join("bad");
                let cfg = tree.join("fab7-v9.0.1/products").join(BUNDLE_PRODUCT).join("config");
                copy_tree(&crate::testing::fixture_config(), &cfg).unwrap();
                for f in std::fs::read_dir(cfg.join("harnesses")).unwrap().flatten() {
                    let text = std::fs::read_to_string(f.path()).unwrap();
                    std::fs::write(f.path(), text.replace(PROFILE_SCHEMA, "ringframe.profile/9"))
                        .unwrap();
                }
                std::fs::create_dir_all(tree.join("fab7-v9.0.1/products/tool/files")).unwrap();
                let archive = home.join("bad.tar.gz");
                assert!(
                    Command::new("tar")
                        .arg("-czf")
                        .arg(&archive)
                        .arg("-C")
                        .arg(&tree)
                        .arg("fab7-v9.0.1")
                        .status()
                        .unwrap()
                        .success()
                );
                std::fs::read(&archive).unwrap()
            };
            let archive = |tag: &str| Ok(if tag == "v9.0.1" { bad.clone() } else { good.clone() });
            let releases = Releases { tags: &tags, bundle: &bundle, archive: &archive };
            let files = home.join("tool-home/files");
            let parts = [Part { from: "tool/files".into(), to: files.clone() }];
            let out = install_from(None, &parts, &releases, &|_| {}).unwrap();
            assert_eq!(out["revision"], "v9.0.0");
            assert_eq!(std::fs::read_to_string(files.join("codex.toml")).unwrap(), "# v9.0.0\n");
            for dir in [crate::config::config_dir(), files.clone()] {
                assert_eq!(std::fs::read_to_string(dir.join(".revision")).unwrap(), "v9.0.0\n");
            }
            let beside: Vec<String> = std::fs::read_dir(files.parent().unwrap())
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            assert_eq!(beside, ["files"], "no staging left beside it");
        });
    }

    /// A swap names its backup its own way and clears what an earlier one left, so a leftover
    /// (as RingFrame 0.8.0's `.previous-<pid>` was) never blocks the next install.
    #[test]
    fn a_swap_clears_what_an_earlier_one_left() {
        let dir = crate::testing::tmp_dir();
        let target = dir.path().join("config");
        std::fs::create_dir_all(target.join("harnesses")).unwrap();
        for left in [".previous-74254", ".config.previous-1-ab"] {
            std::fs::create_dir_all(dir.path().join(left).join("harnesses")).unwrap();
        }
        let staged = dir.path().join(".staging-1");
        std::fs::create_dir_all(staged.join("deltas")).unwrap();
        assert_eq!(replace(&staged, &target).unwrap(), None);
        assert!(target.join("deltas").is_dir());
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["config"]);
    }

    /// A configuration from a folder a program's archive carries is recorded as `bundled <tag>`.
    #[test]
    fn a_bundled_copy_is_recorded_with_the_release_it_came_from() {
        let dir = crate::testing::tmp_dir();
        let config = dir.path().join("fab7/ringframe/config");
        std::fs::create_dir_all(&config).unwrap();
        assert_eq!(folder_revision(&config), "local");
        std::fs::write(dir.path().join("fab7/.revision"), "v0.8.0\n").unwrap();
        assert_eq!(folder_revision(&config), "bundled v0.8.0");
    }

    #[test]
    fn a_path_outside_the_opened_directory_is_refused() {
        let dir = crate::testing::tmp_dir();
        let ws = resolve(Some(dir.path()), None).unwrap();
        std::fs::create_dir_all(ws.root.join("inside")).unwrap();
        assert_eq!(within(&ws, &ws.root.join("inside")).unwrap(), ws.root.join("inside"));
        assert_eq!(within(&ws, &ws.root).unwrap(), ws.root, "the root itself is within it");

        let outside = crate::testing::tmp_dir();
        let e = within(&ws, outside.path()).unwrap_err();
        assert_eq!(e.code, "workspace.outside");

        // `..` does not get through, and neither does a link that leads out.
        assert!(within(&ws, &ws.root.join("inside/../..")).is_err());
        std::os::unix::fs::symlink(outside.path(), ws.root.join("link")).unwrap();
        assert!(
            within(&ws, &ws.root.join("link")).is_err(),
            "a link inside pointing out is still out"
        );

        // A path that does not resolve is refused rather than guessed at.
        assert!(within(&ws, &ws.root.join("not-there")).is_err());
    }

    #[test]
    fn a_project_with_no_plans_reports_none_rather_than_failing() {
        let dir = crate::testing::tmp_dir();
        let ws = resolve(Some(dir.path()), None).unwrap();
        assert!(plans(&ws).is_empty(), "no plans/ directory is not an error");
    }

    #[test]
    fn the_plans_a_project_holds_are_its_directories_under_plans() {
        let dir = crate::testing::tmp_dir();
        let ws = resolve(Some(dir.path()), None).unwrap();
        for slug in ["crypto-trading-agent", "ringframe", ".scratch"] {
            std::fs::create_dir_all(ws.root.join("plans").join(slug)).unwrap();
        }
        // A loose file is not a plan, and neither is a hidden directory.
        std::fs::write(ws.root.join("plans/README.md"), "why this exists\n").unwrap();
        let held = plans(&ws);
        let slugs: Vec<&str> = held.iter().filter_map(|p| p["slug"].as_str()).collect();
        assert_eq!(slugs, ["crypto-trading-agent", "ringframe"]);
    }

    #[test]
    fn a_projects_plans_are_listed_with_their_project_beside_the_flat_ones() {
        let dir = crate::testing::tmp_dir();
        let ws = resolve(Some(dir.path()), None).unwrap();
        for plan in ["weft/harden", "weft/launch", "sense/harden", "site"] {
            std::fs::create_dir_all(ws.root.join("plans").join(plan)).unwrap();
            std::fs::write(ws.root.join("plans").join(plan).join("plan.md"), "# T\n").unwrap();
        }
        // A plan's own folders (its ADRs) are not plans of a project.
        std::fs::create_dir_all(ws.root.join("plans/site/adr")).unwrap();
        std::fs::write(ws.root.join("plans/site/adr/plan.md"), "# Not a plan\n").unwrap();
        let held = plans(&ws);
        let names: Vec<(&str, &str, &Value)> = held
            .iter()
            .map(|p| (p["name"].as_str().unwrap(), p["path"].as_str().unwrap(), &p["project"]))
            .collect();
        assert_eq!(
            names,
            [
                ("sense/harden", "plans/sense/harden/plan.md", &json!("sense")),
                ("site", "plans/site/plan.md", &Value::Null),
                ("weft/harden", "plans/weft/harden/plan.md", &json!("weft")),
                ("weft/launch", "plans/weft/launch/plan.md", &json!("weft")),
            ]
        );
        assert_eq!(held[0]["slug"], "harden");
    }

    #[test]
    fn a_plan_comes_with_its_title_and_first_paragraph() {
        let dir = crate::testing::tmp_dir();
        let ws = resolve(Some(dir.path()), None).unwrap();
        std::fs::create_dir_all(ws.root.join("plans/sync")).unwrap();
        let md = "<!-- x -->\n# Offline sync\n\n  Notes sync between devices\n  when they reconnect.\n\n## Phase 1\n";
        std::fs::write(ws.root.join("plans/sync/plan.md"), md).unwrap();
        std::fs::create_dir_all(ws.root.join("plans/long")).unwrap();
        std::fs::write(
            ws.root.join("plans/long/plan.md"),
            format!("# Long\n{}\n", "é".repeat(400)),
        )
        .unwrap();
        let held = plans(&ws);
        assert_eq!(held[1]["title"], "Offline sync");
        assert_eq!(held[1]["paragraph"], "Notes sync between devices when they reconnect.");
        assert_eq!(held[0]["paragraph"].as_str().unwrap().chars().count(), 300);
    }

    #[test]
    fn resolve_uses_the_cwd_and_says_so() {
        let dir = crate::testing::tmp_dir();
        let ws = resolve(Some(dir.path()), None).unwrap();
        assert_eq!(ws.rule, "cwd");
        assert_eq!(ws.root, canonical_path(dir.path()).unwrap());
        assert_eq!(ws.rf_dir(), ws.root.join(".fab7/rf"));
    }

    #[test]
    fn an_explicit_root_says_so() {
        let dir = crate::testing::tmp_dir();
        let ws = resolve(None, Some(dir.path())).unwrap();
        assert_eq!(ws.rule, "explicit");
        assert_eq!(ws.describe()["rule"], "explicit");
    }

    #[test]
    fn ensure_makes_a_private_ignored_tree() {
        use std::os::unix::fs::PermissionsExt;
        let repo = crate::testing::repo();
        let ws = resolve(Some(repo.path()), None).unwrap();
        ws.ensure().unwrap();
        let rf = ws.rf_dir();
        assert_eq!(std::fs::read_to_string(rf.join(".gitignore")).unwrap(), "*\n");
        assert_eq!(rf.metadata().unwrap().permissions().mode() & 0o777, 0o700);
        for sub in ["tmp", "asks", "evals", "seals", "sessions"] {
            assert!(rf.join(sub).is_dir(), "no {sub}/");
        }
        // Idempotent: a second call must not disturb what is there.
        std::fs::write(rf.join("asks/keep.txt"), b"x").unwrap();
        ws.ensure().unwrap();
        assert!(rf.join("asks/keep.txt").exists());
    }

    #[test]
    fn git_is_required_and_says_which_way_it_is_missing() {
        let bare = crate::testing::tmp_dir();
        let ws = resolve(Some(bare.path()), None).unwrap();
        assert_eq!(require_git(&ws).unwrap_err().code, "workspace.no_git");

        let empty = crate::testing::tmp_dir();
        crate::testing::run(&["git", "init", "-q", &empty.path().to_string_lossy()]);
        let ws = resolve(Some(empty.path()), None).unwrap();
        assert_eq!(require_git(&ws).unwrap_err().code, "workspace.no_commit");

        let repo = crate::testing::repo();
        let ws = resolve(Some(repo.path()), None).unwrap();
        assert!(require_git(&ws).is_ok());
    }

    #[test]
    fn release_tags_are_read_from_git_ls_remote() {
        let listing = "a153a72\trefs/tags/v0.1.0\nf08ca91\trefs/tags/v0.1.10\n71ca6fc\trefs/tags/v0.1.2\n0000000\trefs/tags/nightly\n";
        assert_eq!(tags_listed(listing), ["v0.1.0", "v0.1.10", "v0.1.2", "nightly"]);
        assert_eq!(newest_first(tags_listed(listing)).first().map(String::as_str), Some("v0.1.10"));
    }

    #[test]
    fn freshness_compares_the_installed_revision_with_the_latest_release() {
        let manifest = br#"{"plugins": [{"name": "rf", "version": "0.1.2"}]}"#;
        let behind = freshness("v0.1.0", "v0.1.1", manifest);
        assert_eq!(behind["behind"], true);
        assert_eq!(behind["plugin"], "0.1.2");
        assert_eq!(freshness("v0.1.1", "v0.1.1", manifest)["behind"], false);
        // A configuration from a folder is replaced by the next release too.
        assert_eq!(freshness("local", "v0.1.1", manifest)["behind"], true);
        assert_eq!(freshness("none", "v0.1.1", manifest)["behind"], true);
        assert_eq!(freshness("v0.1.1", "v0.1.1", b"not json")["plugin"], Value::Null);
    }

    #[test]
    fn the_latest_tag_is_the_highest_not_the_first() {
        assert_eq!(
            newest_first(["v0.0.9", "v0.1.0", "v0.0.10"]).first().map(String::as_str),
            Some("v0.1.0")
        );
        assert_eq!(
            newest_first(["v0.0.2", "v0.0.10"]).first().map(String::as_str),
            Some("v0.0.10")
        );
        // Unparseable tags sort lowest rather than raising.
        assert_eq!(newest_first(["vnope", "v0.0.1"]).first().map(String::as_str), Some("v0.0.1"));
        assert_eq!(newest_first(["nope", "0.0.1"]).first().map(String::as_str), None);
    }
}
