//! A client can leave and come back, and the agents keep working while it is
//! gone.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use weft::protocol;
use weft::protocol::{Call, Event, Line, Lines, PROTOCOL};
use weft::server;
use weft::turns::Turn;

mod support;

/// An agent program that behaves as `cat` and tells the test its pane.
fn agent(socket: &Path) -> String {
    support::agent(&support::agents_dir(socket))
}

/// What the agent in `pane` would signal, as its hook runs `weft signal`.
fn agent_says(socket: &Path, root: &Path, harness: &str, pane: u32, session: &str, event: &str) {
    support::pane_says(socket, &support::agents_dir(socket), root, harness, pane, session, event);
}

struct Client {
    next: u64,
    /// What `project.open` answered: this project's panes and board.
    opened: serde_json::Value,
    stream: UnixStream,
    frames: Lines<UnixStream>,
}

impl Client {
    fn attach(socket: &PathBuf, project: &Path) -> Self {
        Self::attach_at(socket, project, 24, 80)
    }

    /// Attach from a window of this size.
    fn attach_at(socket: &PathBuf, project: &Path, rows: u16, cols: u16) -> Self {
        Self::attach_as(socket, project, rows, cols, "test")
    }

    /// Attach as a client that calls itself `name` in its hello.
    fn attach_as(socket: &PathBuf, project: &Path, rows: u16, cols: u16, name: &str) -> Self {
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
        c.answer(Call::Hello { client: name.into(), protocol: PROTOCOL }).expect("hello");
        c.opened = c
            .answer(Call::Open {
                rows,
                cols,
                path: project.to_string_lossy().into_owned(),
                raw: true,
            })
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
            // A connection the daemon has shut refuses a timeout on some
            // systems; its lines are still read to their end.
            let _ = self.stream.set_read_timeout(Some(Duration::from_millis(100)));
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

    /// What this project's panes showed when it was opened, history and
    /// screen, drawn from the screens the answer gave.
    fn replay(&self) -> String {
        self.opened
            .get("screens")
            .and_then(|v| v.as_array())
            .map(|screens| {
                screens
                    .iter()
                    .filter_map(weft::protocol::screen_of)
                    .map(|s| {
                        let mut term = weft_term::Term::new(s.rows, s.cols, 10_000, |_| {});
                        term.feed(&s.bytes);
                        term.history(2000)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }

    /// The panes this project had when it was opened, from the answer.
    fn hello(&mut self) -> Vec<weft::protocol::PaneInfo> {
        weft::protocol::panes_of(self.opened.get("panes").expect("panes in the answer"))
            .expect("panes")
    }

    /// The first pane's state, once the daemon reports it as this.
    fn wait_for_state(&mut self, want: Option<Turn>) {
        self.wait_until(|e| match e {
            Event::Agents { panes, .. } if panes.first().copied().flatten() == want => Some(()),
            _ => None,
        })
    }

    fn wait_for_refusal(&mut self) -> Option<String> {
        self.wait_until(|e| match e {
            Event::Injected { refusal, .. } => Some(refusal),
            _ => None,
        })
    }

    /// The next `injected` within `within`, if one comes.
    fn injected_within(&mut self, within: Duration) -> Option<Option<String>> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if let Some(Event::Injected { refusal, .. }) =
                self.next_event(Duration::from_millis(250))
            {
                return Some(refusal);
            }
        }
        None
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
    first.send(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into(), session: None });
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
fn a_project_closes_when_its_last_client_leaves_and_no_agent_runs() {
    let root = std::env::temp_dir().join(format!("weft-closes-{}", std::process::id()));
    let (busy, idle) = (root.join("busy"), root.join("idle"));
    for p in [&busy, &idle] {
        std::fs::create_dir_all(p).expect("project");
    }
    // What an earlier daemon left listed is not open in this one.
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(
        root.join("projects.json"),
        r#"{"projects":[{"path":"/gone/before","opened":1}]}"#,
    )
    .expect("left over");
    let socket = protocol::private_socket("weft-detach");
    let listening = socket.clone();
    let config = root.join("config.toml");
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &config);
    });
    let listed = || {
        let text = std::fs::read_to_string(root.join("projects.json")).unwrap_or_default();
        (text.contains("/busy\""), text.contains("/idle\""))
    };

    // A window in each project; an agent runs in one.
    let mut a = Client::attach(&socket, &busy);
    let left_over = std::fs::read_to_string(root.join("projects.json")).unwrap_or_default();
    assert!(
        !left_over.contains("/gone/before"),
        "a new daemon starts with none listed: {left_over}"
    );
    a.send(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into(), session: None });
    std::thread::sleep(Duration::from_millis(300));
    let mut b = Client::attach(&socket, &idle);
    // `weft remote` opens every listed project; it keeps none open.
    let mut remote = Client::attach_as(&socket, &idle, 24, 80, "weft-remote/0.2.0");
    let _remote_busy = Client::attach_as(&socket, &busy, 24, 80, "weft-remote/0.2.0");
    assert_eq!(listed(), (true, true));

