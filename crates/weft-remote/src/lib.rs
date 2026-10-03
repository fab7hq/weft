//! `weft remote`: the agents on this machine, from another device
//! ([remote.md](../../../../plans/weft/spec/remote.md)).
//!
//! A client of the daemon's socket, like the TUI, and the one part of Weft
//! that listens on a network port: a loopback one, which `cloudflared`
//! carries. Fab7's account service says who a device's person is; only this
//! machine admits a device. Nothing here types what the person did not ask
//! for, or runs a harness.

pub mod agents;
pub mod daemon;
pub mod tunnel;

/// Run until told to stop.
pub fn run() -> anyhow::Result<()> {
    anyhow::bail!("weft remote: not built yet")
}
