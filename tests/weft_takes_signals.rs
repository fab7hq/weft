//! weftd takes the signals: its socket is the
//! user's alone, a harness it starts is told its pane, its token and its
//! socket, a signal moves only the pane whose token it carries, one with no
//! pane is a session no pane holds, a wrong token records nothing, and the
//! receipts outlive the daemon.

mod support;

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;
use weft::turns::Turn;
use weft_core::signal::Sent;

struct Client {
    next: u64,
    stream: UnixStream,
    lines: Lines<UnixStream>,
    /// Each pane's turn and the sessions carried, as the open answer and
    /// every `agents` event since have told them.
    turns: Vec<Option<Turn>>,
    rows: Vec<weft_core::sessions::Row>,
}

impl Client {
    fn attach(socket: &Path, root: &Path) -> Self {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            if let Ok(s) = UnixStream::connect(socket) {
                break s;
            }
            assert!(Instant::now() < deadline, "the daemon never came up");
            std::thread::sleep(Duration::from_millis(20));
        };
        let lines = Lines::new(stream.try_clone().expect("clone"));
        let mut c = Client { next: 1, stream, lines, turns: Vec::new(), rows: Vec::new() };
        c.answer(Call::Hello { client: "test".into(), protocol: PROTOCOL });
        let path = root.to_string_lossy().into_owned();
        let opened = c.answer(Call::Open { path, rows: 24, cols: 80, raw: false });
        c.turns = serde_json::from_value(opened["agents"]["panes"].clone()).unwrap();
        c.rows = serde_json::from_value(opened["agents"]["sessions"].clone()).unwrap();
        c
    }

    /// What an `agents` event moved, taken.
    fn take(&mut self, line: &Line) -> bool {
        let Line::Event(Event::Agents { panes, changed, gone, .. }) = line else { return false };
        self.turns = panes.clone();
        weft_core::sessions::apply(&mut self.rows, changed, gone);
        true
    }

    fn send(&mut self, call: Call) -> u64 {
        let id = self.next;
        self.next += 1;
        protocol::send(&mut self.stream, &Line::Call { id, call }.encode()).expect("send");
        id
    }

    fn answer(&mut self, call: Call) -> serde_json::Value {
        let id = self.send(call);
        loop {
            match self.lines.next().expect("read").expect("a line") {
                Line::Result { id: got, result } if got == id => return result,
                Line::Error { id: got, message, .. } if got == id => panic!("{message}"),
                other => {
                    self.take(&other);
                }
            }
        }
    }

    /// Until each pane's turn and the sessions, as told, are what `pick`
    /// says yes to.
    fn agents_until(
        &mut self,
        pick: impl Fn(&[Option<Turn>], &[weft_core::sessions::Row]) -> bool,
    ) {
        self.stream.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
        while !pick(&self.turns, &self.rows) {
            match self.lines.next() {
                Ok(Some(line)) => {
                    self.take(&line);
                }
                other => panic!("the agents never said so: {other:?}"),
            }
        }
    }
}

fn serve(socket: &Path, config: &Path) {
    let (listening, config) = (socket.to_path_buf(), config.to_path_buf());
    let mut harness = weft::harness_table::fixture::file("codex");
    harness["program"] = "cat".into();
    let harnesses = weft::harness_table::Harnesses(vec![
        weft::harness_table::Harness::of("codex", &harness).expect("a harness"),
    ]);
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(&listening, &config, Some(harnesses));
    });
}

fn receipt(config: &Path, root: &Path, session: &str) -> PathBuf {
    weft::receipts::project_dir(&weft::receipts::base_of(config), root)
        .join("codex")
        .join(format!("{session}.jsonl"))
}

fn state(event: &str, session: &str, root: &Path) -> Sent {
    Sent {
        harness: "codex".into(),
        hook: "Hook".into(),
        session: session.into(),
        workspace: Some(root.to_string_lossy().into_owned()),
        state: Some(event.into()),
        ..Sent::default()
    }
}

#[test]
fn the_socket_is_the_users_alone() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let socket = protocol::private_socket("weft-private");
    serve(&socket, &home.path().join("config.toml"));
    let root = home.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let mut c = Client::attach(&socket, &root);
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&socket), 0o600, "the socket");
    assert_eq!(mode(socket.parent().unwrap()), 0o700, "its folder");
    c.send(Call::Shutdown);
}

