//! What Weft shows when it is reopened in a workspace that has history.
//!
//!   cargo run --example reopen -- /path/to/project [keys]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use weft::app::App;
use weft::client::Session;
use weft::keys::Toggle;
use weft::protocol;
use weft::server;

fn main() -> anyhow::Result<()> {
    let root = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()))
        .canonicalize()?;
    let keys: Vec<char> = std::env::args().nth(2).unwrap_or_default().chars().collect();

    let socket = protocol::private_socket("weft-reopen");
    let _ = std::fs::remove_file(&socket);
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening);
    });
    let session = Session::connect(&socket, &root, 24, 80)?;
    let mut app = App::with_session(root, Toggle, session);
    app.refresh_for_test();
    for c in keys {
        let code = match c {
            'v' => KeyCode::Down,
            '^' => KeyCode::Up,
            '\n' | '=' => KeyCode::Enter,
            other => KeyCode::Char(other),
        };
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE))?;
    }
    let mut terminal = Terminal::new(TestBackend::new(80, 24))?;
    terminal.draw(|f| weft::ui::draw(f, &mut app))?;
    let buffer = terminal.backend().buffer().clone();
    for y in 0..24 {
        let row: String =
            (0..80).map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" ")).collect();
        println!("{}", row.trim_end());
    }
    Ok(())
}
