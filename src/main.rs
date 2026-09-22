use anyhow::Result;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;

use weft::app::App;
use weft::client::Session;
use weft::keys;
use weft::protocol;
use weft::server;

/// The first screen when no directory was named. It opens nothing: the
/// daemon is not asked for anything until there is a project to ask about.
fn choose_project(terminal: &mut ratatui::DefaultTerminal) -> Result<Option<std::path::PathBuf>> {
    use crossterm::event::{self, Event, KeyCode};
    use ratatui::text::Line;
    use ratatui::widgets::Paragraph;

    let mut typed = String::new();
    let mut said: Option<String> = None;
    loop {
        terminal.draw(|frame| {
            let mut lines = vec![
                Line::raw(" ▚▞ WEFT"),
                Line::raw(""),
                Line::raw("   Open a project"),
                Line::raw(""),
                Line::raw(format!("   {typed}█")),
                Line::raw(""),
                Line::raw("   The path of a repository to work in."),
            ];
            if let Some(why) = &said {
                lines.push(Line::raw(""));
                lines.push(Line::raw(format!("   {why}")));
            }
            lines.push(Line::raw(""));
            lines.push(Line::raw("  [Enter] OPEN   [X] QUIT"));
            frame.render_widget(Paragraph::new(lines), frame.area());
        })?;
        let Event::Key(key) = event::read()? else { continue };
        if !key.is_press() {
            continue;
        }
        match key.code {
            KeyCode::Enter if !typed.trim().is_empty() => {
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

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let first = args.next();

    // Started by a client to own the panes. One per machine, headless, and it
    // outlives whatever asked for it (ADR-0007).
    if first.as_deref() == Some("--serve") {
        return server::Session::serve(&protocol::socket_path());
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

    if matches!(first.as_deref(), Some("--help" | "-h")) {
        println!("weft {}\n", env!("CARGO_PKG_VERSION"));
        println!("weft [project-dir] [agent ...]\n");
        println!("  project-dir   the repository to work in (default: .)");
        println!("  agent         claude and/or codex; optional, for scripts and probes\n");
        println!("An agent may carry its own arguments, quoted as one word:\n");
        println!("  weft . \"claude --model sonnet --effort medium\"");
        println!("  weft . \"codex -m gpt-5.6-luna -c model_reasoning_effort=medium\"\n");
        println!("Weft passes them through untouched. It never chooses a model,");
        println!("and starts no agent unless you name one or press N.\n");
        println!("Agents run in a background session that outlives this window.");
        println!("Closing Weft leaves them working; `weft stop` ends them.");
        return Ok(());
    }

    let named = first.clone();
    // Weft starts no agent on its own. Named agents are a convenience for
    // scripts and probes; the ordinary way in is to press N.
    let agents: Vec<String> = args.collect();

    let toggle = keys::Toggle;

    let mut terminal = ratatui::init();
    let mut out = std::io::stdout();
    // Weft does not capture the mouse. With capture on, the terminal hands
    // every click and drag to Weft instead of doing its own selection, so
    // click-drag-copy stops working over the agent — which is the one thing
    // a person most wants the mouse for here. Weft is keys only.
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
        let _ = execute!(out, DisableBracketedPaste);
        ratatui::restore();
        return Ok(());
    };

    let size = terminal.size().unwrap_or_default();
    let session = Session::open(&root, size.height.max(24), size.width.max(80))?;
    let mut weft = App::with_session(root, toggle, session);
    let mut started = Ok(());
    for agent in &agents {
        let program = agent.split_whitespace().next().unwrap_or(agent);
        let harness = if program == "claude" { "claude-code" } else { program };
        if let Err(e) = weft.add(harness, agent) {
            started = Err(e);
            break;
        }
    }

    let result = started.and_then(|()| weft.run(&mut terminal));

    let _ = execute!(out, DisableBracketedPaste);
    ratatui::restore();

    if let Err(e) = &result {
        eprintln!("weft: {e}");
    }
    result
}
