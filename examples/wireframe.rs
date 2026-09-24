//! Print each v2 screen as it actually renders, to read beside the drawings.
//!
//!   cargo run --example wireframe
//!
//! No harness is started: the panes run `/bin/cat`, and the work comes from a
//! fixture ledger in a scratch directory, so the screens are reproducible.

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use weft::app::App;
use weft::client::Session;
use weft::keys::Toggle;
use weft::ledger::{Check, Sent, Unit, Verdict};
use weft::protocol;
use weft::readiness::{Gap, Readiness};
use weft::server;

fn main() {
    show("Screen 1 — a project opened, no agent", 80, 24, &[], false);
    show("Screen 2 — the sidebar", 80, 24, &[], true);
    show("Screen 2b — a project folded", 80, 24, &[KeyCode::Up, KeyCode::Up, KeyCode::Enter], true);
    show("Screen 3 — in the agent", 80, 24, &[KeyCode::Char(']')], true);
    show("Screen 4 — side by side", 120, 32, &[], true);
    show("Screen 5 — the detail view", 120, 32, &[KeyCode::Enter], true);
    show("Screen 6 — ask", 120, 32, &[KeyCode::Char('a')], true);
    follow_up("Screen 6b — follow up", 120, 32);
    gathered("Screen 5b — an Eval gathered, its debate next", 120, 32);
    show("Screen 7 — before Weft types", 80, 24, &[KeyCode::Char('e')], true);
    show("Screen 8 — the Weft menu", 80, 24, &[KeyCode::Char('w')], true);
    show("Screen 9 — quit", 80, 24, &[KeyCode::Char('x')], true);
    show("Screen 10 — sidebar hidden", 120, 32, &[KeyCode::Char('b')], true);
    show("Screen 10b — help", 80, 24, &[KeyCode::Char('h')], true);
    show("Screen 11 — no agent running", 120, 32, &[], false);
    pick_up("Screen 12 — picking the work back up", 80, 24, true);
    pick_up("Screen 12b — nothing on record to resume", 80, 24, false);
    unready("Readiness A — something is missing", 80, 24, &[], Gap::Plugin);
    unready("Readiness B — sync RingFrame", 80, 24, &[KeyCode::Char('u')], Gap::Plugin);
}

/// Work whose agent is not running. What Weft can offer depends on whether
/// the record names a session, so both are drawn.
fn pick_up(name: &str, width: u16, height: u16, on_record: bool) {
    let mut app = fixture(name, false);
    app.modal = Some(weft::app::Modal::PickUp {
        harness: "codex".into(),
        session: on_record.then(|| weft::sessions::Recorded {
            id: "01a0bdb6-1d1f-79c2-84b0-8b03496d7db0".into(),
            at: "2026-09-20T07:27:14.769Z".into(),
            last: "research crypto trading".into(),
        }),
        then: None,
    });
    draw_it(name, width, height, &[], &mut app);
}

/// A new Ask about work already on the board: it carries that work's id.
fn follow_up(name: &str, width: u16, height: u16) {
    let mut app = fixture(name, true);
    app.modal = Some(weft::app::Modal::Ask {
        text: "read the build number from package.json".into(),
        target: 0,
        follows: Some(weft::app::Follows {
            ask_id: "ask_1".into(),
            title: "health endpoint".into(),
            carries: "evl_1".into(),
        }),
    });
    draw_it(name, width, height, &[], &mut app);
}

/// An Eval split across harnesses, after its first stage: Codex mapped the
/// change, and the debate goes to Claude Code.
fn gathered(name: &str, width: u16, height: u16) {
    let mut app = fixture(name, true);
    let mut units = units();
    units[0].check = None;
    units[0].gathered =
        Some(weft::ledger::Gathered { eval_id: "evl_2".into(), by: "codex".into() });
    app.set_units(units);
    app.set_routing(weft::routing::read(
        r#"{"/p": {"eval": {"gather": {"harness": "codex"}, "debate": {"harness": "claude-code"}}}}"#,
        Path::new("/p"),
    ));
    draw_it(name, width, height, &[KeyCode::Enter], &mut app);
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
        let modifiers =
            if *key == KeyCode::Char(']') { KeyModifiers::CONTROL } else { KeyModifiers::NONE };
        app.on_key(KeyEvent::new(*key, modifiers)).expect("key");
    }
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| weft::ui::draw(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    println!("\n=== {name} · {width}×{height} ===");
    for y in 0..height {
        let row: String =
            (0..width).map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" ")).collect();
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

    let socket = protocol::private_socket("weft-wireframe");
    let _ = std::fs::remove_file(&socket);
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening);
    });
    let session = Session::connect(&socket, &root, 24, 80).expect("connect");
    let mut app = App::with_session(root, Toggle, session);
    if with_agent {
        app.add("codex", "/bin/cat").expect("a pane");
        app.add("claude-code", "/bin/cat").expect("a pane");
        app.set_units(units());
    }
    app
}

fn write_record(root: &Path) {
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
                 "votes": [{"angle": "drift", "vote": "no", "counted_as": "no",
                            "reason": "it still reads a literal \"dev\" at line 41"},
                           {"angle": "coverage", "vote": "no", "counted_as": "no",
                            "reason": "no version lookup was added to health()"},
                           {"angle": "adversary", "vote": "no", "counted_as": "no",
                            "reason": "health() is unchanged in the diff"}]},
                {"id": "i2", "status": "active", "text": "the endpoint responds at /health",
                 "majority": "yes", "agreement": 1.0,
                 "votes": [{"angle": "coverage", "vote": "yes", "counted_as": "yes",
                            "reason": "src/server.js keeps the /health branch and returns status ok"},
                           {"angle": "drift", "vote": "yes", "counted_as": "yes",
                            "reason": "the route still answers in src/server.js"},
                           {"angle": "adversary", "vote": "yes", "counted_as": "yes",
                            "reason": "npm test covers it"}]},
                {"id": "i3", "status": "active", "text": "reverts the README change",
                 "majority": "unknown", "agreement": 0.67,
                 "votes": [{"angle": "adversary", "vote": "unknown", "counted_as": "unknown",
                            "reason": "README.md is in the diff; the change is unrelated to the ask"},
                           {"angle": "drift", "vote": "unknown", "counted_as": "unknown",
                            "reason": "cannot tie README.md to the ask"},
                           {"angle": "coverage", "vote": "yes", "counted_as": "unknown",
                            "uncited": true, "reason": "looks reverted"}]}
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
        session_ref: Some("01a0bdb6-1d1f-79c2-84b0-8b03496d7db0".into()),
        delivery: Default::default(),
        route: "native_plan".into(),
        asked_at: "2026-09-19T14:02:00Z".into(),
        delivery_mode: "human_handoff".into(),
        cancelled: false,
        unanswered: false,
        confirmed: true,
        sent,
        check: None,
        gathered: None,
        sealed: None,
        seal_id: None,
        sealed_at: None,
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
    sealed.seal_id = Some("sel_1".into());
    sealed.sealed_at = Some("2026-09-19T16:40:00Z".into());
    vec![
        checked,
        base("ask_2", "readme fix", "claude-code", Sent::ReadyToSend),
        base("ask_3", "auth refactor", "codex", Sent::TakenByAgent),
        sealed,
    ]
}
