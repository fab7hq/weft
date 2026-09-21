//! The daemon: what Weft does, as opposed to what it decides.
//!
//! ADR-0007. One per machine, holding every project anyone opens. It owns the
//! pane processes, follows each project's record, runs the RingFrame CLI, and
//! holds what is waiting on a person. Every rule it applies comes from
//! [`weft_core`]; nothing is decided twice.

pub mod acts;
pub mod harness;
pub mod ledger;
pub mod pane;
pub mod readiness;
pub mod record;
pub mod ringframe;
pub mod routing;
pub mod server;
pub mod sessions;
