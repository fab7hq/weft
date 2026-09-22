//! RingFrame's core: the record, and the commands that write it.

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
