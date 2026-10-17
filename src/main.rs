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

    let offered = offered_projects(
        &std::fs::read_to_string(weft::routing::file().with_file_name("projects.json"))
            .unwrap_or_default(),
    );
    let mut typed = String::new();
    let mut picked = 0usize;
    let mut said: Option<String> = None;
    loop {
        let (lines, keys) = picker_lines(&typed, said.as_deref(), &offered, picked);
        terminal.draw(|frame| draw_picker(frame, &lines, &keys))?;
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

/// What the first screen says, under the window's mark: the path being
/// typed, and the projects open in Weft to pick from instead; and its keys.
fn picker_lines(
    typed: &str,
    said: Option<&str>,
    offered: &[std::path::PathBuf],
    picked: usize,
) -> (Vec<String>, String) {
    let mut lines = vec![
        String::new(),
        "   Open a project".into(),
        format!("   {typed}█"),
        "   The path of a repository to work in.".into(),
    ];
    if let Some(why) = said {
        lines.push(format!("   {why}"));
    }
    if !offered.is_empty() {
        lines.push(String::new());
        lines.push("   Or one open in Weft:".into());
        for (i, p) in offered.iter().enumerate() {
            let mark = if i == picked && typed.trim().is_empty() { "▸" } else { " " };
            lines.push(format!("   {mark} {}", p.display()));
        }
    }
    let keys = if offered.is_empty() {
        "  [Enter] OPEN   [X] QUIT"
    } else {
        "  [↑↓] PICK   [Enter] OPEN   [X] QUIT"
    };
    (lines, keys.into())
}

/// The picker: the mark and WEFT as the window draws them, its lines, and
/// its keys on the bar at the bottom, over the hint line.
fn draw_picker(frame: &mut ratatui::Frame, lines: &[String], keys: &str) {
    use ratatui::layout::{Constraint, Layout};
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;
    let th = weft::theme::Theme::default();
    let [body, rule, bar, _hint] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let lit = Style::default().fg(th.accent());
    let mut text = vec![Line::from(vec![
        Span::raw(" "),
        Span::styled("╸", lit),
        Span::styled("┃", Style::default().fg(th.warp())),
        Span::styled("╺ ", lit),
        Span::styled("WEFT", lit.add_modifier(Modifier::BOLD)),
    ])];
    text.extend(lines.iter().enumerate().map(|(i, l)| match i {
        1 => Line::styled(l.clone(), th.title()),
        3 => Line::styled(l.clone(), th.label()),
        _ => Line::raw(l.clone()),
    }));
    frame.render_widget(Paragraph::new(text), body);
    frame.render_widget(
        Paragraph::new(Line::styled("─".repeat(rule.width as usize), th.rule())),
        rule,
    );
    frame.render_widget(Paragraph::new(Line::styled(keys.to_string(), th.label())), bar);
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
    // Weft from another device: started by the daemon while remote access is
    // on (remote.md §5).
    // Every hook of every harness: answers the hook, tells the
    // daemon, and exits 0 whatever happens, so no hook fails a harness's turn.
    if first.as_deref() == Some("signal") {
        std::panic::set_hook(Box::new(|_| {}));
        let args: Vec<String> = args.collect();
        let _ = std::panic::catch_unwind(|| {
            let at = weftd::signal::Where::from_env();
            weftd::signal::run(&args, &mut std::io::stdin().lock(), &mut std::io::stdout(), &at)
        });
        std::process::exit(0);
    }
    if first.as_deref() == Some("remote") {
        // Its version is kept while it runs.
        let _held = weftd::versions::hold_own();
        return weft_remote::run();
    }
    if first.as_deref() == Some("--serve") {
        write_config_if_absent(&weft::routing::file());
        return server::Session::serve(&protocol::socket_path(), &weft::routing::file());
    }

    if first.as_deref() == Some("stop") {
        let socket = protocol::socket_path();
        if protocol::is_live(&socket) {
            protocol::stop(&socket)?;
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

    // No arguments: the desktop app, which starts or joins the daemon itself.
    if first.is_none() {
        return open_app();
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
        println!("`weft signal <harness> <hook>` is what a harness's hooks run.");
        return Ok(());
    }

    // This window's version is kept while it runs.
    let _held = weftd::versions::hold_own();
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

    // With "Start at login" on, the daemon is started through Weft's service,
    // so one owner keeps it running; else, or if that fails,
    // the window starts it itself.
    let socket = protocol::socket_path();
    if !protocol::is_live(&socket) {
        let service = weftd::service::Service::here();
        let _ = weftd::service::start_daemon(service.as_ref(), &socket);
    }
    let size = terminal.size().unwrap_or_default();
    // Refused or unreachable, the terminal is given back before saying so.
    let session = match Session::open(&root, size.height.max(24), size.width.max(80)) {
        Ok(session) => session,
        Err(e) => {
            release(&mut out);
            ratatui::restore();
            return Err(e);
        }
    };
    let mut weft = App::with_session(root, toggle, session);
    if let Some(home) = std::env::var_os("HOME") {
        weft.keep_layout_in(std::path::PathBuf::from(home).join(".fab7/weft/layout.toml"));
    }
    weft.look_for_updates();
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

    match result {
        // Weft's service is a newer Weft now: this window starts again as it,
        // in the same terminal, on the same project.
        Ok(Some(root)) => {
            use std::os::unix::process::CommandExt as _;
            let failed = std::process::Command::new("weft").arg(&root).exec();
            eprintln!("weft: Weft was updated; run weft again ({failed})");
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(e) => {
            eprintln!("weft: {e}");
            Err(e)
        }
    }
}

/// What `weft` with no arguments opens.
#[derive(Debug, PartialEq, Eq)]
enum Opens {
    /// Weft.app, which this `weft` is inside: opened as an app.
    Bundle(std::path::PathBuf),
    /// The desktop program beside this `weft`.
    Program(std::path::PathBuf),
}

/// The app that came with the `weft` at `exe`, if there is one.
fn app_to_open(exe: &std::path::Path) -> Option<Opens> {
    let dir = exe.parent()?;
    let desktop = dir.join("weft-desktop");
    if !desktop.is_file() {
        return None;
    }
    let bundle = dir.parent().and_then(std::path::Path::parent);
    match bundle {
        Some(b) if dir.ends_with("Contents/MacOS") && b.extension().is_some_and(|e| e == "app") => {
            Some(Opens::Bundle(b.to_path_buf()))
        }
        _ => Some(Opens::Program(desktop)),
    }
}

/// Open the desktop app beside this `weft`, and leave it running.
fn open_app() -> Result<()> {
    use std::os::unix::process::CommandExt as _;
    let exe = std::env::current_exe()?.canonicalize()?;
    let mut run = match app_to_open(&exe) {
        Some(Opens::Bundle(app)) => {
            let mut open = std::process::Command::new("/usr/bin/open");
            open.arg(app);
            open
        }
        Some(Opens::Program(desktop)) => {
            let mut program = std::process::Command::new(desktop);
            // Its own session, so closing this terminal leaves it open.
            unsafe {
                program.pre_exec(|| {
                    unsafe extern "C" {
                        fn setsid() -> i32;
                    }
                    setsid();
                    Ok(())
                });
            }
            program
        }
        None => anyhow::bail!(
            "the desktop app is not installed beside {}; install Weft again",
            exe.display()
        ),
    };
    run.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
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

/// A release's tag, and nothing else: `v`, then numbers and dots.
fn is_release_tag(tag: &str) -> bool {
    tag.strip_prefix('v')
        .is_some_and(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

/// Replace this Weft with the latest release, with that release's own
/// installer, pinned to it: never `main`'s, which no release has vouched for.
/// Restarting is the person's.
fn update() -> Result<()> {
    let now = env!("CARGO_PKG_VERSION");
    let Some(tag) = latest_release() else {
        anyhow::bail!("could not reach the latest release");
    };
    if !is_release_tag(&tag) {
        anyhow::bail!("the latest release is not tagged as one: {tag}");
    }
    if !is_newer(&tag) {
        println!("weft {now} is the latest");
        // Installed by hand, or by an earlier update: a service on an older
        // Weft is told to start on this one once no agent runs.
        let socket = protocol::socket_path();
        if protocol::is_live(&socket) {
            let _ = protocol::tell_update(&socket, now);
        }
        return Ok(());
    }
    println!("weft {now} → {tag}");
    // Downloaded whole before it runs, in a folder of the person's own.
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let dir = std::path::PathBuf::from(home).join(".fab7/weft/update");
    std::fs::create_dir_all(&dir)?;
    let script = dir.join("install.sh");
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/install.sh");
    let got = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "60", "-o"])
        .arg(&script)
        .arg(&url)
        .status()?;
    if !got.success() {
        let _ = std::fs::remove_dir_all(&dir);
        anyhow::bail!("{tag} publishes no installer");
    }
    let ran = std::process::Command::new("sh").arg(&script).env("WEFT_VERSION", &tag).status();
    let _ = std::fs::remove_dir_all(&dir);
    if !ran?.success() {
        anyhow::bail!("the installer did not finish");
    }
    println!("\n{}", after_update(&protocol::socket_path(), tag.trim_start_matches('v')));
    Ok(())
}

/// Tell a running daemon a newer Weft is installed beside it, which it starts
/// on once no agent runs; what to say to the person.
fn after_update(socket: &std::path::Path, version: &str) -> String {
    if !protocol::is_live(socket) {
        return format!("Weft {version} starts the next time you run weft.");
    }
    match protocol::tell_update(socket, version) {
        Ok(Ok(())) => format!(
            "Weft {version} is installed. It starts when no agent is running; nothing to restart."
        ),
        Ok(Err(why)) => format!("Weft {version} is installed, but Weft's service said: {why}"),
        Err(e) => format!("Weft {version} is installed; Weft's service could not be told ({e})."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the panic hook writes before the terminal is restored: mouse
    /// capture and bracketed paste off, which `ratatui::restore` leaves on.
    #[test]
    fn only_a_release_tag_is_updated_to() {
        for tag in ["v0.2.0", "v1.10.3"] {
            assert!(is_release_tag(tag), "{tag}");
        }
        for tag in ["0.2.0", "v", "v0.2.0;rm -rf ~", "v0.2.0-rc1", "main", "v0.2.0 "] {
            assert!(!is_release_tag(tag), "{tag:?}");
        }
    }

    #[test]
    fn with_no_arguments_weft_opens_the_app_it_came_with() {
        let dir = std::env::temp_dir().join(format!("weft-opens-{}", std::process::id()));
        // Inside Weft.app, the bundle, so the system shows it as the app.
        let macos = dir.join("Weft.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("weft-desktop"), "").unwrap();
        assert_eq!(app_to_open(&macos.join("weft")), Some(Opens::Bundle(dir.join("Weft.app"))));
        // Elsewhere, the desktop program beside it.
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        assert_eq!(app_to_open(&bin.join("weft")), None, "no app beside it");
        std::fs::write(bin.join("weft-desktop"), "").unwrap();
        assert_eq!(app_to_open(&bin.join("weft")), Some(Opens::Program(bin.join("weft-desktop"))));
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        let (drawn, keys) = picker_lines("", None, &offered, 0);
        assert!(drawn.join("\n").contains(&format!("▸ {}", here.display())), "{drawn:?}");
        assert!(keys.contains("[↑↓] PICK"), "{keys}");
        let (typing, _) = picker_lines("/else", None, &offered, 0);
        assert!(!typing.join("\n").contains('▸'), "a typed path is what Enter opens: {typing:?}");
        assert!(!picker_lines("", None, &[], 0).1.contains("PICK"));
    }

    /// T1.2: the picker as the Atlas draws it, the window's mark over it.
    #[test]
    fn atlas_t1_2_the_project_picker() {
        let (lines, keys) = picker_lines("", None, &[], 0);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 32)).expect("terminal");
        terminal.draw(|frame| draw_picker(frame, &lines, &keys)).expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let drawn: Vec<String> = (0..32)
            .map(|y| {
                let row: String = (0..120).map(|x| buffer[(x, y)].symbol().to_string()).collect();
                row.trim_end().to_string()
            })
            .collect();
        insta::assert_snapshot!("atlas_t1_2", drawn.join("\n"));
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
