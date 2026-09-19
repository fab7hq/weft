//! Print the board for a project, exactly as the list renders it.
//!
//!   cargo run --example board -- /path/to/project

use weft::ledger::Ledger;
use weft::record::Record;

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let mut led = Ledger::at(std::path::Path::new(&root));
    led.refresh();
    let units = led.units();
    println!("--- {} · {} units ---", root, units.len());
    for u in &units {
        let mark = if u.needs_you() { "●" } else { " " };
        println!("{mark} {:<34} {:<12} {}", truncate(&u.title, 34), u.harness, u.status());
        if let Some(c) = &u.check {
            let root = std::path::Path::new(&root);
            match Record::read(root, &c.eval_id) {
                Some(r) => println!(
                    "    {} · judged by {} · recorded: {}",
                    r.agreed(c.agreement),
                    r.judged_by().join(" and "),
                    c.verdict.recorded()
                ),
                None => println!(
                    "    agreement {:.2} · judging host not recorded · recorded: {}",
                    c.agreement,
                    c.verdict.recorded()
                ),
            }
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n - 1).chain(['…']).collect() }
}
