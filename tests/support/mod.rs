//! What the daemon tests share: an agent that behaves as `cat` and tells the
//! test which pane it runs in, and the hooks such an agent would signal.

#![allow(dead_code)]

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use weft::protocol::{Event, Line};
use weft_core::signal::Sent;

/// An agent's command line: it writes the pane variables weftd gave it to
/// `<dir>/pane-<id>`, one `NAME=value` a line, then behaves as `cat`. Run
/// through `/bin/sh`, because a script written a moment ago can still be
/// open in a process another test thread is forking, and executing it then
/// fails as busy.
pub fn agent(dir: &Path) -> String {
    std::fs::create_dir_all(dir).expect("agent dir");
    let path = dir.join("agent.sh");
    let script = "#!/bin/sh\n\
        d=$(dirname \"$0\")\n\
        printf 'WEFT_PANE=%s\\nWEFT_PANE_TOKEN=%s\\nWEFT_SOCKET=%s\\n' \
        \"$WEFT_PANE\" \"$WEFT_PANE_TOKEN\" \"$WEFT_SOCKET\" > \"$d/pane-$WEFT_PANE.tmp\"\n\
        mv \"$d/pane-$WEFT_PANE.tmp\" \"$d/pane-$WEFT_PANE\"\n\
        exec cat\n";
    std::fs::write(&path, script).expect("agent script");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    format!("/bin/sh {}", path.display())
}

/// The variables the agent in `pane` was started with, once it has said.
pub fn pane_env(dir: &Path, pane: u32) -> Vec<(String, String)> {
    let path = dir.join(format!("pane-{pane}"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < deadline, "the agent in pane {pane} never said where it is");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::read_to_string(path)
        .expect("pane file")
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

pub fn token_of(dir: &Path, pane: u32) -> String {
    pane_env(dir, pane).into_iter().find(|(k, _)| k == "WEFT_PANE_TOKEN").expect("token").1
}

/// One `signal` line to the daemon, as `weft signal` sends it.
pub fn signal(socket: &Path, sent: Sent) {
    let mut s = UnixStream::connect(socket).expect("the daemon");
    s.write_all(&Line::Event(Event::Signal(Box::new(sent))).encode()).expect("sent");
}

/// What the agent in `pane` (started from [`agent`] in `dir`) signals:
/// `event`, of `session`, in `root`.
pub fn pane_says(
    socket: &Path,
    dir: &Path,
    root: &Path,
    harness: &str,
    pane: u32,
    session: &str,
    event: &str,
) {
    signal(
        socket,
        Sent {
            harness: harness.into(),
            hook: "Hook".into(),
            session: session.into(),
            workspace: Some(root.to_string_lossy().into_owned()),
            state: Some(event.into()),
            pane: Some(pane),
            token: Some(token_of(dir, pane)),
            ..Sent::default()
        },
    );
}

/// Where a test's agents tell it which pane they are in.
pub fn agents_dir(socket: &Path) -> PathBuf {
    socket.with_extension("agents")
}

/// A real RingFrame record at `root`, as the daemon finds one: a Git
/// repository, RingFrame's test configuration in `home`, and an Ask for the
/// harness `host`, compiled and confirmed, followed by a commit of work.
/// `HOME` must already be `home`. Returns the Ask's id.
pub fn real_ask(root: &Path, home: &Path, host: &str) -> String {
    use ringframe::api;
    std::fs::create_dir_all(root).unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("/usr/bin/git")
            .arg("-C")
            .arg(root)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap();
        assert!(ok.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    assert_eq!(std::env::var_os("HOME").as_deref(), Some(home.as_os_str()));
    let config =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/ringframe/tests/fixtures/config");
    ringframe::workspace::install_config(Some(&config)).unwrap();
    let ws = api::workspace(root).unwrap();
    ws.ensure().unwrap();
    let overrides = ringframe::config::Overrides::default();
    let compiled = api::compile(
        &ws,
        api::Compile {
            source: "Ship it.",
            harness: api::Harness { name: host, version: None, session_ref: None },
            capability: "human_handoff",
            classification: api::Classification {
                task: vec!["plan".into()],
                result: "plan".into(),
                effects: vec!["read".into()],
            },
            plan: None,
            practices: Vec::new(),
            classified_by: api::Chooser::Person,
            selected_by: api::Chooser::Person,
            links: Vec::new(),
            overrides: &overrides,
            via: "weft",
        },
    )
    .unwrap();
    std::fs::write(root.join("work.txt"), "the work\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "work"]);
    compiled.ask_id
}
