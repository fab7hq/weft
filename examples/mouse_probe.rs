//! What mouse reporting does a harness ask for?
use weft::pane::Pane;
fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let spec = a.next().unwrap_or_else(|| "codex".into());
    let cwd = a.next().unwrap_or_else(|| ".".into());
    let mut parts = spec.split_whitespace();
    let prog = parts.next().unwrap().to_string();
    let extra: Vec<String> = parts.map(str::to_string).collect();
    let argv: Vec<&str> = extra.iter().map(String::as_str).collect();
    let pane = Pane::spawn_args("probe", &prog, &argv, &cwd, 40, 120)?;
    std::thread::sleep(std::time::Duration::from_secs(10));
    pane.with_screen(|s| {
        println!(
            "{prog}: mode={:?} encoding={:?} alt={:?}",
            s.mouse_protocol_mode(),
            s.mouse_protocol_encoding(),
            s.alternate_screen()
        );
    });
    Ok(())
}
