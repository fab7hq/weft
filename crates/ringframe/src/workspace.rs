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
        crate::deltas::initialize(&rf)?;
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
    Ok(Workspace { root: canonical_path(&here)?, rule: "cwd".into() })
}

/// The plans this project already holds, by slug, sorted.
///
/// `plans/<slug>/` is the layout the `plan_as_files` practice writes, and a
/// plan that is already written is a terminal condition: the Ask that names
/// it builds it rather than planning it again. Which wording named it — "the
/// crypto trading agent plan", "plans/crypto-trading-agent", "crypto trading
/// agent" — is not something the record can settle, so RingFrame reports what
/// is on disk and the routing reads it from there.
///
/// A project that keeps its plans somewhere else simply has none to report,
/// which is what this said before there was a list at all.
pub fn plans(ws: &Workspace) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(ws.root.join("plans"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|name| !name.starts_with('.'))
        .collect();
    out.sort();
    out
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
/// Everything RingFrame itself reads and writes stays inside the workspace. A
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

/// The highest `vX.Y.Z`. The API does not promise an order, so never take the
/// first.
pub fn latest_tag<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    names.into_iter().filter(|n| n.starts_with(BUNDLE_TAG_PREFIX)).max_by_key(|n| version_key(n))
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

/// Swap in the staged tree, putting the previous one back if the swap fails.
fn replace(staged: &Path, target: &Path) -> std::io::Result<()> {
    let previous =
        target.exists().then(|| target.with_file_name(format!(".previous-{}", std::process::id())));
    if let Some(prev) = &previous {
        std::fs::rename(target, prev)?;
    }
    if let Err(e) = std::fs::rename(staged, target) {
        if let Some(prev) = &previous {
            std::fs::rename(prev, target)?;
        }
        return Err(e);
    }
    if let Some(prev) = &previous {
        let _ = std::fs::remove_dir_all(prev);
    }
    Ok(())
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

fn newest_tag() -> Result<String, WorkspaceError> {
    let body = fetch(&format!("https://api.github.com/repos/{BUNDLE_REPO}/tags"), "30")?;
    let tags: Value = serde_json::from_slice(&body)
        .map_err(|e| WorkspaceError::new("config.fetch", format!("tags: {e}")))?;
    let names: Vec<&str> =
        tags.as_array().into_iter().flatten().filter_map(|t| t["name"].as_str()).collect();
    latest_tag(names).map(str::to_string).ok_or_else(|| {
        WorkspaceError::new(
            "config.no_release",
            format!("{BUNDLE_REPO} has no {BUNDLE_TAG_PREFIX}* release tag"),
        )
    })
}

/// Extract `products/<product>/config` into `dest`. Returns the tag and the
/// bundle manifest text.
fn download(dest: &Path) -> Result<(String, Option<String>), WorkspaceError> {
    let tag = newest_tag()?;
    let raw =
        fetch(&format!("https://codeload.github.com/{BUNDLE_REPO}/tar.gz/refs/tags/{tag}"), "120")?;
    let work = dest.with_file_name(format!(
        ".extract-{}-{}",
        std::process::id(),
        crate::ids::random_hex(4)
    ));
    let outcome = (|| {
        std::fs::create_dir_all(&work).map_err(io("config.bundle"))?;
        let archive = work.join("bundle.tar.gz");
        std::fs::write(&archive, &raw).map_err(io("config.bundle"))?;
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
        let product = top.join("products").join(BUNDLE_PRODUCT);
        let config = product.join("config");
        if !config.is_dir() {
            return Err(WorkspaceError::new(
                "config.bundle",
                format!("{tag} contains no products/{BUNDLE_PRODUCT}/config/"),
            ));
        }
        copy_tree(&config, dest).map_err(io("config.bundle"))?;
        // The manifest sits beside config/, so it is read but never mirrored:
        // the installed tree stays exactly the config/ tree.
        Ok((tag.clone(), bundle_manifest(&config)?))
    })();
    let _ = std::fs::remove_dir_all(&work);
    outcome
}

/// What `sync` would install, against what is installed. Reads, never writes.
pub fn check_config() -> Result<Value, WorkspaceError> {
    let latest = newest_tag()?;
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
}

/// `local` is someone's working tree, and is never behind.
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
        "behind": revision != "local" && revision != latest,
    })
}

/// Replace the synced config layer.
pub fn install_config(source: Option<&Path>) -> Result<Value, WorkspaceError> {
    use crate::config;

    let home_dir = config::home();
    std::fs::create_dir_all(&home_dir).map_err(io("config.source"))?;
    set_private(&home_dir).map_err(io("config.source"))?;
    let staged =
        home_dir.join(format!(".staging-{}-{}", std::process::id(), crate::ids::random_hex(4)));
    let outcome = (|| {
        let (revision, bundle) = match source {
            Some(src) => {
                let src = canonical_path(src).map_err(io("config.source"))?;
                if !src.join("harnesses").is_dir() {
                    return Err(WorkspaceError::new(
                        "config.source",
                        format!("{} has no harnesses/ directory", src.display()),
                    ));
                }
                copy_tree(&src, &staged).map_err(io("config.source"))?;
                ("local".to_string(), bundle_manifest(&src)?)
            }
            None => download(&staged)?,
        };
        validate(&staged, bundle.as_deref())?;
        std::fs::write(staged.join(".revision"), format!("{revision}\n"))
            .map_err(io("config.source"))?;
        replace(&staged, &config::config_dir()).map_err(io("config.source"))?;
        Ok(revision)
    })();
    let _ = std::fs::remove_dir_all(&staged);
    let revision = outcome?;
    Ok(json!({
        "rf_dir": home_dir.to_string_lossy(),
        "config": config::config_dir().to_string_lossy(),
        "revision": revision,
    }))
}

fn io(code: &'static str) -> impl Fn(std::io::Error) -> WorkspaceError {
    move |e| WorkspaceError::new(code, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(plans(&ws), vec!["crypto-trading-agent", "ringframe"]);
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
    fn freshness_compares_the_installed_revision_with_the_latest_release() {
        let manifest = br#"{"plugins": [{"name": "rf", "version": "0.1.2"}]}"#;
        let behind = freshness("v0.1.0", "v0.1.1", manifest);
        assert_eq!(behind["behind"], true);
        assert_eq!(behind["plugin"], "0.1.2");
        assert_eq!(freshness("v0.1.1", "v0.1.1", manifest)["behind"], false);
        assert_eq!(freshness("local", "v0.1.1", manifest)["behind"], false);
        assert_eq!(freshness("none", "v0.1.1", manifest)["behind"], true);
        assert_eq!(freshness("v0.1.1", "v0.1.1", b"not json")["plugin"], Value::Null);
    }

    #[test]
    fn the_latest_tag_is_the_highest_not_the_first() {
        assert_eq!(latest_tag(["v0.0.9", "v0.1.0", "v0.0.10"]), Some("v0.1.0"));
        assert_eq!(latest_tag(["v0.0.2", "v0.0.10"]), Some("v0.0.10"));
        // Unparseable tags sort lowest rather than raising.
        assert_eq!(latest_tag(["vnope", "v0.0.1"]), Some("v0.0.1"));
        assert_eq!(latest_tag(["nope", "0.0.1"]), None);
    }
}
