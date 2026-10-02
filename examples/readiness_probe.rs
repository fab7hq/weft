//! Ask each harness RingFrame has a profile for where it stands, and which
//! `rf` version it has. No panes, no model, no writes.
//!
//!   cargo run --example readiness_probe [<harness files folder>]

use std::path::PathBuf;

use weft::harness::OnThisMachine as _;
use weft::readiness;

fn main() {
    let machine = weft::outside::Machine;
    let cli = weft::ringframe::installed(&machine);
    println!("ringframe CLI installed: {cli}");
    let folder = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| weft::routing::file().with_file_name("harnesses"));
    for h in weft::harness::installed(&folder).0.iter() {
        let found = h.on_path();
        let (state, version) = readiness::look(&machine, h, cli);
        println!(
            "\n{:<12} on PATH {:<5} config {}\n             {:?} · rf {}{}",
            h.name,
            found,
            h.config_home().display(),
            state,
            version.as_deref().unwrap_or("?"),
            state.say(&h.name).map(|s| format!("  — {s}")).unwrap_or_default()
        );
    }
}
