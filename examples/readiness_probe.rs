//! Ask each supported harness where it stands. No panes, no model, no writes.
//!
//!   cargo run --example readiness_probe

use weft::harness::OnThisMachine as _;
use weft::{harness, readiness};

fn main() {
    let cli = weft::ringframe::installed();
    println!("ringframe CLI installed: {cli}");
    for h in harness::SUPPORTED {
        let found = h.on_path();
        let state = readiness::check(h, cli);
        println!(
            "\n{:<12} on PATH {:<5} config {}\n             {:?}{}",
            h.name,
            found,
            h.config_home().display(),
            state,
            state.say(h.name).map(|s| format!("  — {s}")).unwrap_or_default()
        );
    }
}
