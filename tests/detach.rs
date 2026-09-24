//! Gate W2's open criterion: a client can leave and come back, and the agents
//! keep working while it is gone.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use weft::protocol;
use weft::protocol::{Call, Event, Line, Lines, PROTOCOL};
use weft::server;

struct Client {
    next: u64,
    /// What `project.open` answered: this project's panes and board.
    opened: serde_json::Value,
    stream: UnixStream,
    frames: Lines<UnixStream>,
}

impl Client {
    fn attach(socket: &PathBuf, project: &Path) -> Self {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            if let Ok(s) = UnixStream::connect(socket) {
                break s;
            }
            assert!(Instant::now() < deadline, "the server never came up");
            std::thread::sleep(Duration::from_millis(20));
        };
        let frames = Lines::new(stream.try_clone().expect("clone"));
        let mut c = Client { stream, frames, next: 1, opened: serde_json::json!({}) };
        c.answer(Call::Hello { client: "test".into(), protocol: PROTOCOL }).expect("hello");
        c.opened = c
            .answer(Call::Open { rows: 24, cols: 80, path: project.to_string_lossy().into_owned() })
            .expect("project.open");
        c
    }

    fn send(&mut self, call: Call) {
        self.call(call);
    }

    fn call(&mut self, call: Call) {
        let id = self.next;
        self.next += 1;
        self.stream.write_all(&Line::Call { id, call }.encode()).expect("write");
        self.stream.flush().expect("flush");
    }

    /// Make a call and wait for its answer: the result, or the error code.
    fn answer(&mut self, call: Call) -> Result<serde_json::Value, String> {
        let id = self.next;
        self.call(call);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(100))).expect("timeout");
            match self.frames.next() {
                Ok(Some(Line::Result { id: got, result })) if got == id => return Ok(result),
                Ok(Some(Line::Error { id: got, code, .. })) if got == id => return Err(code),
                Ok(None) => break,
                _ => continue,
            }
        }
        panic!("no answer to call {id}");
    }

    /// The next event, or nothing within the budget.
    fn next_event(&mut self, within: Duration) -> Option<Event> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(100))).expect("timeout");
            match self.frames.next() {
                Ok(Some(Line::Event(e))) => return Some(e),
                Ok(Some(_)) => continue,
                Ok(None) => return None,
                Err(_) => continue, // a read timeout, not a failure
            }
        }
        None
    }

    /// Events until one of them is what is being waited for.
    fn wait_until<T>(&mut self, mut pick: impl FnMut(Event) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(found) = self.next_event(Duration::from_millis(250)).and_then(&mut pick) {
                return found;
            }
        }
        panic!("nothing arrived");
    }

    /// Collect output until `needle` shows up, or give up.
    fn wait_for(&mut self, needle: &str, secs: u64) -> String {
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut seen = String::new();
        while Instant::now() < deadline {
            if let Some(Event::Output { bytes, .. }) = self.next_event(Duration::from_millis(250)) {
                seen.push_str(&String::from_utf8_lossy(&bytes));
                if seen.contains(needle) {
                    return seen;
                }
            }
        }
        seen
    }

    fn wait_for_added(&mut self) -> (u32, String) {
        self.wait_until(|e| match e {
            Event::Added { pane, harness, .. } => Some((pane, harness)),
            _ => None,
        })
    }

    fn wait_for_pending(&mut self) -> (String, String) {
        self.wait_until(|e| match e {
            Event::Pending { id, what, .. } => Some((id, what)),
            _ => None,
        })
    }

    fn wait_for_resolved(&mut self) -> (String, bool) {
        self.wait_until(|e| match e {
            Event::Resolved { id, yes } => Some((id, yes)),
            _ => None,
        })
    }

    /// What this project's panes had printed when it was opened.
    fn replay(&self) -> String {
        self.opened
            .get("replay")
            .and_then(|v| v.as_array())
            .map(|chunks| {
                chunks
                    .iter()
                    .filter_map(weft::protocol::output_of)
                    .map(|(_, bytes)| String::from_utf8_lossy(&bytes).into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The panes this project had when it was opened, from the answer.
    fn hello(&mut self) -> Vec<weft::protocol::PaneInfo> {
        weft::protocol::panes_of(self.opened.get("panes").expect("panes in the answer"))
            .expect("panes")
    }

    /// Whether nothing that would type or add a pane arrives.
    fn quiet_for(&mut self, how_long: Duration) -> bool {
        !matches!(self.next_event(how_long), Some(Event::Added { .. } | Event::Output { .. }))
    }
}

fn session(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("weft-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("root");
    let socket = protocol::private_socket("weft-detach");
    let _ = std::fs::remove_file(&socket);
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
    });
    (root, socket)
}

#[test]
fn an_agent_keeps_working_while_no_client_is_attached() {
    let (root, socket) = session("detach");

    // A client starts an agent and gives it something slow to do.
    let mut first = Client::attach(&socket, &root);
    first.hello();
    first.send(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into() });
    first.send(Call::Input { pane: 0, bytes: b"printf 'before-the-client-left\\n'\n".to_vec() });
    assert!(
        first.wait_for("before-the-client-left", 5).contains("before-the-client-left"),
        "the first client sees its own output"
    );

    // It leaves. Nothing is stopped.
    first.send(Call::Detach);
    drop(first);
    std::thread::sleep(Duration::from_millis(400));

    // A new client arrives and is shown what happened while it was away.
    let mut second = Client::attach(&socket, &root);
    let panes = second.hello();
    assert_eq!(panes.len(), 1, "the pane outlived the client");
    assert!(panes[0].running, "and its process is still alive");

    let replayed = second.replay();
    assert!(
        replayed.contains("before-the-client-left"),
        "a client that arrives late is shown what it missed: {replayed:?}"
    );

    // And it is a live pane, not a recording.
    second
        .send(Call::Input { pane: 0, bytes: b"printf 'after-the-client-returned\\n'\n".to_vec() });
    assert!(
        second.wait_for("after-the-client-returned", 5).contains("after-the-client-returned"),
        "the agent still takes input from the new client"
    );

    second.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn two_clients_see_the_same_session_at_once() {
    let (root, socket) = session("two");

    let mut a = Client::attach(&socket, &root);
    a.hello();
    a.send(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into() });
    std::thread::sleep(Duration::from_millis(300));

    let mut b = Client::attach(&socket, &root);
    assert_eq!(b.hello().len(), 1, "the second client sees the first's pane");

    // Input from one is seen by both.
    a.send(Call::Input { pane: 0, bytes: b"printf 'shared-output\\n'\n".to_vec() });
    assert!(a.wait_for("shared-output", 5).contains("shared-output"));
    assert!(
        b.wait_for("shared-output", 5).contains("shared-output"),
        "both clients are views of one session"
    );

    a.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_socket_left_behind_by_a_dead_server_does_not_block_a_new_one() {
    let root = std::env::temp_dir().join(format!("weft-stale-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("root");
    let socket = protocol::private_socket("weft-detach");
    std::fs::create_dir_all(socket.parent().unwrap()).expect("dir");
    std::fs::write(&socket, b"not a server").expect("stale file");

    assert!(!protocol::is_live(&socket), "a leftover file is not a session");
    protocol::clear_dead(&socket);

    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
    });
    let mut c = Client::attach(&socket, &root);
    assert!(c.hello().is_empty(), "a fresh session starts with no panes");

    c.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// One daemon, many projects. Two clients on two projects see only their own
/// panes, each numbered from one. Nothing crosses.
#[test]
fn two_projects_share_a_daemon_and_see_none_of_each_others_panes() {
    let socket = protocol::private_socket("weft-two-projects");
    let a = project("two-a");
    let b = project("two-b");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
    });

    let mut one = Client::attach(&socket, &a);
    let mut two = Client::attach(&socket, &b);
    one.hello();
    two.hello();
    one.send(Call::StartAgent { harness: "codex".into(), spec: "/bin/cat".into() });
    two.send(Call::StartAgent { harness: "claude-code".into(), spec: "/bin/cat".into() });

    // Each is told about its own, numbered from one within its project.
    let added_a = one.wait_for_added();
    let added_b = two.wait_for_added();
    assert_eq!(added_a, (0, "codex".to_string()));
    assert_eq!(added_b, (0, "claude-code".to_string()));

    // And neither hears the other's. A second Added on either would be it.
    assert!(one.quiet_for(Duration::from_millis(400)), "project a heard project b");
    assert!(two.quiet_for(Duration::from_millis(400)), "project b heard project a");

    let mut stop = Client::attach(&socket, &a);
    stop.send(Call::Shutdown);
    std::fs::remove_dir_all(&a).ok();
    std::fs::remove_dir_all(&b).ok();
}

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("weft-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("project dir");
    dir
}

/// The pending queue is the daemon's. Two clients on one project both see a
/// staged prompt, and only the first answer lands.
#[test]
fn a_staged_prompt_reaches_every_client_and_is_answered_once() {
    let socket = protocol::private_socket("weft-pending");
    let root = project("pending");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
    });

    let mut one = Client::attach(&socket, &root);
    let mut two = Client::attach(&socket, &root);
    // Both are watching before anything happens: the daemon reads each client
    // on its own thread, so a spawn could otherwise be handled before the
    // second attach and that client would never hear about the pane.
    one.hello();
    two.hello();
    one.send(Call::StartAgent { harness: "codex".into(), spec: "/bin/cat".into() });
    one.wait_for_added();
    two.wait_for_added();

    one.send(Call::Stage {
        pane: 0,
        bytes: b"ship the endpoint".to_vec(),
        how: weft::inject::Handoff::Whole,
        what: "Ready to send to codex".into(),
        why: "This is the exact wording.".into(),
    });

    // Both are asked, with the same id.
    let (id_one, what) = one.wait_for_pending();
    let (id_two, _) = two.wait_for_pending();
    assert_eq!(id_one, id_two, "one question, not one each");
    assert_eq!(what, "Ready to send to codex");

    // The second client answers. The first is told, and its own answer finds
    // nothing left to answer.
    two.send(Call::Resolve { pending: id_two.clone(), yes: true, force: false });
    assert_eq!(one.wait_for_resolved(), (id_one.clone(), true));
    assert!(one.wait_for("ship the endpoint", 3).contains("ship the endpoint"), "typed once");

    // And the first client's own answer finds nothing left to answer, rather
    // than typing the prompt a second time.
    assert_eq!(
        one.answer(Call::Resolve { pending: id_one, yes: true, force: false }),
        Err("gone".to_string())
    );

    let mut stop = Client::attach(&socket, &root);
    stop.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// Delegation goes to a pane of the act's harness that is free, and when none
