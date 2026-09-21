//! Workspace root resolution and the `.fab7/rf/` directory.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

/// Every file that crosses the boundary names the schema it is written to, and
/// these are the versions this release reads. Nothing else gates on a version
/// (ADR-0012).
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

/// Python's `Path.resolve()`: absolute, with symlinks followed. A workspace
/// root reached two ways has to be one root, or the record forks.
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
    names
        .into_iter()
        .filter(|n| n.starts_with(BUNDLE_TAG_PREFIX))
        .max_by_key(|n| version_key(n))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_latest_tag_is_the_highest_not_the_first() {
        assert_eq!(latest_tag(["v0.0.9", "v0.1.0", "v0.0.10"]), Some("v0.1.0"));
        assert_eq!(latest_tag(["v0.0.2", "v0.0.10"]), Some("v0.0.10"));
        // Unparseable tags sort lowest rather than raising.
        assert_eq!(latest_tag(["vnope", "v0.0.1"]), Some("v0.0.1"));
        assert_eq!(latest_tag(["nope", "0.0.1"]), None);
    }
}
