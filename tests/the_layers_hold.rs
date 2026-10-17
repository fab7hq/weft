//! The crate layering, enforced rather than asserted.
//!
//! Three layers only work if each can only reach the one below it. The moment
//! the app can reach a file or the CLI, it starts re-deriving what
//! the daemon already decided — and a second client disagrees with the first.
//!
//! This reads the manifests. A crate's *dev*-dependencies are not checked:
//! a test that starts a real daemon to talk to is exactly what should be
//! written, and it is not in the graph of anything that ships the crate.

use std::path::Path;

/// What each crate may depend on, and the plain reason.
const ALLOWED: &[(&str, &[&str], &str)] = &[
    (
        "weft-core",
        &["serde", "serde_json", "sha2", "toml"],
        "the rules parse, hash and decide; they do not do",
    ),
    (
        "weft-proto",
        &["serde_json", "weft-core"],
        "the wire carries the rules' types and nothing else",
    ),
    (
        "weft-sense",
        &["anyhow", "serde", "serde_json", "sha2"],
        "the dual-head classifier normalizes screens and transcript entries and runs its bundle; \
         it never touches panes, the ledger, or the wire",
    ),
    (
        "weft-term",
        &["alacritty_terminal"],
        "the terminal emulates and writes escape codes; it reaches nothing",
    ),
    (
        "weftd",
        &[
            "anyhow",
            "libc",
            "portable-pty",
            "serde_json",
            "toml",
            "tungstenite",
            "weft-core",
            "weft-proto",
            "weft-sense",
            "weft-term",
        ],
        "the daemon does: panes, files, processes (libc reads a hook's harness process), \
         listens to a harness's own stream (tungstenite frames it), and refines turns/conversations \
         with weft-sense",
    ),
    (
        "weft-desktop",
        &[
            "anyhow",
            "gpui",
            "gpui_platform",
            "serde",
            "serde_json",
            "unicode-segmentation",
            "weft-core",
            "weft-proto",
            "weft-term",
        ],
        "the desktop client draws the Toile screens with GPUI, edits its text fields a whole \
         character at a time, persists UI state, and asks the daemon for everything else; never \
         weftd",
    ),
    (
        "weft-remote",
        &[
            "anyhow",
            "base64ct",
            "ed25519-dalek",
            "getrandom",
            "http",
            "libc",
            "p256",
            "serde_json",
            "sha2",
            "toml",
            "tungstenite",
            "ureq",
            "web-push-native",
            "weft-core",
            "weft-proto",
        ],
        "another device's way in: a socket client like the desktop app, the tunnel, the WebSocket \
         (tungstenite), HTTPS (ureq with rustls), Web Push (web-push-native, its VAPID JWT signed \
         with p256), Fab7's assertions (ed25519-dalek), its own randomness (getrandom) and a bounded \
         socket buffer for each device (libc); never weftd",
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
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/weft-desktop/src");
    let mut broken = Vec::new();
    for path in sources(&dir) {
        let text = std::fs::read_to_string(&path).expect("read");
        // Test modules sit at the end of a file and may do as they like: a
        // fixture that makes a Git repository is not the client deciding.
        let production = text.split("#[cfg(test)]").next().unwrap_or("");
        let runs: Vec<_> = production
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains("Command::new") && !l.trim_start().starts_with("//"))
            .collect();
        let is_client = path.file_name().is_some_and(|n| n == "daemon.rs");
        for (n, _) in runs {
            if !is_client {
                broken.push(format!("{}:{} runs a process", path.display(), n + 1));
            }
        }
        if is_client && production.matches("Command::new").count() > 1 {
            broken.push("daemon.rs runs more than the daemon".to_string());
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
        Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/weft-desktop/Cargo.toml"),
    )
    .expect("weft-desktop");
    assert!(
        !dependencies(&manifest).iter().any(|d| d == "weftd"),
        "weft-desktop depends on weftd: the interface would stop being replaceable"
    );
}

/// Every Rust file under a directory, but a `tests.rs`, which is a test
/// module by its declaration.
fn sources(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("src").flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(sources(&path));
        } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("tests.rs") {
            out.push(path);
        }
    }
    out
}

