//! Weft: one interface for RingFrame, across your harnesses.
//!
//! - [`weft_core`] — the rules. No files, no processes, no terminal.
//! - [`weft_proto`] — the wire, and the types on it.
//! - [`weftd`] — the daemon: panes, the record, the CLI.
//! - `weft-desktop` — the app, a client of the daemon.
//!
//! This crate is the command line: it opens the app and starts the daemon.

pub use weft_core::{board, harness as harness_table, inject, offers, projects, turns};
pub use weft_proto as protocol;
pub use weftd::{
    acts, harness, ledger, outside, pane, readiness, receipts, record, ringframe, routing, server,
};
