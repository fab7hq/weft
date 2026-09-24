//! What this project routes, and where each act would go.
//!
//!   cargo run --example routing_probe -- /path/to/project
use weft::app::{Act, App};
use weft::client::Session;
use weft::keys::Toggle;
use weft::protocol;
use weft::server;

fn main() -> anyhow::Result<()> {
    let root = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()))
        .canonicalize()?;
    println!("config file: {}", weft::routing::file().display());
    let r = weft::routing::read_at(&weft::routing::file()).routing(&root);
    println!("routes: {:?}  unknown: {:?}", r.each(), r.unknown);

    let socket = protocol::private_socket("weft-routing");
    let _ = std::fs::remove_file(&socket);
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &weft::routing::file());
    });
    let session = Session::connect(&socket, &root, 24, 80)?;
    let mut app = App::with_session(root, Toggle, session);
    app.refresh_for_test();
    // Readiness is normally asked when a pane starts; this probe starts none.
    let cli = weft::ringframe::installed();
    for h in weft::harness::SUPPORTED {
        app.set_readiness(h.name, weft::readiness::check(h, cli));
    }
    for act in [Act::Ask, Act::Eval, Act::Seal, Act::Proceed] {
        println!(
            "{act:?}  -> {:?}   unavailable: {:?}",
            app.deciding_harness(act),
            app.unavailable(act)
        );
    }
    Ok(())
}
