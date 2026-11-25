//! Read-only copies of the product for an Eval's reader: each repository as it
//! was at a commit, unpacked once and shared by every Eval of that commit, and,
//! for work not yet committed, a copy of the files Git lists, made for one Eval.
//! They live in RingFrame's cache, `~/.fab7/cache/<workspace id>/`, outside
//! every repository. The cache is RingFrame's alone: made and pruned only
//! here, never a path a caller names.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use crate::digest;
use crate::workspace::Workspace;

/// The cache is pruned, least recently used first, once it holds more than this.
pub const LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The workspace's folder in the cache: the first 12 hex of its root's digest.
pub fn cache_dir(ws: &Workspace) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(cache_in(&PathBuf::from(home).join(".fab7/cache"), ws))
}

fn cache_in(base: &Path, ws: &Workspace) -> PathBuf {
    let id = digest::sha256_bytes(ws.root.to_string_lossy().as_bytes());
    base.join(&id[..12])
}

/// A repository's folder name in the cache, from its root.
fn repo_key(root: &Path) -> String {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let id = digest::sha256_bytes(canonical.to_string_lossy().as_bytes());
    id[..12].to_string()
}

fn io(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The workspace's folder in the cache, made and kept for its owner alone.
fn private(base: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(base).map_err(io)?;
    std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o700)).map_err(io)
}

/// Make a tree read-only, or writable again by its owner so it can be
/// removed. Only the write bits change: a file only its owner could read
/// stays so.
fn set_writable(path: &Path, writable: bool) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    if meta.file_type().is_symlink() {
        return;
    }
    let mode_of = |mode: u32| if writable { mode | 0o200 } else { mode & !0o222 };
    let dir = meta.is_dir();
    let mode = mode_of(meta.permissions().mode());
    if dir && writable {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    if dir && let Ok(entries) = std::fs::read_dir(path) {
        for e in entries.flatten() {
            set_writable(&e.path(), writable);
        }
    }
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

fn remove(path: &Path) {
    if path.exists() {
        set_writable(path, true);
        let _ = std::fs::remove_dir_all(path);
    }
}

/// Record that `eval` uses `dir`, and when it was last used.
fn mark(meta: &Path, eval: &str) -> Result<(), String> {
    std::fs::create_dir_all(meta.join("users")).map_err(io)?;
    std::fs::write(meta.join("users").join(eval), b"").map_err(io)?;
    let mut f = std::fs::File::create(meta.join("used")).map_err(io)?;
    f.write_all(format!("{:?}\n", SystemTime::now()).as_bytes()).map_err(io)
}

/// The repository at `root` as it is at `commit`, read-only, made the first
/// time any Eval asks for it; `eval` is recorded as using it.
pub fn at_commit(ws: &Workspace, root: &Path, commit: &str, eval: &str) -> Result<PathBuf, String> {
    let base = cache_dir(ws).ok_or("HOME is not set")?;
    at_commit_in(&base, root, commit, eval)
}

fn at_commit_in(base: &Path, root: &Path, commit: &str, eval: &str) -> Result<PathBuf, String> {
    let key = repo_key(root);
    let dir = base.join("snapshots").join(&key).join(commit);
    let meta = base.join("meta").join(&key).join(commit);
    private(base)?;
    if !dir.join(".complete").exists() {
        remove(&dir);
        let parent = dir.parent().expect("a snapshot has a parent");
        std::fs::create_dir_all(parent).map_err(io)?;
        let tmp =
            parent.join(format!(".{commit}.{}.{}", std::process::id(), crate::ids::random_hex(4)));
        std::fs::create_dir_all(&tmp).map_err(io)?;
        unpack(root, commit, &tmp)?;
        std::fs::write(tmp.join(".complete"), b"").map_err(io)?;
        set_writable(&tmp, false);
        if std::fs::rename(&tmp, &dir).is_err() {
            // Another Eval made it first: theirs is the same tree.
            remove(&tmp);
        }
    }
    mark(&meta, eval)?;
    Ok(dir)
}

fn unpack(root: &Path, commit: &str, into: &Path) -> Result<(), String> {
    let mut archive = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["archive", "--format=tar", commit])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(io)?;
    let out = archive.stdout.take().ok_or("git archive gave no output")?;
    let tar = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(into)
        .stdin(out)
        .stderr(Stdio::null())
        .status()
        .map_err(io)?;
    let git = archive.wait().map_err(io)?;
    if git.success() && tar.success() {
        Ok(())
    } else {
        Err(format!("{commit} could not be unpacked from {}", root.display()))
    }
}

