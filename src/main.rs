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
    let program = args.next().unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()));
    let cwd = args.next().unwrap_or_else(|| ".".into());
    let project = std::path::Path::new(&cwd)
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "project".into());

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

    let mut weft = App::new(project, toggle);
    let started = weft.add(shorten(&program), &program, &cwd);

    let result = match started {
        Ok(()) => weft.run(&mut terminal),
        Err(e) => Err(e),
    };

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

fn shorten(program: &str) -> String {
    std::path::Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string())
}
