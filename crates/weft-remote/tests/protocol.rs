//! The remote protocol (remote.md §4, §6): §14 test 5, and a `req` always
//! answered.

mod common;

use std::time::Duration;

use common::Harness;
use serde_json::{Value, json};

#[test]
fn nothing_the_never_sent_list_names_reaches_the_daemon() {
    let mut h = Harness::start("never");
    let mut shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    let attempts = [
        json!({"type": "act", "agent": agent, "act": "send", "req": 1}),
        json!({"type": "act", "agent": agent, "act": "follow_up", "text": "x", "req": 2}),
        json!({"type": "act", "agent": agent, "act": "start", "req": 3}),
        json!({"type": "act", "agent": agent, "act": "confirm", "req": 4}),
        json!({"type": "resolve", "pending": "pnd_1", "yes": true, "force": true, "req": 5}),
        json!({"type": "read", "agent": agent, "what": "wording", "req": 6}),
        json!({"type": "start", "harness": "sh", "spec": "/bin/sh", "req": 7}),
        json!({"type": "agent.start", "harness": "sh", "req": 8}),
        json!({"type": "close", "agent": agent, "req": 9}),
        json!({"type": "stage", "agent": agent, "text": "rm -rf", "req": 10}),
        json!({"type": "setup", "req": 11}),
        json!({"type": "turbo", "on": true, "req": 12}),
        json!({"type": "shutdown", "req": 13}),
        json!({"type": "confirm", "unit": "ask_1", "req": 14}),
        // A resume names a session's id, never "the latest" (remote.md §6).
        json!({"type": "start", "project": "work", "harness": "sh", "resume": true, "req": 15}),
    ];
    for (n, attempt) in attempts.iter().enumerate() {
        d.send(attempt.clone());
        let answer = d.answer(n as i64 + 1);
        assert_eq!(answer["type"], "error", "{attempt}");
        assert!(
            answer["code"] == "not_allowed" || answer["code"] == "bad_request",
            "{attempt}: {answer}"
        );
    }
    // The daemon is as it was: one agent, still running, and nothing added.
    d.send(json!({"type": "ping", "t": 7}));
    assert_eq!(d.next_of("pong")["t"], 7);
    let _ = shell.app.views(Duration::from_millis(300));
    let (mut fresh, welcome) = h.allowed("d_2");
    let _ = &mut fresh;
    assert_eq!(welcome["agents"].as_array().map(Vec::len), Some(1), "no agent started or closed");
    assert_eq!(welcome["agents"][0]["turn"], "ready", "still running");
}

