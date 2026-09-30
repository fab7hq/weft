//! Asking a harness what it has installed.
//!
//! Reading the answer is a rule and lives in [`weft_core::readiness`].
//! Running the command that produces one is here, because that is the only
//! part that needs a process.

use std::process::Command;

use serde_json::Value;
use weft_core::harness::Harness;
pub use weft_core::readiness::*;

/// Ask one harness. `cli` is the global check, made once by the caller.
pub fn check(h: &Harness, cli: bool) -> Readiness {
    look(h, cli).0
}

/// Its readiness, and the `rf` version it has installed, from one listing.
pub fn look(h: &Harness, cli: bool) -> (Readiness, Option<String>) {
    if !cli {
        return (Readiness::Missing(Gap::Cli), None);
    }
    match ask(h) {
        Some(listing) => (read(h, &listing), weft_core::sync::installed(h, &listing)),
        None => (Readiness::Unknown, None),
    }
}

/// What the harness said, or nothing at all when it would not say.
fn ask(h: &Harness) -> Option<Value> {
    let out = Command::new(&h.program).args(&h.list).output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok().or_else(|| {
        // Some harnesses say "nothing installed" in words: that is an empty
        // listing, not a harness that would not answer.
        let said = String::from_utf8_lossy(&out.stdout);
        let none = h.listing.as_ref()?.none.as_deref()?;
        (said.trim() == none).then(|| serde_json::json!({}))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use weft_core::readiness::Gap;

    fn absent() -> crate::harness::Harness {
        let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        crate::harness::Harness {
            name: "nowhere".into(),
            title: "Nowhere".into(),
            program: "definitely-not-a-real-binary-weft".into(),
            config_env: Some("NOWHERE_HOME".into()),
            config_default: ".nowhere".into(),
            list: argv(&["plugin", "list", "--json"]),
            add_marketplace: Some(argv(&["x"])),
            install_plugin: Some(argv(&["y"])),
            update_marketplace: Some(argv(&["z"])),
            update_plugin: Some(argv(&["w"])),
            listing: Some(weft_core::harness::Listing {
                installed: argv(&["installed"]),
                available: argv(&["available"]),
                name: "id".into(),
                on: argv(&["enabled"]),
                none: None,
            }),
            resume: argv(&["resume"]),
            transcript: None,
            turbo: Vec::new(),
        }
    }

    #[test]
    fn no_cli_is_the_first_gap_and_nothing_else_is_asked() {
        assert_eq!(check(&absent(), false), Readiness::Missing(Gap::Cli));
    }

    #[test]
    fn a_harness_that_will_not_answer_is_unknown_rather_than_missing() {
        // `read` never sees the output, because there was none. The distinction
        // matters: Missing can be fixed by installing, Unknown cannot.
        assert_eq!(
            read(&absent(), &json!("not a listing at all")),
            Readiness::Missing(Gap::Marketplace)
        );
        assert_eq!(check(&absent(), true), Readiness::Unknown);
    }
}
