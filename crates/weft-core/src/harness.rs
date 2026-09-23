//! The harnesses Weft supports, and how to find and prepare one.
//!
//! One declared table; nothing outside it is touched. Weft asks a harness
//! through its own documented commands and
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
    /// Refreshes this harness's copy of the marketplace.
    pub update_marketplace: &'static [&'static str],
    /// Moves the installed plugin to the marketplace's version.
    pub update_plugin: &'static [&'static str],
    /// How this harness is told to pick a recorded session back up. The id is
    /// appended; `claude --resume <id>`, `codex resume <id>`.
    pub resume: &'static [&'static str],
    /// What to press to see what scrolled away, when the harness keeps its own
    /// transcript instead of letting the terminal keep it. `None` means it
    /// prints inline and Weft's own scrollback is the whole of it.
    pub transcript: Option<&'static str>,
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
        update_marketplace: &["plugin", "marketplace", "update", "fab7"],
        update_plugin: &["plugin", "update", "rf@fab7"],
        resume: &["--resume"],
        // Claude Code prints its transcript inline and lets the terminal keep
        // it, so what Weft captured is what there is to scroll.
        transcript: None,
    },
    Harness {
        name: "codex",
        program: "codex",
        config_env: "CODEX_HOME",
        config_default: ".codex",
        list: &["plugin", "list", "--json"],
        add_marketplace: &["plugin", "marketplace", "add", "fab7hq/fab7"],
        install_plugin: &["plugin", "add", "rf@fab7"],
        update_marketplace: &["plugin", "marketplace", "upgrade", "fab7"],
        // Adding again installs the version the refreshed marketplace holds.
        update_plugin: &["plugin", "add", "rf@fab7"],
        resume: &["resume"],
        // Codex repaints its viewport rather than scrolling, so nothing ever
        // reaches Weft's scrollback. Its own transcript is behind Ctrl+T.
        transcript: Some("Ctrl+T"),
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
    /// The rule itself, separated from the environment so it can be tested
    /// without setting a variable every other test in the binary can see.
    /// Public because the environment half now lives in another crate.
    pub fn config_home_from(&self, set: Option<std::ffi::OsString>, home: PathBuf) -> PathBuf {
        match set {
            Some(set) if !set.is_empty() => PathBuf::from(set),
            _ => home.join(self.config_default),
        }
    }

    /// The command line that opens a recorded session again, exactly as a
    /// person would type it.
    pub fn resume_spec(&self, session: &str) -> String {
        format!("{} {} {session}", self.program, self.resume.join(" "))
    }

    /// The command line that starts a fresh one.
    pub fn spec(&self) -> String {
        self.program.to_string()
    }
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
    fn a_recorded_session_is_reopened_the_way_each_harness_spells_it() {
        assert_eq!(
            find("claude-code")
                .expect("claude")
                .resume_spec("883b9d12-5745-4ffa-9aac-eaaeb7e0dd47"),
            "claude --resume 883b9d12-5745-4ffa-9aac-eaaeb7e0dd47"
        );
        assert_eq!(
            find("codex").expect("codex").resume_spec("01a0bdb6-1d1f-79c2-84b0-8b03496d7db0"),
            "codex resume 01a0bdb6-1d1f-79c2-84b0-8b03496d7db0"
        );
    }

    #[test]
    fn only_a_harness_that_keeps_its_own_transcript_names_a_key_for_it() {
        // Measured, not assumed: Codex repaints its viewport, so Weft's
        // scrollback stays empty and its transcript is behind Ctrl+T. Claude
        // Code prints inline, so there is nothing extra to point at.
        assert_eq!(find("codex").expect("codex").transcript, Some("Ctrl+T"));
        assert_eq!(find("claude-code").expect("claude").transcript, None);
    }

    #[test]
    fn every_harness_is_asked_for_json_so_nothing_parses_a_table() {
        for h in SUPPORTED {
            assert!(h.list.contains(&"--json"), "{} must answer in JSON", h.name);
            assert_eq!(h.list[0], "plugin");
        }
    }
}