    // Both windows leave.
    a.send(Call::Detach);
    drop(a);
    b.send(Call::Detach);
    drop(b);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        listed(),
        (true, false),
        "the project with an agent running stays; the other closes"
    );
    // Already shut, on some systems setting a timeout on it is refused.
    let _ = remote.stream.set_read_timeout(Some(Duration::from_secs(5)));
    let ended = loop {
        match remote.frames.next() {
            Ok(Some(_)) => continue,
            Ok(None) => break true,
            Err(_) => break false,
        }
    };
    assert!(ended, "weft remote's connection to the closed project ends");

    // The next window to open it opens it again.
    let mut c = Client::attach(&socket, &idle);
    assert!(c.hello().is_empty());
    assert_eq!(listed(), (true, true));
    c.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_window_that_stops_its_agents_stops_only_its_own() {
    let root = std::env::temp_dir().join(format!("weft-stops-own-{}", std::process::id()));
    let (mine, theirs) = (root.join("mine"), root.join("theirs"));
    for p in [&mine, &theirs] {
        std::fs::create_dir_all(p).expect("project");
    }
    let socket = protocol::private_socket("weft-detach");
    let listening = socket.clone();
    let config = root.join("config.toml");
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &config);
    });
    let mut a = Client::attach(&socket, &mine);
    a.send(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into(), session: None });
    let mut b = Client::attach(&socket, &theirs);
    b.send(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into(), session: None });
    std::thread::sleep(Duration::from_millis(300));

    // Quit and stop the agents, as the TUI does: its own panes, then leave.
    a.send(Call::CloseAgent { pane: 0 });
    a.send(Call::Detach);
    drop(a);
    std::thread::sleep(Duration::from_millis(400));

    let mut again = Client::attach(&socket, &theirs);
    let panes = again.hello();
    assert_eq!(panes.len(), 1, "the other project's agent is still there");
    assert!(panes[0].running, "and still running");
    let listed = std::fs::read_to_string(root.join("projects.json")).unwrap_or_default();
    assert!(!listed.contains("/mine\""), "the stopped project closed: {listed}");
    drop(b);
    again.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn two_clients_see_the_same_session_at_once() {
    let (root, socket) = session("two");

    let mut a = Client::attach(&socket, &root);
    a.hello();
    a.send(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into(), session: None });
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
    one.send(Call::StartAgent { harness: "codex".into(), spec: "/bin/cat".into(), session: None });
    two.send(Call::StartAgent {
        harness: "claude-code".into(),
        spec: "/bin/cat".into(),
        session: None,
    });

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
    one.send(Call::StartAgent { harness: "codex".into(), spec: agent(&socket), session: None });
    one.wait_for_added();
    two.wait_for_added();
    agent_says(&socket, &root, "codex", 0, "s1", "ready");
    one.wait_for_state(Some(Turn::Ready));
    two.wait_for_state(Some(Turn::Ready));

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
    two.send(Call::Resolve {
        pending: id_two.clone(),
        yes: true,
        force: false,
        to: None,
        start: None,
    });
    assert_eq!(one.wait_for_resolved(), (id_one.clone(), true));
    assert!(one.wait_for("ship the endpoint", 3).contains("ship the endpoint"), "typed once");

    // And the first client's own answer finds nothing left to answer, rather
    // than typing the prompt a second time.
    assert_eq!(
        one.answer(Call::Resolve {
            pending: id_one,
            yes: true,
            force: false,
            to: None,
            start: None
        }),
        Err("gone".to_string())
    );

    let mut stop = Client::attach(&socket, &root);
    stop.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// Delegation goes to an agent of the act's harness that is free — its latest
/// event `ready` or `turn_ended` (turn-state.md §4) — and when none is, the
/// daemon starts one of that harness.
#[test]
fn an_eval_goes_to_a_free_agent_of_its_harness_or_starts_one() {
    let socket = protocol::private_socket("weft-delegate");
    let root = project("delegate");
    ask_on_record(&root, "sh");
    gathered_on_record(&root);
    let listening = socket.clone();
    let program = agent(&socket);
    let harnesses = weft::harness_table::Harnesses(vec![harness_running("sh", &program)]);
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(
            &listening,
            &listening.with_extension("no-config.toml"),
            Some(harnesses),
        );
    });

    let mut c = Client::attach(&socket, &root);
    c.hello();
    c.send(Call::StartAgent { harness: "sh".into(), spec: program.clone(), session: None });
    assert_eq!(c.wait_for_added(), (0, "sh".to_string()));
    agent_says(&socket, &root, "sh", 0, "s1", "ready");
    c.wait_for_state(Some(Turn::Ready));
    let check = || Call::Act {
        act: "check".into(),
        unit: Some("ask_1".into()),
        pane: None,
        text: None,
        eval: None,
        start: None,
    };
    let pending_pane = |c: &mut Client| {
        c.wait_until(|e| match e {
            Event::Pending { pane, why, .. } => Some((pane, why)),
            _ => None,
        })
    };

    // Free: it goes to the agent that is there.
    // Sent rather than answered: the Pending event comes before the answer.
    c.send(check());
    let (pane, why) = pending_pane(&mut c);
    assert_eq!(pane, 0);
    assert!(!why.contains("started one"), "{why}");

    // Working, as its hook reports it: a new agent of that harness.
    agent_says(&socket, &root, "sh", 0, "s1", "working");
    c.wait_for_state(Some(Turn::Working));
    c.send(check());
    assert_eq!(c.wait_for_added(), (1, "sh".to_string()));
    let (pane, why) = pending_pane(&mut c);
    assert_eq!(pane, 1);
    assert!(why.contains("No sh agent here was free, so Weft started one."), "{why}");

    let mut stop = Client::attach(&socket, &root);
    let panes = stop.hello();
    assert_eq!(panes.iter().map(|p| p.spec.as_str()).collect::<Vec<_>>(), [&program, &program]);
    stop.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// A harness of this name that runs `program`, from a fixture profile.
