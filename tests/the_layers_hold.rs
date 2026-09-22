//! The crate layering, enforced rather than asserted.
//!
//! Three layers only work if each can only reach the one below it. The moment
//! the terminal client can reach a file or the CLI, it starts re-deriving what
//! the daemon already decided — and a second client disagrees with the first.
//!
//! This reads the manifests. A crate's *dev*-dependencies are not checked:
//! a test that starts a real daemon to talk to is exactly what should be
//! written, and it is not in the graph of anything that ships the crate.

use std::path::Path;

/// What each crate may depend on, and the plain reason.
const ALLOWED: &[(&str, &[&str], &str)] = &[
    ("weft-core", &["serde", "serde_json"], "the rules parse and decide; they do not do"),
    (
        "weft-proto",
        &["serde_json", "weft-core"],
        "the wire carries the rules' types and nothing else",
    ),
    (
        "weftd",
        &["anyhow", "portable-pty", "serde_json", "vt100", "weft-core", "weft-proto"],
        "the daemon does: panes, files, processes",
    ),
    (
        "weft-tui",
        &[
            "anyhow",
            "crossterm",
            "ratatui",
            "serde_json",
            "tui-term",
            "vt100",
            "weft-core",
            "weft-proto",
        ],
        "the client draws, and asks the daemon for everything else",
    ),
];

fn dependencies(manifest: &str) -> Vec<String> {
    let Some(rest) = manifest.split("\n[dependencies]\n").nth(1) else {
        return Vec::new();
    };
    rest.split("\n[")
        .next()
        .unwrap_or("")
        .lines()
        .filter(|l| l.contains('=') && !l.trim_start().starts_with('#'))
        .map(|l| l.split('=').next().unwrap().trim().to_string())
        .collect()
}

#[test]
fn each_crate_reaches_only_what_its_layer_may() {
    let mut broken = Vec::new();
    for (crate_name, allowed, why) in ALLOWED {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("crates")
            .join(crate_name)
            .join("Cargo.toml");
        let manifest =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut found = dependencies(&manifest);
        found.sort();
        let mut want: Vec<String> = allowed.iter().map(|s| s.to_string()).collect();
        want.sort();
        if found != want {
            broken.push(format!("{crate_name} depends on {found:?}, not {want:?} — {why}"));
        }
    }
    assert!(broken.is_empty(), "the layers moved:\n{}", broken.join("\n"));
}

/// The client may start the daemon it talks to, and run nothing else.
///
/// The dependency check above already stops it reaching `weftd`, so it could
/// only get to RingFrame or the record by shelling out itself. This is where
/// that would show up.
#[test]
fn the_client_runs_nothing_but_the_daemon() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/weft-tui/src");
    let mut broken = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("weft-tui/src").flatten() {
        let path = entry.path();
        let text = std::fs::read_to_string(&path).expect("read");
        // Test modules sit at the end of a file and may do as they like: a
        // fixture that makes a Git repository is not the client deciding.
        let production = text.split("#[cfg(test)]").next().unwrap_or("");
        let runs: Vec<_> = production
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains("Command::new") && !l.trim_start().starts_with("//"))
            .collect();
        let is_client = path.file_name().is_some_and(|n| n == "client.rs");
        for (n, _) in runs {
            if !is_client {
                broken.push(format!("{}:{} runs a process", path.display(), n + 1));
            }
        }
        if is_client && production.matches("Command::new").count() > 1 {
            broken.push("client.rs runs more than the daemon".to_string());
        }
    }
    assert!(broken.is_empty(), "the client started doing:\n{}", broken.join("\n"));
}

/// RingFrame shares this repository; it is not part of Weft.
///
/// One repository is not one product. The moment a Weft crate can call the
/// core's Rust functions, Weft stops reading a record another program wrote
/// and starts reporting on itself — which is the whole of what makes its
/// claims worth anything.
#[test]
fn nothing_in_weft_reaches_into_ringframe() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut broken = Vec::new();
    let mut manifests = vec![("weft".to_string(), root.join("Cargo.toml"))];
    for (crate_name, ..) in ALLOWED {
        manifests.push((
            crate_name.to_string(),
            root.join("crates").join(crate_name).join("Cargo.toml"),
        ));
    }
    for (name, path) in manifests {
        let manifest =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        if dependencies(&manifest).iter().any(|d| d == "ringframe") {
            broken.push(format!("{name} depends on the ringframe crate"));
        }
    }
    // Weft reaches RingFrame by running `ringframe`, and that is the contract
    // the skills in `fab7` share with it.
    assert!(broken.is_empty(), "the product boundary moved:\n{}", broken.join("\n"));
}

#[test]
fn the_client_cannot_reach_the_daemon() {
    // The one that matters most, stated on its own so a failure says why.
    let manifest = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/weft-tui/Cargo.toml"),
    )
    .expect("weft-tui");
    assert!(
        !dependencies(&manifest).iter().any(|d| d == "weftd"),
        "weft-tui depends on weftd: the interface would stop being replaceable"
    );
}