/// The repository's worktree as Git lists it (tracked and untracked, not
/// ignored), copied read-only for `eval` alone.
pub fn of_worktree(ws: &Workspace, root: &Path, eval: &str) -> Result<PathBuf, String> {
    let base = cache_dir(ws).ok_or("HOME is not set")?;
    of_worktree_in(&base, root, eval)
}

fn of_worktree_in(base: &Path, root: &Path, eval: &str) -> Result<PathBuf, String> {
    let dir = base.join("evals").join(eval).join("after").join(repo_key(root));
    private(base)?;
    remove(&dir);
    std::fs::create_dir_all(&dir).map_err(io)?;
    let listed = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-co", "--exclude-standard", "-z"])
        .output()
        .map_err(io)?;
    for rel in String::from_utf8_lossy(&listed.stdout).split('\0').filter(|p| !p.is_empty()) {
        // A link is copied as the link, as Git keeps it: never the file it
        // points to, which may be outside the repository.
        let from = root.join(rel);
        let Ok(meta) = std::fs::symlink_metadata(&from) else { continue };
        let to = dir.join(rel);
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&from).map_err(io)?;
            std::fs::create_dir_all(to.parent().expect("a link has a folder")).map_err(io)?;
            std::os::unix::fs::symlink(target, &to).map_err(io)?;
        } else if meta.is_file() {
            std::fs::create_dir_all(to.parent().expect("a file has a folder")).map_err(io)?;
            std::fs::copy(&from, &to).map_err(io)?;
        }
    }
    set_writable(&dir, false);
    Ok(dir)
}

/// What `eval` held: its worktree copies go, and it no
/// longer keeps any shared snapshot from being pruned.
pub fn release(ws: &Workspace, eval: &str) {
    if let Some(base) = cache_dir(ws) {
        release_in(&base, eval);
    }
}

fn release_in(base: &Path, eval: &str) {
    remove(&base.join("evals").join(eval));
    for user in users(base) {
        if user.file_name().is_some_and(|n| n == eval) {
            let _ = std::fs::remove_file(&user);
        }
    }
}

fn users(base: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for repo in std::fs::read_dir(base.join("meta")).into_iter().flatten().flatten() {
        for commit in std::fs::read_dir(repo.path()).into_iter().flatten().flatten() {
            for u in std::fs::read_dir(commit.path().join("users")).into_iter().flatten().flatten()
            {
                out.push(u.path());
            }
        }
    }
    out
}

fn size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if meta.is_dir() {
        std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| size(&e.path())).sum()
    } else {
        meta.len()
    }
}

/// Remove the least recently used snapshots until the cache holds at most
/// `limit` bytes, never one an open Eval uses (`open` says which are).
pub fn prune(ws: &Workspace, limit: u64, open: &dyn Fn(&str) -> bool) -> Vec<PathBuf> {
    cache_dir(ws).map(|b| prune_in(&b, limit, open)).unwrap_or_default()
}

