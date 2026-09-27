use anyhow::Result;
use crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste};
use crossterm::execute;

use weft::app::App;
use weft::client::Session;
use weft::keys;
use weft::protocol;
use weft::server;

/// The first screen when no directory was named. It opens nothing: the
/// daemon is not asked for anything until there is a project to ask about.
/// It offers the projects a daemon has open, from Weft's own
/// `projects.json`, which is read and never written here.
fn choose_project(terminal: &mut ratatui::DefaultTerminal) -> Result<Option<std::path::PathBuf>> {
    use crossterm::event::{self, Event, KeyCode};
    use ratatui::text::Line;
    use ratatui::widgets::Paragraph;

    let offered = offered_projects(
        &std::fs::read_to_string(weft::routing::file().with_file_name("projects.json"))
            .unwrap_or_default(),
    );
    let mut typed = String::new();
    let mut picked = 0usize;
    let mut said: Option<String> = None;
    loop {
        let lines = picker_lines(&typed, said.as_deref(), &offered, picked);
        terminal.draw(|frame| {
            let lines: Vec<Line> = lines.iter().map(|l| Line::raw(l.clone())).collect();
            frame.render_widget(Paragraph::new(lines), frame.area());
        })?;
        let Event::Key(key) = event::read()? else { continue };
        if !key.is_press() {
            continue;
        }
        match key.code {
            KeyCode::Up => picked = picked.saturating_sub(1),
            KeyCode::Down => picked = (picked + 1).min(offered.len().saturating_sub(1)),
            KeyCode::Enter if typed.trim().is_empty() => {
                if let Some(root) = offered.get(picked) {
                    return Ok(Some(root.clone()));
                }
            }
            KeyCode::Enter => {
                let want = typed.trim();
                let want = match want.strip_prefix('~') {
                    Some(rest) => match std::env::var_os("HOME") {
                        Some(home) => format!("{}{rest}", home.to_string_lossy()),
                        None => want.to_string(),
                    },
                    None => want.to_string(),
                };
                match std::path::PathBuf::from(&want).canonicalize() {
                    Ok(root) if root.is_dir() => return Ok(Some(root)),
                    _ => said = Some("There is no directory there.".into()),
                }
            }
            KeyCode::Backspace => {
                typed.pop();
            }
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('x' | 'X') if typed.is_empty() => return Ok(None),
            KeyCode::Char(c) => typed.push(c),
            _ => {}
        }
    }
}

/// The projects `projects.json` lists that are still directories here.
fn offered_projects(text: &str) -> Vec<std::path::PathBuf> {
    weft::projects::read(text).into_iter().map(|p| p.path).filter(|p| p.is_dir()).collect()
}

/// What the first screen says: the path being typed, and the projects open
/// in Weft to pick from instead.
fn picker_lines(
    typed: &str,
    said: Option<&str>,
    offered: &[std::path::PathBuf],
    picked: usize,
) -> Vec<String> {
    let mut lines = vec![
        " ▚▞ WEFT".to_string(),
        String::new(),
        "   Open a project".into(),
        String::new(),
        format!("   {typed}█"),
        String::new(),
        "   The path of a repository to work in.".into(),
    ];
    if !offered.is_empty() {
        lines.push(String::new());
        lines.push("   Or one open in Weft:".into());
        for (i, p) in offered.iter().enumerate() {
            let mark = if i == picked && typed.trim().is_empty() { "▸" } else { " " };
            lines.push(format!("   {mark} {}", p.display()));
        }
    }
    if let Some(why) = said {
        lines.push(String::new());
        lines.push(format!("   {why}"));
    }
    lines.push(String::new());
    lines.push(if offered.is_empty() {
        "  [Enter] OPEN   [X] QUIT".into()
    } else {
        "  [↑↓] PICK   [Enter] OPEN   [X] QUIT".into()
    });
    lines
}

/// Stop capturing the mouse and pastes, which `ratatui::restore` leaves on.
fn release(out: &mut impl std::io::Write) {
    let _ = execute!(out, DisableBracketedPaste, DisableMouseCapture);
}