#[test]
fn a_signal_moves_only_the_pane_whose_token_it_carries_and_outlives_the_daemon() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    let root = home.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let socket = protocol::private_socket("weft-signals");
    let agents = support::agents_dir(&socket);
    serve(&socket, &config);
    let mut c = Client::attach(&socket, &root);
    let spec = support::agent(&agents);
    c.answer(Call::StartAgent { harness: "codex".into(), spec, session: None });

    // The harness is told its pane, its token and the daemon's socket.
    let env = support::pane_env(&agents, 0);
    let var = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap();
    assert_eq!(var("WEFT_PANE"), "0");
    assert_eq!(var("WEFT_PANE_TOKEN").len(), 32, "128 bits, as hex");
    assert_eq!(var("WEFT_SOCKET"), socket.to_string_lossy());

    // A wrong token, or a pane that is not there, records nothing.
    let mut forged = state("waiting", "s1", &root);
    forged.pane = Some(0);
    forged.token = Some("0".repeat(32));
    support::signal(&socket, forged.clone());
    forged.pane = Some(9);
    forged.token = Some(var("WEFT_PANE_TOKEN"));
    support::signal(&socket, forged);

    // The pane's own signal: its session is bound, and its state is the pane's.
    support::pane_says(&socket, &agents, &root, "codex", 0, "s1", "working");
    c.agents_until(|panes, _| panes.first() == Some(&Some(Turn::Working)));
    let map: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".fab7/weft/panes.json")).unwrap())
            .unwrap();
    assert_eq!(
        map["panes"][0]["session"],
        serde_json::json!({"session": "s1", "how": "signalled"})
    );
    let lines = std::fs::read_to_string(receipt(&config, &root, "s1")).unwrap();
    assert_eq!(lines.lines().count(), 1, "the forged events left no line: {lines}");

    // A session no pane holds is recorded and moves no pane; one a pane holds
    // is moved only by that pane.
    support::signal(&socket, state("waiting", "t9", &root));
    c.agents_until(|_, sessions| {
        sessions.iter().any(|s| s.id == "t9" && s.turn == Some(Turn::Waiting))
    });
    support::signal(&socket, state("turn_ended", "s1", &root));
    support::pane_says(&socket, &agents, &root, "codex", 0, "s1", "waiting");
    c.agents_until(|panes, _| panes.first() == Some(&Some(Turn::Waiting)));
    let lines = std::fs::read_to_string(receipt(&config, &root, "s1")).unwrap();
    assert!(!lines.contains("turn_ended"), "{lines}");

    // A session another harness runs inside the pane (it inherits the token)
    // is that harness's, and takes the pane from no one.
    let mut nested = state("turn_ended", "n1", &root);
    nested.harness = "claude-code".into();
    nested.pane = Some(0);
    nested.token = Some(var("WEFT_PANE_TOKEN"));
    support::signal(&socket, nested);
    c.agents_until(|_, sessions| sessions.iter().any(|s| s.id == "n1"));
    let map: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".fab7/weft/panes.json")).unwrap())
            .unwrap();
    assert_eq!(map["panes"][0]["session"]["session"], "s1", "still the pane's own session");

    // A session or harness that is not only an id names no file anywhere.
    let mut escape = state("waiting", "../../escape", &root);
    escape.harness = "../codex".into();
    support::signal(&socket, escape);
    // A folder under one that holds `.fab7/` but no record is its own project.
    let outer = home.path().join("outer");
    let inner = outer.join("inner");
    std::fs::create_dir_all(outer.join(".fab7/weft")).unwrap();
    std::fs::create_dir_all(&inner).unwrap();
    let inner = inner.canonicalize().unwrap();
    support::signal(&socket, state("working", "i1", &inner));
    let mine = receipt(&config, &inner, "i1");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !mine.exists() {
        assert!(Instant::now() < deadline, "the inner project's receipt never came");
        std::thread::sleep(Duration::from_millis(20));
    }
    let base = weft::receipts::base_of(&config);
    let walked = std::process::Command::new("find").arg(&base).output().unwrap();
    let walked = String::from_utf8_lossy(&walked.stdout);
    assert!(!walked.contains("escape"), "{walked}");
    assert!(!home.path().join("escape").exists() && !base.join("escape").exists());
    c.send(Call::Shutdown);

    // Started again, the daemon reads every session's state from its receipts.
    let deadline = Instant::now() + Duration::from_secs(5);
    while protocol::is_live(&socket) {
        assert!(Instant::now() < deadline, "the daemon never stopped");
        std::thread::sleep(Duration::from_millis(20));
    }
    serve(&socket, &config);
    let mut again = Client::attach(&socket, &root);
    again.agents_until(|_, sessions| {
        let of = |id: &str| sessions.iter().find(|s| s.id == id).and_then(|s| s.turn);
        of("s1") == Some(Turn::Waiting) && of("t9") == Some(Turn::Waiting)
    });
    again.send(Call::Shutdown);
}

