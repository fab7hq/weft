//! The harnesses Weft supports, and how to find and prepare one.
//!
//! Spec: `plans/weft/spec/readiness.md`. One declared table; nothing outside
//! it is touched. Weft asks a harness through its own documented commands and
//! never reads its configuration files: those are not a surface anyone
//! promised to keep, and a reader that guesses at them is wrong the day they
//! change.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Harness {
    /// The name RingFrame records, which is what everything else keys on.
    pub name: &'static str,
    /// The executable, as it is found on `PATH`.
    pub program: &'static str,
    /// The variable that moves this harness's configuration home.
    pub config_env: &'static str,
    /// Where it lives when that variable is unset, under the person's home.
    pub config_default: &'static str,
    /// Asks the harness what plugins it has, as JSON.
    pub list: &'static [&'static str],
    /// Adds the marketplace the `rf` plugin is published in.
    pub add_marketplace: &'static [&'static str],
    /// Installs the plugin itself.
    pub install_plugin: &'static [&'static str],
}

/// The marketplace and plugin RingFrame publishes. Named here once.
pub const MARKETPLACE: &str = "fab7";
pub const PLUGIN: &str = "rf@fab7";

pub const SUPPORTED: &[Harness] = &[
    Harness {
        name: "claude-code",
        program: "claude",
        config_env: "CLAUDE_CONFIG_DIR",
        config_default: ".claude",
        list: &["plugin", "list", "--available", "--json"],
        add_marketplace: &["plugin", "marketplace", "add", "fab7hq/fab7"],
        install_plugin: &["plugin", "install", "rf@fab7", "--scope", "user"],
    },
    Harness {
        name: "codex",
        program: "codex",
        config_env: "CODEX_HOME",
        config_default: ".codex",
        list: &["plugin", "list", "--json"],
        add_marketplace: &["plugin", "marketplace", "add", "fab7hq/fab7"],
        install_plugin: &["plugin", "add", "rf@fab7"],
    },
];

/// The harness RingFrame would record under this name, if Weft supports it.
pub fn find(name: &str) -> Option<&'static Harness> {
    SUPPORTED.iter().find(|h| h.name == name)
}

/// The harness a command line starts, by its program name. `weft . "claude
/// --model sonnet"` names a harness the same way a person would.
pub fn for_program(program: &str) -> Option<&'static Harness> {
    let file = program.rsplit('/').next().unwrap_or(program);
    SUPPORTED.iter().find(|h| h.program == file)
}

impl Harness {
    /// Where this harness keeps its configuration, honouring its own variable.
    pub fn config_home(&self) -> PathBuf {
        self.config_home_from(std::env::var_os(self.config_env), home())
    }

    /// The rule itself, separated from the environment so it can be tested
    /// without setting a variable every other test in the binary can see.
    fn config_home_from(&self, set: Option<std::ffi::OsString>, home: PathBuf) -> PathBuf {
        match set {
            Some(set) if !set.is_empty() => PathBuf::from(set),
            _ => home.join(self.config_default),
        }
    }

    pub fn on_path(&self) -> bool {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(self.program).is_file())
        })
    }

    /// The two commands, exactly as they will be run and as they are shown.
    /// One list, so what is displayed cannot drift from what happens.
    pub fn setup_commands(&self) -> Vec<String> {
        [self.add_marketplace, self.install_plugin]
            .iter()
            .map(|args| format!("{} {}", self.program, args.join(" ")))
            .collect()
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_supported_harnesses_are_the_ones_ringframe_names() {
        let names: Vec<_> = SUPPORTED.iter().map(|h| h.name).collect();
        assert_eq!(names, vec!["claude-code", "codex"]);
        assert_eq!(find("claude-code").map(|h| h.program), Some("claude"));
        assert_eq!(find("codex").map(|h| h.program), Some("codex"));
        assert!(find("aider").is_none(), "nothing outside the table");
    }

    #[test]
    fn a_command_line_names_its_harness_the_way_a_person_would() {
        assert_eq!(for_program("claude").map(|h| h.name), Some("claude-code"));
        assert_eq!(for_program("/opt/homebrew/bin/codex").map(|h| h.name), Some("codex"));
        assert!(for_program("bash").is_none());
    }

    #[test]
    fn a_configuration_home_follows_the_harnesses_own_variable() {
        let home = PathBuf::from("/home/someone");
        for (name, var, set, unset) in [
            ("claude-code", "CLAUDE_CONFIG_DIR", "/elsewhere/claude", "/home/someone/.claude"),
            ("codex", "CODEX_HOME", "/elsewhere/codex", "/home/someone/.codex"),
        ] {
            let h = find(name).expect(name);
            assert_eq!(h.config_env, var);
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
    fn the_commands_shown_are_the_commands_run() {
        let codex = find("codex").expect("codex");
        assert_eq!(
            codex.setup_commands(),
            vec![
                "codex plugin marketplace add fab7hq/fab7".to_string(),
                "codex plugin add rf@fab7".to_string()
            ]
        );
        let claude = find("claude-code").expect("claude");
        assert_eq!(
            claude.setup_commands(),
            vec![
                "claude plugin marketplace add fab7hq/fab7".to_string(),
                "claude plugin install rf@fab7 --scope user".to_string()
            ]
        );
    }

    #[test]
    fn every_harness_is_asked_for_json_so_nothing_parses_a_table() {
        for h in SUPPORTED {
            assert!(h.list.contains(&"--json"), "{} must answer in JSON", h.name);
            assert_eq!(h.list[0], "plugin");
        }
    }
}
