//! The RINGFRAME view (`[U]`): RingFrame's configuration and every harness
//! Weft has a file for, whether each can be started, whether RingFrame's
//! plugin is in it and current, and the harness's own commands that set it up
//! or catch it up. One place for both.
//!
//! Weft names no harness: the rows are the harness files, by title. The tick
//! is the harness's own answer (readiness), asked again after the commands
//! run, never a command's exit code, and never kept: a kept tick would
//! outlive an uninstall. Signing a harness in is the person's; nothing here
//! asks.

use serde::{Deserialize, Serialize};

use crate::harness::Harness;
use crate::readiness::{Gap, Readiness};
use crate::sync::{Step, older};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// RingFrame's plugin is in it, on, and current.
    Ready { version: Option<String> },
    /// The plugin is in it and an older version than the latest release's.
    Behind { have: String, latest: String },
    /// Its program is not on `PATH` and not where the person said.
    NotFound,
    /// It runs; the plugin is not in it, and its file says how to put it there.
    NotSetUp,
    /// It runs; the plugin is not in it, or behind, and its file names no
    /// command that installs or updates it.
    CannotInstall,
    /// It would not say what it has.
    Unknown,
    /// It is being asked what it has, and has not answered yet.
    Checking,
    /// The `ringframe` CLI is missing: Weft's installer put it there, so it
    /// is a reinstall of Weft, not a step here.
    NoCli,
    /// A step is running: its command line.
    Running(String),
    /// A step failed: its command line and the last lines it printed.
    Failed { step: String, said: String },
}

