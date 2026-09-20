//! Print each v2 screen as it actually renders, to read beside the drawings.
//!
//!   cargo run --example wireframe
//!
//! No harness is started: the panes run `/bin/cat`, and the work comes from a
//! fixture ledger in a scratch directory, so the screens are reproducible.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use weft::app::App;
use weft::client::Session;
use weft::keys::Toggle;
use weft::ledger::{Check, Sent, Unit, Verdict};
use weft::readiness::{Gap, Readiness};
use weft::server;

fn main() {
    show("Screen 1 — first run", 80, 24, &[], false);
    show("Screen 2 — work, one row expanded", 80, 24, &[KeyCode::Enter], true);
    show("Screen 3 — in the agent", 80, 24, &[KeyCode::Char(']')], true);
    show("Screen 4 — side by side", 120, 32, &[], true);
    show("Screen 5 — the drawer", 120, 32, &[KeyCode::Char('j')], true);
    show("Screen 6 — ask", 120, 32, &[KeyCode::Char('a')], true);
    show("Screen 7 — before Weft types", 80, 24, &[KeyCode::Down, KeyCode::Char('c')], true);
    show("Screen 9 — quit", 80, 24, &[KeyCode::Char('x')], true);
    show("Screen 10 — work list hidden", 120, 32, &[KeyCode::Char('w')], true);
    unready("Readiness A — something is missing", 80, 24, &[], Gap::Plugin);
    unready("Readiness B — the CLI itself", 80, 24, &[KeyCode::Char('r')], Gap::Cli);
    unready("Readiness C — the setup proposal", 80, 24, &[KeyCode::Char('r')], Gap::Plugin);
}

/// The same screens with the agent not set up for RingFrame.
fn unready(name: &str, width: u16, height: u16, keys: &[KeyCode], gap: Gap) {
    let mut app = fixture(name, true);
    app.set_units(Vec::new());
    app.set_readiness("codex", Readiness::Missing(gap));
    draw_it(name, width, height, keys, &mut app);
}

fn show(name: &str, width: u16, height: u16, keys: &[KeyCode], with_agent: bool) {
    let mut app = fixture(name, with_agent);
    draw_it(name, width, height, keys, &mut app);
}

fn draw_it(name: &str, width: u16, height: u16, keys: &[KeyCode], app: &mut App) {
    for key in keys {
        let modifiers = if *key == KeyCode::Char(']') {
            KeyModifiers::CONTROL
        } else {
            KeyModifiers::NONE
        };
        app.on_key(KeyEvent::new(*key, modifiers)).expect("key");
    }
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| weft::ui::draw(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    println!("\n=== {name} · {width}×{height} ===");
    for y in 0..height {
        let row: String = (0..width)
            .map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
            .collect();
        println!("{}", row.trim_end());
    }
}

fn fixture(name: &str, with_agent: bool) -> App {
    // The last path element is the project name Weft shows in the title bar.
    let root = std::env::temp_dir()
        .join(format!(
            "weft-wireframe-{}-{}",
            std::process::id(),
            name.chars().filter(char::is_ascii_alphanumeric).collect::<String>()
        ))
        .join("test");
    std::fs::create_dir_all(&root).expect("root");
    write_record(&root);

    let socket = server::socket_path(&root);
    let _ = std::fs::remove_file(&socket);
    let serving = root.clone();
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(serving, &listening);
    });
    let session = Session::connect(&socket, 24, 80).expect("connect");
    let mut app = App::with_session(root, Toggle, session);
    if with_agent {
        app.add("codex", "/bin/cat").expect("a pane");
        app.add("claude-code", "/bin/cat").expect("a pane");
        app.set_units(units());
    }
    app
}

fn write_record(root: &PathBuf) {
    let dir = root.join(".fab7/rf/evals/evl_1");
    std::fs::create_dir_all(&dir).expect("evals");
    std::fs::write(
        dir.join("record.json"),
        serde_json::json!({
            "eval_id": "evl_1",
            "verdict": "drifted",
            "confidence": 0.67,
            "judgements": [
                {"judge": {"angle": "coverage", "host": "codex", "model": "gpt-5.6-luna"}},
                {"judge": {"angle": "drift", "host": "codex", "model": "gpt-5.6-luna"}},
                {"judge": {"angle": "adversary", "host": "codex", "model": "gpt-5.6-luna"}}
            ],
            "items": [
                {"id": "i1", "status": "active", "text": "returns the real build number",
                 "majority": "no", "agreement": 1.0,
                 "votes": [{"angle": "drift", "vote": "no",
                            "reason": "it still reads a literal \"dev\" at line 41"},
                           {"angle": "coverage", "vote": "no",
                            "reason": "no version lookup was added to health()"}]},
                {"id": "i2", "status": "active", "text": "the endpoint responds at /health",
                 "majority": "yes", "agreement": 1.0,
                 "votes": [{"angle": "coverage", "vote": "yes",
                            "reason": "src/server.js keeps the /health branch and returns status ok"}]},
                {"id": "i3", "status": "active", "text": "reverts the README change",
                 "majority": "unknown", "agreement": 0.67,
                 "votes": [{"angle": "adversary", "vote": "unknown",
                            "reason": "README.md is in the diff; the change is unrelated to the ask"}]}
            ],
            "drift": {"commission": [{"path": "README.md"}]}
        })
        .to_string(),
    )
    .expect("record");
}

fn units() -> Vec<Unit> {
    let base = |id: &str, title: &str, harness: &str, sent: Sent| Unit {
        ask_id: id.into(),
        title: title.into(),
        harness: harness.into(),
        route: "native_plan".into(),
        asked_at: "2026-09-19T14:02:00Z".into(),
        delivery_mode: "human_handoff".into(),
        cancelled: false,
        confirmed: true,
        sent,
        check: None,
        sealed: None,
    };
    let mut checked = base("ask_1", "health endpoint", "codex", Sent::Arrived { exact: true });
    checked.check = Some(Check {
        eval_id: "evl_1".into(),
        verdict: Verdict::DoesntMatch,
        agreement: 0.67,
        judged_by: Some("codex".into()),
    });
    let mut sealed = base("ask_4", "logging cleanup", "claude-code", Sent::Arrived { exact: true });
    sealed.sealed = Some("accepted".into());
    vec![
        checked,
        base("ask_2", "readme fix", "claude-code", Sent::ReadyToSend),
        base("ask_3", "auth refactor", "codex", Sent::TakenByAgent),
        sealed,
    ]
}
