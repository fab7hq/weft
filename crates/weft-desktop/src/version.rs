//! The app's own version, held while it runs: each version of Weft lives in
//! `~/.fab7/weft/versions/<version>`, and a folder is removed only when no
//! process holds it, so an update never takes the app from under itself.

use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// A version's folder, held for as long as this lives.
pub struct Held(#[allow(dead_code)] std::fs::File);

/// Hold the version folder in `versions` that `exe` runs from, if it runs
/// from one.
pub fn hold(versions: &Path, exe: &Path) -> Option<Held> {
    let real = exe.canonicalize().ok()?;
    let versions = versions.canonicalize().ok()?;
    let folder = real.ancestors().skip(1).find(|f| f.parent() == Some(versions.as_path()))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(folder.join(".held"))
        .ok()?;
    unsafe extern "C" {
        fn flock(fd: i32, op: i32) -> i32;
    }
    const SHARED: i32 = 1;
    // SAFETY: a valid descriptor this function owns.
    (unsafe { flock(file.as_raw_fd(), SHARED) } == 0).then_some(Held(file))
}

/// Hold this program's own version, under `~/.fab7/weft/versions`.
pub fn hold_own() -> Option<Held> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    hold(&home.join(".fab7/weft/versions"), &std::env::current_exe().ok()?)
}
