//! The PATH the person's own shell gives. Started by an app opened from
//! Finder or the Dock, or by the login service, the daemon has only the
//! system's bare PATH, and would find none of the harnesses the person
//! installed.

use std::io::{IsTerminal, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const MARK: &str = "__weft_path__";

/// Takes the login shell's PATH when the daemon was not started from a
/// terminal, which already gave it. Called before any thread starts.
pub fn adopt() {
    if std::io::stdin().is_terminal() {
        return;
    }
    let Some(shell) = std::env::var_os("SHELL") else { return };
    if let Some(path) = of_shell(Path::new(&shell), Duration::from_secs(5)) {
        // SAFETY: no other thread is running yet.
        unsafe { std::env::set_var("PATH", path) };
    }
}

/// The PATH `shell` has once it has read the person's files, as a login and
/// interactive shell does; `None` if it gives none within `within`.
pub fn of_shell(shell: &Path, within: Duration) -> Option<String> {
    let script = format!("printf '{MARK}%s{MARK}' \"$PATH\"");
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        let _ = tx.send(out);
    });
    let out = rx.recv_timeout(within).ok();
    let _ = child.kill();
    let _ = child.wait();
    between(&out?)
}

/// What the shell printed between the marks: its rc files may print more.
fn between(out: &str) -> Option<String> {
    let (_, rest) = out.split_once(MARK)?;
    let (path, _) = rest.split_once(MARK)?;
    (!path.is_empty()).then(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_is_read_between_the_marks_whatever_the_shell_says_around_it() {
        let out = format!("Welcome!\n{MARK}/opt/homebrew/bin:/usr/bin{MARK}bye");
        assert_eq!(between(&out).as_deref(), Some("/opt/homebrew/bin:/usr/bin"));
        assert_eq!(between("no marks at all"), None);
        assert_eq!(between(&format!("{MARK}{MARK}")), None);
        assert_eq!(between(&format!("{MARK}/usr/bin")), None, "cut short");
    }

    #[test]
    fn a_shell_gives_its_path() {
        let path = of_shell(Path::new("/bin/sh"), Duration::from_secs(10)).unwrap();
        assert!(std::env::split_paths(&path).any(|d| d.join("sh").is_file()), "{path}");
    }

    #[test]
    fn no_shell_gives_none() {
        assert_eq!(of_shell(Path::new("/no/such/shell"), Duration::from_secs(1)), None);
    }
}
