//! The recorded mode, the chosen handoff, and the host's answer.
//!
//!   cargo run --example handoff_live -- <cwd> <codex|claude> <mode|inline>
//!
//! By-hand, in the W4 tradition: one model call per run, and what is claimed
//! is Weft's own mechanism, not anything about the model.
use weft::inject::Handoff;
use weft::ledger::Delivery;
use weft::pane::Pane;

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let cwd = a.next().unwrap_or_else(|| ".".into());
    let prog = a.next().unwrap_or_else(|| "codex".into());
    let which = a.next().unwrap_or_else(|| "mode".into());

    // Exactly what RingFrame now records for such a prompt.
    let delivery = match which.as_str() {
        "mode" => Delivery {
            mode: Some("/plan".into()),
            kind: Some("mode".into()),
            active: Some("Plan mode".into()),
            prefix_bytes: 6,
            folds: true,
        },
        _ => Delivery {
            mode: Some("/goal".into()),
            kind: Some("inline".into()),
            active: None,
            prefix_bytes: 6,
            folds: true,
        },
    };
    let how = Handoff::choose(&delivery);
    println!("recorded: {delivery:?}\nchosen:   {how:?}");

    let filler = "Say only the word ACK and nothing else. ".repeat(40);
    let prompt = format!("{} {}", delivery.mode.clone().unwrap(), &filler[..1400]);

    let mut pane = Pane::spawn_args(prog.as_str(), &prog, &[], &cwd, 32, 100)?;
    std::thread::sleep(std::time::Duration::from_secs(7));
    let attempt =
        pane.inject_as(prompt.as_bytes(), false, &how).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    println!("attempt:  {attempt:?}");
    std::thread::sleep(std::time::Duration::from_secs(12));
    let screen = pane.with_screen(|s| s.contents());
    for l in screen
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .iter()
        .rev()
    {
        println!("{l}");
    }
    pane.send(&[3])?;
    pane.send(&[3])?;
    Ok(())
}