#[test]
fn the_tree_comes_in_welcome_and_again_when_an_agent_ends() {
    let mut h = Harness::start("tree");
    let mut shell = h.shell("work");
    let (mut first, _) = h.allowed("d_1");
    first.agents(1);
    // Once the project is open, a device is welcomed with its tree.
    let (mut d, welcome) = h.allowed("d_2");
    let agent = welcome["agents"][0]["agent"].clone();
    let tree = &welcome["tree"];
    assert_eq!(tree.as_array().map(Vec::len), Some(1), "{tree}");
    assert_eq!((tree[0]["project"].clone(), tree[0]["open"].clone()), (json!("work"), json!(0)));
    assert!(tree[0].get("setup").is_some(), "whether RingFrame is set up here: {tree}");
    // No session named yet: a row all the same, with no id, its agent the pane's.
    let row = &tree[0]["agents"]["rows"][0];
    assert_eq!(
        (row["session"].clone(), row["state"].clone(), row["agent"].clone()),
        (json!(""), json!("running"), agent.clone()),
        "{tree}"
    );
    assert!(tree[0].get("harnesses").is_none(), "no harness rows (remote.md §6)");

    // The shell's hook names its session: it is a row, its agent the pane's.
    let told = std::fs::read_to_string(shell.project.with_extension("pane")).unwrap();
    let (token, socket) = told.split_once('\n').unwrap();
    let sent = weft_core::signal::Sent {
        harness: "sh".into(),
        hook: "Hook".into(),
        session: "s1".into(),
        workspace: Some(shell.project.to_string_lossy().into_owned()),
        state: Some("ready".into()),
        pane: Some(0),
        token: Some(token.into()),
        ..Default::default()
    };
    let line = weft_proto::Line::Event(weft_proto::Event::Signal(Box::new(sent))).encode();
    use std::io::Write as _;
    std::os::unix::net::UnixStream::connect(socket.trim()).unwrap().write_all(&line).unwrap();
    let named = loop {
        let m = d.next_of("tree");
        if m["tree"][0]["agents"]["rows"][0]["session"] == json!("s1") {
            break m;
        }
    };
    let row = &named["tree"][0]["agents"]["rows"][0];
    assert_eq!(
        (row["session"].clone(), row["state"].clone(), row["agent"].clone()),
        (json!("s1"), json!("running"), agent.clone())
    );

    let pane = agent.as_str().unwrap().rsplit_once('-').unwrap().1.parse().unwrap();
    shell.app.call(weft_proto::Call::Input { pane, bytes: b"exit\r".to_vec() });
    let ended = loop {
        let m = d.next_of("tree");
        if m["tree"][0]["agents"]["rows"][0]["state"] == "ended" {
            break m;
        }
    };
    let a = &ended["tree"][0]["agents"];
    assert_eq!(
        (a["running"].clone(), a["ended"].clone(), a["rows"][0]["agent"].clone()),
        (json!(0), json!(1), Value::Null),
        "its row stays, ended: {ended}"
    );
}

#[test]
fn a_device_starts_only_a_harness_the_machine_offers() {
    let mut h = Harness::start("start");
    let _shell = h.shell("work");
    let (mut first, _) = h.allowed("d_1");
    first.agents(1);
    let (mut d, welcome) = h.allowed("d_2");
    // No harness file here is set up for RingFrame, so none can be started.
    assert_eq!(welcome["harnesses"], json!([]), "{welcome}");
    for (n, (start, code)) in [
        (json!({"type": "start", "project": "work", "harness": "sh", "req": 1}), "no_harness"),
        (json!({"type": "start", "project": "elsewhere", "harness": "sh", "req": 2}), "no_project"),
        (json!({"type": "start", "harness": "sh", "req": 3}), "bad_request"),
    ]
    .into_iter()
    .enumerate()
    {
        d.send(start.clone());
        let answer = d.answer(n as i64 + 1);
        assert_eq!(
            (answer["type"].clone(), answer["code"].clone()),
            (json!("error"), json!(code)),
            "{start}"
        );
    }
}

