//! ADR-0007's invariant, enforced rather than asserted.
//!
//! `weft-core` holds the rules and nothing that does anything. The moment it
//! can reach a file, a process, the network or a terminal, a second client
//! stops being able to share the rules with the first, because using them
//! would mean doing something.
//!
//! This reads the crate's own source. A grep is a blunt instrument, and it is
//! the right one here: the thing being prevented is someone typing
//! `std::fs::read` without noticing what it costs.

use std::path::Path;

/// What the rules may not touch, and the plain reason for each.
const FORBIDDEN: &[(&str, &str)] = &[
    ("std::fs", "the rules must be testable without a filesystem"),
    ("std::process", "running something is the shell's job, not a rule's"),
    ("std::net", "nothing here talks to anything"),
    ("std::env", "a rule that reads the environment is not a rule"),
    ("ratatui", "the rules must outlive the interface"),
    ("crossterm", "the rules must outlive the interface"),
    ("portable_pty", "a pane is the daemon's"),
];

fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("src").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("read");
                out.push((path.display().to_string(), text));
            }
        }
    }
    let mut out = Vec::new();
    walk(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut out);
    assert!(!out.is_empty(), "found no source to check");
    out
}

#[test]
fn the_rules_reach_nothing() {
    let mut broken = Vec::new();
    for (path, text) in sources() {
        for (line_no, line) in text.lines().enumerate() {
            let code = line.trim_start();
            // A doc comment may name any of these; it is saying why not to.
            if code.starts_with("//") {
                continue;
            }
            for (needle, why) in FORBIDDEN {
                if code.contains(needle) {
                    broken.push(format!("{path}:{} uses {needle} — {why}", line_no + 1));
                }
            }
        }
    }
    assert!(broken.is_empty(), "weft-core stopped being only rules:\n{}", broken.join("\n"));
}

#[test]
fn the_rules_depend_on_one_thing_that_parses() {
    // Every dependency added here has to keep the invariant true, so there
    // should be almost none, and a reader should be able to see that at once.
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("Cargo.toml");
    let deps: Vec<&str> = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("a dependencies section")
        .lines()
        .filter(|l| l.contains('='))
        .map(|l| l.split('=').next().unwrap().trim())
        .collect();
    assert_eq!(
        deps,
        vec!["serde", "serde_json"],
        "a new dependency has to earn its place here, and both of these only parse"
    );
}
