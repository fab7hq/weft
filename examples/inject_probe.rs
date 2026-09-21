//! Type a prompt into a real harness on the person's behalf, then show the screen.
//!
//! Verification aid for gates W2 and W3:
//!   cargo run --example inject_probe -- claude /path/to/repo "some prompt"

use std::time::Duration;
use weft::pane::Pane;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let program = args.next().unwrap_or_else(|| "/bin/sh".into());
    let cwd = args.next().unwrap_or_else(|| ".".into());
    let payload = args.next().unwrap_or_else(|| "hello from weft".into());

    let mut pane = Pane::spawn("probe", &program, &cwd, 40, 120)?;
    std::thread::sleep(Duration::from_secs(10));

    // Clear the trust prompt the way a person would: move down, press Enter.
    pane.send(b"\x1b[B")?;
    std::thread::sleep(Duration::from_millis(400));
    pane.send(b"\r")?;
    std::thread::sleep(Duration::from_secs(8));

    let bracketed = pane.with_screen(|s| s.bracketed_paste());
    println!("--- bracketed paste advertised: {bracketed} ---");

    match pane.inject(payload.as_bytes(), false) {
        Ok(attempt) => {
            println!("--- injected {} bytes (bracketed={}) ---", attempt.bytes, attempt.bracketed)
        }
        Err(refusal) => println!("--- refused: {refusal:?} ---"),
    }
    std::thread::sleep(Duration::from_secs(6));

    println!("{}", pane.with_screen(|s| s.contents()));
    Ok(())
}