#[test]
fn every_req_gets_ok_or_error() {
    let mut h = Harness::start("reqs");
    let _shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    let requests = [
        (json!({"type": "keys", "agent": agent, "bytes": "MQ==", "req": 1}), "ok"),
        (json!({"type": "keys", "agent": agent, "bytes": "not base64!", "req": 2}), "error"),
        (json!({"type": "keys", "agent": "000000000000-9", "bytes": "MQ==", "req": 3}), "error"),
        (json!({"type": "text", "agent": agent, "text": "echo hi", "req": 4}), "ok"),
        (json!({"type": "text", "agent": agent, "text": "", "req": 5}), "error"),
        (json!({"type": "act", "agent": agent, "act": "eval", "req": 6}), "error"),
        (json!({"type": "act", "agent": agent, "act": "ask", "req": 7}), "error"),
        (json!({"type": "resolve", "pending": "pnd_404", "yes": false, "req": 8}), "error"),
        (json!({"type": "read", "agent": agent, "what": "history", "req": 9}), "ok"),
        (json!({"type": "read", "agent": agent, "what": "eval", "req": 10}), "error"),
        (json!({"type": "nonsense", "req": 11}), "error"),
        (json!({"type": "read", "what": "detail", "unit": "ask_404", "req": 12}), "error"),
        (json!({"type": "read", "what": "eval_view", "req": 13}), "error"),
        (json!({"type": "read", "what": "sessions", "project": "work", "q": "x", "req": 14}), "ok"),
        (json!({"type": "read", "what": "sealed", "project": "work", "req": 15}), "ok"),
        (
            json!({"type": "read", "what": "session", "project": "work", "session": "s_9", "req": 16}),
            "error",
        ),
        (
            json!({"type": "read", "what": "eval", "project": "work", "eval": "evl_9", "req": 17}),
            "error",
        ),
        (
            json!({"type": "read", "what": "seal", "project": "work", "seal": "sel_9", "req": 18}),
            "error",
        ),
        (
            json!({"type": "start", "project": "work", "harness": "sh", "resume": "s_9", "req": 19}),
            "error",
        ),
        (json!({"type": "act", "act": "eval", "project": "nowhere", "req": 20}), "error"),
    ];
    for (n, (request, want)) in requests.iter().enumerate() {
        d.send(request.clone());
        let answer = d.answer(n as i64 + 1);
        assert_eq!(answer["type"], *want, "{request}: {answer}");
    }
    // Exactly one answer each: nothing more for any of them.
    d.send(json!({"type": "ping", "t": 1}));
    loop {
        let m = d.next();
        assert!(m["type"] != "ok" && m["type"] != "error", "a second answer: {m}");
        if m["type"] == "pong" {
            break;
        }
    }
    // Without a req, a message the protocol does not allow ends the socket.
    d.send(json!({"type": "nonsense"}));
    assert!(d.closed_within(Duration::from_secs(1)));
}

#[test]
fn watching_sends_the_screen_whole_then_as_it_changes() {
    let mut h = Harness::start("watch");
    let _shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    d.send(json!({"type": "watch", "agent": agent}));
    let first = d.next_of("screen");
    assert_eq!(first["full"], true);
    assert_eq!(first["agent"], agent.as_str());
    assert_eq!((first["rows"].as_u64(), first["cols"].as_u64()), (Some(24), Some(80)));
    d.send(json!({"type": "keys", "agent": agent, "bytes": "ZWNobyB3YXRjaGVkLWZyb20tYS1waG9uZQ0=", "req": 1}));
    let mut seen = String::new();
    while !seen.contains("watched-from-a-phone") {
        let frame = d.next_of("screen");
        assert_eq!(frame["full"], false);
        use base64ct::Encoding;
        let bytes = base64ct::Base64::decode_vec(frame["bytes"].as_str().unwrap()).unwrap();
        seen.push_str(&String::from_utf8_lossy(&bytes));
    }
}

