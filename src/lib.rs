//! Weft: one interface for RingFrame, across your harnesses.
//!
//! Four crates:
//!
//! - [`weft_core`] — the rules. No files, no processes, no terminal.
//! - [`weft_proto`] — the wire, and the types on it.
//! - [`weftd`] — the daemon: panes, the record, the CLI.
//! - [`weft_tui`] — the terminal client.
//!
//! This crate is the binary that starts one of the last two.

pub use weft_core::{blocked, board, harness as harness_table, inject, offers};
pub use weft_proto as protocol;
pub use weft_tui::{app, client, encode, keys, layout, theme, ui};
pub use weftd::{
    acts, harness, ledger, pane, readiness, record, ringframe, routing, server, sessions,
};
