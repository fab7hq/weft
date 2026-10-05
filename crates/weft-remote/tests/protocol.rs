//! The remote protocol (remote.md §4, §6): §14 test 5, and a `req` always
//! answered.

mod common;

use std::time::Duration;

use common::Harness;
use serde_json::json;

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
    let _ = shell.tui.views(Duration::from_millis(300));
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
    assert_eq!((tree[0]["project"].clone(), tree[0]["open"].clone()), (json!("work"), json!(1)));
    let sh = &tree[0]["harnesses"][0];
    assert_eq!(
        (sh["harness"].clone(), sh["agent"].clone(), sh["running"].clone()),
        (json!("sh"), agent.clone(), json!(true))
    );
    assert_eq!(sh["works"], json!([]), "nothing asked yet");

    let pane = agent.as_str().unwrap().rsplit_once('-').unwrap().1.parse().unwrap();
    shell.tui.call(weft_proto::Call::Input { pane, bytes: b"exit\r".to_vec() });
    let ended = d.next_of("tree");
    assert_eq!(ended["tree"][0]["open"], 0, "{ended}");
    assert_eq!(ended["tree"][0]["harnesses"], json!([]), "no agent and nothing open: no row");
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
