//! An agent's conversation through weft remote (remote.md §6d): a device
//! reads the newest page, follows the agent while the file grows, gets a long
//! message whole in parts, and is told to start over when the file is
//! replaced. A harness without `[conversation]` reads as unreadable.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Harness, Shell};
use serde_json::{Value, json};
use weft_core::signal::Sent;
use weft_proto::{Event, Line};

const FIXTURE: &str = include_str!("../../weft-core/tests/fixtures/conversation/grok.jsonl");

/// The Grok fixture harness file, run as a shell, its configuration home in
/// the test's folder.
fn grok_beside(dir: &Path) {
    let mut table: toml::Table = weft_core::harness::fixture::text("grok").parse().unwrap();
    table.insert("program".into(), "/bin/sh".into());
    let config = table["config"].as_table().cloned().unwrap();
    let mut config = config;
    config.remove("env");
    config.insert("default".into(), dir.join("grok-home").to_string_lossy().into_owned().into());
    table.insert("config".into(), config.into());
    std::fs::create_dir_all(dir.join("harnesses")).unwrap();
    std::fs::write(dir.join("harnesses/grok.toml"), toml::to_string(&table).unwrap()).unwrap();
}

/// A project with a Grok pane: the shell, its token, and its transcript's path.
fn grok_pane(h: &Harness) -> (Shell, String, PathBuf) {
    grok_pane_in(h, "conversation", "g1")
}

