//! Run one real harness in a Weft pane and say what Weft saw of its state.
//!
//! The subject of gate W5: whether the harness's own hooks, mapped by
//! RingFrame's plugin, report `ready`, `working`, `waiting` and `turn_ended`
//! when they should. This runs Weft's own path — the session server, the pane,
//! the state it reads from receipts, the confirmation and the send — and acts
//! as the person would: it answers the harness's startup questions, and it
//! answers the harness only when Weft says the agent is waiting for its
//! person.
//!
//!   cargo run --example turn_probe -- <workspace> <ask-id> "<agent>" <harness> <out-dir> <weft>
//!
//! It prints one JSON observation on stdout, with the time of each step in
//! milliseconds since the epoch, and judges nothing. The receipts, the ledger
//! and the host's transcript are read by whoever runs it.

use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use weft::app::{App, Modal};
use weft::client::Session;
use weft::keys::Toggle;
use weft::turns::Turn;

/// A harness settling after launch.
const SETTLE: Duration = Duration::from_secs(3);
/// How long a person would give an agent to say it is ready.
const READY_WITHIN: Duration = Duration::from_secs(90);
/// How long the turn may take, answers included.
const TURN_WITHIN: Duration = Duration::from_secs(420);
/// How long a turn that ended must stay ended before the probe stops.
const ENDED_FOR: Duration = Duration::from_secs(10);
/// How many times the probe answers the agent before it stops answering.
const ANSWERS: usize = 6;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let workspace = args.next().expect("a workspace directory");
    let ask_id = args.next().expect("an ask id");
    let spec = args.next().expect("the agent command line");
    let harness = args.next().expect("the harness name RingFrame records");
    let out = std::path::PathBuf::from(args.next().expect("a directory to retain evidence in"));
    let weft_bin = args.next().expect("the weft binary that serves");
    std::fs::create_dir_all(&out)?;

    let root = std::path::PathBuf::from(&workspace).canonicalize()?;
    let session = attach(&root, &weft_bin)?;
    let mut app = App::with_session(root, Toggle, session);
    let mut steps: Vec<Value> = Vec::new();

    app.add(&harness, &spec)?;

    app.settle();
    steps.push(json!({"step": "spawn", "at": now(), "harness": harness, "spec": spec}));

    // Startup questions belong to the agent and are answered there, the way a
    // person would. Weft never answers one on anyone's behalf.
    let answered = settle_startup(&mut app);
    steps.push(json!({"step": "startup", "at": now(), "answered": answered, "screen": tail(&app)}));

    // Weft types on its own only once the agent said it is ready.
    let until = Instant::now() + READY_WITHIN;
    while Instant::now() < until && !ready(app.pane_turn(0)) {
        app.pump();
        std::thread::sleep(Duration::from_millis(100));
    }
    let state = app.pane_turn(0);
    steps.push(json!({"step": "ready", "at": now(), "state": state, "screen": tail(&app)}));
    // An agent that never says it is ready is itself the finding for
    // `ready`. The probe goes on as its person would, so the other events
    // are still seen: Weft will ask, and the person says type it anyway.
    let never_ready = !ready(state);

    app.refresh_for_test();
    let Some(index) = app.units().iter().position(|u| u.ask_id == ask_id) else {
        return done(json!({"outcome": "no-such-ask", "steps": steps}));
    };
    for _ in 0..index {
        press(&mut app, KeyCode::Down);
    }
    // [P]ROCEED on a confirmed handoff is the send, and it asks first.
    press(&mut app, KeyCode::Char('p'));
    let Some(Modal::Confirm(pending)) = app.modal.clone() else {
        // Weft will not act for a harness whose readiness it cannot read. The
        // person then types the prompt into the agent themselves, which is
        // what the probe does, so the hooks are still exercised.
        let refused = app.hint_text().map(str::to_string);
        if !never_ready || refused.is_none() {
            return done(json!({
                "outcome": "not-offered",
                "hint": refused,
                "modal": format!("{:?}", app.modal),
                "steps": steps
            }));
        }
        let prompt = app.units()[index].ask_id.clone();
        let bytes = std::fs::read(
            std::path::Path::new(&workspace).join(".fab7/rf/asks").join(&prompt).join("prompt.txt"),
        )?;
        std::fs::write(out.join("typed.bin"), &bytes)?;
        let typed_at = now();
        app.input(0, &bytes)?;
        std::thread::sleep(Duration::from_secs(1));
        app.input(0, b"\r")?;
        steps.push(json!({"step": "typed", "at": typed_at, "bytes": bytes.len(),
                          "by": "the person", "weft_refused": refused}));
        return watch(&mut app, steps, never_ready);
    };
    std::fs::write(out.join("typed.bin"), &pending.payload)?;
    let typed_at = now();
    press(&mut app, KeyCode::Enter);
    let mut forced = None;
    if let Some(Modal::SendAnyway { why, .. }) = &app.modal {
        // Weft asked instead of typing. Only an agent that never said it was
        // ready is typed into anyway, the way its person would; asking about
        // one that did would be Weft's state failing, which is the outcome.
        let why = why.to_string();
        if !never_ready {
            return done(json!({"outcome": "weft-asked", "why": why, "steps": steps}));
        }
        app.modal_choice = 0; // "Type it anyway"
        press(&mut app, KeyCode::Enter);
        forced = Some(why);
    }
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until && app.last_refusal().is_none() {
        app.pump();
        std::thread::sleep(Duration::from_millis(100));
    }
    let refusal = app.last_refusal();
    steps.push(json!({"step": "typed", "at": typed_at, "bytes": pending.payload.len(),
                      "weft_refused": refusal, "forced": forced}));
    if refusal.is_some() {
        return done(json!({"outcome": "weft-refused", "weft_refused": refusal, "steps": steps}));
    }

    watch(&mut app, steps, never_ready)
}