/// A device with four panes watches four agents: a whole screen of each, at
/// the agent's own size, whatever the device's panes are; a fifth is ignored.
/// `welcome` says `panes`.
#[test]
fn a_device_watches_four_agents_each_at_the_agents_size() {
    let mut h = Harness::start("watch-four");
    let _shells: Vec<_> = ["a", "b", "c", "d", "e"].iter().map(|n| h.shell(n)).collect();
    let (mut d, welcome) = h.allowed("d_1");
    assert_eq!((welcome["panes"].clone(), welcome["split"].clone()), (json!(4), json!(true)));
    let agents = d.agents(5);
    let id = |i: usize| agents[i]["agent"].as_str().unwrap().to_string();
    let also: Vec<Value> =
        (1..5).map(|i| json!({"agent": id(i), "rows": 20 + i, "cols": 40 + i})).collect();
    d.send(json!({"type": "watch", "agent": id(0), "rows": 20, "cols": 40, "also": also}));
    let mut whole = std::collections::HashMap::new();
    while whole.len() < 4 {
        let frame = d.next_of("screen");
        let agent = frame["agent"].as_str().unwrap().to_string();
        assert_ne!(agent, id(4), "a fifth agent is not watched");
        if frame["full"] == true {
            whole.insert(agent, (frame["rows"].clone(), frame["cols"].clone()));
        }
    }
    for i in 0..4 {
        assert_eq!(whole[&id(i)], (json!(24), json!(80)), "agent {i}");
    }
    // The fifth's output never comes; the first's does.
    d.send(json!({"type": "keys", "agent": id(4), "bytes": "ZWNobyBub3Qtd2F0Y2hlZA0=", "req": 1}));
    d.send(
        json!({"type": "keys", "agent": id(0), "bytes": "ZWNobyBzdGlsbC13YXRjaGVkDQ==", "req": 2}),
    );
    let mut seen = String::new();
    while !seen.contains("still-watched") {
        let frame = d.next_of("screen");
        assert_ne!(frame["agent"], id(4).as_str(), "a fifth agent is not watched");
        if frame["agent"] == id(0).as_str() {
            use base64ct::Encoding;
            let bytes = base64ct::Base64::decode_vec(frame["bytes"].as_str().unwrap()).unwrap();
            seen.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
}

/// A device may watch a second agent beside the first, each at the
/// agent's own size: both screens come, each whole first, and the second is let go when
/// the device watches one again. A machine says it can, in its welcome.
#[test]
fn a_device_watches_two_agents_side_by_side() {
    let mut h = Harness::start("watch-two");
    let _work = h.shell("work");
    let _more = h.shell("more");
    let (mut d, welcome) = h.allowed("d_1");
    assert_eq!(welcome["split"], true, "{welcome}");
    let agents = d.agents(2);
    let (a, b) = (agents[0]["agent"].as_str().unwrap(), agents[1]["agent"].as_str().unwrap());
    d.send(json!({"type": "watch", "agent": a, "rows": 30, "cols": 40,
                  "also": {"agent": b, "rows": 30, "cols": 41}}));
    let mut whole = std::collections::HashSet::new();
    while whole.len() < 2 {
        let frame = d.next_of("screen");
        if frame["full"] == true {
            whole.insert(frame["agent"].as_str().unwrap().to_string());
        }
    }
    assert!(whole.contains(a) && whole.contains(b), "{whole:?}");
    // One again: only the first agent's frames come.
    d.send(json!({"type": "watch", "agent": a}));
    d.send(json!({"type": "keys", "agent": b, "bytes": "ZWNobyBub3Qtd2F0Y2hlZA0=", "req": 1}));
    d.send(json!({"type": "keys", "agent": a, "bytes": "ZWNobyBzdGlsbC13YXRjaGVkDQ==", "req": 2}));
    let mut seen = String::new();
    while !seen.contains("still-watched") {
        let frame = d.next_of("screen");
        assert_eq!(frame["agent"], a, "the second agent was let go");
        use base64ct::Encoding;
        let bytes = base64ct::Base64::decode_vec(frame["bytes"].as_str().unwrap()).unwrap();
        seen.push_str(&String::from_utf8_lossy(&bytes));
    }
}

/// The machine's settings, read and changed from a device (weft-after-desktop
/// ADR-0003 D6): every device is told what changed, Turbo included.
#[test]
fn a_device_reads_and_changes_the_machine_s_settings() {
    let mut h = Harness::start("settings");
    let _shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    d.agents(1);
    let (mut other, _) = h.allowed("d_2");
    other.agents(1);
    d.send(json!({"type": "settings", "project": "work", "req": 1}));
    let got = d.answer(1);
    assert_eq!(got["type"], "ok", "{got}");
    assert_eq!(got["settings"]["effective"]["turbo"], false);
    let set = json!({"type": "set", "project": "work", "scope": "machine", "key": ["turbo"],
                     "value": true, "req": 2});
    d.send(set);
    let changed = d.answer(2);
    assert_eq!(changed["settings"]["effective"]["turbo"], true, "{changed}");
    let told = other.next_of("settings");
    assert_eq!(told["project"], "work");
    assert_eq!(told["settings"]["machine"]["turbo"], true);
    d.send(json!({"type": "set", "project": "work", "scope": "machine", "key": ["turbo"],
                  "value": "yes", "req": 3}));
    assert_eq!(d.answer(3)["type"], "error", "what Weft would not read is refused");
}
