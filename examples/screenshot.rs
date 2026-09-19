//! Render a real program inside a Weft pane and print the screen.
//!
//! Verification aid for gate W2:
//!   cargo run --example screenshot -- claude /path/to/repo 12

use weft::pane::Pane;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let program = args.next().unwrap_or_else(|| "/bin/sh".into());
    let cwd = args.next().unwrap_or_else(|| ".".into());
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(8);

    let mut pane = Pane::spawn("probe", &program, &cwd, 40, 120)?;
    std::thread::sleep(std::time::Duration::from_secs(seconds));

    let running = pane.running();
    let (rows, cols) = pane.with_screen(|s| s.size());
    let contents = pane.with_screen(|s| s.contents());
    println!("--- {program} in a Weft pane · {cols}x{rows} · running={running} ---");
    println!("{contents}");
    Ok(())
}
