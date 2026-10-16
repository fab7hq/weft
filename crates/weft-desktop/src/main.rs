//! `weft-desktop` binary entrypoint.

use std::path::PathBuf;

use weft_desktop::app::WeftDesktopApp;
use weft_desktop::window::run_interactive;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("weft-desktop {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let machine = std::process::Command::new("scutil")
        .args(["--get", "LocalHostName"])
        .output()
        .ok()
        .and_then(|o| {
            o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "studio".to_string());

    let mut app = WeftDesktopApp::launch_deferred(home, machine);

    for arg in &args {
        if !arg.starts_with('-') {
            let p = PathBuf::from(arg);
            if let Ok(canon) = p.canonicalize()
                && canon.is_dir()
            {
                let norm = weft_core::state::normalize_project_path(&canon);
                if !app
                    .state
                    .open_projects
                    .iter()
                    .any(|e| weft_core::state::normalize_project_path(e) == norm)
                {
                    app.state.open_projects.push(norm.clone());
                }
                if !app
                    .clients
                    .iter()
                    .any(|c| weft_core::state::normalize_project_path(&c.root) == norm)
                {
                    let name =
                        norm.file_name().and_then(|n| n.to_str()).unwrap_or("project").to_string();
                    let mut c = weft_desktop::daemon::ProjectClient::disconnected(norm, name);
                    c.turbo = app.state.settings.turbo;
                    c.start_at_login = app.state.settings.start_at_login;
                    app.clients.push(c);
                }
                let _ = app.save_state();
            }
        }
    }

    if let Err(err) = run_interactive(app) {
        eprintln!("weft-desktop: {err}");
        std::process::exit(1);
    }
}
