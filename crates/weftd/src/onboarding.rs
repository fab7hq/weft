//! Setting up a harness from Weft: what only the machine can answer. What a
//! row means lives in [`weft_core::onboarding`].

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use weft_core::onboarding::{Next, Row, State, View, checking, row, steps};
use weft_core::sync::{Mark, Step};

use crate::harness::{Harness, Harnesses};
use crate::outside::Outside;

/// How long a program has to answer `--version` before it counts as not
/// starting.
const STARTS_WITHIN: Duration = Duration::from_secs(15);

/// How long an answer about the latest release is kept. The daemon asks when
/// it starts and again this often (version checking
/// should not be frequent); the view's looks, `[U]`, `Enter` and `P` read the
/// kept answer, and only a configuration sync asks again at once.
pub const CHECK_EVERY: Duration = Duration::from_secs(12 * 60 * 60);

static CHECKED: std::sync::Mutex<Option<(Instant, Result<serde_json::Value, String>)>> =
    std::sync::Mutex::new(None);

/// What `ringframe sync --check` says of the latest release, or why it could
/// not tell, kept for [`CHECK_EVERY`]; `fresh` asks again, as the daemon's own
/// job and a configuration sync do.
pub fn check(fresh: bool) -> Result<serde_json::Value, String> {
    let mut kept = CHECKED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, answer)) = kept.as_ref()
        && !fresh
        && at.elapsed() < CHECK_EVERY
    {
        return answer.clone();
    }
    let answer = ask_check();
    *kept = Some((Instant::now(), answer.clone()));
    answer
}

fn ask_check() -> Result<serde_json::Value, String> {
    let out = Command::new("ringframe")
        .args(["sync", "--check"])
        .output()
        .map_err(|e| format!("ringframe would not run: {e}"))?;
    let said: Option<serde_json::Value> = serde_json::from_slice(&out.stdout).ok();
    match said {
        Some(v) if out.status.success() => Ok(v),
        // RingFrame says why, in its own words: the reason is the person's to read.
        Some(v) => Err(v["detail"].as_str().unwrap_or("it did not say why").to_string()),
        None => {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(err.lines().last().unwrap_or("it did not say why").trim().to_string())
        }
    }
}

/// The view: RingFrame's configuration against the latest release (`check`),
/// and every harness file's row, found or not, current or behind; and, for
/// each harness found, its readiness, to tell the clients.
pub fn look(
    out: &dyn Outside,
    harnesses: &Harnesses,
    check: &Result<serde_json::Value, String>,
    cli: bool,
) -> (View, Vec<(String, weft_core::readiness::Readiness)>) {
    look_as_they_answer(out, harnesses, check, cli, |_| {})
}

/// The view before anything has answered: every harness found on this machine
/// being asked, the others not found, and the latest release still pending.
/// Finding a program is a look at `PATH`, so this costs no process.
pub fn pending(out: &dyn Outside, harnesses: &Harnesses) -> View {
    let rows = harnesses
        .iter()
        .map(|h| if out.on_path(&h.program) { checking(h) } else { row(h, None, None) })
        .collect();
    View { pending_release: true, rows, ..View::default() }
}

/// [`look`], asking every harness found at once rather than one after another,
/// and handing `shown` the view again as each one answers: a slow harness keeps
/// only its own row waiting.
pub fn look_as_they_answer(
    out: &dyn Outside,
    harnesses: &Harnesses,
    check: &Result<serde_json::Value, String>,
    cli: bool,
    mut shown: impl FnMut(&View),
) -> (View, Vec<(String, weft_core::readiness::Readiness)>) {
    let known = check.as_ref().ok();
    let text = |k: &str| known.and_then(|c| c[k].as_str()).map(str::to_string);
    let (latest, plugin) = (text("latest"), text("plugin"));
    let (mut configuration, step) = weft_core::sync::configuration(known);
    if let Err(why) = check {
        configuration = format!("{configuration}: {why}");
    }
    let found: Vec<bool> = harnesses.iter().map(|h| out.on_path(&h.program)).collect();
    let mut view = View {
        latest,
        configuration,
        configuration_behind: step.is_some(),
        rows: harnesses
            .iter()
            .zip(&found)
            .map(|(h, &on)| if on { checking(h) } else { row(h, None, plugin.as_deref()) })
            .collect(),
        plugin,
        ..View::default()
    };
    let mut states = Vec::new();
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        for (at, h) in harnesses.iter().enumerate().filter(|(at, _)| found[*at]) {
            let tx = tx.clone();
            scope.spawn(move || {
                let _ = tx.send((at, crate::readiness::look(out, h, cli)));
            });
        }
        drop(tx);
        for (at, answer) in rx {
            let h = &harnesses.0[at];
            states.push((h.name.clone(), answer.0));
            view.rows[at] = row(h, Some(answer), view.plugin.as_deref());
            shown(&view);
        }
    });
    // By the harnesses' order, as the rows are, whatever order they answered in.
    states.sort_by_key(|(name, _)| harnesses.iter().position(|h| &h.name == name));
    (view, states)
}

