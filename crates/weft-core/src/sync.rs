//! The RingFrame view: the latest release, what the configuration and each
//! harness need to reach it, and the commands that get them there, in order.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::harness::Harness;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mark {
    Waiting,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub program: String,
    pub args: Vec<String>,
    pub mark: Mark,
    /// The last lines a failed command printed.
    #[serde(default)]
    pub said: String,
    /// Its failure does not stop the steps after it: adding a marketplace the
    /// harness already has fails, and the install that follows still runs, and
    /// says for itself whether the marketplace is there.
    #[serde(default)]
    pub may_fail: bool,
}

impl Step {
    pub fn new(program: &str, args: &[impl AsRef<str>]) -> Self {
        Step {
            program: program.into(),
            args: args.iter().map(|a| a.as_ref().to_string()).collect(),
            mark: Mark::Waiting,
            said: String::new(),
            may_fail: false,
        }
    }

    /// The same step, its failure not stopping the run.
    pub fn may_fail(mut self) -> Self {
        self.may_fail = true;
        self
    }

    pub fn line(&self) -> String {
        format!("{} {}", self.program, self.args.join(" "))
    }
}

/// The installed `rf` version, from the same listing readiness reads, under
/// the name readiness looks for: `rf@fab7` where the harness has
/// marketplaces, `rf` where it has none or its file wants `rf`. The version
/// is the row's own, or beside a dotted name (`plugin.version` for
/// `plugin.id`).
pub fn installed(h: &Harness, listing: &Value) -> Option<String> {
    use crate::readiness::field;
    let shape = h.listing.as_ref()?;
    let plugin = crate::harness::PLUGIN;
    let short = plugin.split('@').next().unwrap_or(plugin);
    let wanted = if shape.want.iter().any(|w| w == short) || h.add_marketplace.is_none() {
        short
    } else {
        plugin
    };
    let beside = shape.name.rsplit_once('.').map(|(at, _)| format!("{at}.version"));
    crate::readiness::rows(listing, &shape.installed).iter().find_map(|row| {
        (field(row, &shape.name)?.as_str()? == wanted).then_some(())?;
        let version = beside.as_deref().and_then(|b| field(row, b)).or_else(|| row.get("version"));
        version?.as_str().map(str::to_string)
    })
}

/// The version a plugin's `plugin.json` gives, for a harness whose listing
/// gives none.
pub fn manifest_version(text: &str) -> Option<String> {
    let manifest: Value = serde_json::from_str(text).ok()?;
    manifest["version"].as_str().filter(|v| !v.is_empty()).map(str::to_string)
}

/// The configuration's line, from what `ringframe sync --check` printed (or
/// `None` when it could not tell), and the step that catches it up when it
/// is behind. Versions read as Weft's do: no `v`.
pub fn configuration(check: Option<&Value>) -> (String, Option<Step>) {
    let text = |v: &Value, k: &str| {
        v[k].as_str().map(|t| t.trim_start_matches('v').to_string()).unwrap_or_default()
    };
    match check {
        None => ("could not reach the latest release".into(), None),
        Some(c) if c["behind"] == true => (
            format!("{} → {}", text(c, "revision"), text(c, "latest")),
            Some(Step::new("ringframe", &["sync"])),
        ),
        Some(c) if c["revision"] == "local" => ("local, not replaced".into(), None),
        Some(c) if c["latest"].is_string() => {
            (format!("{} is the latest", text(c, "latest")), None)
        }
        Some(_) => ("up to date".into(), None),
    }
}

/// Where Weft's releases are published.
pub const WEFT_REPO: &str = "fab7hq/weft";

