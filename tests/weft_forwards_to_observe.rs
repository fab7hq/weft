//! weft signal → weftd → RingFrame's observe calls, end to end: a harness's
//! hooks run the real `weft signal` inside a pane, and weftd passes a typed
//! prompt byte for byte and a shell tool's outcome to RingFrame, and nothing
//! else. Its own test binary because it sets `HOME` for RingFrame's
//! configuration.

mod support;

use std::path::Path;
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Line, Lines, PROTOCOL};
use weft::server;

/// An agent whose hooks fire as a harness's would: each runs `weft signal`
/// with a payload, from inside the pane.
fn agent(dir: &Path, home: &Path, root: &Path) -> String {
    let weft = env!("CARGO_BIN_EXE_weft");
    let hook = |hook: &str, payload: &str| {
        format!(
            "printf '%s' '{payload}' | HOME='{}' '{weft}' signal claude-code {hook}\n",
            home.display()
        )
    };
    let r = root.display();
    let script = [
        "#!/bin/sh\n".to_string(),
        hook("UserPromptSubmit", &format!(r#"{{"session_id":"s1","cwd":"{r}","prompt":"/rf:ask fix the login bug\n  exactly"}}"#)),
        hook("PostToolUseFailure", &format!(r#"{{"session_id":"s1","cwd":"{r}","tool_name":"Bash","tool_input":{{"command":"npm test"}},"tool_response":{{"stderr":"SECRET"}}}}"#)),
        hook("PostToolUse", &format!(r#"{{"session_id":"s1","cwd":"{r}","tool_name":"Write","tool_input":{{}}}}"#)),
        hook("Stop", &format!(r#"{{"session_id":"s1","cwd":"{r}"}}"#)),
        "exec cat\n".to_string(),
    ]
    .concat();
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("hooks.sh");
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn a_typed_prompt_and_a_shell_tool_reach_observe_and_nothing_else_does() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: this binary holds exactly one test.
    unsafe { std::env::set_var("HOME", &home) };
    let harnesses = home.join(".fab7/weft/harnesses");
    std::fs::create_dir_all(&harnesses).unwrap();
    std::fs::write(
        harnesses.join("claude-code.toml"),
        weft_core::harness::fixture::text("claude-code"),
    )
    .unwrap();
    let root = tmp.path().join("project");
    // A real record: Git, RingFrame's test configuration, and an Ask.
    support::real_ask(&root, &home, "sh");
    let elsewhere = tmp.path().join("no-record");
    std::fs::create_dir_all(&elsewhere).unwrap();

    // Written before the daemon starts, so no program it runs meanwhile holds
    // the script open while it is written.
    let spec = agent(&tmp.path().join("agent"), &home, &root);
    let socket = protocol::private_socket("weft-forwards");
    let (listening, config) = (socket.clone(), tmp.path().join("config.toml"));
    let fixture = weft::harness_table::Harnesses::read(vec![(
        "claude-code".into(),
        weft_core::harness::fixture::text("claude-code").into(),
    )])
    .0;
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(&listening, &config, Some(fixture));
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let stream = loop {
        if let Ok(s) = std::os::unix::net::UnixStream::connect(&socket) {
            break s;
        }
        assert!(Instant::now() < deadline, "no daemon");
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut writer = stream.try_clone().unwrap();
    let mut lines = Lines::new(stream);
    // Every call is answered; reading each answer keeps the daemon writing.
    let mut call = |id: u64, call: Call| {
        protocol::send(&mut writer, &Line::Call { id, call }.encode()).unwrap();
        loop {
            match lines.next() {
                Ok(Some(Line::Result { id: got, .. } | Line::Error { id: got, .. }))
                    if got == id =>
                {
                    return;
                }
                Ok(Some(_)) => {}
                _ => return,
            }
        }
    };
    call(1, Call::Hello { client: "test".into(), protocol: PROTOCOL });
    let path = root.to_string_lossy().into_owned();
    call(2, Call::Open { path, rows: 24, cols: 80, raw: false });
    // A prompt from a project with no record goes nowhere.
    support::signal(
        &socket,
        weft_core::signal::Sent {
            harness: "claude-code".into(),
            hook: "UserPromptSubmit".into(),
            session: "s2".into(),
            workspace: Some(elsewhere.to_string_lossy().into_owned()),
            forward: Some("prompt".into()),
            prompt: Some("/rf:ask elsewhere".into()),
            ..Default::default()
        },
    );
    let spec = format!("/bin/sh {spec}");
    // A session that is not only an id is never passed on.
    support::signal(
        &socket,
        weft_core::signal::Sent {
            harness: "claude-code".into(),
            hook: "UserPromptSubmit".into(),
            session: "--json".into(),
            workspace: Some(root.to_string_lossy().into_owned()),
            state: Some("working".into()),
            forward: Some("prompt".into()),
            prompt: Some("/rf:ask not an id".into()),
            ..Default::default()
        },
    );
    call(3, Call::StartAgent { harness: "claude-code".into(), spec, session: None });

    let session = |s: &str| root.join(".fab7/rf/sessions/claude-code").join(s);
    let prompts = || std::fs::read_to_string(session("s1").join("prompts.jsonl"));
    let facts = || -> Vec<serde_json::Value> {
        let ws = ringframe::api::workspace(&root).unwrap();
        let events = ringframe::api::events(&ws).unwrap();
        events.into_iter().filter(|e| e["type"] == "tool.fact").collect()
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while prompts().is_err() || facts().is_empty() {
        if Instant::now() >= deadline {
            panic!("RingFrame never recorded them: {:?} {:?}", prompts(), facts());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));
    let seen: Vec<serde_json::Value> =
        prompts().unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let sent = ringframe::digest::sha256_bytes(b"/rf:ask fix the login bug\n  exactly");
    assert_eq!(seen[0]["sha256"], sent, "byte for byte");
    assert!(seen[0].get("prompt").is_none(), "its text is never kept");
    assert_eq!(seen[0]["observed_by"], "weft");
    let facts = facts();
    assert_eq!(facts.len(), 1, "Write and Stop are not forwarded: {facts:?}");
    assert_eq!(
        (&facts[0]["data"]["outcome"], &facts[0]["data"]["command"]),
        (&serde_json::json!("failed"), &serde_json::json!("npm test"))
    );
    assert!(!elsewhere.join(".fab7").exists(), "no record made elsewhere");
    assert!(!session("--json").exists(), "a session that is not an id is never passed on");
    let everything = std::process::Command::new("/usr/bin/grep")
        .args(["-r", "SECRET"])
        .arg(root.join(".fab7/rf"))
        .output()
        .unwrap();
    assert!(everything.stdout.is_empty(), "{}", String::from_utf8_lossy(&everything.stdout));
    call(4, Call::Shutdown);
}
