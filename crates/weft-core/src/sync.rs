//! The RingFrame view: the latest release, what the configuration and each
//! harness need to reach it, and the commands that get them there, in order.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::harness::Harness;
use crate::readiness::{Gap, Readiness};

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
}

impl Step {
    fn new(program: &str, args: &[impl AsRef<str>]) -> Self {
        Step {
            program: program.into(),
            args: args.iter().map(|a| a.as_ref().to_string()).collect(),
            mark: Mark::Waiting,
            said: String::new(),
        }
    }

    pub fn line(&self) -> String {
        format!("{} {}", self.program, self.args.join(" "))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct View {
    /// The latest release and its `rf` version, when it could be reached.
    pub latest: Option<String>,
    pub plugin: Option<String>,
    /// What each row stands at: the configuration first, then each harness.
    pub rows: Vec<(String, String)>,
    pub steps: Vec<Step>,
    pub running: bool,
}

impl View {
    pub fn needs_anything(&self) -> bool {
        self.steps.iter().any(|s| s.mark != Mark::Done)
    }
}

/// The installed `rf` version, from the same listing readiness reads.
pub fn installed(h: &Harness, listing: &Value) -> Option<String> {
    let shape = h.listing.as_ref()?;
    crate::readiness::rows(listing, &shape.installed).iter().find_map(|row| {
        let id = row.get(&shape.name)?.as_str()?;
        (id == crate::harness::PLUGIN).then(|| row["version"].as_str().map(str::to_string))?
    })
}

/// `check` is what `ringframe sync --check` printed, or `None` when it could
/// not tell. Each harness comes with its readiness and its installed version.
pub fn view(check: Option<&Value>, harnesses: &[(&Harness, Readiness, Option<String>)]) -> View {
    let text = |v: &Value, k: &str| v[k].as_str().map(str::to_string);
    let latest = check.and_then(|c| text(c, "latest"));
    let plugin = check.and_then(|c| text(c, "plugin"));
    let mut v = View { latest: latest.clone(), plugin: plugin.clone(), ..View::default() };

    let config = match check {
        None => "could not reach the latest release".to_string(),
        Some(c) if c["behind"] == true => {
            v.steps.push(Step::new("ringframe", &["sync"]));
            format!("{} → {}", text(c, "revision").unwrap_or_default(), latest.unwrap_or_default())
        }
        Some(c) if c["revision"] == "local" => "local, not replaced".into(),
        Some(_) => "up to date".into(),
    };
    v.rows.push(("configuration".into(), config));

    for (h, state, version) in harnesses {
        let now = match (state, version) {
            (Readiness::Missing(Gap::Cli), _) => "RingFrame is not installed".into(),
            (Readiness::Unknown, _) => "could not ask it".into(),
            // A harness with nothing to install from gets no step it
            // cannot run.
            (Readiness::Missing(_), _) if h.install_plugin.is_none() => {
                "not set up; Weft cannot install it".into()
            }
            (Readiness::Missing(gap), _) => {
                if let (Gap::Marketplace, Some(add)) = (gap, &h.add_marketplace) {
                    v.steps.push(Step::new(&h.program, add));
                }
                v.steps.extend(h.install_plugin.iter().map(|i| Step::new(&h.program, i)));
                "not set up".into()
            }
            (Readiness::Ready, Some(have)) if plugin.as_deref().is_some_and(|p| older(have, p)) => {
                match &h.update_plugin {
                    None => format!("rf {have}; Weft cannot update it"),
                    Some(update) => {
                        if let Some(refresh) = &h.update_marketplace {
                            v.steps.push(Step::new(&h.program, refresh));
                        }
                        v.steps.push(Step::new(&h.program, update));
                        format!("rf {have} → {}", plugin.as_deref().unwrap_or_default())
                    }
                }
            }
            (Readiness::Ready, Some(have)) => format!("rf {have}, up to date"),
            (Readiness::Ready, None) => "up to date".into(),
        };
        v.rows.push((h.name.clone(), now));
    }
    v
}

fn older(have: &str, latest: &str) -> bool {
    let parts = |s: &str| s.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parts(have) < parts(latest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn find(name: &str) -> Option<crate::harness::Harness> {
        crate::harness::fixture::harnesses().find(name).cloned()
    }

    fn lines(v: &View) -> Vec<String> {
        v.steps.iter().map(Step::line).collect()
    }

    #[test]
    fn behind_everywhere_runs_configuration_then_each_harness_in_order() {
        let check =
            json!({"revision": "v0.1.0", "latest": "v0.1.1", "plugin": "0.1.2", "behind": true});
        let claude = find("claude-code").unwrap();
        let codex = find("codex").unwrap();
        let v = view(
            Some(&check),
            &[
                (&claude, Readiness::Ready, Some("0.1.1".into())),
                (&codex, Readiness::Missing(Gap::Marketplace), None),
            ],
        );
        assert_eq!(
            lines(&v),
            [
                "ringframe sync",
                "claude plugin marketplace update fab7",
                "claude plugin update rf@fab7",
                "codex plugin marketplace add fab7hq/fab7",
                "codex plugin add rf@fab7",
            ]
        );
        assert_eq!(v.rows[0].1, "v0.1.0 → v0.1.1");
        assert_eq!(v.rows[1].1, "rf 0.1.1 → 0.1.2");
        assert_eq!(v.rows[2].1, "not set up");
    }

    #[test]
    fn nothing_to_run_when_current_local_or_unreachable() {
        let claude = find("claude-code").unwrap();
        let current =
            json!({"revision": "v0.1.1", "latest": "v0.1.1", "plugin": "0.1.2", "behind": false});
        let v = view(Some(&current), &[(&claude, Readiness::Ready, Some("0.1.2".into()))]);
        assert!(!v.needs_anything());
        let local =
            json!({"revision": "local", "latest": "v0.1.1", "plugin": "0.1.2", "behind": false});
        assert_eq!(view(Some(&local), &[]).rows[0].1, "local, not replaced");
        // Unreachable still offers setup, which needs no network.
        let v = view(None, &[(&claude, Readiness::Missing(Gap::Plugin), None)]);
        assert_eq!(lines(&v), ["claude plugin install rf@fab7 --scope user"]);
    }

    /// A harness whose plugin has no source to install from yet names no
    /// install or update: it is still offered, and nothing is run that
    /// cannot work.
    #[test]
    fn a_harness_with_no_plugin_command_gets_no_step_it_cannot_run() {
        let mut p = crate::harness::fixture::file("codex");
        p["plugin"] = json!({"list": ["plugin", "list"]});
        let bare = crate::harness::Harness::of("codex", &p).expect("still a harness");
        let check =
            json!({"revision": "v0.1.1", "latest": "v0.1.1", "plugin": "0.1.2", "behind": false});
        let v = view(
            Some(&check),
            &[
                (&bare, Readiness::Missing(Gap::Plugin), None),
                (&bare, Readiness::Missing(Gap::Marketplace), None),
                (&bare, Readiness::Ready, Some("0.1.1".into())),
            ],
        );
        assert!(lines(&v).is_empty(), "{:?}", lines(&v));
        assert_eq!(v.rows[1].1, "not set up; Weft cannot install it");
        assert_eq!(v.rows[3].1, "rf 0.1.1; Weft cannot update it");
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
