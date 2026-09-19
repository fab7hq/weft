//! The two structural guards gate W1 owes, against a real ledger on disk.
//!
//! 1. **Traceability.** Every field the board shows traces to a ledger event.
//!    A screen may not say a thing the record did not say.
//! 2. **Read-only.** Weft takes no project lock and writes no file under
//!    `.fab7/rf/`, however long it is left running and whatever is pressed.
//!
//! Both are the guard against drifting into the session-viewer category, where
//! nothing is evidenced, so they are tests rather than review notes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};
use weft::app::App;
use weft::client::Session;
use weft::keys::Toggle;
use weft::server;

/// Words the screen may use only once a particular event is in the ledger.
const SAYS_A_VERDICT: &[&str] = &[
    "MATCHES WHAT YOU ASKED",
    "DOESN'T MATCH WHAT YOU ASKED",
    "NOT ENOUGH PROOF",
    "judges agreed",
    "judged by",
    "recorded as",
];
const SAYS_A_DECISION: &[&str] = &["ACCEPTED", "REJECTED", "PARKED", "DROPPED"];
const SAYS_A_RECEIPT: &[&str] = &["WORD FOR WORD", "SENT, REWORDED"];

#[test]
fn the_board_says_nothing_the_ledger_did_not_say() {
    let (root, mut app) = board(&[compiled("ask_1", "health endpoint", "codex")]);

    let drawn = screen(&mut app, 100, 30);
    assert!(drawn.contains("health endpoint"), "{drawn}");
    assert!(drawn.contains("NOT SENT YET"), "{drawn}");
    for phrase in SAYS_A_VERDICT.iter().chain(SAYS_A_DECISION).chain(SAYS_A_RECEIPT) {
        assert!(
            !drawn.contains(phrase),
            "with only ask.compiled in the ledger the board claimed {phrase:?}:\n{drawn}"
        );
    }
    clean(&root);
}

#[test]
fn each_field_appears_only_once_its_own_event_is_written() {
    // A verdict needs an eval.completed; a decision needs a seal.created; a
    // receipt needs an ask.submission. Adding them one at a time shows that
    // each phrase is carried by its own event and by nothing else.
    let compiled_only = vec![compiled("ask_1", "health endpoint", "codex")];
    let with_receipt = [
        compiled_only.clone(),
        vec![submission("ask_1")],
    ]
    .concat();
    let with_verdict = [with_receipt.clone(), vec![evaluated("evl_1", "ask_1")]].concat();
    let with_decision = [with_verdict.clone(), vec![sealed("sea_1", "ask_1")]].concat();

    for (events, expected, not_yet) in [
        (with_receipt, "WORD FOR WORD", SAYS_A_VERDICT),
        (with_verdict, "DOESN'T MATCH WHAT YOU ASKED", SAYS_A_DECISION),
        (with_decision, "ACCEPTED", &[] as &[&str]),
    ] {
        let (root, mut app) = board(&events);
        let drawn = screen(&mut app, 100, 30);
        assert!(drawn.contains(expected), "expected {expected:?} in:\n{drawn}");
        for phrase in not_yet {
            assert!(!drawn.contains(phrase), "too early for {phrase:?}:\n{drawn}");
        }
        clean(&root);
    }
}

#[test]
fn weft_takes_no_lock_and_writes_nothing_under_the_record() {
    let (root, mut app) = board(&[
        compiled("ask_1", "health endpoint", "codex"),
        submission("ask_1"),
        evaluated("evl_1", "ask_1"),
    ]);
    let before = snapshot(&root.join(".fab7"));

    // Everything a person does that is not an explicit injection: look at the
    // board, expand a row, read it, hide the list, ask for what needs them.
    for key in [
        KeyCode::Down,
        KeyCode::Enter,
        KeyCode::Char('j'),
        KeyCode::Left,
        KeyCode::Char(' '),
        KeyCode::Char('w'),
        KeyCode::Char('w'),
        KeyCode::Up,
        KeyCode::Char('h'),
        KeyCode::Enter,
    ] {
        app.on_key(KeyEvent::new(key, KeyModifiers::NONE)).expect("key");
        screen(&mut app, 100, 30);
    }

    let after = snapshot(&root.join(".fab7"));
    assert_eq!(before, after, "Weft wrote to the record it only reads");
    assert!(
        !root.join(".fab7/rf/lock").exists() && !root.join(".fab7/rf/.lock").exists(),
        "Weft took a project lock"
    );
    clean(&root);
}

// --- the fixture ----------------------------------------------------------

fn board(events: &[Value]) -> (PathBuf, App) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("weft-board-{}-{n}", std::process::id()));
    std::fs::create_dir_all(root.join(".fab7/rf")).expect("record");
    let lines: String = events.iter().map(|e| format!("{e}\n")).collect();
    std::fs::write(root.join(".fab7/rf/ledger.jsonl"), lines).expect("ledger");

    let socket = server::socket_path(&root);
    let _ = std::fs::remove_file(&socket);
    let serving = root.clone();
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(serving, &listening);
    });
    let session = Session::connect(&socket, 24, 80).expect("connect");
    let mut app = App::with_session(root.clone(), Toggle, session);
    app.add("codex", "/bin/cat").expect("a pane");
    app.refresh_for_test();
    (root, app)
}

fn screen(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| weft::ui::draw(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every path under the record, with its bytes. A write of any kind shows up.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.insert(path, bytes);
            }
        }
    }
    out
}

fn clean(root: &Path) {
    std::fs::remove_dir_all(root).ok();
}

fn compiled(id: &str, title: &str, host: &str) -> Value {
    event("ask.compiled", id, json!({
        "title": title,
        "selected_capability": "native_plan",
        "delivery_mode": "human_handoff",
        "host": {"name": host},
        "source": {}, "prompt": {}, "source_verified": "exact",
        "limitations": [], "classification": {}, "route_explanation": {}
    }))
}

fn submission(ask: &str) -> Value {
    event("ask.submission", ask, json!({
        "state": "observed",
        "observed_by": "hook:UserPromptSubmit",
        "prompt_sha256": "a".repeat(64),
        "as_modified": false
    }))
}

fn evaluated(id: &str, ask: &str) -> Value {
    event("eval.completed", id, json!({
        "verdict": "drifted",
        "confidence": 0.67,
        "judged_by": "codex",
        "basis": {"asks": [ask]}
    }))
}

fn sealed(id: &str, ask: &str) -> Value {
    event("seal.created", id, json!({
        "disposition": "accepted",
        "basis": {"asks": [ask]}
    }))
}

fn event(kind: &str, id: &str, data: Value) -> Value {
    json!({
        "schema": "ringframe.ledger/1",
        "event_id": format!("evt_{kind}_{id}"),
        "type": kind,
        "time": "2026-09-19T14:02:00Z",
        "id": id,
        "actor": {"kind": "human", "id": "me"},
        "links": [],
        "data": data
    })
}