/// A Grok pane in this project, its session's transcript named for the session.
fn grok_pane_in(h: &Harness, project: &str, session: &str) -> (Shell, String, PathBuf) {
    let shell = h.shell_of(project, "grok");
    let pane = shell.project.with_extension("pane");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pane.exists() {
        assert!(Instant::now() < deadline, "the pane never said its token");
        std::thread::sleep(Duration::from_millis(20));
    }
    let token = std::fs::read_to_string(&pane).unwrap().lines().next().unwrap().to_string();
    let transcript = h.dir.join(format!("grok-home/sessions/%2F{project}/{session}/updates.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    (shell, token, transcript)
}

/// What the pane's hook says, naming its transcript.
fn hook(h: &Harness, shell: &Shell, token: &str, transcript: &Path) {
    hook_of(h, shell, token, transcript, "g1");
}

fn hook_of(h: &Harness, shell: &Shell, token: &str, transcript: &Path, session: &str) {
    let sent = Sent {
        harness: "grok".into(),
        hook: "SessionStart".into(),
        session: session.into(),
        workspace: Some(shell.project.to_string_lossy().into_owned()),
        state: Some("ready".into()),
        pane: Some(0),
        token: Some(token.into()),
        transcript: Some(transcript.to_string_lossy().into_owned()),
        ..Sent::default()
    };
    let mut s = std::os::unix::net::UnixStream::connect(&h.socket).unwrap();
    s.write_all(&Line::Event(Event::Signal(Box::new(sent))).encode()).unwrap();
}

/// A transcript line written three days ago.
fn line(kind: &str, text: &str) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let at = now.as_secs() - 3 * 86_400;
    json!({"timestamp": at, "params": {"update": {"sessionUpdate": kind,
        "content": {"type": "text", "text": text}}}})
    .to_string()
}

#[test]
fn a_device_reads_and_follows_a_conversation() {
    let mut h = Harness::with("conversation", grok_beside);
    let (shell, token, transcript) = grok_pane(&h);
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    let read = |d: &mut common::Device, req: i64, before: Value| {
        d.send(json!({"type": "read", "what": "conversation", "agent": agent, "before": before, "req": req}));
        d.answer(req)
    };

    // No hook has named a file yet.
    assert_eq!(read(&mut d, 1, Value::Null)["conversation"]["readable"], false);

    let lines: Vec<&str> = FIXTURE.lines().collect();
    std::fs::write(&transcript, format!("{}\n", lines[..10].join("\n"))).unwrap();
    hook(&h, &shell, &token, &transcript);
    let deadline = Instant::now() + Duration::from_secs(5);
    let page = loop {
        let page = read(&mut d, 2, Value::Null)["conversation"].clone();
        if page["readable"] == true {
            break page;
        }
        assert!(Instant::now() < deadline, "the hook's path never reached weftd");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(page["items"][0]["text"], "Reply with the single word ok.");
    assert_eq!(page["before"], Value::Null);
    let after = page["after"].as_str().unwrap().to_string();

    // Followed: what the file adds comes once, with the next cursor.
    d.send(json!({"type": "watch", "agent": null}));
    d.send(json!({"type": "follow", "agent": agent, "after": after}));
    std::fs::write(&transcript, FIXTURE).unwrap();
    let grown = d.next_of("conversation");
    assert!(grown["items"].as_array().unwrap().iter().any(|i| i["kind"] == "step"), "{grown}");
    assert!(!grown.to_string().contains("transcript") && !grown.to_string().contains("grok-home"));

    // A reply too long for one message comes in parts, whole.
    let long = "word ".repeat(30_000);
    let mut f = std::fs::OpenOptions::new().append(true).open(&transcript).unwrap();
    writeln!(f, "{}", line("agent_message_chunk", &long)).unwrap();
    let mut text = String::new();
    while text.len() < long.len() {
        let m = d.next_of("conversation");
        for part in m["items"].as_array().unwrap() {
            assert!(part["parts"].as_u64().unwrap() > 1, "{part}");
            text.push_str(part["text"].as_str().unwrap());
        }
    }
    assert_eq!(text, long);

    // The file replaced: the device starts over from the newest page.
    let other = transcript.with_file_name("new.jsonl");
    std::fs::write(&other, FIXTURE).unwrap();
    std::fs::rename(&other, &transcript).unwrap();
    assert_eq!(
        d.next_of("conversation"),
        json!({"type": "conversation", "agent": agent, "reset": true})
    );

    // Not followed: the file grows, and nothing is sent.
    writeln!(
        std::fs::OpenOptions::new().append(true).open(&transcript).unwrap(),
        "{}",
        line("agent_message_chunk", "late")
    )
    .unwrap();
    d.ws.get_mut().set_read_timeout(Some(Duration::from_millis(2_500))).unwrap();
    while let Ok(tungstenite::Message::Text(t)) = d.ws.read() {
        let m: Value = serde_json::from_str(t.as_str()).unwrap();
        assert_ne!(m["type"], "conversation", "{m}");
    }
    let _ = shell;
}

#[test]
fn a_harness_without_a_mapping_reads_as_unreadable() {
    let mut h = Harness::start("conversation-none");
    let _shell = h.shell("p");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    d.send(json!({"type": "read", "what": "conversation", "agent": agent, "req": 1}));
    let ok = d.answer(1);
    assert_eq!(ok["conversation"], json!({"readable": false, "items": []}), "{ok}");
    d.send(json!({"type": "read", "what": "conversation", "agent": agent, "before": 5, "req": 2}));
    assert_eq!(d.answer(2)["code"], "bad_request");
    d.send(json!({"type": "read", "what": "conversation", "agent": "nobody-9", "req": 3}));
    assert_eq!(d.answer(3)["code"], "no_agent");
}

/// A device that would fall behind (more than its queue holds at once) is
/// told to start over from the newest page, and stays connected.
#[test]
fn a_device_that_falls_behind_starts_over() {
    let mut h = Harness::with("conversation-behind", grok_beside);
    let (shell, token, transcript) = grok_pane(&h);
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    std::fs::write(&transcript, format!("{}\n", line("user_message_chunk", "hi"))).unwrap();
    hook(&h, &shell, &token, &transcript);
    let deadline = Instant::now() + Duration::from_secs(5);
    let after = loop {
        d.send(json!({"type": "read", "what": "conversation", "agent": agent, "req": 1}));
        let page = d.answer(1)["conversation"].clone();
        if let Some(after) = page["after"].as_str() {
            break after.to_string();
        }
        assert!(Instant::now() < deadline, "the hook's path never reached weftd");
        std::thread::sleep(Duration::from_millis(50));
    };
    d.send(json!({"type": "follow", "agent": agent, "after": after}));
    let burst: String = (0..100)
        .map(|n| {
            format!("{}\n", line("agent_message_chunk", &format!("{n}{}", "x".repeat(50_000))))
        })
        .collect();
    let mut f = std::fs::OpenOptions::new().append(true).open(&transcript).unwrap();
    f.write_all(burst.as_bytes()).unwrap();
    assert_eq!(
        d.next_of("conversation"),
        json!({"type": "conversation", "agent": agent, "reset": true})
    );
    d.send(json!({"type": "read", "what": "conversation", "agent": agent, "req": 2}));
    assert_eq!(d.answer(2)["type"], "ok", "still connected");
}

/// A device with two panes in Conversation follows both agents; when one
/// pane leaves Conversation the other is still followed.
#[test]
fn a_device_follows_each_of_its_panes_and_one_closing_leaves_the_other() {
    let mut h = Harness::with("conversation-panes", grok_beside);
    let (one, token1, t1) = grok_pane_in(&h, "one", "g1");
    let (two, token2, t2) = grok_pane_in(&h, "two", "g2");
    let (mut d, _) = h.allowed("d_1");
    let rows = d.agents(2);
    let id = |p: &str| {
        rows.iter().find(|r| r["project"] == p).unwrap()["agent"].as_str().unwrap().to_string()
    };
    let (a1, a2) = (id("one"), id("two"));
    std::fs::write(&t1, format!("{}\n", line("user_message_chunk", "one"))).unwrap();
    std::fs::write(&t2, format!("{}\n", line("user_message_chunk", "two"))).unwrap();
    hook_of(&h, &one, &token1, &t1, "g1");
    hook_of(&h, &two, &token2, &t2, "g2");
    let after = |d: &mut common::Device, agent: &str, req: i64| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            d.send(json!({"type": "read", "what": "conversation", "agent": agent, "req": req}));
            if let Some(a) = d.answer(req)["conversation"]["after"].as_str() {
                return a.to_string();
            }
            assert!(Instant::now() < deadline, "the hook's path never reached weftd");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let (c1, c2) = (after(&mut d, &a1, 1), after(&mut d, &a2, 2));
    d.send(
        json!({"type": "follow", "agent": a1, "after": c1, "also": [{"agent": a2, "after": c2}]}),
    );
    let grow = |t: &Path, text: &str| {
        let mut f = std::fs::OpenOptions::new().append(true).open(t).unwrap();
        writeln!(f, "{}", line("agent_message_chunk", text)).unwrap();
    };
    grow(&t1, "reply one");
    grow(&t2, "reply two");
    let mut heard = std::collections::BTreeSet::new();
    while heard.len() < 2 {
        let m = d.next_of("conversation");
        heard.insert(m["agent"].as_str().unwrap().to_string());
    }
    assert_eq!(heard, [a1.clone(), a2.clone()].into());

    // The first pane leaves Conversation: the second is still followed.
    d.send(json!({"type": "follow", "agent": a2, "after": c2}));
    grow(&t1, "unheard");
    grow(&t2, "still heard");
    let m = d.next_of("conversation");
    assert_eq!(m["agent"], a2.as_str(), "{m}");
    assert!(m.to_string().contains("still heard"), "{m}");
    d.ws.get_mut().set_read_timeout(Some(Duration::from_millis(2_500))).unwrap();
    while let Ok(tungstenite::Message::Text(t)) = d.ws.read() {
        let m: Value = serde_json::from_str(t.as_str()).unwrap();
        assert!(m["type"] != "conversation" || m["agent"] != a1.as_str(), "{m}");
    }
}

#[test]
fn a_device_asking_for_a_suggestion_is_told_there_is_no_such_message() {
    let mut h = Harness::with("nosuggest", grok_beside);
    let (_shell, _token, _transcript) = grok_pane_in(&h, "watched", "g1");
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();
    d.send(json!({"type": "suggest", "agent": agent, "draft": "run the", "req": 1}));
    let refused = d.answer(1);
    assert_eq!(
        (refused["type"].as_str(), refused["code"].as_str()),
        (Some("error"), Some("bad_request")),
        "{refused}"
    );
}

/// The files an agent names come with its conversation, wherever they are,
/// and a device reads one a page at a time; a path the agent never named is
/// refused.
#[test]
fn a_device_reads_the_files_an_agent_named() {
    let mut h = Harness::with("conversation", grok_beside);
    let (shell, token, transcript) = grok_pane(&h);
    let (mut d, _) = h.allowed("d_1");
    let agent = d.agents(1)[0]["agent"].as_str().unwrap().to_string();

    std::fs::create_dir_all(shell.project.join("out")).unwrap();
    std::fs::write(shell.project.join("out/report.md"), "# Q3\n").unwrap();
    std::fs::write(shell.project.join("out/secret.md"), "not named\n").unwrap();
    let outside = h.dir.join("tmp/board.pdf");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    let pdf: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&outside, &pdf).unwrap();
    let said = format!("Wrote `out/report.md` and `{}`.", outside.display());
    std::fs::write(&transcript, format!("{}\n", line("agent_message_chunk", &said))).unwrap();
    hook(&h, &shell, &token, &transcript);

    let deadline = Instant::now() + Duration::from_secs(5);
    let refs = loop {
        d.send(json!({"type": "read", "what": "conversation", "agent": agent, "req": 1}));
        let page = d.answer(1)["conversation"].clone();
        if page["readable"] == true {
            break page["items"][0]["refs"].clone();
        }
        assert!(Instant::now() < deadline, "the hook's path never reached weftd");
        std::thread::sleep(Duration::from_millis(50));
    };
    let board = outside.canonicalize().unwrap().to_string_lossy().into_owned();
    assert_eq!(refs[0]["path"], "out/report.md");
    assert_eq!(refs[0]["kind"], "markdown");
    assert_eq!(refs[1]["path"], board.as_str());
    assert_eq!(refs[1]["kind"], "pdf");
    assert_eq!(refs[1]["size"], 100_000);

    d.send(json!({"type": "read", "what": "file", "agent": agent, "path": board, "req": 10}));
    let file = d.answer(10)["file"].clone();
    assert_eq!((file["kind"].as_str(), file["size"].as_u64()), (Some("pdf"), Some(100_000)));
    assert!(file.get("abs").is_none(), "the machine's own path stays on it");
    let url = file["url"].as_str().unwrap();
    assert!(url.starts_with("/f/") && url.ends_with("/board.pdf"), "{url}");

    let (head, body) = fetch(h.port, url, None);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.contains("Content-Type: application/pdf"), "{head}");
    assert_eq!(body, pdf);
    let (head, body) = fetch(h.port, url, Some("bytes=99990-"));
    assert!(
        head.starts_with("HTTP/1.1 206")
            && head.contains("Content-Range: bytes 99990-99999/100000")
    );
    assert_eq!(body, &pdf[99_990..]);
    let forged = url.replace("/f/", "/f/x");
    assert!(fetch(h.port, &forged, None).0.starts_with("HTTP/1.1 404"));

    d.send(
        json!({"type": "read", "what": "file", "agent": agent, "path": "out/secret.md", "req": 99}),
    );
    let refused = d.answer(99);
    assert_eq!(
        (refused["type"].as_str(), refused["code"].as_str()),
        (Some("error"), Some("no_file"))
    );
}

/// A GET on the tunnel's port, as the tunnel would pass it: the head and the
/// body, read to the end.
fn fetch(port: u16, path: &str, range: Option<&str>) -> (String, Vec<u8>) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let range = range.map(|r| format!("Range: {r}\r\n")).unwrap_or_default();
    write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n{range}Connection: close\r\n\r\n").unwrap();
    let mut all = Vec::new();
    s.read_to_end(&mut all).unwrap();
    let split = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    (String::from_utf8_lossy(&all[..split]).into_owned(), all[split + 4..].to_vec())
}
