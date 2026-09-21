//! Render a real program inside a Weft pane and print the screen.
//!
//! Verification aid for gate W2. The program may carry its own arguments,
//! quoted as one word, exactly as Weft passes a spec through:
//!   cargo run --example screenshot -- claude /path/to/repo 12
//!   cargo run --example screenshot -- "codex resume 01a0bdb6-…" /path 20

use weft::pane::Pane;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let program = args.next().unwrap_or_else(|| "/bin/sh".into());
    let cwd = args.next().unwrap_or_else(|| ".".into());
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(8);

    let mut parts = program.split_whitespace();
    let exe = parts.next().unwrap_or(&program).to_string();
    let args: Vec<String> = parts.map(str::to_string).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut pane = Pane::spawn_args("probe", &exe, &argv, &cwd, 40, 120)?;
    std::thread::sleep(std::time::Duration::from_secs(seconds));

    let running = pane.running();
    let (rows, cols) = pane.with_screen(|s| s.size());
    let contents = pane.with_screen(|s| s.contents());
    println!("--- {program} in a Weft pane · {cols}x{rows} · running={running} ---");
    println!("{contents}");
    Ok(())
}
