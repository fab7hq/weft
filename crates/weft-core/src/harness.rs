//! The harnesses Weft can run, and how to find and prepare one.
//!
//! Weft names no harness (ADR-0015). Everything it knows about one comes from
//! RingFrame's harness profile, read through `ringframe profile list --json`
//! by the daemon and handed to every client: the name a person reads, the
//! program, where it keeps its configuration, the plugin commands, and how to
//! resume a session. A harness with no profile is not offered. Weft asks a
//! harness through its own documented commands and never reads its
//! configuration files.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Harness {
    /// The name RingFrame records, which is what everything else keys on.
    pub name: String,
    /// The name a person reads.
    pub title: String,
    /// The executable, as it is found on `PATH`.
    pub program: String,
    /// The variable that moves this harness's configuration home, when it
    /// has one.
    pub config_env: Option<String>,
    /// Where it lives when that variable is unset, under the person's home.
    pub config_default: String,
    /// Asks the harness what plugins it has, as JSON.
    pub list: Vec<String>,
    /// Adds the marketplace the `rf` plugin is published in, when the
    /// harness has marketplaces to add.
    pub add_marketplace: Option<Vec<String>>,
    /// Installs the plugin itself, when there is a source to install it from.
    pub install_plugin: Option<Vec<String>>,
    /// Refreshes this harness's copy of the marketplace, when it keeps one.
    pub update_marketplace: Option<Vec<String>>,
    /// Moves the installed plugin to the marketplace's version, when it can.
    pub update_plugin: Option<Vec<String>>,
    /// How this harness is told to pick a recorded session back up. The id is
    /// appended.
    pub resume: Vec<String>,
    /// What to press to see what scrolled away, when the harness keeps its own
    /// transcript instead of letting the terminal keep it. `None` means it
    /// prints inline and Weft's own scrollback is the whole of it.
    pub transcript: Option<String>,
    /// Turbo mode: the flags that grant this harness every permission and
    /// skip its questions, added to every agent Weft starts when Weft's
    /// `config.toml` turns turbo on. Empty when the profile names none.
    #[serde(default)]
    pub turbo: Vec<String>,
}

/// The marketplace and plugin RingFrame publishes. Named here once.
pub const MARKETPLACE: &str = "fab7";
pub const PLUGIN: &str = "rf@fab7";

impl Harness {
    /// One harness from its profile, as `ringframe profile show --json`
    /// returns it. A profile that does not define a harness — `unknown`, or
    /// one missing a field Weft needs — defines none.
    pub fn from_profile(p: &Value) -> Option<Self> {
        let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
        let argv = |v: &Value| -> Option<Vec<String>> {
            v.as_array()?.iter().map(|a| a.as_str().map(str::to_string)).collect()
        };
        let plugin = &p["plugin"];
        Some(Harness {
            name: text(&p["host"])?,
            title: text(&p["title"])?,
            program: text(&p["program"])?,
            config_env: text(&p["config"]["env"]),
            config_default: text(&p["config"]["default"])?,
            list: argv(&plugin["list"])?,
            add_marketplace: argv(&plugin["add_marketplace"]),
            install_plugin: argv(&plugin["install"]),
            update_marketplace: argv(&plugin["update_marketplace"]),
            update_plugin: argv(&plugin["update"]),
            resume: argv(&p["resume"])?,
            transcript: text(&p["transcript"]),
            turbo: argv(&p["turbo"]).unwrap_or_default(),
        })
    }

    /// The rule itself, separated from the environment so it can be tested
    /// without setting a variable every other test in the binary can see.
    /// `set` is the value of `config_env`; a harness with no variable has
    /// nothing that moves its home.
    pub fn config_home_from(&self, set: Option<std::ffi::OsString>, home: PathBuf) -> PathBuf {
        match set.filter(|_| self.config_env.is_some()) {
            Some(set) if !set.is_empty() => PathBuf::from(set),
            _ => home.join(&self.config_default),
        }
    }

    /// The command line that opens a recorded session again, exactly as a
    /// person would type it.
    pub fn resume_spec(&self, session: &str) -> String {
        let mut words = vec![self.program.clone()];
        words.extend(self.resume.iter().cloned());
        words.push(session.to_string());
        words.join(" ")
    }