fn harness_running(name: &str, command: &str) -> weft::harness_table::Harness {
    let mut p = weft::harness_table::fixture::file("codex");
    let mut words = command.split_whitespace();
    p["program"] = words.next().unwrap_or_default().into();
    p["args"] = words.collect::<Vec<_>>().into();
    weft::harness_table::Harness::of(name, &p).expect("a harness")
}

/// One Ask on this project's ledger, asked of `host`.
fn ask_on_record(root: &Path, host: &str) {
    std::fs::create_dir_all(root.join(".fab7/rf")).expect("record");
    let compiled = serde_json::json!({
        "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.compiled",
        "time": "2026-09-19T14:02:00Z", "id": "ask_1",
        "actor": {"kind": "human", "id": "me"}, "links": [],
        "data": {"title": "Ship it", "selected_capability": "native_plan",
                 "delivery_mode": "human_handoff", "host": {"name": host},
                 "source": {}, "prompt": {}, "source_verified": "exact", "limitations": [],
                 "classification": {}, "route_explanation": {}}
    });
    std::fs::write(root.join(".fab7/rf/ledger.jsonl"), format!("{compiled}\n")).expect("ledger");
}

/// An Eval of that Ask, opened and gathered: what `ringframe eval open` leaves.
/// Weft debates it without gathering again, so these tests need no CLI.
fn gathered_on_record(root: &Path) {
    let ev = |kind: &str, n: u32, data: serde_json::Value| {
        serde_json::json!({"schema": "ringframe.ledger/1", "event_id": format!("evt_g{n}"),
            "type": kind, "time": "2026-09-19T14:03:00Z", "id": "evl_1",
            "actor": {"kind": "human", "id": "me"}, "links": [], "data": data})
    };
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(root.join(".fab7/rf/ledger.jsonl"))
        .expect("ledger");
    writeln!(f, "{}", ev("eval.opened", 1, serde_json::json!({"basis": {"asks": ["ask_1"]}})))
        .expect("opened");
    writeln!(
        f,
        "{}",
        ev("eval.gathered", 2, serde_json::json!({"eval_id": "evl_1", "host": "weft"}))
    )
    .expect("gathered");
}

/// A send into an agent Weft just started waits for it to say `ready`, and
/// the daemon does not stop while it waits: another window is answered and
/// every other pane's output keeps arriving.
#[test]
fn a_send_waiting_for_a_fresh_agent_stops_nothing_else() {
    let root = project("fresh-wait");
    ask_on_record(&root, "cat-agent");
    gathered_on_record(&root);
    let socket = protocol::private_socket("weft-fresh-wait");
    let listening = socket.clone();
    let program = agent(&socket);
    let harnesses = weft::harness_table::Harnesses(vec![harness_running("cat-agent", &program)]);
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(
            &listening,
            &listening.with_extension("no-config.toml"),
            Some(harnesses),
        );
    });

    let mut a = Client::attach(&socket, &root);
    a.send(Call::StartAgent { harness: "cat-agent".into(), spec: program, session: None });
    a.wait_for_added();
    agent_says(&socket, &root, "cat-agent", 0, "busy", "ready");
    a.wait_for_state(Some(Turn::Ready));
    agent_says(&socket, &root, "cat-agent", 0, "busy", "working");
    a.wait_for_state(Some(Turn::Working));
    // Nothing is free, so the daemon starts an agent for the Eval, which will
    // never say it is ready.
    a.send(Call::Act {
        act: "check".into(),
        unit: Some("ask_1".into()),
        pane: None,
        text: None,
        eval: None,
        start: None,
    });
    let id = a.wait_until(|e| match e {
        Event::Pending { id, .. } => Some(id),
        _ => None,
    });
    a.send(Call::Resolve { pending: id, yes: true, force: false, to: None, start: None });
    a.wait_for_resolved();

    let began = Instant::now();
    let mut b = Client::attach(&socket, &root);
    assert!(began.elapsed() < Duration::from_secs(2), "another window waited on the send");
    b.send(Call::Input { pane: 0, bytes: b"still-moving\n".to_vec() });
    assert!(b.wait_for("still-moving", 2).contains("still-moving"), "output stopped arriving");
    // And once the new agent says it is ready, the send goes.
    agent_says(&socket, &root, "cat-agent", 1, "fresh", "ready");
    assert_eq!(b.wait_for_refusal(), None, "typed once it said it was ready");

    b.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// An agent Weft starts for an Eval runs its harness's own command, never
