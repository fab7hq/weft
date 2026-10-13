//! Control, size and messages (remote.md §9, §4): §14 test 7, and presence
//! handing the keys back to the Mac.

mod common;

use std::time::{Duration, Instant};

use base64ct::Encoding;
use common::Harness;
use serde_json::json;
use weft_proto::Call;

fn keys(text: &str) -> String {
    base64ct::Base64::encode_string(text.as_bytes())
}

/// §14 test 7: a `text` message is the bracketed paste, then Enter in a
/// write of its own.
#[test]
fn a_message_is_a_bracketed_paste_then_a_separate_enter() {
    let mut h = Harness::start("text");
    let shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    let log = shell.project.join("reads.log");
    // Each read the program gets, with when, as hex.
    let recorder = format!(
        "exec python3 -u -c \"import os,sys,time,tty; tty.setraw(0); f=open('{}','w')\n\
         while True:\n b=os.read(0,4096)\n f.write('%.3f %s\\\\n'%(time.time(),b.hex())); f.flush()\"\r",
        log.display()
    );
    d.send(json!({"type": "keys", "agent": agent, "bytes": keys(&recorder), "req": 1}));
    assert_eq!(d.answer(1)["type"], "ok");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !log.exists() {
        assert!(Instant::now() < deadline, "the recorder never started");
        std::thread::sleep(Duration::from_millis(50));
    }

    d.send(json!({"type": "text", "agent": agent, "text": "ship it", "req": 2}));
    assert_eq!(d.answer(2)["type"], "ok");
    let reads = loop {
        let text = std::fs::read_to_string(&log).unwrap();
        let reads: Vec<(f64, String)> = text
            .lines()
            .map(|l| {
                let (t, hex) = l.split_once(' ').unwrap();
                (t.parse().unwrap(), hex.to_string())
            })
            .collect();
        if reads.iter().any(|(_, h)| h == "0d") {
            break reads;
        }
        assert!(Instant::now() < deadline, "no Enter: {reads:?}");
        std::thread::sleep(Duration::from_millis(50));
    };
    let paste = format!("{}{}{}", hex(b"\x1b[200~"), hex(b"ship it"), hex(b"\x1b[201~"));
    let all: String = reads.iter().map(|(_, h)| h.as_str()).collect();
    assert_eq!(all, format!("{paste}0d"), "the paste, then Enter, and nothing else");
    let enter = reads.iter().find(|(_, h)| h == "0d").unwrap();
    let pasted = reads.iter().find(|(_, h)| h.ends_with(&hex(b"\x1b[201~"))).unwrap();
    assert!(enter.0 - pasted.0 >= 0.05, "Enter came on its own, {:.3} s later", enter.0 - pasted.0);
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The device that types has the keys; the person typing on the Mac takes
/// them back (§9).
#[test]
fn typing_on_the_mac_takes_the_keys_back() {
    let mut h = Harness::start("control");
    let mut shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    d.send(json!({"type": "size", "agent": agent, "rows": 30, "cols": 40}));
    d.send(json!({"type": "keys", "agent": agent, "bytes": keys("true\r"), "req": 1}));
    assert_eq!(d.next_of("control"), json!({"type": "control", "agent": agent, "by": "you"}));

    // The TUI's typing, as presence.
    shell.tui.call(Call::Input { pane: 0, bytes: b"true\r".to_vec() });
    assert_eq!(d.next_of("control"), json!({"type": "control", "agent": agent, "by": "mac"}));
    // Typing from the device again takes them again.
    d.send(json!({"type": "keys", "agent": agent, "bytes": keys("true\r"), "req": 2}));
    assert_eq!(d.next_of("control")["by"], "you");
}

/// What arrives up to the first whole screen of `agent`: the messages' types,
/// and that screen.
fn up_to_a_screen(d: &mut common::Device, agent: &str) -> (Vec<String>, serde_json::Value) {
    let mut seen = Vec::new();
    loop {
        let m = d.next();
        if m["type"] == "screen" && m["agent"] == agent && m["full"] == true {
            return (seen, m);
        }
        seen.push(m["type"].as_str().unwrap_or_default().to_string());
    }
}

/// Weft owns an agent's size (remote.md §9): opening it while nobody has used
/// the Mac for a while keeps its size and gives the device no keys.
#[test]
fn opening_an_agent_while_the_mac_is_away_keeps_its_size() {
    let mut h = Harness::start("watch-away");
    let _shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    d.send(json!({"type": "watch", "agent": agent, "rows": 30, "cols": 40}));
    let (seen, screen) = up_to_a_screen(&mut d, &agent);
    assert!(!seen.contains(&"control".to_string()), "{seen:?}");
    assert_eq!((screen["rows"].clone(), screen["cols"].clone()), (json!(24), json!(80)));
}

/// Typing from a device, after the size an older app sends first: the keys
/// are the device's, and the agent keeps its size.
#[test]
fn typing_from_a_device_keeps_the_agents_size() {
    let mut h = Harness::start("typing-size");
    let _shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    d.send(json!({"type": "watch", "agent": agent}));
    assert_eq!(d.next_of("screen")["cols"], 80);
    d.send(json!({"type": "size", "agent": agent, "rows": 30, "cols": 40}));
    d.send(json!({"type": "keys", "agent": agent, "bytes": keys("true\r"), "req": 1}));
    assert_eq!(d.next_of("control")["by"], "you");
    // The shell's echo of what was typed, at the agent's own size.
    let m = d.next_of("screen");
    assert_eq!((m["rows"].clone(), m["cols"].clone()), (json!(24), json!(80)), "{m}");
}

/// While someone is using the Mac, watching keeps the Mac's size and keys.
#[test]
fn opening_an_agent_while_the_mac_is_in_use_keeps_its_size() {
    let mut h = Harness::start("watch-here");
    let mut shell = h.shell("work");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    // The TUI's typing, as presence.
    shell.tui.call(Call::Input { pane: 0, bytes: b"true\r".to_vec() });
    std::thread::sleep(Duration::from_millis(300));
    d.send(json!({"type": "watch", "agent": agent, "rows": 30, "cols": 40}));
    let (seen, screen) = up_to_a_screen(&mut d, &agent);
    assert!(!seen.contains(&"control".to_string()), "{seen:?}");
    assert_eq!((screen["rows"].clone(), screen["cols"].clone()), (json!(24), json!(80)));
}