/// Watch the state Weft reads, answering the agent as its person only when
/// Weft says it is waiting, until its turn has stayed ended.
fn watch(app: &mut App, mut steps: Vec<Value>, never_ready: bool) -> anyhow::Result<()> {
    // Watch the state Weft reads. Answer the agent, as its person, only when
    // Weft says it is waiting: Enter takes the agent's own first choice.
    let deadline = Instant::now() + TURN_WITHIN;
    let mut last = app.pane_turn(0);
    let mut changes = vec![json!({"at": now(), "state": last})];
    let mut answers: Vec<Value> = Vec::new();
    let mut unreported: Vec<Value> = Vec::new();
    let mut ended: Option<Instant> = None;
    let mut outcome = "turn-never-ended";
    while Instant::now() < deadline {
        app.pump();
        let state = app.pane_turn(0);
        if state != last {
            changes.push(json!({"at": now(), "state": state}));
            last = state;
            ended = None;
        }
        match state {
            Some(Turn::Waiting) if answers.len() < ANSWERS => {
                std::thread::sleep(Duration::from_secs(2));
                answers.push(json!({"at": now(), "screen": tail(app)}));
                app.input(0, b"\r").ok();
                // The answer lands before the next look.
                std::thread::sleep(Duration::from_secs(2));
            }
            // A question the harness asks its person without reporting it:
            // Weft cannot see it, but the person can, and answers it. Only
            // while the turn is still going, and each one is recorded.
            _ if state != Some(Turn::TurnEnded) && unreported.len() < ANSWERS => {
                let screen = app.pane_text(0).unwrap_or_default();
                if let Some((name, _, keys)) =
                    UNREPORTED.iter().find(|(_, n, _)| screen.contains(n))
                {
                    std::thread::sleep(Duration::from_secs(2));
                    unreported.push(json!({"at": now(), "question": name, "screen": tail(app)}));
                    app.input(0, keys).ok();
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
            Some(Turn::TurnEnded) => {
                let since = *ended.get_or_insert_with(Instant::now);
                if since.elapsed() >= ENDED_FOR {
                    outcome = "turn-ended";
                    break;
                }
            }
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    steps.push(json!({"step": "watched", "at": now(), "changes": changes, "answers": answers, "unreported": unreported,
                      "screen": tail(app)}));
    done(json!({"outcome": outcome, "never_ready": never_ready, "steps": steps}))
}

fn ready(state: Option<Turn>) -> bool {
    matches!(state, Some(Turn::Ready | Turn::TurnEnded))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Attach to the daemon this `weft` serves, started here if none is: the
/// production topology, a separate server process owning the PTY.
fn attach(root: &std::path::Path, bin: &str) -> anyhow::Result<Session> {
    let socket = weft::protocol::socket_path();
    if !weft::protocol::is_live(&socket) {
        weft::protocol::clear_dead(&socket);
        // Detached, as the product detaches it.
        std::process::Command::new(bin)
            .arg("--serve")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !weft::protocol::is_live(&socket) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    Session::connect(&socket, root, 40, 120)
}

fn done(observation: Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(&observation)?);
    Ok(())
}

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE)).expect("key");
    app.settle();
    app.pump();
    std::thread::sleep(Duration::from_millis(300));
}

/// The harness's own startup questions, as it words them, and the keys a
/// person would answer them with. The probe is the person here, not Weft:
/// Weft reads no screen. Declared in full so that what is answered is on the
/// record and nothing else is.
const STARTUP: &[(&str, &str, &[u8])] = &[
    ("update", "Update available", b"\x1b[B\r"),
    // Antigravity, which defaults to "Yes, I trust this folder". First, since
    // that choice's own words contain Claude Code's question below.
    ("folder-trust", "Do you trust the contents of this project?", b"\r"),
    ("hook-trust", "Press t to trust", b"t"),
    // Codex 0.156 asks it as a menu; the second choice trusts them and goes on.
    ("hook-trust", "Hooks need review", b"\x1b[B\r"),
    // Claude Code, which defaults to "No, exit", so the choice moves down.
    ("folder-trust", "Is this a project you created", b"\x1b[B\r"),
    ("folder-trust", "trust this folder", b"\x1b[B\r"),
    // Codex, which defaults to "1. Yes, continue".
    ("folder-trust", "Do you trust the contents of this directory", b"\r"),
    // Codex 0.156, which words it anew and defaults to "1. Trust and continue".
    ("folder-trust", "Trust this folder?", b"\r"),
    ("mcp-offer", "New MCP server found", b"\r"),
    ("imports", "disable external imports", b"\r"),
];

/// Questions a harness asks its person mid-turn without any hook, as the
/// person reads them, and the key that takes its first choice.
const UNREPORTED: &[(&str, &str, &[u8])] = &[
    // Antigravity, before a command it may not run on its own.
    ("run-command", "Run this command?", b"\r"),
];

/// Answer those questions, each with the time it was answered.
fn settle_startup(app: &mut App) -> Vec<Value> {
    let mut answered = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut quiet = Instant::now();
    while Instant::now() < deadline {
        app.pump();
        let screen = app.pane_text(0).unwrap_or_default();
        match STARTUP.iter().find(|(_, needle, _)| screen.contains(needle)) {
            Some((name, _, keys)) => {
                app.input(0, keys).ok();
                answered.push(json!({"name": name, "at": now()}));
                quiet = Instant::now();
                std::thread::sleep(Duration::from_secs(3));
            }
            None if quiet.elapsed() > SETTLE => return answered,
            None => std::thread::sleep(Duration::from_millis(250)),
        }
    }
    answered.push(json!({"name": "timed-out", "at": now()}));
    answered
}

fn tail(app: &App) -> String {
    let screen = app.pane_text(0).unwrap_or_default();
    let lines: Vec<&str> = screen.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}
