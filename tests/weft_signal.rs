//! `weft signal`, as a harness's hook runs it (ADR-0030 D1): it answers the
//! hook and exits 0 whatever it is given, with or without a daemon.

use std::io::Write;
use std::process::{Command, Stdio};

fn signal(args: &[&str], stdin: &[u8]) -> (i32, String) {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_weft"))
        .arg("signal")
        .args(args)
        .env("HOME", home.path())
        .env("WEFT_SOCKET", home.path().join("nobody.sock"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(stdin);
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn a_hook_is_answered_and_never_fails_the_turn() {
    let allow = r#"{"decision":"allow"}"#;
    assert_eq!(signal(&["--reply", allow, "any", "PreToolUse"], b"{}"), (0, format!("{allow}\n")));
    assert_eq!(signal(&["--reply", "{}", "any", "Stop"], b"\xff not json"), (0, "{}\n".into()));
    assert_eq!(signal(&["../../etc/passwd", "Stop"], b"{}"), (0, String::new()));
    assert_eq!(signal(&[], b""), (0, String::new()));
}
