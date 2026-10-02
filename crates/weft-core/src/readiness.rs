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
    let name = |row: &Value| field(row, &shape.name).and_then(Value::as_str).map(str::to_string);
    let on = |row: &Value| {
        // Absent means on: a listing that says nothing lists what it has.
        shape.on.iter().all(|f| field(row, f).and_then(Value::as_bool).unwrap_or(true))
    };

    // A harness whose listing names something else that proves the setup:
    // each of those listed and on. Missing, its marketplace comes first when
    // its file has one to add.
    if !shape.want.is_empty() {
        let listed = |w: &String| installed.iter().any(|r| name(r).as_ref() == Some(w) && on(r));
        return match (shape.want.iter().all(listed), marketplaces) {
            (true, _) => Readiness::Ready,
            (false, true) => Readiness::Missing(Gap::Marketplace),
            (false, false) => Readiness::Missing(Gap::Plugin),
        };
    }

    let from_ours = |row: &Value| {
        name(row).is_some_and(|n| n.rsplit_once('@').is_some_and(|(_, m)| m == MARKETPLACE))
    };
    if marketplaces && !known.iter().any(from_ours) {
        return Readiness::Missing(Gap::Marketplace);
    }
    let short = PLUGIN.split('@').next().unwrap_or(PLUGIN);
    let want = if marketplaces { PLUGIN } else { short };
    let ready = installed.iter().any(|row| name(row).as_deref() == Some(want) && on(row));
    if ready { Readiness::Ready } else { Readiness::Missing(Gap::Plugin) }
}

/// A row's field by its name, or by a dotted path into it (`plugin.id`) for a
/// listing that nests what names a plugin.
pub fn field<'a>(row: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(row, |v, key| v.get(key))
}

/// Every row of these lists in a listing. The list `.` is the listing
/// itself, when it is an array.
pub fn rows(listing: &Value, lists: &[String]) -> Vec<Value> {
    lists
        .iter()
        .flat_map(|k| {
            let list = if k == "." { Some(listing) } else { listing.get(k) };
            list.and_then(Value::as_array).cloned().unwrap_or_default()
        })
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

    /// A harness that lists skills, or marketplaces, rather than the plugin:
    /// its file names the rows that prove setup, and `.` reads a bare array.
    #[test]
    fn a_listing_that_never_names_the_plugin_is_read_by_what_its_file_wants() {
        let mut p = crate::harness::fixture::file("antigravity");
        p["plugin"]["listing"] = json!({"installed": ["skills"], "available": [], "name": "id",
                                        "want": ["rf-ask", "rf-eval", "rf-seal"]});
        let skills = crate::harness::Harness::of("skills", &p).expect("a harness");
        let all = json!({"skills": [{"id": "rf-ask"}, {"id": "rf-eval"}, {"id": "rf-seal"}]});
        assert_eq!(read(&skills, &all), Readiness::Ready);
        let some = json!({"skills": [{"id": "rf-ask"}, {"id": "plan"}]});
        assert_eq!(read(&skills, &some), Readiness::Missing(Gap::Plugin), "every one, not any");
        assert_eq!(read(&skills, &json!({"skills": []})), Readiness::Missing(Gap::Plugin));

        let mut p = crate::harness::fixture::file("codex");
        p["plugin"]["listing"] =
            json!({"installed": ["."], "available": [], "name": "name", "want": ["fab7"]});
        let bare = crate::harness::Harness::of("bare", &p).expect("a harness");
        let added = json!([{"name": "cursor-public"}, {"name": "fab7", "scope": "user"}]);
        assert_eq!(read(&bare, &added), Readiness::Ready, "a bare array, read as `.`");
        assert_eq!(
            read(&bare, &json!([{"name": "cursor-public"}])),
            Readiness::Missing(Gap::Marketplace),
            "missing, and its file has a marketplace to add"
        );
        assert!(crate::sync::installed(&bare, &added).is_none(), "no version to read");

        p["plugin"]["listing"] = json!({"installed": ["plugins"], "available": [], "name": "plugin.id", "want": ["rf-x"]});
        let nested = crate::harness::Harness::of("nested", &p).expect("a harness");
        let has = json!({"plugins": [{"active": true, "plugin": {"id": "rf-x"}}]});
        assert_eq!(read(&nested, &has), Readiness::Ready, "a dotted name reaches into the row");
        assert_eq!(read(&nested, &json!({"plugins": []})), Readiness::Missing(Gap::Marketplace));
    }

    /// The version is read under the same name readiness looks for, so a
    /// harness that lists `rf` can be shown behind as one listing `rf@fab7` is.
    #[test]
    fn the_version_is_read_under_the_name_readiness_looks_for() {
        let version =
            |h: &crate::harness::Harness, listing: Value| crate::sync::installed(h, &listing);
        assert_eq!(
            version(&without(), json!({"imports": [{"name": "rf", "version": "0.1.2"}]}))
                .as_deref(),
            Some("0.1.2"),
            "no marketplaces: `rf`"
        );

        let mut p = crate::harness::fixture::file("codex");
        p["plugin"]["listing"] =
            json!({"installed": ["."], "available": [], "name": "name", "want": ["rf"]});
        let bare = crate::harness::Harness::of("bare", &p).expect("a harness");
        let listed = json!([{"name": "rf", "version": "0.1.2"}, {"name": "other", "version": "9"}]);
        assert_eq!(version(&bare, listed).as_deref(), Some("0.1.2"), "marketplaces, wanting `rf`");

        p["plugin"]["listing"] = json!({"installed": ["plugins"], "available": [], "name": "plugin.id",
                                        "on": ["active"], "want": ["rf"]});
        let nested = crate::harness::Harness::of("nested", &p).expect("a harness");
        let listed =
            json!({"plugins": [{"active": true, "plugin": {"id": "rf", "version": "0.1.2"}}]});
        assert_eq!(version(&nested, listed).as_deref(), Some("0.1.2"), "beside a dotted name");

        assert_eq!(
            version(&with(), json!({"installed": [{"id": "rf", "version": "0.1.2"}]})),
            None,
            "a harness with marketplaces and no `want` still names it `rf@fab7`"
        );
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
