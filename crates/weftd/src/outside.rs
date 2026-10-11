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
    /// Run a program with `input` on its stdin, to its end.
    fn run_input(&self, program: &str, args: &[&str], input: &[u8]) -> std::io::Result<Output>;
    /// Whether a harness's program is on this machine: on `PATH`, or where
    /// the person said it is (an absolute path).
    fn on_path(&self, program: &str) -> bool;
    /// What `ringframe sync --check` says of the latest release, or why it
    /// could not tell; `fresh` asks again rather than reading a kept answer.
    fn release(&self, fresh: bool) -> Result<serde_json::Value, String>;
    /// The latest Weft release, by version, or nothing when it cannot be
    /// reached; kept like `release`.
    fn weft_release(&self, fresh: bool) -> Option<String>;
    /// Whether Weft `version` is installed on this machine beside the one
    /// running.
    fn weft_installed(&self, version: &str) -> bool;
    /// The program the person pointed at, if it is one that starts.
    fn check_program(&self, path: &str) -> Result<String, String>;
    /// The text of the newest file at `pattern`, where a `*` folder stands
    /// for any one folder: a harness's installed `plugin.json`.
    fn newest(&self, pattern: &Path) -> Option<String>;
    /// Run `weft remote` until it ends (remote.md D10).
    fn remote(&self) -> std::io::Result<ExitStatus>;
    /// Weft's own service on this machine, "Start at login",
    /// where Weft has one.
    fn service(&self) -> Option<crate::service::Service>;
}

pub type Shared = Arc<dyn Outside>;

/// The machine Weft runs on.
pub struct Machine;

pub fn machine() -> Shared {
    let _ = started_as();
    Arc::new(Machine)
}

/// This program's real path, links followed, as it was the first time this
/// was asked: when the daemon started.
pub fn started_as() -> std::io::Result<std::path::PathBuf> {
    static AT: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    AT.get_or_init(|| std::env::current_exe().and_then(|e| e.canonicalize()).ok())
        .clone()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "this program's path"))
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

    fn run_input(&self, program: &str, args: &[&str], input: &[u8]) -> std::io::Result<Output> {
        use std::io::Write as _;
        use std::process::Stdio;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdin = child.stdin.take().expect("piped");
        let written = stdin.write_all(input);
        drop(stdin);
        let out = child.wait_with_output()?;
        written.map(|_| out)
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

    fn weft_release(&self, fresh: bool) -> Option<String> {
        crate::onboarding::weft_latest(fresh)
    }

    fn weft_installed(&self, version: &str) -> bool {
        std::env::var_os("HOME").is_some_and(|home| {
            Path::new(&home).join(".fab7/weft/versions").join(version).join("weft").is_file()
        })
    }

    fn check_program(&self, path: &str) -> Result<String, String> {
        crate::onboarding::check_program(path)
    }

    fn newest(&self, pattern: &Path) -> Option<String> {
        let mut found = vec![std::path::PathBuf::new()];
        for part in pattern.components() {
            let part = part.as_os_str();
            found = if part == "*" {
                found
                    .iter()
                    .filter_map(|dir| std::fs::read_dir(dir).ok())
                    .flatten()
                    .flatten()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .map(|e| e.path())
                    .collect()
            } else {
                found.into_iter().map(|dir| dir.join(part)).collect()
            };
        }
        let modified =
            |p: &std::path::PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let newest = found.into_iter().filter(|p| p.is_file()).max_by_key(modified)?;
        std::fs::read_to_string(newest).ok()
    }

    /// This same program, as `weft remote`: the daemon is `weft --serve`.
    /// Its own version's file, as it was when the daemon started, so a
    /// `weft` pointed at a newer version since is not it.
    fn remote(&self) -> std::io::Result<ExitStatus> {
        Command::new(started_as()?)
            .arg("remote")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
    }

    fn service(&self) -> Option<crate::service::Service> {
        crate::service::Service::here()
    }
}