    /// The command line that starts a fresh one.
    pub fn spec(&self) -> String {
        self.program.clone()
    }
}

/// Every harness the profiles define, in the order RingFrame lists them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Harnesses(pub Vec<Harness>);

impl Harnesses {
    /// From what `ringframe profile list --json` answers, keeping each profile
    /// that defines a harness.
    pub fn of(listing: &Value) -> Self {
        Harnesses(
            listing["profiles"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Harness::from_profile)
                .collect(),
        )
    }

    /// The harness RingFrame records under this name, if a profile defines it.
    pub fn find(&self, name: &str) -> Option<&Harness> {
        self.0.iter().find(|h| h.name == name)
    }

    /// The harness a command line starts, by its program name. `weft .
    /// "<program> --its --own --args"` names a harness the way a person would.
    pub fn for_program(&self, program: &str) -> Option<&Harness> {
        let file = program.rsplit('/').next().unwrap_or(program);
        self.0.iter().find(|h| h.program == file)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Harness> {
        self.0.iter()
    }

    /// The names a person reads, for a sentence: "A or B".
    pub fn titles(&self) -> String {
        let t: Vec<&str> = self.0.iter().map(|h| h.title.as_str()).collect();
        match t.as_slice() {
            [] => "a harness RingFrame has a profile for".into(),
            [one] => (*one).into(),
            [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
        }
    }
}

/// Test fixtures: RingFrame's own fixture profiles, read the way the daemon
/// reads `profile list`, so every harness test runs on profile-loaded data.
/// Built only for tests, and for the crates above that ask for `fixtures`.
#[cfg(any(test, feature = "fixtures"))]
pub mod fixture {
    use super::*;

    pub fn profile(name: &str) -> Value {
        let text = match name {
            "claude-code" => {
                include_str!("../../ringframe/tests/fixtures/config/harnesses/claude-code.toml")
            }
            "codex" => include_str!("../../ringframe/tests/fixtures/config/harnesses/codex.toml"),
            "unknown" => {
                include_str!("../../ringframe/tests/fixtures/config/harnesses/unknown.toml")
            }
            other => panic!("no fixture profile {other}"),
        };
        serde_json::to_value(text.parse::<toml::Table>().expect("toml")).expect("json")
    }

    /// The profiles RingFrame ships, and `unknown`, as `profile list` would
    /// answer: `unknown` defines no harness and is left out by `of`.
    pub fn harnesses() -> Harnesses {
        Harnesses::of(&serde_json::json!({"profiles": [
            profile("claude-code"), profile("codex"), profile("unknown")
        ]}))
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{harnesses, profile};
    use super::*;

    #[test]
    fn the_harnesses_are_the_ones_the_profiles_define() {
        let all = harnesses();
        let names: Vec<_> = all.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["claude-code", "codex"], "unknown defines none");
        assert_eq!(all.find("claude-code").map(|h| h.program.as_str()), Some("claude"));
        assert_eq!(all.find("codex").map(|h| h.title.as_str()), Some("Codex"));
        assert!(all.find("aider").is_none(), "nothing without a profile");
        assert_eq!(all.titles(), "Claude Code or Codex");
    }

    #[test]
    fn a_command_line_names_its_harness_the_way_a_person_would() {
        let all = harnesses();
        assert_eq!(all.for_program("claude").map(|h| h.name.as_str()), Some("claude-code"));
        assert_eq!(
            all.for_program("/opt/homebrew/bin/codex").map(|h| h.name.as_str()),
            Some("codex")
        );
        assert!(all.for_program("bash").is_none());
    }

    #[test]
    fn a_configuration_home_follows_the_harnesses_own_variable() {
        let all = harnesses();
        let home = PathBuf::from("/home/someone");
        for (name, var, set, unset) in [
            ("claude-code", "CLAUDE_CONFIG_DIR", "/elsewhere/claude", "/home/someone/.claude"),
            ("codex", "CODEX_HOME", "/elsewhere/codex", "/home/someone/.codex"),
        ] {
            let h = all.find(name).expect(name);
            assert_eq!(h.config_env.as_deref(), Some(var));
            assert_eq!(h.config_home_from(Some(set.into()), home.clone()), PathBuf::from(set));
            assert_eq!(h.config_home_from(None, home.clone()), PathBuf::from(unset));
            assert_eq!(
                h.config_home_from(Some("".into()), home.clone()),
                PathBuf::from(unset),
                "an empty variable is not a path"
            );
        }
    }

    #[test]
    fn a_recorded_session_is_reopened_the_way_each_harness_spells_it() {
        let all = harnesses();
        assert_eq!(
            all.find("claude-code")
                .expect("claude")
                .resume_spec("883b9d12-5745-4ffa-9aac-eaaeb7e0dd47"),
            "claude --resume 883b9d12-5745-4ffa-9aac-eaaeb7e0dd47"
        );
        assert_eq!(
            all.find("codex").expect("codex").resume_spec("01a0bdb6-1d1f-79c2-84b0-8b03496d7db0"),
            "codex resume 01a0bdb6-1d1f-79c2-84b0-8b03496d7db0"
        );
    }

    #[test]
    fn only_a_harness_that_keeps_its_own_transcript_names_a_key_for_it() {
        let all = harnesses();
        assert_eq!(all.find("codex").expect("codex").transcript.as_deref(), Some("Ctrl+T"));
        assert_eq!(all.find("claude-code").expect("claude").transcript, None);
    }

    #[test]
    fn every_harness_is_asked_for_json_so_nothing_parses_a_table() {
        for h in harnesses().iter() {
            assert!(h.list.iter().any(|a| a == "--json"), "{} must answer in JSON", h.name);
            assert_eq!(h.list[0], "plugin");
        }
    }

    /// Turbo mode's flags are the profile's, for each harness; none without.
    #[test]
    fn turbo_is_each_harnesses_own_flag_from_its_profile() {
        let all = harnesses();
        assert_eq!(
            all.find("claude-code").expect("claude").turbo,
            ["--dangerously-skip-permissions"]
        );
        assert_eq!(
            all.find("codex").expect("codex").turbo,
            ["--dangerously-bypass-approvals-and-sandbox"]
        );
        let mut p = profile("codex");
        p["turbo"] = Value::Null;
        assert!(
            Harness::from_profile(&p).expect("a harness").turbo.is_empty(),
            "no turbo, no flags"
        );
    }

    /// A harness keeps its configuration where it keeps it, whether or not a
    /// variable moves it, and may have no marketplace to add: a profile says
    /// only what is true of it, and Weft asks nothing it has no command for.
    #[test]
    fn a_harness_may_have_no_configuration_variable_and_no_marketplace_commands() {
        let mut p = profile("codex");
        p["config"] = serde_json::json!({"default": ".elsewhere/agent"});
        p["plugin"]["add_marketplace"] = Value::Null;
        p["plugin"]["update_marketplace"] = Value::Null;
        let h = Harness::from_profile(&p).expect("still a harness");
        assert_eq!(h.config_env, None);
        let home = PathBuf::from("/home/someone");
        assert_eq!(
            h.config_home_from(Some("/set".into()), home.clone()),
            PathBuf::from("/home/someone/.elsewhere/agent"),
            "no variable, so nothing set moves it"
        );
        assert_eq!(h.add_marketplace, None);
        assert_eq!(h.update_marketplace, None);
    }

    /// harness-profile.md §5.3: a made-up third harness is read and offered
    /// from its profile alone.
    #[test]
    fn a_made_up_third_harness_is_read_from_its_profile_alone() {
        let mut zed = profile("codex");
        zed["host"] = "zed-agent".into();
        zed["title"] = "Zed Agent".into();
        zed["program"] = "zed-agent".into();
        zed["resume"] = serde_json::json!(["--thread"]);
        zed["transcript"] = Value::Null;
        let all = Harnesses::of(&serde_json::json!({"profiles": [profile("claude-code"), zed]}));
        let h = all.for_program("zed-agent").expect("offered");
        assert_eq!(h.resume_spec("t1"), "zed-agent --thread t1");
        assert_eq!(all.titles(), "Claude Code or Zed Agent");
        let mut broken = profile("codex");
        broken["plugin"] = Value::Null;
        assert_eq!(Harness::from_profile(&broken), None, "a profile missing a field defines none");
    }
}
