//! Whether a harness can do RingFrame work, and what is missing if not.
//!
//! Spec: `plans/weft/spec/readiness.md`. Three checks: the `ringframe` CLI,
//! globally; the `fab7` marketplace and the `rf` plugin, per harness. Weft
//! asks the harness and believes the answer; when the harness will not answer,
//! that is `Unknown` and not `Missing`, because "I could not tell" and "it is
//! not there" are different facts and only one of them is fixable by
//! installing something.

use std::process::Command;

use serde_json::Value;

use crate::harness::{Harness, MARKETPLACE, PLUGIN};

/// What a harness is short of. In the order the checks run, because each is
/// useless without the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    /// The CLI that owns the record is not installed at all.
    Cli,
    /// The harness has never been told where the plugin is published.
    Marketplace,
    /// The marketplace is there; the plugin is not installed, or is disabled.
    Plugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// All three checks pass. Ask, check and decide work in this pane.
    Ready,
    /// A check failed, and Weft knows which.
    Missing(Gap),
    /// The person said no. Weft does not ask again this session.
    Declined,
    /// The harness would not answer. Weft says so and offers nothing.
    Unknown,
}

impl Readiness {
    pub fn is_ready(self) -> bool {
        self == Readiness::Ready
    }

    /// Whether offering to set this up would make sense. There is nothing to
    /// offer for a missing CLI, which is not Weft's to install.
    pub fn can_be_set_up(self) -> bool {
        matches!(self, Readiness::Missing(Gap::Marketplace | Gap::Plugin) | Readiness::Declined)
    }

    /// One sentence, in plain words, naming the harness it is about.
    pub fn say(self, harness: &str) -> Option<String> {
        match self {
            Readiness::Ready => None,
            Readiness::Missing(Gap::Cli) => Some("RingFrame is not installed.".into()),
            Readiness::Missing(Gap::Marketplace | Gap::Plugin) => {
                Some(format!("{harness} is not set up for RingFrame."))
            }
            Readiness::Declined => Some(format!("You said not now for {harness}.")),
            Readiness::Unknown => {
                Some(format!("Weft could not ask {harness} whether it is set up."))
            }
        }
    }
}

/// Ask one harness. `cli` is the global check, made once by the caller.
pub fn check(h: &Harness, cli: bool) -> Readiness {
    if !cli {
        return Readiness::Missing(Gap::Cli);
    }
    let Some(listing) = ask(h) else {
        return Readiness::Unknown;
    };
    read(&listing)
}