/// Each line of a crate's non-test Rust source: `#[cfg(test)]` items left
/// out, and comments too unless `comments`.
fn non_test_lines(dir: &Path, comments: bool) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for path in sources(dir) {
        let text = std::fs::read_to_string(&path).expect("read");
        let (mut depth, mut skipping, mut pending) = (0i64, false, false);
        for line in text.lines() {
            let t = line.trim_start();
            if pending && !line.contains('{') && line.trim_end().ends_with(';') {
                // A test item with no body, such as `mod tests;`.
                pending = false;
                continue;
            }
            if skipping || pending {
                depth += line.matches('{').count() as i64 - line.matches('}').count() as i64;
                if pending && line.contains('{') {
                    (pending, skipping) = (false, true);
                }
                if skipping && depth <= 0 {
                    (skipping, depth) = (false, 0);
                }
                continue;
            }
            if t.starts_with("#[cfg(test)]") {
                (pending, depth) = (true, 0);
            } else if comments || !t.starts_with("//") {
                out.push((path.display().to_string(), line.to_string()));
            }
        }
    }
    out
}

/// The client waits on two threads of its own and nowhere else: the one that
/// takes what the daemon sent, and the one that reconnects. Neither is the
/// window's.
#[test]
fn the_client_sleeps_only_on_its_own_threads() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/weft-desktop/src");
    let lines = non_test_lines(&src, false);
    assert!(lines.len() > 1000, "found too little code to check");
    let sleeping: Vec<_> = lines
        .iter()
        .filter(|(_, l)| l.contains("thread::sleep"))
        .map(|(p, l)| format!("{p}: {}", l.trim()))
        .collect();
    let own = sleeping.iter().filter(|l| l.contains("window.rs")).count();
    assert!(
        own == 2 && sleeping.len() == 2,
        "the client sleeps elsewhere:\n{}",
        sleeping.join("\n")
    );
}

/// NEEDS YOU is counted in `weft_core::turns` only: the client asks it,
/// and compares no agent's state to `waiting` itself.
#[test]
fn the_client_counts_nothing_that_needs_you_itself() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/weft-desktop/src");
    let found: Vec<_> = non_test_lines(&src, false)
        .into_iter()
        .filter(|(_, l)| l.contains("Turn::Waiting"))
        .map(|(p, l)| format!("{p}: {}", l.trim()))
        .collect();
    assert!(found.is_empty(), "the client decides NEEDS YOU:\n{}", found.join("\n"));
}

/// RingFrame is not aware of Weft: its code says what RingFrame does and who
/// may call it, a person or a tool, and its shipped product, when the fab7
/// checkout is beside this one, says the same. Only its plugins' hooks run
/// `weft signal`, through the `weft-signal` shim, because one release
/// installs both.
#[test]
fn ringframe_names_no_weft() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found: Vec<String> = non_test_lines(&here.join("crates/ringframe/src"), true)
        .into_iter()
        .filter(|(p, l)| !p.ends_with("testing.rs") && l.to_lowercase().contains("weft"))
        .map(|(p, l)| format!("{p}: {}", l.trim()))
        .collect();
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() { walk(&path, out) } else { out.push(path) }
        }
    }
    let mut shipped = Vec::new();
    walk(&here.join("../fab7/products/ringframe"), &mut shipped);
    for path in shipped.into_iter().filter(|p| !p.ends_with("weft-signal")) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let hook = |l: &str| l.contains("weft-signal") || l.contains("weft signal");
        for line in text.lines().filter(|l| l.to_lowercase().contains("weft") && !hook(l)) {
            found.push(format!("{}: {}", path.display(), line.trim()));
        }
    }
    assert!(found.is_empty(), "RingFrame names Weft:\n{}", found.join("\n"));
}