/// another pane's: that one may be resuming someone else's session.
#[test]
fn an_agent_started_for_an_eval_resumes_nobodys_session() {
    let root = project("own-command");
    ask_on_record(&root, "cat-agent");
    gathered_on_record(&root);
    let socket = protocol::private_socket("weft-own-command");
    let listening = socket.clone();
    let program = agent(&socket);
    let harnesses = weft::harness_table::Harnesses(vec![harness_running("cat-agent", &program)]);
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(
            &listening,
            &listening.with_extension("no-config.toml"),
            Some(harnesses),
        );
    });
    let mut c = Client::attach(&socket, &root);
    // Their session is on record (run in a terminal), so it can be resumed.
    support::signal(
        &socket,
        weft_core::signal::Sent {
            harness: "cat-agent".into(),
            hook: "Hook".into(),
            session: "theirs".into(),
            workspace: Some(root.to_string_lossy().into_owned()),
            state: Some("turn_ended".into()),
            ..Default::default()
        },
    );
    c.wait_until(|e| match e {
        Event::Agents { sessions, .. } if sessions.iter().any(|s| s.id == "theirs") => Some(()),
        _ => None,
    });
    let theirs = harness_running("cat-agent", &program).resume_spec("theirs").expect("resume");
    let resumed = Call::StartAgent {
        harness: "cat-agent".into(),
        spec: format!("{program} -"),
        session: Some("theirs".into()),
    };
    c.send(resumed);
    c.wait_for_added();
    agent_says(&socket, &root, "cat-agent", 0, "theirs", "working");
    c.wait_for_state(Some(Turn::Working));
    c.send(Call::Act {
        act: "check".into(),
        unit: Some("ask_1".into()),
        pane: None,
        text: None,
        eval: None,
        start: None,
    });
    c.wait_for_added();

    let mut look = Client::attach(&socket, &root);
    let specs: Vec<String> = look.hello().into_iter().map(|p| p.spec).collect();
    assert_eq!(specs, [theirs, program], "the harness's own command");
    look.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// Weft's own pane map: which pane runs which session, decided once, kept by
/// the daemon, and written to `.fab7/weft/panes.json` with how it was bound.
/// A client leaving changes none of it.
#[test]
fn the_pane_map_is_the_daemons_and_outlives_a_client() {
    let (root, socket) = session("map");
    let mut a = Client::attach(&socket, &root);
    let program = agent(&socket);
    a.send(Call::StartAgent { harness: "codex".into(), spec: program.clone(), session: None });
    a.wait_for_added();
    agent_says(&socket, &root, "codex", 0, "s1", "ready");
    a.wait_for_state(Some(Turn::Ready));
    a.send(Call::Detach);
    drop(a);

    let b = Client::attach(&socket, &root);
    assert_eq!(b.opened["agents"]["panes"], serde_json::json!(["ready"]), "still bound");
    let dir = root.join(".fab7/weft");
    let map: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("panes.json")).expect("the map"))
            .expect("json");
    let pane = &map["panes"][0];
    assert_eq!(
        (pane["harness"].as_str(), pane["spec"].as_str()),
        (Some("codex"), Some(program.as_str()))
    );
    assert_eq!(pane["session"], serde_json::json!({"session": "s1", "how": "signalled"}));
    assert!(pane["at"].as_i64().is_some_and(|t| t > 0), "{pane}");
    assert_eq!(std::fs::read_to_string(dir.join(".gitignore")).expect("ignored"), "*\n");

    let mut stop = b;
    stop.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// Weft's machine data: the projects open in the daemon, beside its
/// `config.toml`, for a second window's picker to offer.
#[test]
fn a_second_window_is_offered_the_projects_the_first_opened() {
    let home = project("machine");
    let config = home.join("config.toml");
    let socket = protocol::private_socket("weft-projects");
    let listening = socket.clone();
    let serving = config.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &serving);
    });
    let (one, two) = (project("opened-one"), project("opened-two"));
    let _a = Client::attach(&socket, &one);
    let mut b = Client::attach(&socket, &two);

    let text = std::fs::read_to_string(home.join("projects.json")).expect("the list");
    let offered: Vec<PathBuf> = weft::projects::read(&text).into_iter().map(|p| p.path).collect();
    assert_eq!(offered, [one.clone(), two.clone()], "{text}");
    assert!(weft::projects::read(&text).iter().all(|p| p.opened > 0), "{text}");

    b.send(Call::Shutdown);
    for dir in [home, one, two] {
        std::fs::remove_dir_all(dir).ok();
    }
}