/// What the harness said, or nothing at all when it would not say.
fn ask(h: &Harness) -> Option<Value> {
    let out = Command::new(h.program).args(h.list).output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

/// Both harnesses answer with `installed` and `available` lists. They name a
/// plugin differently — `id` against `pluginId` — and that is the whole of the
/// difference, so one reader handles both.
pub fn read(listing: &Value) -> Readiness {
    let rows = |key: &str| {
        listing
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let installed = rows("installed");
    let known: Vec<Value> = installed.iter().cloned().chain(rows("available")).collect();

    if !known.iter().any(|row| from(row, MARKETPLACE)) {
        return Readiness::Missing(Gap::Marketplace);
    }
    let ready = installed.iter().any(|row| {
        id_of(row) == Some(PLUGIN.to_string())
            // Absent means enabled: Claude Code lists only what is installed
            // and says `enabled` explicitly; Codex says both.
            && row.get("enabled").and_then(Value::as_bool).unwrap_or(true)
            && row.get("installed").and_then(Value::as_bool).unwrap_or(true)
    });
    if ready { Readiness::Ready } else { Readiness::Missing(Gap::Plugin) }
}

fn id_of(row: &Value) -> Option<String> {
    row.get("id")
        .or_else(|| row.get("pluginId"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Whether a listed plugin comes from the marketplace we are looking for.
fn from(row: &Value, marketplace: &str) -> bool {
    if row.get("marketplaceName").and_then(Value::as_str) == Some(marketplace) {
        return true;
    }
    id_of(row).is_some_and(|id| id.rsplit('@').next() == Some(marketplace))
}

/// Run the two commands, in order, and stop at the first that fails. Returns
/// what went wrong, and never what it means: the caller re-checks rather than
/// believing an exit code.
pub fn set_up(h: &Harness) -> Result<(), String> {
    for args in [h.add_marketplace, h.install_plugin] {
        let out = Command::new(h.program)
            .args(args)
            .output()
            .map_err(|e| format!("{} {}: {e}", h.program, args.join(" ")))?;
        if !out.status.success() {
            let said = String::from_utf8_lossy(&out.stderr);
            let said = said.trim();
            return Err(format!(
                "{} {} said: {}",
                h.program,
                args.join(" "),
                if said.is_empty() { "nothing" } else { said }
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Captured from `claude plugin list --available --json`.
    fn claude(enabled: bool) -> Value {
        json!({
            "installed": [
                {"id": "feature-dev@claude-plugins-official", "enabled": true},
                {"id": "rf@fab7", "version": "0.0.3", "scope": "user", "enabled": enabled}
            ],
            "available": []
        })
    }

    /// Captured from `codex plugin list --json`.
    fn codex(installed: bool) -> Value {
        json!({
            "installed": [
                {"pluginId": "rf@fab7", "name": "rf", "marketplaceName": "fab7",
                 "version": "0.0.3", "installed": installed, "enabled": true}
            ],
            "available": []
        })
    }

    #[test]
    fn both_harnesses_are_read_by_one_reader() {
        assert_eq!(read(&claude(true)), Readiness::Ready);
        assert_eq!(read(&codex(true)), Readiness::Ready);
    }

    #[test]
    fn a_plugin_that_is_present_but_disabled_is_not_ready() {
        assert_eq!(read(&claude(false)), Readiness::Missing(Gap::Plugin));
    }

    #[test]
    fn a_marketplace_that_was_never_added_reads_as_the_marketplace_missing() {
        let bare = json!({"installed": [{"id": "feature-dev@claude-plugins-official"}], "available": []});
        assert_eq!(read(&bare), Readiness::Missing(Gap::Marketplace));
    }

    #[test]
    fn the_marketplace_counts_as_added_when_the_plugin_is_only_available() {
        // Added but never installed: the gap is the plugin, not the marketplace.
        let available = json!({
            "installed": [],
            "available": [{"pluginId": "rf@fab7", "marketplaceName": "fab7", "installed": false}]
        });
        assert_eq!(read(&available), Readiness::Missing(Gap::Plugin));
    }

    #[test]
    fn no_cli_is_the_first_gap_and_nothing_else_is_asked() {
        let h = crate::harness::find("codex").expect("codex");
        assert_eq!(check(h, false), Readiness::Missing(Gap::Cli));
    }

    #[test]
    fn each_state_says_a_different_thing_and_names_its_harness() {
        assert_eq!(Readiness::Ready.say("codex"), None);
        assert_eq!(
            Readiness::Missing(Gap::Plugin).say("codex").as_deref(),
            Some("codex is not set up for RingFrame.")
        );
        assert_eq!(
            Readiness::Missing(Gap::Cli).say("codex").as_deref(),
            Some("RingFrame is not installed.")
        );
        assert!(Readiness::Declined.say("codex").unwrap().contains("not now"));
        assert!(Readiness::Unknown.say("codex").unwrap().contains("could not ask"));
    }

    #[test]
    fn only_what_weft_may_install_is_offered() {
        assert!(Readiness::Missing(Gap::Plugin).can_be_set_up());
        assert!(Readiness::Missing(Gap::Marketplace).can_be_set_up());
        assert!(Readiness::Declined.can_be_set_up(), "a no can be changed");
        assert!(!Readiness::Missing(Gap::Cli).can_be_set_up(), "not Weft's to install");
        assert!(!Readiness::Unknown.can_be_set_up(), "nothing to offer for an unanswered question");
        assert!(!Readiness::Ready.can_be_set_up());
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
        };
        assert_eq!(check(&absent, true), Readiness::Unknown);
    }
}