/// The program the person pointed at, if it is one that starts: an absolute
/// path (or `~/…`) to an executable file that answers `--version`.
pub fn check_program(path: &str) -> Result<String, String> {
    let path = match path.trim().strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|h| Path::new(&h).join(rest).to_string_lossy().to_string())
            .unwrap_or_default(),
        None => path.trim().to_string(),
    };
    if !Path::new(&path).is_absolute() {
        return Err("give the program's whole path, starting with / or ~/".into());
    }
    if !crate::harness::runnable(Path::new(&path)) {
        return Err(format!("{path} is not a program that can run"));
    }
    let mut child = Command::new(&path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{path} would not start: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(s)) if s.success() => return Ok(path),
            Ok(Some(_)) => return Err(format!("{path} --version did not succeed")),
            Ok(None) if started.elapsed() > STARTS_WITHIN => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{path} --version did not answer"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("{path}: {e}")),
        }
    }
}

/// Set one key of `[harnesses.<name>]` in `config.toml`, keeping the rest of
/// the file as the person wrote it.
pub fn save(config: &Path, name: &str, key: &str, value: serde_json::Value) -> Result<(), String> {
    let text = std::fs::read_to_string(config).unwrap_or_default();
    let out = weft_core::config::set_harness(&text, name, key, &value)?;
    if let Some(dir) = config.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(config, out).map_err(|e| format!("config.toml could not be written: {e}"))
}

/// Run this harness's setup commands in order, showing its row at every
/// step, and stop at the first that fails. The row that comes back is the
/// harness's own answer afterwards, or the failed step.
pub fn run(
    out: &dyn Outside,
    h: &Harness,
    cli: bool,
    latest: Option<&str>,
    show: impl FnMut(State),
) -> Option<State> {
    let (state, have) = crate::readiness::look(out, h, cli);
    proceed(out, steps(h, state, have.as_deref(), latest), show)
}

/// Run these steps in order, showing each as it starts, and stop at the first
/// that fails; that one comes back.
pub fn proceed(out: &dyn Outside, steps: Vec<Step>, mut show: impl FnMut(State)) -> Option<State> {
    let mut steps = steps;
    crate::sync::proceed(out, &mut steps, |steps| {
        if let Some(s) = steps.iter().find(|s| s.mark == Mark::Running) {
            show(State::Running(s.line()));
        }
    });
    steps
        .iter()
        .find(|s| s.mark == Mark::Failed)
        .map(|s| State::Failed { step: s.line(), said: s.said.clone() })
}

/// Whether `[P]ROCEED` runs this row: set up or updated, never located.
pub fn runs(r: &Row) -> bool {
    matches!(r.state.next(), Some(Next::SetUp | Next::Update))
}

