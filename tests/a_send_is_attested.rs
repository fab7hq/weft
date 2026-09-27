//! A prompt Weft sends on the person's yes is recorded as submitted by them,
//! through RingFrame (`ringframe ask submitted`), so the Ask knows it was
//! sent even where no hook can observe it arriving; a send Weft did not
//! complete records nothing.
//!
//! Its own test binary because it puts a stand-in `ringframe` first on PATH,
//! which is global: one test, one process.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;

/// A `ringframe` that hands over one prompt, answers everything else, and
/// logs every call it gets.
fn stand_in(dir: &Path) {
    let log = dir.join("calls.log");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$*\" in\n  *\"ask copy\"*) printf 'Ship the endpoint.' ;;\n  *\"profile list\"*) printf '{{\"profiles\":[]}}' ;;\n  *) printf '{{}}' ;;\nesac\nexit 0\n",
        log.display()
    );
    let path = dir.join("ringframe");
    std::fs::write(&path, script).expect("stand-in");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn calls(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default()
}

fn wait<T>(
    frames: &mut Lines<UnixStream>,
    stream: &UnixStream,
    mut pick: impl FnMut(Event) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        stream.set_read_timeout(Some(Duration::from_millis(100))).expect("timeout");
        if let Ok(Some(Line::Event(e))) = frames.next()
            && let Some(found) = pick(e)
        {
            return found;
        }
    }
    panic!("nothing arrived");
}

#[test]
fn a_send_weft_typed_is_recorded_as_submitted_and_a_refused_one_is_not() {
    let tmp = std::env::temp_dir().join(format!("weft-attest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let bin = tmp.join("bin");
    let root = tmp.join("project");
    std::fs::create_dir_all(&bin).expect("bin");
    std::fs::create_dir_all(root.join(".fab7/rf")).expect("record");
    stand_in(&bin);
    // SAFETY: this binary holds exactly one test.
    unsafe {
        std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    }
    let event = |kind: &str, time: &str, data: serde_json::Value| {
        serde_json::json!({"schema": "ringframe.ledger/1", "event_id": format!("evt_{kind}"),
            "type": kind, "time": time, "id": "ask_1", "actor": {"kind": "human", "id": "me"},
            "links": [], "data": data})
    };
    let ledger = [
        event(
            "ask.compiled",
            "2026-09-27T10:00:00Z",
            serde_json::json!({
            "title": "Ship it", "selected_capability": "native_plan",
            "delivery_mode": "human_handoff", "host": {"name": "sh"},
            "source": {}, "prompt": {}, "source_verified": "exact", "limitations": [],
            "classification": {}, "route_explanation": {}}),
        ),
        event("ask.confirmed", "2026-09-27T10:00:01Z", serde_json::json!({})),
    ];
    let lines: String = ledger.iter().map(|e| format!("{e}\n")).collect();
    std::fs::write(root.join(".fab7/rf/ledger.jsonl"), lines).expect("ledger");

    let socket = protocol::private_socket("weft-attest");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        if let Ok(s) = UnixStream::connect(&socket) {
            break s;
        }
        assert!(Instant::now() < deadline, "no daemon");
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut frames = Lines::new(stream.try_clone().expect("clone"));
    let mut id = 0;
    let mut call = |stream: &mut UnixStream, c: Call| {
        id += 1;
        stream.write_all(&Line::Call { id, call: c }.encode()).expect("write");
    };
    call(&mut stream, Call::Hello { client: "test".into(), protocol: PROTOCOL });
    call(&mut stream, Call::Open { rows: 24, cols: 80, path: root.to_string_lossy().into_owned() });
    call(&mut stream, Call::StartAgent { harness: "sh".into(), spec: "/bin/cat".into() });
    wait(&mut frames, &stream, |e| matches!(e, Event::Added { .. }).then_some(()));
    let send = |stream: &mut UnixStream,
                frames: &mut Lines<UnixStream>,
                call: &mut dyn FnMut(&mut UnixStream, Call),
                force: bool| {
        call(
            stream,
            Call::Act { act: "send".into(), unit: Some("ask_1".into()), pane: None, text: None },
        );
        let pending = wait(frames, stream, |e| match e {
            Event::Pending { id, .. } => Some(id),
            _ => None,
        });
        call(stream, Call::Resolve { pending, yes: true, force });
        wait(frames, stream, |e| match e {
            Event::Injected { refusal, .. } => Some(refusal),
            _ => None,
        })
    };

    // The agent never said it was ready, so Weft refuses: nothing is recorded.
    let refused = send(&mut stream, &mut frames, &mut call, false);
    assert!(refused.is_some(), "Weft asked instead of typing");
    assert!(
        !calls(&bin).contains("ask submitted"),
        "a refused send is not a submission:\n{}",
        calls(&bin)
    );

    // Told to type it anyway, Weft types it, and RingFrame records the person's submission.
    let typed = send(&mut stream, &mut frames, &mut call, true);
    assert_eq!(typed, None, "typed");
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !calls(&bin).contains("ask submitted") {
        std::thread::sleep(Duration::from_millis(50));
    }
    let log = calls(&bin);
    assert!(log.contains("ask submitted --ask ask_1"), "{log}");
    assert_eq!(log.matches("ask submitted").count(), 1, "once: {log}");

    call(&mut stream, Call::Shutdown);
    std::fs::remove_dir_all(&tmp).ok();
}
