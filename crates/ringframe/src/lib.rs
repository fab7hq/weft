//! RingFrame's core: the record, and the commands that write it.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod api;
pub mod ask;
pub mod changes;
pub mod checks;
pub mod cli;
pub mod compile;
pub mod config;
pub mod decisions;
pub mod deltas;
pub mod digest;
pub mod documents;
pub mod evaluate;
pub mod evidence;
pub mod ids;
pub mod knowledge;
pub mod observe;
pub mod output;
pub mod profiles;
pub mod read_check;
pub mod reads;
pub mod rules;
pub mod schema;
pub mod seal;
pub mod sessions;
pub mod snapshots;
pub mod store;
pub mod tasks;
mod untracked;
pub mod workspace;

#[cfg(test)]
mod testing;
