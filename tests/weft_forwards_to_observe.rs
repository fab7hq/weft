//! weft signal → weftd → `ringframe observe` (ADR-0030 D9), end to end: a
//! harness's hooks run the real `weft signal` inside a pane, and weftd passes
//! a typed prompt byte for byte and a shell tool's outcome to a stand-in
//! `ringframe`, and nothing else. Its own test binary because it puts the
//! stand-in first on PATH.

mod support;

use std::path::Path;
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Line, Lines, PROTOCOL};
use weft::server;

/// A `ringframe` that logs each call's arguments, and what `observe prompt`
/// read on stdin, and says the profile names `Bash` as its shell tool.
fn stand_in(bin: &Path, log: &Path) {
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}/calls'\ncase \"$*\" in\n  \
         *\"observe prompt\"*) cat >> '{log}/stdin' ;;\n  \
         *\"profile show\"*) printf '{{\"observe\":{{\"shell_tool\":\"Bash\"}}}}' ;;\n  \
         *) printf '{{}}' ;;\nesac\nexit 0\n",
        log = log.display()
    );
    std::fs::create_dir_all(bin).unwrap();
    let path = bin.join("ringframe");
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

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
    let (bin, log, home) =
        (tmp.path().join("bin"), tmp.path().join("log"), tmp.path().join("home"));
    std::fs::create_dir_all(&log).unwrap();
    stand_in(&bin, &log);
    // SAFETY: this binary holds exactly one test.
    unsafe { std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display())) };
    let harnesses = home.join(".fab7/weft/harnesses");
    std::fs::create_dir_all(&harnesses).unwrap();
    std::fs::write(
        harnesses.join("claude-code.toml"),
        weft_core::harness::fixture::text("claude-code"),
    )
    .unwrap();
    let root = tmp.path().join("project");
    std::fs::create_dir_all(root.join(".fab7/rf")).unwrap();
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
    call(3, Call::StartAgent { harness: "claude-code".into(), spec, session: None });

    let calls = || std::fs::read_to_string(log.join("calls")).unwrap_or_default();
    let deadline = Instant::now() + Duration::from_secs(10);
    while calls().lines().filter(|l| l.contains(" observe ")).count() < 2 {
        if Instant::now() >= deadline {
            panic!("the calls never came: {}", calls());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));
    let root = root.display().to_string();
    let observed: Vec<String> =
        calls().lines().filter(|l| l.contains(" observe ")).map(str::to_string).collect();
    assert_eq!(
        observed,
        [
            format!("--workspace {root} observe prompt --by weft --host claude-code --session s1"),
            format!(
                "--workspace {root} observe tool --by weft --host claude-code --session s1 \
                 --tool Bash --outcome failed --command npm test"
            ),
        ],
        "in the order the hooks fired; Write and Stop are not forwarded"
    );
    assert_eq!(
        std::fs::read_to_string(log.join("stdin")).unwrap(),
        "/rf:ask fix the login bug\n  exactly",
        "byte for byte"
    );
    assert!(!calls().contains("elsewhere") && !calls().contains("SECRET"), "{}", calls());
    call(4, Call::Shutdown);
}
