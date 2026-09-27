//! Finding a harness on this machine.
//!
//! What a harness is comes from its RingFrame profile, read by
//! [`crate::ringframe::harnesses`]; every rule about it lives in
//! [`weft_core::harness`].
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
        let set = self.config_env.as_ref().and_then(std::env::var_os);
        self.config_home_from(set, home())
    }
    fn on_path(&self) -> bool {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(&self.program).is_file())
        })
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
