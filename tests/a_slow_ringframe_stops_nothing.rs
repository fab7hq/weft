//! A slow `ringframe` holds up only what waits on it: while `init`,
//! `ask preflight`, `eval open` and `seal check` run, the daemon goes on
//! answering every other call.
//!
//! Its own test binary because it puts a stand-in `ringframe` first on PATH,
//! which is global: one test, one process.

mod support;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;

/// How long the stand-in takes over each of the four.
const SLOW: &str = "2";

/// A `ringframe` that takes its time over `init`, `ask preflight`, `eval
/// open` and `seal check`, then answers as RingFrame does.
fn stand_in(dir: &Path, ledger: &Path) {
    let opened = r#"{"schema":"ringframe.ledger/1","event_id":"evt_o","type":"eval.opened","time":"2026-09-28T10:00:00Z","id":"evl_new","actor":{"kind":"human","id":"me"},"links":[],"data":{"basis":{"asks":["ask_1"]}}}"#;
    let gathered = r#"{"schema":"ringframe.ledger/1","event_id":"evt_g","type":"eval.gathered","time":"2026-09-28T10:00:01Z","id":"evl_new","actor":{"kind":"human","id":"me"},"links":[],"data":{"eval_id":"evl_new","host":"weft"}}"#;
    let script = format!(
        "#!/bin/sh\ncase \"$*\" in\n  \
         *\" init\") sleep {SLOW}; mkdir -p \"$2/.fab7/rf\" ;;\n  \
         *\"ask preflight\"*) sleep {SLOW} ;;\n  \
         *\"eval open\"*) sleep {SLOW}; printf '%s\\n%s\\n' '{opened}' '{gathered}' >> '{ledger}'; printf '{{\"eval_id\":\"evl_new\"}}' ;;\n  \
         *\"seal check\"*) sleep {SLOW}; printf '{{\"fresh\":true,\"subject_matches\":true}}' ;;\n  \
         *\"profile show\"*) printf '{{\"invocation_prefix\":\"/rf:\"}}' ;;\n  \
         *\"profile list\"*) printf '{{\"profiles\":[]}}' ;;\n  \
         *) printf '{{}}' ;;\nesac\nexit 0\n",
        ledger = ledger.display(),
    );
    let path = dir.join("ringframe");
    std::fs::write(&path, script).expect("stand-in");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

struct Client {
    stream: UnixStream,
    lines: Lines<UnixStream>,
    id: u64,
    /// The events read while waiting for an answer.
    seen: Vec<Event>,
}

impl Client {
    fn connect(socket: &Path) -> Client {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            if let Ok(s) = UnixStream::connect(socket) {
                break s;
            }
            assert!(Instant::now() < deadline, "no daemon");
            std::thread::sleep(Duration::from_millis(20));
        };
        let lines = Lines::new(stream.try_clone().expect("clone"));
        Client { stream, lines, id: 0, seen: Vec::new() }
    }

    fn call(&mut self, call: Call) -> u64 {
        self.id += 1;
        self.stream.write_all(&Line::Call { id: self.id, call }.encode()).expect("write");
        self.id
    }

    /// The answer to call `id`; the events before it are kept in `seen`.
    fn answer(&mut self, id: u64, within: Duration) -> serde_json::Value {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(100))).expect("timeout");
            match self.lines.next() {
                Ok(Some(Line::Result { id: got, result })) if got == id => return result,
                Ok(Some(Line::Error { id: got, message, .. })) if got == id => {
                    panic!("call {id} refused: {message}")
                }
                Ok(Some(Line::Event(e))) => self.seen.push(e),
                _ => {}
            }
        }
        panic!("no answer to call {id} in {within:?}");
    }

    fn wait<T>(&mut self, mut pick: impl FnMut(Event) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(100))).expect("timeout");
            if let Ok(Some(Line::Event(e))) = self.lines.next()
                && let Some(found) = pick(e)
            {
                return found;
            }
        }
        panic!("nothing arrived");
    }
}

/// How long the daemon takes to answer a new client's hello.
fn probe(socket: &Path) -> Duration {
    let mut c = Client::connect(socket);
    let asked = Instant::now();
    let id = c.call(Call::Hello { client: "probe".into(), protocol: PROTOCOL });
    c.answer(id, Duration::from_secs(10));
    asked.elapsed()
}

