//! The remote protocol (remote.md §6): the machine's
//! `update`, a project reopened after an unclean end and dismissed, and the
//! refusals a device acts on.

mod common;

use std::time::{Duration, Instant};

use common::{Device, Harness, good};
use serde_json::json;
use weft_proto::Call;

/// The loopback port the harness's fake tunnel was pointed at.
fn port(h: &Harness) -> u16 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let args = std::fs::read_to_string(h.dir.join("cloudflared.args")).unwrap_or_default();
        if let Some(Ok(port)) = args.trim().rsplit(':').next().map(str::parse) {
            return port;
        }
        assert!(Instant::now() < deadline, "the tunnel never started");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A machine not signed in to Weft's service refuses a pair as such, which
/// only the person at the machine can fix.
#[test]
fn a_machine_that_is_signed_out_says_so() {
    let h = Harness::until(
        "signed-out",
        |dir| std::fs::remove_file(dir.join("remote/machine.toml")).unwrap(),
        "signed_out",
    );
    let mut d = Device::connect(port(&h));
    d.send(json!({"type": "pair", "v": 1, "device": "d_1", "name": "iPhone", "assertion": good("d_1")}));
    assert_eq!(d.next(), json!({"type": "refused", "code": "signed_out"}));
}

/// An update installed while agents run is told to every device, and a
/// device that says hello later is welcomed with it.
#[test]
fn an_update_waiting_is_told_to_devices() {
    let mut h = Harness::with("update", |dir| {
        std::fs::create_dir_all(dir.join("versions/99.0.0")).unwrap();
        std::fs::write(dir.join("versions/99.0.0/weft"), "#!/bin/sh\n").unwrap();
    });
    let (mut d, welcome) = h.allowed("d_1");
    assert_eq!(welcome["update"], serde_json::Value::Null, "nothing waits");
    // An agent runs, so the update waits for it.
    let _shell = h.shell("busy");
    h.tui.call(Call::Update { version: "99.0.0".into() });
    assert_eq!(d.next_of("update"), json!({"type": "update", "update": {"version": "99.0.0"}}));
    let (_, later) = h.allowed("d_2");
    assert_eq!(later["update"], json!({"version": "99.0.0"}));
}

/// A project weftd reopened after it ended uncleanly is in the tree with
/// when, and its agents ended; Dismiss takes it away.
#[test]
fn a_reopened_project_is_in_the_tree_until_it_is_dismissed() {
    let mut h = Harness::with("reopened", |dir| {
        let project = dir.join("left-open");
        std::fs::create_dir_all(&project).unwrap();
        let list = weft_core::projects::write(&[weft_core::projects::Opened {
            path: project.canonicalize().unwrap(),
            opened: 1,
        }]);
        std::fs::write(dir.join("projects.json"), list).unwrap();
    });
    let (mut d, welcome) = h.allowed("d_1");
    let tree = if welcome["tree"].as_array().is_some_and(|t| !t.is_empty()) {
        welcome["tree"].clone()
    } else {
        d.next_of("tree")["tree"].clone()
    };
    let row = tree.as_array().unwrap().iter().find(|p| p["project"] == "left-open").cloned();
    let row = row.expect("the reopened project is listed");
    assert!(row["reopened"].as_i64().is_some_and(|t| t > 0), "{row}");
    assert_eq!(row["agents"]["running"], 0, "{row}");

    d.send(json!({"type": "act", "act": "dismiss", "project": "left-open", "req": 1}));
    let answer = d.answer(1);
    assert_eq!(answer["type"], "ok", "{answer}");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let tree = d.next_of("tree")["tree"].clone();
        if tree.as_array().is_some_and(|t| t.iter().all(|p| p["project"] != "left-open")) {
            break;
        }
        assert!(Instant::now() < deadline, "still listed: {tree}");
    }
}
