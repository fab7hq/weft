//! `weft remote`: the agents on this machine, from another device
//! (remote.md).
//!
//! A client of the daemon's socket, like the desktop app, and the one part of Weft
//! that listens on a network port: a loopback one, which `cloudflared`
//! carries. Fab7's account service says who a device's person is; only this
//! machine admits a device. Nothing here types what the person did not ask
//! for, or runs a harness.

pub mod account;
pub mod agents;
pub mod conversation;
pub mod daemon;
pub mod devices;
pub mod health;
pub mod push;
pub mod run;
pub mod socket;
pub mod store;
pub mod tool;
pub mod tunnel;
pub mod view;

/// Run until told to stop, or the daemon goes.
pub fn run() -> anyhow::Result<()> {
    run::serve(run::Settings::here())
}
