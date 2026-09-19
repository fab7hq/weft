use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;

use weft::app::App;
use weft::keys;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let first = args.next();

    if matches!(first.as_deref(), Some("--help" | "-h")) {
        println!("weft [project-dir] [agent ...]\n");
        println!("  project-dir   the repository to work in (default: .)");
        println!("  agent         claude and/or codex; optional, for scripts and probes\n");
        println!("An agent may carry its own arguments, quoted as one word:\n");
        println!("  weft . \"claude --model sonnet --effort medium\"");
        println!("  weft . \"codex -m gpt-5.6-luna -c model_reasoning_effort=medium\"\n");
        println!("Weft passes them through untouched. It never chooses a model,");
        println!("and starts no agent unless you name one or press N.");
        return Ok(());
    }

    let root = std::path::PathBuf::from(first.unwrap_or_else(|| ".".into()))
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    // Weft starts no agent on its own. Named agents are a convenience for
    // scripts and probes; the ordinary way in is to press N.
    let agents: Vec<String> = args.collect();

    let toggle = keys::Toggle;

    let mut terminal = ratatui::init();
    let mut out = std::io::stdout();
    let _ = execute!(out, EnableMouseCapture, EnableBracketedPaste);

    let mut weft = App::new(root, toggle);
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

    let _ = execute!(out, DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();

    if let Err(e) = &result {
        eprintln!("weft: {e}");
    }
    result
}

