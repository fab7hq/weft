//! Fixtures the ported tests share. The Python suite had these in `conftest`.

use std::path::Path;
use std::process::Command;

pub use tempfile::TempDir;

pub fn tmp_dir() -> TempDir {
    tempfile::tempdir().expect("a temporary directory")
}

pub fn run(argv: &[&str]) {
    let out = Command::new(argv[0]).args(&argv[1..]).output().expect(argv[0]);
    assert!(out.status.success(), "{argv:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A fresh Git worktree root to act as a consumer workspace.
pub fn repo() -> TempDir {
    let dir = tmp_dir();
    let root = dir.path().to_string_lossy().to_string();
    run(&["git", "init", "-q", &root]);
    std::fs::write(dir.path().join("README.md"), "fixture\n").unwrap();
    run(&["git", "-C", &root, "add", "-A"]);
    run(&[
        "git", "-C", &root, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init",
    ]);
    dir
}

/// The workspace for a fixture repository, with `.fab7/rf/` already made.
pub fn ws_for(path: &Path) -> crate::workspace::Workspace {
    let ws = crate::workspace::resolve(Some(path), None).expect("resolve");
    ws.ensure().expect("ensure");
    ws
}
