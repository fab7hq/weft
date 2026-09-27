//! No harness in either program's code (ADR-0015), and no screen text Weft
//! reads to guess an agent's state (ADR-0013): harness-profile.md §5 test 2,
//! turn-state.md §6 test 7.
//!
//! Weft names no harness anywhere it ships or runs: not in its code or its
//! comments, its examples, its scripts or its installer. The words are every
//! harness's id, title and program, read from Weft's own fixture harness files,
//! so a harness added there is covered here without an edit. RingFrame's code
//! may know its hosts' profiles by name no more than as quoted literals.
//!
//! Weft reads a pane's screen only to guard its own typing, never to guess an
//! agent's state: the echo of what it typed, a folded paste, the mode a prompt
//! entered, whether a started agent has gone still, and the paste mode the
//! agent asked for. Drawing a pane for the person is not a read.

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

/// The only functions that read a pane's screen, and why each may.
const SCREEN_READS: &[&str] = &[
    "wait_for_echo",    // the echo of what Weft typed
    "folded_command",   // a paste the harness folded away
    "wait_for",         // the mode a prompt entered
    "gone_quiet",       // a started agent that has gone still
    "bracketed_paste",  // the paste mode the agent asked for, not its words
    "with_pane_screen", // drawing the pane for the person
];

/// Every harness a Weft fixture harness file defines: its id, its title and
/// its program.
fn harness_words() -> Vec<String> {
    let dir = root().join("crates/weft-core/tests/fixtures/harnesses");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("fixture harness files").flatten() {
        let path = entry.path();
        out.push(path.file_stem().and_then(|s| s.to_str()).expect("an id").to_string());
        let text = std::fs::read_to_string(&path).expect("harness file");
        for key in ["title", "program"] {
            if let Some(value) = text
                .lines()
                .find(|l| l.starts_with(&format!("{key} = ")))
                .and_then(|l| l.split('"').nth(1))
            {
                out.push(value.to_string());
            }
        }
    }
    assert!(out.iter().any(|w| w == "agy"), "an agy fixture: {out:?}");
    assert!(out.len() >= 9, "the fixture files named too few harnesses: {out:?}");
    out.sort();
    out.dedup();
    out
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every file under these directories and files, with its text.
fn files(roots: &[&str]) -> Vec<(PathBuf, String)> {
    fn walk(path: &Path, out: &mut Vec<PathBuf>) {
        if path.is_dir() {
            for entry in std::fs::read_dir(path).expect("a directory").flatten() {
                walk(&entry.path(), out);
            }
        } else {
            out.push(path.to_path_buf());
        }
    }
    let mut paths = Vec::new();
    for r in roots {
        walk(&root().join(r), &mut paths);
    }
    paths
        .into_iter()
        // The CLI's shared test fixtures are a test module of their own, and
        // so is any `tests.rs`, declared under `#[cfg(test)]`; a snapshot is
        // a test's expected screen.
        .filter(|p| !p.ends_with("ringframe/src/testing.rs") && !p.ends_with("tests.rs"))
        .filter(|p| !p.components().any(|c| c.as_os_str() == "snapshots"))
        .filter_map(|p| std::fs::read_to_string(&p).ok().map(|text| (p, text)))
        .collect()
}

/// Each non-test line of a file, with its number: a Rust file's
/// `#[cfg(test)]` items left out, and its comments too unless `comments`.
fn lines(path: &Path, text: &str, comments: bool) -> Vec<(usize, String)> {
    if path.extension().is_none_or(|e| e != "rs") {
        return text.lines().enumerate().map(|(n, l)| (n + 1, l.to_string())).collect();
    }
    let mut out = Vec::new();
    let (mut skipping, mut depth, mut pending) = (false, 0i64, false);
    for (n, line) in text.lines().enumerate() {
        let t = line.trim_start();
        if skipping || pending {
            // Inside a `#[cfg(test)]` item: skip until its braces close.
            depth += line.matches('{').count() as i64 - line.matches('}').count() as i64;
            if pending && line.contains('{') {
                (pending, skipping) = (false, true);
            }
            if skipping && depth <= 0 {
                (skipping, depth) = (false, 0);
            }
            continue;
        }
        if t.starts_with("#[cfg(test)]") || t.starts_with("#[cfg(any(test") {
            (pending, depth) = (true, 0);
            continue;
        }
        if comments || !t.starts_with("//") {
            out.push((n + 1, line.to_string()));
        }
    }
    out
}

/// Whether `word` is in `line` as a whole word, in any case.
fn names(line: &str, word: &str) -> bool {
    let (line, word) = (line.to_lowercase(), word.to_lowercase());
    let edge = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
    line.match_indices(&word).any(|(at, _)| {
        edge(line[..at].chars().next_back()) && edge(line[at + word.len()..].chars().next())
    })
}

const WEFT: &[&str] = &[
    "src",
    "crates/weft-core/src",
    "crates/weft-proto/src",
    "crates/weft-tui/src",
    "crates/weftd/src",
];

#[test]
fn weft_names_no_harness_in_what_it_ships_or_runs() {
    let words = harness_words();
    let scanned = [WEFT, &["examples", "bin", "install.sh"]].concat();
    let mut found = Vec::new();
    for (path, text) in files(&scanned) {
        for (n, line) in lines(&path, &text, true) {
            for w in words.iter().filter(|w| names(&line, w)) {
                found.push(format!("{}:{n} names {w}: {}", path.display(), line.trim()));
            }
        }
    }
    assert!(found.is_empty(), "Weft still names a harness:\n{}", found.join("\n"));
}

#[test]
fn no_hook_or_screen_text_is_left_in_either_programs_code() {
    let quoted: Vec<String> = harness_words().iter().map(|w| format!("\"{w}\"")).collect();
    let mut found = Vec::new();
    for (path, text) in files(&[WEFT, &["crates/ringframe/src"]].concat()) {
        let ringframe = path.starts_with(root().join("crates/ringframe"));
        for (n, line) in lines(&path, &text, false) {
            let names = quoted.iter().filter(|_| ringframe).map(String::as_str);
            for needle in names.chain(HOOKS.iter().copied()).chain(SCREEN.iter().copied()) {
                if line.contains(needle) {
                    found.push(format!("{}:{n} names {needle}: {}", path.display(), line.trim()));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "the code still names what only a profile, a harness file or a plugin may:\n{}",
        found.join("\n")
    );
}

#[test]
fn weft_reads_a_screen_only_where_it_guards_its_own_typing() {
    let mut found = Vec::new();
    for (path, text) in files(WEFT) {
        let mut inside = String::new();
        for (n, line) in lines(&path, &text, false) {
            if let Some(name) = line.split("fn ").nth(1).and_then(|r| r.split(['(', '<']).next()) {
                inside = name.trim().to_string();
            }
            if line.contains("with_screen(") && !SCREEN_READS.contains(&inside.as_str()) {
                found.push(format!("{}:{n} in {inside}: {}", path.display(), line.trim()));
            }
        }
    }
    assert!(found.is_empty(), "a new screen read:\n{}", found.join("\n"));
}
