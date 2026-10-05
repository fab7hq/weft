//! Allowing a device (remote.md §8): §14 tests 3 and 4.

mod common;

use std::time::Duration;

use common::{Harness, assertion, fab7, good, now};
use serde_json::json;

/// §14 test 3: a bad signature, an expired assertion, another user, another
/// machine, and a fourth request in ten minutes are each refused, and none
/// reaches the view.
#[test]
fn a_stranger_or_a_flood_is_refused_without_asking_the_person() {
    let mut h = Harness::start("refused");
    let other = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let fields = |sub: u64, machine: &str, exp: i64| json!({"iss": "fab7", "sub": sub, "login": "octo", "machine": machine, "device": "d_x", "exp": exp});
    let bad = [
        ("a bad signature", assertion(&other, fields(42, "m_1", now() + 60))),
        ("an expired one", assertion(&fab7(), fields(42, "m_1", now() - 1))),
        ("another user", assertion(&fab7(), fields(43, "m_1", now() + 60))),
        ("another machine", assertion(&fab7(), fields(42, "m_2", now() + 60))),
    ];
    for (what, a) in bad {
        let mut d = h.device();
        d.send(json!({"type": "pair", "v": 2, "device": "d_x", "name": "Phone", "assertion": a}));
        assert_eq!(d.next(), json!({"type": "refused", "code": "not_owner"}), "{what}");
        assert!(d.closed_within(Duration::from_secs(2)), "{what}: closed");
    }
    // Three real requests are asked, and refused by the person.
    for n in 0..3 {
        let id = format!("d_{n}");
        let mut d = h.device();
        d.send(
            json!({"type": "pair", "v": 2, "device": id, "name": "Phone", "assertion": good(&id)}),
        );
        assert_eq!(d.next()["type"], "asking");
        h.tui.wait_view(|v| v["asking"]["device"] == id.as_str());
        h.tui.act(json!({"act": "allow", "device": id, "yes": false}));
        assert_eq!(d.next(), json!({"type": "refused", "code": "denied"}));
    }
    let views = h.tui.views(Duration::from_millis(500));
    let mut d = h.device();
    d.send(
        json!({"type": "pair", "v": 2, "device": "d_3", "name": "Phone", "assertion": good("d_3")}),
    );
    assert_eq!(d.next(), json!({"type": "refused", "code": "busy"}), "a fourth in ten minutes");
    let views: Vec<_> = views.into_iter().chain(h.tui.views(Duration::from_millis(800))).collect();
    let asked: Vec<_> = views.iter().filter_map(|v| v["asking"]["device"].as_str()).collect();
    assert!(!asked.iter().any(|d| *d == "d_x" || *d == "d_3"), "{asked:?}");
}

/// §14 test 4: a removed device's open WebSocket is closed within a second.
#[test]
fn a_removed_device_is_closed_at_once() {
    let mut h = Harness::start("removed");
    let (mut phone, welcome) = h.allowed("d_phone");
    assert_eq!(welcome["machine"], json!({"id": "m_1", "name": "studio"}));
    let view = h.tui.wait_view(|v| v["devices"].as_array().is_some_and(|d| d.len() == 1));
    assert_eq!(view["devices"][0]["name"], "iPhone");
    assert_eq!(view["devices"][0]["login"], "octo");

    h.tui.act(json!({"act": "remove", "device": "d_phone"}));
    assert!(phone.closed_within(Duration::from_secs(1)), "closed within a second");
    h.tui.wait_view(|v| v["devices"] == json!([]));

    // And it cannot come back with the token it had.
    let devices = std::fs::read_to_string(h.dir.join("remote/devices.toml")).unwrap();
    assert!(!devices.contains("d_phone"));
}

#[test]
fn a_device_that_is_not_allowed_is_refused_at_hello() {
    let h = Harness::start("unknown");
    let mut d = h.device();
    d.send(json!({"type": "hello", "v": 2, "device": "d_nobody", "token": "guess"}));
    assert_eq!(d.next(), json!({"type": "refused", "code": "unknown_device"}));
    assert!(d.closed_within(Duration::from_secs(1)));
    let mut d = h.device();
    d.send(json!({"type": "watch", "agent": null}));
    assert!(d.closed_within(Duration::from_secs(1)), "nothing before hello");
}

/// A page that speaks another version of the protocol is told so, at hello
/// and at pair, and the person is never asked.
#[test]
fn another_version_is_refused_as_such() {
    let mut h = Harness::start("version");
    let mut d = h.device();
    d.send(json!({"type": "hello", "v": 1, "device": "d_old", "token": "t"}));
    assert_eq!(d.next(), json!({"type": "refused", "code": "version"}));
    let views = h.tui.views(Duration::from_millis(300));
    let mut d = h.device();
    d.send(json!({"type": "pair", "v": 1, "device": "d_old", "name": "Phone", "assertion": good("d_old")}));
    assert_eq!(d.next(), json!({"type": "refused", "code": "version"}));
    let views: Vec<_> = views.into_iter().chain(h.tui.views(Duration::from_millis(500))).collect();
    assert!(views.iter().all(|v| v["asking"].is_null()), "{views:?}");
}