#[test]
fn a_slow_ringframe_holds_up_only_what_waits_on_it() {
    let tmp = std::env::temp_dir().join(format!("weft-slow-rf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (bin, fresh, root) = (tmp.join("bin"), tmp.join("fresh"), tmp.join("project"));
    std::fs::create_dir_all(&bin).expect("bin");
    std::fs::create_dir_all(&fresh).expect("fresh");
    std::fs::create_dir_all(root.join(".fab7/rf")).expect("record");
    let ledger = root.join(".fab7/rf/ledger.jsonl");
    stand_in(&bin, &ledger);
    // SAFETY: this binary holds exactly one test.
    unsafe {
        std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    }
    let compiled = serde_json::json!({"schema": "ringframe.ledger/1", "event_id": "evt_1",
        "type": "ask.compiled", "time": "2026-09-27T10:00:00Z", "id": "ask_1",
        "actor": {"kind": "human", "id": "me"}, "links": [],
        "data": {"title": "Ship it", "selected_capability": "native_plan",
                 "delivery_mode": "human_handoff", "host": {"name": "sh"},
                 "source": {}, "prompt": {}, "source_verified": "exact", "limitations": [],
                 "classification": {}, "route_explanation": {}}});
    std::fs::write(&ledger, format!("{compiled}\n")).expect("ledger");

    let socket = protocol::private_socket("weft-slow-rf");
    let agent = support::agent(&support::agents_dir(&socket));
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
    });
    let mut a = Client::connect(&socket);
    let hello = a.call(Call::Hello { client: "test".into(), protocol: PROTOCOL });
    a.answer(hello, Duration::from_secs(5));
    let open = |a: &mut Client, path: &Path| {
        a.call(Call::Open { rows: 24, cols: 80, path: path.to_string_lossy().into(), raw: true })
    };
    let quick = Duration::from_millis(500);

    // Opening a project RingFrame is not set up in: `init`, then `ask preflight`.
    let opening = open(&mut a, &fresh);
    std::thread::sleep(Duration::from_millis(300));
    let took = probe(&socket);
    assert!(took < quick, "a hello waited {took:?} on init");
    let opened = a.answer(opening, Duration::from_secs(10));
    assert_eq!(opened["setup"], serde_json::Value::Null, "set up by its init: {opened}");

    // A set-up project: `ask preflight` only. A window already in it stages
    // a send meanwhile: the opening window gets it in its answer, and not as
    // an event before the answer too.
    let mut b = Client::connect(&socket);
    let hello = b.call(Call::Hello { client: "test".into(), protocol: PROTOCOL });
    b.answer(hello, Duration::from_secs(5));
    let opening = open(&mut b, &root);
    b.answer(opening, Duration::from_secs(10));
    let opening = open(&mut a, &root);
    std::thread::sleep(Duration::from_millis(300));
    let took = probe(&socket);
    assert!(took < quick, "a hello waited {took:?} on ask preflight");
    let staged = b.call(Call::Stage {
        pane: 0,
        bytes: b"hello".to_vec(),
        how: weft_core::inject::Handoff::Whole,
        what: "a send".into(),
        why: String::new(),
    });
    b.answer(staged, Duration::from_secs(5));
    a.seen.clear();
    let opened = a.answer(opening, Duration::from_secs(10));
    assert_eq!(opened["pendings"].as_array().map(Vec::len), Some(1), "{opened}");
    assert!(
        !a.seen.iter().any(|e| matches!(e, Event::Pending { .. })),
        "the send came before the answer too: {:?}",
        a.seen
    );

    // Eval: `eval open`, then the debate is staged.
    a.call(Call::StartAgent { harness: "sh".into(), spec: agent, session: None });
    a.wait(|e| matches!(e, Event::Added { .. }).then_some(()));
    support::pane_says(&socket, &support::agents_dir(&socket), &root, "sh", 0, "s1", "ready");
    a.wait(|e| match e {
        Event::Agents { panes, .. }
            if panes.first().copied().flatten() == Some(weft::turns::Turn::Ready) =>
        {
            Some(())
        }
        _ => None,
    });
    let check = a.call(Call::Act {
        act: "check".into(),
        unit: Some("ask_1".into()),
        pane: None,
        text: None,
        eval: None,
        start: None,
    });
    std::thread::sleep(Duration::from_millis(300));
    let took = probe(&socket);
    assert!(took < quick, "a hello waited {took:?} on eval open");
    let staged = a.answer(check, Duration::from_secs(10));
    assert!(staged["pending"].is_string(), "the debate staged: {staged}");

    // A receipt's check: `seal check`.
    let read = a.call(Call::Read {
        what: "seal".into(),
        unit: "sel_1".into(),
        pane: None,
        lines: None,
        before: None,
        after: None,
    });
    std::thread::sleep(Duration::from_millis(300));
    let took = probe(&socket);
    assert!(took < quick, "a hello waited {took:?} on seal check");
    let checked = a.answer(read, Duration::from_secs(10));
    assert_eq!(checked["fresh"], true, "{checked}");

    a.call(Call::Shutdown);
    std::fs::remove_dir_all(&tmp).ok();
}
