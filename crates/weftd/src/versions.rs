//! Versions side by side: `install.sh` puts each Weft in
//! `~/.fab7/weft/versions/<version>/weft` and points `weft` at the newest, so
//! an update never replaces a program under a process running it.
//!
//! Every Weft process holds its own version's folder while it runs: a shared
//! lock on `<folder>/.held`, which the system lets go of when the process
//! ends, however it ends. A folder is removed only when no process holds it.

use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// The folder of each version, under Weft's own.
pub fn dir(weft: &Path) -> PathBuf {
    weft.join("versions")
}

/// `~/.fab7/weft/versions`.
pub fn here() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| dir(&PathBuf::from(h).join(".fab7/weft")))
}

/// The version's folder this program is in, when it is one: the program's
/// real path, links followed, inside `versions`.
pub fn own(versions: &Path, exe: &Path) -> Option<PathBuf> {
    let real = exe.canonicalize().ok()?;
    let versions = versions.canonicalize().ok()?;
    let folder = real.parent()?;
    (folder.parent()? == versions).then(|| folder.to_path_buf())
}

/// A version's folder, held while this lives.
pub struct Held(#[allow(dead_code)] File);

/// Hold `folder` for as long as the result lives (this process, as a rule).
pub fn hold(folder: &Path) -> std::io::Result<Held> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(folder.join(".held"))?;
    // SAFETY: a valid descriptor this function owns.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Held(file))
}

/// Hold this program's own version, when it runs from one.
pub fn hold_own() -> Option<Held> {
    let exe = std::env::current_exe().ok()?;
    hold(&own(&here()?, &exe)?).ok()
}

/// Remove every version but `keep` that no process holds; the ones removed.
pub fn prune(versions: &Path, keep: &[PathBuf]) -> Vec<PathBuf> {
    let keep: Vec<PathBuf> = keep.iter().filter_map(|k| k.canonicalize().ok()).collect();
    let Ok(entries) = std::fs::read_dir(versions) else { return Vec::new() };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let Ok(folder) = entry.path().canonicalize() else { continue };
        if !folder.is_dir() || keep.contains(&folder) {
            continue;
        }
        let held = folder.join(".held");
        let opened =
            std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(held);
        let Ok(file) = opened else { continue };
        // SAFETY: a valid descriptor this function owns.
        let free = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if free && std::fs::remove_dir_all(&folder).is_ok() {
            removed.push(folder);
        }
    }
    removed
}

/// The version `weft` on `PATH` points at: the newest installed.
pub fn newest(versions: &Path) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    let link = std::env::split_paths(&paths).map(|d| d.join("weft")).find(|p| p.exists())?;
    own(versions, &link)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("weft-versions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let versions = dir(&root);
        let (old, new) = (versions.join("0.2.0"), versions.join("0.2.1"));
        for v in [&old, &new] {
            std::fs::create_dir_all(v).unwrap();
            std::fs::write(v.join("weft"), "#!/bin/sh\n").unwrap();
        }
        (versions, old, new)
    }

    #[test]
    fn a_program_knows_its_version_through_the_link_it_was_started_by() {
        let (versions, old, _) = layout("own");
        let link = versions.parent().unwrap().join("weft");
        std::os::unix::fs::symlink(old.join("weft"), &link).unwrap();
        assert_eq!(own(&versions, &link), Some(old.canonicalize().unwrap()));
        assert_eq!(own(&versions, Path::new("/bin/sh")), None, "not one of Weft's");
        let _ = std::fs::remove_dir_all(versions.parent().unwrap());
    }

    /// An old version is removed only once nothing runs it; the one kept
    /// (this program's, and the newest) never is.
    #[test]
    fn an_old_version_is_removed_only_when_no_process_holds_it() {
        let (versions, old, new) = layout("prune");
        let held = hold(&old).unwrap();
        assert!(prune(&versions, std::slice::from_ref(&new)).is_empty(), "held: kept");
        assert!(old.join("weft").exists());
        drop(held);
        assert_eq!(prune(&versions, std::slice::from_ref(&new)).len(), 1);
        assert!(!old.exists() && new.join("weft").exists(), "the kept one stays");
        let _ = std::fs::remove_dir_all(versions.parent().unwrap());
    }

    /// Held by another process, as a window of the old version would.
    #[test]
    fn a_version_another_process_runs_is_kept() {
        let (versions, old, new) = layout("other");
        let mut other = std::process::Command::new("/usr/bin/env")
            .args(["perl", "-e"])
            .arg(
                "use Fcntl qw(:flock); open(my $f, '>>', $ARGV[0]) or die; \
                 flock($f, LOCK_SH) or die; print \"held\\n\"; $| = 1; sleep 30;",
            )
            .arg(old.join(".held"))
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(other.stdout.take().unwrap()),
            &mut line,
        )
        .unwrap();
        assert_eq!(line.trim(), "held");
        assert!(prune(&versions, std::slice::from_ref(&new)).is_empty());
        other.kill().unwrap();
        other.wait().unwrap();
        assert_eq!(prune(&versions, std::slice::from_ref(&new)).len(), 1, "it ended: removed");
        let _ = std::fs::remove_dir_all(versions.parent().unwrap());
    }
}