/// Agents are told when they change, and only then: two sessions and no new
/// receipt send nothing more.
#[test]
fn agents_are_sent_once_until_a_receipt_moves() {
    let (root, socket) = session("agents-once");
    let mut c = Client::attach(&socket, &root);
    // Two sessions no pane holds, as a harness run elsewhere signals them.
    for (harness, id) in [("codex", "s1"), ("claude-code", "s2")] {
        support::signal(
            &socket,
            weft_core::signal::Sent {
                harness: harness.into(),
                hook: "Hook".into(),
                session: id.into(),
                workspace: Some(root.to_string_lossy().into_owned()),
                state: Some("turn_ended".into()),
                ..Default::default()
            },
        );
    }
    c.wait_until(|e| match e {
        Event::Agents { sessions, .. } if sessions.len() == 2 => Some(()),
        _ => None,
    });
    c.send(Call::StartAgent { harness: "codex".into(), spec: "/bin/cat".into(), session: None });
    c.wait_until(|e| match e {
        Event::Agents { panes, .. } if panes.len() == 1 => Some(()),
        _ => None,
    });
    let deadline = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < deadline {
        let again = c.next_event(Duration::from_millis(100));
        assert!(!matches!(again, Some(Event::Agents { .. })), "sent again: {again:?}");
    }
    c.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// What `stty size` says in this client's pane: the size the agent was given.
/// The mark is printed through an octal escape, so the echoed command line
/// never matches it.
fn size_seen(c: &mut Client, mark: &str) -> String {
    size_seen_in(c, 0, mark)
}

fn size_seen_in(c: &mut Client, pane: u32, mark: &str) -> String {
    let cmd = format!("printf '\\075{mark}\\075 %s\\n' \"$(stty size)\"\n");
    c.send(Call::Input { pane, bytes: cmd.into_bytes() });
    let needle = format!("={mark}= ");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = String::new();
    while Instant::now() < deadline {
        if let Some(Event::Output { bytes, .. }) = c.next_event(Duration::from_millis(250)) {
            seen.push_str(&String::from_utf8_lossy(&bytes));
        }
        if let Some(rest) = seen.split(&needle).nth(1)
            && let Some((size, _)) = rest.split_once('\n')
        {
            return size.trim().to_string();
        }
    }
    seen
}

/// A new agent starts at the size of the window that asked for it: the size
/// it opened the project at, then the size it last drew an agent at.
#[test]
fn a_new_pane_starts_at_the_size_of_the_window_that_asked() {
    let (root, socket) = session("start-size");
    let mut c = Client::attach_at(&socket, &root, 30, 100);
    c.send(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into(), session: None });
    c.wait_for_added();
    assert_eq!(size_seen(&mut c, "opened"), "30 100", "the size it opened at");
    c.send(Call::Resize { pane: 0, rows: 20, cols: 60 });
    c.send(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into(), session: None });
    let (pane, _) = c.wait_for_added();
    assert_eq!(size_seen_in(&mut c, pane, "drawn"), "20 60", "the size it last drew at");
    c.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// A second, smaller window opening the project leaves the
/// agent the size it had; the agent takes the size of whichever window last
/// focused it or typed into it.
#[test]
fn a_second_window_resizes_nothing_and_the_pane_follows_whoever_types() {
    let (root, socket) = session("sizes");
    let mut big = Client::attach(&socket, &root);
    big.hello();
    big.send(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into(), session: None });
    big.wait_for_added();
    big.send(Call::Resize { pane: 0, rows: 40, cols: 120 });
    assert_eq!(size_seen(&mut big, "one"), "40 120", "the first window's size");

    // Opening from a smaller window changes nothing.
    let mut small = Client::attach(&socket, &root);
    small.hello();
    assert_eq!(size_seen(&mut big, "two"), "40 120", "opening resized the pane");

    // The small window focuses the pane: it takes the small size.
    small.send(Call::Resize { pane: 0, rows: 20, cols: 60 });
    assert_eq!(size_seen(&mut small, "three"), "20 60", "the window that focused it");

    // The big window types into it: it takes the big size back.
    assert_eq!(size_seen(&mut big, "four"), "40 120", "the window that typed into it");
    // And the small one again.
    assert_eq!(size_seen(&mut small, "five"), "20 60", "the window that typed into it");

    big.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// turn-state.md §6 tests 3 and 4, at the daemon: with no receipt it types
/// nothing and says why; after `ready` it types; told to type anyway, it
/// types whatever the state.
#[test]
fn the_daemon_types_on_its_own_only_into_an_agent_that_said_it_is_ready() {
    let (root, socket) = session("ready");
    let mut c = Client::attach(&socket, &root);
    c.hello();
    c.send(Call::StartAgent { harness: "codex".into(), spec: agent(&socket), session: None });
    c.wait_for_added();
    let stage = |c: &mut Client, text: &str| {
        c.answer(Call::Stage {
            pane: 0,
            bytes: text.as_bytes().to_vec(),
            how: weft::inject::Handoff::Whole,
            what: "send".into(),
            why: String::new(),
        })
        .expect("staged")["pending"]
            .as_str()
            .expect("an id")
            .to_string()
    };

    // Not ready: the send waits for the agent rather than being refused at
    // once, and is refused only when the wait is over.
    let id = stage(&mut c, "not-yet");
    let asked = Instant::now();
    c.send(Call::Resolve { pending: id, yes: true, force: false, to: None, start: None });
    assert_eq!(c.injected_within(Duration::from_secs(2)), None, "waited for, not refused");

    // The person types in it and presses Enter, and its hook says it is
    // working: still not typed into, and refused once the wait is over.
    c.send(Call::Input { pane: 0, bytes: b"go\r".to_vec() });
    std::thread::sleep(Duration::from_millis(50));
    agent_says(&socket, &root, "codex", 0, "c1", "working");
    c.wait_for_state(Some(Turn::Working));
    let said = c.injected_within(Duration::from_secs(20)).expect("refused when the wait is over");
    assert_eq!(said.as_deref(), Some("the agent is working"));
    assert!(asked.elapsed() >= Duration::from_secs(14), "after the wait: {:?}", asked.elapsed());

    let id = stage(&mut c, "anyway-typed");
    c.send(Call::Resolve { pending: id, yes: true, force: true, to: None, start: None });
    assert_eq!(c.wait_for_refusal(), None, "the person said to type it anyway");

    agent_says(&socket, &root, "codex", 0, "c1", "turn_ended");
    c.wait_for_state(Some(Turn::TurnEnded));
    let id = stage(&mut c, "now-typed");
    c.send(Call::Resolve { pending: id, yes: true, force: false, to: None, start: None });
    assert_eq!(c.wait_for_refusal(), None);

    let mut stop = Client::attach(&socket, &root);
    let replay = stop.replay();
    assert!(!replay.contains("not-yet"), "{replay}");
    assert!(replay.contains("anyway-typed") && replay.contains("now-typed"), "{replay}");
    stop.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// Turbo mode: with `turbo = true` in Weft's config, every agent the daemon
/// starts gets its profile's turbo flags; the pane keeps the command line it
/// was asked for, and says it runs in turbo. A project that turns it off
/// starts agents without them.
#[test]
fn turbo_adds_each_harnesses_own_flags_to_the_agents_it_starts() {
    let mut echo = weft::harness_table::fixture::file("codex");
    echo["program"] = "echo".into();
    echo["turbo"] = serde_json::json!(["TURBO-FLAG"]);
    let harnesses = weft::harness_table::Harnesses(vec![
        weft::harness_table::Harness::of("echo-agent", &echo).expect("a harness"),
    ]);
    let on = project("turbo-on");
    let off = project("turbo-off");
    let socket = protocol::private_socket("weft-turbo");
    let config = socket.with_extension("turbo-config.toml");
    std::fs::write(
        &config,
        format!(
            "turbo = true\n\n[projects.\"{}\"]\nturbo = false\n",
            off.canonicalize().unwrap().display()
        ),
    )
    .expect("config");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(&listening, &config, Some(harnesses));
    });

    for (root, want, turbo) in [(&on, "hello TURBO-FLAG", true), (&off, "hello", false)] {
        let mut c = Client::attach(&socket, &root.canonicalize().unwrap());
        c.hello();
        c.send(Call::StartAgent {
            harness: "echo-agent".into(),
            spec: "echo hello".into(),
            session: None,
        });
        c.wait_for_added();
        // The whole line: under load its end can arrive in a later read.
        let seen = c.wait_for(want, 5);
        let line = seen.lines().find(|l| l.contains("hello")).unwrap_or("").trim().to_string();
        assert_eq!(line, want, "turbo {turbo}");
        let mut look = Client::attach(&socket, &root.canonicalize().unwrap());
        let panes = look.hello();
        assert_eq!(panes[0].spec, "echo hello", "the command line as asked");
        assert_eq!(panes[0].turbo, turbo);
    }

    let mut stop = Client::attach(&socket, &on);
    stop.send(Call::Shutdown);
    std::fs::remove_dir_all(&on).ok();
    std::fs::remove_dir_all(&off).ok();
}

