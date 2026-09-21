//! RingFrame's core: the record, and the commands that write it.
//!
//! A port of the Python core at `ringframe` 0.0.5, module for module. The
//! Python tests came with it as the specification; where a choice here looks
//! odd, it is usually because 0.0.5 made it and something depends on it.

/// The Rust era starts at 0.1.0 (ADR-0013). `ringframe` 0.0.5 was the last of
/// the Python one, and stays where it is.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod ask;
pub mod cli;
pub mod config;
pub mod deltas;
pub mod digest;
pub mod evaluate;
pub mod ids;
pub mod output;
pub mod profiles;
pub mod schema;
pub mod seal;
pub mod sessions;
pub mod store;
pub mod workspace;

#[cfg(test)]
mod testing;
