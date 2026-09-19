use anyhow::Result;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::supports_keyboard_enhancement;

use weft::app::App;
use weft::keys;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let first = args.next();

    if matches!(first.as_deref(), Some("--help" | "-h")) {
        println!("weft [project-dir] [agent ...]\n");
        println!("  project-dir   the repository to work in (default: .)");
        println!("  agent         claude and/or codex (default: whichever is installed)");
        return Ok(());
    }

    let root = std::path::PathBuf::from(first.unwrap_or_else(|| ".".into()))
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut agents: Vec<String> = args.collect();
    if agents.is_empty() {
        agents = ["claude", "codex"]
            .iter()
            .filter(|a| which(a).is_some())
            .map(|a| a.to_string())
            .collect();
    }

    // Ctrl+Shift+W is only distinguishable where the terminal reports modifiers
    // on a Ctrl+letter chord; everywhere else Weft uses a legacy-safe key.
    let kitty = supports_keyboard_enhancement().unwrap_or(false);
    let toggle = keys::negotiate(kitty);

    let mut terminal = ratatui::init();
    let mut out = std::io::stdout();
    if kitty {
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    let _ = execute!(out, EnableMouseCapture);

    let mut weft = App::new(root, toggle);
    let mut started = Ok(());
    for agent in &agents {
        let harness = if agent == "claude" { "claude-code" } else { agent.as_str() };
        if let Err(e) = weft.add(harness, agent) {
            started = Err(e);
            break;
        }
    }

    let result = started.and_then(|()| weft.run(&mut terminal));

    let _ = execute!(out, DisableMouseCapture);
    if kitty {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    ratatui::restore();

    if let Err(e) = &result {
        eprintln!("weft: {e}");
    }
    result
}

fn which(program: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|p| p.is_file())
    })
}
