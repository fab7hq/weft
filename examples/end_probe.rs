//! A real harness quits from inside. Does Weft notice, and can it name the
//! session to open again?
//!
//!   cargo run --example end_probe -- codex /path/to/a/workspace
//!
//! Run by hand: no model call is made and nothing about the model is
//! claimed. What is under test is Weft's own mechanism — exit detection, the
//! receipt read, and the command it would run — against the harness a person
//! actually has installed.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use weft::app::{App, Modal};
use weft::client::Session;
use weft::keys::Toggle;
use weft::protocol;
use weft::server;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "codex".into());
    let root = PathBuf::from(args.next().unwrap_or_else(|| ".".into())).canonicalize()?;
    let h = weft::harness::find(&name).expect("a supported harness");

    let socket = protocol::private_socket("weft-end");
    let _ = std::fs::remove_file(&socket);
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening);
    });
    let session = Session::connect(&socket, &root, 40, 120)?;
    let mut app = App::with_session(root.clone(), Toggle, session);
    app.add(&name, h.program)?;
    println!("started {name}: {} pane(s)", app.pane_count());

    // Let it come up, then quit it the way a person would.
    settle(&mut app, Duration::from_secs(8));
    for quit in [&b"/quit\r"[..], &[3][..], &[4][..]] {
        app.input(0, quit)?;
        settle(&mut app, Duration::from_secs(3));
        if app.modal.is_some() {
            break;
        }
    }

    match app.modal.clone() {
        Some(Modal::Ended { harness, session, .. }) => {
            println!("noticed: {harness} has ended");
            match session {
                Some(s) => println!(
                    "would run: {}\n  (last prompt {} at {})",
                    h.resume_spec(&s.id),
                    s.last,
                    s.at
                ),
                None => println!("no session on record here, so nothing is offered to resume"),
            }
        }
        other => println!("NOT noticed within the budget; modal was {other:?}"),
    }
    Ok(())
}

/// Pump until the exit is noticed, or the budget runs out.
fn settle(app: &mut App, budget: Duration) {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        app.pump();
        app.notice_an_agent_that_ended();
        if app.modal.is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
