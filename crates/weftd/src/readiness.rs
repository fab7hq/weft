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
        Some(listing) => (read(&listing), weft_core::sync::installed(&listing)),
        None => (Readiness::Unknown, None),
    }
}

/// What the harness said, or nothing at all when it would not say.
fn ask(h: &Harness) -> Option<Value> {
    let out = Command::new(h.program).args(h.list).output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use weft_core::readiness::Gap;

    #[test]
    fn no_cli_is_the_first_gap_and_nothing_else_is_asked() {
        let h = crate::harness::find("codex").expect("codex");
        assert_eq!(check(h, false), Readiness::Missing(Gap::Cli));
    }

    #[test]
    fn a_harness_that_will_not_answer_is_unknown_rather_than_missing() {
        // `read` never sees the output, because there was none. The distinction
        // matters: Missing can be fixed by installing, Unknown cannot.
        assert_eq!(read(&json!("not a listing at all")), Readiness::Missing(Gap::Marketplace));
        let absent = crate::harness::Harness {
            name: "nowhere",
            program: "definitely-not-a-real-binary-weft",
            config_env: "NOWHERE_HOME",
            config_default: ".nowhere",
            list: &["plugin", "list", "--json"],
            add_marketplace: &["x"],
            install_plugin: &["y"],
            update_marketplace: &["z"],
            update_plugin: &["w"],
            resume: &["resume"],
            transcript: None,
        };
        assert_eq!(check(&absent, true), Readiness::Unknown);
    }
}
