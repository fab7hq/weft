//! The harnesses Weft can run, and how to find and prepare one.
//!
//! Weft names no harness (ADR-0015). Everything it knows about one comes from
//! that harness's Weft harness file, `~/.fab7/weft/harnesses/<id>.toml`,
//! shipped by fab7, read by the daemon and handed to every client: the name a
//! person reads, the program, where it keeps its configuration, the plugin
//! commands, and how to resume a session. A harness with no file is not
//! offered. RingFrame's own facts about a harness, such as how its skills are
//! typed, stay in RingFrame's profile and are asked of RingFrame. Weft asks a
//! harness through its own documented commands and never reads its
//! configuration files.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Harness {
    /// The name RingFrame records, which is what everything else keys on:
    /// its harness file's name.
    pub name: String,
    /// The name a person reads.
    pub title: String,
    /// The executable, as it is found on `PATH`.
    pub program: String,
    /// The words that always follow the program, for a harness reached
    /// through another command (`cursor agent`): every command Weft runs for
    /// it starts with them, and finding it on `PATH` looks for the program
    /// alone. Empty for most.
    #[serde(default)]
    pub args: Vec<String>,
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
    /// How what `list` answers reads, when the file says: without it, Weft
    /// cannot tell whether the plugin is there.
    pub listing: Option<Listing>,
    /// How this harness is told to pick a recorded session back up. The id is
    /// appended.
    pub resume: Vec<String>,
    /// What to press to see what scrolled away, when the harness keeps its own
    /// transcript instead of letting the terminal keep it. `None` means it
    /// prints inline and Weft's own scrollback is the whole of it.
    pub transcript: Option<String>,
    /// Turbo mode: the flags that grant this harness every permission and
    /// skip its questions, added to every agent Weft starts when Weft's
    /// `config.toml` turns turbo on. Empty when the file names none.
    #[serde(default)]
    pub turbo: Vec<String>,
    /// Whether its file marks it as beta: offered and used like any other
    /// harness, and said to be beta where its setup is shown, because it has
    /// not been tested as fully as the rest.
    #[serde(default)]
    pub beta: bool,
}

/// How a harness's plugin listing reads: the lists holding what it has
/// installed, the lists holding what it could install, the field that names
/// a plugin in either, and the fields of a row that say it is off when false.
/// A list named `.` is the listing itself, for a harness that answers with a
/// bare array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub installed: Vec<String>,
    pub available: Vec<String>,
    pub name: String,
    #[serde(default)]
    pub on: Vec<String>,
    /// The names that say RingFrame is set up, every one of them listed as
    /// installed and on, for a harness whose listing never names the plugin
    /// as such (it lists skills, or marketplaces). Empty: the plugin itself.
    #[serde(default)]
    pub want: Vec<String>,
    /// What `list` prints instead of JSON when nothing is installed, for a
    /// harness that says so in words.
    #[serde(default)]
    pub none: Option<String>,
}

/// The marketplace and plugin RingFrame publishes. Named here once.
pub const MARKETPLACE: &str = "fab7";
pub const PLUGIN: &str = "rf@fab7";

