//! `[E]VAL` gathers the Eval in Weft, through `ringframe eval open`, then
//! offers only the debate to a harness; an Eval already gathered is debated
//! without opening again.
//!
//! Its own test binary because it puts a stand-in `ringframe` first on PATH,
//! which is global: one test, one process.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;

/// A `ringframe` that opens an Eval as RingFrame does, recording it opened and
/// gathered, answers everything else, and logs every call it gets.
fn stand_in(dir: &Path, ledger: &Path) {
    let log = dir.join("calls.log");
    let opened = r#"{"schema":"ringframe.ledger/1","event_id":"evt_o","type":"eval.opened","time":"2026-09-28T10:00:00Z","id":"evl_new","actor":{"kind":"human","id":"me"},"links":[],"data":{"basis":{"asks":["ask_1"]}}}"#;
    let gathered = r#"{"schema":"ringframe.ledger/1","event_id":"evt_g","type":"eval.gathered","time":"2026-09-28T10:00:01Z","id":"evl_new","actor":{"kind":"human","id":"me"},"links":[],"data":{"eval_id":"evl_new","host":"weft"}}"#;
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\ncase \"$*\" in\n  *\"eval open\"*) printf '%s\\n%s\\n' '{opened}' '{gathered}' >> '{ledger}'; printf '{{\"eval_id\":\"evl_new\"}}' ;;\n  *\"profile show\"*) printf '{{\"invocation_prefix\":\"/rf:\"}}' ;;\n  *\"profile list\"*) printf '{{\"profiles\":[]}}' ;;\n  *) printf '{{}}' ;;\nesac\nexit 0\n",
        log = log.display(),
        ledger = ledger.display(),
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
        match frames.next() {
            Ok(Some(Line::Event(e))) => {
                if let Some(found) = pick(e) {
                    return found;
                }
            }
            Ok(Some(Line::Error { code, message, .. })) => {
                eprintln!("answered no: {code} {message}")
            }
            _ => {}
        }
    }
    panic!("nothing arrived");
}

#[test]
fn one_press_gathers_in_weft_and_offers_the_debate_and_the_next_does_not_gather_again() {
    let tmp = std::env::temp_dir().join(format!("weft-gathers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let bin = tmp.join("bin");
    let root = tmp.join("project");
    std::fs::create_dir_all(&bin).expect("bin");
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

    let socket = protocol::private_socket("weft-gathers");
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
    call(
        &mut stream,
        Call::StartAgent { harness: "sh".into(), spec: "/bin/cat".into(), session: None },
    );
    wait(&mut frames, &stream, |e| matches!(e, Event::Added { .. }).then_some(()));
    // The agent says it is ready, as its hook would: a free agent to debate in.
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("time");
    let receipt = root.join(".fab7/rf/sessions/sh/s1");
    std::fs::create_dir_all(&receipt).expect("session dir");
    let line = serde_json::json!({"event": "ready", "session_id": "s1",
                                  "time": weft::turns::stamp(ms.as_millis() as i64)});
    std::fs::write(receipt.join("turns.jsonl"), format!("{line}\n")).expect("receipt");
    wait(&mut frames, &stream, |e| match e {
        Event::Agents { panes, .. }
            if panes.first().copied().flatten() == Some(weft::turns::Turn::Ready) =>
        {
            Some(())
        }
        _ => None,
    });
    let check =
        || Call::Act { act: "check".into(), unit: Some("ask_1".into()), pane: None, text: None };
    let pending = |frames: &mut Lines<UnixStream>, stream: &UnixStream| {
        wait(frames, stream, |e| match e {
            Event::Pending { payload, .. } => Some(String::from_utf8_lossy(&payload).to_string()),
            _ => None,
        })
    };

    // One press: Weft opens and gathers, then offers the debate.
    let started = Instant::now();
    call(&mut stream, check());
    let typed = pending(&mut frames, &stream);
    let took = started.elapsed();
    let log = calls(&bin);
    assert!(log.contains("eval open --host weft"), "Weft gathered through the CLI:\n{log}");
    assert!(typed.contains("eval debate evl_new"), "only the debate goes to the harness: {typed}");
    assert!(!typed.contains("gather"), "{typed}");
    assert!(took < Duration::from_secs(3), "the gather held the press for {took:?}");

    // The board has seen the gathered Eval: the next press debates it and
    // opens nothing.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline
        && !std::fs::read_to_string(&ledger).unwrap_or_default().contains("eval.gathered")
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(600));
    call(&mut stream, check());
    let again = pending(&mut frames, &stream);
    assert!(again.contains("eval debate evl_new"), "{again}");
    assert_eq!(calls(&bin).matches("eval open").count(), 1, "gathered once:\n{}", calls(&bin));

    call(&mut stream, Call::Shutdown);
    std::fs::remove_dir_all(&tmp).ok();
}
