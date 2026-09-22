//! The terminal client.
//!
//! It draws, and asks the daemon for everything else. It reaches no file and
//! runs no process — if it could, it would start re-deriving what the daemon
//! already decided, and a second client would disagree with it.

pub mod app;
pub mod client;
pub mod encode;
pub mod keys;
pub mod layout;
pub mod theme;
pub mod ui;

// The rules, for drawing what they decided.
pub use weft_core::{blocked, board, inject, ledger, offers, readiness, record, routing, sessions};
pub use weft_proto as protocol;