/// A release tag's version: `v`, then numbers and dots, and nothing else.
pub fn release_version(tag: &str) -> Option<&str> {
    let v = tag.strip_prefix('v')?;
    (!v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.')).then_some(v)
}

/// Weft's own line: where this Weft stands against the latest release (`None`
/// when it could not be reached), and whether it is behind. Weft is updated
/// the way it was installed, so no step catches it up. `waiting` says the
/// latest is already on this machine and starts once no agent runs.
pub fn weft(have: &str, latest: Option<&str>, waiting: bool) -> (String, bool) {
    match latest {
        None => ("could not reach the latest release".into(), false),
        Some(l) if older(have, l) && waiting => {
            (format!("{l} is installed: it starts when no agent is running"), false)
        }
        Some(l) if older(have, l) => (format!("{have} → {l}"), true),
        Some(_) => (format!("{have} is the latest"), false),
    }
}

/// Whether `have` is an older version than `latest`, part by part.
pub fn older(have: &str, latest: &str) -> bool {
    let parts = |s: &str| s.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parts(have) < parts(latest)
}

/// Sense bundle's line: where the installed `~/.fab7/weft/sense/current.json`
/// bundle stands against the latest release, and the step that updates and
/// hot-reloads it (`ADR-0003`, `D7.2`).
pub fn sense(have: Option<&str>, latest: Option<&str>) -> (String, Option<Step>) {
    let have = have.map(|s| s.trim_start_matches('v')).filter(|s| !s.is_empty());
    let latest = latest.map(|s| s.trim_start_matches('v')).filter(|s| !s.is_empty());
    match (have, latest) {
        (Some(h), Some(l)) if older(h, l) => {
            (format!("{h} → {l}"), Some(Step::new("weft", &["update", "--sense"])))
        }
        (None, Some(l)) => {
            (format!("not installed → {l}"), Some(Step::new("weft", &["update", "--sense"])))
        }
        (Some(h), Some(_)) => (format!("{h} is the latest"), None),
        (Some(h), None) => (format!("{h} is installed"), None),
        (None, None) => ("could not reach the latest release".into(), None),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn find(name: &str) -> Option<crate::harness::Harness> {
        crate::harness::fixture::harnesses().find(name).cloned()
    }

    #[test]
    fn the_configuration_is_behind_local_current_or_out_of_reach() {
        let behind = json!({"revision": "v0.1.0", "latest": "v0.1.1", "behind": true});
        let (line, step) = configuration(Some(&behind));
        assert_eq!(
            (line.as_str(), step.map(|s| s.line())),
            ("0.1.0 → 0.1.1", Some("ringframe sync".into()))
        );
        let local = json!({"revision": "local", "latest": "v0.1.1", "behind": false});
        assert_eq!(configuration(Some(&local)).0, "local, not replaced");
        assert_eq!(configuration(Some(&json!({"behind": false}))).0, "up to date");
        let current = json!({"revision": "v0.2.0", "latest": "v0.2.0", "behind": false});
        assert_eq!(configuration(Some(&current)).0, "0.2.0 is the latest");
        assert_eq!(configuration(None).0, "could not reach the latest release");
        assert!(older("0.1.2", "0.1.10") && !older("0.1.3", "0.1.3"));
    }

    #[test]
    fn only_a_release_tag_names_a_version() {
        assert_eq!(release_version("v0.2.1"), Some("0.2.1"));
        for tag in ["0.2.1", "v", "v0.2.1;rm -rf ~", "v0.2.1-rc1", "main"] {
            assert_eq!(release_version(tag), None, "{tag:?}");
        }
    }

    #[test]
    fn weft_is_behind_installed_current_or_out_of_reach() {
        assert_eq!(weft("0.2.0", Some("0.2.1"), false), ("0.2.0 → 0.2.1".into(), true));
        let (line, behind) = weft("0.2.0", Some("0.2.1"), true);
        assert_eq!(line, "0.2.1 is installed: it starts when no agent is running");
        assert!(!behind);
        assert_eq!(weft("0.2.1", Some("0.2.1"), false).0, "0.2.1 is the latest");
        assert_eq!(weft("0.2.1", None, false).0, "could not reach the latest release");
    }

    #[test]
    fn sense_is_behind_current_or_uninstalled() {
        let (line, step) = sense(Some("0.1.0"), Some("0.2.0"));
        assert_eq!(
            (line.as_str(), step.map(|s| s.line())),
            ("0.1.0 → 0.2.0", Some("weft update --sense".into()))
        );
        assert_eq!(sense(Some("0.2.0"), Some("0.2.0")).0, "0.2.0 is the latest");
        assert_eq!(sense(Some("0.2.0"), None).0, "0.2.0 is installed");
        let (line, step) = sense(None, Some("0.2.0"));
        assert_eq!(
            (line.as_str(), step.map(|s| s.line())),
            ("not installed → 0.2.0", Some("weft update --sense".into()))
        );
    }

    #[test]
    fn the_installed_version_comes_from_either_harness_listing() {
        assert_eq!(
            installed(
                &find("claude-code").unwrap(),
                &json!({"installed": [{"id": "rf@fab7", "version": "0.1.2"}]})
            )
            .as_deref(),
            Some("0.1.2")
        );
        assert_eq!(
            installed(
                &find("codex").unwrap(),
                &json!({"installed": [{"pluginId": "rf@fab7", "version": "0.1.1"}]})
            )
            .as_deref(),
            Some("0.1.1")
        );
        assert_eq!(installed(&find("codex").unwrap(), &json!({"installed": []})), None);
    }
}
