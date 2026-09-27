//! No harness in either program's code (ADR-0015), and no screen text Weft
//! reads to guess an agent's state (ADR-0013): harness-profile.md §5 test 2,
//! turn-state.md §6 test 7.
//!
//! This reads the non-test source of Weft's crates and of the `ringframe` CLI.
//! Comments and test modules may name anything; code may not. The harness
//! names come from RingFrame's own fixture profiles, so a harness added there
//! is covered here without an edit.
//!
//! Known exception, recorded in the plan (Phase 3.16 step 7): the checks that
//! guard typing itself — the echo of what Weft typed, the harnesses' folded-
//! paste placeholders and the mode a prompt entered — still read the screen.
//! They guess nothing about an agent's state, and they are not listed here.

use std::path::{Path, PathBuf};

/// Hook names, which only a plugin's `hooks.json` may know.
const HOOKS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PermissionRequest",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PreInvocation",
    "PostInvocation",
    "\"Stop\"",
    "\"Notification\"",
    "conversationId",
    "AskUserQuestion",
];

/// The screen text `blocked.rs` matched to decide an agent was waiting or busy.
const SCREEN: &[&str] = &[
    "Press enter to continue",
    "Enter to confirm",
    "Allow command?",
    "Do you want to proceed?",
    "Do you trust",
    "trust this folder",
    "Update available",
    "Press t to trust",
    "esc to interrupt",
    "looks_blocked",
    "looks_busy",
    "idle_pane",
];

/// Every harness a Weft fixture harness file defines: its id, title,
/// program and configuration, each as a quoted literal.
fn harness_names() -> Vec<String> {
    let dir = root().join("crates/weft-core/tests/fixtures/harnesses");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("fixture harness files").flatten() {
        let path = entry.path();
        let id = path.file_stem().and_then(|s| s.to_str()).expect("an id").to_string();
        out.push(format!("\"{id}\""));
        let text = std::fs::read_to_string(&path).expect("harness file");
        for key in ["title", "program", "env", "default"] {
            for line in text.lines().filter(|l| l.starts_with(&format!("{key} = "))) {
                if let Some(value) = line.split('"').nth(1) {
                    out.push(format!("\"{value}\""));
                }
            }
        }
    }
    assert!(out.len() > 8, "the fixture files named too few harnesses: {out:?}");
    out
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Each non-test, non-comment line of Rust source under these directories.
fn code(dirs: &[&str]) -> Vec<(String, usize, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("src").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    for d in dirs {
        walk(&root().join(d), &mut files);
    }
    let mut out = Vec::new();
    for path in files {
        // The CLI's shared test fixtures are a test module of their own, and
        // so is any `tests.rs`, declared under `#[cfg(test)]`.
        if path.ends_with("ringframe/src/testing.rs") || path.ends_with("tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let mut skipping = 0usize;
        let mut depth = 0i64;
        let mut pending_test = false;
        for (n, line) in text.lines().enumerate() {
            let t = line.trim_start();
            if skipping > 0 || pending_test {
                // Inside a `#[cfg(test)]` item: skip until its braces close.
                depth += line.matches('{').count() as i64 - line.matches('}').count() as i64;
                if pending_test && line.contains('{') {
                    pending_test = false;
                    skipping = 1;
                }
                if skipping > 0 && depth <= 0 {
                    skipping = 0;
                    depth = 0;
                }
                continue;
            }
            if t.starts_with("#[cfg(test)]") || t.starts_with("#[cfg(any(test") {
                pending_test = true;
                depth = 0;
                continue;
            }
            if t.starts_with("//") {
                continue;
            }
            out.push((path.display().to_string(), n + 1, line.to_string()));
        }
    }
    assert!(out.len() > 1000, "found too little code to check");
    out
}

#[test]
fn no_harness_hook_or_screen_text_is_left_in_either_programs_code() {
    let names = harness_names();
    let lines = code(&[
        "src",
        "crates/weft-core/src",
        "crates/weft-proto/src",
        "crates/weft-tui/src",
        "crates/weftd/src",
        "crates/ringframe/src",
    ]);
    let mut found = Vec::new();
    for (path, n, line) in &lines {
        let words = names.iter().map(String::as_str).chain(HOOKS.iter().copied());
        for needle in words.chain(SCREEN.iter().copied()) {
            if line.contains(needle) {
                found.push(format!("{path}:{n} names {needle}: {}", line.trim()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "the code still names what only a profile or plugin may:\n{}",
        found.join("\n")
    );
}
