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
