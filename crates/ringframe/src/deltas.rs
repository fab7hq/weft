//! Delta catalogs: host deltas keyed by (host, capability); practice deltas
//! keyed by classification.
//!
//! The CLI selects YAML rules; the model adapts them to the task. Deltas guide
//! the work but do not run checks.

use std::path::Path;

pub const SCHEMA: &str = "ringframe.deltas/1";
pub const DEFAULT_DOMAIN: &str = "software-development";

/// Create a project's empty base-domain override file; preserve existing
/// configuration.
pub fn initialize(root: &Path) -> std::io::Result<Vec<String>> {
    std::fs::create_dir_all(root)?;
    crate::workspace::set_private(root)?;
    let target = root.join("deltas").join("practices").join(format!("{DEFAULT_DOMAIN}.yaml"));
    std::fs::create_dir_all(target.parent().expect("a practices directory"))?;
    // Create it only if it is not there; an existing file is the person's.
    let _ = std::fs::OpenOptions::new().write(true).create_new(true).open(&target);
    let ignore = root.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n")?;
    }
    Ok(vec![target.to_string_lossy().to_string()])
}