/// The person turns turbo mode on and off from Weft. It starts
/// as `config.toml` says; an agent started after a switch follows it, and one
/// already running keeps the mode it was started in.
#[test]
fn turbo_can_be_switched_and_the_next_agent_follows_it() {
    let mut echo = weft::harness_table::fixture::file("codex");
    echo["program"] = "echo".into();
    echo["turbo"] = serde_json::json!(["TURBO-FLAG"]);
    let harnesses = weft::harness_table::Harnesses(vec![
        weft::harness_table::Harness::of("echo-agent", &echo).expect("a harness"),
    ]);
    let root = project("turbo-switch");
    let socket = protocol::private_socket("weft-turbo-switch");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(
            &listening,
            &listening.with_extension("no-config.toml"),
            Some(harnesses),
        );
    });
    let mut c = Client::attach(&socket, &root.canonicalize().unwrap());
    assert_eq!(c.opened.get("turbo"), Some(&serde_json::json!(false)), "off unless configured");
    let answer = c.answer(Call::Turbo { on: true }).expect("switched on");
    assert_eq!(answer.get("turbo"), Some(&serde_json::json!(true)));
    c.send(Call::StartAgent {
        harness: "echo-agent".into(),
        spec: "echo one".into(),
        session: None,
    });
    c.wait_for_added();
    c.answer(Call::Turbo { on: false }).expect("switched off");
    c.send(Call::StartAgent {
        harness: "echo-agent".into(),
        spec: "echo two".into(),
        session: None,
    });
    c.wait_for_added();
    let mut look = Client::attach(&socket, &root.canonicalize().unwrap());
    assert_eq!(look.opened.get("turbo"), Some(&serde_json::json!(false)), "as last switched");
    let panes = look.hello();
    assert_eq!(panes.iter().map(|p| p.turbo).collect::<Vec<_>>(), [true, false]);

    look.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// Eval acts on the Open round and Seal on an Eval's: a
