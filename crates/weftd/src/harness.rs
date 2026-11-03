//! Finding a harness on this machine.
//!
//! What a harness is comes from its Weft harness file, read here from
//! `~/.fab7/weft/harnesses/`; every rule about it lives in
//! [`weft_core::harness`]. What else is here is the two questions only a
//! machine can answer: where this harness keeps its configuration, and
//! whether it is installed.

use std::path::{Path, PathBuf};

pub use weft_core::harness::*;

/// Every harness file in `dir`, read tolerantly: a file that will not read is
/// left out and named in what comes back second. No directory, no harness.
pub fn installed(dir: &Path) -> (Harnesses, Vec<String>) {
    let mut files: Vec<(String, String)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .filter_map(|p| {
            let id = p.file_stem()?.to_str()?.to_string();
            Some((id, std::fs::read_to_string(&p).unwrap_or_default()))
        })
        .collect();
    files.sort();
    Harnesses::read(files)
}

/// Put the item that makes `h` name its session in its window's title
/// (`[window_title]`) into its configuration, keeping all else, before a pane
/// of it starts. Nothing when it has none or the item is there; a file that
/// does not read as the table expects is left as it is.
pub fn name_session_in_title(h: &Harness) {
    let Some(w) = &h.window_title else { return };
    let path = h.config_home().join(&w.file);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return,
    };
    let Some(added) = w.added(&text) else { return };
    let part = path.with_extension("weft.part");
    if path.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok())
        && std::fs::write(&part, added).is_ok()
    {
        let _ = std::fs::rename(&part, &path);
    }
}

/// Asked of the machine, not of the table.
pub trait OnThisMachine {
    fn config_home(&self) -> PathBuf;
    fn on_path(&self) -> bool;
}

impl OnThisMachine for Harness {
    /// Where this harness keeps its configuration, honouring its own variable.
    fn config_home(&self) -> PathBuf {
        let set = self.config_env.as_ref().and_then(std::env::var_os);
        self.config_home_from(set, home())
    }
    /// On `PATH`, or where the person said it is (an absolute path).
    fn on_path(&self) -> bool {
        if self.program.contains('/') {
            return runnable(Path::new(&self.program));
        }
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(&self.program).is_file())
        })
    }
}

/// A file that exists and may be run.
pub fn runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
