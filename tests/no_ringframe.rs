//! With no `ringframe` on `PATH`, panes still run and the board says plainly
//! what RingFrame needs before it can be set up here: ringframe first. Gate
//! W1, "prerequisite absent".
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
fn without_the_cli_the_agents_still_run_and_the_board_says_what_is_missing() {
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !protocol::is_live(&socket) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let session = Session::connect(&socket, &root, 32, 120).expect("connect");
    let mut app = App::with_session(root.clone(), Toggle, session);
    // Whether the CLI is there is the daemon's answer: it asked RingFrame to
    // set the project up as it opened, and nothing ran.
    let needs = app.needs().expect("not set up");
    assert!(!needs.ringframe, "no ringframe is on PATH here");

    app.add("codex", "/bin/cat").expect("a pane still runs");
    app.settle();
    assert_eq!(app.pane_count(), 1, "the runtime does not depend on the CLI");

    let mut terminal = Terminal::new(TestBackend::new(120, 32)).expect("terminal");
    terminal.draw(|frame| weft::ui::draw(frame, &mut app)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let drawn: String = (0..32)
        .map(|y| {
            (0..120)
                .map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Said plainly, and said about the record rather than about the agent:
    // the agent beside it is running, and the board is empty because nothing
    // is writing one, not because nothing has happened.
    assert!(drawn.contains("RingFrame is not set up here yet."), "{drawn}");
    assert!(drawn.contains("✗ ringframe is installed"), "{drawn}");
    assert!(drawn.contains("codex"), "the agent is still on the tab row: {drawn}");
    std::fs::remove_dir_all(&root).ok();
}
