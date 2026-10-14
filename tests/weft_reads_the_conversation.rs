//! weftd reads an agent's conversation from its harness's own transcript,
//! whose path a hook of that pane named, only when asked, and keeps nothing:
//! a path outside the harness's configuration home or another session's
//! file reads as unreadable, and a read writes nothing under Weft's own
//! folders.

mod support;

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;
use weft::turns::Turn;
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
        let lines = Lines::new(stream.try_clone().expect("clone"));
        let mut c = Client { next: 1, stream, lines };
        c.answer(Call::Hello { client: "test".into(), protocol: PROTOCOL });
        let path = root.to_string_lossy().into_owned();
        c.answer(Call::Open { path, rows: 24, cols: 80, raw: false });
        c
    }

    fn send(&mut self, call: Call) -> u64 {
        let id = self.next;
        self.next += 1;
        protocol::send(&mut self.stream, &Line::Call { id, call }.encode()).expect("send");
        id
    }

    fn answer(&mut self, call: Call) -> serde_json::Value {
        self.reply(call).unwrap_or_else(|message| panic!("{message}"))
    }

    fn reply(&mut self, call: Call) -> Result<serde_json::Value, String> {
        let id = self.send(call);
        loop {
            match self.lines.next().expect("read").expect("a line") {
                Line::Result { id: got, result } if got == id => return Ok(result),
                Line::Error { id: got, message, .. } if got == id => return Err(message),
                _ => {}
            }
        }
    }

    fn working(&mut self) {
        self.stream.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
        loop {
            match self.lines.next() {
                Ok(Some(Line::Event(Event::Agents { panes, .. })))
                    if panes.first() == Some(&Some(Turn::Working)) =>
                {
                    return;
                }
                Ok(Some(_)) => {}
                other => panic!("the pane never worked: {other:?}"),
            }
        }
    }

    fn conversation(&mut self, before: Option<&str>, after: Option<&str>) -> serde_json::Value {
        let answer = self.answer(Call::Read {
            what: "conversation".into(),
            unit: String::new(),
            pane: Some(0),
            lines: None,
            before: before.map(str::to_string),
            after: after.map(str::to_string),
        });
        answer["conversation"].clone()
    }
}

/// Every file under a folder.
fn files(dir: &Path) -> Vec<String> {
    let out = std::process::Command::new("find").arg(dir).arg("-type").arg("f").output().unwrap();
    let mut all: Vec<String> =
        String::from_utf8_lossy(&out.stdout).lines().map(String::from).collect();
    all.sort();
    all
}

#[test]
fn a_pane_reads_the_transcript_its_hook_named_and_nothing_else() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    let root = home.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    // The harness's configuration home, where it keeps its transcripts.
    let grok_home = home.path().join("grok-home");
    let transcript = grok_home.join("sessions/%2Fproject/g1/updates.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    let fixture = include_str!("../crates/weft-core/tests/fixtures/conversation/grok.jsonl");
    let lines: Vec<&str> = fixture.lines().collect();
    std::fs::write(&transcript, format!("{}\n", lines[..10].join("\n"))).unwrap();

    let socket = protocol::private_socket("weft-conversation");
    let agents = support::agents_dir(&socket);
    let mut harness = weft::harness_table::fixture::file("grok");
    harness["program"] = "cat".into();
    harness["config"]["env"] = serde_json::Value::Null;
    harness["config"]["default"] = grok_home.to_string_lossy().into_owned().into();
    let harnesses = weft::harness_table::Harnesses(vec![
        weft::harness_table::Harness::of("grok", &harness).expect("a harness"),
    ]);
    let (listening, at) = (socket.clone(), config.clone());
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(&listening, &at, Some(harnesses));
    });
    let mut c = Client::attach(&socket, &root);
    c.answer(Call::StartAgent {
        harness: "grok".into(),
        spec: support::agent(&agents),
        session: None,
    });

    // No hook yet, and no session: nothing to read.
    assert_eq!(c.conversation(None, None)["readable"], false);

    let token = support::token_of(&agents, 0);
    let says = |transcript: &Path| Sent {
        harness: "grok".into(),
        hook: "SessionStart".into(),
        session: "g1".into(),
        workspace: Some(root.to_string_lossy().into_owned()),
        state: Some("working".into()),
        pane: Some(0),
        token: Some(token.clone()),
        transcript: Some(transcript.to_string_lossy().into_owned()),
        ..Sent::default()
    };
    support::signal(&socket, says(&transcript));
    c.working();
    let weft_files = files(home.path());

    let page = c.conversation(None, None);
    assert_eq!(page["readable"], true, "{page}");
    assert_eq!(page["before"], serde_json::Value::Null, "the file's start");
    let texts: Vec<&str> =
        page["items"].as_array().unwrap().iter().filter_map(|i| i["text"].as_str()).collect();
    assert_eq!(texts[..2], ["Reply with the single word ok.", "ok"]);
    let after = page["after"].as_str().unwrap().to_string();

    // The file grows: only what was added comes.
    assert_eq!(c.conversation(None, Some(&after))["items"], serde_json::json!([]));
    std::fs::write(&transcript, fixture).unwrap();
    let grown = c.conversation(None, Some(&after));
    let kinds: Vec<&str> =
        grown["items"].as_array().unwrap().iter().map(|i| i["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"step") && kinds.contains(&"outcome") && !kinds.is_empty(), "{grown}");
    assert!(!grown.to_string().contains("probe-marker\\n"), "no tool output");
    let forged = c.reply(Call::Read {
        what: "conversation".into(),
        unit: String::new(),
        pane: Some(0),
        lines: None,
        before: Some("not a cursor".into()),
        after: None,
    });
    assert!(forged.is_err(), "a cursor weftd never gave is refused");

    // A read wrote nothing.
    assert_eq!(files(home.path()), weft_files);

    // A hook that names a file outside the home, or another session's.
    let outside = home.path().join("elsewhere/g1/updates.jsonl");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::write(&outside, fixture).unwrap();
    support::signal(&socket, says(&outside));
    let deadline = Instant::now() + Duration::from_secs(5);
    while c.conversation(None, None)["readable"] != false {
        assert!(Instant::now() < deadline, "a file outside the home was read");
        std::thread::sleep(Duration::from_millis(20));
    }
    c.send(Call::Shutdown);
}
