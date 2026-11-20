//! `weft-desktop` binary entrypoint.

use std::path::PathBuf;

use weft_desktop::app::WeftDesktopApp;
use weft_desktop::instance;
use weft_desktop::window::run_interactive;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("weft-desktop {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let project = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .filter_map(|a| PathBuf::from(a).canonicalize().ok())
        .find(|dir| dir.is_dir());
    // One app: one already running opens the project and comes in front.
    if instance::hand_over(&instance::socket_path(), project.as_deref()) {
        return;
    }

    // This version stays while it runs, whatever version the daemon moves to.
    let _held = weft_desktop::version::hold_own();
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    // The daemon names the machine once it answers.
    let this_machine = weft_core::words::word("desktop.this_machine");
    let mut app = WeftDesktopApp::new(home, this_machine);
    if let Some(dir) = project {
        app.open_project_path(dir);
    }

    if let Err(err) = run_interactive(app) {
        eprintln!("weft-desktop: {err}");
        std::process::exit(1);
    }
}