/// A view with one row's state set.
pub fn with(mut view: View, name: &str, state: State) -> View {
    if let Some(r) = view.rows.iter_mut().find(|r: &&mut Row| r.name == name) {
        r.state = state;
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outside::Machine;

    #[test]
    fn a_program_the_person_names_must_be_a_whole_path_that_starts() {
        assert!(check_program("claude").unwrap_err().contains("whole path"));
        assert!(check_program("/definitely/not/here").unwrap_err().contains("not a program"));
        assert_eq!(check_program("/bin/echo").as_deref(), Ok("/bin/echo"));
        assert!(check_program("/usr/bin/false").unwrap_err().contains("did not succeed"));
    }

    /// Two harnesses are asked at once: each says it has `rf` only once it has
    /// seen the other one being asked too, so asked one after another, the
    /// first would give up and read as not set up. Each answer is shown as it
    /// comes, and the view opens before either has answered.
    #[test]
    fn the_harnesses_are_asked_at_once_and_shown_as_they_answer() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("weft-setup-at-once-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut files = Vec::new();
        for (name, other) in [("slowa", "slowb"), ("slowb", "slowa")] {
            let program = dir.join(name);
            let (mine, theirs) =
                (dir.join(format!("{name}.asked")), dir.join(format!("{other}.asked")));
            std::fs::write(
                &program,
                format!(
                    "#!/bin/sh\ntouch {mine}\ni=0\nwhile [ ! -f {theirs} ] && [ $i -lt 100 ]; do sleep 0.05; i=$((i+1)); done\n\
                     if [ -f {theirs} ]; then echo '{{\"installed\":[{{\"name\":\"rf\"}}]}}'; else echo '{{\"installed\":[]}}'; fi\n",
                    mine = mine.display(),
                    theirs = theirs.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            files.push((
                name.to_string(),
                format!(
                    "title = \"{name}\"\nprogram = \"{}\"\nresume = []\n\n[config]\ndefault = \".x\"\n\n\
                     [plugin]\nlist = [\"list\"]\n\n[plugin.listing]\ninstalled = [\"installed\"]\n\
                     available = []\nname = \"name\"\n",
                    program.display()
                ),
            ));
        }
        let (harnesses, unread) = Harnesses::read(files);
        assert!(unread.is_empty(), "{unread:?}");
        let opened = pending(&Machine, &harnesses);
        assert!(opened.pending_release);
        assert!(opened.rows.iter().all(|r| r.state == State::Checking), "{:?}", opened.rows);

        let mut seen = Vec::new();
        let (view, states) =
            look_as_they_answer(&Machine, &harnesses, &Ok(serde_json::json!({})), true, |v| {
                seen.push(v.rows.iter().filter(|r| r.state == State::Checking).count());
            });
        assert_eq!(seen, [1, 0], "each answer shown as it came, the other still being asked");
        assert!(!view.pending_release);
        assert!(
            view.rows.iter().all(|r| matches!(r.state, State::Ready { .. })),
            "asked one after another, the first gave up: {:?}",
            view.rows
        );
        assert_eq!(states.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A harness whose program is a script: it lists no `rf` until its
    /// install has run.
    fn stub(dir: &Path) -> Harness {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let program = dir.join("stubh");
        let marker = dir.join("installed");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n  list) if [ -f {m} ]; then echo '{{\"installed\":[{{\"name\":\"rf\"}}]}}'; else echo '{{\"installed\":[]}}'; fi ;;\n  install) touch {m} ;;\n  --version) echo 1 ;;\nesac\n",
                m = marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A harness file, as fab7 ships one, read the way the daemon reads one.
        let file = format!(
            "title = \"Stub\"\nprogram = \"{}\"\nresume = []\n\n[config]\ndefault = \".stub\"\n\n\
             [plugin]\nlist = [\"list\"]\ninstall = [\"install\"]\n\n\
             [plugin.listing]\ninstalled = [\"installed\"]\navailable = []\nname = \"name\"\n",
            program.display()
        );
        let (read, unread) = Harnesses::read(vec![("stub".into(), file)]);
        assert!(unread.is_empty(), "{unread:?}");
        read.find("stub").cloned().expect("the stub harness file reads")
    }

    #[test]
    fn setting_a_harness_up_runs_its_commands_and_the_tick_is_its_answer_afterwards() {
        let dir = std::env::temp_dir().join(format!("weft-setup-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let h = stub(&dir);
        let harnesses = Harnesses(vec![h.clone()]);
        let (v, _) = look(&Machine, &harnesses, &Err("offline".into()), true);
        assert_eq!(v.rows[0].state, State::NotSetUp);
        let mut seen = Vec::new();
        assert_eq!(run(&Machine, &h, true, None, |s| seen.push(s)), None, "no step failed");
        assert_eq!(seen, [State::Running(format!("{} install", h.program))]);
        let (v, states) = look(&Machine, &harnesses, &Err("offline".into()), true);
        assert_eq!(v.rows[0].state, State::Ready { version: None });
        assert_eq!(states, [("stub".to_string(), weft_core::readiness::Readiness::Ready)]);
        let (v, _) = look(&Machine, &harnesses, &Err("offline".into()), false);
        assert_eq!(v.rows[0].state, State::NoCli, "RingFrame missing is Weft's installer's");
        let mut gone = h.clone();
        gone.program = dir.join("nothing-here").display().to_string();
        let (v, _) = look(&Machine, &Harnesses(vec![gone]), &Err("offline".into()), true);
        assert_eq!(v.rows[0].state, State::NotFound);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_choice_is_saved_into_config_toml_and_read_back() {
        let dir = std::env::temp_dir().join(format!("weft-setup-save-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        save(&path, "codex", "program", serde_json::json!("/opt/codex")).unwrap();
        let known = weft_core::harness::fixture::harnesses();
        let c = weft_core::config::read(&std::fs::read_to_string(&path).unwrap(), &known);
        assert_eq!(c.program("codex"), Some("/opt/codex"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