/// Put the starting `config.toml` where the person can see and change it,
/// once. The daemon only reads it; this runs before it starts.
fn write_config_if_absent(path: &std::path::Path) {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, weft::routing::STARTING);
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let first = args.next();

    // Started by a client to own the panes. One per machine, headless, and it
    // outlives whatever asked for it.
    if first.as_deref() == Some("--serve") {
        write_config_if_absent(&weft::routing::file());
        return server::Session::serve(&protocol::socket_path(), &weft::routing::file());
    }

    if first.as_deref() == Some("stop") {
        let root = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()))
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        let socket = protocol::socket_path();
        if protocol::is_live(&socket) {
            let mut s = Session::connect(&socket, &root, 24, 80)?;
            s.shutdown();
            println!("weft: stopped the session and its agents");
        } else {
            println!("weft: nothing is running here");
        }
        return Ok(());
    }

    if matches!(first.as_deref(), Some("--version" | "-V")) {
        println!("weft {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if first.as_deref() == Some("update") {
        return update();
    }

    if matches!(first.as_deref(), Some("--help" | "-h")) {
        println!("weft {}\n", env!("CARGO_PKG_VERSION"));
        println!("weft [project-dir] [agent ...]\n");
        println!("  project-dir   the repository to work in (default: .)");
        println!("  agent         a harness's program, as its harness file names it;");
        println!("                optional, for scripts and probes\n");
        println!("An agent may carry its own arguments, quoted as one word:\n");
        println!("  weft . \"<program> --its --own --arguments\"\n");
        println!("Weft passes them through untouched. It never chooses a model,");
        println!("and starts no agent unless you name one or press N.\n");
        println!("Agents run in a background session that outlives this window.");
        println!("Closing Weft leaves them working; `weft stop` ends them.\n");
        println!("`weft update` installs the latest Weft and RingFrame.");
        return Ok(());
    }

    let named = first.clone();
    // Weft starts no agent on its own. Named agents are a convenience for
    // scripts and probes; the ordinary way in is to press N.
    let agents: Vec<String> = args.collect();

    let toggle = keys::Toggle;

    let mut terminal = ratatui::init();
    // A panic lets go of the mouse and pastes too, then restores as ratatui's
    // own hook does.
    let restore = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        release(&mut std::io::stdout());
        restore(info);
    }));
    let mut out = std::io::stdout();
    // The mouse is captured while Weft runs; see `App::run`.
    let _ = execute!(out, EnableBracketedPaste);

    // Named a directory, or asked for one. Nothing is opened and no daemon
    // work is done until there is one.
    let root = match named {
        Some(path) => Some(
            std::path::PathBuf::from(path)
                .canonicalize()
                .unwrap_or_else(|_| std::path::PathBuf::from(".")),
        ),
        None => choose_project(&mut terminal)?,
    };
    let Some(root) = root else {
        release(&mut out);
        ratatui::restore();
        return Ok(());
    };

    let size = terminal.size().unwrap_or_default();
    let session = Session::open(&root, size.height.max(24), size.width.max(80))?;
    let mut weft = App::with_session(root, toggle, session);
    weft.look_for_updates();
    let (newer, wake) = (weft.newer.clone(), weft.waker());
    std::thread::spawn(move || {
        if let Some(tag) = latest_release().filter(|t| is_newer(t)) {
            let _ = newer.set(tag.trim_start_matches('v').to_string());
            let _ = wake.send(weft::client::Wake::Arrived);
        }
    });
    let mut started = Ok(());
    for agent in &agents {
        let program = agent.split_whitespace().next().unwrap_or(agent);
        // The harness this program starts, as RingFrame's profiles say.
        let harness = weft.harness_for_program(program).unwrap_or_else(|| program.to_string());
        if let Err(e) = weft.add(&harness, agent) {
            started = Err(e);
            break;
        }
    }

    let result = started.and_then(|()| weft.run(&mut terminal));

    release(&mut out);
    ratatui::restore();

    if let Err(e) = &result {
        eprintln!("weft: {e}");
    }
    result
}

const REPO: &str = "fab7hq/weft";

/// The latest published release's tag, or nothing when it cannot be reached.
fn latest_release() -> Option<String> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "10"])
        .arg(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .output()
        .ok()?;
    let release: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    release["tag_name"].as_str().map(str::to_string)
}

fn is_newer(tag: &str) -> bool {
    let parts = |s: &str| s.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parts(tag.trim_start_matches('v')) > parts(env!("CARGO_PKG_VERSION"))
}

/// Replace this Weft with the latest release. Restarting is the person's.
fn update() -> Result<()> {
    let now = env!("CARGO_PKG_VERSION");
    let Some(tag) = latest_release() else {
        anyhow::bail!("could not reach the latest release");
    };
    if !is_newer(&tag) {
        println!("weft {now} is the latest");
        return Ok(());
    }
    println!("weft {now} → {tag}");
    let installer =
        format!("curl -fsSL https://raw.githubusercontent.com/{REPO}/main/install.sh | sh");
    if !std::process::Command::new("sh").args(["-c", &installer]).status()?.success() {
        anyhow::bail!("the installer did not finish");
    }
    println!("\nRestart Weft to use it: `weft stop`, then `weft`.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the panic hook writes before the terminal is restored: mouse
    /// capture and bracketed paste off, which `ratatui::restore` leaves on.
    #[test]
    fn letting_go_of_the_terminal_turns_off_the_mouse_and_pastes() {
        let mut out = Vec::new();
        release(&mut out);
        let said = String::from_utf8(out).expect("escapes");
        assert!(said.contains("\x1b[?1000l"), "mouse capture: {said:?}");
        assert!(said.contains("\x1b[?2004l"), "bracketed paste: {said:?}");
    }

    #[test]
    fn the_picker_offers_the_projects_open_in_weft() {
        let here = std::env::temp_dir().canonicalize().expect("a directory");
        let open = [weft::projects::Opened { path: here.clone(), opened: 1 }];
        let gone = weft::projects::Opened { path: "/no/such/place".into(), opened: 2 };
        let text = weft::projects::write(&[open[0].clone(), gone]);
        let offered = offered_projects(&text);
        assert_eq!(offered, std::slice::from_ref(&here), "one that is gone is not offered");
        let drawn = picker_lines("", None, &offered, 0).join("\n");
        assert!(drawn.contains(&format!("▸ {}", here.display())), "{drawn}");
        assert!(drawn.contains("[↑↓] PICK"), "{drawn}");
        let typing = picker_lines("/else", None, &offered, 0).join("\n");
        assert!(!typing.contains('▸'), "a typed path is what Enter opens: {typing}");
        assert!(!picker_lines("", None, &[], 0).join("\n").contains("PICK"));
    }

    #[test]
    fn the_starting_config_is_written_once_and_then_left_alone() {
        let dir = std::env::temp_dir().join(format!("weft-starting-config-{}", std::process::id()));
        let path = dir.join("weft").join("config.toml");
        write_config_if_absent(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), weft::routing::STARTING);
        std::fs::write(&path, "[routing]\nask = \"codex\"\n").unwrap();
        write_config_if_absent(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[routing]\nask = \"codex\"\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