/// Seal names its Eval, `--eval`, so RingFrame closes exactly its Asks; one
/// on an Eval still judged, already sealed, or unnamed is refused; Eval with
/// no Ask named goes on with the round being judged.
#[test]
fn a_seal_names_its_eval_and_an_eval_acts_on_the_round() {
    let root = project("rounds");
    ask_on_record(&root, "sh");
    gathered_on_record(&root);
    let socket = protocol::private_socket("weft-rounds");
    let listening = socket.clone();
    let program = agent(&socket);
    let harnesses = weft::harness_table::Harnesses(vec![harness_running("sh", &program)]);
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(
            &listening,
            &listening.with_extension("no-config.toml"),
            Some(harnesses),
        );
    });
    let mut c = Client::attach(&socket, &root);
    c.hello();
    c.send(Call::StartAgent { harness: "sh".into(), spec: program.clone(), session: None });
    c.wait_for_added();
    agent_says(&socket, &root, "sh", 0, "s1", "ready");
    c.wait_for_state(Some(Turn::Ready));
    let act = |act: &str, eval: Option<&str>| Call::Act {
        act: act.into(),
        unit: None,
        pane: None,
        text: None,
        eval: eval.map(str::to_string),
        start: None,
    };
    assert_eq!(c.answer(act("decide", None)), Err("no_eval".into()));
    assert_eq!(c.answer(act("decide", Some("evl_1"))), Err("judging".into()));
    assert_eq!(c.answer(act("decide", Some("evl_9"))), Err("no_eval".into()));

    // Eval on the round being judged sends its debate.
    c.send(act("check", None));
    let payload = c.wait_until(|e| match e {
        Event::Pending { payload, .. } => Some(String::from_utf8_lossy(&payload).into_owned()),
        _ => None,
    });
    assert!(payload.contains("evl_1"), "{payload}");

    // Judged, the Seal on it names it.
    let line = |kind: &str, id: &str, data: serde_json::Value, links: serde_json::Value| {
        serde_json::json!({"schema": "ringframe.ledger/1", "event_id": "evt_r", "type": kind,
            "time": "2026-09-19T14:04:00Z", "id": id,
            "actor": {"kind": "human", "id": "me"}, "links": links, "data": data})
    };
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(root.join(".fab7/rf/ledger.jsonl"))
        .expect("ledger");
    let basis = serde_json::json!({"basis": {"asks": ["ask_1"]}, "verdict": "aligned",
                                   "confidence": 1.0});
    writeln!(f, "{}", line("eval.completed", "evl_1", basis, serde_json::json!([]))).unwrap();
    drop(f);
    c.wait_until(|e| match e {
        Event::Units { rounds, .. } if rounds.evals.iter().any(|r| r.eval == "evl_1") => Some(()),
        _ => None,
    });
    c.send(act("decide", Some("evl_1")));
    let staged = c.wait_until(|e| match e {
        Event::Pending { payload, .. } => Some(String::from_utf8_lossy(&payload).into_owned()),
        _ => None,
    });
    assert!(staged.contains("seal") && staged.ends_with("--eval evl_1"), "{staged}");

    // Sealed, the act stays and says so (D12.6).
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(root.join(".fab7/rf/ledger.jsonl"))
        .expect("ledger");
    let links =
        serde_json::json!([{"rel": "seals", "id": "ask_1"}, {"rel": "seals", "id": "evl_1"}]);
    let data = serde_json::json!({"basis": {"asks": ["ask_1"]}, "disposition": "accepted"});
    writeln!(f, "{}", line("seal.created", "sel_1", data, links)).unwrap();
    drop(f);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got = c.answer(act("decide", Some("evl_1")));
        if got == Err("sealed_already".into()) {
            break;
        }
        assert!(Instant::now() < deadline, "a sealed Eval was sealed again: {got:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut stop = Client::attach(&socket, &root);
    stop.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

// --- an unclean end --------------------------------------------------------

/// `weft --serve`, the real program, in a home and a runtime folder of its
/// own: its socket, and the process to kill.
fn real_daemon(home: &Path) -> (std::process::Child, PathBuf) {
    let runtime = home.join("run");
    std::fs::create_dir_all(&runtime).unwrap();
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_weft"))
        .arg("--serve")
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("PATH", "/usr/bin:/bin")
        .env_remove(weftd::service::MANAGED)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("weft --serve");
    let socket = runtime.join("weft").join("weft.sock");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !protocol::is_live(&socket) {
        assert!(Instant::now() < deadline, "the daemon never came up");
        std::thread::sleep(Duration::from_millis(20));
    }
    (child, socket)
}

fn listed(home: &Path) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(home.join(".fab7/weft/projects.json")).unwrap_or_default();
    weft::projects::read(&text).into_iter().map(|p| p.path).collect()
}

/// A daemon killed with two projects open starts again with both, each
/// reopened, with no agent running; a `weft stop` starts again with none.
#[test]
fn a_daemon_killed_with_two_projects_open_reopens_both() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path().canonicalize().unwrap();
    let (one, two) = (home.join("one"), home.join("two"));
    for p in [&one, &two] {
        std::fs::create_dir_all(p).unwrap();
    }
    let (mut daemon, socket) = real_daemon(&home);
    let mut a = Client::attach(&socket, &one);
    let b = Client::attach(&socket, &two);
    assert_eq!(a.opened["reopened"], serde_json::Value::Null, "opened by a window");
    a.send(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into(), session: None });
    a.wait_for_added();

    // Killed: no `weft stop`, as a crash or a restart of the machine.
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    drop((a, b));
    assert_eq!(listed(&home), [one.clone(), two.clone()], "the list outlives it");

    let started = weftd::turns::now_millis();
    let (mut daemon, socket) = real_daemon(&home);
    assert_eq!(listed(&home), [one.clone(), two.clone()], "both open again");
    let mut remote = Client::attach_as(&socket, &one, 24, 80, "weft-remote/0.2.0");
    let since = remote.opened["reopened"].as_i64().expect("reopened, with when");
    assert!(since >= started, "{since} is when this daemon started");
    assert_eq!(remote.hello().len(), 0, "no agent runs in it");

    // `weft stop`: the next daemon opens nothing.
    protocol::stop(&socket).unwrap();
    daemon.wait().unwrap();
    assert!(listed(&home).is_empty(), "a stop empties the list");
    let (mut daemon, socket) = real_daemon(&home);
    assert!(listed(&home).is_empty(), "and nothing is reopened");
    protocol::stop(&socket).unwrap();
    daemon.wait().unwrap();
}

