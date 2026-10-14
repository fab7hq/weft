//! What the daemon gives `weft remote` (remote.md §3, D4 to D8): a client
//! that never takes raw output, the pendings already open, presence, history,
//! and the remote view relayed between `weft remote` and the TUI.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;

struct Client {
    next: u64,
    opened: serde_json::Value,
    stream: UnixStream,
    lines: Lines<UnixStream>,
}

impl Client {
    /// Say hello as `name`, and open `project` with raw output or none.
    fn open(socket: &Path, project: &Path, name: &str, raw: bool) -> Self {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            if let Ok(s) = UnixStream::connect(socket) {
                break s;
            }
            assert!(Instant::now() < deadline, "the server never came up");
            std::thread::sleep(Duration::from_millis(20));
        };
        let lines = Lines::new(stream.try_clone().unwrap());
        let mut c = Client { next: 1, opened: serde_json::json!({}), stream, lines };
        c.answer(Call::Hello { client: name.into(), protocol: PROTOCOL }).expect("hello");
        let path = project.to_string_lossy().into_owned();
        c.opened = c.answer(Call::Open { path, rows: 24, cols: 80, raw }).expect("open");
        c
    }

    fn send(&mut self, call: Call) -> u64 {
        let id = self.next;
        self.next += 1;
        self.stream.write_all(&Line::Call { id, call }.encode()).unwrap();
        id
    }

    fn answer(&mut self, call: Call) -> Result<serde_json::Value, String> {
        let id = self.send(call);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            match self.lines.next() {
                Ok(Some(Line::Result { id: got, result })) if got == id => return Ok(result),
                Ok(Some(Line::Error { id: got, code, .. })) if got == id => return Err(code),
                Ok(None) => break,
                _ => continue,
            }
        }
        panic!("no answer to call {id}");
    }

    /// Every event that arrives within `within`.
    fn events(&mut self, within: Duration) -> Vec<Event> {
        let deadline = Instant::now() + within;
        let mut out = Vec::new();
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
            match self.lines.next() {
                Ok(Some(Line::Event(e))) => out.push(e),
                Ok(None) => break,
                _ => {}
            }
        }
        out
    }

    /// Events until one is picked, or panic after five seconds.
    fn wait_for<T>(&mut self, mut pick: impl FnMut(&Event) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            for e in self.events(Duration::from_millis(100)) {
                if let Some(t) = pick(&e) {
                    return t;
                }
            }
        }
        panic!("it never came");
    }
}

fn daemon(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("weft-remote-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let socket = protocol::private_socket("weft-remote");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
    });
    (root, socket)
}

fn sh(c: &mut Client) -> u32 {
    let started =
        c.answer(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into(), session: None });
    started.unwrap()["pane"].as_u64().unwrap() as u32
}

