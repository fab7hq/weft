//! Eval is one dialog, and nothing is opened before its Send: then Weft opens
//! and gathers the Eval through RingFrame's crate, starts its judges, and
//! types nothing into the agent's pane. An Eval already gathered is judged
//! without opening again, and a Cancel records nothing.
//!
//! Its own test binary because it sets `HOME`, which is global: one test, one
//! process.

mod support;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;

/// The ledger's events of `kind`.
fn events(ledger: &Path, kind: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(ledger)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["type"] == kind)
        .collect()
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
fn an_eval_is_one_dialog_and_is_opened_by_its_send_once() {
    let tmp = std::env::temp_dir().join(format!("weft-gathers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (root, home) = (tmp.join("project"), tmp.join("home"));
    // SAFETY: this binary holds exactly one test.
    unsafe {
        std::env::set_var("HOME", &home);
    }
    let ask = support::real_ask(&root, &home, "sh");
    let ledger = root.join(".fab7/rf/ledger.jsonl");

    let socket = protocol::private_socket("weft-gathers");
    let agent = support::agent(&support::agents_dir(&socket));
    let harnesses = support::stub_harness(&socket.with_extension("judges"), "sh");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let config = listening.with_extension("no-config.toml");
        let _ = server::Session::serve_with(&listening, &config, Some(harnesses));
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
    call(
        &mut stream,
        Call::Open { rows: 24, cols: 80, path: root.to_string_lossy().into_owned(), raw: true },
    );
    // A window waits for its project's answer, as Weft's own do: call 2.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        stream.set_read_timeout(Some(Duration::from_millis(100))).expect("timeout");
        if let Ok(Some(Line::Result { id: 2, .. })) = frames.next() {
            break;
        }
        assert!(Instant::now() < deadline, "the project was never opened");
    }
    call(
        &mut stream,
        Call::StartAgent { harness: "sh".into(), spec: agent.clone(), session: None },
    );
    wait(&mut frames, &stream, |e| matches!(e, Event::Added { .. }).then_some(()));
    // The agent says it is ready, as its hook would: one nothing is typed into.
    support::pane_says(&socket, &support::agents_dir(&socket), &root, "sh", 0, "s1", "ready");
    wait(&mut frames, &stream, |e| match e {
        Event::Agents { panes, .. }
            if panes.first().copied().flatten() == Some(weft::turns::Turn::Ready) =>
        {
            Some(())
        }
        _ => None,
    });
    let check = || Call::Act {
        act: "check".into(),
        unit: Some(ask.clone()),
        pane: None,
        text: None,
        eval: None,
        start: None,
        draft: None,
    };
    let pending = |frames: &mut Lines<UnixStream>, stream: &UnixStream| {
        wait(frames, stream, |e| match e {
            Event::Pending { id, what, why, .. } => Some((id, what, why)),
            _ => None,
        })
    };
    let judge = |frames: &mut Lines<UnixStream>, stream: &UnixStream| {
        wait(frames, stream, |e| match e {
            Event::Added { pane, .. } if pane != 0 => Some(pane),
            _ => None,
        })
    };

    // One press, one dialog: it names the work and who checks it, and
    // nothing is opened yet.
    call(&mut stream, check());
    let (pnd, what, why) = pending(&mut frames, &stream);
    assert_eq!(what, "Eval this work?");
    assert!(why.contains("Ship it.") && why.contains("Stub"), "the work and its judge: {why}");
    assert!(why.contains("Nothing is typed into the agent's pane."), "{why}");
    assert!(events(&ledger, "eval.opened").is_empty(), "nothing opened before Send");

    // Cancel records nothing.
    call(
        &mut stream,
        Call::Resolve { pending: pnd, yes: false, force: false, to: None, start: None },
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(events(&ledger, "eval.opened").is_empty(), "a Cancel opens nothing");

    // Eval: Weft opens and gathers through the crate, then starts a judge.
    call(&mut stream, check());
    let (pnd, _, _) = pending(&mut frames, &stream);
    call(
        &mut stream,
        Call::Resolve { pending: pnd, yes: true, force: false, to: None, start: None },
    );
    let first = judge(&mut frames, &stream);
    let opened = events(&ledger, "eval.opened");
    assert_eq!(opened.len(), 1, "{opened:?}");
    let eval = opened[0]["id"].as_str().expect("an Eval id").to_string();
    let gathered = events(&ledger, "eval.gathered");
    assert_eq!(gathered[0]["data"]["host"], "weft", "{gathered:?}");
    let mut history = |stream: &mut UnixStream, frames: &mut Lines<UnixStream>| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            call(
                stream,
                Call::Read {
                    what: "history".into(),
                    unit: String::new(),
                    pane: Some(0),
                    lines: Some(50),
                    before: None,
                    after: None,
                },
            );
            std::thread::sleep(Duration::from_millis(100));
            let mut text = String::new();
            stream.set_read_timeout(Some(Duration::from_millis(200))).expect("timeout");
            while let Ok(Some(line)) = frames.next() {
                if let Line::Result { result, .. } = line
                    && let Some(t) = result["text"].as_str()
                {
                    text = t.to_string();
                    break;
                }
            }
            if !text.is_empty() || Instant::now() >= deadline {
                return text;
            }
        }
    };
    let screen = history(&mut stream, &mut frames);
    assert!(!screen.contains(&eval) && !screen.contains("/rf:"), "nothing is typed: {screen}");
    assert!(first > 0, "the judge is a pane of its own");

    // The board has seen the gathered Eval: the next Eval is the same one,
    // and opens nothing.
    std::thread::sleep(Duration::from_millis(600));
    call(&mut stream, check());
    let (pnd, _, _) = pending(&mut frames, &stream);
    call(
        &mut stream,
        Call::Resolve { pending: pnd, yes: true, force: false, to: None, start: None },
    );
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(events(&ledger, "eval.opened").len(), 1, "gathered once");

    call(&mut stream, Call::Shutdown);
    std::fs::remove_dir_all(&tmp).ok();
}
