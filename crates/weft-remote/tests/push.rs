//! Push (remote.md §10): §14 test 6, against a recording push endpoint.

mod common;

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use base64ct::{Base64UrlUnpadded, Encoding};
use common::{Fake, Harness};
use serde_json::{Value, json};
use web_push_native::p256;
use weft_proto::Call;

/// The agent's hook says it is in this state now, as `weft signal` would
/// send it from inside its pane.
fn agent_says(project: &Path, event: &str) {
    let told = project.with_extension("pane");
    while !told.exists() {
        std::thread::sleep(Duration::from_millis(10));
    }
    let text = std::fs::read_to_string(told).unwrap();
    let (token, socket) = text.split_once('\n').unwrap();
    let sent = weft_core::signal::Sent {
        harness: "sh".into(),
        hook: "Hook".into(),
        session: "s1".into(),
        workspace: Some(project.to_string_lossy().into_owned()),
        state: Some(event.into()),
        pane: Some(0),
        token: Some(token.into()),
        ..Default::default()
    };
    let line = weft_proto::Line::Event(weft_proto::Event::Signal(Box::new(sent))).encode();
    std::os::unix::net::UnixStream::connect(socket.trim()).unwrap().write_all(&line).unwrap();
    std::thread::sleep(Duration::from_millis(20));
}

/// Agents lists until this agent's turn is `turn`.
fn turn_is(d: &mut common::Device, turn: &str) {
    while d.next_of("agents")["agents"][0]["turn"] != turn {}
}

#[test]
fn a_push_goes_when_an_agent_waits_for_a_person_who_is_away() {
    let mut h = Harness::start("push");
    let mut shell = h.shell("work");
    let (mut d, welcome) = h.allowed("d_1");
    let vapid = welcome["vapid"].as_str().unwrap().to_string();
    assert_eq!(Base64UrlUnpadded::decode_vec(&vapid).unwrap().len(), 65, "a P-256 point");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();

    // The browser's side of the subscription.
    let browser = p256::SecretKey::from_slice(&[3; 32]).unwrap();
    let public = browser.public_key().to_sec1_bytes();
    let auth = [5u8; 16];
    let endpoint = Fake::start();
    endpoint.route("POST", "/push/d_1", &[(201, "")]);
    d.send(json!({"type": "push", "subscription": {
        "endpoint": format!("{}/push/d_1", endpoint.url),
        "keys": {"p256dh": Base64UrlUnpadded::encode_string(&public), "auth": Base64UrlUnpadded::encode_string(&auth)}
    }}));
    let pushes = || endpoint.requests("POST", "/push/d_1");
    let wait_for = |n: usize| {
        for _ in 0..60 {
            if pushes().len() >= n {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("{} pushes, not {n}", pushes().len());
    };

    // Waiting, and nobody has typed on the Mac: sent.
    agent_says(&shell.project, "ready");
    // The daemon binds the session to the pane when it reads the ready.
    std::thread::sleep(Duration::from_millis(600));
    agent_says(&shell.project, "waiting");
    wait_for(1);
    let push = &pushes()[0];
    let plain = web_push_native::decrypt(push.body.clone(), &browser, &auth.into()).unwrap();
    let payload: Value = serde_json::from_slice(&plain).unwrap();
    assert_eq!(
        payload,
        json!({"agent": agent, "title": "sh in work", "body": "needs your input", "count": 1})
    );
    assert_eq!(push.headers["ttl"], "3600");
    assert_eq!(push.headers["urgency"], "high");
    assert_eq!(push.headers["topic"], agent.as_str());
    assert_eq!(push.headers["content-encoding"], "aes128gcm");
    let authorization = &push.headers["authorization"];
    assert!(
        authorization.starts_with("vapid t=") && authorization.ends_with(&format!(", k={vapid}"))
    );

    // Once per change.
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(pushes().len(), 1);

    // Not while a device has the agent in front of the person.
    d.send(json!({"type": "seen", "foreground": true, "agent": agent}));
    std::thread::sleep(Duration::from_millis(200));
    agent_says(&shell.project, "working");
    turn_is(&mut d, "working");
    agent_says(&shell.project, "waiting");
    turn_is(&mut d, "waiting");
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(pushes().len(), 1, "it was on the person's screen");

    // Held while the person is at the Mac.
    d.send(json!({"type": "seen", "foreground": false, "agent": null}));
    shell.app.call(Call::Input { pane: 0, bytes: b"true\r".to_vec() });
    std::thread::sleep(Duration::from_millis(300));
    agent_says(&shell.project, "working");
    turn_is(&mut d, "working");
    agent_says(&shell.project, "waiting");
    turn_is(&mut d, "waiting");
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(pushes().len(), 1, "the person typed on the Mac a moment ago");
    // The JWT is one per push service, not one per push.
    let _ = &h;
}

#[test]
fn a_subscription_the_push_service_says_is_gone_is_dropped() {
    let mut h = Harness::start("push-gone");
    let shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    d.agents(1);
    let browser = p256::SecretKey::from_slice(&[3; 32]).unwrap();
    let endpoint = Fake::start();
    endpoint.route("POST", "/push/d_1", &[(410, "")]);
    d.send(json!({"type": "push", "subscription": {
        "endpoint": format!("{}/push/d_1", endpoint.url),
        "keys": {"p256dh": Base64UrlUnpadded::encode_string(&browser.public_key().to_sec1_bytes()),
                 "auth": Base64UrlUnpadded::encode_string(&[5u8; 16])}
    }}));
    std::thread::sleep(Duration::from_millis(200));
    let devices = || std::fs::read_to_string(h.dir.join("remote/devices.toml")).unwrap();
    assert!(devices().contains("push"), "kept with the device");
    agent_says(&shell.project, "ready");
    // The daemon binds the session to the pane when it reads the ready.
    std::thread::sleep(Duration::from_millis(600));
    agent_says(&shell.project, "waiting");
    for _ in 0..50 {
        if !devices().contains("push") {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("the subscription was kept: {}", devices());
}