/// remote.md D4: `output: "none"` is never sent a pane's bytes, and hears the rest.
#[test]
fn a_client_that_asked_for_no_output_gets_none_and_every_other_event() {
    let (root, socket) = daemon("quiet");
    let mut tui = Client::open(&socket, &root, "weft-tui", true);
    let pane = sh(&mut tui);
    tui.send(Call::Input { pane, bytes: b"printf 'printed-before\\n'\n".to_vec() });
    tui.wait_for(|e| {
        matches!(e, Event::Output { bytes, .. }
        if String::from_utf8_lossy(bytes).contains("printed-before"))
        .then_some(())
    });

    let mut remote = Client::open(&socket, &root, "weft-remote", false);
    assert_eq!(remote.opened["replay"], serde_json::json!([]), "no replay either");

    tui.send(Call::Input { pane, bytes: b"printf 'printed-after\\n'\n".to_vec() });
    let staged = tui.answer(Call::Stage {
        pane,
        bytes: b"echo hi".to_vec(),
        how: weft::inject::Handoff::Whole,
        what: "a test".into(),
        why: "to see it arrive".into(),
    });
    let id = staged.unwrap()["pending"].as_str().unwrap().to_string();
    let second = sh(&mut tui);

    let heard = remote.events(Duration::from_millis(1500));
    assert!(!heard.iter().any(|e| matches!(e, Event::Output { .. })), "no output: {heard:?}");
    assert!(heard.iter().any(|e| matches!(e, Event::Pending { id: got, .. } if *got == id)));
    assert!(heard.iter().any(|e| matches!(e, Event::Added { pane, .. } if *pane == second)));

    tui.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// remote.md D5: a client that opens the project after a `stage` sees it, and can
/// resolve it.
#[test]
fn a_client_attaching_after_a_stage_sees_it_and_can_answer_it() {
    let (root, socket) = daemon("pendings");
    let mut tui = Client::open(&socket, &root, "weft-tui", true);
    let pane = sh(&mut tui);
    let staged = tui.answer(Call::Stage {
        pane,
        bytes: b"echo staged-earlier".to_vec(),
        how: weft::inject::Handoff::Whole,
        what: "Ready to send".into(),
        why: "the exact wording".into(),
    });
    let id = staged.unwrap()["pending"].as_str().unwrap().to_string();

    let mut remote = Client::open(&socket, &root, "weft-remote", false);
    let listed = remote.opened["pendings"].as_array().expect("pendings").clone();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(
        protocol::pending_of(&listed[0]),
        Some(Event::Pending {
            id: id.clone(),
            pane,
            what: "Ready to send".into(),
            why: "the exact wording".into(),
            payload: b"echo staged-earlier".to_vec(),
            sends: None,
        })
    );

    remote
        .answer(Call::Resolve {
            pending: id.clone(),
            yes: false,
            force: false,
            to: None,
            start: None,
        })
        .unwrap();
    tui.wait_for(|e| {
        matches!(e, Event::Resolved { id: got, yes: false } if *got == id).then_some(())
    });
    let again = Client::open(&socket, &root, "weft-remote", false);
    assert_eq!(again.opened["pendings"], serde_json::json!([]), "answered, so gone");

    tui.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// remote.md D6: the TUI's typing is told as `presence`, at most once in ten seconds;
/// `weft-remote`'s own typing never is.
#[test]
fn the_tuis_typing_is_presence_at_most_once_in_ten_seconds() {
    let (root, socket) = daemon("presence");
    let mut tui = Client::open(&socket, &root, "weft-tui/0.1.3", true);
    let pane = sh(&mut tui);
    let mut remote = Client::open(&socket, &root, "weft-remote", false);
    let presences = |events: Vec<Event>| {
        events.into_iter().filter(|e| matches!(e, Event::Presence { .. })).count()
    };

    remote.answer(Call::Input { pane, bytes: b"true\n".to_vec() }).unwrap();
    assert_eq!(
        presences(remote.events(Duration::from_millis(600))),
        0,
        "a device is not the person"
    );

    tui.answer(Call::Input { pane, bytes: b"true\n".to_vec() }).unwrap();
    tui.answer(Call::Input { pane, bytes: b"true\n".to_vec() }).unwrap();
    let at = remote.wait_for(|e| match e {
        Event::Presence { at } => Some(*at),
        _ => None,
    });
    assert!(at > 1_700_000_000_000, "milliseconds: {at}");
    tui.answer(Call::Input { pane, bytes: b"true\n".to_vec() }).unwrap();
    assert_eq!(presences(remote.events(Duration::from_millis(600))), 0, "once in ten seconds");

    tui.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// remote.md D7: `read {what: "history", pane, lines}` is the last lines of
/// scrollback and screen as text, at most 2,000.
#[test]
fn history_is_the_last_lines_of_scrollback_and_screen_as_text() {
    let (root, socket) = daemon("history");
    let mut tui = Client::open(&socket, &root, "weft-tui", true);
    let pane = sh(&mut tui);
    tui.send(Call::Input {
        pane,
        bytes: b"clear; for i in $(seq 1 3000); do echo history-line-$i; done\n".to_vec(),
    });
    tui.wait_for(|e| {
        matches!(e, Event::Output { bytes, .. }
        if String::from_utf8_lossy(bytes).contains("history-line-3000"))
        .then_some(())
    });
    std::thread::sleep(Duration::from_millis(300));

    let read = |c: &mut Client, lines: u64| {
        let what = "history".to_string();
        let answer = c.answer(Call::Read {
            what,
            unit: String::new(),
            pane: Some(pane),
            lines: Some(lines),
            before: None,
            after: None,
        });
        answer.unwrap()["text"].as_str().unwrap().to_string()
    };
    let mut remote = Client::open(&socket, &root, "weft-remote", false);
    let ten = read(&mut remote, 10);
    let ten: Vec<&str> = ten.lines().collect();
    assert_eq!(ten.len(), 10, "{ten:?}");
    assert_eq!(ten[8], "history-line-3000", "the last line before the prompt: {ten:?}");
    assert_eq!(ten[0], "history-line-2992");

    let all = read(&mut remote, 50_000);
    let all: Vec<&str> = all.lines().collect();
    assert_eq!(all.len(), 2000, "capped");
    assert_eq!(all[0], "history-line-1002", "scrollback and screen, in order, none twice");
    assert!(all.windows(2).take(1998).all(|w| w[0] != w[1]));

    tui.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// remote.md D8: `weft-remote` publishes its view and every client hears it; an act
/// from any other client reaches `weft-remote` unchanged.
#[test]
fn the_remote_view_and_its_acts_are_relayed() {
    let world = weft::outside::Fixed::new();
    let (root, socket, _) = daemon_in("relay", "[remote]\nenabled = true\n", world);
    let mut tui = Client::open(&socket, &root, "weft-tui", true);
    let mut remote = Client::open(&socket, &root, "weft-remote", false);
    let view = serde_json::json!({"state": "on", "address": "https://a-b.trycloudflare.com"});

    assert_eq!(
        tui.answer(Call::RemoteView { view: view.clone() }),
        Err("not_remote".into()),
        "only weft-remote publishes it"
    );
    remote.answer(Call::RemoteView { view: view.clone() }).unwrap();
    let heard = tui.wait_for(|e| match e {
        Event::Remote { view } => Some(view.clone()),
        _ => None,
    });
    assert_eq!(heard, view);
    let late = Client::open(&socket, &root, "weft-tui", true);
    assert_eq!(late.opened["remote"], view, "kept for a client that opens later");

    let act = serde_json::json!({"act": "allow", "device": "d_1", "yes": true});
    tui.answer(Call::Remote { act: act.clone() }).unwrap();
    let got = remote.wait_for(|e| match e {
        Event::RemoteAct { act } => Some(act.clone()),
        _ => None,
    });
    assert_eq!(got, act, "unchanged");

    tui.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// A daemon in a world of its own, reading `config` as its `config.toml`.
fn daemon_in(name: &str, config: &str, world: weft::outside::Fixed) -> (PathBuf, PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("weft-remote-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("config.toml");
    std::fs::write(&file, config).unwrap();
    let socket = protocol::private_socket("weft-remote");
    let (listening, read) = (socket.clone(), file.clone());
    std::thread::spawn(move || {
        let _ = server::Session::serve_in(&listening, &read, None, world.shared());
    });
    (root, socket, file)
}

/// remote.md D9, D10: `enabled = true` keeps `weft remote` running, restarted with a
/// 2, 4, 8 … second backoff; turning it off sends `stop` and it is not
/// started again; an unknown key in `[remote]` is reported as ignored.
#[test]
fn the_daemon_keeps_weft_remote_running_while_it_is_turned_on() {
    let world = weft::outside::Fixed::new();
    let runs = world.remote_runs();
    let (root, socket, file) =
        daemon_in("supervised", "[remote]\nenabled = true\nrelay = \"x\"\n", world);
    let mut tui = Client::open(&socket, &root, "weft-tui", true);
    let ignored = tui.opened["routing"]["ignored"].clone();
    assert_eq!(ignored, serde_json::json!(["remote.relay"]));
    let mut remote = Client::open(&socket, &root, "weft-remote", false);
    remote.answer(Call::RemoteView { view: serde_json::json!({"state": "on"}) }).unwrap();

    // Each run here ends at once, as a crash would.
    std::thread::sleep(Duration::from_millis(6_500));
    let at = runs.lock().unwrap().clone();
    assert_eq!(at.len(), 3, "started, then again after 2 s and 4 s: {at:?}");
    let gaps: Vec<_> = at.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(gaps[0] >= Duration::from_secs(2) && gaps[0] < Duration::from_secs(3), "{gaps:?}");
    assert!(gaps[1] >= Duration::from_secs(4) && gaps[1] < Duration::from_secs(5), "{gaps:?}");

    tui.answer(Call::Remote { act: serde_json::json!({"act": "off"}) }).unwrap();
    let told = remote.wait_for(|e| match e {
        Event::RemoteAct { act } => Some(act.clone()),
        _ => None,
    });
    assert_eq!(told, serde_json::json!({"act": "stop"}));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("enabled = false"), "{text}");
    std::thread::sleep(Duration::from_millis(9_500));
    assert_eq!(runs.lock().unwrap().len(), 3, "turned off, so not started again");

    tui.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// Off in `config.toml` starts nothing; `on` from a client turns it on.
#[test]
fn weft_remote_starts_when_turned_on_and_not_before() {
    let world = weft::outside::Fixed::new();
    let runs = world.remote_runs();
    let (root, socket, file) = daemon_in("off", "", world);
    let mut tui = Client::open(&socket, &root, "weft-tui", true);
    std::thread::sleep(Duration::from_millis(1_500));
    assert!(runs.lock().unwrap().is_empty(), "off unless turned on");

    tui.answer(Call::Remote { act: serde_json::json!({"act": "on"}) }).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(runs.lock().unwrap().len(), 1);
    assert!(std::fs::read_to_string(&file).unwrap().contains("[remote]\nenabled = true"));

    tui.answer(Call::Remote { act: serde_json::json!({"act": "off"}) }).unwrap();
    tui.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

/// Turned off before `weft remote` first said its view, it is told to stop
/// as soon as it does: no tunnel stays up while Remote is off (found on
/// Linux).
#[test]
fn a_weft_remote_that_speaks_while_remote_is_off_is_told_to_stop() {
    let world = weft::outside::Fixed::new();
    let (root, socket, _) = daemon_in("late-view", "", world);
    let mut tui = Client::open(&socket, &root, "weft-tui", true);
    let mut remote = Client::open(&socket, &root, "weft-remote", false);
    remote.send(Call::RemoteView { view: serde_json::json!({"state": "on"}) });
    let told = remote.wait_for(|e| match e {
        Event::RemoteAct { act } => Some(act.clone()),
        _ => None,
    });
    assert_eq!(told, serde_json::json!({"act": "stop"}));
    tui.send(Call::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}