/// is, the daemon starts one there on the same command line.
#[test]
fn an_eval_goes_to_an_idle_pane_of_its_harness_or_starts_one() {
    let socket = protocol::private_socket("weft-delegate");
    let root = project("delegate");
    std::fs::create_dir_all(root.join(".fab7/rf")).expect("record");
    let compiled = serde_json::json!({
        "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.compiled",
        "time": "2026-09-19T14:02:00Z", "id": "ask_1",
        "actor": {"kind": "human", "id": "me"}, "links": [],
        "data": {"title": "Ship it", "selected_capability": "native_plan",
                 "delivery_mode": "human_handoff", "host": {"name": "sh"},
                 "source": {}, "prompt": {}, "source_verified": "exact", "limitations": [],
                 "classification": {}, "route_explanation": {}}
    });
    std::fs::write(root.join(".fab7/rf/ledger.jsonl"), format!("{compiled}\n")).expect("ledger");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
    });

    let mut c = Client::attach(&socket, &root);
    c.hello();
    c.send(Call::StartAgent { harness: "sh".into(), spec: "/bin/cat".into() });
    assert_eq!(c.wait_for_added(), (0, "sh".to_string()));
    let check =
        || Call::Act { act: "check".into(), unit: Some("ask_1".into()), pane: None, text: None };
    let pending_pane = |c: &mut Client| {
        c.wait_until(|e| match e {
            Event::Pending { pane, why, .. } => Some((pane, why)),
            _ => None,
        })
    };

    // Idle: it goes to the pane that is there.
    // Sent rather than answered: the Pending event comes before the answer.
    c.send(check());
    let (pane, why) = pending_pane(&mut c);
    assert_eq!(pane, 0);
    assert!(!why.contains("started one"), "{why}");

    // Busy, as a harness at work draws it: a new pane on the same command line.
    c.send(Call::Input { pane: 0, bytes: b"working... esc to interrupt\n".to_vec() });
    c.wait_for("esc to interrupt", 5);
    c.send(check());
    assert_eq!(c.wait_for_added(), (1, "sh".to_string()));
    let (pane, why) = pending_pane(&mut c);
    assert_eq!(pane, 1);
    assert!(why.contains("No sh agent here was free, so Weft started one."), "{why}");

    let mut stop = Client::attach(&socket, &root);
    let panes = stop.hello();
    assert_eq!(panes.iter().map(|p| p.spec.as_str()).collect::<Vec<_>>(), ["/bin/cat", "/bin/cat"]);
    stop.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}
