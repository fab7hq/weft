//! Whether a harness can do RingFrame work, and what is missing if not.
//!
//! Three checks: the `ringframe` CLI, globally; the `fab7` marketplace and the
//! `rf` plugin, per harness. Weft
//! asks the harness and believes the answer; when the harness will not answer,
//! that is `Unknown` and not `Missing`, because "I could not tell" and "it is
//! not there" are different facts and only one of them is fixable by
//! installing something.

use serde_json::Value;

use crate::harness::{MARKETPLACE, PLUGIN};

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
        matches!(self, Readiness::Missing(Gap::Marketplace | Gap::Plugin))
    }

    /// One sentence, in plain words, naming the harness it is about.
    pub fn say(self, harness: &str) -> Option<String> {
        match self {
            Readiness::Ready => None,
            Readiness::Missing(Gap::Cli) => Some("RingFrame is not installed.".into()),
            Readiness::Missing(Gap::Marketplace | Gap::Plugin) => {
                Some(format!("{harness} is not set up for RingFrame."))
            }
            Readiness::Unknown => {
                Some(format!("Weft could not ask {harness} whether it is set up."))
            }
        }
    }
}

/// What a harness's plugin listing says, read the way its harness file says
/// it reads. A harness with marketplaces names a plugin `rf@fab7`, and has
/// the marketplace when anything it lists comes from it; one with none names
/// it `rf`, and has no marketplace to add.
pub fn read(h: &crate::harness::Harness, listing: &Value) -> Readiness {
    let Some(shape) = &h.listing else { return Readiness::Unknown };
    let marketplaces = h.add_marketplace.is_some();
    let installed = rows(listing, &shape.installed);
    let known: Vec<Value> =
        installed.iter().cloned().chain(rows(listing, &shape.available)).collect();
    let name = |row: &Value| row.get(&shape.name).and_then(Value::as_str).map(str::to_string);

    let from_ours = |row: &Value| {
        name(row).is_some_and(|n| n.rsplit_once('@').is_some_and(|(_, m)| m == MARKETPLACE))
    };
    if marketplaces && !known.iter().any(from_ours) {
        return Readiness::Missing(Gap::Marketplace);
    }
    let short = PLUGIN.split('@').next().unwrap_or(PLUGIN);
    let want = if marketplaces { PLUGIN } else { short };
    let ready = installed.iter().any(|row| {
        name(row).as_deref() == Some(want)
            // Absent means on: a listing that says nothing lists what it has.
            && shape.on.iter().all(|f| row.get(f).and_then(Value::as_bool).unwrap_or(true))
    });
    if ready { Readiness::Ready } else { Readiness::Missing(Gap::Plugin) }
}

/// Every row of these lists in a listing.
pub fn rows(listing: &Value, lists: &[String]) -> Vec<Value> {
    lists
        .iter()
        .flat_map(|k| listing.get(k).and_then(Value::as_array).cloned().unwrap_or_default())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Each fixture harness, as its file reads.
    fn harness(name: &str) -> crate::harness::Harness {
        crate::harness::Harness::of(name, &crate::harness::fixture::file(name)).expect(name)
    }

    fn with() -> crate::harness::Harness {
        harness("claude-code")
    }

    fn without() -> crate::harness::Harness {
        harness("antigravity")
    }

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
        assert_eq!(read(&with(), &claude(true)), Readiness::Ready);
        assert_eq!(read(&harness("codex"), &codex(true)), Readiness::Ready);
        assert_eq!(read(&harness("codex"), &codex(false)), Readiness::Missing(Gap::Plugin));
    }

    #[test]
    fn a_plugin_that_is_present_but_disabled_is_not_ready() {
        assert_eq!(read(&with(), &claude(false)), Readiness::Missing(Gap::Plugin));
    }

    #[test]
    fn a_marketplace_that_was_never_added_reads_as_the_marketplace_missing() {
        let bare =
            json!({"installed": [{"id": "feature-dev@claude-plugins-official"}], "available": []});
        assert_eq!(read(&with(), &bare), Readiness::Missing(Gap::Marketplace));
    }

    #[test]
    fn the_marketplace_counts_as_added_when_the_plugin_is_only_available() {
        // Added but never installed: the gap is the plugin, not the marketplace.
        let available = json!({
            "installed": [],
            "available": [{"pluginId": "rf@fab7", "marketplaceName": "fab7", "installed": false}]
        });
        assert_eq!(read(&harness("codex"), &available), Readiness::Missing(Gap::Plugin));
    }

    /// Captured from `agy plugin list` (Antigravity 1.2.12) once `rf` is
    /// installed: `imports`, named by `name`, and no marketplace at all.
    fn antigravity(names: &[&str]) -> Value {
        let rows: Vec<Value> = names
            .iter()
            .map(|n| json!({"name": n, "source": "antigravity",
                            "importedAt": "2026-09-27T06:18:59Z", "components": ["skills", "hooks"]}))
            .collect();
        json!({"imports": rows})
    }

    /// A harness with no marketplace to add has its plugin by name alone.
    #[test]
    fn a_harness_with_no_marketplaces_is_ready_when_it_has_the_plugin() {
        assert_eq!(read(&without(), &antigravity(&["rf"])), Readiness::Ready);
        assert_eq!(read(&without(), &antigravity(&["other"])), Readiness::Missing(Gap::Plugin));
        assert_eq!(read(&without(), &json!({"imports": []})), Readiness::Missing(Gap::Plugin));
        // And the two with marketplaces read exactly as before.
        assert_eq!(read(&with(), &claude(true)), Readiness::Ready);
        assert_eq!(read(&with(), &antigravity(&["rf"])), Readiness::Missing(Gap::Marketplace));
    }

    /// A harness whose listing holds its plugins under `plugins`, named by
    /// `slug`, is read from what its harness file says of that shape.
    #[test]
    fn a_listing_is_read_the_way_its_harness_file_says() {
        let mut p = crate::harness::fixture::file("codex");
        p["plugin"]["listing"] =
            json!({"installed": ["plugins"], "available": ["catalog"], "name": "slug"});
        let odd = crate::harness::Harness::of("odd", &p).expect("a harness");
        let has = json!({"plugins": [{"slug": "rf@fab7"}], "catalog": []});
        assert_eq!(read(&odd, &has), Readiness::Ready);
        let offered = json!({"plugins": [], "catalog": [{"slug": "rf@fab7"}]});
        assert_eq!(read(&odd, &offered), Readiness::Missing(Gap::Plugin));
        assert_eq!(read(&odd, &json!({"plugins": []})), Readiness::Missing(Gap::Marketplace));
        assert_eq!(
            crate::sync::installed(
                &odd,
                &json!({"plugins": [{"slug": "rf@fab7",
            "version": "0.2.0"}]})
            )
            .as_deref(),
            Some("0.2.0")
        );
        p["plugin"]["listing"] = Value::Null;
        let blind = crate::harness::Harness::of("blind", &p).expect("still offered");
        assert_eq!(read(&blind, &has), Readiness::Unknown, "no shape, no answer");
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
        assert!(Readiness::Unknown.say("codex").unwrap().contains("could not ask"));
    }

    #[test]
    fn only_what_weft_may_install_is_offered() {
        assert!(Readiness::Missing(Gap::Plugin).can_be_set_up());
        assert!(Readiness::Missing(Gap::Marketplace).can_be_set_up());
        assert!(!Readiness::Missing(Gap::Cli).can_be_set_up(), "not Weft's to install");
        assert!(!Readiness::Unknown.can_be_set_up(), "nothing to offer for an unanswered question");
        assert!(!Readiness::Ready.can_be_set_up());
    }
}
