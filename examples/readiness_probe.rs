//! Ask each harness RingFrame has a profile for where it stands. No panes, no
//! model, no writes.
//!
//!   cargo run --example readiness_probe

use weft::harness::OnThisMachine as _;
use weft::readiness;

fn main() {
    let machine = weft::outside::Machine;
    let cli = weft::ringframe::installed(&machine);
    println!("ringframe CLI installed: {cli}");
    for h in weft::harness::installed(&weft::routing::file().with_file_name("harnesses")).0.iter() {
        let found = h.on_path();
        let state = readiness::check(&machine, h, cli);
        println!(
            "\n{:<12} on PATH {:<5} config {}\n             {:?}{}",
            h.name,
            found,
            h.config_home().display(),
            state,
            state.say(&h.name).map(|s| format!("  — {s}")).unwrap_or_default()
        );
    }
}