/// A world a test describes: the programs in it and what each answers.
/// Nothing else exists, so nothing from the machine running the test reaches
/// the daemon.
#[derive(Clone)]
pub struct Fixed {
    programs: BTreeMap<String, Vec<(Vec<String>, i32, String)>>,
    release: Result<serde_json::Value, String>,
    /// The latest Weft release, and whether it is installed here already.
    weft: (Option<String>, bool),
    files: Vec<(std::path::PathBuf, String)>,
    /// When `weft remote` was started. Each run ends at once, failing.
    remote_runs: Arc<std::sync::Mutex<Vec<std::time::Instant>>>,
    /// Its service, when it has one: a scratch home's.
    service: Option<crate::service::Service>,
}

impl Default for Fixed {
    fn default() -> Self {
        Fixed {
            programs: BTreeMap::new(),
            release: Err("no release check in this world".into()),
            weft: (None, false),
            files: Vec::new(),
            remote_runs: Arc::default(),
            service: None,
        }
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

    /// The latest Weft release in this world, and whether it is installed.
    pub fn with_weft(mut self, latest: Option<&str>, installed: bool) -> Self {
        self.weft = (latest.map(str::to_string), installed);
        self
    }

    /// A file at `path`, under whatever home asks for it, holding `text`.
    /// The last one given that a pattern matches is its newest.
    pub fn file(mut self, path: &str, text: &str) -> Self {
        self.files.push((path.into(), text.to_string()));
        self
    }

    /// When `weft remote` was started in this world, as it runs.
    pub fn remote_runs(&self) -> Arc<std::sync::Mutex<Vec<std::time::Instant>>> {
        Arc::clone(&self.remote_runs)
    }

    /// This world's "Start at login" service.
    pub fn with_service(mut self, service: crate::service::Service) -> Self {
        self.service = Some(service);
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

    fn run_input(&self, program: &str, args: &[&str], _input: &[u8]) -> std::io::Result<Output> {
        self.run(program, args, None)
    }

    fn on_path(&self, program: &str) -> bool {
        self.programs.contains_key(program)
    }

    fn release(&self, _fresh: bool) -> Result<serde_json::Value, String> {
        self.release.clone()
    }

    fn weft_release(&self, _fresh: bool) -> Option<String> {
        self.weft.0.clone()
    }

    fn weft_installed(&self, _version: &str) -> bool {
        self.weft.1
    }

    fn check_program(&self, path: &str) -> Result<String, String> {
        if self.programs.contains_key(path) {
            Ok(path.to_string())
        } else {
            Err(format!("{path} is not a program that can run"))
        }
    }

    fn newest(&self, pattern: &Path) -> Option<String> {
        let asked: Vec<_> = pattern.components().map(|c| c.as_os_str().to_owned()).collect();
        self.files.iter().rev().find_map(|(path, text)| {
            let have: Vec<_> = path.components().map(|c| c.as_os_str().to_owned()).collect();
            let tail = asked.get(asked.len().checked_sub(have.len())?..)?;
            tail.iter().zip(&have).all(|(a, h)| a == "*" || a == h).then(|| text.clone())
        })
    }

    fn remote(&self) -> std::io::Result<ExitStatus> {
        self.remote_runs.lock().expect("runs").push(std::time::Instant::now());
        Ok(ExitStatus::from_raw(1 << 8))
    }

    fn service(&self) -> Option<crate::service::Service> {
        self.service.clone()
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

    /// On the machine, a `*` folder is any one folder, and the most recently
    /// written match is the one read.
    #[test]
    fn the_newest_file_a_pattern_matches_is_read() {
        let root = std::env::temp_dir().join(format!("weft-newest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, text) in [("aaa", "old"), ("bbb", "new")] {
            let at = root.join("rf").join(dir);
            std::fs::create_dir_all(&at).unwrap();
            std::fs::write(at.join("plugin.json"), text).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::fs::write(root.join("rf/stray.json"), "not a folder").unwrap();
        assert_eq!(Machine.newest(&root.join("rf/*/plugin.json")).as_deref(), Some("new"));
        assert_eq!(Machine.newest(&root.join("rf/aaa/plugin.json")).as_deref(), Some("old"));
        assert_eq!(Machine.newest(&root.join("rf/*/missing.json")), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
