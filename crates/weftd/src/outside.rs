//! Everything the daemon asks of the machine it runs on: the programs it runs
//! (a harness's plugin listing, the `ringframe` CLI, a setup step), whether a
//! harness's program is here, what the latest release is, and whether a
//! program the person pointed at starts.
//!
//! One seam, so that Weft's own behaviour can be tested without the machine's:
//! the daemon is given an `Outside`, which is [`Machine`] when Weft runs and a
//! [`Fixed`] world in a test, where each test says which programs exist and
//! what they answer. What a real harness does is not a unit test's question.

use std::collections::BTreeMap;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Output};
use std::sync::Arc;

use crate::harness::runnable;

pub trait Outside: Send + Sync {
    /// Run a program to its end and hand back what it said.
    fn run(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> std::io::Result<Output>;
    /// Whether a harness's program is on this machine: on `PATH`, or where
    /// the person said it is (an absolute path).
    fn on_path(&self, program: &str) -> bool;
    /// What `ringframe sync --check` says of the latest release, or why it
    /// could not tell; `fresh` asks again rather than reading a kept answer.
    fn release(&self, fresh: bool) -> Result<serde_json::Value, String>;
    /// The program the person pointed at, if it is one that starts.
    fn check_program(&self, path: &str) -> Result<String, String>;
}

pub type Shared = Arc<dyn Outside>;

/// The machine Weft runs on.
pub struct Machine;

pub fn machine() -> Shared {
    Arc::new(Machine)
}

impl Outside for Machine {
    fn run(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> std::io::Result<Output> {
        let mut command = Command::new(program);
        command.args(args);
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        command.output()
    }

    fn on_path(&self, program: &str) -> bool {
        if program.contains('/') {
            return runnable(Path::new(program));
        }
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
        })
    }

    fn release(&self, fresh: bool) -> Result<serde_json::Value, String> {
        crate::onboarding::check(fresh)
    }

    fn check_program(&self, path: &str) -> Result<String, String> {
        crate::onboarding::check_program(path)
    }
}

/// A world a test describes: the programs in it and what each answers.
/// Nothing else exists, so nothing from the machine running the test reaches
/// the daemon.
#[derive(Clone)]
pub struct Fixed {
    programs: BTreeMap<String, Vec<(Vec<String>, i32, String)>>,
    release: Result<serde_json::Value, String>,
}

impl Default for Fixed {
    fn default() -> Self {
        Fixed { programs: BTreeMap::new(), release: Err("no release check in this world".into()) }
    }
}

impl Fixed {
    /// A world with no programs at all: no harness, no RingFrame.
    pub fn new() -> Self {
        Self::default()
    }

    /// `program` exists, and answers a call whose arguments hold `words`, in
    /// order, with `status` and `stdout`. A call it has no answer for fails.
    /// The first matching answer wins, so the more specific goes first.
    pub fn answers(mut self, program: &str, words: &[&str], status: i32, stdout: &str) -> Self {
        self.programs.entry(program.to_string()).or_default().push((
            words.iter().map(|w| w.to_string()).collect(),
            status,
            stdout.to_string(),
        ));
        self
    }

    /// `program` exists and every call to it fails: installed, and nothing more.
    pub fn exists(mut self, program: &str) -> Self {
        self.programs.entry(program.to_string()).or_default();
        self
    }

    pub fn with_release(mut self, release: Result<serde_json::Value, String>) -> Self {
        self.release = release;
        self
    }

    pub fn shared(self) -> Shared {
        Arc::new(self)
    }
}

/// Whether `words` appear in `args`, in order.
fn holds(args: &[&str], words: &[String]) -> bool {
    let mut at = args.iter();
    words.iter().all(|w| at.any(|a| a == w))
}

impl Outside for Fixed {
    fn run(&self, program: &str, args: &[&str], _cwd: Option<&Path>) -> std::io::Result<Output> {
        let Some(answers) = self.programs.get(program) else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, program.to_string()));
        };
        let (status, stdout) = answers
            .iter()
            .find(|(words, _, _)| holds(args, words))
            .map_or((1, String::new()), |(_, s, o)| (*s, o.clone()));
        Ok(Output {
            status: ExitStatus::from_raw(status << 8),
            stdout: stdout.into_bytes(),
            stderr: Vec::new(),
        })
    }

    fn on_path(&self, program: &str) -> bool {
        self.programs.contains_key(program)
    }

    fn release(&self, _fresh: bool) -> Result<serde_json::Value, String> {
        self.release.clone()
    }

    fn check_program(&self, path: &str) -> Result<String, String> {
        if self.programs.contains_key(path) {
            Ok(path.to_string())
        } else {
            Err(format!("{path} is not a program that can run"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_world_has_only_the_programs_it_names() {
        let w = Fixed::new()
            .answers(
                "ringframe",
                &["profile", "show", "codex"],
                0,
                "{\"invocation_prefix\":\"$rf:\"}",
            )
            .exists("codex");
        assert!(w.on_path("codex") && w.on_path("ringframe") && !w.on_path("claude"));
        let said =
            w.run("ringframe", &["--workspace", "/p", "profile", "show", "--host", "codex"], None);
        let said = said.unwrap();
        assert!(said.status.success());
        assert_eq!(said.stdout, b"{\"invocation_prefix\":\"$rf:\"}");
        assert!(!w.run("ringframe", &["ask", "preflight"], None).unwrap().status.success());
        assert!(w.run("claude", &["--version"], None).is_err(), "no such program");
        assert!(!w.run("codex", &["plugin", "list"], None).unwrap().status.success());
    }
}
