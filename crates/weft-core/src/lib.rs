//! Weft's rules: what the record means, and what may be done about it.
//!
//! This crate holds every rule and **nothing that does anything** — no
//! filesystem, no processes, no network, no terminal. That is what makes the
//! rules testable in milliseconds, and what lets a second client agree with
//! the first instead of re-deriving them.
//!
//! The shell that does reach those things lives above: it reads the files,
//! runs `ringframe`, owns the PTYs, and asks this crate what any of it means.

pub mod blocked;
pub mod board;
pub mod harness;
pub mod inject;
pub mod ledger;
pub mod offers;
pub mod readiness;
pub mod record;
pub mod routing;
pub mod sessions;
pub mod sync;
