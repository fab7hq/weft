//! Finding a supported harness on this machine.
//!
//! The table itself, and every rule about it, live in [`weft_core::harness`].
//! What is here is the two questions only a machine can answer: where this
//! harness keeps its configuration, and whether it is installed.

use std::path::PathBuf;

pub use weft_core::harness::*;

/// Asked of the machine, not of the table.
pub trait OnThisMachine {
    fn config_home(&self) -> PathBuf;
    fn on_path(&self) -> bool;
}

impl OnThisMachine for Harness {
    /// Where this harness keeps its configuration, honouring its own variable.
    fn config_home(&self) -> PathBuf {
        self.config_home_from(std::env::var_os(self.config_env), home())
    }
    fn on_path(&self) -> bool {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(self.program).is_file())
        })
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
