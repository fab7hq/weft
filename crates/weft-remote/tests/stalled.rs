//! remote.md §14 test 2, end to end: a device that stops reading never delays
//! another device's frames, and gets a whole screen when it reads again (§5).

mod common;

use std::time::{Duration, Instant};

use common::Harness;
use serde_json::json;

#[test]
fn a_stalled_device_delays_no_one_and_starts_again_from_a_whole_screen() {
    let mut h = Harness::start("stalled");
    let _shell = h.shell("busy");
    let (mut a, _) = h.allowed("d_a");
    let agent = a.agents(1)[0]["agent"].as_str().unwrap().to_string();
    let (mut b, _) = h.allowed("d_b");
    for d in [&mut a, &mut b] {
        d.send(json!({"type": "watch", "agent": agent}));
        assert_eq!(d.next_of("screen")["full"], true);
    }
    // A big screen that changes all the time.
    a.send(json!({"type": "size", "agent": agent, "rows": 100, "cols": 300}));
    // The resize is a whole screen for both; b takes it before it stalls, so
    // any whole screen it gets later is the one a stall earns.
    while b.next_of("screen")["rows"] != 100 {}
    let busy = "while true; do head -c 30000 /dev/urandom | base64; done\r";
    use base64ct::Encoding;
    a.send(json!({"type": "keys", "agent": agent, "bytes": base64ct::Base64::encode_string(busy.as_bytes()), "req": 1}));

    // b stops reading; a's frames keep coming, never more than a second apart.
    let started = Instant::now();
    let mut last = Instant::now();
    let mut slowest = Duration::ZERO;
    let mut frames = 0;
    while started.elapsed() < Duration::from_secs(14) {
        let m = a.next();
        if m["type"] == "screen" {
            slowest = slowest.max(last.elapsed());
            last = Instant::now();
            frames += 1;
        }
    }
    assert!(frames > 50, "a got {frames} frames");
    assert!(slowest < Duration::from_secs(1), "a waited {slowest:?} for a frame");

    // b reads again: what was queued, then a whole screen after the
    // differences it could not keep up with.
    let resumed = Instant::now();
    let (mut b_frames, mut diffs) = (0, 0);
    let full = loop {
        assert!(resumed.elapsed() < Duration::from_secs(30), "no whole screen for b");
        let m = b.next();
        if m["type"] != "screen" {
            continue;
        }
        b_frames += 1;
        if m["full"] == true && diffs > 0 {
            break m;
        }
        if m["full"] == false {
            diffs += 1;
        }
    };
    assert!(b_frames < frames, "b's frames were dropped: {b_frames} of {frames}");
    assert_eq!((full["rows"].as_u64(), full["cols"].as_u64()), (Some(100), Some(300)));
    a.send(json!({"type": "keys", "agent": agent, "bytes": "Aw==", "req": 2}));
}
