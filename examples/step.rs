//! Drive Weft one deliberate step at a time, looking at each screen.
//!
//!   cargo run --example step -- <workspace> [options]
//!
//!     --spawn "<harness> <command line>"   start an agent in the session
//!     --keys  "a"                          keys to Weft, before the text
//!     --type  "fix the build"              text typed into Weft's ask box
//!     --after "Enter,Enter"                keys to Weft, after the text
//!     --pane  "\r"                         bytes to the agent, as a person types
//!     --wait  <seconds>                    how long to watch before reporting
//!     --stop                               end the session and its agents
//!
//! The session server outlives this process, so each run is one move: look at
//! what came back, decide the next move, run it again. That is what W4 means
//! by driving Weft by hand — a person answers the harness's own questions, and
//! `loop_drive`'s heuristics kept proving they are not a person.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use weft::app::App;
use weft::client::Session;
use weft::keys::Toggle;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let workspace = args.next().expect("a workspace directory");
    let root = std::path::PathBuf::from(&workspace).canonicalize()?;

    let mut spawn: Option<String> = None;
    let mut keys: Option<String> = None;
    let mut typed: Option<String> = None;
    let mut after: Option<String> = None;
    let mut pane: Option<String> = None;
    let mut wait = 4u64;
    let mut stop = false;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--spawn" => spawn = args.next(),
            "--keys" => keys = args.next(),
            "--type" => typed = args.next(),
            "--after" => after = args.next(),
            "--pane" => pane = args.next(),
            "--wait" => wait = args.next().and_then(|v| v.parse().ok()).unwrap_or(4),
            "--stop" => stop = true,
            other => panic!("unknown option {other}"),
        }
    }

    let session = attach(&root)?;
    let mut app = App::with_session(root.clone(), Toggle, session);
    // The server answers an attach asynchronously, so a key pressed before the
    // panes arrive lands on an App that believes it has none — and `[A]SK`,
    // quite correctly, refuses. Take delivery first.
    let settle = Instant::now() + Duration::from_secs(2);
    while Instant::now() < settle {
        app.pump();
        if app.pane_count() > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    app.refresh_for_test();

    if let Some(spec) = &spawn {
        let (harness, command) = spec.split_once(' ').unwrap_or((spec.as_str(), spec.as_str()));
        app.add(harness, command)?;
        println!("spawned {harness}: {command}");
    }
    if let Some(bytes) = &pane {
        let sent = unescape(bytes);
        app.input(0, &sent)?;
        println!("to the agent: {:?}", String::from_utf8_lossy(&sent));
    }
    let press = |app: &mut App, list: &str| -> anyhow::Result<()> {
        for name in list.split(',') {
            app.on_key(KeyEvent::new(key_of(name), KeyModifiers::NONE))?;
            app.pump();
            std::thread::sleep(Duration::from_millis(500));
        }
        println!("to Weft: {list}");
        Ok(())
    };
    if let Some(list) = &keys {
        press(&mut app, list)?;
    }
    if let Some(text) = &typed {
        for c in text.chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))?;
        }
        app.pump();
        println!("typed into Weft: {text:?}");
    }
    if let Some(list) = &after {
        press(&mut app, list)?;
    }

    let deadline = Instant::now() + Duration::from_secs(wait);
    while Instant::now() < deadline {
        app.pump();
        std::thread::sleep(Duration::from_millis(200));
    }
    app.refresh_for_test();

    println!("\n── the board ──");
    if app.units().is_empty() {
        println!("  (nothing asked for yet)");
    }
    for (i, unit) in app.units().iter().enumerate() {
        let mark = if i == app.selected { "▸" } else { " " };
        println!(
            "  {mark} {:<44} {:<12} {}{}",
            unit.title,
            unit.harness,
            unit.marker(),
            unit.status().to_uppercase()
        );
    }
    println!("\n── Weft ──");
    println!("  focus {:?} · modal {:?}", app.focus, app.modal.as_ref().map(kind_of));
    if let Some(weft::app::Modal::Ask { text, .. }) = app.modal.as_ref() {
        println!("  the ask box holds: {text:?}");
    }
    if let Some(weft::app::Modal::Confirm(p)) = app.modal.as_ref() {
        println!("  about to type: {:?}", String::from_utf8_lossy(&p.payload));
    }
    if let Some(hint) = app.hint_text() {
        println!("  hint: {hint}");
    }
    if let Some(refusal) = app.last_refusal() {
        println!("  Weft did not type it: {refusal}");
    }
    match app.waiting(0) {
        Some(e) => println!("  the agent is waiting (from the screen): {} · {}", e.rule, e.line),
        None => println!("  the agent is not waiting, as far as the screen shows"),
    }
    println!("\n── the agent's pane ──\n{}", app.pane_text(0).unwrap_or_default().trim_end());

    if stop {
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))?;
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))?;
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))?;
        println!("\nthe session and its agents are stopped");
    }
    Ok(())
}

fn kind_of(modal: &weft::app::Modal) -> &'static str {
    use weft::app::Modal::*;
    match modal {
        Quit => "Quit",
        Help => "Help",
        Ask { .. } => "Ask",
        Confirm(_) => "Confirm",
        Note(_) => "Note",
        StartAgent { .. } => "StartAgent",
        SetUp { .. } => "SetUp",
        Ended { .. } => "Ended",
    }
}

fn key_of(name: &str) -> KeyCode {
    match name {
        "Enter" => KeyCode::Enter,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Tab" => KeyCode::Tab,
        "Esc" => KeyCode::Esc,
        "Backspace" => KeyCode::Backspace,
        other => KeyCode::Char(other.chars().next().unwrap_or(' ')),
    }
}

/// `\r`, `\n`, `\e` and `\\`, so a chooser can be answered from a shell.
fn unescape(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buffer = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        match chars.next() {
            Some('r') => out.push(b'\r'),
            Some('n') => out.push(b'\n'),
            Some('e') => out.push(0x1b),
            Some('t') => out.push(b'\t'),
            Some(other) => out.push(other as u8),
            None => out.push(b'\\'),
        }
    }
    out
}

fn attach(root: &std::path::Path) -> anyhow::Result<Session> {
    let socket = weft::protocol::private_socket("weft-step");
    if !weft::protocol::is_live(&socket) {
        weft::protocol::clear_dead(&socket);
        std::process::Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/target/debug/weft"))
            .arg("--serve")
            .arg(root)
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
