//! With no `ringframe` on `PATH`, panes still run and the board says plainly
//! that there is no record to read. Gate W1, "prerequisite absent".
//!
//! Its own test binary because it edits the process environment, which is
//! global: one test, one process, no race with anything else.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use weft::app::App;
use weft::client::Session;
use weft::keys::Toggle;
use weft::protocol;
use weft::server;

#[test]
fn without_the_cli_the_agents_still_run_and_the_board_says_why_it_is_empty() {
    // SAFETY: this binary holds exactly one test, so nothing else is reading
    // the environment while it is changed.
    unsafe {
        std::env::set_var("PATH", "");
    }

    let root = std::env::temp_dir().join(format!("weft-no-rf-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("root");
    let socket = protocol::private_socket("weft-no-rf");
    let _ = std::fs::remove_file(&socket);
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
    });
    let session = Session::connect(&socket, &root, 24, 80).expect("connect");
    let mut app = App::with_session(root.clone(), Toggle, session);

    app.add("codex", "/bin/cat").expect("a pane still runs");
    assert_eq!(app.pane_count(), 1, "the runtime does not depend on the CLI");
    // Whether the CLI is there is the daemon's answer, and it asks when a pane
    // starts — because asking costs a process, and opening a project must not.
    assert!(!app.record_available(), "no ringframe is on PATH here");

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal.draw(|frame| weft::ui::draw(frame, &mut app)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let drawn: String = (0..24)
        .map(|y| {
            (0..80)
                .map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Said plainly, and said about the record rather than about the agent:
    // the pane above is running, and the board below it is empty because
    // there is nothing writing one, not because nothing has happened.
    assert!(drawn.contains("RingFrame is not installed."), "{drawn}");
    assert!(drawn.contains("Nothing is written down"), "{drawn}");
    assert!(drawn.contains("codex"), "the agent is still on the tab row: {drawn}");
    std::fs::remove_dir_all(&root).ok();
}
