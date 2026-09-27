//! A real harness quits from inside. Does Weft notice and close its pane?
//!
//!   cargo run --example end_probe -- codex /path/to/a/workspace
//!
//! Run by hand: no model call is made and nothing about the model is
//! claimed. What is under test is Weft's own exit detection against the harness
//! a person actually has installed.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use weft::app::App;
use weft::client::Session;
use weft::keys::Toggle;
use weft::protocol;
use weft::server;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "codex".into());
    let root = PathBuf::from(args.next().unwrap_or_else(|| ".".into())).canonicalize()?;
    let all = weft::harness::installed(&weft::routing::file().with_file_name("harnesses")).0;
    let h = all.find(&name).expect("a harness Weft has a file for").clone();

    let socket = protocol::private_socket("weft-end");
    let _ = std::fs::remove_file(&socket);
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &weft::routing::file());
    });
    while !protocol::is_live(&socket) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let session = Session::connect(&socket, &root, 40, 120)?;
    let mut app = App::with_session(root.clone(), Toggle, session);
    app.add(&name, &h.program)?;
    app.settle();
    println!("started {name}: {} pane(s)", app.pane_count());

    // Let it come up, then quit it the way a person would.
    settle(&mut app, Duration::from_secs(8));
    for quit in [&b"/quit\r"[..], &[3][..], &[4][..]] {
        app.input(0, quit)?;
        settle(&mut app, Duration::from_secs(3));
        if app.pane_count() == 0 {
            break;
        }
    }

    if app.pane_count() == 0 {
        println!("noticed: {name} has ended and its pane is closed");
    } else {
        println!("NOT noticed within the budget");
    }
    Ok(())
}

/// Pump until the exit is noticed, or the budget runs out.
fn settle(app: &mut App, budget: Duration) {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        app.pump();
        app.notice_an_agent_that_ended();
        if app.pane_count() == 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
