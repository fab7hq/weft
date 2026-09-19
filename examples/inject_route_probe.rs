//! Type one confirmed prompt into a real harness, and say only what Weft did.
//!
//! The subject of gate W3: whether a client typing `/plan …` or `/goal …` into
//! the real TUI is honoured as that route, with RingFrame's hook recording the
//! submission. This runs Weft's own path — the session server, the pane, the
//! confirmation, the ordered submission — with no terminal and no rendering,
//! so what is under test is the product and not a screen scraper.
//!
//!   cargo run --example inject_route_probe -- <workspace> <ask-id> "<agent>" <harness> <out-dir>
//!
//! It prints one JSON observation on stdout and judges nothing. Whether the
//! harness honoured the route, and whether the hook recorded it, are read from
//! the ledger by whoever runs this — never asserted here.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use weft::app::{App, Modal};
use weft::client::Session;
use weft::keys::Toggle;

/// A harness settling after launch, and after being typed into.
const SETTLE: Duration = Duration::from_secs(3);

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let workspace = args.next().expect("a workspace directory");
    let ask_id = args.next().expect("an ask id");
    let spec = args.next().expect("the agent command line");
    let harness = args.next().expect("the harness name RingFrame records");
    let out = std::path::PathBuf::from(args.next().expect("a directory to retain evidence in"));
    std::fs::create_dir_all(&out)?;

    let root = std::path::PathBuf::from(&workspace).canonicalize()?;
    let session = attach(&root)?;
    let mut app = App::with_session(root, Toggle, session);
    let mut steps: Vec<Value> = Vec::new();

    app.add(&harness, &spec)?;
    steps.push(json!({"step": "spawn", "harness": harness, "spec": spec, "panes": app.pane_count()}));

    // Startup questions belong to the agent and are answered there, the way a
    // person would. Weft never answers one on anyone's behalf.
    let answered = settle_startup(&mut app);
    steps.push(json!({"step": "startup", "answered": answered, "screen": tail(&app)}));

    app.refresh_for_test();
    let Some(index) = app.units().iter().position(|u| u.ask_id == ask_id) else {
        return done(json!({
            "outcome": "no-such-ask",
            "ask_id": ask_id,
            "known": app.units().iter().map(|u| u.ask_id.clone()).collect::<Vec<_>>(),
            "steps": steps
        }));
    };
    for _ in 0..index {
        press(&mut app, KeyCode::Down);
    }
    let selected = app.selected_unit().map(|u| u.ask_id.clone());
    steps.push(json!({"step": "select", "ask_id": selected, "status": app.selected_unit().map(|u| u.status())}));

    // [S]END asks first, always. What it is about to type is the observation.
    press(&mut app, KeyCode::Char('s'));
    let Some(Modal::Confirm(pending)) = app.modal.clone() else {
        return done(json!({
            "outcome": "not-offered",
            "hint": app.hint_text(),
            "modal": format!("{:?}", app.modal),
            "steps": steps
        }));
    };
    let payload = pending.payload.clone();
    // The bytes themselves are the evidence. Weft hashes nothing and compares
    // nothing: the digest RingFrame recorded is the claim, and whoever runs
    // this checks it against these bytes.
    std::fs::write(out.join("typed.bin"), &payload)?;
    steps.push(json!({
        "step": "confirm",
        "typed_bytes_at": out.join("typed.bin").display().to_string(),
        "bytes": payload.len(),
        "starts_with": String::from_utf8_lossy(&payload).chars().take(24).collect::<String>()
    }));

    press(&mut app, KeyCode::Enter);
    let refusal = app.last_refusal();
    steps.push(json!({"step": "inject", "weft_refused": refusal}));

    // Give the harness room to react, and stop once the record has stopped
    // moving. Waiting for the ledger to settle is not reading a verdict from
    // it: what it says is the runner's to judge, not the probe's.
    let ledger = std::path::Path::new(&workspace).join(".fab7/rf/ledger.jsonl");
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut size = std::fs::metadata(&ledger).map(|m| m.len()).unwrap_or(0);
    let mut still = Instant::now();
    while Instant::now() < deadline && still.elapsed() < Duration::from_secs(15) {
        app.pump();
        std::thread::sleep(Duration::from_millis(250));
        let now = std::fs::metadata(&ledger).map(|m| m.len()).unwrap_or(0);
        if now != size {
            size = now;
            still = Instant::now();
        }
    }
    steps.push(json!({"step": "after", "screen": tail(&app)}));

    done(json!({
        "outcome": "typed",
        "ask_id": ask_id,
        "harness": harness,
        "typed_bytes_at": out.join("typed.bin").display().to_string(),
        "weft_refused": refusal,
        "steps": steps
    }))
}

/// Attach to a session the real `weft` binary is serving, so the probe runs
/// the production topology — a separate server process owning the PTY — and
/// not a library shortcut around it.
fn attach(root: &std::path::Path) -> anyhow::Result<Session> {
    let socket = weft::server::socket_path(root);
    if !weft::server::is_live(&socket) {
        weft::server::clear_dead(&socket);
        let bin = concat!(env!("CARGO_MANIFEST_DIR"), "/target/debug/weft");
        std::process::Command::new(bin).arg("--serve").arg(root).spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !weft::server::is_live(&socket) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    Session::connect(&socket, 40, 120)
}

fn done(observation: Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(&observation)?);
    Ok(())
}

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE)).expect("key");
    app.pump();
    std::thread::sleep(Duration::from_millis(300));
}

/// Answer the harness's own startup questions in its pane: an update offer, a
/// hook-trust panel, a folder-trust question. Returns what was answered.
fn settle_startup(app: &mut App) -> Vec<String> {
    let mut answered = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut quiet = Instant::now();
    while Instant::now() < deadline {
        app.pump();
        let screen = app.pane_text(0).unwrap_or_default();
        if screen.contains("Update available") {
            app.input(0, b"\x1b[B").ok();
            app.input(0, b"\r").ok();
            answered.push("update".into());
        } else if screen.contains("Press t to trust") {
            app.input(0, b"t").ok();
            answered.push("hook-trust".into());
        } else if screen.contains("trust this folder") || screen.contains("Is this a project you created") {
            if screen.contains("No, exit") {
                app.input(0, b"\x1b[B").ok();
            }
            app.input(0, b"\r").ok();
            answered.push("folder-trust".into());
        } else if app.waiting(0).is_none() && quiet.elapsed() > SETTLE {
            return answered;
        } else {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        quiet = Instant::now();
        std::thread::sleep(Duration::from_secs(2));
    }
    answered.push("timed-out".into());
    answered
}

fn tail(app: &App) -> String {
    let screen = app.pane_text(0).unwrap_or_default();
    let lines: Vec<&str> = screen.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}
