//! Reading an Eval record off disk.
//!
//! What one means lives in [`weft_core::record`]; this opens the file and
//! hands the bytes over.

use std::path::Path;

use serde_json::Value;

pub use weft_core::record::*;

pub fn read(project_root: &Path, eval_id: &str) -> Option<Record> {
    let path = project_root.join(".fab7/rf/evals").join(eval_id).join("record.json");
    let bytes = std::fs::read(path).ok()?;
    Record::parse(&serde_json::from_slice::<Value>(&bytes).ok()?)
}