/// A turn that ends with a reply asking its person is NEEDS YOU, by Sense's
/// turn-end head, while Weft may still type into it; one that only reports
/// stays a turn that ended. Needs a bundle with that head: `SENSE_D_BUNDLE`.
#[test]
fn a_turn_that_ends_asking_needs_you_and_one_that_reports_does_not() {
    let Some(bundle) = std::env::var_os("SENSE_D_BUNDLE") else {
        eprintln!("skipped: SENSE_D_BUNDLE names no bundle with a turn_end head");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    std::fs::create_dir_all(home.path().join("sense")).unwrap();
    let current = serde_json::json!({"version": "0.2.0", "path": bundle.to_string_lossy()});
    std::fs::write(home.path().join("sense/current.json"), current.to_string()).unwrap();
    let root = home.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let socket = protocol::private_socket("weft-asked");
    let agents = support::agents_dir(&socket);
    serve(&socket, &config);
    let mut c = Client::attach(&socket, &root);
    let spec = support::agent(&agents);
    c.answer(Call::StartAgent { harness: "codex".into(), spec, session: None });
    let ended = |said: &str| Sent {
        said: Some(said.into()),
        pane: Some(0),
        token: Some(support::token_of(&agents, 0)),
        ..state("turn_ended", "s1", &root)
    };
    let asked = |sessions: &[weft_core::sessions::Row]| {
        sessions.iter().find(|s| s.id == "s1").map(|s| s.asked)
    };
    // Sense loads in the background: the turn is asked about until it answers.
    let asking = "Should I also update the docs? Please confirm before I continue.";
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        support::pane_says(&socket, &agents, &root, "codex", 0, "s1", "working");
        support::signal(&socket, ended(asking));
        let lines = std::fs::read_to_string(receipt(&config, &root, "s1")).unwrap_or_default();
        if lines.contains("\"asked\":true") {
            break;
        }
        assert!(Instant::now() < until, "Sense never read the reply: {lines}");
        std::thread::sleep(Duration::from_millis(300));
    }
    c.agents_until(|panes, sessions| {
        panes.first() == Some(&Some(Turn::TurnEnded)) && asked(sessions) == Some(true)
    });
    assert!(weft_core::turns::may_type(Some(Turn::TurnEnded)).is_ok(), "Weft may still type");
    // The person types an answer into the pane: it no longer needs them.
    c.send(Call::Input { pane: 0, bytes: b"y".to_vec() });
    c.agents_until(|panes, sessions| {
        panes.first() == Some(&Some(Turn::TurnEnded)) && asked(sessions) == Some(false)
    });
    // Asked again; then the next turn starts, then one that only reports.
    support::signal(&socket, ended(asking));
    c.agents_until(|_, sessions| asked(sessions) == Some(true));
    support::pane_says(&socket, &agents, &root, "codex", 0, "s1", "working");
    c.agents_until(|_, sessions| asked(sessions) == Some(false));
    support::signal(&socket, ended("Done. The tests pass and the change is committed."));
    c.agents_until(|panes, sessions| {
        panes.first() == Some(&Some(Turn::TurnEnded)) && asked(sessions) == Some(false)
    });
    c.send(Call::Shutdown);
}