fn prune_in(base: &Path, limit: u64, open: &dyn Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut snaps: Vec<(SystemTime, PathBuf, PathBuf, u64)> = Vec::new();
    for repo in std::fs::read_dir(base.join("snapshots")).into_iter().flatten().flatten() {
        for commit in std::fs::read_dir(repo.path()).into_iter().flatten().flatten() {
            let name = commit.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let meta = base.join("meta").join(repo.file_name()).join(&name);
            let held = std::fs::read_dir(meta.join("users"))
                .into_iter()
                .flatten()
                .flatten()
                .any(|u| open(&u.file_name().to_string_lossy()));
            if held {
                continue;
            }
            let used = std::fs::metadata(meta.join("used"))
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            snaps.push((used, commit.path(), meta, size(&commit.path())));
        }
    }
    let mut total = size(&base.join("snapshots"));
    snaps.sort_by_key(|s| s.0);
    let mut removed = Vec::new();
    for (_, dir, meta, bytes) in snaps {
        if total <= limit {
            break;
        }
        remove(&dir);
        remove(&meta);
        total = total.saturating_sub(bytes);
        removed.push(dir);
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{commit, repo, tmp_dir, ws_for};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_snapshot_is_the_commit_read_only_and_shared_by_two_evals() {
        let dir = repo();
        let root = dir.path();
        let ws = ws_for(root);
        let cache = tmp_dir();
        let base = cache_in(cache.path(), &ws);
        let one = commit(root, &[("src/a.rs", Some("one\n"))], "one");
        commit(root, &[("src/a.rs", Some("two\n"))], "two");
        let a = at_commit_in(&base, root, &one, "ev_1").unwrap();
        let b = at_commit_in(&base, root, &one, "ev_2").unwrap();
        assert_eq!(a, b, "one snapshot per commit");
        assert_eq!(std::fs::read_to_string(a.join("src/a.rs")).unwrap(), "one\n");
        assert!(std::fs::write(a.join("src/a.rs"), "x").is_err(), "read-only");
        assert!(!a.starts_with(root), "outside the repository");
    }

    #[test]
    fn a_worktree_copy_holds_what_git_lists_and_goes_with_its_eval() {
        let dir = repo();
        let root = dir.path();
        let ws = ws_for(root);
        let cache = tmp_dir();
        let base = cache_in(cache.path(), &ws);
        commit(root, &[(".gitignore", Some("out/\n")), ("a.txt", Some("one\n"))], "base");
        std::fs::write(root.join("a.txt"), "edited\n").unwrap();
        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("out/build.bin"), "x").unwrap();
        let outside = tmp_dir();
        std::fs::write(outside.path().join("secret"), "key\n").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), root.join("link")).unwrap();
        std::fs::write(root.join("mine.txt"), "only me\n").unwrap();
        std::fs::set_permissions(root.join("mine.txt"), std::fs::Permissions::from_mode(0o600))
            .unwrap();
        let copy = of_worktree_in(&base, root, "ev_1").unwrap();
        assert_eq!(std::fs::read_to_string(copy.join("a.txt")).unwrap(), "edited\n");
        assert!(copy.join("new.txt").exists() && !copy.join("out").exists());
        // A link stays a link; a file only its owner reads stays so, read-only.
        let link = std::fs::symlink_metadata(copy.join("link")).unwrap();
        assert!(link.file_type().is_symlink());
        let mode = std::fs::metadata(copy.join("mine.txt")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o400);
        let base_mode = std::fs::metadata(&base).unwrap().permissions().mode();
        assert_eq!(base_mode & 0o777, 0o700, "the cache is its owner's alone");
        release_in(&base, "ev_1");
        assert!(!copy.exists());
    }

    #[test]
    fn pruning_removes_the_least_recently_used_and_never_one_in_use() {
        let dir = repo();
        let root = dir.path();
        let ws = ws_for(root);
        let cache = tmp_dir();
        let base = cache_in(cache.path(), &ws);
        let c1 = commit(root, &[("a.txt", Some("one\n"))], "1");
        let c2 = commit(root, &[("a.txt", Some("two\n"))], "2");
        let c3 = commit(root, &[("a.txt", Some("three\n"))], "3");
        let s1 = at_commit_in(&base, root, &c1, "ev_old").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let s2 = at_commit_in(&base, root, &c2, "ev_open").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let s3 = at_commit_in(&base, root, &c3, "ev_new").unwrap();
        let removed = prune_in(&base, 0, &|e| e == "ev_open");
        assert!(!s1.exists() && !s3.exists(), "{removed:?}");
        assert!(s2.exists(), "an open Eval's snapshot is kept");
        assert_eq!(removed.first(), Some(&s1), "oldest first");
    }

    #[test]
    fn released_snapshots_can_be_pruned() {
        let dir = repo();
        let root = dir.path();
        let ws = ws_for(root);
        let cache = tmp_dir();
        let base = cache_in(cache.path(), &ws);
        let c1 = commit(root, &[("a.txt", Some("one\n"))], "1");
        let s1 = at_commit_in(&base, root, &c1, "ev_1").unwrap();
        release_in(&base, "ev_1");
        prune_in(&base, 0, &|e| e == "ev_1");
        assert!(!s1.exists(), "released, so no longer held");
    }
}
