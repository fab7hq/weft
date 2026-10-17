//! How a shortcut is shown beside what it does.

/// The shortcut `key` with the system's command key.
pub fn hint(key: &str) -> String {
    format!("⌘{}", key.to_uppercase())
}
