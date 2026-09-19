//! How long after a paste must Enter wait before a harness submits it?
//!
//!   cargo run --example enter_probe -- "codex -m gpt-5.6-luna" /path/to/project

use std::time::Duration;
use weft::pane::Pane;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let spec = args.next().unwrap_or_else(|| "codex".into());
    let cwd = args.next().unwrap_or_else(|| ".".into());

    let mut parts = spec.split_whitespace();
    let program = parts.next().unwrap().to_string();
    let extra: Vec<String> = parts.map(str::to_string).collect();
    let argv: Vec<&str> = extra.iter().map(String::as_str).collect();

    let mut pane = Pane::spawn_args("probe", &program, &argv, &cwd, 40, 120)?;
    std::thread::sleep(Duration::from_secs(12));

    // Clear whatever startup panels are up, the way a person would.
    for keys in [&b"t"[..], &b"\x1b"[..], &b"\x1b[B"[..], &b"\r"[..]] {
        pane.send(keys)?;
        std::thread::sleep(Duration::from_millis(600));
    }
    std::thread::sleep(Duration::from_secs(6));

    let bracketed = pane.with_screen(|s| s.bracketed_paste());
    println!("bracketed paste advertised: {bracketed}");

    for (label, wrap, delay) in [
        ("bracketed", true, 300u64),
        ("plain", false, 300),
        ("plain-fast", false, 80),
        ("bracketed-then-second-enter", true, 300),
    ] {
        let marker = format!("probe-{label}");
        let body = if wrap && bracketed {
            let mut b = b"\x1b[200~".to_vec();
            b.extend_from_slice(marker.as_bytes());
            b.extend_from_slice(b"\x1b[201~");
            b
        } else {
            marker.clone().into_bytes()
        };
        pane.send(&body)?;
        std::thread::sleep(Duration::from_millis(delay));
        pane.send(b"\r")?;
        if label == "bracketed-then-second-enter" {
            std::thread::sleep(Duration::from_millis(400));
            pane.send(b"\r")?;
        }
        std::thread::sleep(Duration::from_secs(4));

        // If the text is still sitting in the composer, Enter did not submit.
        let screen = pane.with_screen(|s| s.contents());
        let stuck = screen.lines().any(|l| {
            let t = l.trim();
            (t.starts_with('›') || t.starts_with('❯') || t.starts_with('>')) && t.contains(&marker)
        });
        println!("{label:>28} → {}", if stuck { "STILL IN THE BOX" } else { "submitted" });
        println!("---- screen after {label} ----");
        for line in screen.lines().filter(|l| !l.trim().is_empty()) {
            println!("{line}");
        }
        println!("---- end ----");

        // Leave the composer clean for the next round.
        if stuck {
            for _ in 0..marker.len() + 2 {
                pane.send(&[0x7f])?;
            }
            std::thread::sleep(Duration::from_millis(400));
        }
    }
    Ok(())
}