impl State {
    pub fn mark(&self) -> &'static str {
        match self {
            State::Ready { .. } => "✓",
            State::Behind { .. } => "↑",
            State::NotFound | State::NotSetUp | State::CannotInstall | State::NoCli => "✗",
            State::Unknown => "?",
            State::Running(_) | State::Checking => "…",
            State::Failed { .. } => "!",
        }
    }

    /// What the row says after its name.
    pub fn words(&self) -> String {
        match self {
            State::Ready { version: Some(v) } => format!("ready · rf {v}"),
            State::Ready { version: None } => "ready".into(),
            State::Behind { have, latest } => format!("rf {have} → {latest}"),
            State::NotFound => "not found".into(),
            State::NotSetUp => "not set up".into(),
            State::CannotInstall => "Weft cannot do this one: its setup page says how".into(),
            State::Unknown => "would not say whether it is set up".into(),
            State::Checking => "checking".into(),
            State::NoCli => "RingFrame is missing: run Weft's installer again".into(),
            State::Running(line) => format!("running: {line}"),
            State::Failed { step, .. } => format!("failed: {step}"),
        }
    }

    /// What `Enter` does on this row.
    pub fn next(&self) -> Option<Next> {
        match self {
            State::NotFound => Some(Next::Locate),
            State::NotSetUp | State::Failed { .. } => Some(Next::SetUp),
            State::Behind { .. } => Some(Next::Update),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Ask where its program is.
    Locate,
    /// Run its setup commands.
    SetUp,
    /// Run its update commands.
    Update,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub name: String,
    pub title: String,
    /// The program Weft runs for it: a name looked up on `PATH`, or where the
    /// person said it is.
    pub program: String,
    pub state: State,
    /// Its harness file marks it as beta: said on the row, and nothing else
    /// about it differs.
    #[serde(default)]
    pub beta: bool,
}

impl Row {
    /// The row as the view prints it: the mark, the name, and what it needs,
    /// with `beta` before that for a harness its file marks as beta.
    pub fn line(&self) -> String {
        let beta = if self.beta { "beta · " } else { "" };
        format!("{}  {:<14}{beta}{}", self.state.mark(), self.title, self.state.words())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// The latest release, and its `rf` version, when it could be reached.
    #[serde(default)]
    pub latest: Option<String>,
    #[serde(default)]
    pub plugin: Option<String>,
    /// RingFrame's configuration: what it stands at, and whether a step
    /// catches it up.
    #[serde(default)]
    pub configuration: String,
    #[serde(default)]
    pub configuration_behind: bool,
    /// The latest release has not answered yet: `latest`, `plugin` and
    /// `configuration` say nothing until it has.
    #[serde(default)]
    pub pending_release: bool,
    /// Every harness, by title.
    pub rows: Vec<Row>,
    /// What the last thing the person did came to, when it did not work: a
    /// path that is not a program, a config.toml that could not be edited, a
    /// configuration sync that failed.
    #[serde(default)]
    pub note: Option<String>,
}

impl View {
    /// Nothing is ready and something could be: what makes the view open by
    /// itself.
    pub fn wants_setting_up(&self) -> bool {
        !self.rows.is_empty()
            && !self
                .rows
                .iter()
                .any(|r| matches!(r.state, State::Ready { .. } | State::Behind { .. }))
            && self.rows.iter().any(|r| r.state.next().is_some())
    }

    /// Something is behind, or found and not set up: what lights `↑ [U]PDATE`.
    /// A harness that is not on this machine lights nothing.
    pub fn needs_anything(&self) -> bool {
        self.configuration_behind
            || self.rows.iter().any(|r| {
                matches!(r.state, State::Behind { .. } | State::NotSetUp | State::Failed { .. })
            })
    }

    /// Whether `[P]ROCEED` has anything to run: the configuration, and every
    /// harness to set up or update. Finding a program is the person's to say.
    pub fn proceeds(&self) -> bool {
        self.configuration_behind
            || self.rows.iter().any(|r| matches!(r.state.next(), Some(Next::SetUp | Next::Update)))
    }

    pub fn row(&self, name: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.name == name)
    }
}

/// The commands that bring this harness to RingFrame's plugin, as its file
/// names them, for what its readiness says: the marketplace then the plugin
/// when the plugin is missing; its marketplace refresh then its update when it
/// is behind the latest release.
pub fn steps(h: &Harness, state: Readiness, have: Option<&str>, latest: Option<&str>) -> Vec<Step> {
    let mut out = Vec::new();
    match state {
        Readiness::Missing(gap @ (Gap::Marketplace | Gap::Plugin)) => {
            if let (Gap::Marketplace, Some(add)) = (gap, &h.add_marketplace) {
                // A harness that already has the marketplace refuses to add it
                // again; its install is what says whether it is there.
                let add = Step::new(&h.program, &h.argv(add));
                out.push(if h.install_plugin.is_some() { add.may_fail() } else { add });
            }
            out.extend(h.install_plugin.iter().map(|i| Step::new(&h.program, &h.argv(i))));
        }
        Readiness::Ready if behind(have, latest) => {
            if let Some(update) = &h.update_plugin {
                if let Some(refresh) = &h.update_marketplace {
                    out.push(Step::new(&h.program, &h.argv(refresh)));
                }
                out.push(Step::new(&h.program, &h.argv(update)));
            }
        }
        _ => {}
    }
    out
}

fn behind(have: Option<&str>, latest: Option<&str>) -> bool {
    matches!((have, latest), (Some(h), Some(l)) if older(h, l))
}

/// One harness's row while it is being asked: found, with no answer yet.
pub fn checking(h: &Harness) -> Row {
    Row {
        name: h.name.clone(),
        title: h.title.clone(),
        program: h.program.clone(),
        state: State::Checking,
        beta: h.beta,
    }
}

/// One harness's row, from what the machine said of it: whether its program
/// was found, and when it was, its readiness and installed `rf` version;
/// `latest` is the latest release's `rf`.
pub fn row(h: &Harness, found: Option<(Readiness, Option<String>)>, latest: Option<&str>) -> Row {
    let state = match found {
        None => State::NotFound,
        Some((Readiness::Ready, Some(have))) if behind(Some(&have), latest) => {
            if h.update_plugin.is_some() {
                State::Behind { have, latest: latest.unwrap_or_default().to_string() }
            } else {
                State::CannotInstall
            }
        }
        Some((Readiness::Ready, version)) => State::Ready { version },
        Some((Readiness::Missing(Gap::Cli), _)) => State::NoCli,
        Some((Readiness::Unknown, _)) => State::Unknown,
        Some((r @ Readiness::Missing(_), _)) if steps(h, r, None, None).is_empty() => {
            State::CannotInstall
        }
        Some((Readiness::Missing(_), _)) => State::NotSetUp,
    };
    Row {
        name: h.name.clone(),
        title: h.title.clone(),
        program: h.program.clone(),
        state,
        beta: h.beta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harness(name: &str) -> Harness {
        Harness::of(name, &crate::harness::fixture::file(name)).expect(name)
    }

    fn lines(steps: &[Step]) -> Vec<String> {
        steps.iter().map(Step::line).collect()
    }

    #[test]
    fn a_harness_is_set_up_or_updated_by_its_own_commands() {
        let claude = harness("claude-code");
        assert_eq!(
            lines(&steps(&claude, Readiness::Missing(Gap::Marketplace), None, Some("0.1.3"))),
            [
                "claude plugin marketplace add fab7hq/fab7",
                "claude plugin install rf@fab7 --scope user"
            ]
        );
        assert_eq!(
            lines(&steps(&claude, Readiness::Ready, Some("0.1.2"), Some("0.1.3"))),
            ["claude plugin marketplace update fab7", "claude plugin update rf@fab7"],
            "behind: its refresh, then its update"
        );
        assert!(steps(&claude, Readiness::Ready, Some("0.1.3"), Some("0.1.3")).is_empty());
        assert_eq!(
            lines(&steps(&harness("antigravity"), Readiness::Missing(Gap::Plugin), None, None)),
            [
                "agy plugin install https://github.com/fab7hq/fab7/tree/main/products/ringframe/plugins/antigravity"
            ],
            "no marketplace to add: the repository, at the plugin's path"
        );
    }

    /// A row still being asked says so, offers nothing, and neither lights
    /// `↑ [U]PDATE` nor opens the view by itself.
    #[test]
    fn a_row_being_asked_waits_quietly() {
        let r = checking(&harness("antigravity"));
        assert_eq!(
            (r.state.mark(), r.state.words().as_str(), r.state.next()),
            ("…", "checking", None)
        );
        let v = View { pending_release: true, rows: vec![r], ..View::default() };
        assert!(!v.needs_anything() && !v.wants_setting_up() && !v.proceeds());
    }

    /// A harness file that says `beta = true` is said to be beta on its row,
    /// whatever its state; the others read as before.
    #[test]
    fn a_beta_harness_says_so_on_its_row_and_only_there() {
        let mut p = crate::harness::fixture::file("antigravity");
        p["beta"] = serde_json::json!(true);
        let beta = Harness::of("fresh", &p).expect("a harness");
        assert!(beta.beta);
        let r = row(&beta, Some((Readiness::Ready, None)), None);
        assert_eq!(r.line(), format!("✓  {:<14}beta · ready", beta.title));
        let r = row(&beta, Some((Readiness::Missing(Gap::Plugin), None)), None);
        assert!(r.line().ends_with("beta · not set up"), "{}", r.line());

        let plain = harness("antigravity");
        assert!(!plain.beta, "absent means not beta");
        let r = row(&plain, Some((Readiness::Ready, None)), None);
        assert_eq!(r.line(), format!("✓  {:<14}ready", plain.title));
    }

    /// A file that can add the marketplace but names no install is still set
    /// up from here: adding the marketplace is a step.
    #[test]
    fn a_marketplace_alone_is_a_step_to_offer() {
        let mut p = crate::harness::fixture::file("codex");
        p["plugin"]["install"] = serde_json::Value::Null;
        p["plugin"]["update"] = serde_json::Value::Null;
        let h = Harness::of("marketplace-only", &p).expect("a harness");
        let r = row(&h, Some((Readiness::Missing(Gap::Marketplace), None)), None);
        assert_eq!((r.state.clone(), r.state.next()), (State::NotSetUp, Some(Next::SetUp)));
        assert_eq!(steps(&h, Readiness::Missing(Gap::Marketplace), None, None).len(), 1);
        let r = row(&h, Some((Readiness::Missing(Gap::Plugin), None)), None);
        assert_eq!(r.state, State::CannotInstall, "no install, and the marketplace is there");

        p["args"] = serde_json::json!(["agent"]);
        let via = Harness::of("via", &p).expect("a harness");
        assert_eq!(
            lines(&steps(&via, Readiness::Missing(Gap::Marketplace), None, None)),
            [format!("{} agent plugin marketplace add fab7hq/fab7", via.program)],
            "each step runs with the harness's args first"
        );
        assert!(
            !steps(&via, Readiness::Missing(Gap::Marketplace), None, None)[0].may_fail,
            "nothing after it"
        );
        let claude = harness("claude-code");
        let both = steps(&claude, Readiness::Missing(Gap::Marketplace), None, None);
        assert_eq!(
            both.iter().map(|s| s.may_fail).collect::<Vec<_>>(),
            [true, false],
            "an add that fails because the marketplace is there does not stop its install"
        );
    }

    #[test]
    fn each_row_says_what_it_needs_and_what_enter_does() {
        let claude = harness("claude-code");
        let r = row(&claude, None, None);
        assert_eq!((r.state.mark(), r.state.next()), ("✗", Some(Next::Locate)));
        let r = row(&claude, Some((Readiness::Missing(Gap::Plugin), None)), None);
        assert_eq!((r.state.clone(), r.state.next()), (State::NotSetUp, Some(Next::SetUp)));
        let r = row(&claude, Some((Readiness::Ready, Some("0.1.3".into()))), Some("0.1.3"));
        assert_eq!((r.state.words().as_str(), r.state.next()), ("ready · rf 0.1.3", None));
        let r = row(&claude, Some((Readiness::Ready, Some("0.1.2".into()))), Some("0.1.3"));
        assert_eq!(
            (r.state.words().as_str(), r.state.next()),
            ("rf 0.1.2 → 0.1.3", Some(Next::Update))
        );
        assert_eq!(
            row(&claude, Some((Readiness::Missing(Gap::Cli), None)), None).state,
            State::NoCli
        );
        let mut bare = claude.clone();
        bare.install_plugin = None;
        bare.update_plugin = None;
        assert_eq!(
            row(&bare, Some((Readiness::Missing(Gap::Plugin), None)), None).state,
            State::CannotInstall
        );
        assert_eq!(
            row(&bare, Some((Readiness::Ready, Some("0.1.1".into()))), Some("0.1.3")).state,
            State::CannotInstall,
            "behind, with nothing that updates it"
        );
    }

    #[test]
    fn it_opens_by_itself_while_nothing_is_ready_and_lights_only_what_can_be_done() {
        let claude = harness("claude-code");
        let codex = harness("codex");
        let not_set = |h: &Harness| row(h, Some((Readiness::Missing(Gap::Plugin), None)), None);
        let mut v =
            View { rows: vec![not_set(&claude), row(&codex, None, None)], ..View::default() };
        assert!(v.wants_setting_up() && v.needs_anything() && v.proceeds());
        v.rows[0] = row(&claude, Some((Readiness::Ready, None)), None);
        assert!(!v.wants_setting_up(), "one ready is enough");
        assert!(
            !v.needs_anything() && !v.proceeds(),
            "a harness not on this machine lights nothing"
        );
        v.configuration_behind = true;
        assert!(v.needs_anything() && v.proceeds());
    }
}
