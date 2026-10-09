//! weftd keeps every session of a project (Weft ADR-0029, sessions.md "The
//! session index"): a session is listed as soon as its receipt is written,
//! held by a pane or not; it ends the moment the process its receipts name
//! does, whatever its last event; and `agent.start` resumes exactly the id it
//! is given, when the index holds it and nothing runs it — never "the
//! latest" (the `gl` case).

mod support;

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;
use weft_core::sessions::{Row, State};
use weft_core::signal::Sent;

struct Client {
    next: u64,
    stream: UnixStream,
    lines: Lines<UnixStream>,
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
        stream.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
        let lines = Lines::new(stream.try_clone().expect("clone"));
        let mut c = Client { next: 1, stream, lines };
        c.call(Call::Hello { client: "test".into(), protocol: PROTOCOL }).expect("hello");
        let path = root.to_string_lossy().into_owned();
        c.call(Call::Open { path, rows: 24, cols: 80, raw: false }).expect("open");
        c
    }

    /// The answer, or the refusal's code.
    fn call(&mut self, call: Call) -> Result<serde_json::Value, String> {
        let id = self.next;
        self.next += 1;
        protocol::send(&mut self.stream, &Line::Call { id, call }.encode()).expect("send");
        loop {
            match self.lines.next().expect("read").expect("a line") {
                Line::Result { id: got, result } if got == id => return Ok(result),
                Line::Error { id: got, code, .. } if got == id => return Err(code),
                _ => {}
            }
        }
    }

    /// The index, from the next `agents` event that `pick` says yes to.
    fn index_until(&mut self, pick: impl Fn(&[Row]) -> bool) -> Vec<Row> {
        loop {
            match self.lines.next() {
                Ok(Some(Line::Event(Event::Agents { sessions, .. }))) if pick(&sessions) => {
                    return sessions;
                }
                Ok(Some(_)) => {}
                other => panic!("the index never said so: {other:?}"),
            }
        }
    }
}

fn serve(socket: &Path, config: &Path) {
    let (listening, config) = (socket.to_path_buf(), config.to_path_buf());
    let mut harness = weft::harness_table::fixture::file("codex");
    harness["program"] = "cat".into();
    // Resumed, it reads its pane (`cat - <id>`) and keeps running.
    harness["resume"] = serde_json::json!(["-"]);
    let harnesses = weft::harness_table::Harnesses(vec![
        weft::harness_table::Harness::of("codex", &harness).expect("a harness"),
    ]);
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(&listening, &config, Some(harnesses));
    });
}

/// A hook of a session no Weft pane holds, run under process `pid`.
fn outside(root: &Path, session: &str, event: &str, pid: Option<u32>) -> Sent {
    let start = pid.and_then(weftd::process::info).map(|i| i.start);
    Sent {
        harness: "codex".into(),
        hook: "Hook".into(),
        session: session.into(),
        workspace: Some(root.to_string_lossy().into_owned()),
        state: Some(event.into()),
        pid,
        pid_start: start,
        ..Sent::default()
    }
}

fn row<'a>(index: &'a [Row], id: &str) -> Option<&'a Row> {
    index.iter().find(|r| r.id == id)
}

fn panes(root: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(root.join(".fab7/weft/panes.json")).unwrap()).unwrap()
}

#[test]
fn every_session_is_kept_and_a_resume_names_exactly_its_id() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    let root = home.path().join("gl");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let socket = protocol::private_socket("weft-index");
    serve(&socket, &config);
    let mut c = Client::attach(&socket, &root);

    // The gl case: an older session that has ended, and a newer one open in
    // another program (Zed), its harness process alive.
    let gone = std::process::Command::new("true").spawn().unwrap();
    let gone_pid = gone.id();
    let mut zed = std::process::Command::new("sleep").arg("60").spawn().unwrap();
    let start = weftd::process::info(gone_pid).map(|i| i.start);
    let mut older = outside(&root, "s_old", "turn_ended", Some(gone_pid));
    older.pid_start = start.or(Some("0.000001".into()));
    support::signal(&socket, older);
    let mut gone = gone;
    gone.wait().unwrap();
    support::signal(&socket, outside(&root, "s_new", "working", Some(zed.id())));
    let index = c.index_until(|i| row(i, "s_old").is_some() && row(i, "s_new").is_some());
    let (old, new) = (row(&index, "s_old").unwrap(), row(&index, "s_new").unwrap());
    assert_eq!((new.state, new.pane), (State::Running, None), "listed with no pane");
    assert_eq!(old.state, State::Ended, "its process is gone");
    assert_eq!(index.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["s_new", "s_old"]);

    // Nothing resumes what the index does not hold, or what runs elsewhere.
    let resume = |c: &mut Client, id: &str| {
        c.call(Call::StartAgent {
            harness: "codex".into(),
            spec: "cat --last".into(),
            session: Some(id.into()),
        })
    };
    assert_eq!(resume(&mut c, "s_nope"), Err("no_session".into()));
    assert_eq!(resume(&mut c, "--last"), Err("no_session".into()), "flag-shaped");
    assert_eq!(resume(&mut c, "s_new"), Err("running".into()), "open in another program");

    // The older one is resumed by its own id, with the harness's resume line.
    let pane = resume(&mut c, "s_old").expect("resumed")["pane"].as_u64().unwrap();
    let map = panes(&root);
    let p = &map["panes"][pane as usize];
    assert_eq!(p["session"], serde_json::json!({"session": "s_old", "how": "resumed"}));
    assert_eq!(p["spec"], "cat - s_old", "the harness file's line, not the client's");
    assert!(!map.to_string().contains("s_new"), "the newer session is never chosen");
    // Its row runs again, in the new pane (T11.5: the row turns ●).
    let index = c.index_until(|i| row(i, "s_old").is_some_and(|r| r.pane == Some(pane as u32)));
    assert_eq!(row(&index, "s_old").unwrap().state, State::Running);

    // The newer session ends the moment its process does, whatever its last
    // event said.
    zed.kill().unwrap();
    let index = c.index_until(|i| row(i, "s_new").is_some_and(|r| r.state == State::Ended));
    assert_eq!(row(&index, "s_new").unwrap().turn, Some(weft_core::turns::Turn::Working));
    zed.wait().unwrap();
    let _ = c.call(Call::Shutdown);
}

#[test]
fn a_session_held_by_a_running_pane_is_refused_with_the_pane() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    let root = home.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let socket = protocol::private_socket("weft-held");
    let agents = support::agents_dir(&socket);
    serve(&socket, &config);
    let mut c = Client::attach(&socket, &root);
    let spec = support::agent(&agents);
    c.call(Call::StartAgent { harness: "codex".into(), spec, session: None }).unwrap();
    support::pane_says(&socket, &agents, &root, "codex", 0, "s_here", "turn_ended");
    let index = c.index_until(|i| row(i, "s_here").is_some_and(|r| r.pane == Some(0)));
    assert_eq!(row(&index, "s_here").unwrap().state, State::Running);
    let again = Call::StartAgent {
        harness: "codex".into(),
        spec: "cat".into(),
        session: Some("s_here".into()),
    };
    assert_eq!(c.call(again), Err("running".into()));
    let _ = c.call(Call::Shutdown);
}