/// A daemon that finds a project listed (the one before it ended without a
/// stop) reopens it with its sessions ended, each resumable by its id.
#[test]
fn a_reopened_project_shows_its_sessions_ended_and_resumable() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    let root = home.path().join("gl");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let mut harness = weft::harness_table::fixture::file("codex");
    harness["program"] = "cat".into();
    harness["resume"] = serde_json::json!(["-"]);
    let harnesses = weft::harness_table::Harnesses(vec![
        weft::harness_table::Harness::of("codex", &harness).expect("a harness"),
    ]);
    let serve = |socket: &PathBuf| {
        let (listening, config, harnesses) = (socket.clone(), config.clone(), harnesses.clone());
        std::thread::spawn(move || {
            let _ = server::Session::serve_with(&listening, &config, Some(harnesses));
        });
    };

    // The first daemon: a session of this project, its process now gone.
    let first = protocol::private_socket("weft-reopen-a");
    serve(&first);
    let mut c = Client::attach(&first, &root);
    let gone = std::process::Command::new("true").spawn().unwrap();
    let pid = gone.id();
    let start = weftd::process::info(pid).map(|i| i.start).or(Some("0.000001".into()));
    support::signal(
        &first,
        weft_core::signal::Sent {
            harness: "codex".into(),
            hook: "Hook".into(),
            session: "s_old".into(),
            workspace: Some(root.to_string_lossy().into_owned()),
            state: Some("turn_ended".into()),
            pid: Some(pid),
            pid_start: start,
            ..Default::default()
        },
    );
    let mut gone = gone;
    gone.wait().unwrap();
    c.wait_until(|e| match e {
        Event::Agents { sessions, .. } if sessions.iter().any(|r| r.id == "s_old") => Some(()),
        _ => None,
    });

    // The next daemon finds it listed, as one would after a crash.
    let second = protocol::private_socket("weft-reopen-b");
    serve(&second);
    let mut d = Client::attach_as(&second, &root, 24, 80, "weft-remote/0.2.0");
    assert!(d.opened["reopened"].is_i64(), "{}", d.opened["reopened"]);
    let rows = &d.opened["agents"]["sessions"];
    let old = rows.as_array().unwrap().iter().find(|r| r["id"] == "s_old").expect("listed");
    assert_eq!(old["state"], "ended", "{old}");

    // Resumed by its own id: no longer only reopened.
    d.send(Call::StartAgent {
        harness: "codex".into(),
        spec: "cat".into(),
        session: Some("s_old".into()),
    });
    let cleared = d.wait_until(|e| match e {
        Event::Reopened { since } => Some(since),
        _ => None,
    });
    assert_eq!(cleared, None, "a device resumed something in it");
    d.wait_for_added();
    d.send(Call::Shutdown);
    c.send(Call::Shutdown);
}

/// A reopened project with no window is kept while nothing has been done in
/// it; a window's visit then closes it by the 2026-10-07 rule, and Dismiss
/// closes one at once.
#[test]
fn a_reopened_project_is_kept_until_someone_acts_in_it() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path().canonicalize().unwrap();
    let (kept, dismissed) = (home.join("kept"), home.join("dismissed"));
    for p in [&kept, &dismissed] {
        std::fs::create_dir_all(p).unwrap();
    }
    let list = weft::projects::write(&[
        weft::projects::Opened { path: kept.clone(), opened: 1 },
        weft::projects::Opened { path: dismissed.clone(), opened: 2 },
        weft::projects::Opened { path: "/gone/before".into(), opened: 3 },
    ]);
    std::fs::write(home.join("projects.json"), list).unwrap();
    let socket = protocol::private_socket("weft-reopen-kept");
    let (listening, config) = (socket.clone(), home.join("config.toml"));
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &config);
    });
    let now = || {
        let text = std::fs::read_to_string(home.join("projects.json")).unwrap_or_default();
        weft::projects::read(&text).into_iter().map(|p| p.path).collect::<Vec<_>>()
    };
    let mut remote = Client::attach_as(&socket, &kept, 24, 80, "weft-remote/0.2.0");
    assert_eq!(now(), [kept.clone(), dismissed.clone()], "a folder that is gone is not");

    // `weft remote` comes and goes: it holds nothing open, and nothing closes.
    remote.send(Call::Detach);
    drop(remote);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(now(), [kept.clone(), dismissed.clone()], "kept while nobody acted");

    // A window opens it, which is someone in it; when it leaves with no
    // agent running, it closes as any project would.
    let mut w = Client::attach(&socket, &kept);
    assert_eq!(w.opened["reopened"], serde_json::Value::Null, "a window holds it now");
    w.send(Call::Detach);
    drop(w);
    let until = |want: &[PathBuf]| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while now() != want {
            assert!(Instant::now() < deadline, "listed: {:?}, not {want:?}", now());
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    until(std::slice::from_ref(&dismissed));

    // Dismissed from a device: closed at once.
    let mut device = Client::attach_as(&socket, &dismissed, 24, 80, "weft-remote/0.2.0");
    assert_eq!(device.answer(Call::Dismiss), Ok(serde_json::json!({})));
    until(&[]);
    let mut stop = Client::attach(&socket, &home);
    stop.send(Call::Shutdown);
}