impl Harness {
    /// One harness from its Weft harness file, `<id>.toml`, as a table. A
    /// file missing a field Weft needs defines none.
    pub fn of(id: &str, p: &Value) -> Option<Self> {
        let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
        let argv = |v: &Value| -> Option<Vec<String>> {
            v.as_array()?.iter().map(|a| a.as_str().map(str::to_string)).collect()
        };
        let plugin = &p["plugin"];
        Some(Harness {
            name: Some(id).filter(|id| !id.is_empty())?.to_string(),
            title: text(&p["title"])?,
            program: text(&p["program"])?,
            args: argv(&p["args"]).unwrap_or_default(),
            config_env: text(&p["config"]["env"]),
            config_default: text(&p["config"]["default"])?,
            list: argv(&plugin["list"])?,
            add_marketplace: argv(&plugin["add_marketplace"]),
            install_plugin: argv(&plugin["install"]),
            update_marketplace: argv(&plugin["update_marketplace"]),
            update_plugin: argv(&plugin["update"]),
            listing: serde_json::from_value(plugin["listing"].clone()).ok(),
            resume: argv(&p["resume"])?,
            transcript: text(&p["transcript"]),
            turbo: argv(&p["turbo"]).unwrap_or_default(),
            beta: p["beta"].as_bool().unwrap_or(false),
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
        let mut words = vec![self.spec()];
        words.extend(self.resume.iter().cloned());
        words.push(session.to_string());
        words.join(" ")
    }

    /// The command line that starts it fresh: the program and its `args`.
    pub fn spec(&self) -> String {
        std::iter::once(&self.program).chain(&self.args).cloned().collect::<Vec<_>>().join(" ")
    }

    /// The words after the program for one of its commands: its `args`, then
    /// the command's own.
    pub fn argv(&self, command: &[String]) -> Vec<String> {
        self.args.iter().chain(command).cloned().collect()
    }
}

/// The words a command line runs as, with turbo mode's flags after the
/// person's own: each flag once, and a flag of more than one word only when
/// it is not already there whole.
pub fn with_turbo(spec: &str, flags: &[String]) -> Vec<String> {
    let mut words: Vec<String> = spec.split_whitespace().map(str::to_string).collect();
    for flag in flags {
        let flag: Vec<String> = flag.split_whitespace().map(str::to_string).collect();
        if !flag.is_empty() && !words.windows(flag.len()).any(|w| w == flag.as_slice()) {
            words.extend(flag);
        }
    }
    words
}

/// Every harness Weft has a file for, in the order of their names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Harnesses(pub Vec<Harness>);

impl Harnesses {
    /// Every harness file, by id and text. One that does not parse, or is
    /// missing a field Weft needs, defines no harness and is named in what
    /// comes back second, so that it can be said once.
    pub fn read(files: Vec<(String, String)>) -> (Self, Vec<String>) {
        let (mut read, mut unread) = (Vec::new(), Vec::new());
        for (id, text) in files {
            let table = text.parse::<toml::Table>().ok().and_then(|t| serde_json::to_value(t).ok());
            match table.and_then(|t| Harness::of(&id, &t)) {
                Some(h) => read.push(h),
                None => unread.push(id),
            }
        }
        // By the name a person reads, A to Z: every list of harnesses Weft
        // shows is in this order.
        read.sort_by_key(|h| h.title.to_lowercase());
        (Harnesses(read), unread)
    }

    /// The harness RingFrame records under this name, if Weft has its file.
    pub fn find(&self, name: &str) -> Option<&Harness> {
        self.0.iter().find(|h| h.name == name)
    }

    /// The harness a command line starts, by its program name. `weft .
    /// "<program> --its --own --args"` names a harness the way a person would.
    pub fn for_program(&self, program: &str) -> Option<&Harness> {
        let file = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
        self.0.iter().find(|h| file(&h.program) == file(program))
    }

    /// Each harness's program where the person said it is, for those they
    /// named: everything that runs it, runs it from there.
    pub fn with_programs(mut self, program: impl Fn(&str) -> Option<String>) -> Self {
        for h in &mut self.0 {
            if let Some(p) = program(&h.name) {
                h.program = p;
            }
        }
        self
    }

    pub fn iter(&self) -> impl Iterator<Item = &Harness> {
        self.0.iter()
    }

    /// The names a person reads, for a sentence: "A or B".
    pub fn titles(&self) -> String {
        let t: Vec<&str> = self.0.iter().map(|h| h.title.as_str()).collect();
        match t.as_slice() {
            [] => "a harness Weft has a harness file for".into(),
            [one] => (*one).into(),
            [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
        }
    }
}

/// Test fixtures: Weft's own fixture harness files, read the way the daemon
/// reads `~/.fab7/weft/harnesses/`, so every harness test runs on file-loaded
/// data. Built only for tests, and for the crates above that ask for
/// `fixtures`.
#[cfg(any(test, feature = "fixtures"))]
pub mod fixture {
    use super::*;

    /// The text of a fixture harness file: `claude-code`, `codex` or `antigravity`.
    pub fn text(name: &str) -> &'static str {
        match name {
            "claude-code" => include_str!("../tests/fixtures/harnesses/claude-code.toml"),
            "codex" => include_str!("../tests/fixtures/harnesses/codex.toml"),
            "antigravity" => include_str!("../tests/fixtures/harnesses/antigravity.toml"),
            other => panic!("no fixture harness file {other}"),
        }
    }

    /// One fixture harness file, as a table to change and read with `of`.
    pub fn file(name: &str) -> Value {
        serde_json::to_value(text(name).parse::<toml::Table>().expect("toml")).expect("json")
    }

    /// Claude Code and Codex, as the daemon would read them.
    pub fn harnesses() -> Harnesses {
        let files = ["claude-code", "codex"].map(|n| (n.to_string(), text(n).to_string()));
        Harnesses::read(files.to_vec()).0
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{file, harnesses, text};
    use super::*;

    /// A harness reached through another command: its `args` follow the
    /// program in every command line Weft builds, and the program alone is
    /// what is looked for and what a person's path replaces.
    #[test]
    fn args_follow_the_program_everywhere_it_runs() {
        let mut p = file("codex");
        p["program"] = serde_json::json!("launcher");
        p["args"] = serde_json::json!(["agent"]);
        let h = Harness::of("via", &p).expect("a harness");
        assert_eq!(h.spec(), "launcher agent");
        assert_eq!(h.resume_spec("s1"), "launcher agent resume s1");
        assert_eq!(h.argv(&h.list), ["agent", "plugin", "list", "--json"]);
        let all = Harnesses(vec![h]).with_programs(|_| Some("/opt/launcher".into()));
        let moved = all.find("via").unwrap();
        assert_eq!(
            (moved.program.as_str(), moved.spec().as_str()),
            ("/opt/launcher", "/opt/launcher agent")
        );
        assert!(all.for_program("launcher").is_some(), "named by its program, not its args");

        let plain = Harness::of("codex", &file("codex")).unwrap();
        assert!(plain.args.is_empty(), "absent means none");
        assert_eq!(
            (plain.spec(), plain.argv(&["x".into()])),
            ("codex".into(), vec!["x".to_string()])
        );
    }

    #[test]
    fn the_harnesses_are_the_ones_the_profiles_define() {
        let all = harnesses();
        let names: Vec<_> = all.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["claude-code", "codex"]);
        assert_eq!(all.find("claude-code").map(|h| h.program.as_str()), Some("claude"));
        assert_eq!(all.find("codex").map(|h| h.title.as_str()), Some("Codex"));
        assert!(all.find("aider").is_none(), "nothing without a file");
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
        let mut p = file("codex");
        p["turbo"] = Value::Null;
        assert!(
            Harness::of("codex", &p).expect("a harness").turbo.is_empty(),
            "no turbo, no flags"
        );
    }

    /// A harness keeps its configuration where it keeps it, whether or not a
    /// variable moves it, and may have no marketplace to add: a profile says
    /// only what is true of it, and Weft asks nothing it has no command for.
    #[test]
    fn a_harness_may_have_no_configuration_variable_and_no_marketplace_commands() {
        let mut p = file("codex");
        p["config"] = serde_json::json!({"default": ".elsewhere/agent"});
        p["plugin"]["add_marketplace"] = Value::Null;
        p["plugin"]["update_marketplace"] = Value::Null;
        let h = Harness::of("codex", &p).expect("still a harness");
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

    /// Turbo mode's flags go after the person's own, each once, and a flag
    /// of more than one word goes whole, and only when it is not there whole.
    #[test]
    fn turbo_adds_each_flag_once_after_the_command_line_as_typed() {
        let flags = |f: &[&str]| f.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            with_turbo("codex --model x", &flags(&["--yolo"])),
            ["codex", "--model", "x", "--yolo"]
        );
        assert_eq!(with_turbo("codex --yolo", &flags(&["--yolo"])), ["codex", "--yolo"], "once");
        let two = flags(&["-c approval_policy=never"]);
        assert_eq!(
            with_turbo("codex -c model=x", &two),
            ["codex", "-c", "model=x", "-c", "approval_policy=never"],
            "a word it shares with another flag is not the flag"
        );
        assert_eq!(
            with_turbo("codex -c approval_policy=never", &two),
            ["codex", "-c", "approval_policy=never"]
        );
        assert_eq!(with_turbo("claude", &[]), ["claude"]);
    }

    /// A made-up harness, from its file alone, is read and offered.
    #[test]
    fn a_made_up_harness_is_read_from_its_file_alone() {
        let zed = text("codex")
            .replace("program = \"codex\"", "program = \"zed-agent\"")
            .replace("title = \"Codex\"", "title = \"Zed Agent\"")
            .replace("resume = [\"resume\"]", "resume = [\"--thread\"]");
        let files = vec![
            ("claude-code".to_string(), text("claude-code").to_string()),
            ("zed-agent".to_string(), zed),
        ];
        let (all, unread) = Harnesses::read(files);
        assert!(unread.is_empty());
        let h = all.for_program("zed-agent").expect("offered");
        assert_eq!(h.name, "zed-agent", "named by its file");
        assert_eq!(h.resume_spec("t1"), "zed-agent --thread t1");
        assert_eq!(all.titles(), "Claude Code or Zed Agent");
    }

    /// A harness file that is missing, broken or short of a field leaves the
    /// others offered, and is named so it can be said.
    #[test]
    fn a_broken_harness_file_leaves_the_others_offered() {
        let files = vec![
            ("broken".to_string(), "title = [not toml".to_string()),
            ("claude-code".to_string(), text("claude-code").to_string()),
            ("half".to_string(), "title = \"Half\"\nprogram = \"half\"\n".to_string()),
        ];
        let (all, unread) = Harnesses::read(files);
        assert_eq!(all.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["claude-code"]);
        assert_eq!(unread, ["broken", "half"]);
        let mut broken = file("codex");
        broken["plugin"] = Value::Null;
        assert_eq!(Harness::of("codex", &broken), None, "a file missing a field defines none");
        assert!(
            Harness::of("antigravity", &file("antigravity")).is_some(),
            "no marketplace, no install: still one"
        );
    }
}
